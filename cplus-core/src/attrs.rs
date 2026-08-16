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
//! Sema-level rules for `#[test]` functions (signature, `export` rejection,
//! `impl`-placement rejection — E0358/E0359/E0360) live in sema where the
//! type info is available. This pass only enforces the surface-level
//! attribute-shape rules.

use crate::ast::*;
use crate::diagnostics::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

const TARGET_FN: u16 = 0b0_0000_0001;
const TARGET_METHOD: u16 = 0b0_0000_0010;
const TARGET_STRUCT: u16 = 0b0_0000_0100;
const TARGET_ENUM: u16 = 0b0_0000_1000;
const TARGET_FIELD: u16 = 0b0_0001_0000;
const TARGET_VARIANT: u16 = 0b0_0010_0000;
/// v0.0.7 Slice 1.3: attribute on a loop statement (`while`, `loop`,
/// `for`). Used by `#[unroll(N)]` / `#[vectorize_width(N)]`.
const TARGET_LOOP_STMT: u16 = 0b0_0100_0000;
/// Interfaces get their OWN bit, which no attribute currently sets — so every
/// attribute on an interface is E0356, which is the correct state today. They
/// used to be validated with `TARGET_STRUCT`, so `#[watch]`, `#[repr(C)]` and
/// `#[lang]` passed validation there and then did nothing: the write-barrier
/// machinery, the layout rule and the lang-item registry all look only at
/// structs. A user who wrote one believed a feature was active
/// (reports/bug-26). An attribute meant to be legal on interfaces adds this
/// bit to its own registration, and a consumer alongside it.
const TARGET_INTERFACE: u16 = 0b1_0000_0000;

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
    /// `#[name]` or `#[name("VAL")]` — zero args, or one opaque string.
    /// Used by `#[deprecated]`, where the string is the migration note
    /// carried into the warning and omitting it is legal.
    OptionalStr,
    /// v0.0.7 Slice 1.3: `#[name(N)]` — exactly one integer-literal
    /// arg. Range validation is per-attribute and lives in sema (so
    /// the diagnostic carries the loop-statement context).
    ExactlyOneInt,
    /// v0.0.27 contracts: one or more full-expression args
    /// (`#[requires(n > 0)]`). The parser produced `AttrArg::Expr`s;
    /// sema owns type-checking and the purity rule (E0924).
    ExprArgs,
    /// v0.0.28 packing: `#[repr(...)]` takes a LIST now, because a C shape
    /// can need two independent claims at once — `#[repr(C, packed)]` is
    /// "C field order" plus "no internal padding". Each element is either an
    /// ident from the allow-list or `packed = N`. Which combinations mean
    /// anything on which item is sema's rule (E0926), not this table's: the
    /// vocabulary is all that is checked here.
    ReprArgs(&'static [&'static str]),
}

struct AttrSpec {
    name: &'static str,
    args: ArgsSpec,
    /// Bitmask of legal placements.
    targets: u16,
    /// True iff the attribute may appear multiple times on the same item.
    allow_duplicate: bool,
}

/// `Some(note)` iff `attrs` carries `#[deprecated]`, where `note` is the
/// attribute's optional string. The two levels are distinct and both matter:
/// the outer says whether to warn at all, the inner whether there is a
/// migration hint to print.
pub fn deprecation_note(attrs: &[Attribute]) -> Option<Option<String>> {
    let a = attrs.iter().find(|a| a.path.name == "deprecated")?;
    Some(a.args.iter().find_map(|g| match g {
        AttrArg::Str(s, _) => Some(s.clone()),
        _ => None,
    }))
}

