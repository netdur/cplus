//! Slice 4A.5 — `if let` / `guard let` lowering.
//!
//! Pattern-binding sugar over slice-3I `match`. The parser produces
//! `StmtKind::IfLet` / `StmtKind::GuardLet`; this pass:
//!
//! 1. Emits the slice-specific diagnostics:
//!    - E0347: irrefutable `if let` pattern (use plain `let`)
//!    - E0348: `guard let` else block must diverge (return / break / continue)
//!    - E0349: `guard let` else complement is not exhaustive with the
//!      success pattern (only fires when the user wrote an explicit
//!      `else |Pat|` form — without a complement we synthesize `_`
//!      which is trivially exhaustive)
//!    - E0350: `guard let` complement overlaps the success pattern
//!    - E0351: `guard let` requires the success pattern to bind at least
//!      one value (else it's just an `if let` with side effects)
//!    - E0352: multi-binding `guard let` patterns are deferred to a
//!      follow-up slice
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
use crate::lexer::{NumSuffix, Span};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Run the lowering pass over a merged Program. Mutates `prog` so all
/// `StmtKind::IfLet` / `StmtKind::GuardLet` nodes are replaced with
/// equivalent match-using forms.
///
/// v0.0.9 Phase 4: also validates module-scope `const` / `static`
/// initializers are literals (E0911) and substitutes every use-site
/// reference to a const with the initializer expression. After this
/// pass returns, sema sees literal expressions where the user wrote a
/// const name — codegen never observes a const-name reference.
pub fn lower(prog: &mut Program, file: &Path, src: &str) -> Vec<Diagnostic> {
    // Single-file entry: build a one-entry files map and delegate to the
    // multi-file path. Mirrors `sema::check` / `attrs::check`.
    let mut files: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
    files.insert(String::new(), (file.to_path_buf(), src.to_string()));
    lower_multi(prog, file, src, files)
}

