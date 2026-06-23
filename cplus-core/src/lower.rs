//! Slice 4A.5 — `if let` / `guard let` lowering.
//!
//! Pattern-binding sugar over slice-3I `match`. The parser produces
//! `StmtKind::IfLet` / `StmtKind::GuardLet`; this pass:
//!
//! 1. Emits the slice-specific diagnostics:
//!    - E0347: irrefutable `if let` pattern (use plain `let`)
//!    - E0348: `guard let` else block must diverge (return / break / continue)
//!    - E0349: `guard let` else complement is not exhaustive with the
//!             success pattern (only fires when the user wrote an explicit
//!             `else |Pat|` form — without a complement we synthesize `_`
//!             which is trivially exhaustive)
//!    - E0350: `guard let` complement overlaps the success pattern
//!    - E0351: `guard let` requires the success pattern to bind at least
//!             one value (else it's just an `if let` with side effects)
//!    - E0352: multi-binding `guard let` patterns are deferred to a
//!             follow-up slice
//!
//! 2. Rewrites each `IfLet` / `GuardLet` statement in place to an
//!    equivalent form built from existing AST nodes (match expression for
//!    `if let`; `let` + match expression for `guard let`). Sema and codegen
//!    never see the original nodes — they hit a `panic!` arm in their
//!    statement matches.
//!
//! No codegen changes; the desugar produces match IR that slice 3I already
//! lowers. See `docs/design/phase4-pattern-let.md`.

use crate::ast::*;
use crate::diagnostics::{DiagCode, Diagnostic, LineMap, Severity};
use crate::lexer::Span;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Run the lowering pass over a merged Program. Mutates `prog` so all
/// `StmtKind::IfLet` / `StmtKind::GuardLet` nodes are replaced with
/// equivalent match-using forms.
///
/// v0.0.9 Phase 4: also validates module-scope `const` / `static`
/// initializers are literals (E0X30) and substitutes every use-site
/// reference to a const with the initializer expression. After this
/// pass returns, sema sees literal expressions where the user wrote a
/// const name — codegen never observes a const-name reference.
pub fn lower(prog: &mut Program, file: &PathBuf, src: &str) -> Vec<Diagnostic> {
    // Single-file entry: build a one-entry files map and delegate to the
    // multi-file path. Mirrors `sema::check` / `attrs::check`.
    let mut files: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
    files.insert(String::new(), (file.clone(), src.to_string()));
    lower_multi(prog, file, src, files)
}

/// Multi-file entry point. Mirrors `sema::check_multi` / `attrs::check_multi`.
///
/// GAP 3 (v0.0.19): the merged program carries items from several files, each
/// tagged with its `origin_file`. Diagnostics raised here (E0X30 on a bad
/// static/const initializer, the if-let/guard-let desugar errors, E0X36 on a
/// bad const array length) must render against the file the *item* came from,
/// not the entry file. Previously `lower` knew only the entry path + source, so
/// an error in an imported file pointed at the entry file (wrong file) and a
/// byte offset past the entry source's length (wrong / clamped line). Track
/// `current_file` per item and resolve spans through that file's `LineMap`.
pub fn lower_multi(
    prog: &mut Program,
    entry_file: &PathBuf,
    entry_src: &str,
    files: BTreeMap<String, (PathBuf, String)>,
) -> Vec<Diagnostic> {
    let mut cx = Lower::new(entry_file.clone(), entry_src, files);
    // v0.0.9 Phase 4: collect consts and validate initializers (both
    // const and static initializers must be literals). Done before the
    // per-item walk so the substitution pass sees a populated table.
    let const_values = cx.collect_consts_and_validate_inits(prog);
    // Collect free-fn / method parameters (names + defaults) up front so named
    // arguments can be reordered and omitted defaults spliced during the
    // per-item expression walk.
    cx.collect_call_params(prog);
    for it in &mut prog.items {
        cx.set_current_file(it.origin_file.as_deref());
        cx.lower_item(it);
    }
    cx.set_current_file(None);
    // v0.0.9 Phase 4: substitute every `Ident(qualified_const_name)`
    // use site with the const's initializer. Done after per-item
    // lowering so any pattern-let desugar already turned `if let` /
    // `guard let` bodies into walkable expression trees.
    cx.substitute_consts(prog, &const_values);
    // v0.0.13: fold `const`-name array lengths (`[T; N]`, `[v; N]`) into
    // literal `u32`s using the same const table. After this, every later pass
    // sees a plain length; `len_name` / `count_name` are cleared.
    cx.resolve_const_array_lengths(prog, &const_values);
    cx.diags
}

/// One parameter as seen by the named-argument / default-value lowering: its
/// name (the label) and its default value expression, if any.
#[derive(Clone)]
struct ParamInfo {
    name: String,
    default: Option<Expr>,
}

/// Where a lowered call's argument in one parameter position comes from.
#[derive(Clone, PartialEq)]
enum ArgSlot {
    /// The argument originally at this index in the (written-order) call.
    Arg(usize),
    /// The parameter's default value (spliced in because the call omitted it).
    Default,
}

fn param_info(p: &Param) -> ParamInfo {
    ParamInfo {
        name: p.name.name.clone(),
        default: p.default.as_deref().cloned(),
    }
}

struct Lower {
    entry_file: PathBuf,
    entry_src: String,
    entry_lm: LineMap,
    /// `origin_file` id -> (path, source, line map) for every project file.
    files: BTreeMap<String, (PathBuf, String, LineMap)>,
    /// The file the item currently being lowered came from, if tagged.
    current_file: Option<String>,
    diags: Vec<Diagnostic>,
    /// Parameters (name + default) per non-extern free function, keyed by fn
    /// name. Collected up front (across all files) and used to lower named
    /// arguments into positional order and to splice omitted defaults. Extern
    /// fns are absent.
    fn_params: std::collections::HashMap<String, Vec<ParamInfo>>,
    /// Parameters (receiver excluded) for every `impl` method, keyed by method
    /// name. A name may map to several overloads across types; for a `v.m(..)`
    /// call the labels / arity usually single one out (lower has no type info).
    method_params: std::collections::HashMap<String, Vec<Vec<ParamInfo>>>,
}

impl Lower {
    fn new(
        entry_file: PathBuf,
        entry_src: &str,
        files: BTreeMap<String, (PathBuf, String)>,
    ) -> Self {
        let entry_lm = LineMap::new(entry_src);
        let mut compiled = BTreeMap::new();
        for (id, (path, src)) in files {
            let lm = LineMap::new(&src);
            compiled.insert(id, (path, src, lm));
        }
        Self {
            entry_file,
            entry_src: entry_src.to_string(),
            entry_lm,
            files: compiled,
            current_file: None,
            diags: vec![],
            fn_params: std::collections::HashMap::new(),
            method_params: std::collections::HashMap::new(),
        }
    }

    /// Collect, across the whole (merged) program, the parameters (name +
    /// default) that named arguments are matched against and that omitted
    /// defaults are spliced from: every non-extern free function (by name) and
    /// every `impl` method (by name, receiver excluded — a name may have several
    /// overloads). Also validates default placement (trailing-only) and that
    /// `extern fn`s have none. Done up front so the per-item expression walk can
    /// lower `f(b: .., a: ..)` / `v.m(b:)` and fill omitted defaults.
    fn collect_call_params(&mut self, prog: &Program) {
        for it in &prog.items {
            match &it.kind {
                ItemKind::Function(f) => {
                    self.validate_param_defaults(&f.params, f.is_extern);
                    if !f.is_extern {
                        self.fn_params
                            .insert(f.name.name.clone(), f.params.iter().map(param_info).collect());
                    }
                }
                ItemKind::Impl(b) => {
                    for m in &b.methods {
                        self.validate_param_defaults(&m.params, false);
                        self.method_params
                            .entry(m.name.name.clone())
                            .or_default()
                            .push(m.params.iter().map(param_info).collect());
                    }
                }
                _ => {}
            }
        }
    }

    /// A default value must be trailing (no required parameter after one with a
    /// default), and an `extern fn` may not have defaults at all.
    fn validate_param_defaults(&mut self, params: &[Param], is_extern: bool) {
        let mut seen_default = false;
        for p in params {
            if p.default.is_some() {
                if is_extern {
                    self.err(
                        "E1008",
                        "an `extern fn` parameter cannot have a default value".to_string(),
                        p.span,
                    );
                }
                seen_default = true;
            } else if seen_default {
                self.err(
                    "E1007",
                    format!(
                        "required parameter `{}` cannot follow a parameter with a default value",
                        p.name.name
                    ),
                    p.span,
                );
            }
        }
    }

    /// True if a call to `callee` with `n_args` positional args might need a
    /// default spliced in — i.e. the callee is a known free fn / method with
    /// more parameters than arguments given. Used to gate the lowering so that
    /// exact-arity positional calls are left untouched.
    fn call_may_need_defaults(&self, callee: &Expr, n_args: usize) -> bool {
        match &callee.kind {
            ExprKind::Ident(name) => self.fn_params.get(name).is_some_and(|p| p.len() > n_args),
            ExprKind::Field { name, .. } => self
                .method_params
                .get(&name.name)
                .is_some_and(|cs| cs.iter().any(|p| p.len() > n_args)),
            _ => false,
        }
    }