/// Memory-model contract §5 (docs/compiler/design/memory-model.md): true iff `attrs`
/// carries `#[keeps(ARG)]` for the given argument (`"this"` / `"nothing"`).
/// Shared by sema (E0515 exemption, return-tie suppression) and borrowck
/// (caller-side receiver ties, elision short-circuit).
pub fn has_keeps(attrs: &[Attribute], arg: &str) -> bool {
    attrs.iter().any(|a| {
        a.path.name == "keeps"
            && a.args
                .iter()
                .any(|g| matches!(g, AttrArg::Ident(i) if i.name == arg))
    })
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
    // v0.0.27 FFI enums: `#[repr(u8)]` (and the other integer names)
    // pins a PLAIN enum's representation for the C boundary; `C` stays
    // the struct-layout promise and doubles as the i32 default on enums.
    // Sema owns the per-shape rules (integer reprs reject payload enums,
    // E0923); this table only gates the argument vocabulary.
    AttrSpec {
        name: "repr",
        args: ArgsSpec::ReprArgs(&[
            "C", "packed", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64",
        ]),
        targets: TARGET_STRUCT | TARGET_ENUM,
        allow_duplicate: false,
    },
    // v0.0.28 bitfields: `#[bits(N)] flags: u32` gives a field a WIDTH in
    // bits and packs it beside its neighbours inside one storage unit. The
    // width's legality against the field's type, and the requirement that
    // the struct be `#[repr(C)]` at all, are sema's (E0927) — the layout
    // rules are C's and need the resolved field type to state.
    AttrSpec {
        name: "bits",
        args: ArgsSpec::ExactlyOneInt,
        targets: TARGET_FIELD,
        allow_duplicate: false,
    },
    // v0.0.27 contracts: `#[requires(EXPR, ...)]` — machine-checked
    // preconditions in the signature. Sema type-checks each expression
    // (bool, parameter scope, pure — E0924); codegen emits them through
    // the `assert` path at function entry.
    AttrSpec {
        name: "requires",
        args: ArgsSpec::ExprArgs,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: true,
    },
    // v0.0.28 contracts: `#[ensures(EXPR, ...)]` — the other half. Same
    // shape and same purity rule as `#[requires]`; what it adds is the
    // binding `result`, which names the value being returned. Sema owns
    // both (E0924 purity, E0928 for a `result` that names nothing);
    // codegen emits them at every return, after the value exists.
    AttrSpec {
        name: "ensures",
        args: ArgsSpec::ExprArgs,
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: true,
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
    // issue-06 step 6: `#[runtime_abi]` — this declaration names a symbol the
    // COMPILER generates (`__cplus_*`: the reactor helpers, coroutine hooks,
    // thread trampolines). The prefix is reserved, so a declaration under it
    // must say what it is doing (E0919); without the marker a program could
    // quietly claim a runtime symbol and be linked against instead of it.
    AttrSpec {
        name: "runtime_abi",
        args: ArgsSpec::None,
        targets: TARGET_FN,
        allow_duplicate: false,
    },
    // Memory-model contract §5: `#[keeps(...)]` — a declared view-flow
    // summary for a function whose body the checker cannot read through
    // (raw-pointer stores, extern). `keeps(this)` = view arguments survive
    // inside the receiver (callers tie the receiver to each view argument's
    // owner; the callee-side store deny E0515 is lifted). `keeps(nothing)` =
    // the function copies what it needs; its returned view borrows no
    // argument (suppresses the Rule E-VIEW/E1/E3 conservative tie —
    // `text::intern` is the canonical case). Trusted, not verified — the
    // same accountability model as `opaque` on a raw-pointer field.
    AttrSpec {
        name: "keeps",
        args: ArgsSpec::OneIdentFrom(&["this", "nothing"]),
        targets: TARGET_FN | TARGET_METHOD,
        allow_duplicate: false,
    },
    // v0.0.10 Phase 1: `#[no_alloc]` — verifiable real-time contract.
    // A `#[no_alloc]`-marked function and everything it transitively calls
    // must not heap-allocate. Surface-shape only; the call-graph walk and
    // E0901 emission live in sema (see `check_no_alloc`). Accepted on free
    // functions and on methods — sema's `collect_methods` normalizes both to
    // FnSig entries, and the walk reads the marker from there.
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
    // TEXT.R1 / issue-06: `#[lang("...")]` — lang-item marker. Tags the ONE
    // stdlib declaration the compiler treats as a well-known type: the owned
    // string (`Text`), the `gen fn` protocol type (`Iterator`), the `async fn`
    // protocol type (`Future`), `Option` (what `Iterator::next` returns) and
    // `JoinHandle`. The compiler records it during collection and reads it
    // where the feature needs it. The alternative — locating these types by
    // suffix-matching their NAME — is shadowable by any user type and, on a
    // two-key match, per-process nondeterministic (reports/bug-08).
    //
    // Surface shape only here (one string arg, from the known set); the
    // designation and the lowering live in sema.
    AttrSpec {
        name: "lang",
        args: ArgsSpec::ExactlyOneStr,
        // issue-06: `Option` is an enum, so the marker has to reach enums too.
        targets: TARGET_STRUCT | TARGET_ENUM,
        allow_duplicate: false,
    },
    // OBS.1: `#[watch]` — field-write barrier. Every store to a field of
    // an `#[watch]` struct is followed by a call to the struct's
    // `fn on_value(ref this, field: str)` hook, with the written field's name
    // as the argument. Declarative per plan.md §2.8d: the attribute generates
    // no code and transforms no AST — it marks the type, and codegen reads
    // the mark the same way it reads `drop` to decide where teardown runs.
    // Sema enforces that an `#[watch]` struct supplies the hook (E0361)
    // with the exact signature (E0362). See
    // [docs/compiler/design/watch-structs.md](../../docs/compiler/design/watch-structs.md).
    AttrSpec {
        name: "watch",
        args: ArgsSpec::None,
        targets: TARGET_STRUCT,
        allow_duplicate: false,
    },
    // `#[deprecated]` / `#[deprecated("use parse_v2 instead")]` — this item
    // still works and should stop being used. Sema warns (W0006) at each
    // USE, never at the declaration, and the optional string is carried into
    // the warning verbatim as the migration note.
    //
    // The point is the migration shape it makes available: a stdlib
    // refinement can land as a warning list a consumer fixes at leisure and
    // break one release later, instead of arriving as a wall of hard errors.
    // Iris took 84 of those in one sitting; none of them had to be errors on
    // the day they appeared.
    AttrSpec {
        name: "deprecated",
        args: ArgsSpec::OptionalStr,
        targets: TARGET_FN
            | TARGET_METHOD
            | TARGET_STRUCT
            | TARGET_ENUM
            | TARGET_FIELD
            | TARGET_VARIANT,
        allow_duplicate: false,
    },
];

/// issue-06: every lang item the compiler resolves. A `#[lang("...")]` naming
/// anything else is E0359 — see the `lang` spec above.
pub const LANG_ITEMS: &[&str] = &[
    "string",
    "iterator",
    "future",
    "option",
    "join_handle",
    "rc",
    "mutex_guard",
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
                ctx.check_attrs(&i.attributes, TARGET_INTERFACE, "interface");
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
            StmtKind::LetDestructure { init, .. } => self.walk_expr_for_loop_attrs(init),
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

    fn check_attrs(&mut self, attrs: &[Attribute], target: u16, target_label: &str) {
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
        target: u16,
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
            ArgsSpec::ReprArgs(allowed) => {
                let ok = !attr.args.is_empty()
                    && attr.args.iter().all(|a| match a {
                        AttrArg::Ident(id) => allowed.contains(&id.name.as_str()),
                        // `packed = N` is the only key-value repr argument.
                        // The value's range is sema's (E0926); the shape is
                        // this table's.
                        AttrArg::KeyValue(k, _) => k.name == "packed",
                        _ => false,
                    });
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
                    return;
                }
                // issue-06: a lang item names something the compiler knows
                // about. A typo used to designate nothing, silently.
                if spec.name == "lang" {
                    if let [AttrArg::Str(v, _)] = attr.args.as_slice() {
                        if !LANG_ITEMS.contains(&v.as_str()) {
                            self.emit_unknown_lang_item(attr, v);
                        }
                    }
                }
            }
            ArgsSpec::OptionalStr => {
                let ok = matches!(attr.args.as_slice(), [] | [AttrArg::Str(_, _)]);
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
            ArgsSpec::ExprArgs => {
                // The parser produced Expr args for this attribute name; at
                // least one is required. Type/purity rules live in sema.
                let ok = !attr.args.is_empty()
                    && attr.args.iter().all(|a| matches!(a, AttrArg::Expr(_)));
                if !ok {
                    self.emit_wrong_args(attr, spec);
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

    /// issue-06: `#[lang("...")]` naming something the compiler does not
    /// resolve. Silently designating nothing is how a typo becomes "the
    /// feature quietly stopped working".
    fn emit_unknown_lang_item(&mut self, attr: &Attribute, name: &str) {
        let primary = self.make_span(attr.span);
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0390"),
            message: format!(
                "unknown lang item `{}` — the compiler resolves: {}",
                name,
                LANG_ITEMS.join(", ")
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

fn describe_targets(mask: u16) -> String {
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
    if mask & TARGET_LOOP_STMT != 0 {
        parts.push("loop statements");
    }
    if mask & TARGET_INTERFACE != 0 {
        parts.push("interfaces");
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

use crate::diagnostics::edit_distance;

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
    fn deprecated_is_a_registered_attribute_not_an_unknown_one() {
        // `#[deprecated]` used to be E0354. It read as supported because the
        // parser accepts any `#[ident("...")]` shape and one parser test
        // happened to use the name as its sample string — neither of which
        // is a registration. Both spellings must validate now.
        assert!(
            check_src("#[deprecated(\"gone\")] fn old() { return; }").is_empty(),
            "string-arg form must be accepted"
        );
        assert!(
            check_src("#[deprecated] fn old() { return; }").is_empty(),
            "bare form must be accepted"
        );
        // Every placement the spec claims. (An `impl` BLOCK carries no
        // attributes in Phase 5 — the parser says so before this pass runs —
        // so the method case is written where the attribute actually goes.)
        for (label, src) in [
            ("struct", "#[deprecated] struct S { n: i32 }".to_string()),
            ("enum", "#[deprecated] enum E { A }".to_string()),
            (
                "method",
                "struct S { n: i32 }\n\
                 impl S { #[deprecated] fn m(this) -> i32 { return 0; } }"
                    .to_string(),
            ),
            (
                "field",
                "struct S { #[deprecated] n: i32 }".to_string(),
            ),
            ("variant", "enum E { #[deprecated] A }".to_string()),
        ] {
            assert!(
                !codes(&check_src(&src)).contains(&"E0356"),
                "placement must be legal on a {label}"
            );
        }
    }

    #[test]
    fn deprecated_rejects_a_non_string_argument() {
        // `OptionalStr` means zero args or ONE string — not "anything goes".
        let diags = check_src("#[deprecated(4)] fn old() { return; }");
        assert!(
            !diags.is_empty(),
            "an int argument must still be refused, got none"
        );
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

    /// issue-06: a lang item names something the compiler resolves. A typo
    /// used to designate nothing at all, and the feature that needed the type
    /// reported a missing-stdlib error somewhere else entirely.
    #[test]
    fn lang_item_name_must_be_one_the_compiler_knows() {
        let diags = check_src("#[lang(\"iterater\")] struct Iterator[T] { opaque h: *u8 }");
        assert_eq!(codes(&diags), vec!["E0390"]);
        assert!(
            diags[0].message.contains("iterator"),
            "the message lists what IS known, got: {}",
            diags[0].message
        );
    }

    /// The four lang items beyond `string`, on the declaration kinds they
    /// actually appear on — `option` is an ENUM, which the marker did not
    /// reach before.
    #[test]
    fn every_lang_item_is_accepted_on_its_declaration() {
        for (item, decl) in [
            ("iterator", "struct Iterator[T] { opaque h: *u8 }"),
            ("future", "struct Future[T] { opaque h: *u8 }"),
            ("join_handle", "struct JoinHandle[O] { tid: u64 }"),
            ("option", "enum Option[T] { Some(T), None }"),
        ] {
            let diags = check_src(&format!("#[lang(\"{item}\")] {decl}"));
            assert!(
                diags.is_empty(),
                "`{item}` rejected: {:?}",
                codes(&diags)
            );
        }
    }

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
    fn repr_on_enum_accepted_v0027() {
        // v0.0.27 FFI enums: `#[repr(C)]` / `#[repr(u8)]` pin a plain
        // enum's representation (was E0356 before).
        let diags = check_src("#[repr(C)] enum E { A, B }
#[repr(u8)] enum F { X, Y }");
        assert!(codes(&diags).is_empty(), "got {:?}", codes(&diags));
    }

    #[test]
    fn repr_bad_arg_on_enum_rejected() {
        // A word this table does not know is rejected here, on shape alone.
        let diags = check_src("#[repr(f32)] enum E { A, B }");
        assert!(!codes(&diags).is_empty(), "bad repr arg must reject");
    }

    #[test]
    fn repr_packed_is_vocabulary_here_and_a_rule_in_sema() {
        // v0.0.28: `packed` is a legal repr WORD, so this pass accepts it
        // wherever `repr` is legal. Whether it MEANS anything on an enum is
        // sema's (E0926) — this table gates vocabulary, not sense.
        let diags = check_src("#[repr(packed)] enum E { A, B }");
        assert!(codes(&diags).is_empty(), "got {:?}", codes(&diags));
        let list = check_src("#[repr(C, packed)] struct S { a: u8, b: u32 }");
        assert!(codes(&list).is_empty(), "got {:?}", codes(&list));
        let kv = check_src("#[repr(C, packed = 2)] struct S { a: u8, b: u32 }");
        assert!(codes(&kv).is_empty(), "got {:?}", codes(&kv));
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

    // ---- OBS.1: `#[watch]` surface-shape validation ----
    //
    // The hook-existence (E0361) and hook-signature (E0362) rules need the
    // method table, so they live in sema; this pass only pins the four
    // shape rules the attribute spec buys.

    #[test]
    fn watch_on_struct_clean() {
        let diags = check_src("#[watch] struct S { x: i32 }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }

    #[test]
    fn watch_with_args_rejected_e0355() {
        let diags = check_src("#[watch(deep)] struct S { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0355"]);
    }

    #[test]
    fn watch_on_function_rejected_e0356() {
        let diags = check_src("#[watch] fn f() { return; }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn watch_on_enum_rejected_e0356() {
        let diags = check_src("#[watch] enum E { A, B }");
        assert_eq!(codes(&diags), vec!["E0356"]);
    }

    #[test]
    fn watch_duplicate_e0357() {
        let diags = check_src("#[watch] #[watch] struct S { x: i32 }");
        assert_eq!(codes(&diags), vec!["E0357"]);
    }

    /// reports/bug-26. Interfaces were validated with the STRUCT target mask,
    /// so struct-only attributes passed there and then did nothing —
    /// `#[watch]`'s write barrier, `#[repr(C)]`'s layout rule and `#[lang]`'s
    /// registry all look only at structs. A user who wrote one believed a
    /// feature was on. Interfaces have their own bit now, which no attribute
    /// sets, so any attribute on an interface is E0356.
    #[test]
    fn struct_only_attributes_on_an_interface_rejected_e0356() {
        for src in [
            "#[watch] interface I { fn p(this) -> i32; }",
            "#[repr(C)] interface I { fn p(this) -> i32; }",
            "#[lang(\"string\")] interface I { fn p(this) -> i32; }",
        ] {
            let diags = check_src(src);
            assert_eq!(codes(&diags), vec!["E0356"], "for: {src}");
        }
    }

    #[test]
    fn watch_composes_with_repr_c() {
        // Two distinct struct attributes on one declaration is not a
        // duplicate — the uniqueness rule is per-name.
        let diags = check_src("#[repr(C)] #[watch] struct S { x: i32 }");
        assert!(diags.is_empty(), "expected clean, got: {:?}", codes(&diags));
    }
}