/// Multi-file entry point. Mirrors `sema::check_multi` / `attrs::check_multi`.
///
/// GAP 3 (v0.0.19): the merged program carries items from several files, each
/// tagged with its `origin_file`. Diagnostics raised here (E0911 on a bad
/// static/const initializer, the if-let/guard-let desugar errors, E0912 on a
/// bad const array length) must render against the file the *item* came from,
/// not the entry file. Previously `lower` knew only the entry path + source, so
/// an error in an imported file pointed at the entry file (wrong file) and a
/// byte offset past the entry source's length (wrong / clamped line). Track
/// `current_file` per item and resolve spans through that file's `LineMap`.
pub fn lower_multi(
    prog: &mut Program,
    entry_file: &Path,
    entry_src: &str,
    files: BTreeMap<String, (PathBuf, String)>,
) -> Vec<Diagnostic> {
    let mut cx = Lower::new(entry_file.to_path_buf(), entry_src, files);
    // Derive through the empty impl: expand `impl T: Eq {}` (and Ord /
    // Hash / Clone / ToText) into memberwise implementations FIRST, so the
    // synthesized methods take part in every later lowering step and reach
    // sema exactly like user-written code.
    cx.expand_derives(prog);
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
    /// Parameters for receiver-less associated functions, keyed by their
    /// `Type::function` suffix. A qualified call such as `json::Value::parse`
    /// uses the final two path segments to select this table.
    assoc_params: std::collections::HashMap<String, Vec<Vec<ParamInfo>>>,
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
            assoc_params: std::collections::HashMap::new(),
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
                        let params: Vec<ParamInfo> = m.params.iter().map(param_info).collect();
                        if m.receiver.is_some() {
                            self.method_params
                                .entry(m.name.name.clone())
                                .or_default()
                                .push(params);
                        } else {
                            self.assoc_params
                                .entry(format!("{}::{}", b.target.name, m.name.name))
                                .or_default()
                                .push(params);
                        }
                    }
                }
                // An interface's method *declarations* are call candidates too:
                // a call through an interface-bounded generic (`w: W` where
                // `W: Window`) resolves by name here, and if the only registered
                // candidate is a same-named `impl` method on another type (e.g.
                // `Node::width(v)`), a no-arg interface call like `w.width()`
                // mis-selects it and reports a bogus "missing argument". Register
                // interface methods so the 0-arg arrangement matches, regardless
                // of whether a concrete implementor is present in this build.
                ItemKind::Interface(d) => {
                    for m in &d.methods {
                        let params: Vec<ParamInfo> = m.params.iter().map(param_info).collect();
                        if m.receiver.is_some() {
                            self.method_params
                                .entry(m.name.name.clone())
                                .or_default()
                                .push(params);
                        } else {
                            self.assoc_params
                                .entry(format!("{}::{}", d.name.name, m.name.name))
                                .or_default()
                                .push(params);
                        }
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
            ExprKind::Path { segments } if segments.len() >= 2 => {
                let key = format!(
                    "{}::{}",
                    segments[segments.len() - 2].name,
                    segments[segments.len() - 1].name
                );
                self.assoc_params
                    .get(&key)
                    .is_some_and(|cs| cs.iter().any(|p| p.len() > n_args))
            }
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
            ExprKind::Path { segments } if segments.len() >= 2 => {
                let key = format!(
                    "{}::{}",
                    segments[segments.len() - 2].name,
                    segments[segments.len() - 1].name
                );
                match self.assoc_params.get(&key) {
                    Some(c) => c.clone(),
                    None => return,
                }
            }
            _ => return,
        };
        let mut results: Vec<(usize, Vec<ArgSlot>)> = Vec::new();
        let mut first_err: Option<(&'static str, String, Span)> = None;
        // Two arrangements are the same only if they'd produce the same
        // lowered call: same slot shape AND the same exprs spliced into the
        // `Default` slots. Comparing slots alone deduped `Sig::on(v, ctx=0,
        // once=false)` against `Bus::on(name, v, ctx=0)` for a 2-arg call
        // (both are `[Arg0, Arg1, Default]`) and spliced the FIRST
        // candidate's `false` into the other type's `*u8` slot.
        let splice_sig = |ci: usize, slots: &[ArgSlot]| -> Vec<Option<Expr>> {
            slots
                .iter()
                .enumerate()
                .map(|(pos, sl)| match sl {
                    ArgSlot::Default => candidates[ci][pos].default.clone(),
                    ArgSlot::Arg(_) => None,
                })
                .collect()
        };
        for (ci, params) in candidates.iter().enumerate() {
            match Self::match_call(params, args, arg_labels, call_span) {
                Ok(slots) => {
                    if !results
                        .iter()
                        .any(|(pci, s)| *s == slots && splice_sig(*pci, s) == splice_sig(ci, &slots))
                    {
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
                mutable,
            } => {
                s.kind = self.lower_if_let(pattern, scrutinee, body, else_body, mutable, s.span);
            }
            StmtKind::GuardLet {
                pattern,
                scrutinee,
                complement,
                else_body,
                mutable,
            } => {
                s.kind =
                    self.lower_guard_let(pattern, scrutinee, complement, else_body, mutable, s.span);
            }
            StmtKind::WhileLet {
                pattern,
                scrutinee,
                body,
                mutable,
            } => {
                s.kind = self.lower_while_let(pattern, scrutinee, body, mutable, s.span);
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
        // reports/bug-25: a `match` with literal arms becomes a temp binding
        // plus an if/else chain over equality tests. Same discipline as the
        // pattern-let and builder desugars above: the new AST node exists
        // between the parser and here and nowhere else, so sema, borrowck,
        // monomorphize and codegen never learn a new pattern kind.
        if self.desugar_literal_match(e) {
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
            ExprKind::FnRef { callee, .. } => {
                self.lower_expr(callee);
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

    /// reports/bug-25. `match SCRUT { L1 => B1, L2 => B2, n => BD }` where the
    /// arms are literals becomes
    ///
    /// ```text
    /// { let __lit_m<span> = SCRUT;
    ///   if __lit_m == L1 { B1 } else if __lit_m == L2 { B2 } else { let n = __lit_m; BD } }
    /// ```
    ///
    /// The temp is what makes a side-effecting scrutinee (`match f() { .. }`)
    /// evaluate once, which the repeated equality tests would otherwise not
    /// guarantee.
    ///
    /// Exhaustiveness: a literal arm covers one value out of a type's whole
    /// range, so a catch-all is REQUIRED and must come last — there is no
    /// finite set of literals that exhausts an integer, and the desugar needs
    /// a final `else`. E0344 (the existing non-exhaustive-match code) says so.
    ///
    /// Returns true when it rewrote `e`.
    fn desugar_literal_match(&mut self, e: &mut Expr) -> bool {
        let ExprKind::Match { scrutinee, arms } = &e.kind else {
            return false;
        };
        if !arms
            .iter()
            .any(|a| matches!(a.pattern.kind, PatternKind::Lit(_)))
        {
            return false;
        }
        let span = e.span;
        let tmp = format!("__lit_m{}", span.start);
        // Split into the literal arms and the trailing catch-all.
        let mut tests: Vec<(Expr, Expr)> = Vec::new();
        let mut fallback: Option<(Option<Ident>, Expr)> = None;
        for arm in arms {
            if fallback.is_some() {
                self.err(
                    "E0344",
                    "unreachable `match` arm: an earlier arm already matches every value"
                        .to_string(),
                    arm.pattern.span,
                );
                break;
            }
            match &arm.pattern.kind {
                PatternKind::Lit(lit) => tests.push(((**lit).clone(), arm.body.clone())),
                PatternKind::Wildcard => fallback = Some((None, arm.body.clone())),
                PatternKind::Binding(name) => {
                    fallback = Some((Some(name.clone()), arm.body.clone()))
                }
                PatternKind::Variant { .. } => {
                    self.err(
                        "E0343",
                        "a `match` cannot mix literal patterns with variant patterns — \
                         literals match a value, variants match a case"
                            .to_string(),
                        arm.pattern.span,
                    );
                    return false;
                }
            }
        }
        let Some((bind, fallback_body)) = fallback else {
            self.err(
                "E0344",
                "non-exhaustive `match`: literal arms cover one value each, so a \
                 catch-all arm (`_` or a binding) is required"
                    .to_string(),
                span,
            );
            return false;
        };
        let ident = |name: &str| Expr {
            kind: ExprKind::Ident(name.to_string()),
            span,
        };
        let as_block = |body: Expr| match body.kind {
            ExprKind::Block(b) => b,
            _ => Block {
                stmts: Vec::new(),
                tail: Some(Box::new(body)),
                span,
            },
        };
        // Innermost `else`: the catch-all, with its binding (if any) rebound
        // to the temp so the arm body reads as written.
        let mut chain_else = as_block(fallback_body);
        if let Some(name) = bind {
            chain_else.stmts.insert(
                0,
                Stmt {
                    kind: StmtKind::Let {
                        mutable: false,
                        name,
                        ty: None,
                        init: Some(ident(&tmp)),
                    },
                    span,
                },
            );
        }
        let mut chain = Expr {
            kind: ExprKind::Block(chain_else),
            span,
        };
        for (lit, body) in tests.into_iter().rev() {
            chain = Expr {
                kind: ExprKind::If {
                    cond: Box::new(Expr {
                        kind: ExprKind::Binary {
                            op: BinOp::Eq,
                            lhs: Box::new(ident(&tmp)),
                            rhs: Box::new(lit),
                        },
                        span,
                    }),
                    then: as_block(body),
                    else_branch: Some(Box::new(chain)),
                },
                span,
            };
        }
        e.kind = ExprKind::Block(Block {
            stmts: vec![Stmt {
                kind: StmtKind::Let {
                    mutable: false,
                    name: Ident {
                        name: tmp,
                        span,
                    },
                    ty: None,
                    init: Some((**scrutinee).clone()),
                },
                span,
            }],
            tail: Some(Box::new(chain)),
            span,
        });
        true
    }

    /// `if let PAT = E { B }` →  `match E { PAT => { B; }, _ => {} }`
    /// `if let PAT = E { B } else { B2 }` → `match E { PAT => { B; }, _ => { B2; } }`
    /// `if var PAT = E { B }` → same match, with each binding in PAT
    /// renamed to a fresh temp and `var NAME = TEMP;` rebinds prepended to
    /// B — sema sees ordinary mutable locals (arm bindings stay immutable).
    fn lower_if_let(
        &mut self,
        mut pattern: Pattern,
        scrutinee: Expr,
        mut body: Block,
        else_body: Option<Block>,
        mutable: bool,
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
        if mutable {
            let rebinds = mutable_rebinds(&mut pattern);
            body.stmts.splice(0..0, rebinds);
        }
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
    /// `guard var` emits the same rewrite with a `var` head instead of
    /// `let` — the enclosing-scope binding is mutable. A complement
    /// binding stays immutable: it is an ordinary match-arm binding
    /// scoped to the diverging else block.
    fn lower_guard_let(
        &mut self,
        pattern: Pattern,
        scrutinee: Expr,
        complement: Option<Pattern>,
        else_body: Block,
        mutable: bool,
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
            mutable,
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
        mut pattern: Pattern,
        scrutinee: Expr,
        body: Block,
        mutable: bool,
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
        let mut body_block = into_unit_block(body);
        if mutable {
            // `while var` — same rebind rewrite as `if var`; the bindings
            // are fresh mutable locals each iteration.
            let rebinds = mutable_rebinds(&mut pattern);
            body_block.stmts.splice(0..0, rebinds);
        }
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
            // A literal complement covers exactly one value, so it can never
            // be exhaustive with the success pattern. Reported below by the
            // "both must be Variant" path, which it also fails.
            PatternKind::Lit(_) => {}
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
    /// `static` initializer is a literal (E0911). Returns a map from
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
        prog: &mut Program,
    ) -> std::collections::HashMap<String, (Expr, Type)> {
        // Phase 1: gather every const declaration (name → initializer, type,
        // origin file) so const expressions can reference consts declared in
        // any order or file.
        let mut raw: std::collections::HashMap<String, (Expr, Type, Option<String>)> =
            std::collections::HashMap::new();
        for item in &prog.items {
            if let ItemKind::Const(c) = &item.kind {
                raw.insert(
                    c.name.name.clone(),
                    (c.value.clone(), c.ty.clone(), item.origin_file.clone()),
                );
            }
        }
        // Phase 2: resolve each const once (memoized, cycle-checked). A
        // successfully evaluated EXPRESSION initializer is folded to a typed
        // literal; plain-literal initializers keep their original shape (and
        // their historical diagnostics).
        let mut resolved: std::collections::HashMap<String, Option<CVal>> =
            std::collections::HashMap::new();
        let mut visiting: Vec<String> = Vec::new();
        let names: Vec<String> = raw.keys().cloned().collect();
        for name in &names {
            let mut cx = ConstCx {
                raw: &raw,
                resolved: &mut resolved,
                visiting: &mut visiting,
            };
            let _ = self.resolve_const_scalar(name, &mut cx);
        }
        // Phase 3: write folded initializers back into the declaration nodes
        // (so sema / codegen / substitution all see literals), validate
        // statics, and build the substitution table.
        let mut consts: std::collections::HashMap<String, (Expr, Type)> =
            std::collections::HashMap::new();
        for item in &mut prog.items {
            // GAP 3: an E0911 on a bad initializer must point at the file the
            // const/static was declared in, not always the entry file.
            self.set_current_file(item.origin_file.as_deref());
            match &mut item.kind {
                ItemKind::Const(c) => {
                    if !is_const_initializer(&c.value) {
                        // An expression initializer: replace with its folded
                        // literal. A miss here means resolution failed and
                        // already diagnosed (E0911/E0921) — skip the entry so
                        // downstream passes don't chew on a non-literal.
                        match resolved.get(&c.name.name) {
                            Some(Some(v)) => c.value = cval_to_expr(*v, c.value.span),
                            _ => continue,
                        }
                    }
                    consts.insert(c.name.name.clone(), (c.value.clone(), c.ty.clone()));
                }
                ItemKind::Static(s) => {
                    if !is_static_initializer(&s.value) {
                        // A scalar-typed static takes the same constant
                        // expressions a const does (`static M: u64 =
                        // (1u64 << 40) - 1;`), folded in place.
                        if let Some(exp) = cscalar_of_type(&s.ty) {
                            let mut cx = ConstCx {
                                raw: &raw,
                                resolved: &mut resolved,
                                visiting: &mut visiting,
                            };
                            if let Ok(v) = self.const_eval(&s.value, Some(exp), &mut cx, false) {
                                s.value = cval_to_expr(v, s.value.span);
                            }
                        } else {
                            self.err(
                                "E0911",
                                "static initializer must be a literal (integer, float, bool, string, unary-negated numeric literal), `#zero::[T]()`, an array literal/fill, a (non-generic) struct literal of such, or a scalar constant expression".to_string(),
                                s.value.span,
                            );
                        }
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
    /// **E0912**. After this pass `len_name` / `count_name` are `None`.
    fn resolve_const_array_lengths(
        &mut self,
        prog: &mut Program,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        for item in &mut prog.items {
            // GAP 3: an E0912 on a bad const array length renders against the
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

    /// Resolve a single `const`-name length to a `u32`, emitting E0912 on a
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
                    "E0912",
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
                        "E0912",
                        format!("array length `const {name}` exceeds the u32 maximum"),
                        span,
                    );
                    0
                }
                _ => {
                    self.err(
                        "E0912",
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

    /// v0.0.27 const expressions: evaluate an inline array-length /
    /// fill-count expression (`[T; CAP * 2]`, `[v; 1 << SHIFT]`) at `usize`
    /// against the (already folded) const table, and range-check the result
    /// into the `u32` every later pass expects. Errors ride the evaluator's
    /// E0921 codes; the u32 ceiling keeps E0912's historical message.
    fn resolve_len_expr(
        &mut self,
        e: &Expr,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) -> u32 {
        // The evaluator resolves `Ident`s through a ConstCx. Seed one whose
        // memo table is pre-filled from the folded const table — every entry
        // there is a literal, so the quiet probe cannot fail loudly.
        let mut raw: std::collections::HashMap<String, (Expr, Type, Option<String>)> =
            std::collections::HashMap::new();
        for (name, (value, ty)) in consts {
            raw.insert(name.clone(), (value.clone(), ty.clone(), None));
        }
        let mut resolved: std::collections::HashMap<String, Option<CVal>> =
            std::collections::HashMap::new();
        let mut visiting: Vec<String> = Vec::new();
        let mut cx = ConstCx {
            raw: &raw,
            resolved: &mut resolved,
            visiting: &mut visiting,
        };
        let usize_ty = CScalar::Int(CInt {
            bits: 64,
            signed: false,
            size: true,
        });
        match self.const_eval(e, Some(usize_ty), &mut cx, false) {
            Ok(CVal::Int { v, .. }) if (0..=u32::MAX as i128).contains(&v) => v as u32,
            Ok(CVal::Int { .. }) => {
                self.err(
                    "E0912",
                    "array length expression exceeds the u32 maximum".to_string(),
                    e.span,
                );
                0
            }
            Ok(_) => {
                self.err(
                    "E0912",
                    "array length expression must evaluate to an unsigned integer".to_string(),
                    e.span,
                );
                0
            }
            // The evaluator already diagnosed (E0921).
            Err(()) => 0,
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
                len_expr,
            } => {
                if let Some(name) = len_name.take() {
                    *len = self.resolve_one_len(&name, span, consts);
                }
                // v0.0.27 const expressions: an inline `[T; CAP * 2]` length
                // evaluates at `usize` against the const environment.
                if let Some(e) = len_expr.take() {
                    *len = self.resolve_len_expr(&e, consts);
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

    /// issue-01: both const-length lenses ride the generic walk. The
    /// hand-rolled expression walk this replaced had an `_ => {}`
    /// fallthrough, so `[v; N]` nested in a tuple literal, an inferred struct
    /// literal, an interpolated string or an `await` kept `count_name` unset
    /// and sema then rejected the fill as a 0-element array literal (E0330) —
    /// a diagnostic about a program the user did not write.
    fn resolve_lens_in_block(
        &mut self,
        b: &mut Block,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        let resolved = walk_block(b, &mut LenResolver { lower: self, consts });
        *b = resolved;
    }

    fn resolve_lens_in_expr(
        &mut self,
        e: &mut Expr,
        consts: &std::collections::HashMap<String, (Expr, Type)>,
    ) {
        let resolved = walk_expr(e, &mut LenResolver { lower: self, consts });
        *e = resolved;
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
/// and any other shape are rejected with E0911. Future slices may
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
        // E0911 ("casts aren't literals") even though the plain-literal
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
/// issue-01: the const-length lens resolver. Two node shapes carry a lens —
/// an array TYPE (`[T; N]`) and an array-fill EXPRESSION (`[v; N]`) — and
/// both resolve through `Lower::resolve_one_len`, which needs `&mut Lower` to
/// report E0912.
struct LenResolver<'a> {
    lower: &'a mut Lower,
    consts: &'a std::collections::HashMap<String, (Expr, Type)>,
}

impl ExprRewriter for LenResolver<'_> {
    fn visit_type(&mut self, t: &Type) -> Option<Type> {
        let mut resolved = t.clone();
        self.lower.resolve_lens_in_type(&mut resolved, self.consts);
        Some(resolved)
    }

    fn visit_expr(&mut self, e: &Expr) -> Option<Expr> {
        let ExprKind::ArrayFill {
            fill,
            count_name,
            count_expr,
            ..
        } = &e.kind
        else {
            return None;
        };
        // No lens on this fill: let the generic walk handle its children.
        let count = if let Some(name) = count_name {
            self.lower.resolve_one_len(name, e.span, self.consts)
        } else if let Some(ce) = count_expr {
            // v0.0.27 const expressions: inline `[v; CAP * 2]` count.
            self.lower.resolve_len_expr(ce, self.consts)
        } else {
            return None;
        };
        Some(Expr {
            kind: ExprKind::ArrayFill {
                fill: Box::new(walk_expr(fill, self)),
                count,
                count_name: None,
                count_expr: None,
            },
            span: e.span,
        })
    }
}

/// issue-01: const substitution rides the generic walk — the only node it has
/// an opinion about is an `Ident` naming a `const`.
struct ConstSubst<'a> {
    consts: &'a std::collections::HashMap<String, (Expr, Type)>,
}

impl ExprRewriter for ConstSubst<'_> {
    fn visit_expr(&mut self, e: &Expr) -> Option<Expr> {
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
        let ExprKind::Ident(name) = &e.kind else {
            return None;
        };
        let (value, decl_ty) = self.consts.get(name)?;
        // GAP 3 (v0.0.19): the cloned const *value* still carries the
        // const's definition-site byte spans. With multi-file builds, a
        // const defined in file A but used in file B would, on a downstream
        // type error against the substituted literal, render at file A's
        // offsets while sema believes it is in file B (current_file = B) —
        // the wrong file, and a clamped/wrong line. Re-stamp the whole
        // cloned subtree to the use site so any such diagnostic points where
        // the user actually wrote the reference.
        let mut value = value.clone();
        respan_tree(&mut value, e.span);
        Some(Expr {
            kind: ExprKind::Cast {
                expr: Box::new(value),
                ty: decl_ty.clone(),
            },
            span: e.span,
        })
    }
}

fn subst_block(b: &mut Block, consts: &std::collections::HashMap<String, (Expr, Type)>) {
    let resolved = walk_block(b, &mut ConstSubst { consts });
    *b = resolved;
}

/// GAP 3 (v0.0.19): overwrite the span of `e` and every sub-expression it
/// contains with `span`. Used when a `const` value is substituted into a use
/// site in (possibly) another file: the cloned literal must not keep its
/// definition-site coordinates, or a downstream diagnostic would render against
/// the wrong file.
///
/// issue-01: this used to cover only the shapes `is_const_initializer` accepted
/// at the time, with an `_ => {}` for the rest — so when that grammar grew
/// (struct-literal and array initializers), their field values kept the
/// definition-site spans. The generic walk has no such list to keep in sync.
struct Respan {
    span: Span,
}

impl ExprRewriter for Respan {
    fn visit_expr(&mut self, e: &Expr) -> Option<Expr> {
        // Rebuild the children first (the rewriter is re-entered on each of
        // them), then stamp this node's span.
        Some(Expr {
            kind: walk_expr_kind(e, self),
            span: self.span,
        })
    }
}

fn respan_tree(e: &mut Expr, span: Span) {
    let respanned = walk_expr(e, &mut Respan { span });
    *e = respanned;
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
        container_args,
        container_arg_labels,
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

    // Finisher: root -> `__b.finish()`; container -> `ctx::name(__b)`,
    // or `ctx::name(__b, args...)` when the element carried arguments
    // (DSL.5) — the Builder stays first, so the zero-arg finisher
    // signature is the same contract.
    let tail = match container {
        None => method_call(&b_name, "finish", Vec::new(), block_span),
        Some(name) => {
            let mut path = context;
            path.push(name);
            let mut args = vec![Expr {
                kind: ExprKind::Ident(b_name.clone()),
                span: block_span,
            }];
            let arg_labels = if container_arg_labels.is_empty() {
                Vec::new()
            } else {
                let mut labels = vec![None];
                labels.extend(container_arg_labels);
                labels
            };
            args.extend(container_args);
            Expr {
                kind: ExprKind::Call {
                    callee: Box::new(Expr {
                        kind: ExprKind::Path { segments: path },
                        span: block_span,
                    }),
                    args,
                    type_args: Vec::new(),
                    arg_labels,
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
        BuilderEntry::Let(s) => out.push(*s),
        BuilderEntry::Item { expr, modifiers } => {
            let item_span = expr.span;
            // A descriptive temp name (not `__i…`, which read like a user's
            // loop variable `i` and sent people chasing phantom "loop var
            // moved" errors). sema keys the builder-chain E0335 note on this
            // prefix — keep the two in sync.
            let i_name = format!("__builder_item{}", item_span.start);
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
                match m.kind {
                    BuilderModifierKind::Assign(value) => {
                        // `.field = value` — a field write, never a move.
                        let stmt_expr = Expr {
                            kind: ExprKind::Assign {
                                op: AssignOp::Assign,
                                target: Box::new(place),
                                value: Box::new(value),
                            },
                            span: m.span,
                        };
                        out.push(Stmt {
                            kind: StmtKind::Expr(stmt_expr),
                            span: m.span,
                        });
                    }
                    BuilderModifierKind::Call(args) => {
                        let call = Expr {
                            kind: ExprKind::Call {
                                callee: Box::new(place),
                                args,
                                type_args: Vec::new(),
                                arg_labels: Vec::new(),
                            },
                            span: m.span,
                        };
                        // A modifier is either a `take self -> Node` BUILDER
                        // (`.width`/`.grow`/…, returns a new item) or a `ref self`
                        // MUTATOR (`.set_pad`/`.boost`/…, mutates in place, returns
                        // unit). We can't tell them apart HERE — the receiver type
                        // and the method's return type aren't resolved until sema.
                        // So always thread the result — `__i = __i.m(..)`. For a
                        // builder that re-inits the temp after the take-self move
                        // (they compose in one chain); for a mutator the RHS is
                        // unit, and sema recognizes a `__builder_item` reassign from
                        // a unit-returning call as the in-place mutation it is (no
                        // rebind, no E0302). This is type-directed, so a mutator
                        // needs no naming convention (`set_*` or otherwise).
                        out.push(Stmt {
                            kind: StmtKind::Expr(Expr {
                                kind: ExprKind::Assign {
                                    op: AssignOp::Assign,
                                    target: Box::new(Expr {
                                        kind: ExprKind::Ident(i_name.clone()),
                                        span: m.name.span,
                                    }),
                                    value: Box::new(call),
                                },
                                span: m.span,
                            }),
                            span: m.span,
                        });
                    }
                };
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
        // A literal matches one value out of many.
        PatternKind::Lit(_) | PatternKind::Variant { .. } => true,
    }
}

/// `if var` / `while var` desugar support: rewrite `pattern` in place so
/// every binding `name` becomes a fresh `__var<start>_<name>` temp
/// (span-start keeps siblings unique, mirroring `_discard<start>`), and
/// return the `var NAME = TEMP;` rebind statements to prepend to the
/// success body. Sema then sees ordinary mutable locals; the match-arm
/// bindings themselves stay immutable and are each read exactly once.
fn mutable_rebinds(pattern: &mut Pattern) -> Vec<Stmt> {
    fn walk(p: &mut Pattern, out: &mut Vec<Stmt>) {
        match &mut p.kind {
            PatternKind::Wildcard | PatternKind::Lit(_) => {}
            PatternKind::Binding(id) => {
                let user = id.clone();
                id.name = format!("__var{}_{}", id.span.start, id.name);
                out.push(Stmt {
                    kind: StmtKind::Let {
                        mutable: true,
                        name: user.clone(),
                        ty: None,
                        init: Some(Expr {
                            kind: ExprKind::Ident(id.name.clone()),
                            span: user.span,
                        }),
                    },
                    span: user.span,
                });
            }
            PatternKind::Variant { payload, .. } => {
                for sub in payload {
                    walk(sub, out);
                }
            }
        }
    }
    let mut out = vec![];
    walk(pattern, &mut out);
    out
}

fn collect_pattern_bindings(p: &Pattern) -> Vec<Ident> {
    fn walk(p: &Pattern, out: &mut Vec<Ident>) {
        match &p.kind {
            PatternKind::Wildcard | PatternKind::Lit(_) => {}
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

// ===== const expressions =====
//
// `const MASK: u64 = (1u64 << 40) - 1;` — a const (or scalar static)
// initializer may be a pure compile-time expression over literals and
// previously-declared consts, folded here in lower before sema runs. The
// evaluation is TYPED: arithmetic happens at the declared type's width,
// and overflow is a hard error (E0921) rather than a silent wrap — the
// explicit wrap spellings `+%` / `-%` / `*%` wrap, exactly as at runtime.
// This kills the wrap-through-i32 mask-building dance: a big mask is
// written as the arithmetic that defines it.
//
// Grammar of a const expression: literals, names of other consts,
// `+ - * / %`, `+% -% *%`, `<< >>`, `& | ^ ~`, unary `-` / `!`,
// comparisons and `&& ||` (producing bool), and `as` casts between
// scalar types. References between consts are order-independent
// (memoized resolution); a reference cycle is E0921.
//
// A plain-literal initializer keeps the exact v0.0.9 path (including its
// diagnostics); only non-literal initializers enter the evaluator. On
// success the folded literal is written back into the declaration node,
// so sema, codegen, and the const-substitution pass still see literals
// everywhere — no pass after this one knows const expressions exist.

/// An integer scalar's shape: width + signedness. `isize`/`usize` fold at
/// 64 bits (the compiler's only pointer width today — same assumption
/// `layout_of` makes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CInt {
    bits: u8,
    signed: bool,
    /// True for `isize` / `usize` — same 64-bit fold width, but a DISTINCT
    /// type from `i64` / `u64` (sema keeps them apart, so const evaluation
    /// must too, and fold results must re-emit the right suffix).
    size: bool,
}

/// The scalar type a const expression evaluates at.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CScalar {
    Int(CInt),
    Float(u8),
    Bool,
}

/// A compile-time value. Ints carry their type so range checks and mixed-
/// type errors are exact; the payload is i128, wide enough for every
/// supported width's full range.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CVal {
    Int { v: i128, ty: CInt },
    Float { v: f64, bits: u8 },
    Bool(bool),
}

fn cscalar_of_name(name: &str) -> Option<CScalar> {
    let int = |bits, signed| Some(CScalar::Int(CInt { bits, signed, size: false }));
    let size = |signed| Some(CScalar::Int(CInt { bits: 64, signed, size: true }));
    match name {
        "i8" => int(8, true),
        "i16" => int(16, true),
        "i32" => int(32, true),
        "i64" => int(64, true),
        "isize" => size(true),
        "u8" => int(8, false),
        "u16" => int(16, false),
        "u32" => int(32, false),
        "u64" => int(64, false),
        "usize" => size(false),
        "f16" => Some(CScalar::Float(16)),
        "f32" => Some(CScalar::Float(32)),
        "f64" => Some(CScalar::Float(64)),
        "bool" => Some(CScalar::Bool),
        _ => None,
    }
}

fn cscalar_of_type(t: &Type) -> Option<CScalar> {
    match &t.kind {
        TypeKind::Path(p) => cscalar_of_name(p),
        _ => None,
    }
}

/// Display name for diagnostics. Width-64 renders as the fixed-width name
/// (`i64`/`u64`) — good enough even when the source wrote `isize`/`usize`.
fn cscalar_name(s: CScalar) -> &'static str {
    match s {
        CScalar::Int(CInt { signed: true, size: true, .. }) => "isize",
        CScalar::Int(CInt { signed: false, size: true, .. }) => "usize",
        CScalar::Int(CInt { bits: 8, signed: true, .. }) => "i8",
        CScalar::Int(CInt { bits: 16, signed: true, .. }) => "i16",
        CScalar::Int(CInt { bits: 32, signed: true, .. }) => "i32",
        CScalar::Int(CInt { bits: 64, signed: true, .. }) => "i64",
        CScalar::Int(CInt { bits: 8, signed: false, .. }) => "u8",
        CScalar::Int(CInt { bits: 16, signed: false, .. }) => "u16",
        CScalar::Int(CInt { bits: 32, signed: false, .. }) => "u32",
        CScalar::Int(CInt { bits: 64, signed: false, .. }) => "u64",
        CScalar::Int(_) => "int",
        CScalar::Float(16) => "f16",
        CScalar::Float(32) => "f32",
        CScalar::Float(_) => "f64",
        CScalar::Bool => "bool",
    }
}

fn cint_min(t: CInt) -> i128 {
    if t.signed {
        -(1i128 << (t.bits - 1))
    } else {
        0
    }
}

fn cint_max(t: CInt) -> i128 {
    if t.signed {
        (1i128 << (t.bits - 1)) - 1
    } else {
        (1i128 << t.bits) - 1
    }
}

fn cint_in_range(v: i128, t: CInt) -> bool {
    v >= cint_min(t) && v <= cint_max(t)
}

/// Two's-complement wrap of `v` into `t` — the semantic of `+%`-family ops
/// and of `as` narrowing, matching runtime behavior bit for bit.
fn cint_wrap(v: i128, t: CInt) -> i128 {
    let mask = if t.bits == 128 { -1i128 } else { (1i128 << t.bits) - 1 };
    let low = v & mask;
    if t.signed && (low >> (t.bits - 1)) & 1 == 1 {
        low - (1i128 << t.bits)
    } else {
        low
    }
}

fn cint_of_suffix(s: NumSuffix) -> Option<CInt> {
    let int = |bits, signed| Some(CInt { bits, signed, size: false });
    let size = |signed| Some(CInt { bits: 64, signed, size: true });
    match s {
        NumSuffix::I8 => int(8, true),
        NumSuffix::I16 => int(16, true),
        NumSuffix::I32 => int(32, true),
        NumSuffix::I64 => int(64, true),
        NumSuffix::Isize => size(true),
        NumSuffix::U8 => int(8, false),
        NumSuffix::U16 => int(16, false),
        NumSuffix::U32 => int(32, false),
        NumSuffix::U64 => int(64, false),
        NumSuffix::Usize => size(false),
        _ => None,
    }
}

/// The literal suffix that pins an emitted fold result to its type.
/// 64-bit renders as `i64`/`u64` even for `isize`/`usize` declarations —
/// the substitution pass wraps every use in a cast to the declared type,
/// which re-types the literal at the use site.
fn suffix_of_cint(t: CInt) -> NumSuffix {
    if t.size {
        return if t.signed { NumSuffix::Isize } else { NumSuffix::Usize };
    }
    match (t.bits, t.signed) {
        (8, true) => NumSuffix::I8,
        (16, true) => NumSuffix::I16,
        (32, true) => NumSuffix::I32,
        (8, false) => NumSuffix::U8,
        (16, false) => NumSuffix::U16,
        (32, false) => NumSuffix::U32,
        (_, true) => NumSuffix::I64,
        (_, false) => NumSuffix::U64,
    }
}

fn suffix_of_float_bits(bits: u8) -> NumSuffix {
    match bits {
        16 => NumSuffix::F16,
        32 => NumSuffix::F32,
        _ => NumSuffix::F64,
    }
}

/// Render a folded value back to a literal expression every later pass
/// accepts. A signed minimum (e.g. `i64::MIN`) has no positive-magnitude
/// spelling, so it renders as its bit pattern cast to the signed type —
/// the same value the runtime `as` produces.
fn cval_to_expr(v: CVal, span: Span) -> Expr {
    let e = |kind| Expr { kind, span };
    match v {
        CVal::Int { v, ty } => {
            if v >= 0 {
                e(ExprKind::IntLit(v as u64, suffix_of_cint(ty)))
            } else if v == cint_min(ty) {
                let pattern = (v as u128 as u64) & if ty.bits == 64 { u64::MAX } else { (1u64 << ty.bits) - 1 };
                let unsigned = CInt { bits: ty.bits, signed: false, size: false };
                e(ExprKind::Cast {
                    expr: Box::new(e(ExprKind::IntLit(pattern, suffix_of_cint(unsigned)))),
                    ty: Type {
                        kind: TypeKind::Path(cscalar_name(CScalar::Int(ty)).to_string()),
                        span,
                    },
                })
            } else {
                e(ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(e(ExprKind::IntLit((-v) as u64, suffix_of_cint(ty)))),
                })
            }
        }
        CVal::Float { v, bits } => {
            if v.is_sign_negative() {
                e(ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(e(ExprKind::FloatLit(-v, suffix_of_float_bits(bits)))),
                })
            } else {
                e(ExprKind::FloatLit(v, suffix_of_float_bits(bits)))
            }
        }
        CVal::Bool(b) => e(ExprKind::BoolLit(b)),
    }
}

/// The const-resolution state threaded through evaluation: raw declarations,
/// memoized results (`None` = declared but not usable as a scalar value),
/// and the in-progress stack for cycle detection.
struct ConstCx<'a> {
    raw: &'a std::collections::HashMap<String, (Expr, Type, Option<String>)>,
    resolved: &'a mut std::collections::HashMap<String, Option<CVal>>,
    visiting: &'a mut Vec<String>,
}

impl Lower {
    /// Evaluate a const expression. `expected` is the declared type's scalar
    /// shape (propagated so unsuffixed literals type correctly); `quiet`
    /// suppresses diagnostics (used when probing a plain-literal initializer
    /// for the cross-const environment — its own path already diagnoses).
    /// `Err(())` always means: diagnostics were emitted unless quiet.
    fn const_eval(
        &mut self,
        e: &Expr,
        expected: Option<CScalar>,
        cx: &mut ConstCx,
        quiet: bool,
    ) -> Result<CVal, ()> {
        macro_rules! bail {
            ($span:expr, $($msg:tt)*) => {{
                if !quiet {
                    self.err("E0921", format!($($msg)*), $span);
                }
                return Err(());
            }};
        }
        match &e.kind {
            ExprKind::IntLit(v, suffix) => {
                let ty = match cint_of_suffix(*suffix) {
                    Some(t) => {
                        if let Some(CScalar::Int(exp)) = expected {
                            if exp != t {
                                bail!(
                                    e.span,
                                    "type mismatch in constant expression: expected `{}`, literal is `{}` — change the suffix or add an `as` cast",
                                    cscalar_name(CScalar::Int(exp)),
                                    cscalar_name(CScalar::Int(t))
                                );
                            }
                        }
                        t
                    }
                    None => match expected {
                        Some(CScalar::Int(t)) => t,
                        Some(CScalar::Float(bits)) => {
                            // An int literal does not float implicitly —
                            // same rule as the runtime language.
                            let _ = bits;
                            bail!(
                                e.span,
                                "type mismatch in constant expression: expected a float literal (write `{v}.0`)"
                            );
                        }
                        Some(CScalar::Bool) => {
                            bail!(e.span, "type mismatch in constant expression: expected `bool`, found an integer literal")
                        }
                        None => CInt { bits: 32, signed: true, size: false },
                    },
                };
                let val = *v as i128;
                if !cint_in_range(val, ty) {
                    bail!(
                        e.span,
                        "literal `{v}` is out of range for `{}`",
                        cscalar_name(CScalar::Int(ty))
                    );
                }
                Ok(CVal::Int { v: val, ty })
            }
            ExprKind::FloatLit(v, suffix) => {
                let bits = match suffix {
                    NumSuffix::F16 => 16,
                    NumSuffix::F32 => 32,
                    NumSuffix::F64 => 64,
                    NumSuffix::None => match expected {
                        Some(CScalar::Float(b)) => b,
                        Some(other) => bail!(
                            e.span,
                            "type mismatch in constant expression: expected `{}`, found a float literal",
                            cscalar_name(other)
                        ),
                        None => 64,
                    },
                    _ => bail!(e.span, "integer suffix on a float literal"),
                };
                if let Some(CScalar::Float(exp)) = expected {
                    if exp != bits {
                        bail!(
                            e.span,
                            "type mismatch in constant expression: expected `{}`, literal is `{}`",
                            cscalar_name(CScalar::Float(exp)),
                            cscalar_name(CScalar::Float(bits))
                        );
                    }
                }
                Ok(CVal::Float { v: *v, bits })
            }
            ExprKind::BoolLit(b) => {
                if let Some(exp) = expected {
                    if exp != CScalar::Bool {
                        bail!(
                            e.span,
                            "type mismatch in constant expression: expected `{}`, found `bool`",
                            cscalar_name(exp)
                        );
                    }
                }
                Ok(CVal::Bool(*b))
            }
            ExprKind::Ident(name) => {
                let Some(val) = self.resolve_const_scalar(name, cx) else {
                    if cx.raw.contains_key(name) {
                        bail!(
                            e.span,
                            "`{name}` is not a numeric or bool `const`, so it cannot appear in a constant expression"
                        );
                    }
                    bail!(
                        e.span,
                        "`{name}` is not a known `const`; constant expressions may only reference module-scope `const` names"
                    );
                };
                let actual = match val {
                    CVal::Int { ty, .. } => CScalar::Int(ty),
                    CVal::Float { bits, .. } => CScalar::Float(bits),
                    CVal::Bool(_) => CScalar::Bool,
                };
                if let Some(exp) = expected {
                    if exp != actual {
                        bail!(
                            e.span,
                            "type mismatch in constant expression: expected `{}`, `{}` is `{}` — add an `as` cast",
                            cscalar_name(exp),
                            name,
                            cscalar_name(actual)
                        );
                    }
                }
                Ok(val)
            }
            ExprKind::Unary { op, operand } => match op {
                UnaryOp::Neg => match self.const_eval(operand, expected, cx, quiet)? {
                    CVal::Int { v, ty } => {
                        if !ty.signed {
                            bail!(e.span, "unary `-` on unsigned constant of type `{}`", cscalar_name(CScalar::Int(ty)));
                        }
                        if !cint_in_range(-v, ty) {
                            bail!(e.span, "negation overflows `{}`", cscalar_name(CScalar::Int(ty)));
                        }
                        Ok(CVal::Int { v: -v, ty })
                    }
                    CVal::Float { v, bits } => Ok(CVal::Float { v: -v, bits }),
                    CVal::Bool(_) => bail!(e.span, "unary `-` on a bool constant"),
                },
                UnaryOp::Not => match self.const_eval(operand, Some(CScalar::Bool), cx, quiet)? {
                    CVal::Bool(b) => Ok(CVal::Bool(!b)),
                    _ => bail!(e.span, "`!` requires a bool constant"),
                },
                UnaryOp::BitNot => match self.const_eval(operand, expected, cx, quiet)? {
                    CVal::Int { v, ty } => Ok(CVal::Int {
                        v: cint_wrap(!v, ty),
                        ty,
                    }),
                    _ => bail!(e.span, "`~` requires an integer constant"),
                },
                UnaryOp::Ref { .. } | UnaryOp::Deref => {
                    bail!(e.span, "not a constant expression")
                }
            },
            ExprKind::Binary { op, lhs, rhs } => {
                use BinOp::*;
                match op {
                    And | Or => {
                        let l = self.const_eval(lhs, Some(CScalar::Bool), cx, quiet)?;
                        let r = self.const_eval(rhs, Some(CScalar::Bool), cx, quiet)?;
                        match (l, r) {
                            (CVal::Bool(a), CVal::Bool(b)) => Ok(CVal::Bool(if *op == And {
                                a && b
                            } else {
                                a || b
                            })),
                            _ => bail!(e.span, "`&&` / `||` require bool constants"),
                        }
                    }
                    Eq | Ne | Lt | Le | Gt | Ge => {
                        if let Some(exp) = expected {
                            if exp != CScalar::Bool {
                                bail!(
                                    e.span,
                                    "type mismatch in constant expression: comparison produces `bool`, expected `{}`",
                                    cscalar_name(exp)
                                );
                            }
                        }
                        let l = self.const_eval(lhs, None, cx, quiet)?;
                        let lscalar = match l {
                            CVal::Int { ty, .. } => CScalar::Int(ty),
                            CVal::Float { bits, .. } => CScalar::Float(bits),
                            CVal::Bool(_) => CScalar::Bool,
                        };
                        let r = self.const_eval(rhs, Some(lscalar), cx, quiet)?;
                        let cmp = match (l, r) {
                            (CVal::Int { v: a, .. }, CVal::Int { v: b, .. }) => a.partial_cmp(&b),
                            (CVal::Float { v: a, .. }, CVal::Float { v: b, .. }) => {
                                a.partial_cmp(&b)
                            }
                            (CVal::Bool(a), CVal::Bool(b)) if matches!(op, Eq | Ne) => {
                                a.partial_cmp(&b)
                            }
                            _ => bail!(e.span, "invalid comparison in constant expression"),
                        };
                        let Some(ord) = cmp else {
                            bail!(e.span, "NaN comparison in constant expression");
                        };
                        let b = match op {
                            Eq => ord.is_eq(),
                            Ne => !ord.is_eq(),
                            Lt => ord.is_lt(),
                            Le => ord.is_le(),
                            Gt => ord.is_gt(),
                            Ge => ord.is_ge(),
                            _ => unreachable!(),
                        };
                        Ok(CVal::Bool(b))
                    }
                    Shl | Shr => {
                        let l = self.const_eval(lhs, expected, cx, quiet)?;
                        let CVal::Int { v, ty } = l else {
                            bail!(e.span, "shift requires an integer constant");
                        };
                        let CVal::Int { v: amt, .. } = self.const_eval(rhs, None, cx, quiet)? else {
                            bail!(e.span, "shift amount must be an integer constant");
                        };
                        if amt < 0 || amt >= ty.bits as i128 {
                            bail!(
                                e.span,
                                "shift amount {amt} is out of range for `{}`",
                                cscalar_name(CScalar::Int(ty))
                            );
                        }
                        let raw = if *op == Shl {
                            cint_wrap(v << amt, ty)
                        } else if ty.signed {
                            v >> amt
                        } else {
                            // Logical shift on the unsigned bit pattern.
                            (v as u128 & ((1u128 << ty.bits) - 1)) as i128 >> amt
                        };
                        if *op == Shl && raw != v << amt {
                            bail!(
                                e.span,
                                "`<<` overflows `{}` in constant expression",
                                cscalar_name(CScalar::Int(ty))
                            );
                        }
                        Ok(CVal::Int { v: raw, ty })
                    }
                    Add | Sub | Mul | Div | Mod | AddWrap | SubWrap | MulWrap | BitAnd | BitOr
                    | BitXor => {
                        let l = self.const_eval(lhs, expected, cx, quiet)?;
                        match l {
                            CVal::Int { v: a, ty } => {
                                let CVal::Int { v: b, .. } =
                                    self.const_eval(rhs, Some(CScalar::Int(ty)), cx, quiet)?
                                else {
                                    bail!(e.span, "mixed types in constant expression");
                                };
                                let wrap = |x| cint_wrap(x, ty);
                                let v = match op {
                                    Add => a + b,
                                    Sub => a - b,
                                    Mul => a * b,
                                    AddWrap => wrap(a + b),
                                    SubWrap => wrap(a - b),
                                    MulWrap => wrap(a * b),
                                    Div => {
                                        if b == 0 {
                                            bail!(e.span, "division by zero in constant expression");
                                        }
                                        a / b
                                    }
                                    Mod => {
                                        if b == 0 {
                                            bail!(e.span, "modulo by zero in constant expression");
                                        }
                                        a % b
                                    }
                                    BitAnd => a & b,
                                    BitOr => a | b,
                                    BitXor => a ^ b,
                                    _ => unreachable!(),
                                };
                                if !cint_in_range(v, ty) {
                                    bail!(
                                        e.span,
                                        "constant arithmetic overflows `{}`; use `{}` to wrap",
                                        cscalar_name(CScalar::Int(ty)),
                                        match op {
                                            Add => "+%",
                                            Sub => "-%",
                                            Mul => "*%",
                                            _ => "+%",
                                        }
                                    );
                                }
                                Ok(CVal::Int { v, ty })
                            }
                            CVal::Float { v: a, bits } => {
                                let CVal::Float { v: b, .. } =
                                    self.const_eval(rhs, Some(CScalar::Float(bits)), cx, quiet)?
                                else {
                                    bail!(e.span, "mixed types in constant expression");
                                };
                                let v = match op {
                                    Add => a + b,
                                    Sub => a - b,
                                    Mul => a * b,
                                    Div => a / b,
                                    _ => bail!(
                                        e.span,
                                        "operator not supported on float constants"
                                    ),
                                };
                                Ok(CVal::Float { v, bits })
                            }
                            CVal::Bool(_) => bail!(e.span, "arithmetic on a bool constant"),
                        }
                    }
                }
            }
            ExprKind::Cast { expr, ty } => {
                let Some(target) = cscalar_of_type(ty) else {
                    bail!(
                        e.span,
                        "`as` in a constant expression must target a scalar type"
                    );
                };
                if let Some(exp) = expected {
                    if exp != target {
                        bail!(
                            e.span,
                            "type mismatch in constant expression: expected `{}`, cast produces `{}`",
                            cscalar_name(exp),
                            cscalar_name(target)
                        );
                    }
                }
                let inner = self.const_eval(expr, None, cx, quiet)?;
                match (inner, target) {
                    // Int→int truncates/extends by bit pattern — the runtime
                    // `as` semantic, wrap included.
                    (CVal::Int { v, .. }, CScalar::Int(t)) => Ok(CVal::Int {
                        v: cint_wrap(v, t),
                        ty: t,
                    }),
                    (CVal::Int { v, .. }, CScalar::Float(bits)) => Ok(CVal::Float {
                        v: v as f64,
                        bits,
                    }),
                    (CVal::Float { v, .. }, CScalar::Int(t)) => {
                        let t0 = v.trunc();
                        if !t0.is_finite()
                            || t0 < cint_min(t) as f64
                            || t0 > cint_max(t) as f64
                        {
                            bail!(
                                e.span,
                                "float value does not fit `{}` in constant cast",
                                cscalar_name(CScalar::Int(t))
                            );
                        }
                        Ok(CVal::Int { v: t0 as i128, ty: t })
                    }
                    (CVal::Float { v, .. }, CScalar::Float(bits)) => {
                        let v = if bits == 32 { v as f32 as f64 } else { v };
                        Ok(CVal::Float { v, bits })
                    }
                    (CVal::Bool(b), CScalar::Int(t)) => Ok(CVal::Int {
                        v: if b { 1 } else { 0 },
                        ty: t,
                    }),
                    (CVal::Bool(b), CScalar::Bool) => Ok(CVal::Bool(b)),
                    (_, CScalar::Bool) => {
                        bail!(e.span, "cannot cast to `bool` in a constant expression")
                    }
                    (CVal::Bool(_), CScalar::Float(_)) => {
                        bail!(e.span, "cannot cast `bool` to a float in a constant expression")
                    }
                }
            }
            _ => bail!(
                e.span,
                "not a constant expression: const initializers allow literals, `const` names, arithmetic / bitwise / comparison operators, and `as` casts"
            ),
        }
    }

    /// The scalar value of the named const, resolving through its (possibly
    /// not-yet-visited) initializer. Memoized; `None` means the const exists
    /// but has no scalar value (a `str` const, a `#zero`, or an initializer
    /// that already failed) — or the name is unknown entirely.
    fn resolve_const_scalar(&mut self, name: &str, cx: &mut ConstCx) -> Option<CVal> {
        if let Some(done) = cx.resolved.get(name) {
            return *done;
        }
        let (value, ty, origin) = cx.raw.get(name)?.clone();
        if cx.visiting.iter().any(|n| n == name) {
            let chain: Vec<&str> = cx.visiting.iter().map(String::as_str).collect();
            self.err(
                "E0921",
                format!(
                    "const reference cycle: {} -> `{name}`",
                    chain
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(" -> ")
                ),
                value.span,
            );
            cx.resolved.insert(name.to_string(), None);
            return None;
        }
        let expected = cscalar_of_type(&ty);
        let prev_file = self.current_file.clone();
        self.set_current_file(origin.as_deref());
        cx.visiting.push(name.to_string());
        // A plain-literal initializer keeps the historical path: it is
        // probed QUIETLY for the cross-const environment (its own type
        // mismatches stay sema's E0302, exactly as before). Anything else
        // is a const expression: evaluated loudly at the declared type.
        let result = if is_const_initializer(&value) {
            match expected {
                Some(exp) => self.const_eval(&value, Some(exp), cx, true).ok(),
                None => None,
            }
        } else if let Some(exp) = expected {
            self.const_eval(&value, Some(exp), cx, false).ok()
        } else {
            // Non-scalar declared type with a non-literal initializer:
            // the historical E0911.
            self.err(
                "E0911",
                "const initializer must be a literal (integer, float, bool, string, unary-negated numeric literal, or `#zero::[T]()`) or a scalar constant expression".to_string(),
                value.span,
            );
            None
        };
        cx.visiting.pop();
        self.set_current_file(prev_file.as_deref());
        cx.resolved.insert(name.to_string(), result);
        result
    }
}

// ===== derive through the empty impl =====
//
// `impl Point: Eq {}` — an EMPTY impl block against one of the five
// derivable blessed interfaces (`Eq`, `Ord`, `Hash`, `Clone`, `ToText`)
// asks the compiler to generate the memberwise implementation. This pass
// synthesizes the method AST directly into the impl block before any other
// lowering, so generated code flows through named-arg lowering, sema,
// borrowck, monomorphize and codegen exactly like a hand-written impl —
// no sema or codegen surface knows derive exists.
//
// The empty body IS the request, extending the language's existing idiom
// (`impl Handle: Send {}` asserts a marker the same way). Empty impls
// against `Send`/`Sync` remain marker assertions (sema slice 7GEN.6);
// empty impls against user interfaces remain E0916.
//
// Per-interface field support (E0920 on anything outside it):
//   Eq     — primitives/str/ptrs via `==`, payload-free enums via `==`,
//            nominal types (structs / generic insts / type params) via `.eq()`
//   Ord    — numerics via `<`/`>`, bool + payload-free enums via `as i64`,
//            nominal via `.cmp()` fold, `str` via its blessed `compare`
//   Hash   — FNV-1a 64 fold: ints/str via blessed `.hash()`, floats via
//            `.to_bits()`, bool/enums via `as u64`, nominal via `.hash()`
//   Clone  — copyables verbatim, nominal via `.clone()`
//   ToText — one interpolated literal `"Name { f: ${...}, ... }"`; nominal
//            fields spelled `${this.f.to_text()}` (a bare struct part is
//            the open codegen gap found 2026-08-08), payload-free enums as
//            their `as i64` discriminant
//
// Every synthesized node carries the impl header's interface-name span, so
// any downstream diagnostic in generated code (a missing `T: Eq` bound, a
// field type without the needed method) points at the `impl X: Eq {}` line
// the user wrote.

/// The five interfaces the empty impl derives. `Send`/`Sync` are NOT here —
/// an empty impl against them is a marker assertion, not a derive.
const DERIVABLE: [&str; 5] = ["Eq", "Ord", "Hash", "Clone", "ToText"];

/// FNV-1a 64-bit offset basis / prime — the fold constants for derived `hash`.
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// What derive generation can do with one field, classified from the AST
/// type alone (plus the merged program's enum table). `Nominal` covers
/// structs, generic instantiations and type params uniformly: generation
/// emits a method call and sema enforces the method's existence (or the
/// `T: Eq`-style bound) at the impl header's span.
#[derive(Clone, Copy, PartialEq)]
enum DeriveFieldKind {
    Int,
    Float,
    Bool,
    Str,
    PlainEnum,
    PayloadEnum,
    Ptr,
    Nominal,
    Unsupported(&'static str),
}

fn classify_derive_field(
    ty: &Type,
    enums_payload_free: &std::collections::HashMap<String, bool>,
) -> DeriveFieldKind {
    match &ty.kind {
        TypeKind::Path(p) => match p.as_str() {
            "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
                DeriveFieldKind::Int
            }
            "f16" | "f32" | "f64" => DeriveFieldKind::Float,
            "bool" => DeriveFieldKind::Bool,
            "str" => DeriveFieldKind::Str,
            _ => match enums_payload_free.get(p) {
                Some(true) => DeriveFieldKind::PlainEnum,
                Some(false) => DeriveFieldKind::PayloadEnum,
                // Structs, type params, and anything sema will reject on its
                // own (an unknown name) all take the method-call route.
                None => DeriveFieldKind::Nominal,
            },
        },
        // A generic instantiation is nominal (Vec[T], Pair[A, B]) unless the
        // template is an enum (Option[i32], Result[T, E]) — those get the
        // targeted payload-enum diagnostic instead of a puzzling
        // no-method-on-enum error.
        TypeKind::Generic { name, .. } => match enums_payload_free.get(name) {
            Some(true) => DeriveFieldKind::PlainEnum,
            Some(false) => DeriveFieldKind::PayloadEnum,
            None => DeriveFieldKind::Nominal,
        },
        TypeKind::RawPtr(_) | TypeKind::FnPtr { .. } => DeriveFieldKind::Ptr,
        TypeKind::Array { .. } => DeriveFieldKind::Unsupported("array fields"),
        TypeKind::Slice(_) => DeriveFieldKind::Unsupported("slice fields"),
        TypeKind::Tuple(_) => DeriveFieldKind::Unsupported("tuple fields"),
        TypeKind::Borrowed { .. } => DeriveFieldKind::Unsupported("borrow fields"),
    }
}

// -- tiny AST builders, all stamped with the impl header's span --

fn d_ident(name: &str, span: Span) -> Ident {
    Ident {
        name: name.to_string(),
        span,
    }
}

fn d_expr(kind: ExprKind, span: Span) -> Expr {
    Expr { kind, span }
}

fn d_path_ty(name: &str, span: Span) -> Type {
    Type {
        kind: TypeKind::Path(name.to_string()),
        span,
    }
}

/// `this.f` / `other.f`
fn d_field(base: &str, field: &str, span: Span) -> Expr {
    d_expr(
        ExprKind::Field {
            receiver: Box::new(d_expr(ExprKind::Ident(base.to_string()), span)),
            name: d_ident(field, span),
        },
        span,
    )
}

/// `recv.method(args)`
fn d_mcall(recv: Expr, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    let arg_labels = vec![None; args.len()];
    d_expr(
        ExprKind::Call {
            callee: Box::new(d_expr(
                ExprKind::Field {
                    receiver: Box::new(recv),
                    name: d_ident(method, span),
                },
                span,
            )),
            args,
            arg_labels,
            type_args: Vec::new(),
        },
        span,
    )
}

fn d_binary(op: BinOp, lhs: Expr, rhs: Expr, span: Span) -> Expr {
    d_expr(
        ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    )
}

fn d_cast(e: Expr, ty_name: &str, span: Span) -> Expr {
    d_expr(
        ExprKind::Cast {
            expr: Box::new(e),
            ty: d_path_ty(ty_name, span),
        },
        span,
    )
}

fn d_int(v: u64, suffix: NumSuffix, span: Span) -> Expr {
    d_expr(ExprKind::IntLit(v, suffix), span)
}

/// `if COND { return RET; }` as a statement.
fn d_if_return(cond: Expr, ret: Expr, span: Span) -> Stmt {
    Stmt {
        kind: StmtKind::Expr(d_expr(
            ExprKind::If {
                cond: Box::new(cond),
                then: Block {
                    stmts: vec![Stmt {
                        kind: StmtKind::Return(Some(ret)),
                        span,
                    }],
                    tail: None,
                    span,
                },
                else_branch: None,
            },
            span,
        )),
        span,
    }
}

fn d_return(e: Expr, span: Span) -> Stmt {
    Stmt {
        kind: StmtKind::Return(Some(e)),
        span,
    }
}

fn d_method(
    name: &str,
    params: Vec<Param>,
    return_type: Type,
    stmts: Vec<Stmt>,
    span: Span,
) -> Method {
    Method {
        name: d_ident(name, span),
        generic_params: Vec::new(),
        receiver: Some(Receiver::Read),
        params,
        return_type: Some(return_type),
        body: Block {
            stmts,
            tail: None,
            span,
        },
        is_declaration: false,
        span,
        is_pub: false,
        attributes: Vec::new(),
        is_async: false,
        is_gen: false,
    }
}

/// The impl target spelled as a type: `Point`, or `Pair[T]` for a generic
/// target (the impl's own params become the type arguments).
fn d_self_type(target: &str, generic_params: &[GenericParam], span: Span) -> Type {
    if generic_params.is_empty() {
        d_path_ty(target, span)
    } else {
        Type {
            kind: TypeKind::Generic {
                name: target.to_string(),
                args: generic_params
                    .iter()
                    .map(|gp| d_path_ty(&gp.name.name, span))
                    .collect(),
            },
            span,
        }
    }
}

fn d_other_param(self_ty: Type, span: Span) -> Param {
    Param {
        name: d_ident("other", span),
        ty: self_ty,
        mutable: false,
        move_: false,
        restrict: false,
        borrow_: false,
        default: None,
        span,
    }
}

impl Lower {
    /// Expand every empty derivable-interface impl in the merged program.
    /// Runs before all other lowering steps so the generated methods are
    /// first-class citizens of every later pass.
    fn expand_derives(&mut self, prog: &mut Program) {
        // Pass 1 (read-only): the type tables generation consults.
        let mut struct_fields: std::collections::HashMap<String, Vec<(String, Type)>> =
            std::collections::HashMap::new();
        let mut enums_payload_free: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        let mut designated_string: Option<String> = None;
        for item in &prog.items {
            match &item.kind {
                ItemKind::Struct(s) => {
                    struct_fields.insert(
                        s.name.name.clone(),
                        s.fields
                            .iter()
                            .map(|f| (f.name.name.clone(), f.ty.clone()))
                            .collect(),
                    );
                    let is_lang_string = s.attributes.iter().any(|a| {
                        a.path.name == "lang"
                            && a.args
                                .iter()
                                .any(|arg| matches!(arg, AttrArg::Str(v, _) if v == "string"))
                    });
                    if is_lang_string {
                        designated_string = Some(s.name.name.clone());
                    }
                }
                ItemKind::Enum(e) => {
                    let payload_free = e.variants.iter().all(|v| v.payload.is_empty());
                    enums_payload_free.insert(e.name.name.clone(), payload_free);
                }
                _ => {}
            }
        }
        // Pass 2: rewrite matching impl blocks in place.
        for item in &mut prog.items {
            let origin = item.origin_file.clone();
            let ItemKind::Impl(b) = &mut item.kind else {
                continue;
            };
            if !b.methods.is_empty() {
                continue;
            }
            let Some(iface) = b.interface_name.clone() else {
                continue;
            };
            if !DERIVABLE.contains(&iface.name.as_str()) {
                continue;
            }
            // Unknown target or an enum target: leave the impl empty — sema's
            // existing E0325/E0916 diagnostics own those shapes.
            let Some(fields) = struct_fields.get(&b.target.name).cloned() else {
                continue;
            };
            self.set_current_file(origin.as_deref());
            let span = iface.span;
            let target = b.target.name.clone();
            let kinds: Vec<(String, DeriveFieldKind)> = fields
                .iter()
                .map(|(n, t)| (n.clone(), classify_derive_field(t, &enums_payload_free)))
                .collect();
            let mut bad = |this: &mut Self, fname: &str, why: &str| {
                this.err(
                    "E0920",
                    format!(
                        "cannot derive `{}` for `{}`: field `{}` — {}",
                        iface.name,
                        target.rsplit('.').next().unwrap_or(&target),
                        fname,
                        why
                    ),
                    span,
                );
            };
            let method = match iface.name.as_str() {
                "Eq" => {
                    let mut stmts = Vec::new();
                    for (fname, kind) in &kinds {
                        let cond = match kind {
                            DeriveFieldKind::Int
                            | DeriveFieldKind::Float
                            | DeriveFieldKind::Bool
                            | DeriveFieldKind::Str
                            | DeriveFieldKind::PlainEnum
                            | DeriveFieldKind::Ptr => Some(d_binary(
                                BinOp::Ne,
                                d_field("self", fname, span),
                                d_field("other", fname, span),
                                span,
                            )),
                            DeriveFieldKind::Nominal => Some(d_expr(
                                ExprKind::Unary {
                                    op: UnaryOp::Not,
                                    operand: Box::new(d_mcall(
                                        d_field("self", fname, span),
                                        "eq",
                                        vec![d_field("other", fname, span)],
                                        span,
                                    )),
                                },
                                span,
                            )),
                            DeriveFieldKind::PayloadEnum => {
                                bad(self, fname, "its enum type has payload variants; write `eq` manually");
                                None
                            }
                            DeriveFieldKind::Unsupported(what) => {
                                bad(self, fname, &format!("{what} are not derivable"));
                                None
                            }
                        };
                        if let Some(cond) = cond {
                            stmts.push(d_if_return(
                                cond,
                                d_expr(ExprKind::BoolLit(false), span),
                                span,
                            ));
                        }
                    }
                    stmts.push(d_return(d_expr(ExprKind::BoolLit(true), span), span));
                    d_method(
                        "eq",
                        vec![d_other_param(
                            d_self_type(&target, &b.target_generic_params, span),
                            span,
                        )],
                        d_path_ty("bool", span),
                        stmts,
                        span,
                    )
                }
                "Ord" => {
                    let mut stmts = Vec::new();
                    for (i, (fname, kind)) in kinds.iter().enumerate() {
                        match kind {
                            DeriveFieldKind::Int | DeriveFieldKind::Float => {
                                let minus_one = d_expr(
                                    ExprKind::Unary {
                                        op: UnaryOp::Neg,
                                        operand: Box::new(d_int(1, NumSuffix::None, span)),
                                    },
                                    span,
                                );
                                stmts.push(d_if_return(
                                    d_binary(
                                        BinOp::Lt,
                                        d_field("self", fname, span),
                                        d_field("other", fname, span),
                                        span,
                                    ),
                                    minus_one,
                                    span,
                                ));
                                stmts.push(d_if_return(
                                    d_binary(
                                        BinOp::Gt,
                                        d_field("self", fname, span),
                                        d_field("other", fname, span),
                                        span,
                                    ),
                                    d_int(1, NumSuffix::None, span),
                                    span,
                                ));
                            }
                            DeriveFieldKind::Bool | DeriveFieldKind::PlainEnum => {
                                let lhs = d_cast(d_field("self", fname, span), "i64", span);
                                let rhs = d_cast(d_field("other", fname, span), "i64", span);
                                let minus_one = d_expr(
                                    ExprKind::Unary {
                                        op: UnaryOp::Neg,
                                        operand: Box::new(d_int(1, NumSuffix::None, span)),
                                    },
                                    span,
                                );
                                stmts.push(d_if_return(
                                    d_binary(BinOp::Lt, lhs.clone(), rhs.clone(), span),
                                    minus_one,
                                    span,
                                ));
                                stmts.push(d_if_return(
                                    d_binary(BinOp::Gt, lhs, rhs, span),
                                    d_int(1, NumSuffix::None, span),
                                    span,
                                ));
                            }
                            // `str` folds through its blessed lexicographic
                            // `compare` (stdlib str.cplus); nominal types
                            // through their own `cmp`. Same three-way fold.
                            DeriveFieldKind::Nominal | DeriveFieldKind::Str => {
                                let method = if *kind == DeriveFieldKind::Str {
                                    "compare"
                                } else {
                                    "cmp"
                                };
                                let cname = format!("c{i}");
                                stmts.push(Stmt {
                                    kind: StmtKind::Let {
                                        mutable: false,
                                        name: d_ident(&cname, span),
                                        ty: None,
                                        init: Some(d_mcall(
                                            d_field("self", fname, span),
                                            method,
                                            vec![d_field("other", fname, span)],
                                            span,
                                        )),
                                    },
                                    span,
                                });
                                stmts.push(d_if_return(
                                    d_binary(
                                        BinOp::Ne,
                                        d_expr(ExprKind::Ident(cname.clone()), span),
                                        d_int(0, NumSuffix::None, span),
                                        span,
                                    ),
                                    d_expr(ExprKind::Ident(cname), span),
                                    span,
                                ));
                            }
                            DeriveFieldKind::Ptr => {
                                bad(self, fname, "pointer fields have no ordering; write `cmp` manually");
                            }
                            DeriveFieldKind::PayloadEnum => {
                                bad(self, fname, "its enum type has payload variants; write `cmp` manually");
                            }
                            DeriveFieldKind::Unsupported(what) => {
                                bad(self, fname, &format!("{what} are not derivable"));
                            }
                        }
                    }
                    stmts.push(d_return(d_int(0, NumSuffix::None, span), span));
                    d_method(
                        "cmp",
                        vec![d_other_param(
                            d_self_type(&target, &b.target_generic_params, span),
                            span,
                        )],
                        d_path_ty("i32", span),
                        stmts,
                        span,
                    )
                }
                "Hash" => {
                    let mut stmts = vec![Stmt {
                        kind: StmtKind::Let {
                            mutable: true,
                            name: d_ident("h", span),
                            ty: Some(d_path_ty("u64", span)),
                            init: Some(d_int(FNV_OFFSET, NumSuffix::U64, span)),
                        },
                        span,
                    }];
                    for (fname, kind) in &kinds {
                        let fh = match kind {
                            DeriveFieldKind::Int | DeriveFieldKind::Str => {
                                Some(d_mcall(d_field("self", fname, span), "hash", vec![], span))
                            }
                            DeriveFieldKind::Float => Some(d_cast(
                                d_mcall(d_field("self", fname, span), "to_bits", vec![], span),
                                "u64",
                                span,
                            )),
                            DeriveFieldKind::Bool | DeriveFieldKind::PlainEnum => {
                                Some(d_cast(d_field("self", fname, span), "u64", span))
                            }
                            DeriveFieldKind::Nominal => {
                                Some(d_mcall(d_field("self", fname, span), "hash", vec![], span))
                            }
                            DeriveFieldKind::Ptr => {
                                bad(self, fname, "pointer fields have no blessed `hash`; write `hash` manually");
                                None
                            }
                            DeriveFieldKind::PayloadEnum => {
                                bad(self, fname, "its enum type has payload variants; write `hash` manually");
                                None
                            }
                            DeriveFieldKind::Unsupported(what) => {
                                bad(self, fname, &format!("{what} are not derivable"));
                                None
                            }
                        };
                        if let Some(fh) = fh {
                            let h = d_expr(ExprKind::Ident("h".to_string()), span);
                            let folded = d_binary(
                                BinOp::Mul,
                                d_binary(BinOp::BitXor, h.clone(), fh, span),
                                d_int(FNV_PRIME, NumSuffix::U64, span),
                                span,
                            );
                            stmts.push(Stmt {
                                kind: StmtKind::Expr(d_expr(
                                    ExprKind::Assign {
                                        op: AssignOp::Assign,
                                        target: Box::new(h),
                                        value: Box::new(folded),
                                    },
                                    span,
                                )),
                                span,
                            });
                        }
                    }
                    stmts.push(d_return(d_expr(ExprKind::Ident("h".to_string()), span), span));
                    d_method("hash", Vec::new(), d_path_ty("u64", span), stmts, span)
                }
                "Clone" => {
                    let mut ok = true;
                    let mut lit_fields = Vec::new();
                    for (fname, kind) in &kinds {
                        let value = match kind {
                            DeriveFieldKind::Int
                            | DeriveFieldKind::Float
                            | DeriveFieldKind::Bool
                            | DeriveFieldKind::Str
                            | DeriveFieldKind::PlainEnum
                            | DeriveFieldKind::Ptr => d_field("self", fname, span),
                            DeriveFieldKind::Nominal => {
                                d_mcall(d_field("self", fname, span), "clone", vec![], span)
                            }
                            DeriveFieldKind::PayloadEnum => {
                                bad(self, fname, "its enum type has payload variants; write `clone` manually");
                                ok = false;
                                continue;
                            }
                            DeriveFieldKind::Unsupported(what) => {
                                bad(self, fname, &format!("{what} are not derivable"));
                                ok = false;
                                continue;
                            }
                        };
                        lit_fields.push(StructLitField {
                            name: d_ident(fname, span),
                            value,
                            span,
                        });
                    }
                    // A failed field means the struct literal can't be built;
                    // emit a self-recursive body so the (already failing)
                    // build doesn't cascade a missing-field error on top.
                    let ret = if ok {
                        if b.target_generic_params.is_empty() {
                            d_expr(
                                ExprKind::StructLit {
                                    name: d_ident(&target, span),
                                    fields: lit_fields,
                                },
                                span,
                            )
                        } else {
                            d_expr(
                                ExprKind::GenericStructLit {
                                    name: d_ident(&target, span),
                                    type_args: b
                                        .target_generic_params
                                        .iter()
                                        .map(|gp| d_path_ty(&gp.name.name, span))
                                        .collect(),
                                    fields: lit_fields,
                                },
                                span,
                            )
                        }
                    } else {
                        d_mcall(
                            d_expr(ExprKind::Ident("self".to_string()), span),
                            "clone",
                            vec![],
                            span,
                        )
                    };
                    d_method(
                        "clone",
                        Vec::new(),
                        d_self_type(&target, &b.target_generic_params, span),
                        vec![d_return(ret, span)],
                        span,
                    )
                }
                "ToText" => {
                    let Some(text_name) = designated_string.clone() else {
                        self.err(
                            "E0920",
                            format!(
                                "cannot derive `ToText` for `{}`: the build has no `#[lang(\"string\")]` type — import the stdlib `text` module",
                                target.rsplit('.').next().unwrap_or(&target)
                            ),
                            span,
                        );
                        self.set_current_file(None);
                        continue;
                    };
                    let leaf = target.rsplit('.').next().unwrap_or(&target).to_string();
                    let mut parts = Vec::new();
                    let mut first = true;
                    for (fname, kind) in &kinds {
                        let part = match kind {
                            DeriveFieldKind::Int
                            | DeriveFieldKind::Float
                            | DeriveFieldKind::Bool
                            | DeriveFieldKind::Str => Some(d_field("self", fname, span)),
                            DeriveFieldKind::PlainEnum => {
                                Some(d_cast(d_field("self", fname, span), "i64", span))
                            }
                            DeriveFieldKind::Nominal => Some(d_mcall(
                                d_field("self", fname, span),
                                "to_text",
                                vec![],
                                span,
                            )),
                            DeriveFieldKind::Ptr => {
                                bad(self, fname, "pointer fields have no text form; write `to_text` manually");
                                None
                            }
                            DeriveFieldKind::PayloadEnum => {
                                bad(self, fname, "its enum type has payload variants; write `to_text` manually");
                                None
                            }
                            DeriveFieldKind::Unsupported(what) => {
                                bad(self, fname, &format!("{what} are not derivable"));
                                None
                            }
                        };
                        let Some(part) = part else { continue };
                        let lead = if first {
                            format!("{leaf} {{ {fname}: ")
                        } else {
                            format!(", {fname}: ")
                        };
                        first = false;
                        parts.push(InterpStrPart::Lit(lead));
                        parts.push(InterpStrPart::Expr(Box::new(part)));
                    }
                    parts.push(InterpStrPart::Lit(if first {
                        format!("{leaf} {{}}")
                    } else {
                        " }".to_string()
                    }));
                    d_method(
                        "to_text",
                        Vec::new(),
                        d_path_ty(&text_name, span),
                        vec![d_return(d_expr(ExprKind::InterpStr { parts }, span), span)],
                        span,
                    )
                }
                _ => unreachable!("DERIVABLE covers the match"),
            };
            b.methods.push(method);
            self.set_current_file(None);
        }
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

    // GAP 3 (v0.0.19): a lower-pass diagnostic (here E0911 on a bad static
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
    fn interface_method_call_not_shadowed_by_same_named_impl_method() {
        // A no-arg call through an interface-bounded generic must resolve to the
        // interface method, not a same-named `impl` method on another type that
        // takes an argument. Before interface decls were registered as call
        // candidates, `t.size()` saw only `Widget::size(v)` and reported a bogus
        // E0308 "missing argument for parameter `v`" — the exact shape that broke
        // facet's `run[W: Window]` calling `owned_window.width()` when no concrete
        // Window implementor was present in the build.
        let src = "\
interface Measured { fn size(this) -> i64; }\n\
struct Widget { w: i64 }\n\
impl Widget { fn size(take this, v: i64) -> Widget { var n: Widget = this; n.w = v; return n; } }\n\
fn measure[T: Measured](take t: T) -> i64 { return t.size(); }\n\
fn main() -> i32 { return 0; }\n";
        let (_prog, diags) = run(src);
        assert!(
            !first_codes(&diags).contains(&"E0308"),
            "interface method call mis-resolved to the arg-taking impl overload: {:?}",
            first_codes(&diags)
        );
    }

    #[test]
    fn multi_file_static_init_error_points_at_origin_file_gap3() {
        let entry_src = "fn main() -> i32 { return 0; }\n";
        // A call is not a constant expression (arithmetic now folds).
        let lib_src = "// lib header\nstatic BAD: i32 = frob();\n";
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
            .find(|d| d.code.0 == "E0921")
            .expect("expected E0921 on the bad static initializer");
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
        // E0912 (unknown const array length) raised in the array-length pass
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
            .find(|d| d.code.0 == "E0912")
            .expect("expected E0912 on the unknown array length");
        assert!(
            d.primary.file.ends_with("lib.cplus"),
            "diagnostic should point at lib.cplus, got {:?}",
            d.primary.file
        );
    }

    #[test]
    fn single_file_static_init_error_unchanged_gap3() {
        // The single-file `lower` entry still renders against the one file.
        // (A call initializer — arithmetic in a scalar static folds now.)
        let (_, diags) = run("static BAD: i32 = frob();\nfn main() -> i32 { return 0; }");
        let d = diags
            .iter()
            .find(|d| d.code.0 == "E0921")
            .expect("expected E0921");
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

    // ---- pattern-binding `var` forms: `guard var` / `if var` / `while var` ----

    fn main_body(prog: &Program) -> &Block {
        prog.items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Function(f) if f.name.name == "main" => Some(&f.body),
                _ => None,
            })
            .expect("main fn")
    }

    #[test]
    fn guard_var_lowers_to_mutable_let() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                guard var Maybe::Some(v) = m else { return 0; };
                return v;
            }
        "#;
        let (prog, diags) = run(src);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        // The guard-var becomes `var v = match ...;` — same rewrite as
        // guard-let with a mutable head.
        match &main_body(&prog).stmts[1].kind {
            StmtKind::Let {
                mutable,
                name,
                init: Some(_),
                ..
            } => {
                assert!(*mutable, "guard var must synthesize a mutable let");
                assert_eq!(name.name, "v");
            }
            other => panic!("expected let, got {other:?}"),
        }
    }

    /// Dig the success-arm rebind out of a lowered `if var` / `while var`
    /// match: the arm pattern's binding must be renamed to a `__var` temp
    /// and the arm body must open with `var NAME = TEMP;`.
    fn assert_success_arm_rebinds(match_expr: &Expr, user_name: &str) {
        let ExprKind::Match { arms, .. } = &match_expr.kind else {
            panic!("expected match, got {:?}", match_expr.kind);
        };
        let PatternKind::Variant { payload, .. } = &arms[0].pattern.kind else {
            panic!("expected variant pattern");
        };
        let PatternKind::Binding(temp) = &payload[0].kind else {
            panic!("expected binding in payload");
        };
        assert!(
            temp.name.starts_with("__var"),
            "arm binding should be a fresh temp, got `{}`",
            temp.name
        );
        let ExprKind::Block(body) = &arms[0].body.kind else {
            panic!("expected block arm body");
        };
        match &body.stmts[0].kind {
            StmtKind::Let {
                mutable,
                name,
                init: Some(init),
                ..
            } => {
                assert!(*mutable);
                assert_eq!(name.name, user_name);
                assert!(
                    matches!(&init.kind, ExprKind::Ident(n) if n == &temp.name),
                    "rebind must read the renamed arm temp"
                );
            }
            other => panic!("expected rebind let, got {other:?}"),
        }
    }

    #[test]
    fn if_var_renames_binding_and_prepends_rebind() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                if var Maybe::Some(v) = m { return v; }
                return 0;
            }
        "#;
        let (prog, diags) = run(src);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        let StmtKind::Expr(match_expr) = &main_body(&prog).stmts[1].kind else {
            panic!("expected lowered match stmt");
        };
        assert_success_arm_rebinds(match_expr, "v");
    }

    #[test]
    fn while_var_renames_binding_and_prepends_rebind() {
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                while var Maybe::Some(v) = next() { break; }
                return 0;
            }
            fn next() -> Maybe { return Maybe::None; }
        "#;
        let (prog, diags) = run(src);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        // while-var lowers to `loop { match ... }`.
        let StmtKind::Loop(loop_body, _) = &main_body(&prog).stmts[0].kind else {
            panic!("expected lowered loop stmt");
        };
        let StmtKind::Expr(match_expr) = &loop_body.stmts[0].kind else {
            panic!("expected match inside loop");
        };
        assert_success_arm_rebinds(match_expr, "v");
    }

    #[test]
    fn guard_var_keeps_guard_diagnostics() {
        // The var spelling goes through the same E0348/E0351 checks.
        let src = r#"
            enum Maybe { Some(i32), None }
            fn main() -> i32 {
                let m: Maybe = Maybe::Some(7);
                guard var Maybe::Some(v) = m else { let x: i32 = 1; };
                return v;
            }
        "#;
        let (_, diags) = run(src);
        assert!(first_codes(&diags).contains(&"E0348"));
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
        assert!(!first_codes(&diags).contains(&"E0912"), "diags: {diags:?}");
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
    fn unknown_const_array_length_e0912() {
        let (_, diags) = run("fn main() -> i32 { let a: [i32; NOPE] = [0; 1]; return a[0]; }");
        assert!(first_codes(&diags).contains(&"E0912"), "diags: {diags:?}");
    }

    #[test]
    fn non_integer_const_array_length_e0912() {
        let (_, diags) = run("const NAME: str = \"hi\";\n\
             fn main() -> i32 { let a: [i32; NAME] = [0; 1]; return 0; }");
        assert!(first_codes(&diags).contains(&"E0912"), "diags: {diags:?}");
    }

    #[test]
    fn const_array_length_in_struct_field_folds() {
        // A const length used in a struct field type resolves too.
        let (prog, diags) = run("const W: u32 = 16;\n\
             struct Buf { data: [u8; W] }\n\
             fn main() -> i32 { return 0; }");
        assert!(!first_codes(&diags).contains(&"E0912"), "diags: {diags:?}");
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
            item_name.name.starts_with("__builder_item"),
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
        // __i = __i.pad(3);  — a non-`set_` modifier is a `take self -> Node`
        // builder, so its result threads back into the item temp (this is what
        // lets builders and `.set_*` mutators compose in one chain).
        let StmtKind::Expr(reassign) = &b.stmts[3].kind else {
            panic!("expected builder reassign stmt, got {:?}", b.stmts[3].kind);
        };
        let ExprKind::Assign {
            op: AssignOp::Assign,
            target: reassign_target,
            value: reassign_value,
        } = &reassign.kind
        else {
            panic!("expected reassign of the item temp, got {:?}", reassign.kind);
        };
        assert!(
            matches!(&reassign_target.kind, ExprKind::Ident(n) if n.starts_with("__builder_item")),
            "reassign target is the item temp"
        );
        let ExprKind::Call { callee: pad_callee, .. } = &reassign_value.kind else {
            panic!("expected `.pad(3)` call on the RHS, got {:?}", reassign_value.kind);
        };
        let ExprKind::Field { name: pad_m, .. } = &pad_callee.kind else {
            panic!("expected method callee for `.pad`");
        };
        assert_eq!(pad_m.name, "pad");
        // then __b.add(__i);
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
    fn builder_container_with_args_desugars_builder_first() {
        // DSL.5: `card(title: t, 2) { ... }` finishes as
        // `card(__b, title: t, 2)` — the Builder stays the first argument
        // and the user's labels shift right by one (position 0 unlabeled).
        let src = "fn main() -> i32 {\n    let v = @view {\n        card(title: t, 2) {\n            text(1)\n        }\n    };\n    return 0;\n}\n";
        let b = desugared_builder(src);
        let StmtKind::Let {
            init: Some(inner), ..
        } = &b.stmts[1].kind
        else {
            panic!("expected container item let");
        };
        let ExprKind::Block(inner) = &inner.kind else {
            panic!("container must desugar to a Block, got {:?}", inner.kind);
        };
        let tail = inner.tail.as_ref().expect("container finisher tail");
        let ExprKind::Call {
            callee,
            args,
            arg_labels,
            ..
        } = &tail.kind
        else {
            panic!("container tail must be a call, got {:?}", tail.kind);
        };
        let ExprKind::Path { segments } = &callee.kind else {
            panic!("constructor must be a path, got {:?}", callee.kind);
        };
        assert_eq!(segments.last().unwrap().name, "card");
        assert_eq!(args.len(), 3, "builder + the two user args");
        assert!(
            matches!(args[0].kind, ExprKind::Ident(ref n) if n.starts_with("__b")),
            "the Builder is the first argument"
        );
        assert_eq!(arg_labels.len(), 3, "labels align with args");
        assert!(arg_labels[0].is_none(), "the builder slot is positional");
        assert_eq!(arg_labels[1].as_ref().unwrap().name, "title");
        assert!(arg_labels[2].is_none());
    }

    #[test]
    fn builder_container_with_positional_args_keeps_labels_empty() {
        // All-positional user args preserve the Call invariant: an empty
        // label vec, not a vec of Nones.
        let src = "fn main() -> i32 {\n    let v = @view {\n        card(2) {\n            text(1)\n        }\n    };\n    return 0;\n}\n";
        let b = desugared_builder(src);
        let StmtKind::Let {
            init: Some(inner), ..
        } = &b.stmts[1].kind
        else {
            panic!("expected container item let");
        };
        let ExprKind::Block(inner) = &inner.kind else {
            panic!("container must desugar to a Block");
        };
        let tail = inner.tail.as_ref().expect("container finisher tail");
        let ExprKind::Call {
            args, arg_labels, ..
        } = &tail.kind
        else {
            panic!("container tail must be a call");
        };
        assert_eq!(args.len(), 2, "builder + one positional arg");
        assert!(arg_labels.is_empty(), "no labels anywhere stays empty");
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

    // ---- const expressions ----

    #[test]
    fn array_length_expression_folds_in_type_and_fill() {
        let (prog, diags) = run(
            "const CAP: usize = 1024;\n\
             fn main() -> i32 { let a: [u8; CAP * 2] = [7u8; CAP * 2]; return a[0] as i32; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        let ItemKind::Function(f) = &prog.items[1].kind else {
            panic!("expected fn");
        };
        let StmtKind::Let { ty, init, .. } = &f.body.stmts[0].kind else {
            panic!("expected let");
        };
        let TypeKind::Array { len, len_expr, .. } = &ty.as_ref().unwrap().kind else {
            panic!("expected array type");
        };
        assert_eq!(*len, 2048);
        assert!(len_expr.is_none(), "len_expr must be folded away");
        let ExprKind::ArrayFill {
            count, count_expr, ..
        } = &init.as_ref().unwrap().kind
        else {
            panic!("expected fill");
        };
        assert_eq!(*count, 2048);
        assert!(count_expr.is_none(), "count_expr must be folded away");
    }

    #[test]
    fn array_length_expression_unknown_name_e0921() {
        let (_prog, diags) = run(
            "fn main() -> i32 { let a: [u8; NOPE * 2] = [0u8; 4]; return 0; }",
        );
        assert!(
            diags.iter().any(|d| d.code.0 == "E0921"),
            "got {diags:#?}"
        );
    }

    #[test]
    fn const_expression_initializer_folds_to_suffixed_literal() {
        let (prog, diags) = run(
            "const MASK: u64 = (1u64 << 40) - 1u64;\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        let ItemKind::Const(c) = &prog.items[0].kind else {
            panic!("expected const");
        };
        assert!(
            matches!(&c.value.kind, ExprKind::IntLit(v, NumSuffix::U64) if *v == (1u64 << 40) - 1),
            "expected folded u64 literal, got {:?}",
            c.value.kind
        );
    }

    #[test]
    fn multi_file_array_length_lens_qualifies_2026_08_08() {
        // The resolver qualifies const DECLARATIONS but never rewrote the
        // `[T; CONST]` lens, so every multi-file binary build missed the
        // table (E0912 "not a known const"). The lens now rides
        // resolve_item_name like other references.
        let entry_src = "fn main() -> i32 { return 0; }\n";
        let lib_src = "const CAP: usize = 8;\n\
                       struct Buf { data: [i32; CAP], extra: [i32; CAP * 2] }\n";
        let (mut prog, files, entry_path) = merge_two_files(
            "main",
            "/proj/main.cplus",
            entry_src,
            "lib",
            "/proj/lib.cplus",
            lib_src,
        );
        let diags = lower_multi(&mut prog, &entry_path, entry_src, files);
        assert!(diags.is_empty(), "got {diags:#?}");
        let lens: Vec<u32> = prog
            .items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Struct(s) if s.name.name.ends_with("Buf") => Some(
                    s.fields
                        .iter()
                        .map(|f| match &f.ty.kind {
                            TypeKind::Array { len, .. } => *len,
                            _ => panic!("expected array field"),
                        })
                        .collect(),
                ),
                _ => None,
            })
            .expect("Buf struct");
        assert_eq!(lens, vec![8, 16]);
    }

    // ---- derive through the empty impl ----

    /// The impl block for `target: iface` in the lowered program.
    fn find_impl<'a>(prog: &'a Program, target: &str, iface: &str) -> &'a ImplBlock {
        prog.items
            .iter()
            .find_map(|it| match &it.kind {
                ItemKind::Impl(b)
                    if b.target.name == target
                        && b.interface_name.as_ref().map(|i| i.name.as_str()) == Some(iface) =>
                {
                    Some(b)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no impl {target}: {iface} in program"))
    }

    #[test]
    fn derive_eq_expands_memberwise_method() {
        let (prog, diags) = run(
            "struct P { x: i32, y: bool }\n\
             impl P: Eq {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        let b = find_impl(&prog, "P", "Eq");
        assert_eq!(b.methods.len(), 1);
        let m = &b.methods[0];
        assert_eq!(m.name.name, "eq");
        assert_eq!(m.receiver, Some(Receiver::Read));
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name.name, "other");
        assert!(matches!(&m.params[0].ty.kind, TypeKind::Path(p) if p == "P"));
        assert!(matches!(&m.return_type.as_ref().unwrap().kind, TypeKind::Path(p) if p == "bool"));
        // Two fields -> two early-return checks + the final `return true`.
        assert_eq!(m.body.stmts.len(), 3);
    }

    #[test]
    fn derive_all_five_expand() {
        let (prog, diags) = run(
            "#[lang(\"string\")] struct Text { p: *u8, n: usize }\n\
             struct P { x: i32 }\n\
             impl P: Eq {}\n\
             impl P: Ord {}\n\
             impl P: Hash {}\n\
             impl P: Clone {}\n\
             impl P: ToText {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        for (iface, method) in [
            ("Eq", "eq"),
            ("Ord", "cmp"),
            ("Hash", "hash"),
            ("Clone", "clone"),
            ("ToText", "to_text"),
        ] {
            let b = find_impl(&prog, "P", iface);
            assert_eq!(b.methods.len(), 1, "impl P: {iface}");
            assert_eq!(b.methods[0].name.name, method);
        }
    }

    #[test]
    fn derive_hash_folds_fnv1a() {
        let (prog, diags) = run(
            "struct P { x: i32 }\n\
             impl P: Hash {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        let m = &find_impl(&prog, "P", "Hash").methods[0];
        // var h: u64 = FNV_OFFSET; fold x; return h
        let StmtKind::Let { mutable, init, .. } = &m.body.stmts[0].kind else {
            panic!("expected let h");
        };
        assert!(*mutable);
        assert!(matches!(
            &init.as_ref().unwrap().kind,
            ExprKind::IntLit(v, NumSuffix::U64) if *v == FNV_OFFSET
        ));
        assert_eq!(m.body.stmts.len(), 3);
    }

    #[test]
    fn derive_generic_target_carries_params() {
        let (prog, diags) = run(
            "struct Pair[T] { a: T, b: i32 }\n\
             impl Pair[T: Eq]: Eq {}\n\
             impl Pair[T: Clone]: Clone {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        let eq = &find_impl(&prog, "Pair", "Eq").methods[0];
        // `other` is typed at the impl's own params: Pair[T].
        let TypeKind::Generic { name, args } = &eq.params[0].ty.kind else {
            panic!("expected Pair[T] param type");
        };
        assert_eq!(name, "Pair");
        assert!(matches!(&args[0].kind, TypeKind::Path(p) if p == "T"));
        // Clone rebuilds through the generic struct literal.
        let clone = &find_impl(&prog, "Pair", "Clone").methods[0];
        let StmtKind::Return(Some(ret)) = &clone.body.stmts[0].kind else {
            panic!("expected return");
        };
        assert!(matches!(&ret.kind, ExprKind::GenericStructLit { name, .. } if name.name == "Pair"));
    }

    #[test]
    fn derive_ord_str_field_uses_blessed_compare() {
        let (prog, diags) = run(
            "struct K { id: i64, name: str }\n\
             impl K: Ord {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        let m = &find_impl(&prog, "K", "Ord").methods[0];
        let has_compare_call = m.body.stmts.iter().any(|s| {
            let StmtKind::Let { init: Some(e), .. } = &s.kind else {
                return false;
            };
            let ExprKind::Call { callee, .. } = &e.kind else {
                return false;
            };
            matches!(&callee.kind, ExprKind::Field { name, .. } if name.name == "compare")
        });
        assert!(has_compare_call, "str field should fold through `compare`");
    }

    #[test]
    fn derive_payload_enum_field_rejected_e0920() {
        let (_prog, diags) = run(
            "enum E { A, B(i32) }\n\
             struct P { e: E }\n\
             impl P: Eq {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            diags.iter().any(|d| d.code.0 == "E0920"),
            "got {diags:#?}"
        );
    }

    #[test]
    fn derive_plain_enum_field_is_supported() {
        let (prog, diags) = run(
            "enum E { A, B }\n\
             struct P { e: E }\n\
             impl P: Eq {}\n\
             impl P: Ord {}\n\
             impl P: Hash {}\n\
             impl P: Clone {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        assert_eq!(find_impl(&prog, "P", "Eq").methods.len(), 1);
    }

    #[test]
    fn derive_totext_without_lang_string_rejected_e0920() {
        let (_prog, diags) = run(
            "struct P { x: i32 }\n\
             impl P: ToText {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            diags.iter().any(|d| d.code.0 == "E0920"),
            "got {diags:#?}"
        );
    }

    #[test]
    fn derive_leaves_markers_user_interfaces_and_enum_targets_alone() {
        let (prog, diags) = run(
            "interface Greet { fn hi(this) -> i32; }\n\
             enum E { A, B }\n\
             struct S { opaque p: *u8 }\n\
             impl S { fn drop(ref this) { return; } }\n\
             impl S: Send {}\n\
             impl S: Greet {}\n\
             impl E: Eq {}\n\
             fn main() -> i32 { return 0; }",
        );
        // No E0920 from lower — sema owns these shapes (E0915/E0916/E0325).
        assert!(
            diags.iter().all(|d| d.code.0 != "E0920"),
            "got {diags:#?}"
        );
        assert!(find_impl(&prog, "S", "Send").methods.is_empty());
        assert!(find_impl(&prog, "S", "Greet").methods.is_empty());
        assert!(find_impl(&prog, "E", "Eq").methods.is_empty());
    }

    #[test]
    fn derive_totext_interpolates_fields() {
        let (prog, diags) = run(
            "#[lang(\"string\")] struct Text { p: *u8, n: usize }\n\
             struct P { x: i32, inner: Q }\n\
             struct Q { v: i32 }\n\
             impl Q: ToText {}\n\
             impl P: ToText {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(diags.is_empty(), "got {diags:#?}");
        let m = &find_impl(&prog, "P", "ToText").methods[0];
        let StmtKind::Return(Some(ret)) = &m.body.stmts[0].kind else {
            panic!("expected return");
        };
        let ExprKind::InterpStr { parts } = &ret.kind else {
            panic!("expected interpolated string");
        };
        assert!(matches!(&parts[0], InterpStrPart::Lit(l) if l == "P { x: "));
        // The nominal field is spelled `${this.inner.to_text()}`.
        let has_to_text_call = parts.iter().any(|p| {
            let InterpStrPart::Expr(e) = p else { return false };
            let ExprKind::Call { callee, .. } = &e.kind else {
                return false;
            };
            matches!(&callee.kind, ExprKind::Field { name, .. } if name.name == "to_text")
        });
        assert!(has_to_text_call, "nested field should call to_text");
    }
}