    /// Lower a call that uses named arguments and/or omits defaulted ones into a
    /// plain positional call. The callee resolves to one or more candidate
    /// parameter lists (a free fn has one; a method may have several overloads).
    /// For each candidate the args/labels are matched to positions and omitted
    /// parameters take their defaults; if exactly one *distinct* successful
    /// arrangement results, the args are rebuilt into it and the labels cleared
    /// (so every later pass — and codegen — sees an ordinary positional call;
    /// evaluation order follows the lowered positional order). If none accept
    /// the call, the first concrete mismatch is reported. If several accept it
    /// *differently*, it is ambiguous without type info — the labels are left
    /// for sema's E1002.
    fn lower_named_call(
        &mut self,
        callee: &Expr,
        args: &mut Vec<Expr>,
        arg_labels: &mut Vec<Option<Ident>>,
        call_span: Span,
    ) {
        let candidates: Vec<Vec<ParamInfo>> = match &callee.kind {
            ExprKind::Ident(name) => match self.fn_params.get(name) {
                Some(p) => vec![p.clone()],
                None => return, // unknown free fn / fn-pointer local — sema handles
            },
            ExprKind::Field { name, .. } => match self.method_params.get(&name.name) {
                Some(c) => c.clone(),
                None => return, // unknown method — sema handles
            },
            _ => return, // assoc (Path) etc. — sema's E1002 guard reports it
        };
        let mut results: Vec<(usize, Vec<ArgSlot>)> = Vec::new();
        let mut first_err: Option<(&'static str, String, Span)> = None;
        for (ci, params) in candidates.iter().enumerate() {
            match Self::match_call(params, args, arg_labels, call_span) {
                Ok(slots) => {
                    if !results.iter().any(|(_, s)| *s == slots) {
                        results.push((ci, slots));
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        if results.len() == 1 {
            let (ci, slots) = &results[0];
            Self::apply_slots(&candidates[*ci], args, arg_labels, slots);
        } else if results.is_empty() {
            if let Some((code, msg, span)) = first_err {
                self.err(code, msg, span);
            }
            // Consumed (errored): clear labels so sema doesn't double-report.
            arg_labels.clear();
        }
        // results.len() > 1: ambiguous without types — leave labels for sema E1002.
    }

    /// Match a call's `args`/`labels` against one parameter list. Returns, per
    /// parameter position, where its value comes from (`Arg(i)` or `Default`),
    /// or `Err((code, msg, span))` on a mismatch. `labels` is either empty (all
    /// positional) or the same length as `args`.
    fn match_call(
        params: &[ParamInfo],
        args: &[Expr],
        labels: &[Option<Ident>],
        call_span: Span,
    ) -> Result<Vec<ArgSlot>, (&'static str, String, Span)> {
        let n = params.len();
        let mut slots: Vec<Option<usize>> = vec![None; n];
        let mut seen_named = false;
        let mut next_pos = 0usize;
        for arg_idx in 0..args.len() {
            match labels.get(arg_idx).and_then(|l| l.as_ref()) {
                None => {
                    if seen_named {
                        return Err((
                            "E1004",
                            "a positional argument cannot follow a named argument".to_string(),
                            args[arg_idx].span,
                        ));
                    }
                    if next_pos >= n {
                        return Err((
                            "E0308",
                            format!("too many arguments: this call expects {n}"),
                            args[arg_idx].span,
                        ));
                    }
                    slots[next_pos] = Some(arg_idx);
                    next_pos += 1;
                }
                Some(lbl) => {
                    seen_named = true;
                    match params.iter().position(|p| p.name == lbl.name) {
                        None => {
                            return Err((
                                "E1005",
                                format!("unknown argument label `{}`", lbl.name),
                                lbl.span,
                            ))
                        }
                        Some(pos) => {
                            if slots[pos].is_some() {
                                return Err((
                                    "E1006",
                                    format!("argument `{}` is provided more than once", lbl.name),
                                    lbl.span,
                                ));
                            }
                            slots[pos] = Some(arg_idx);
                        }
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(n);
        for (pos, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(ai) => out.push(ArgSlot::Arg(ai)),
                None => {
                    if params[pos].default.is_some() {
                        out.push(ArgSlot::Default);
                    } else {
                        return Err((
                            "E0308",
                            format!("missing argument for parameter `{}`", params[pos].name),
                            call_span,
                        ));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Rebuild `args` from `slots` (each position takes an original arg or the
    /// parameter's default) and clear the labels.
    fn apply_slots(
        params: &[ParamInfo],
        args: &mut Vec<Expr>,
        arg_labels: &mut Vec<Option<Ident>>,
        slots: &[ArgSlot],
    ) {
        let mut taken: Vec<Option<Expr>> = std::mem::take(args).into_iter().map(Some).collect();
        let mut out: Vec<Expr> = Vec::with_capacity(slots.len());
        for (pos, slot) in slots.iter().enumerate() {
            match slot {
                ArgSlot::Arg(ai) => {
                    out.push(taken[*ai].take().expect("each arg is used at most once"))
                }
                ArgSlot::Default => out.push(
                    params[pos]
                        .default
                        .clone()
                        .expect("Default slot only for a parameter that has one"),
                ),
            }
        }
        *args = out;
        arg_labels.clear();
    }

    fn set_current_file(&mut self, id: Option<&str>) {
        self.current_file = id.map(String::from);
    }

    /// (path, source, LineMap) a span renders against. v0.0.22
    /// file-aware: a stamped span routes itself; the 0 sentinel falls
    /// back to the current item's file, then the entry file.
    fn file_ctx_for(&self, span: Span) -> (&PathBuf, &str, &LineMap) {
        if span.file != 0 {
            if let Some(fid) = crate::lexer::interned_file(span.file) {
                if let Some((path, src, lm)) = self.files.get(&fid) {
                    return (path, src.as_str(), lm);
                }
            }
        }
        if let Some(id) = self.current_file.as_deref() {
            if let Some((path, src, lm)) = self.files.get(id) {
                return (path, src.as_str(), lm);
            }
        }
        (&self.entry_file, self.entry_src.as_str(), &self.entry_lm)
    }

    fn err(&mut self, code: &'static str, message: String, span: Span) {
        let primary = {
            let (path, src, lm) = self.file_ctx_for(span);
            lm.span(path, span, src)
        };
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode(code),
            message,
            primary,
            labels: vec![],
            notes: vec![],
            suggestions: vec![],
        });
    }

    fn lower_item(&mut self, it: &mut Item) {
        match &mut it.kind {
            ItemKind::Function(f) => self.lower_block(&mut f.body),
            ItemKind::Impl(b) => {
                for m in &mut b.methods {
                    self.lower_block(&mut m.body);
                }
            }
            // Slice 7GEN.3: interface declarations have no bodies to
            // lower (method signatures only); pass through unchanged.
            ItemKind::Struct(_)
            | ItemKind::Enum(_)
            | ItemKind::Interface(_)
            | ItemKind::TypeAlias(_) => {}
            // v0.0.9 Phase 4: const/static initializers are sema-checked
            // for the literal-only rule. The per-item lowering pass
            // doesn't transform them. Cross-program const substitution
            // runs in `substitute_consts` (see end of `lower`), after
            // every item's body has been lowered.
            ItemKind::Const(_) | ItemKind::Static(_) => {}
            // v0.0.15: module-scope `#asm("...")` has no body or expressions
            // to lower — raw assembly text passes through untouched.
            ItemKind::ModuleAsm(_) => {}
        }
    }

    fn lower_block(&mut self, b: &mut Block) {
        for s in &mut b.stmts {
            self.lower_stmt(s);
        }
        if let Some(tail) = &mut b.tail {
            self.lower_expr(tail);
        }
    }

    fn lower_stmt(&mut self, s: &mut Stmt) {
        // Walk *into* `if let` / `guard let` first so any nested
        // pattern-lets in the bodies are rewritten before we rewrite the
        // outer one. After the recursion, take the outer node and replace
        // it with its match-using equivalent.
        match &mut s.kind {
            StmtKind::Let { init, .. } => {
                if let Some(e) = init {
                    self.lower_expr(e);
                }
            }
            StmtKind::LetDestructure { init, .. } => self.lower_expr(init),
            StmtKind::Return(opt) => {
                if let Some(e) = opt {
                    self.lower_expr(e);
                }
            }
            StmtKind::While { cond, body, .. } => {
                self.lower_expr(cond);
                self.lower_block(body);
            }
            StmtKind::For(fl, _) => match fl {
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(init) = init {
                        self.lower_stmt(init);
                    }
                    if let Some(c) = cond {
                        self.lower_expr(c);
                    }
                    for u in update {
                        self.lower_expr(u);
                    }
                    self.lower_block(body);
                }
                ForLoop::Range { iter, body, .. } => {
                    self.lower_expr(iter);
                    self.lower_block(body);
                }
            },
            StmtKind::Expr(e) => self.lower_expr(e),
            StmtKind::Defer(e) => self.lower_expr(e),
            StmtKind::IfLet {
                body,
                else_body,
                scrutinee,
                ..
            } => {
                self.lower_expr(scrutinee);
                self.lower_block(body);
                if let Some(eb) = else_body {
                    self.lower_block(eb);
                }
            }
            StmtKind::GuardLet {
                scrutinee,
                else_body,
                ..
            } => {
                self.lower_expr(scrutinee);
                self.lower_block(else_body);
            }
            StmtKind::Break | StmtKind::Continue => {
                // Leaf control-flow markers — nothing to recurse into.
            }
            StmtKind::Assert(e) => self.lower_expr(e),
            StmtKind::Loop(body, _) => {
                self.lower_block(body);
            }
            StmtKind::WhileLet {
                scrutinee, body, ..
            } => {
                self.lower_expr(scrutinee);
                self.lower_block(body);
            }
        }
        // Now rewrite the outer node, if it's an if-let / guard-let.
        let stolen = std::mem::replace(
            &mut s.kind,
            StmtKind::Expr(Expr {
                kind: ExprKind::BoolLit(false),
                span: s.span,
            }),
        );
        match stolen {
            StmtKind::IfLet {
                pattern,
                scrutinee,
                body,
                else_body,
            } => {
                s.kind = self.lower_if_let(pattern, scrutinee, body, else_body, s.span);
            }
            StmtKind::GuardLet {
                pattern,
                scrutinee,
                complement,
                else_body,
            } => {
                s.kind = self.lower_guard_let(pattern, scrutinee, complement, else_body, s.span);
            }
            StmtKind::WhileLet {
                pattern,
                scrutinee,
                body,
            } => {
                s.kind = self.lower_while_let(pattern, scrutinee, body, s.span);
            }
            other => {
                s.kind = other;
            }
        }
    }

    fn lower_expr(&mut self, e: &mut Expr) {
        // v0.0.22 DSL.2: desugar builder blocks to the ordinary
        // `Builder::new`/`add`/`finish` block. Multi-file projects
        // already desugared during the resolver's rewrite walk (the
        // synthesized `ctx::Builder::new()` path needs alias rewriting);
        // this covers paths that skip the resolver, e.g. single-file
        // mode. Either way, sema and every later pass see only ordinary
        // AST — the same invariant the pattern-let desugar maintains.
        if matches!(e.kind, ExprKind::BuilderBlock { .. }) {
            desugar_builder_block(e);
            self.lower_expr(e);
            return;
        }
        let espan = e.span;
        match &mut e.kind {
            ExprKind::IntLit(..)
            | ExprKind::FloatLit(..)
            | ExprKind::BoolLit(_)
            | ExprKind::StrLit(_)
            | ExprKind::CStrLit(_)
            | ExprKind::IncludeBytes { .. }
            | ExprKind::IncludeStr { .. }
            | ExprKind::EnvVar { .. }
            | ExprKind::Ident(_) => {}
            ExprKind::Intrinsic { args, .. } => {
                for a in args {
                    self.lower_expr(a);
                }
            }
            ExprKind::Asm { operands, .. } => {
                for op in operands {
                    self.lower_expr(&mut op.value);
                }
            }
            ExprKind::InterpStr { parts } => {
                for p in parts {
                    if let crate::ast::InterpStrPart::Expr(e) = p {
                        self.lower_expr(e);
                    }
                }
            }
            ExprKind::Block(b) => self.lower_block(b),
            ExprKind::Await(inner) => self.lower_expr(inner),
            ExprKind::Yield(inner) => self.lower_expr(inner),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.lower_expr(cond);
                self.lower_block(then);
                if let Some(eb) = else_branch {
                    self.lower_expr(eb);
                }
            }
            ExprKind::Call {
                callee,
                args,
                arg_labels,
                ..
            } => {
                self.lower_expr(callee);
                for a in args.iter_mut() {
                    self.lower_expr(a);
                }
                // Lower a call that uses named arguments and/or omits defaulted
                // parameters into a plain positional call. Exact-arity, unlabeled
                // calls are left untouched. Genuinely-ambiguous method overloads
                // keep their labels and are reported by sema (E1002).
                let labeled = arg_labels.iter().any(|l| l.is_some());
                if labeled || self.call_may_need_defaults(callee, args.len()) {
                    self.lower_named_call(callee, args, arg_labels, espan);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.lower_expr(lhs);
                self.lower_expr(rhs);
            }
            ExprKind::Unary { operand, .. } => self.lower_expr(operand),
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.lower_expr(s);
                }
                if let Some(en) = end {
                    self.lower_expr(en);
                }
            }
            ExprKind::Assign { target, value, .. } => {
                self.lower_expr(target);
                self.lower_expr(value);
            }
            ExprKind::Cast { expr, .. } => self.lower_expr(expr),
            ExprKind::Path { .. } => {}
            ExprKind::StructLit { fields, .. }
            | ExprKind::InferredStructLit { fields }
            | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    self.lower_expr(&mut f.value);
                }
            }
            ExprKind::Field { receiver, .. } => self.lower_expr(receiver),
            ExprKind::ArrayFill { fill, .. } => self.lower_expr(fill),
            ExprKind::ArrayLit { elements }
            | ExprKind::GenericEnumCall { args: elements, .. }
            | ExprKind::TupleLit { elements } => {
                for el in elements {
                    self.lower_expr(el);
                }
            }
            ExprKind::Index { receiver, index } => {
                self.lower_expr(receiver);
                self.lower_expr(index);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.lower_expr(scrutinee);
                for a in arms {
                    self.lower_expr(&mut a.body);
                }
            }
            // Handled by the pre-check above; never reached.
            ExprKind::BuilderBlock { .. } => {
                unreachable!("BuilderBlock handled in lower_expr pre-check")
            }
        }
    }

    /// `if let PAT = E { B }` →  `match E { PAT => { B; }, _ => {} }`
    /// `if let PAT = E { B } else { B2 }` → `match E { PAT => { B; }, _ => { B2; } }`
    fn lower_if_let(
        &mut self,
        pattern: Pattern,
        scrutinee: Expr,
        mut body: Block,
        else_body: Option<Block>,
        stmt_span: Span,
    ) -> StmtKind {
        // E0347: pattern must be refutable. A bare binding or wildcard is
        // irrefutable — `if let x = E { ... }` is just `let x = E;` plus
        // some scope confusion. Variant patterns are refutable in C+
        // because every `enum` has ≥ 1 variant and a Variant pattern
        // names exactly one.
        if !is_refutable(&pattern) {
            self.err(
                "E0347",
                "`if let` pattern is irrefutable; use `let` instead".to_string(),
                pattern.span,
            );
        }
        // Normalize both arm bodies to unit-valued blocks so the synthetic
        // match's two arms agree on type (statement-position).
        body = into_unit_block(body);
        let else_blk = match else_body {
            Some(b) => into_unit_block(b),
            None => Block {
                stmts: vec![],
                tail: None,
                span: stmt_span,
            },
        };
        let success_arm = MatchArm {
            pattern,
            body: Expr {
                kind: ExprKind::Block(body.clone()),
                span: body.span,
            },
            span: body.span,
        };
        let else_arm_span = else_blk.span;
        let fallthrough_arm = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Wildcard,
                span: else_arm_span,
            },
            body: Expr {
                kind: ExprKind::Block(else_blk.clone()),
                span: else_arm_span,
            },
            span: else_arm_span,
        };
        let match_expr = Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms: vec![success_arm, fallthrough_arm],
            },
            span: stmt_span,
        };
        StmtKind::Expr(match_expr)
    }

    /// `guard let PAT = E else { ELSE };`
    ///   → `let X = match E { PAT => X, _ => { ELSE } };`
    /// `guard let PAT = E else |COMP| { ELSE };`
    ///   → `let X = match E { PAT => X, COMP => { ELSE } };`
    /// (where `X` is the single binding extracted from `PAT`.)
    fn lower_guard_let(
        &mut self,
        pattern: Pattern,
        scrutinee: Expr,
        complement: Option<Pattern>,
        else_body: Block,
        stmt_span: Span,
    ) -> StmtKind {
        // E0348: the else block must diverge.
        if !block_diverges(&else_body) {
            self.err(
                "E0348",
                "`guard let` else body must diverge (every path must `return`)".to_string(),
                else_body.span,
            );
        }

        // E0351 / E0352: single-binding constraint. Collect binding names
        // from the pattern.
        let bindings = collect_pattern_bindings(&pattern);
        if bindings.is_empty() {
            self.err(
                "E0351",
                "`guard let` requires the pattern to bind at least one value; use `if let` for inspection-only".to_string(),
                pattern.span,
            );
            return placeholder_stmt(stmt_span);
        }
        if bindings.len() > 1 {
            self.err(
                "E0352",
                "multi-binding `guard let` patterns are not yet supported; use one `guard let` per binding".to_string(),
                pattern.span,
            );
            return placeholder_stmt(stmt_span);
        }
        let extracted = bindings.into_iter().next().unwrap();

        // E0349 / E0350: complement (if user wrote `else |Pat|`) must
        // exhaustively cover the scrutinee together with the success
        // pattern AND must not overlap it. Without a complement we
        // synthesize `_` which is trivially exhaustive and disjoint from
        // any non-wildcard pattern.
        let (else_arm_pattern, else_arm_span) = match complement {
            Some(cp) => {
                self.check_complement(&pattern, &cp);
                let sp = cp.span;
                (cp, sp)
            }
            None => (
                Pattern {
                    kind: PatternKind::Wildcard,
                    span: else_body.span,
                },
                else_body.span,
            ),
        };

        // Build the match. Success arm body is just the bound identifier;
        // the pattern's binding scopes it.
        let success_arm = MatchArm {
            pattern: pattern.clone(),
            body: Expr {
                kind: ExprKind::Ident(extracted.name.clone()),
                span: extracted.span,
            },
            span: pattern.span,
        };
        let else_arm = MatchArm {
            pattern: else_arm_pattern,
            body: Expr {
                kind: ExprKind::Block(else_body.clone()),
                span: else_body.span,
            },
            span: else_arm_span,
        };
        let match_expr = Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms: vec![success_arm, else_arm],
            },
            span: stmt_span,
        };

        StmtKind::Let {
            mutable: false,
            name: extracted,
            ty: None,
            init: Some(match_expr),
        }
    }

    /// `while let PAT = E { BODY }`
    ///   →  `loop { match E { PAT => { BODY; () }, _ => break, } }`
    ///
    /// Refutability of PAT is checked (E0347 — same as `if let`). The
    /// fallback arm's `break` statement is what makes the loop
    /// terminate; codegen sees an ordinary `loop` + `match` after
    /// rewriting.
    fn lower_while_let(
        &mut self,
        pattern: Pattern,
        scrutinee: Expr,
        body: Block,
        stmt_span: Span,
    ) -> StmtKind {
        if !is_refutable(&pattern) {
            self.err(
                "E0347",
                "`while let` pattern is irrefutable; use `loop` (or rewrite without `let`) instead"
                    .to_string(),
                pattern.span,
            );
        }
        // Normalize the body to unit-typed (drop any tail expression
        // value) so the success and fallback arms both have type unit.
        let body_block = into_unit_block(body);
        let body_span = body_block.span;

        // Success arm: run body.
        let success_arm = MatchArm {
            pattern,
            body: Expr {
                kind: ExprKind::Block(body_block.clone()),
                span: body_span,
            },
            span: body_span,
        };

        // Fallback arm: `_ => break,` — a single break stmt inside a unit block.
        let fallback_block = Block {
            stmts: vec![Stmt {
                kind: StmtKind::Break,
                span: stmt_span,
            }],
            tail: None,
            span: stmt_span,
        };
        let fallback_arm = MatchArm {
            pattern: Pattern {
                kind: PatternKind::Wildcard,
                span: stmt_span,
            },
            body: Expr {
                kind: ExprKind::Block(fallback_block),
                span: stmt_span,
            },
            span: stmt_span,
        };

        let match_expr = Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms: vec![success_arm, fallback_arm],
            },
            span: stmt_span,
        };
        let loop_body = Block {
            stmts: vec![Stmt {
                kind: StmtKind::Expr(match_expr),
                span: stmt_span,
            }],
            tail: None,
            span: stmt_span,
        };
        StmtKind::Loop(loop_body, Vec::new())
    }

    fn check_complement(&mut self, success: &Pattern, complement: &Pattern) {
        // The complement can always be a catch-all (wildcard / binding) —
        // that is trivially exhaustive (together with the success pattern)
        // and trivially disjoint, so accept and return.
        match &complement.kind {
            PatternKind::Wildcard | PatternKind::Binding(_) => return,
            PatternKind::Variant { .. } => {}
        }
        // Otherwise: both patterns must be Variant. Reject overlap if they
        // reference the same enum + same variant.
        let (
            PatternKind::Variant {
                enum_name: s_enum,
                variant_name: s_var,
                ..
            },
            PatternKind::Variant {
                enum_name: c_enum,
                variant_name: c_var,
                ..
            },
        ) = (&success.kind, &complement.kind)
        else {
            // Success is wildcard/binding and complement is a Variant — the
            // success pattern is irrefutable (E0347 already fired) and the
            // complement is unreachable. No further check needed.
            return;
        };
        if s_enum.name == c_enum.name && s_var.name == c_var.name {
            self.err(
                "E0350",
                format!(
                    "complement pattern `{}::{}` overlaps the success pattern",
                    c_enum.name, c_var.name,
                ),
                complement.span,
            );
        }
        // Exhaustiveness against the full enum cannot be proven without
        // sema's enum table here. We leave the deep check to slice 4B/4C
        // when the lowering pass gets access to a sema context; in the
        // meantime the synthesized match runs through slice-3I
        // exhaustiveness check which will catch missing variants there
        // (sema's E0343 instead of E0349). Accept E0343 as the surface
        // error until the dedicated check moves in.
    }

    // ---- v0.0.9 Phase 4: const + static literal-only check + const substitution ----

    /// Walk the program's items, validating that every `const` and
    /// `static` initializer is a literal (E0X30). Returns a map from
    /// qualified const name → (initializer expression, declared type)
    /// for the substitution pass to consume.
    ///
    /// The declared type is paired in so the substitution can wrap the
    /// literal in a `Cast { expr, ty }`. Without the cast, an
    /// unsuffixed literal `176` substituted into a binary-op operand
    /// position defaults to `i32` per sema's literal-inference rule —
    /// which then mismatches if the other operand is `usize` /
    /// anything else. The cast pins the type at the substitution site
    /// so the const's declared type flows through every use unchanged.
    fn collect_consts_and_validate_inits(
        &mut self,
        prog: &Program,
    ) -> std::collections::HashMap<String, (Expr, Type)> {
        let mut consts: std::collections::HashMap<String, (Expr, Type)> =
            std::collections::HashMap::new();
        for item in &prog.items {
            // GAP 3: an E0X30 on a bad initializer must point at the file the
            // const/static was declared in, not always the entry file.
            self.set_current_file(item.origin_file.as_deref());
            match &item.kind {
                ItemKind::Const(c) => {
                    if !is_const_initializer(&c.value) {
                        self.err(
                            "E0X30",
                            "const initializer must be a literal (integer, float, bool, string, unary-negated numeric literal, or `#zero::[T]()`)".to_string(),
                            c.value.span,
                        );
                        continue;
                    }
                    consts.insert(c.name.name.clone(), (c.value.clone(), c.ty.clone()));
                }
                ItemKind::Static(s) => {
                    if !is_static_initializer(&s.value) {
                        self.err(
                            "E0X30",
                            "static initializer must be a literal (integer, float, bool, string, unary-negated numeric literal), `#zero::[T]()`, an array literal/fill, or a (non-generic) struct literal of such".to_string(),
                            s.value.span,
                        );
                    }
                }
                _ => {}
            }
        }
        consts
    }

    /// Walk every fn / method body in the program and replace each
    /// `ExprKind::Ident(name)` whose name matches a const in `consts`
    /// with a clone of the const's initializer expression. By the time
    /// this pass returns, no const-name reference survives in any
    /// expression position — sema sees only literals.
    fn substitute_consts(
        &self,
        prog: &mut Program,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        if consts.is_empty() {
            return;
        }
        for item in &mut prog.items {
            match &mut item.kind {
                ItemKind::Function(f) => subst_block(&mut f.body, consts),
                ItemKind::Impl(b) => {
                    for m in &mut b.methods {
                        subst_block(&mut m.body, consts);
                    }
                }
                ItemKind::Struct(_)
                | ItemKind::Enum(_)
                | ItemKind::Interface(_)
                | ItemKind::TypeAlias(_)
                | ItemKind::Const(_)
                | ItemKind::Static(_)
                | ItemKind::ModuleAsm(_) => {}
            }
        }
    }

    // ---- v0.0.13: const-eval for array lengths ----

    /// Walk every type and expression in the program, folding `const`-name
    /// array lengths into literal `u32`s. `[T; N]` (type position) and
    /// `[v; N]` (fill expression) where `N` is a non-negative integer `const`
    /// name are resolved against `consts` (the same table the substitution
    /// pass uses); unknown names, non-integer consts, and overflow fire
    /// **E0X36**. After this pass `len_name` / `count_name` are `None`.
    fn resolve_const_array_lengths(
        &mut self,
        prog: &mut Program,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        for item in &mut prog.items {
            // GAP 3: an E0X36 on a bad const array length renders against the
            // file the type/expression was written in.
            self.set_current_file(item.origin_file.as_deref());
            match &mut item.kind {
                ItemKind::Function(f) => {
                    for p in &mut f.params {
                        self.resolve_lens_in_type(&mut p.ty, consts);
                    }
                    if let Some(rt) = &mut f.return_type {
                        self.resolve_lens_in_type(rt, consts);
                    }
                    self.resolve_lens_in_block(&mut f.body, consts);
                }
                ItemKind::Impl(b) => {
                    for m in &mut b.methods {
                        for p in &mut m.params {
                            self.resolve_lens_in_type(&mut p.ty, consts);
                        }
                        if let Some(rt) = &mut m.return_type {
                            self.resolve_lens_in_type(rt, consts);
                        }
                        self.resolve_lens_in_block(&mut m.body, consts);
                    }
                }
                ItemKind::Struct(s) => {
                    for fld in &mut s.fields {
                        self.resolve_lens_in_type(&mut fld.ty, consts);
                    }
                }
                ItemKind::Enum(e) => {
                    for v in &mut e.variants {
                        for t in &mut v.payload {
                            self.resolve_lens_in_type(t, consts);
                        }
                    }
                }
                ItemKind::Interface(i) => {
                    for m in &mut i.methods {
                        for p in &mut m.params {
                            self.resolve_lens_in_type(&mut p.ty, consts);
                        }
                        if let Some(rt) = &mut m.return_type {
                            self.resolve_lens_in_type(rt, consts);
                        }
                    }
                }
                ItemKind::TypeAlias(a) => self.resolve_lens_in_type(&mut a.target, consts),
                ItemKind::Const(c) => {
                    self.resolve_lens_in_type(&mut c.ty, consts);
                    self.resolve_lens_in_expr(&mut c.value, consts);
                }
                ItemKind::Static(s) => {
                    self.resolve_lens_in_type(&mut s.ty, consts);
                    self.resolve_lens_in_expr(&mut s.value, consts);
                }
                // v0.0.15: module-scope `#asm("...")` has no types or
                // expressions carrying `const`-length lenses — nothing to do.
                ItemKind::ModuleAsm(_) => {}
            }
        }
    }

    /// Resolve a single `const`-name length to a `u32`, emitting E0X36 on a
    /// name that is not a usable non-negative integer `const`.
    fn resolve_one_len(
        &mut self,
        name: &str,
        span: Span,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) -> u32 {
        match consts.get(name) {
            None => {
                self.err(
                    "E0X36",
                    format!(
                        "array length `{name}` is not a known `const`; use an integer literal or a `const` (with a non-negative integer literal initializer) in scope"
                    ),
                    span,
                );
                0
            }
            Some((init, _)) => match &init.kind {
                ExprKind::IntLit(v, _) if *v <= u32::MAX as u64 => *v as u32,
                ExprKind::IntLit(_, _) => {
                    self.err(
                        "E0X36",
                        format!("array length `const {name}` exceeds the u32 maximum"),
                        span,
                    );
                    0
                }
                _ => {
                    self.err(
                        "E0X36",
                        format!(
                            "array length `const {name}` must be a non-negative integer literal"
                        ),
                        span,
                    );
                    0
                }
            },
        }
    }

    fn resolve_lens_in_type(
        &mut self,
        t: &mut Type,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        let span = t.span;
        match &mut t.kind {
            TypeKind::Array {
                elem,
                len,
                len_name,
            } => {
                if let Some(name) = len_name.take() {
                    *len = self.resolve_one_len(&name, span, consts);
                }
                self.resolve_lens_in_type(elem, consts);
            }
            TypeKind::Borrowed { inner, .. } => self.resolve_lens_in_type(inner, consts),
            TypeKind::RawPtr(inner) => self.resolve_lens_in_type(inner, consts),
            TypeKind::Slice(inner) => self.resolve_lens_in_type(inner, consts),
            TypeKind::FnPtr {
                params,
                return_type,
                ..
            } => {
                for p in params {
                    self.resolve_lens_in_type(p, consts);
                }
                if let Some(rt) = return_type {
                    self.resolve_lens_in_type(rt, consts);
                }
            }
            TypeKind::Generic { args, .. } => {
                for a in args {
                    self.resolve_lens_in_type(a, consts);
                }
            }
            TypeKind::Tuple(elems) => {
                for e in elems {
                    self.resolve_lens_in_type(e, consts);
                }
            }
            TypeKind::Path(_) => {}
        }
    }

    fn resolve_lens_in_block(
        &mut self,
        b: &mut Block,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        for s in &mut b.stmts {
            self.resolve_lens_in_stmt(s, consts);
        }
        if let Some(t) = &mut b.tail {
            self.resolve_lens_in_expr(t, consts);
        }
    }

    fn resolve_lens_in_stmt(
        &mut self,
        s: &mut Stmt,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        match &mut s.kind {
            StmtKind::Let { ty, init, .. } => {
                if let Some(t) = ty {
                    self.resolve_lens_in_type(t, consts);
                }
                if let Some(e) = init {
                    self.resolve_lens_in_expr(e, consts);
                }
            }
            StmtKind::LetDestructure { init, .. } => self.resolve_lens_in_expr(init, consts),
            StmtKind::Return(opt) => {
                if let Some(e) = opt {
                    self.resolve_lens_in_expr(e, consts);
                }
            }
            StmtKind::While { cond, body, .. } => {
                self.resolve_lens_in_expr(cond, consts);
                self.resolve_lens_in_block(body, consts);
            }
            StmtKind::Loop(b, _) => self.resolve_lens_in_block(b, consts),
            StmtKind::For(fl, _) => match fl {
                ForLoop::Range { iter, body, .. } => {
                    self.resolve_lens_in_expr(iter, consts);
                    self.resolve_lens_in_block(body, consts);
                }
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(i) = init {
                        self.resolve_lens_in_stmt(i, consts);
                    }
                    if let Some(c) = cond {
                        self.resolve_lens_in_expr(c, consts);
                    }
                    for u in update {
                        self.resolve_lens_in_expr(u, consts);
                    }
                    self.resolve_lens_in_block(body, consts);
                }
            },
            StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => {
                self.resolve_lens_in_expr(e, consts)
            }
            StmtKind::IfLet {
                scrutinee,
                body,
                else_body,
                ..
            } => {
                self.resolve_lens_in_expr(scrutinee, consts);
                self.resolve_lens_in_block(body, consts);
                if let Some(b) = else_body {
                    self.resolve_lens_in_block(b, consts);
                }
            }
            StmtKind::GuardLet {
                scrutinee,
                else_body,
                ..
            } => {
                self.resolve_lens_in_expr(scrutinee, consts);
                self.resolve_lens_in_block(else_body, consts);
            }
            StmtKind::WhileLet {
                scrutinee, body, ..
            } => {
                self.resolve_lens_in_expr(scrutinee, consts);
                self.resolve_lens_in_block(body, consts);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn resolve_lens_in_expr(
        &mut self,
        e: &mut Expr,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        let span = e.span;
        match &mut e.kind {
            ExprKind::ArrayFill {
                fill,
                count,
                count_name,
            } => {
                if let Some(name) = count_name.take() {
                    *count = self.resolve_one_len(&name, span, consts);
                }
                self.resolve_lens_in_expr(fill, consts);
            }
            ExprKind::Cast { expr, ty } => {
                self.resolve_lens_in_expr(expr, consts);
                self.resolve_lens_in_type(ty, consts);
            }
            ExprKind::Call {
                callee,
                args,
                type_args,
                arg_labels: _,
            } => {
                self.resolve_lens_in_expr(callee, consts);
                for a in args {
                    self.resolve_lens_in_expr(a, consts);
                }
                for t in type_args {
                    self.resolve_lens_in_type(t, consts);
                }
            }
            ExprKind::Block(b) => self.resolve_lens_in_block(b, consts),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.resolve_lens_in_expr(cond, consts);
                self.resolve_lens_in_block(then, consts);
                if let Some(eb) = else_branch {
                    self.resolve_lens_in_expr(eb, consts);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.resolve_lens_in_expr(lhs, consts);
                self.resolve_lens_in_expr(rhs, consts);
            }
            ExprKind::Unary { operand, .. } => self.resolve_lens_in_expr(operand, consts),
            ExprKind::Range { start, end, .. } => {
                if let Some(e2) = start {
                    self.resolve_lens_in_expr(e2, consts);
                }
                if let Some(e2) = end {
                    self.resolve_lens_in_expr(e2, consts);
                }
            }
            ExprKind::Assign { target, value, .. } => {
                self.resolve_lens_in_expr(target, consts);
                self.resolve_lens_in_expr(value, consts);
            }
            ExprKind::Field { receiver, .. } => self.resolve_lens_in_expr(receiver, consts),
            ExprKind::StructLit { fields, .. } => {
                for f in fields {
                    self.resolve_lens_in_expr(&mut f.value, consts);
                }
            }
            ExprKind::GenericStructLit {
                fields, type_args, ..
            } => {
                for f in fields {
                    self.resolve_lens_in_expr(&mut f.value, consts);
                }
                for t in type_args {
                    self.resolve_lens_in_type(t, consts);
                }
            }
            ExprKind::GenericEnumCall {
                type_args, args, ..
            } => {
                for t in type_args {
                    self.resolve_lens_in_type(t, consts);
                }
                for a in args {
                    self.resolve_lens_in_expr(a, consts);
                }
            }
            ExprKind::ArrayLit { elements } => {
                for el in elements {
                    self.resolve_lens_in_expr(el, consts);
                }
            }
            ExprKind::Index { receiver, index } => {
                self.resolve_lens_in_expr(receiver, consts);
                self.resolve_lens_in_expr(index, consts);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.resolve_lens_in_expr(scrutinee, consts);
                for a in arms {
                    self.resolve_lens_in_expr(&mut a.body, consts);
                }
            }
            ExprKind::Intrinsic {
                type_args,
                args,
                ret_ty,
                ..
            } => {
                for t in type_args {
                    self.resolve_lens_in_type(t, consts);
                }
                for a in args {
                    self.resolve_lens_in_expr(a, consts);
                }
                if let Some(rt) = ret_ty {
                    self.resolve_lens_in_type(rt, consts);
                }
            }
            _ => {}
        }
    }
}

/// v0.0.9 Phase 4: returns true iff `e` is a shape accepted as a
/// const/static initializer for v0.0.9. The literal forms are:
///
/// - integer / float / bool / string literal
/// - `Unary { op: Neg, operand: <numeric literal> }` for negative
///   numeric constants (`-1`, `-3.14`)
///
/// Arithmetic, identifier references, struct literals, array literals,
/// and any other shape are rejected with E0X30. Future slices may
/// widen this (struct-of-literals for the raytracer scene, const
/// arithmetic for derived values); v0.0.9 ships the smallest viable
/// surface.
fn is_const_initializer(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::IntLit(_, _)
        | ExprKind::FloatLit(_, _)
        | ExprKind::BoolLit(_)
        | ExprKind::StrLit(_)
        | ExprKind::CStrLit(_) => true,
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => matches!(
            operand.kind,
            ExprKind::IntLit(_, _) | ExprKind::FloatLit(_, _),
        ),
        // v0.0.19: a narrowing-literal cast `<numeric literal> as T`
        // (`1 as i8`, `-3 as i16`, `2 as f32`). The cast operand is a
        // numeric literal — or a unary-negated one — so the result is a
        // compile-time constant, the const/static-position analog of the
        // value sema would compute at runtime. Previously rejected with
        // E0X30 ("casts aren't literals") even though the plain-literal
        // form `= 1` worked, which was a surprising asymmetry. Bool and
        // string casts are intentionally excluded: they have no
        // narrowing-literal use and would not render as scalar globals.
        ExprKind::Cast { expr, .. } => {
            matches!(
                &expr.kind,
                ExprKind::IntLit(_, _) | ExprKind::FloatLit(_, _),
            ) || matches!(
                &expr.kind,
                ExprKind::Unary { op: UnaryOp::Neg, operand }
                    if matches!(operand.kind, ExprKind::IntLit(_, _) | ExprKind::FloatLit(_, _))
            )
        }
        // v0.0.12 G-033 (llama.cplus G-032): `#zero::[T]()` is a
        // sema-known constant zero of type T. For statics this lowers
        // to LLVM `zeroinitializer` — no runtime memset, just BSS.
        // Closes the inbound side of the flip-ownership story for
        // aggregate globals (lookup tables, struct globals) where
        // the C side previously held an all-zero / partially-init
        // aggregate that cpc now owns. Type-arg arity is validated
        // downstream by `check_intrinsic_zero` (E0501 on wrong shape).
        ExprKind::Intrinsic {
            name,
            args,
            type_args,
            ..
        } => name == "zero" && args.is_empty() && type_args.len() == 1,
        _ => false,
    }
}

