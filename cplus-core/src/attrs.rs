//! Phase 5 slice 5ATTR.1 — attribute validation pass.
//!
//! Runs after parsing, before lower / sema. Walks every collected
//! `Attribute` on every item-bearing AST node and verifies it against
//! the known-attribute spec:
//!
//! - Unknown name → **E0354** (with a did-you-mean suggestion).
//! - Bad target (e.g. `#[test]` on a struct) → **E0356**.
//! - Bad argument shape → **E0355**.
//! - Duplicate where uniqueness is required → **E0357**.
//!
//! Phase 5 ships one attribute: `#[test]`. New attributes drop a row into
//! `KNOWN_ATTRS` along with their own design note (per plan.md §2.8d).
//! The validator returns a flat `Vec<Diagnostic>`; the driver fails the
//! pipeline when any diagnostic carries `Severity::Error`.
//!
//! Sema-level rules for `#[test]` functions (signature, `pub` rejection,
//! `impl`-placement rejection — E0358/E0359/E0360) live in sema where the
//! type info is available. This pass only enforces the surface-level
//! attribute-shape rules.

use crate::ast::*;
use crate::diagnostics::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

const TARGET_FN: u8 = 0b0_0000_0001;
const TARGET_METHOD: u8 = 0b0_0000_0010;
const TARGET_STRUCT: u8 = 0b0_0000_0100;
const TARGET_ENUM: u8 = 0b0_0000_1000;
const TARGET_FIELD: u8 = 0b0_0001_0000;
const TARGET_VARIANT: u8 = 0b0_0010_0000;
/// v0.0.7 Slice 1.3: attribute on a loop statement (`while`, `loop`,
/// `for`). Used by `#[unroll(N)]` / `#[vectorize_width(N)]`.
const TARGET_LOOP_STMT: u8 = 0b0_0100_0000;

enum ArgsSpec {
    /// `#[name]` — no args allowed.
    None,
    /// `#[name(VAL)]` — exactly one ident arg from a fixed allow-list.
    /// Used by `#[repr(C)]` (slice 10.FFI.5).
    OneIdentFrom(&'static [&'static str]),
    /// `#[name]` or `#[name(VAL)]` — zero args, or exactly one ident arg
    /// from a fixed allow-list. Used by `#[inline]` / `#[inline(always)]` /
    /// `#[inline(never)]` (v0.0.13).
    OptionalIdentFrom(&'static [&'static str]),
    /// `#[name = "VAL"]` or `#[name("VAL")]` — exactly one string-literal arg.
    /// No allow-list — the value is opaque (e.g. a linker symbol name).
    /// Used by `#[link_name = "..."]` (Phase 11 / ObjC interop).
    ExactlyOneStr,
    /// v0.0.7 Slice 1.3: `#[name(N)]` — exactly one integer-literal
    /// arg. Range validation is per-attribute and lives in sema (so
    /// the diagnostic carries the loop-statement context).
    ExactlyOneInt,
}

struct AttrSpec {
    name: &'static str,
    args: ArgsSpec,
    /// Bitmask of legal placements.
    targets: u8,
    /// True iff the attribute may appear multiple times on the same item.
    allow_duplicate: bool,
}