/// v0.0.12 G-043 (llama.cplus): a `static` initializer may additionally be an
/// array literal `[a, b, c]` or fill `[v; N]` whose elements are themselves
/// static initializers (recursively, so nested arrays work). Statics become
/// real globals — codegen emits the array as an LLVM constant aggregate — so
/// there is no substitution concern. `const` stays literal-only
/// (`is_const_initializer`): a const is inlined at every use site, where an
/// array literal would be both surprising and substitution-heavy.
///
/// v0.0.13 (G-043 second half): a `static` may also be a **struct literal**
/// `T { f0: v0, f1: v1 }` whose field values are themselves static
/// initializers (recursively — struct-of-struct and array-of-struct compose).
/// This is the ggml `static const sphere_t scene[10] = {...}` pattern. Codegen
/// emits the struct as an LLVM constant aggregate in declared field order. The
/// generic form (`Pair[i32, bool] { ... }`) is intentionally excluded here:
/// it survives to codegen un-monomorphized (static initializers are not walked
/// by the mono expr rewriter), so accept only the concrete `StructLit` shape.
fn is_static_initializer(e: &Expr) -> bool {
    if is_const_initializer(e) {
        return true;
    }
    match &e.kind {
        ExprKind::ArrayLit { elements } => elements.iter().all(is_static_initializer),
        ExprKind::ArrayFill { fill, .. } => is_static_initializer(fill),
        ExprKind::StructLit { fields, .. } => {
            fields.iter().all(|f| is_static_initializer(&f.value))
        }
        _ => false,
    }
}

/// v0.0.9 Phase 4: walk a Block and substitute every const-name Ident
/// in it.
fn subst_block(b: &mut Block, consts: &std::collections::HashMap<String, (Expr, Type)>) {
    for s in &mut b.stmts {
        subst_stmt(s, consts);
    }
    if let Some(t) = &mut b.tail {
        subst_expr(t, consts);
    }
}

fn subst_stmt(s: &mut Stmt, consts: &std::collections::HashMap<String, (Expr, Type)>) {
    match &mut s.kind {
        StmtKind::Let { init, .. } => {
            if let Some(e) = init {
                subst_expr(e, consts);
            }
        }
        StmtKind::LetDestructure { init, .. } => subst_expr(init, consts),
        StmtKind::Return(opt) => {
            if let Some(e) = opt {
                subst_expr(e, consts);
            }
        }
        StmtKind::While { cond, body, .. } => {
            subst_expr(cond, consts);
            subst_block(body, consts);
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                if let Some(i) = init {
                    subst_stmt(i, consts);
                }
                if let Some(c) = cond {
                    subst_expr(c, consts);
                }
                for u in update {
                    subst_expr(u, consts);
                }
                subst_block(body, consts);
            }
            ForLoop::Range { iter, body, .. } => {
                subst_expr(iter, consts);
                subst_block(body, consts);
            }
        },
        StmtKind::Expr(e) => subst_expr(e, consts),
        StmtKind::Defer(e) => subst_expr(e, consts),
        StmtKind::Loop(b, _) => subst_block(b, consts),
        StmtKind::Assert(e) => subst_expr(e, consts),
        // After the slice-4A.5 lowering, IfLet / WhileLet / GuardLet
        // are rewritten into match-using forms; no original nodes
        // survive here. The arms are defense-in-depth no-ops in case
        // a future change orders the passes differently.
        StmtKind::IfLet {
            scrutinee,
            body,
            else_body,
            ..
        } => {
            subst_expr(scrutinee, consts);
            subst_block(body, consts);
            if let Some(eb) = else_body {
                subst_block(eb, consts);
            }
        }
        StmtKind::WhileLet {
            scrutinee, body, ..
        } => {
            subst_expr(scrutinee, consts);
            subst_block(body, consts);
        }
        StmtKind::GuardLet {
            scrutinee,
            else_body,
            ..
        } => {
            subst_expr(scrutinee, consts);
            subst_block(else_body, consts);
        }
        StmtKind::Break | StmtKind::Continue => {}
    }
}