const KNOWN_ATTRS: &[AttrSpec] = &[
    AttrSpec {
        name: "test",
        args: ArgsSpec::None,
        // Free functions only. Method `#[test]` is E0360 — that's a sema
        // rule, but we also reject the placement here so the error fires
        // at the parsing boundary before reaching sema.
        targets: TARGET_FN,
        allow_duplicate: false,
    },
    // Slice 10.FFI.5: `#[repr(C)]` declares C-compatible struct layout
    // for FFI passing. The codegen-side guarantee is that field order
    // is preserved (no reordering) and no implicit padding beyond what
    // C would insert. Today our default struct layout already matches
    // C for primitive-typed fields on x86_64; the attribute is the
    // *promise* that this remains stable across future codegen
    // changes. Only `C` is accepted as the argument.
    AttrSpec {
        name: "repr",
        args: ArgsSpec::OneIdentFrom(&["C"]),
        targets: TARGET_STRUCT,
        allow_duplicate: false,
    },
    // Phase 11 / ObjC interop: `#[link_name = "..."]` aliases an
    // `extern fn`'s linker symbol. Lets the user declare the same
    // C symbol under many typed signatures — the load-bearing trick
    // for ObjC's `objc_msgSend` (which uses no prototype on the C side
    // and relies on each call site picking its own ABI). Sema enforces
    // extern-only placement (E0356 with a more specific message on
    // non-extern fns).
    AttrSpec {
        name: "link_name",
        args: ArgsSpec::ExactlyOneStr,
        targets: TARGET_FN,
        allow_duplicate: false,
    },
    // v0.0.10 Phase 1: `#[no_alloc]` — verifiable real-time contract.
    // A `#[no_alloc]`-marked function and everything it transitively calls
    // must not heap-allocate. Surface-shape only; the call-graph walk and
    // E0901 emission live in sema (see `check_no_alloc`). Free functions
    // only — methods get the marker via their impl block's fn after sema's
    // collect_methods normalizes them to FnSig entries.
    AttrSpec {
        name: "no_alloc",
        args: ArgsSpec::None,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.10 Phase 3: `#[bounded_recursion]` — companion to `#[no_alloc]`.
    // Rejects any function whose call graph leads back to itself. Same
    // call-graph walk machinery as `#[no_alloc]`; sema-emitted E0906.
    AttrSpec {
        name: "bounded_recursion",
        args: ArgsSpec::None,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.12 realtime Phase 3: `#[no_block]` — verifiable no-blocking
    // contract. A `#[no_block]`-marked function and everything it
    // transitively calls must not call a blocking primitive (mutex lock,
    // condvar wait, thread join, sleep, blocking I/O, blocking socket op).
    // Surface-shape only; the call-graph walk and E0907 emission live in
    // sema (see `check_no_block`). Composes transitively like `#[no_alloc]`.
    AttrSpec {
        name: "no_block",
        args: ArgsSpec::None,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.12 realtime Phase 4: `#[realtime]` — bundle attribute. Sugar for
    // the implemented hot-path contracts: `#[no_alloc]` + `#[no_block]` +
    // `#[bounded_recursion]`. A `#[realtime]` fn is checked by all three
    // passes and, transitively, satisfies a no_alloc/no_block requirement at
    // a call site. (Bounded-stack / call-graph-closure checks join the
    // bundle when those passes land.)
    AttrSpec {
        name: "realtime",
        args: ArgsSpec::None,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.12 realtime Phase 4 (bounded stack): `#[max_stack(N)]` — bound the
    // function's estimated stack frame to N bytes. Surface-shape only; the
    // frame estimate (parameters + locals with known types) and E0908
    // emission live in sema (see `check_max_stack`).
    AttrSpec {
        name: "max_stack",
        args: ArgsSpec::ExactlyOneInt,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.13 (topic D): `#[inline]` — LLVM inlining control. `#[inline]`
    // emits `inlinehint` (raises the inliner's likelihood at -O2/-O3);
    // `#[inline(always)]` emits `alwaysinline` (forces inlining, including in
    // debug -O0 and past the cost threshold — the lever for hot SIMD/kernel
    // wrappers that otherwise stay a `bl`); `#[inline(never)]` emits
    // `noinline`. Surface-shape only; codegen attaches the LLVM attribute on
    // the function/method `define`. No sema rule — these are pure hints.
    AttrSpec {
        name: "inline",
        args: ArgsSpec::OptionalIdentFrom(&["always", "never"]),
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.14 inline asm Tier 3: `#[naked]` — emit the function with no
    // prologue/epilogue (LLVM `naked`). Its body must be inline `#asm(...)`
    // that handles the ABI and returns itself (sema's `check_naked` enforces
    // this, E0909). For trampolines, interrupt/entry stubs, custom-ABI shims.
    AttrSpec {
        name: "naked",
        args: ArgsSpec::None,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.7 Slice 1.3: `#[unroll(N)]` on a loop statement. Codegen
    // attaches `!{!"llvm.loop.unroll.count", i32 N}` to the back-edge
    // branch's `!llvm.loop` group. Sema validates N ∈ [1, 256] (E0510).
    AttrSpec {
        name: "unroll",
        args: ArgsSpec::ExactlyOneInt,
        targets: TARGET_LOOP_STMT,
        allow_duplicate: false,
    },
    // v0.0.7 Slice 1.3: `#[vectorize_width(N)]` — hint LLVM's loop
    // vectorizer to a specific vector width. Same shape as `unroll`.
    AttrSpec {
        name: "vectorize_width",
        args: ArgsSpec::ExactlyOneInt,
        targets: TARGET_LOOP_STMT,
        allow_duplicate: false,
    },
    // TEXT.R1: `#[lang("string")]` — lang-item marker. Tags the one stdlib
    // struct that is the designated owned-string type (`Text`). The compiler
    // records its `StructId` during collection and lowers string literals (and,
    // later, interpolation) in a `Text` context into calls to its `from_str`
    // constructor. Surface-shape only here; the designation + lowering live in
    // sema. One string arg names the lang item.
    AttrSpec {
        name: "lang",
        args: ArgsSpec::ExactlyOneStr,
        targets: TARGET_STRUCT,
        allow_duplicate: false,
    },
];

/// Single-file entry point. Mirrors `sema::check`.
pub fn check(prog: &Program, file: PathBuf, src: &str) -> Vec<Diagnostic> {
    let entry_id = String::new();
    let mut files: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
    files.insert(entry_id.clone(), (file.clone(), src.to_string()));
    check_multi(prog, file, src, files)
}

/// Multi-file entry point. Mirrors `sema::check_multi`. `entry_file` +
/// `entry_src` are used as the fallback when an item has no `origin_file`
/// (single-file mode, or items synthesized after resolver merge).
pub fn check_multi(
    prog: &Program,
    entry_file: PathBuf,
    entry_src: &str,
    files: BTreeMap<String, (PathBuf, String)>,
) -> Vec<Diagnostic> {
    let mut ctx = Ctx::new(entry_file, entry_src, files);
    for item in &prog.items {
        ctx.set_current_file(item.origin_file.as_deref());
        match &item.kind {
            ItemKind::Function(f) => {
                ctx.check_attrs(&f.attributes, TARGET_FN, "function");
                ctx.check_async_on_32_bit(f.is_async, &f.name);
                ctx.walk_block_for_loop_attrs(&f.body);
            }
            ItemKind::Struct(s) => {
                ctx.check_attrs(&s.attributes, TARGET_STRUCT, "struct");
                for field in &s.fields {
                    ctx.check_attrs(&field.attributes, TARGET_FIELD, "struct field");
                }
            }
            ItemKind::Enum(e) => {
                ctx.check_attrs(&e.attributes, TARGET_ENUM, "enum");
                for variant in &e.variants {
                    ctx.check_attrs(&variant.attributes, TARGET_VARIANT, "enum variant");
                }
            }
            ItemKind::Impl(b) => {
                for method in &b.methods {
                    ctx.check_attrs(&method.attributes, TARGET_METHOD, "method");
                    ctx.check_async_on_32_bit(method.is_async, &method.name);
                    ctx.walk_block_for_loop_attrs(&method.body);
                }
            }
            // Slice 7GEN.3: interface declarations carry attributes
            // on the interface itself. Phase 7 first cut supports the
            // existing attribute set; new interface-specific
            // attributes (e.g. `#[sealed]`) get added to KNOWN_ATTRS
            // when introduced. For now, validate as-if struct/enum.
            ItemKind::Interface(i) => {
                ctx.check_attrs(&i.attributes, TARGET_STRUCT, "interface");
            }
            // Phase 11 polish: type aliases admit no attributes (the
            // parser rejects them at the source level too).
            ItemKind::TypeAlias(_) => {}
            // v0.0.9 Phase 4: const/static admit no attributes in the
            // first cut. The parser rejects them at the surface; this
            // arm is a defense-in-depth no-op.
            // v0.0.15: module-scope `#asm("...")` carries no attributes
            // either (the parser rejects them); nothing to validate.
            ItemKind::Const(_) | ItemKind::Static(_) | ItemKind::ModuleAsm(_) => {}
        }
    }
    ctx.diags
}

struct Ctx {
    diags: Vec<Diagnostic>,
    entry_file: PathBuf,
    entry_lm: LineMap,
    entry_src: String,
    files: BTreeMap<String, (PathBuf, String, LineMap)>,
    current_file: Option<String>,
}

impl Ctx {
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
            diags: Vec::new(),
            entry_file,
            entry_lm,
            entry_src: entry_src.to_string(),
            files: compiled,
            current_file: None,
        }
    }

    fn set_current_file(&mut self, id: Option<&str>) {
        self.current_file = id.map(String::from);
    }

    /// v0.0.7 Slice 1.3: descend into a function body and validate
    /// statement-level attributes on `while` / `loop` / `for`. Other
    /// statement kinds carry no attributes today and are walked only
    /// to reach their nested bodies (`if let`, etc. — irrelevant for
    /// loop-stmt attrs since the lowering pass hasn't yet run, but
    /// recursing is cheap and future-proofs the walker).
    fn walk_block_for_loop_attrs(&mut self, block: &Block) {
        for s in &block.stmts {
            self.walk_stmt_for_loop_attrs(s);
        }
        if let Some(tail) = &block.tail {
            self.walk_expr_for_loop_attrs(tail);
        }
    }

    fn walk_stmt_for_loop_attrs(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::While {
                cond,
                body,
                attributes,
            } => {
                self.check_attrs(attributes, TARGET_LOOP_STMT, "loop statement");
                self.walk_expr_for_loop_attrs(cond);
                self.walk_block_for_loop_attrs(body);
            }
            StmtKind::Loop(body, attributes) => {
                self.check_attrs(attributes, TARGET_LOOP_STMT, "loop statement");
                self.walk_block_for_loop_attrs(body);
            }
            StmtKind::For(fl, attributes) => {
                self.check_attrs(attributes, TARGET_LOOP_STMT, "loop statement");
                match fl {
                    ForLoop::Range { iter, body, .. } => {
                        self.walk_expr_for_loop_attrs(iter);
                        self.walk_block_for_loop_attrs(body);
                    }
                    ForLoop::CStyle {
                        init,
                        cond,
                        update,
                        body,
                    } => {
                        if let Some(s) = init {
                            self.walk_stmt_for_loop_attrs(s);
                        }
                        if let Some(c) = cond {
                            self.walk_expr_for_loop_attrs(c);
                        }
                        for u in update {
                            self.walk_expr_for_loop_attrs(u);
                        }
                        self.walk_block_for_loop_attrs(body);
                    }
                }
            }
            StmtKind::Let { init: Some(e), .. }
            | StmtKind::Expr(e)
            | StmtKind::Return(Some(e))
            | StmtKind::Defer(e)
            | StmtKind::Assert(e) => self.walk_expr_for_loop_attrs(e),
            StmtKind::Let { init: None, .. }
            | StmtKind::Return(None)
            | StmtKind::Break
            | StmtKind::Continue => {}
            StmtKind::IfLet {
                scrutinee,
                body,
                else_body,
                ..
            } => {
                self.walk_expr_for_loop_attrs(scrutinee);
                self.walk_block_for_loop_attrs(body);
                if let Some(eb) = else_body {
                    self.walk_block_for_loop_attrs(eb);
                }
            }
            StmtKind::WhileLet {
                scrutinee, body, ..
            } => {
                self.walk_expr_for_loop_attrs(scrutinee);
                self.walk_block_for_loop_attrs(body);
            }
            StmtKind::GuardLet {
                scrutinee,
                else_body,
                ..
            } => {
                self.walk_expr_for_loop_attrs(scrutinee);
                self.walk_block_for_loop_attrs(else_body);
            }
        }
    }

    fn walk_expr_for_loop_attrs(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Block(b) => self.walk_block_for_loop_attrs(b),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.walk_expr_for_loop_attrs(cond);
                self.walk_block_for_loop_attrs(then);
                if let Some(eb) = else_branch {
                    self.walk_expr_for_loop_attrs(eb);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk_expr_for_loop_attrs(scrutinee);
                for arm in arms {
                    self.walk_expr_for_loop_attrs(&arm.body);
                }
            }
            // Other expression kinds either carry no statement
            // contexts or carry sub-expressions whose loop-stmt
            // children (rare) are exercised via the more direct
            // statement walker above.
            _ => {}
        }
    }

    /// Get the (path, source, LineMap) a span renders against. v0.0.22
    /// file-aware: a stamped span (`span.file != 0`) routes itself; the
    /// 0 sentinel falls back to the resolver-tagged current item's file,
    /// then the entry file (single-file mode, or pre-resolver items).
    fn file_ctx_for(&self, span: crate::lexer::Span) -> (PathBuf, &str, &LineMap) {
        if span.file != 0 {
            if let Some(fid) = crate::lexer::interned_file(span.file) {
                if let Some((path, src, lm)) = self.files.get(&fid) {
                    return (path.clone(), src.as_str(), lm);
                }
            }
        }
        if let Some(id) = self.current_file.as_deref() {
            if let Some((path, src, lm)) = self.files.get(id) {
                return (path.clone(), src.as_str(), lm);
            }
        }
        (
            self.entry_file.clone(),
            self.entry_src.as_str(),
            &self.entry_lm,
        )
    }

    fn make_span(&self, span: crate::lexer::Span) -> SourceSpan {
        let (path, src, lm) = self.file_ctx_for(span);
        lm.span(&path, span, src)
    }

    fn check_attrs(&mut self, attrs: &[Attribute], target: u8, target_label: &str) {
        // Track seen names for duplicate detection (only matters for attrs
        // whose spec disallows duplicates).
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for attr in attrs {
            self.check_one_attr(attr, target, target_label, &seen);
            *seen.entry(attr.path.name.clone()).or_insert(0) += 1;
        }
    }

    fn check_one_attr(
        &mut self,
        attr: &Attribute,
        target: u8,
        target_label: &str,
        seen: &BTreeMap<String, usize>,
    ) {
        let name = &attr.path.name;
        let spec = match KNOWN_ATTRS.iter().find(|s| s.name == name) {
            Some(s) => s,
            None => {
                self.emit_unknown(attr);
                return;
            }
        };
        // Duplicate check fires before target / arg-shape checks so a
        // user who pastes `#[test] #[test]` sees the duplicate error
        // rather than a downstream complaint about each one.
        if !spec.allow_duplicate {
            if let Some(&prev_count) = seen.get(name) {
                if prev_count >= 1 {
                    self.emit_duplicate(attr);
                    return;
                }
            }
        }
        if (spec.targets & target) == 0 {
            self.emit_wrong_target(attr, spec, target_label);
            return;
        }
        match spec.args {
            ArgsSpec::None => {
                if !attr.args.is_empty() {
                    self.emit_wrong_args(attr, spec);
                }
            }
            ArgsSpec::OneIdentFrom(allowed) => {
                let ok = match attr.args.as_slice() {
                    [AttrArg::Ident(id)] => allowed.contains(&id.name.as_str()),
                    _ => false,
                };
                if !ok {
                    self.emit_bad_repr_arg(attr, spec, allowed);
                }
            }
            ArgsSpec::OptionalIdentFrom(allowed) => {
                let ok = match attr.args.as_slice() {
                    [] => true,
                    [AttrArg::Ident(id)] => allowed.contains(&id.name.as_str()),
                    _ => false,
                };
                if !ok {
                    self.emit_bad_optional_ident_arg(attr, spec, allowed);
                }
            }
            ArgsSpec::ExactlyOneStr => {
                let ok = matches!(attr.args.as_slice(), [AttrArg::Str(_, _)]);
                if !ok {
                    self.emit_expected_str_arg(attr, spec);
                }
            }
            ArgsSpec::ExactlyOneInt => {
                let ok = matches!(attr.args.as_slice(), [AttrArg::Int(_, _)]);
                if !ok {
                    self.emit_expected_int_arg(attr, spec);
                }
            }
        }
    }

    /// v0.0.21 embedded profile (E0867): async fns lower through the
    /// kqueue/epoll reactor and a coroutine runtime that is not yet
    /// pointer-width clean; 32-bit targets reject them at check time
    /// with the profile story instead of an IR-verifier failure.
    fn check_async_on_32_bit(&mut self, is_async: bool, name: &crate::ast::Ident) {
        if !is_async {
            return;
        }
        let tgt = crate::target::active_target();
        if tgt.pointer_width >= 64 {
            return;
        }
        let primary = self.make_span(name.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0867"),
            message: format!(
                "async functions are not supported on 32-bit target `{}`",
                tgt.name
            ),
            primary,
            labels: Vec::new(),
            notes: vec![
                "the async runtime (reactor + coroutine frames) is 64-bit only today".to_string(),
            ],
            suggestions: Vec::new(),
        });
    }

    fn emit_expected_int_arg(&mut self, attr: &Attribute, spec: &AttrSpec) {
        let primary = self.make_span(attr.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0355"),
            message: format!(
                "attribute `#[{}]` requires exactly one integer-literal argument (e.g. `#[{}(4)]`)",
                spec.name, spec.name
            ),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    fn emit_expected_str_arg(&mut self, attr: &Attribute, spec: &AttrSpec) {
        let primary = self.make_span(attr.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0355"),
            message: format!(
                "attribute `#[{}]` requires exactly one string-literal argument (e.g. `#[{} = \"value\"]`)",
                spec.name, spec.name
            ),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    fn emit_unknown(&mut self, attr: &Attribute) {
        let name = &attr.path.name;
        let suggestion = closest_attr_name(name);
        let primary = self.make_span(attr.path.span);
        let mut d = Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0354"),
            message: format!("unknown attribute `#[{name}]`"),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        };
        if let Some(target) = suggestion {
            let span = self.make_span(attr.path.span);
            d.suggestions.push(Suggestion {
                description: format!("did you mean `#[{target}]`?"),
                span,
                replacement: target.to_string(),
                applicability: Applicability::MaybeIncorrect,
            });
        }
        self.diags.push(d);
    }

    fn emit_wrong_args(&mut self, attr: &Attribute, spec: &AttrSpec) {
        let primary = self.make_span(attr.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0355"),
            message: format!("attribute `#[{}]` takes no arguments", spec.name),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    /// v0.0.13: `#[inline(...)]` with an unsupported arg shape.
    fn emit_bad_optional_ident_arg(
        &mut self,
        attr: &Attribute,
        spec: &AttrSpec,
        allowed: &[&'static str],
    ) {
        let primary = self.make_span(attr.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0355"),
            message: format!(
                "attribute `#[{}]` takes no arguments, or exactly one of: {}",
                spec.name,
                allowed
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(" / ")
            ),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    /// Slice 10.FFI.5: `#[repr(...)]` with an unsupported arg.
    fn emit_bad_repr_arg(&mut self, attr: &Attribute, spec: &AttrSpec, allowed: &[&'static str]) {
        let primary = self.make_span(attr.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0355"),
            message: format!(
                "attribute `#[{}]` requires exactly one of: {}",
                spec.name,
                allowed
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(" / ")
            ),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    fn emit_wrong_target(&mut self, attr: &Attribute, spec: &AttrSpec, target_label: &str) {
        let primary = self.make_span(attr.span);
        let allowed = describe_targets(spec.targets);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0356"),
            message: format!(
                "attribute `#[{}]` may only appear on {allowed}, not on {target_label}",
                spec.name
            ),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    fn emit_duplicate(&mut self, attr: &Attribute) {
        let primary = self.make_span(attr.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0357"),
            message: format!("duplicate attribute `#[{}]`", attr.path.name),
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }
}

fn describe_targets(mask: u8) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if mask & TARGET_FN != 0 {
        parts.push("functions");
    }
    if mask & TARGET_METHOD != 0 {
        parts.push("methods");
    }
    if mask & TARGET_STRUCT != 0 {
        parts.push("structs");
    }
    if mask & TARGET_ENUM != 0 {
        parts.push("enums");
    }
    if mask & TARGET_FIELD != 0 {
        parts.push("struct fields");
    }
    if mask & TARGET_VARIANT != 0 {
        parts.push("enum variants");
    }
    match parts.len() {
        0 => "(no targets)".to_string(),
        1 => parts[0].to_string(),
        2 => format!("{} or {}", parts[0], parts[1]),
        _ => {
            let last = parts.pop().unwrap();
            format!("{}, or {last}", parts.join(", "))
        }
    }
}

/// Returns the known attribute name closest to `name` if the edit
/// distance is ≤ 2, otherwise None. Used for E0354 did-you-mean.
fn closest_attr_name(name: &str) -> Option<&'static str> {
    let mut best: Option<(&'static str, usize)> = None;
    for spec in KNOWN_ATTRS {
        let d = edit_distance(name, spec.name);
        match best {
            Some((_, prev)) if prev <= d => {}
            _ => best = Some((spec.name, d)),
        }
    }
    match best {
        Some((target, d)) if d <= 2 => Some(target),
        _ => None,
    }
}

/// A `#[test]`-marked function discovered in the merged Program. The driver
/// (slice 5ATTR.4 `cpc test`) consumes this to synthesize the test-runner
/// `main`. `qualified_name` is the resolver's file-id-qualified form
/// (e.g. `src.math.adds_one`) — the same name codegen mangles to in LLVM.
/// `display_name` is the `::`-flavored form for human + JSON output
/// (e.g. `src::math::adds_one`); the rule resolves design-note §6 open
/// question 1 in favor of human-readable `::` while the resolver's `.`
/// stays in the qualified-name backbone.
///
/// `returns_i32` distinguishes the two accepted signatures so the runner
/// knows whether to capture an exit code or just call-and-return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFn {
    pub qualified_name: String,
    pub display_name: String,
    pub origin_file: Option<String>,
    pub returns_i32: bool,
    pub span: crate::lexer::Span,
}

/// Walk the merged Program and collect every `#[test]`-marked function.
/// Returns in source order. Pure data — does no validation; callers are
/// expected to have run `attrs::check` and `sema::check` first so any
/// E0354–E0360 diagnostics already fired.
pub fn discover_tests(prog: &Program) -> Vec<TestFn> {
    let mut tests = Vec::new();
    for item in &prog.items {
        let ItemKind::Function(f) = &item.kind else {
            continue;
        };
        if !f.attributes.iter().any(|a| a.path.name == "test") {
            continue;
        }
        let qualified_name = f.name.name.clone();
        // Doctests (5DOC) carry a `__doctest_<item>_<idx>` leaf segment;
        // reformat their display name into the design-note's
        // `DOC_TEST::<qualifier>::<item>::<idx>` form. Hand-written tests
        // fall through to the standard `.`→`::` rewrite.
        let display_name = crate::doctest::format_doctest_display_name(&qualified_name)
            .unwrap_or_else(|| qualified_name.replace('.', "::"));
        let returns_i32 = f.return_type.is_some();
        tests.push(TestFn {
            qualified_name,
            display_name,
            origin_file: item.origin_file.clone(),
            returns_i32,
            span: f.name.span,
        });
    }
    tests
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser;

    fn check_src(src: &str) -> Vec<Diagnostic> {
        let toks = tokenize(src).expect("lex");
        let prog = parser::parse(toks).expect("parse");
        check(&prog, PathBuf::from("test.cplus"), src)
    }

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.code.0).collect()
    }

    #[test]
    fn test_attribute_on_free_function_clean() {
        let diags = check_src("#[test] fn ok() { return; }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn unknown_attribute_e0354() {
        let diags = check_src("#[tset] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0354"]);
        // Did-you-mean suggestion fires for "tset" → "test".
        let suggestions = &diags[0].suggestions;
        assert_eq!(suggestions.len(), 1, "expected did-you-mean");
        assert_eq!(suggestions[0].replacement, "test");
    }

    #[test]
    fn unknown_attribute_no_close_match_no_suggestion() {
        let diags = check_src("#[totally_unrelated] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0354"]);
        assert!(
            diags[0].suggestions.is_empty(),
            "no suggestion for distant unknown name"
        );
    }

    // ---- TEXT.R1: `#[lang("string")]` ----

    #[test]
    fn lang_string_on_struct_clean() {
        let diags = check_src("#[lang(\"string\")] struct Text { ptr: *u8 }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn lang_missing_arg_e0355() {
        let diags = check_src("#[lang] struct Text { ptr: *u8 }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn lang_on_function_wrong_target_e0356() {
        let diags = check_src("#[lang(\"string\")] fn f() { return; }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    // ---- Slice 10.FFI.5: `#[repr(C)]` ----

    #[test]
    fn repr_c_on_struct_clean() {
        let diags = check_src("#[repr(C)] struct P { x: i32 }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn repr_missing_arg_e0355() {
        let diags = check_src("#[repr] struct P { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn repr_invalid_arg_e0355() {
        let diags = check_src("#[repr(Rust)] struct P { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn repr_on_function_e0356() {
        let diags = check_src("#[repr(C)] fn f() { return; }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn repr_on_enum_e0356() {
        let diags = check_src("#[repr(C)] enum E { A, B }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn test_attribute_with_args_rejected_e0355() {
        let diags = check_src("#[test(slow)] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn test_attribute_on_struct_rejected_e0356() {
        let diags = check_src("#[test] struct X { v: i32 }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn test_attribute_on_enum_rejected_e0356() {
        let diags = check_src("#[test] enum E { A, B }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn test_attribute_on_method_rejected_e0356() {
        // Methods aren't free fns; E0356 fires here (independent of sema's
        // E0360 rule, which is the same conceptual rejection at a different
        // layer — both errors will eventually point at the same span).
        let diags = check_src(
            "struct X { v: i32 }\n\
             impl X { #[test] fn t(this) { return; } }",
        );
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn duplicate_test_attribute_e0357() {
        let diags = check_src("#[test] #[test] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0357"]);
    }

    #[test]
    fn attribute_on_struct_field_unknown_fires_e0354() {
        let diags = check_src("struct X { #[ohno] v: i32 }");
        assert_eq!(codes(&diags), vec!["E0354"]);
    }

    #[test]
    fn attribute_on_enum_variant_unknown_fires_e0354() {
        let diags = check_src("enum E { #[ohno] A, B }");
        assert_eq!(codes(&diags), vec!["E0354"]);
    }

    #[test]
    fn multiple_attributes_each_validated() {
        // Two distinct unknown attributes → two diagnostics.
        let diags = check_src("#[foo] #[bar] fn x() { return; }");
        let codes_seen: Vec<&str> = codes(&diags);
        assert_eq!(codes_seen, vec!["E0354", "E0354"]);
    }

    #[test]
    fn no_attributes_no_diagnostics() {
        let diags = check_src("fn main() -> i32 { return 0; }");
        assert!(diags.is_empty());
    }

    // ---- v0.0.10 Phase 1: `#[no_alloc]` attribute target validation ----

    #[test]
    fn no_alloc_on_free_fn_clean() {
        let diags = check_src("#[no_alloc] fn ok(x: i32) -> i32 { return x; }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn no_alloc_on_struct_rejected_e0356() {
        let diags = check_src("#[no_alloc] struct S { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn no_alloc_on_enum_rejected_e0356() {
        let diags = check_src("#[no_alloc] enum E { A, B }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn no_alloc_with_args_rejected_e0355() {
        let diags = check_src("#[no_alloc(foo)] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn no_alloc_duplicate_e0357() {
        let diags = check_src("#[no_alloc] #[no_alloc] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0357"]);
    }

    #[test]
    fn no_alloc_on_method_clean() {
        let diags = check_src(
            "struct X { v: i32 }\n\
             impl X { #[no_alloc] fn t(this) -> i32 { return this.v; } }",
        );
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    // ---- v0.0.10 Phase 3: `#[bounded_recursion]` target validation ----

    #[test]
    fn bounded_recursion_on_free_fn_clean() {
        let diags = check_src("#[bounded_recursion] fn ok(x: i32) -> i32 { return x; }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn bounded_recursion_on_struct_rejected_e0356() {
        let diags = check_src("#[bounded_recursion] struct S { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    // ---- v0.0.12 realtime Phase 3/4: `#[no_block]` / `#[realtime]` ----

    #[test]
    fn no_block_on_free_fn_clean() {
        let diags = check_src("#[no_block] fn ok(x: i32) -> i32 { return x; }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn no_block_on_method_clean() {
        let diags = check_src(
            "struct X { v: i32 }\n\
             impl X { #[no_block] fn t(this) -> i32 { return this.v; } }",
        );
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn no_block_on_struct_rejected_e0356() {
        let diags = check_src("#[no_block] struct S { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn no_block_with_args_rejected_e0355() {
        let diags = check_src("#[no_block(foo)] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn no_block_duplicate_e0357() {
        let diags = check_src("#[no_block] #[no_block] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0357"]);
    }

    #[test]
    fn realtime_on_free_fn_clean() {
        let diags = check_src("#[realtime] fn ok(x: i32) -> i32 { return x; }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn realtime_on_enum_rejected_e0356() {
        let diags = check_src("#[realtime] enum E { A, B }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn realtime_with_args_rejected_e0355() {
        let diags = check_src("#[realtime(2048)] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    // ---- v0.0.12 realtime Phase 4: `#[max_stack(N)]` validation ----

    #[test]
    fn max_stack_on_free_fn_clean() {
        let diags = check_src("#[max_stack(4096)] fn ok(x: i32) -> i32 { return x; }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn max_stack_on_method_clean() {
        let diags = check_src(
            "struct X { v: i32 }\n\
             impl X { #[max_stack(256)] fn t(this) -> i32 { return this.v; } }",
        );
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn max_stack_on_struct_rejected_e0356() {
        let diags = check_src("#[max_stack(64)] struct S { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn max_stack_no_arg_rejected_e0355() {
        let diags = check_src("#[max_stack] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn max_stack_string_arg_rejected_e0355() {
        let diags = check_src("#[max_stack(\"big\")] fn x() { return; }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn diagnostic_primary_covers_attribute_span() {
        // The unknown-attribute diagnostic should point at the attribute
        // path, not at the surrounding function. Ensure the byte range
        // sits inside the `#[...]` block in the source.
        let src = "#[whatever] fn x() { return; }";
        let diags = check_src(src);
        assert_eq!(codes(&diags), vec!["E0354"]);
        let p = &diags[0].primary;
        // line 1, somewhere after the `#[`
        assert_eq!(p.start.line, 1);
        assert!(
            p.start.col >= 3,
            "expected column inside `#[...]`, got {}",
            p.start.col
        );
    }

    // ---- 5ATTR.2: discover_tests ----

    fn parse_src(src: &str) -> Program {
        let toks = tokenize(src).expect("lex");
        parser::parse(toks).expect("parse")
    }

    #[test]
    fn discover_tests_finds_single_test() {
        let prog = parse_src(
            "#[test] fn t1() { return; }\n\
             fn main() -> i32 { return 0; }",
        );
        let tests = discover_tests(&prog);
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].qualified_name, "t1");
        assert_eq!(tests[0].display_name, "t1");
        assert!(!tests[0].returns_i32);
    }

    #[test]
    fn discover_tests_ignores_unmarked() {
        let prog = parse_src(
            "fn helper() { return; }\n\
             #[test] fn t1() { return; }\n\
             fn other() -> i32 { return 0; }",
        );
        let tests = discover_tests(&prog);
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].qualified_name, "t1");
    }

    #[test]
    fn discover_tests_preserves_source_order() {
        let prog = parse_src(
            "#[test] fn a() { return; }\n\
             #[test] fn b() { return; }\n\
             #[test] fn c() { return; }\n\
             fn main() -> i32 { return 0; }",
        );
        let tests = discover_tests(&prog);
        let names: Vec<&str> = tests.iter().map(|t| t.qualified_name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn discover_tests_captures_return_type_kind() {
        let prog = parse_src(
            "#[test] fn unit_test() { return; }\n\
             #[test] fn coded_test() -> i32 { return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
        let tests = discover_tests(&prog);
        assert_eq!(tests.len(), 2);
        assert!(
            !tests[0].returns_i32,
            "fn() shouldn't be flagged returns_i32"
        );
        assert!(tests[1].returns_i32, "fn() -> i32 should be flagged");
    }

    #[test]
    fn discover_tests_display_name_uses_double_colon() {
        // Simulate the resolver-merged form by hand-constructing an item
        // with a `.`-qualified name. Discovery should map it to `::` in
        // display while keeping the `.` form in qualified_name.
        let mut prog = parse_src("#[test] fn t() { return; }");
        let ItemKind::Function(ref mut f) = prog.items[0].kind else {
            panic!()
        };
        f.name.name = "src.math.t".to_string();
        let tests = discover_tests(&prog);
        assert_eq!(tests[0].qualified_name, "src.math.t");
        assert_eq!(tests[0].display_name, "src::math::t");
    }

    #[test]
    fn discover_tests_empty_when_no_tests() {
        let prog = parse_src("fn main() -> i32 { return 0; }");
        assert!(discover_tests(&prog).is_empty());
    }

    // ---- v0.0.13 (topic D): `#[inline]` ----

    #[test]
    fn inline_bare_on_fn_clean() {
        let diags = check_src("#[inline] fn f() -> i32 { return 0; }");
        assert!(diags.is_empty(), "got: {:?}", codes(&diags));
    }

    #[test]
    fn inline_always_and_never_on_fn_clean() {
        assert!(check_src("#[inline(always)] fn f() -> i32 { return 0; }").is_empty());
        assert!(check_src("#[inline(never)] fn f() -> i32 { return 0; }").is_empty());
    }

    #[test]
    fn inline_on_method_clean() {
        let diags = check_src(
            "struct P { x: i32 } impl P { #[inline(always)] fn get(this) -> i32 { return this.x; } }",
        );
        assert!(diags.is_empty(), "got: {:?}", codes(&diags));
    }

    #[test]
    fn inline_bad_arg_e0355() {
        let diags = check_src("#[inline(sometimes)] fn f() -> i32 { return 0; }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn inline_on_struct_e0356() {
        let diags = check_src("#[inline] struct S { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn inline_duplicate_e0357() {
        let diags = check_src("#[inline] #[inline(always)] fn f() -> i32 { return 0; }");
        assert_eq!(codes(&diags), vec!["E0357"]);
    }
}