/// GAP 3 (v0.0.19): overwrite the span of `e` and every sub-expression it
/// contains with `span`. Used when a `const` value is substituted into a use
/// site in (possibly) another file: the cloned literal must not keep its
/// definition-site coordinates, or a downstream diagnostic would render against
/// the wrong file. Const initializers are restricted to literal forms
/// (`is_const_initializer`), so the recursion only needs to cover those.
fn respan_tree(e: &mut Expr, span: Span) {
    e.span = span;
    match &mut e.kind {
        ExprKind::Unary { operand, .. } => respan_tree(operand, span),
        ExprKind::Cast { expr, .. } => respan_tree(expr, span),
        ExprKind::Intrinsic { args, .. } => {
            for a in args {
                respan_tree(a, span);
            }
        }
        // All other const-initializer-legal shapes are leaf literals
        // (IntLit / FloatLit / BoolLit / StrLit / CStrLit) with no
        // sub-expressions to re-stamp.
        _ => {}
    }
}

fn subst_expr(e: &mut Expr, consts: &std::collections::HashMap<String, (Expr, Type)>) {
    // Replace this node entirely if it's an Ident naming a const. Span
    // is taken from the original use site so diagnostics still point
    // there if a later pass complains about the substituted literal.
    //
    // The substituted expression is wrapped in `Cast { expr: literal,
    // ty: declared_ty }` so the const's declared type pins the value
    // at every use site — independent of surrounding inference. Without
    // the cast, an unsuffixed `176` substituted into a `usize`-typed
    // binary op falls back to `i32` per sema's literal default and
    // fires a type-mismatch.
    if let ExprKind::Ident(name) = &e.kind {
        if let Some((value, decl_ty)) = consts.get(name) {
            let use_span = e.span;
            // GAP 3 (v0.0.19): the cloned const *value* still carries the
            // const's definition-site byte spans. With multi-file builds, a
            // const defined in file A but used in file B would, on a downstream
            // type error against the substituted literal, render at file A's
            // offsets while sema believes it is in file B (current_file = B) —
            // the wrong file, and a clamped/wrong line. Re-stamp the whole
            // cloned subtree to the use site so any such diagnostic points where
            // the user actually wrote the reference.
            let mut value = value.clone();
            respan_tree(&mut value, use_span);
            *e = Expr {
                kind: ExprKind::Cast {
                    expr: Box::new(value),
                    ty: decl_ty.clone(),
                },
                span: use_span,
            };
            return;
        }
    }
    match &mut e.kind {
        ExprKind::IntLit(_, _)
        | ExprKind::FloatLit(_, _)
        | ExprKind::BoolLit(_)
        | ExprKind::StrLit(_)
        | ExprKind::CStrLit(_)
        | ExprKind::Ident(_)
        | ExprKind::Path { .. }
        | ExprKind::IncludeBytes { .. }
        | ExprKind::IncludeStr { .. }
        | ExprKind::EnvVar { .. } => {}
        ExprKind::Intrinsic { args, .. } => {
            for a in args {
                subst_expr(a, consts);
            }
        }
        ExprKind::Asm { operands, .. } => {
            for op in operands {
                subst_expr(&mut op.value, consts);
            }
        }
        ExprKind::InterpStr { parts } => {
            for p in parts {
                if let InterpStrPart::Expr(inner) = p {
                    subst_expr(inner, consts);
                }
            }
        }
        ExprKind::Block(b) => subst_block(b, consts),
        ExprKind::Await(inner) | ExprKind::Yield(inner) => subst_expr(inner, consts),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            subst_expr(cond, consts);
            subst_block(then, consts);
            if let Some(eb) = else_branch {
                subst_expr(eb, consts);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            subst_expr(callee, consts);
            for a in args {
                subst_expr(a, consts);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            subst_expr(lhs, consts);
            subst_expr(rhs, consts);
        }
        ExprKind::Unary { operand, .. } => subst_expr(operand, consts),
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                subst_expr(s, consts);
            }
            if let Some(en) = end {
                subst_expr(en, consts);
            }
        }
        ExprKind::Assign { target, value, .. } => {
            subst_expr(target, consts);
            subst_expr(value, consts);
        }
        ExprKind::Cast { expr, .. } => subst_expr(expr, consts),
        ExprKind::StructLit { fields, .. }
        | ExprKind::InferredStructLit { fields }
        | ExprKind::GenericStructLit { fields, .. } => {
            for f in fields {
                subst_expr(&mut f.value, consts);
            }
        }
        ExprKind::GenericEnumCall { args, .. } => {
            for a in args {
                subst_expr(a, consts);
            }
        }
        ExprKind::Field { receiver, .. } => subst_expr(receiver, consts),
        ExprKind::ArrayFill { fill, .. } => subst_expr(fill, consts),
        ExprKind::ArrayLit { elements } | ExprKind::TupleLit { elements } => {
            for el in elements {
                subst_expr(el, consts);
            }
        }
        ExprKind::Index { receiver, index } => {
            subst_expr(receiver, consts);
            subst_expr(index, consts);
        }
        ExprKind::Match { scrutinee, arms } => {
            subst_expr(scrutinee, consts);
            for a in arms {
                subst_expr(&mut a.body, consts);
            }
        }
        // v0.0.22 DSL.2: never reached — the per-item lower pass desugars
        // builder blocks before `substitute_consts` runs, so no
        // `BuilderBlock` node survives to here.
        ExprKind::BuilderBlock { .. } => {}
    }
}

/// v0.0.22 DSL.2/DSL.4: desugar a builder block into an ordinary block
/// expression over the fixed builder protocol. Both surface forms share
/// the same accumulator (`ctx::Builder::new()` + `.add(item)`); only the
/// finisher differs:
///
/// ```text
/// // @view { ... }  (root, container = None)        // vstack { ... }  (container)
/// {                                                 {
///     var __b = view::Builder::new();                   var __b = view::Builder::new();
///     ... entries add into __b ...                      ... entries add into __b ...
///     __b.finish()        // -> Root                    view::vstack(__b)   // -> Item
/// }                                                 }
/// ```
///
/// Each item entry becomes `var __i = <item>; <modifiers>; __b.add(__i);`.
/// `if` / `for` entries (DSL.4) lower to an ordinary `if`/`for` whose body
/// adds into the *same* `__b` — Flutter-style collection-if/for. A
/// container item's expr is itself a builder block; it is left in place
/// and desugared when the caller's post-desugar walk reaches it.
///
/// Temporary names derive from byte offsets (`__b<block-start>`,
/// `__i<item-start>`), unique within any one function body — deterministic,
/// no counter state. Synthesized nodes reuse the user's spans so sema's
/// ordinary diagnostics render at the user-written DSL line.
///
/// Called from the resolver's rewrite walk (multi-file: synthesized paths
/// still need alias rewriting) and from `lower_expr` (single-file mode).
pub fn desugar_builder_block(e: &mut Expr) {
    let block_span = e.span;
    let kind = std::mem::replace(
        &mut e.kind,
        ExprKind::IntLit(0, crate::lexer::NumSuffix::None),
    );
    let ExprKind::BuilderBlock {
        context,
        body,
        container,
    } = kind
    else {
        unreachable!("desugar_builder_block called on a non-builder expression");
    };

    let ctx_span = context.last().map(|i| i.span).unwrap_or(block_span);
    let b_name = format!("__b{}", block_span.start);

    // var __b = ctx::Builder::new();
    let mut new_path = context.clone();
    new_path.push(Ident {
        name: "Builder".to_string(),
        span: ctx_span,
    });
    new_path.push(Ident {
        name: "new".to_string(),
        span: ctx_span,
    });
    let mut stmts: Vec<Stmt> = Vec::new();
    stmts.push(Stmt {
        kind: StmtKind::Let {
            mutable: true,
            name: Ident {
                name: b_name.clone(),
                span: ctx_span,
            },
            ty: None,
            init: Some(Expr {
                kind: ExprKind::Call {
                    callee: Box::new(Expr {
                        kind: ExprKind::Path { segments: new_path },
                        span: ctx_span,
                    }),
                    args: Vec::new(),
                    type_args: Vec::new(),
                    arg_labels: Vec::new(),
                },
                span: ctx_span,
            }),
        },
        span: ctx_span,
    });

    for entry in body.entries {
        desugar_builder_entry(entry, &b_name, &mut stmts);
    }

    // Finisher: root -> `__b.finish()`; container -> `ctx::name(__b)`.
    let tail = match container {
        None => method_call(&b_name, "finish", Vec::new(), block_span),
        Some(name) => {
            let mut path = context;
            path.push(name);
            Expr {
                kind: ExprKind::Call {
                    callee: Box::new(Expr {
                        kind: ExprKind::Path { segments: path },
                        span: block_span,
                    }),
                    args: vec![Expr {
                        kind: ExprKind::Ident(b_name.clone()),
                        span: block_span,
                    }],
                    type_args: Vec::new(),
                    arg_labels: Vec::new(),
                },
                span: block_span,
            }
        }
    };
    e.kind = ExprKind::Block(Block {
        stmts,
        tail: Some(Box::new(tail)),
        span: body.span,
    });
}

/// Desugar one builder entry, appending the resulting statements to `out`.
/// Every produced item is added into the builder local named `b_name` —
/// so `if`/`for` bodies add into the same accumulator as their siblings.
fn desugar_builder_entry(entry: BuilderEntry, b_name: &str, out: &mut Vec<Stmt>) {
    match entry {
        BuilderEntry::Let(s) => out.push(s),
        BuilderEntry::Item { expr, modifiers } => {
            let item_span = expr.span;
            let i_name = format!("__i{}", item_span.start);
            // var __i = <item>;  (a container item's expr is itself a
            // builder block, desugared later by the caller's walk.)
            out.push(Stmt {
                kind: StmtKind::Let {
                    mutable: true,
                    name: Ident {
                        name: i_name.clone(),
                        span: item_span,
                    },
                    ty: None,
                    init: Some(expr),
                },
                span: item_span,
            });
            for m in modifiers {
                let place = Expr {
                    kind: ExprKind::Field {
                        receiver: Box::new(Expr {
                            kind: ExprKind::Ident(i_name.clone()),
                            span: m.name.span,
                        }),
                        name: m.name.clone(),
                    },
                    span: m.name.span,
                };
                let stmt_expr = match m.kind {
                    BuilderModifierKind::Assign(value) => Expr {
                        kind: ExprKind::Assign {
                            op: AssignOp::Assign,
                            target: Box::new(place),
                            value: Box::new(value),
                        },
                        span: m.span,
                    },
                    BuilderModifierKind::Call(args) => Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(place),
                            args,
                            type_args: Vec::new(),
                            arg_labels: Vec::new(),
                        },
                        span: m.span,
                    },
                };
                out.push(Stmt {
                    kind: StmtKind::Expr(stmt_expr),
                    span: m.span,
                });
            }
            // __b.add(__i);
            out.push(Stmt {
                kind: StmtKind::Expr(method_call(
                    b_name,
                    "add",
                    vec![Expr {
                        kind: ExprKind::Ident(i_name),
                        span: item_span,
                    }],
                    item_span,
                )),
                span: item_span,
            });
        }
        // `if COND { ... } [else { ... }]` — branches add into the same __b.
        BuilderEntry::If { cond, then, else_ } => {
            let span = cond.span;
            let mut then_stmts = Vec::new();
            for e in then {
                desugar_builder_entry(e, b_name, &mut then_stmts);
            }
            let then_block = Block {
                stmts: then_stmts,
                tail: None,
                span,
            };
            let else_branch = else_.map(|eb| {
                let mut else_stmts = Vec::new();
                for e in eb {
                    desugar_builder_entry(e, b_name, &mut else_stmts);
                }
                Box::new(Expr {
                    kind: ExprKind::Block(Block {
                        stmts: else_stmts,
                        tail: None,
                        span,
                    }),
                    span,
                })
            });
            out.push(Stmt {
                kind: StmtKind::Expr(Expr {
                    kind: ExprKind::If {
                        cond: Box::new(cond),
                        then: then_block,
                        else_branch,
                    },
                    span,
                }),
                span,
            });
        }
        // `for VAR in ITER { ... }` — body adds into the same __b.
        BuilderEntry::For { var, iter, body } => {
            let span = iter.span;
            let mut body_stmts = Vec::new();
            for e in body {
                desugar_builder_entry(e, b_name, &mut body_stmts);
            }
            let body_block = Block {
                stmts: body_stmts,
                tail: None,
                span,
            };
            out.push(Stmt {
                kind: StmtKind::For(
                    ForLoop::Range {
                        var,
                        iter,
                        body: body_block,
                    },
                    Vec::new(),
                ),
                span,
            });
        }
    }
}

/// `recv.method(args)` with every synthesized node stamped `span`.
fn method_call(recv: &str, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Field {
                    receiver: Box::new(Expr {
                        kind: ExprKind::Ident(recv.to_string()),
                        span,
                    }),
                    name: Ident {
                        name: method.to_string(),
                        span,
                    },
                },
                span,
            }),
            args,
            type_args: Vec::new(),
            arg_labels: Vec::new(),
        },
        span,
    }
}

fn placeholder_stmt(span: Span) -> StmtKind {
    // Returned in error paths so downstream sema doesn't trip on a fully
    // malformed AST. The placeholder is a no-op expression statement.
    StmtKind::Expr(Expr {
        kind: ExprKind::BoolLit(false),
        span,
    })
}

fn is_refutable(p: &Pattern) -> bool {
    match &p.kind {
        PatternKind::Wildcard | PatternKind::Binding(_) => false,
        PatternKind::Variant { .. } => true,
    }
}

fn collect_pattern_bindings(p: &Pattern) -> Vec<Ident> {
    fn walk(p: &Pattern, out: &mut Vec<Ident>) {
        match &p.kind {
            PatternKind::Wildcard => {}
            PatternKind::Binding(i) => out.push(i.clone()),
            PatternKind::Variant { payload, .. } => {
                for sub in payload {
                    walk(sub, out);
                }
            }
        }
    }
    let mut out = vec![];
    walk(p, &mut out);
    out
}

fn into_unit_block(b: Block) -> Block {
    // Discard any tail expression so the block has type unit. Pushing the
    // tail as a `Stmt::Expr` keeps its side effects.
    let Block {
        mut stmts,
        tail,
        span,
    } = b;
    if let Some(tail_box) = tail {
        let tail = *tail_box;
        let tspan = tail.span;
        stmts.push(Stmt {
            kind: StmtKind::Expr(tail),
            span: tspan,
        });
    }
    Block {
        stmts,
        tail: None,
        span,
    }
}

pub(crate) fn block_diverges(b: &Block) -> bool {
    if let Some(tail) = &b.tail {
        return expr_diverges(tail);
    }
    match b.stmts.last() {
        Some(s) => stmt_diverges(s),
        None => false,
    }
}

pub(crate) fn stmt_diverges(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Return(_) => true,
        // `break` / `continue` unconditionally transfer control out of
        // the current straight-line execution (to the loop exit / next
        // iteration), so a guard-let `else` block ending in either of
        // them is a valid divergence per slice 4A.5's diverge rule.
        StmtKind::Break | StmtKind::Continue => true,
        StmtKind::Expr(e) => expr_diverges(e),
        _ => false,
    }
}

pub(crate) fn expr_diverges(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Block(b) => block_diverges(b),
        ExprKind::Await(inner) => expr_diverges(inner),
        ExprKind::Yield(inner) => expr_diverges(inner),
        ExprKind::If {
            then, else_branch, ..
        } => {
            let then_d = block_diverges(then);
            let else_d = match else_branch {
                Some(eb) => expr_diverges(eb),
                None => false,
            };
            then_d && else_d
        }
        ExprKind::Match { arms, .. } => {
            // Match diverges iff every arm body diverges.
            !arms.is_empty() && arms.iter().all(|a| expr_diverges(&a.body))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn run(src: &str) -> (Program, Vec<Diagnostic>) {
        let toks = tokenize(src).expect("lex");
        let mut prog = parse(toks).expect("parse");
        let diags = lower(&mut prog, &PathBuf::from("test.cplus"), src);
        (prog, diags)
    }

    fn first_codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.code.0).collect()
    }

    // GAP 3 (v0.0.19): a lower-pass diagnostic (here E0X30 on a bad static
    // initializer) in an *imported* file must render against that file, not the
    // entry file. Before the multi-file `lower_multi`, every diagnostic used the
    // entry path + entry source, so an imported-file error pointed at the wrong
    // file and a byte offset past the entry source's end (wrong/clamped line).
    fn merge_two_files(
        entry_id: &str,
        entry_path: &str,
        entry_src: &str,
        lib_id: &str,
        lib_path: &str,
        lib_src: &str,
    ) -> (
        Program,
        std::collections::BTreeMap<String, (PathBuf, String)>,
        PathBuf,
    ) {
        let mut prog = parse(tokenize(entry_src).expect("lex entry")).expect("parse entry");
        for it in &mut prog.items {
            it.origin_file = Some(entry_id.to_string());
        }
        let mut lib = parse(tokenize(lib_src).expect("lex lib")).expect("parse lib");
        for it in &mut lib.items {
            it.origin_file = Some(lib_id.to_string());
        }
        prog.items.extend(lib.items);
        let mut files: std::collections::BTreeMap<String, (PathBuf, String)> =
            std::collections::BTreeMap::new();
        files.insert(
            entry_id.to_string(),
            (PathBuf::from(entry_path), entry_src.to_string()),
        );
        files.insert(
            lib_id.to_string(),
            (PathBuf::from(lib_path), lib_src.to_string()),
        );
        (prog, files, PathBuf::from(entry_path))
    }

    #[test]
    fn multi_file_static_init_error_points_at_origin_file_gap3() {
        let entry_src = "fn main() -> i32 { return 0; }\n";
        let lib_src = "// lib header\nstatic BAD: i32 = 1 + 2;\n";
        let (mut prog, files, entry_path) = merge_two_files(
            "main",
            "/proj/main.cplus",
            entry_src,
            "lib",
            "/proj/lib.cplus",
            lib_src,
        );
        let diags = lower_multi(&mut prog, &entry_path, entry_src, files);
        let d = diags
            .iter()
            .find(|d| d.code.0 == "E0X30")
            .expect("expected E0X30 on the bad static initializer");
        assert!(
            d.primary.file.ends_with("lib.cplus"),
            "diagnostic should point at lib.cplus, got {:?}",
            d.primary.file
        );
        // The bad static is on line 2 of lib_src — not clamped to the short
        // entry source.
        assert_eq!(d.primary.start.line, 2, "wrong line: {:?}", d.primary.start);
    }

    #[test]
    fn multi_file_const_array_length_error_points_at_origin_file_gap3() {
        // E0X36 (unknown const array length) raised in the array-length pass
        // also routes through the item's origin file.
        let entry_src = "fn main() -> i32 { return 0; }\n";
        let lib_src = "struct Buf { data: [i32; MISSING] }\n";
        let (mut prog, files, entry_path) = merge_two_files(
            "main",
            "/proj/main.cplus",
            entry_src,
            "lib",
            "/proj/lib.cplus",
            lib_src,
        );
        let diags = lower_multi(&mut prog, &entry_path, entry_src, files);
        let d = diags
            .iter()
            .find(|d| d.code.0 == "E0X36")
            .expect("expected E0X36 on the unknown array length");
        assert!(
            d.primary.file.ends_with("lib.cplus"),
            "diagnostic should point at lib.cplus, got {:?}",
            d.primary.file
        );
    }

    #[test]
    fn single_file_static_init_error_unchanged_gap3() {
        // The single-file `lower` entry still renders against the one file.
        let (_, diags) = run("static BAD: i32 = 1 + 2;\nfn main() -> i32 { return 0; }");
        let d = diags
            .iter()
            .find(|d| d.code.0 == "E0X30")
            .expect("expected E0X30");
        assert!(
            d.primary.file.ends_with("test.cplus"),
            "got {:?}",
            d.primary.file
        );
        assert_eq!(d.primary.start.line, 1);
    }

    #[test]
    fn if_let_with_variant_pattern_lowers() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                if let Maybe::Some(v) = m {
                    return v;
                }
                return 0;
            }
        "#;
        let (prog, diags) = run(src);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        // No IfLet should remain.
        let any_iflet = walks_any_iflet(&prog);
        assert!(!any_iflet, "expected if-let to be lowered");
    }

    #[test]
    fn if_let_irrefutable_binding_rejected() {
        let src = r#"
            fn main() -> i32 {
                if let x = 7 { return x; }
                return 0;
            }
        "#;
        let (_, diags) = run(src);
        assert!(
            first_codes(&diags).contains(&"E0347"),
            "expected E0347, got {:?}",
            first_codes(&diags)
        );
    }

    #[test]
    fn if_let_wildcard_rejected_as_irrefutable() {
        let src = r#"
            fn main() -> i32 {
                if let _ = 7 { return 1; }
                return 0;
            }
        "#;
        let (_, diags) = run(src);
        assert!(first_codes(&diags).contains(&"E0347"));
    }

    #[test]
    fn guard_let_basic_lowers() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                guard let Maybe::Some(v) = m else { return 0; };
                return v;
            }
        "#;
        let (prog, diags) = run(src);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        // After lowering the guard-let becomes `let v = match ...;`.
        let main_body = match &prog
            .items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Function(f) if f.name.name == "main" => Some(f),
                _ => None,
            })
            .unwrap()
            .body
            .stmts[1]
            .kind
        {
            StmtKind::Let {
                name,
                init: Some(_),
                ..
            } => name.name.clone(),
            other => panic!("expected let, got {other:?}"),
        };
        assert_eq!(main_body, "v");
    }

    #[test]
    fn guard_let_non_diverging_else_rejected() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                guard let Maybe::Some(v) = m else { let x: i32 = 1; };
                return v;
            }
        "#;
        let (_, diags) = run(src);
        assert!(first_codes(&diags).contains(&"E0348"));
    }

    #[test]
    fn guard_let_with_diverging_match_in_else_accepted() {
        // Else block ends with a match where every arm returns.
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                guard let Maybe::Some(v) = m else {
                    match m {
                        Maybe::Some(_) => { return 1; },
                        Maybe::None => { return 0; },
                    }
                };
                return v;
            }
        "#;
        let (_, diags) = run(src);
        assert!(!first_codes(&diags).contains(&"E0348"));
    }

    #[test]
    fn guard_let_no_binding_rejected() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                guard let Maybe::None = m else { return 0; };
                return 0;
            }
        "#;
        let (_, diags) = run(src);
        assert!(first_codes(&diags).contains(&"E0351"));
    }

    #[test]
    fn guard_let_multi_binding_rejected() {
        let src = r#"
            enum Pair { Both(i32, i32) }
            fn main() -> i32 {
                let p: Pair = Pair::Both(1, 2);
                guard let Pair::Both(a, b) = p else { return 0; };
                return a;
            }
        "#;
        let (_, diags) = run(src);
        assert!(first_codes(&diags).contains(&"E0352"));
    }

    #[test]
    fn guard_let_complement_overlap_rejected() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                guard let Maybe::Some(v) = m else |Maybe::Some(_)| { return 0; };
                return v;
            }
        "#;
        let (_, diags) = run(src);
        assert!(first_codes(&diags).contains(&"E0350"));
    }

    fn walks_any_iflet(prog: &Program) -> bool {
        fn walk_block(b: &Block) -> bool {
            for s in &b.stmts {
                if matches!(s.kind, StmtKind::IfLet { .. } | StmtKind::GuardLet { .. }) {
                    return true;
                }
                if let StmtKind::While { body, .. } = &s.kind {
                    if walk_block(body) {
                        return true;
                    }
                }
            }
            false
        }
        prog.items.iter().any(|it| match &it.kind {
            ItemKind::Function(f) => walk_block(&f.body),
            ItemKind::Impl(b) => b.methods.iter().any(|m| walk_block(&m.body)),
            _ => false,
        })
    }

    // ---- v0.0.13: const-eval for array lengths ----

    /// Find the declared array length of the first `let` binding in `main`.
    fn first_let_array_len(prog: &Program) -> Option<(u32, Option<String>)> {
        let f = prog.items.iter().find_map(|it| match &it.kind {
            ItemKind::Function(f) if f.name.name == "main" => Some(f),
            _ => None,
        })?;
        for s in &f.body.stmts {
            if let StmtKind::Let { ty: Some(t), .. } = &s.kind {
                if let TypeKind::Array { len, len_name, .. } = &t.kind {
                    return Some((*len, len_name.clone()));
                }
            }
        }
        None
    }

    #[test]
    fn const_array_length_folds_to_literal() {
        let (prog, diags) = run("const CAP: usize = 8;\n\
             fn main() -> i32 { let a: [i32; CAP] = [0; CAP]; return a[0]; }");
        assert!(!first_codes(&diags).contains(&"E0X36"), "diags: {diags:?}");
        // The `len_name` placeholder is folded into a literal `8` and cleared.
        assert_eq!(first_let_array_len(&prog), Some((8, None)));
    }

    #[test]
    fn const_fill_count_folds_to_literal() {
        let (prog, _diags) = run("const N: u32 = 4;\n\
             fn main() -> i32 { let a: [i32; 4] = [7; N]; return a[0]; }");
        // Walk to the fill expr and confirm count folded to 4, name cleared.
        let f = prog
            .items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Function(f) if f.name.name == "main" => Some(f),
                _ => None,
            })
            .unwrap();
        let mut found = false;
        for s in &f.body.stmts {
            if let StmtKind::Let { init: Some(e), .. } = &s.kind {
                if let ExprKind::ArrayFill {
                    count, count_name, ..
                } = &e.kind
                {
                    assert_eq!((*count, count_name.clone()), (4, None));
                    found = true;
                }
            }
        }
        assert!(found, "no ArrayFill found");
    }

    #[test]
    fn unknown_const_array_length_e0x36() {
        let (_, diags) = run("fn main() -> i32 { let a: [i32; NOPE] = [0; 1]; return a[0]; }");
        assert!(first_codes(&diags).contains(&"E0X36"), "diags: {diags:?}");
    }

    #[test]
    fn non_integer_const_array_length_e0x36() {
        let (_, diags) = run("const NAME: str = \"hi\";\n\
             fn main() -> i32 { let a: [i32; NAME] = [0; 1]; return 0; }");
        assert!(first_codes(&diags).contains(&"E0X36"), "diags: {diags:?}");
    }

    #[test]
    fn const_array_length_in_struct_field_folds() {
        // A const length used in a struct field type resolves too.
        let (prog, diags) = run("const W: u32 = 16;\n\
             struct Buf { data: [u8; W] }\n\
             fn main() -> i32 { return 0; }");
        assert!(!first_codes(&diags).contains(&"E0X36"), "diags: {diags:?}");
        let s = prog.items.iter().find_map(|it| match &it.kind {
            ItemKind::Struct(s) if s.name.name == "Buf" => Some(s),
            _ => None,
        });
        let fld_ty = &s.unwrap().fields[0].ty;
        assert!(matches!(
            &fld_ty.kind,
            TypeKind::Array {
                len: 16,
                len_name: None,
                ..
            }
        ));
    }

    // ---- v0.0.22 DSL.2: builder-block desugar ----

    /// The desugared block from `let v = @... { ... };` in `src`, after
    /// running the lowering pass.
    fn desugared_builder(src: &str) -> Block {
        let (prog, diags) = run(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        let ItemKind::Function(f) = &prog.items[0].kind else {
            panic!("expected fn");
        };
        let StmtKind::Let {
            init: Some(init), ..
        } = &f.body.stmts[0].kind
        else {
            panic!("expected let with init");
        };
        let ExprKind::Block(b) = &init.kind else {
            panic!("expected desugared Block, got {:?}", init.kind);
        };
        b.clone()
    }

    /// `stmt` is `recv.method(...)`; return (recv, method).
    fn as_method_call(s: &Stmt) -> (String, String) {
        let StmtKind::Expr(e) = &s.kind else {
            panic!("expected expression statement, got {:?}", s.kind);
        };
        let ExprKind::Call { callee, .. } = &e.kind else {
            panic!("expected call, got {:?}", e.kind);
        };
        let ExprKind::Field { receiver, name } = &callee.kind else {
            panic!("expected method callee, got {:?}", callee.kind);
        };
        let ExprKind::Ident(recv) = &receiver.kind else {
            panic!("expected ident receiver, got {:?}", receiver.kind);
        };
        (recv.clone(), name.name.clone())
    }

    #[test]
    fn builder_block_desugars_to_protocol_calls() {
        let src = "fn main() -> i32 {\n    let v = @view {\n        text(1)\n            .font = 2\n            .pad(3)\n        text(4)\n    };\n    return 0;\n}\n";
        let b = desugared_builder(src);
        // var __b = view::Builder::new();
        let StmtKind::Let {
            mutable: true,
            name,
            init: Some(init),
            ..
        } = &b.stmts[0].kind
        else {
            panic!("expected builder let, got {:?}", b.stmts[0].kind);
        };
        assert!(name.name.starts_with("__b"), "builder temp: {}", name.name);
        let ExprKind::Call { callee, .. } = &init.kind else {
            panic!("expected Builder::new call");
        };
        let ExprKind::Path { segments } = &callee.kind else {
            panic!("expected path callee, got {:?}", callee.kind);
        };
        let path: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(path, ["view", "Builder", "new"]);
        // var __i = text(1);
        let StmtKind::Let {
            mutable: true,
            name: item_name,
            ..
        } = &b.stmts[1].kind
        else {
            panic!("expected item let, got {:?}", b.stmts[1].kind);
        };
        assert!(
            item_name.name.starts_with("__i"),
            "item temp: {}",
            item_name.name
        );
        // __i.font = 2;
        let StmtKind::Expr(assign) = &b.stmts[2].kind else {
            panic!("expected assign stmt");
        };
        let ExprKind::Assign {
            op: AssignOp::Assign,
            target,
            ..
        } = &assign.kind
        else {
            panic!("expected plain assign, got {:?}", assign.kind);
        };
        let ExprKind::Field { name: fld, .. } = &target.kind else {
            panic!("expected field target");
        };
        assert_eq!(fld.name, "font");
        // __i.pad(3); then __b.add(__i);
        assert_eq!(as_method_call(&b.stmts[3]).1, "pad");
        let (recv, m) = as_method_call(&b.stmts[4]);
        assert_eq!((recv.starts_with("__b"), m.as_str()), (true, "add"));
        // second item: let + add
        assert!(matches!(&b.stmts[5].kind, StmtKind::Let { .. }));
        assert_eq!(as_method_call(&b.stmts[6]).1, "add");
        assert_eq!(b.stmts.len(), 7);
        // tail: __b.finish()
        let tail = b.tail.as_ref().expect("finish tail");
        let ExprKind::Call { callee, .. } = &tail.kind else {
            panic!("expected finish call");
        };
        let ExprKind::Field { name, .. } = &callee.kind else {
            panic!("expected method callee");
        };
        assert_eq!(name.name, "finish");
    }

    #[test]
    fn builder_block_let_entries_splice_in_order() {
        let src = "fn main() -> i32 {\n    let v = @view {\n        let x = 1;\n        text(x)\n    };\n    return 0;\n}\n";
        let b = desugared_builder(src);
        // builder let, user let, item let, add — in that order.
        assert_eq!(b.stmts.len(), 4);
        let StmtKind::Let { name, .. } = &b.stmts[1].kind else {
            panic!("expected spliced user let");
        };
        assert_eq!(name.name, "x");
        assert_eq!(as_method_call(&b.stmts[3]).1, "add");
    }

    #[test]
    fn container_desugars_to_builder_plus_constructor() {
        // A bare container `row { ... }` desugars to its own Builder block
        // whose finisher is the container constructor `row(__b)` (vs the
        // root's `.finish()`). Single-file lower has no resolver context
        // inheritance, so the path is the bare container name.
        let src = "fn main() -> i32 {\n    let v = @view {\n        row {\n            text(1)\n        }\n    };\n    return 0;\n}\n";
        let b = desugared_builder(src);
        // outer stmts[1] is the item-let; its init is the container's block.
        let StmtKind::Let {
            init: Some(inner), ..
        } = &b.stmts[1].kind
        else {
            panic!("expected container item let");
        };
        let ExprKind::Block(inner) = &inner.kind else {
            panic!("container must desugar to a Block, got {:?}", inner.kind);
        };
        // Inner accumulator: `var __b = Builder::new();`
        let StmtKind::Let {
            init: Some(new_call),
            ..
        } = &inner.stmts[0].kind
        else {
            panic!("expected inner builder let");
        };
        assert!(
            matches!(new_call.kind, ExprKind::Call { .. }),
            "Builder::new call"
        );
        // Inner finisher (tail) is the container constructor call `row(__b)`,
        // NOT `.finish()`.
        let tail = inner.tail.as_ref().expect("container finisher tail");
        let ExprKind::Call { callee, args, .. } = &tail.kind else {
            panic!(
                "container tail must be a constructor call, got {:?}",
                tail.kind
            );
        };
        let ExprKind::Path { segments } = &callee.kind else {
            panic!(
                "container constructor must be a path, got {:?}",
                callee.kind
            );
        };
        assert_eq!(segments.last().unwrap().name, "row");
        assert_eq!(args.len(), 1, "constructor takes the filled Builder");
    }

    #[test]
    fn builder_if_for_lower_to_guarded_looped_adds() {
        // `if`/`for` entries add into the SAME builder as their siblings.
        let src = "fn main() -> i32 {\n    let v = @view {\n        text(0)\n        if flag {\n            text(1)\n        }\n        for x in xs {\n            text(2)\n        }\n    };\n    return 0;\n}\n";
        let b = desugared_builder(src);
        // Locate the `if` statement and the `for` statement among the block.
        let has_if = b.stmts.iter().any(|s| {
            matches!(
                &s.kind,
                StmtKind::Expr(e) if matches!(e.kind, ExprKind::If { .. })
            )
        });
        let has_for = b.stmts.iter().any(|s| matches!(&s.kind, StmtKind::For(..)));
        assert!(has_if, "if entry lowers to an if statement");
        assert!(has_for, "for entry lowers to a for statement");
        // The if's then-block contains an `__b.add(...)` (adds into the
        // enclosing builder, not a fresh one).
        let if_stmt = b
            .stmts
            .iter()
            .find_map(|s| match &s.kind {
                StmtKind::Expr(e) => match &e.kind {
                    ExprKind::If { then, .. } => Some(then),
                    _ => None,
                },
                _ => None,
            })
            .expect("if statement");
        let add_call = if_stmt.stmts.iter().any(|s| {
            matches!(
                &s.kind,
                StmtKind::Expr(e) if matches!(&e.kind, ExprKind::Call { callee, .. }
                    if matches!(&callee.kind, ExprKind::Field { name, .. } if name.name == "add"))
            )
        });
        assert!(add_call, "if-branch items add into the enclosing builder");
    }

    #[test]
    fn builder_temps_are_span_derived_and_distinct() {
        let src = "fn main() -> i32 {\n    let a = @view {\n        text(1)\n    };\n    let b = @view {\n        text(2)\n    };\n    return 0;\n}\n";
        let (prog, diags) = run(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        let ItemKind::Function(f) = &prog.items[0].kind else {
            panic!("expected fn");
        };
        let mut names = Vec::new();
        for s in &f.body.stmts {
            if let StmtKind::Let {
                init: Some(init), ..
            } = &s.kind
            {
                if let ExprKind::Block(b) = &init.kind {
                    if let StmtKind::Let { name, .. } = &b.stmts[0].kind {
                        names.push(name.name.clone());
                    }
                }
            }
        }
        assert_eq!(names.len(), 2);
        assert_ne!(names[0], names[1], "builder temps must not collide");
    }
}
