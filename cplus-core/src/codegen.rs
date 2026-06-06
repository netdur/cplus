//! Codegen: emit LLVM IR text from a sema-validated AST.
//!
//! Strategy (per plan §4.1): allocate every local with `alloca`, read/write
//! through `load`/`store`, let LLVM's `mem2reg` pass do the SSA conversion.
//! Avoids hand-rolled SSA construction.
//!
//! Phase 1 first cut: no overflow or div-by-zero checks. Sample programs don't
//! exercise those paths; they land as a refinement (`llvm.sadd.with.overflow.i32`
//! et al.) before Phase 2 begins.

use crate::ast::*;
use crate::sema::{EnumId, StructId, Ty};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// Slice 1B (v0.0.2): module-level metadata table. `!range` nodes need
/// module-unique IDs and must appear outside any function definition.
/// `register_range` allocates an ID, records the definition, and returns
/// the ID so codegen can splice `, !range !N` onto the relevant load/call.
///
/// IDs start at 100_000 to leave the 0..6 + 6.. range untouched for the
/// DWARF metadata block (see `emit_dwarf_metadata`). The two allocators do
/// not interleave because DWARF emits at module-end after codegen, and the
/// DWARF range never reaches 100_000 — that would require ~50k functions
/// in one module.
#[derive(Default)]
struct ModuleMetadata {
    next_id: Cell<u32>,
    nodes: RefCell<Vec<String>>,
    /// Cache so equal (lo, hi, ty_str) tuples share one MD node.
    cache: RefCell<HashMap<(i64, i64, &'static str), u32>>,
    /// v0.0.6 Slice 1A / v0.0.7 Slice 3.1: per-call-site
    /// `include_bytes!` / `include_str!` lookup. Populated by the
    /// module-init pass from sema's [`MonoInfo::compile_time_blobs`]
    /// (or empty when no mono is plumbed in). gen_expr for
    /// `ExprKind::IncludeBytes` / `ExprKind::IncludeStr` consults this
    /// map to produce the global's symbol + byte length; the AST node
    /// variant picks whether to emit a raw pointer or a `str`
    /// fat-pointer aggregate.
    compile_time_blobs: RefCell<HashMap<crate::lexer::Span, (String, u32)>>,
    /// v0.0.8 Phase 4: per-call-site `env!("NAME")` lookup. Populated by
    /// `emit_env_var_globals` from sema's `MonoInfo::env_vars`. Maps the
    /// macro call's span to `(global_symbol, value_byte_len)`. gen_expr
    /// for `ExprKind::EnvVar` reads this to build the `str` fat-pointer
    /// aggregate.
    env_var_globals: RefCell<HashMap<crate::lexer::Span, (String, u32)>>,
    /// v0.0.7 Slice 1.2: TBAA (Type-Based Alias Analysis) tree.
    /// Lazily populated on first `tbaa_tag_for` call. Layout:
    ///   - root: `!N = !{!"C+ TBAA Root"}`
    ///   - one leaf per primitive name, parented at root:
    ///     `!M = !{!"<name>", !N, i64 0}`
    /// Returned IDs are referenced from `!tbaa !M` clauses on
    /// load/store instructions emitted via `gen_load` / `gen_store`.
    /// Aggregate types (struct, enum, str, slice, string, simd) skip
    /// TBAA today — they get the conservative "may alias anything"
    /// treatment until v0.0.8 ships the per-field tree if raytracer
    /// perf justifies the complexity.
    tbaa_root: Cell<Option<u32>>,
    /// Per-TBAA-leaf cache, keyed by the leaf's name (`"i32"`, `"f64"`,
    /// `"ptr"`, or for aggregates the type's mangled name such as
    /// `"struct.Entry"` / `"enum.Color"` / `"arr16_u8"` / `"f32x4"`).
    /// The key is a `String` (not `&'static str`) so the v0.0.8
    /// finding 4 aggregate-leaf path doesn't need to leak strings.
    tbaa_leaves: RefCell<HashMap<String, u32>>,
    /// v0.0.8 bench-gap fix C: set of user-function mangled names that
    /// are safe to mark `fastcc`. Populated once per module before the
    /// per-item emission loop. A name is in this set iff:
    ///   1. The function has `internal` linkage in the LLVM module
    ///      (non-`pub` user fn, non-`pub` non-drop method).
    ///   2. The function's address is never taken anywhere in the
    ///      program (no bare-`Ident` reference resolving to it outside
    ///      a `Call` callee position).
    ///   3. The function isn't a drop method (those use
    ///      `preserve_nonecc`, which can't compose with `fastcc`).
    ///   4. The function isn't `main` (the C runtime requires C cc).
    /// `fastcc` lets LLVM pick its own register-passing convention for
    /// the callee, skipping the C-ABI's caller-saved register set on
    /// purely-internal calls. obs.md flagged this as a "few percent on
    /// call-heavy code" win, cumulative with fix A.
    fastcc_funcs: RefCell<HashSet<String>>,
    /// v0.0.9 Phase 4: module-scope `static` items. Populated by the
    /// `emit_statics` pre-pass from sema's `MonoInfo::statics`. Maps
    /// the static's qualified name (the same name that survives in
    /// `ExprKind::Ident` after lowering/resolver) to its resolved
    /// `Ty`. Reads and writes consult this map first; on hit, gen_expr
    /// emits a load/store against `@<name>` rather than the local-slot
    /// path. Pre-pass emission ensures the global exists before any
    /// function body references it.
    statics: RefCell<HashMap<String, Ty>>,
    /// v0.0.10 Phase 4A: per-selector cached-pointer globals. Populated
    /// by `emit_selector_globals` from sema's `MonoInfo::selectors`. Maps
    /// selector name → `(data_global, cached_global, byte_len)`. gen_intrinsic
    /// for `#selector(name)` looks up the pair and emits the
    /// load-cached / branch-if-null / sel_registerName-if-null pattern.
    selector_globals: RefCell<HashMap<String, (String, String, u32)>>,
    /// v0.0.10 Phase 4C: per-call shader-blob globals. Populated by
    /// `emit_shader_blob_globals` from sema's `MonoInfo::shader_blobs`.
    /// Maps the `#compile_shader(...)` call span → `(global_symbol, byte_len)`.
    /// gen_intrinsic for `#compile_shader` consults this map to produce
    /// the global's symbol; the result is a `*[u8; N]` pointing at it.
    shader_blob_globals: RefCell<HashMap<crate::lexer::Span, (String, u32)>>,
    /// v0.0.10 Phase 4A/B: whether the module has already declared
    /// `@sel_registerName` and `@objc_msgSend`. Set on first emission to
    /// avoid duplicate `declare` lines (LLVM rejects two declares of the
    /// same symbol with different shapes; we emit each exactly once).
    selector_runtime_declared: Cell<bool>,
    msg_send_declared: Cell<bool>,
    /// B-10: floating-point contraction policy. `true` (the default,
    /// matching clang's `-ffp-contract=on`) lets codegen emit
    /// `llvm.fmuladd` for source-level `a*b+c` and tag scalar/SIMD float
    /// arithmetic with the `contract` fast-math flag so the optimizer may
    /// fuse. `false` (`--fp-contract=off`) suppresses both, so float
    /// output is bit-identical to a C build compiled with
    /// `-ffp-contract=off`. Set once in `generate_inner`; the `Cell`
    /// default of `false` is never observed because that setter always
    /// runs before any function body is emitted.
    fp_contract: Cell<bool>,
}

impl ModuleMetadata {
    fn new() -> Self {
        Self {
            next_id: Cell::new(100_000),
            ..Self::default()
        }
    }
    /// Allocate a `!N = !{<ty> <lo>, <ty> <hi>}` range metadata node and
    /// return `N`. `hi` is exclusive per LLVM convention.
    fn register_range(&self, lo: i64, hi: i64, ty: &'static str) -> u32 {
        if let Some(&id) = self.cache.borrow().get(&(lo, hi, ty)) {
            return id;
        }
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.nodes
            .borrow_mut()
            .push(format!("!{id} = !{{{ty} {lo}, {ty} {hi}}}"));
        self.cache.borrow_mut().insert((lo, hi, ty), id);
        id
    }

    /// Slice 1C: allocate a self-referential `!alias.scope` domain node.
    /// Each function that has >= 2 noalias pointer params gets one domain.
    /// IR form: `!N = distinct !{!N, !"label"}` — the self-reference makes
    /// it distinct from any other domain even if labels collide.
    fn register_alias_domain(&self, label: &str) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.nodes
            .borrow_mut()
            .push(format!("!{id} = distinct !{{!{id}, !\"{label}\"}}"));
        id
    }

    /// Slice 1C: allocate a self-referential scope node tied to `domain_id`.
    /// IR form: `!N = distinct !{!N, !D, !"label"}`.
    fn register_alias_scope(&self, domain_id: u32, label: &str) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.nodes.borrow_mut().push(format!(
            "!{id} = distinct !{{!{id}, !{domain_id}, !\"{label}\"}}"
        ));
        id
    }

    /// Slice 1C: allocate a list of scope ids used by `!alias.scope` or
    /// `!noalias`. Both clauses take a list of scope refs.
    /// IR form: `!N = !{!S1, !S2, ...}`. Empty list returns the "empty
    /// scope-list" id, which has no effect — caller should skip emitting
    /// the attribute entirely in that case.
    fn register_alias_scope_list(&self, scopes: &[u32]) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let items: Vec<String> = scopes.iter().map(|s| format!("!{s}")).collect();
        self.nodes
            .borrow_mut()
            .push(format!("!{id} = !{{{}}}", items.join(", ")));
        id
    }

    /// v0.0.8 fix C: is this user-function's mangled name safe to emit
    /// with `fastcc`? Queried by both the function/method definition
    /// emitters (to compose `internal fastcc` linkage+cc) and every
    /// direct-call site emitter (to compose `call fastcc <ty> @name`).
    /// Returns false when the set hasn't been populated yet, so the
    /// default-cc path always works pre-pre-pass.
    fn is_fastcc(&self, mangled: &str) -> bool {
        self.fastcc_funcs.borrow().contains(mangled)
    }

    /// v0.0.8 fix C: shorthand for the `"fastcc "` prefix string when
    /// the named callee is fastcc, `""` otherwise. Caller concatenates
    /// directly into a `define` or `call` instruction.
    fn fastcc_prefix(&self, mangled: &str) -> &'static str {
        if self.is_fastcc(mangled) {
            "fastcc "
        } else {
            ""
        }
    }

    /// v0.0.7 Slice 1.2: return the TBAA leaf ID for a primitive `ty`,
    /// allocating the root + the relevant leaf on first use. Pointer
    /// types share a single `"ptr"` leaf.
    ///
    /// v0.0.8 bench-gap finding 4: aggregate types (struct, enum,
    /// array, simd, slice, str, string) now get their own leaves too,
    /// keyed by the type's structural name. Each unique aggregate
    /// type produces one leaf — `*Entry` vs `*Bucket` no longer
    /// alias under LLVM's analysis. The naming scheme is whole-type
    /// (not struct-path), which is enough to disambiguate distinct
    /// types but not enough to disambiguate two fields within one
    /// struct. Per-field paths are a v0.0.9+ exercise.
    fn tbaa_tag_for(&self, ty: &Ty, types: &TypeTable) -> Option<u32> {
        let name: String = match ty {
            Ty::I8 => "i8".into(),
            Ty::U8 => "u8".into(),
            Ty::Bool => "bool".into(),
            Ty::I16 => "i16".into(),
            Ty::U16 => "u16".into(),
            Ty::I32 => "i32".into(),
            Ty::U32 => "u32".into(),
            Ty::I64 => "i64".into(),
            Ty::U64 => "u64".into(),
            Ty::Isize => "isize".into(),
            Ty::Usize => "usize".into(),
            Ty::F16 => "f16".into(),
        Ty::F32 => "f32".into(),
            Ty::F64 => "f64".into(),
            Ty::RawPtr(_) | Ty::FnPtr { .. } => "ptr".into(),
            // v0.0.8 bench-gap finding 4: aggregate leaves keyed by
            // structural name.
            Ty::Struct(id) => format!("struct.{}", types.struct_defs[id.0 as usize].name),
            Ty::Enum(id) => {
                let info = &types.enum_defs[id.0 as usize];
                let n = types
                    .enum_by_name
                    .iter()
                    .find_map(|(name, eid)| (*eid == *id).then(|| name.clone()))
                    .unwrap_or_else(|| format!("enum_{}", id.0));
                if info.is_tagged {
                    format!("enum.{n}")
                } else {
                    // Plain enums lower to `i32` — share the i32 leaf
                    // so user code that mixes them with raw i32 reads
                    // through the same TBAA cell.
                    "i32".into()
                }
            }
            Ty::Array(elem, n) => {
                // Recurse to get the element's leaf name; fall back to
                // a synthetic if the element is itself an aggregate
                // without a registered leaf (shouldn't happen post-
                // monomorphization, but keep the helper total).
                let elem_name = match self.tbaa_leaf_name_for(elem, types) {
                    Some(s) => s,
                    None => "any".into(),
                };
                format!("arr{n}_{elem_name}")
            }
            Ty::Simd { elem, lanes } => {
                let elem_name = match self.tbaa_leaf_name_for(elem, types) {
                    Some(s) => s,
                    None => "any".into(),
                };
                format!("{elem_name}x{lanes}")
            }
            Ty::Slice(_) => "slice".into(),
            Ty::Str => "str".into(),
            Ty::String => "string".into(),
            // No TBAA for type params (never reach codegen) / Unit / Error.
            _ => return None,
        };
        if let Some(&id) = self.tbaa_leaves.borrow().get(&name) {
            return Some(id);
        }
        let root = match self.tbaa_root.get() {
            Some(id) => id,
            None => {
                let id = self.next_id.get();
                self.next_id.set(id + 1);
                self.nodes
                    .borrow_mut()
                    .push(format!("!{id} = !{{!\"C+ TBAA Root\"}}"));
                self.tbaa_root.set(Some(id));
                id
            }
        };
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.nodes
            .borrow_mut()
            .push(format!("!{id} = !{{!\"{name}\", !{root}, i64 0}}"));
        self.tbaa_leaves.borrow_mut().insert(name, id);
        Some(id)
    }

    /// Helper: compute the TBAA leaf *name* for `ty` without allocating
    /// a leaf node. Used by `tbaa_tag_for` when building composite
    /// names (`arrN_<elem>`, `<elem>x<lanes>`).
    fn tbaa_leaf_name_for(&self, ty: &Ty, types: &TypeTable) -> Option<String> {
        match ty {
            Ty::I8 => Some("i8".into()),
            Ty::U8 => Some("u8".into()),
            Ty::Bool => Some("bool".into()),
            Ty::I16 => Some("i16".into()),
            Ty::U16 => Some("u16".into()),
            Ty::I32 => Some("i32".into()),
            Ty::U32 => Some("u32".into()),
            Ty::I64 => Some("i64".into()),
            Ty::U64 => Some("u64".into()),
            Ty::Isize => Some("isize".into()),
            Ty::Usize => Some("usize".into()),
            Ty::F16 => Some("f16".into()),
            Ty::F32 => Some("f32".into()),
            Ty::F64 => Some("f64".into()),
            Ty::RawPtr(_) | Ty::FnPtr { .. } => Some("ptr".into()),
            Ty::Struct(id) => Some(format!("struct.{}", types.struct_defs[id.0 as usize].name)),
            Ty::Enum(id) => {
                let info = &types.enum_defs[id.0 as usize];
                if info.is_tagged {
                    let n = types
                        .enum_by_name
                        .iter()
                        .find_map(|(name, eid)| (*eid == *id).then(|| name.clone()))
                        .unwrap_or_else(|| format!("enum_{}", id.0));
                    Some(format!("enum.{n}"))
                } else {
                    Some("i32".into())
                }
            }
            _ => None,
        }
    }

    /// Drain the accumulated metadata definitions into the output. Must be
    /// called once at module-end, after all function bodies are emitted.
    fn emit_into(&self, out: &mut String) {
        let nodes = self.nodes.borrow();
        if nodes.is_empty() {
            return;
        }
        for line in nodes.iter() {
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// v0.0.3 Phase 5 Slice 5B: per-module registry of unique trampolines
/// needed for `thread::spawn` / `thread::spawn_with` call sites. Each
/// intrinsic call site registers its `O` (for spawn) or `(I, O)` (for
/// spawn_with) tuple; after all function bodies are emitted,
/// `emit_thread_trampolines` walks the registry and emits one
/// `define internal ptr @<sym>(ptr %arg)` per unique entry. Modeled
/// on `ModuleMetadata`.
///
/// **Layout convention (shared by spawn / spawn_with):**
///
/// ```text
/// offset 0:                fn pointer (8 bytes)
/// offset 8:                result slot, size_of(O) bytes
/// offset 8 + size_of(O):   input slot, size_of(I) bytes (spawn_with only)
/// ```
/// Keeping the result at a fixed offset of 8 lets `__cplus_thread_join`
/// be the same for both forms — it doesn't need to know whether the
/// thread was spawned with an input.
#[derive(Debug, Clone)]
enum TrampolineSpec {
    Spawn { o: Ty },
    SpawnWith { i: Ty, o: Ty },
}

struct ThreadTrampolines {
    /// Insertion-order list (for stable emit order + numeric indices
    /// for spawn_with symbols).
    specs: std::cell::RefCell<Vec<TrampolineSpec>>,
    /// Dedup keys: mangled-suffix string per entry.
    seen: std::cell::RefCell<std::collections::HashMap<String, usize>>,
}

impl ThreadTrampolines {
    fn new() -> Self {
        Self {
            specs: std::cell::RefCell::new(Vec::new()),
            seen: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// Register a `spawn[O]` trampoline. Returns the symbol name (no
    /// `@` prefix). `types` is needed when `O` mentions struct or enum
    /// types — sema's `mangle_ty_for_name` uses their names to build
    /// the JoinHandle suffix, so codegen must match.
    fn register_spawn(&self, o_ty: &Ty, types: &TypeTable) -> String {
        let suffix = mangle_o_for_tramp_with_types(o_ty, Some(types));
        let key = format!("spawn:{suffix}");
        let mut seen = self.seen.borrow_mut();
        if !seen.contains_key(&key) {
            let idx = self.specs.borrow().len();
            seen.insert(key, idx);
            self.specs
                .borrow_mut()
                .push(TrampolineSpec::Spawn { o: o_ty.clone() });
        }
        format!("__cplus_thread_tramp_{suffix}")
    }

    /// Register a `spawn_with[I, O]` trampoline. The (I, O) pair gets
    /// a monotonically-increasing index — struct/aggregate Tys would
    /// be awkward to mangle into the symbol directly. Returns the
    /// symbol name (no `@` prefix).
    fn register_spawn_with(&self, i_ty: &Ty, o_ty: &Ty) -> String {
        let key = format!("with:{:?}:{:?}", i_ty, o_ty);
        let mut seen = self.seen.borrow_mut();
        if let Some(&idx) = seen.get(&key) {
            return format!("__cplus_thread_tramp_with_{idx}");
        }
        let idx = self.specs.borrow().len();
        seen.insert(key, idx);
        self.specs.borrow_mut().push(TrampolineSpec::SpawnWith {
            i: i_ty.clone(),
            o: o_ty.clone(),
        });
        format!("__cplus_thread_tramp_with_{idx}")
    }
}

/// Compute a stable suffix for a Ty to use in the trampoline symbol.
/// Limited to the Copy-≤8-bytes subset that Slice 5B supports; codegen
/// errors out elsewhere if O is something else, so we don't try to
/// mangle aggregates here.
fn mangle_o_for_tramp(ty: &Ty) -> String {
    mangle_o_for_tramp_with_types(ty, None)
}

/// v0.0.4 Phase 1F: recursive type-name mangler matching sema's
/// `mangle_ty_for_name`. Recurses through `*T`, `T[]`, `[N]T`, `fn(...) -> T`
/// so `JoinHandle__<suffix>` lookups hit the monomorphized struct for
/// non-scalar `O`. Struct / Enum names come from the type table; passing
/// `None` for `types` falls back to a `"struct?"` placeholder (only
/// reachable in error-recovery paths where the sema-side mangle would
/// also have produced a placeholder).
fn mangle_o_for_tramp_with_types(ty: &Ty, types: Option<&TypeTable>) -> String {
    match ty {
        Ty::I8 => "i8".into(),
        Ty::I16 => "i16".into(),
        Ty::I32 => "i32".into(),
        Ty::I64 => "i64".into(),
        Ty::U8 => "u8".into(),
        Ty::U16 => "u16".into(),
        Ty::U32 => "u32".into(),
        Ty::U64 => "u64".into(),
        Ty::Isize => "isize".into(),
        Ty::Usize => "usize".into(),
        Ty::F16 => "f16".into(),
        Ty::F32 => "f32".into(),
        Ty::F64 => "f64".into(),
        Ty::Bool => "bool".into(),
        Ty::Unit => "unit".into(),
        Ty::Str => "str".into(),
        Ty::String => "string".into(),
        Ty::RawPtr(inner) => format!("ptr_{}", mangle_o_for_tramp_with_types(inner, types)),
        Ty::Slice(inner) => format!("slice_{}", mangle_o_for_tramp_with_types(inner, types)),
        Ty::Array(elem, n) => format!("arr{}_{}", n, mangle_o_for_tramp_with_types(elem, types)),
        Ty::FnPtr {
            params,
            return_type,
        } => {
            let mut s = String::from("fn");
            for p in params {
                s.push('_');
                s.push_str(&mangle_o_for_tramp_with_types(p, types));
            }
            if !matches!(**return_type, Ty::Unit) {
                s.push_str("_ret_");
                s.push_str(&mangle_o_for_tramp_with_types(return_type, types));
            }
            s
        }
        Ty::Struct(id) => match types {
            Some(t) => t.struct_defs[id.0 as usize].name.clone(),
            None => "struct?".into(),
        },
        Ty::Enum(id) => match types {
            // codegen's `EnumInfo` doesn't carry the source name; reverse
            // through `enum_by_name`. Slow but only fires when a thread/
            // async return is enum-typed, which is rare.
            Some(t) => t
                .enum_by_name
                .iter()
                .find_map(|(name, eid)| (eid == id).then(|| name.clone()))
                .unwrap_or_else(|| "enum?".into()),
            None => "enum?".into(),
        },
        Ty::Simd { elem, lanes } => {
            format!("{}x{}", mangle_o_for_tramp_with_types(elem, types), lanes)
        }
        // Masks share LLVM lowering with the matching Simd; use a
        // distinct mangled prefix so the trampoline tables don't clash
        // if both forms ever fly across a thread boundary.
        Ty::Mask { elem, lanes } => {
            format!("mask{}x{}", mangle_o_for_tramp_with_types(elem, types), lanes)
        }
        Ty::Param(n) => format!("Param_{n}"),
        Ty::Error => "ERR".into(),
    }
}

/// v0.0.3 Phase 5 Slice 5B: emit one trampoline definition per unique
/// `O` registered during function-body codegen. The trampoline is the
/// `start_routine` handed to `pthread_create`; it reads the user's
/// `fn() -> O` pointer from offset 0 of the malloc'd context, calls
/// it, and writes the result at offset 8. Returns `null` (pthread's
/// `void*` return is ignored — the result-passing channel is the
/// context buffer that `join` reads from).
fn emit_thread_trampolines(out: &mut String, tramps: &ThreadTrampolines, types: &TypeTable) {
    let specs = tramps.specs.borrow().clone();
    if specs.is_empty() {
        return;
    }
    out.push_str("\n; --- v0.0.3 Phase 5 Slice 5B/5C: thread spawn trampolines ---\n");
    for (idx, spec) in specs.iter().enumerate() {
        match spec {
            TrampolineSpec::Spawn { o } => emit_spawn_tramp(out, o, types),
            TrampolineSpec::SpawnWith { i, o } => emit_spawn_with_tramp(out, idx, i, o, types),
        }
    }
    out.push('\n');
}

fn align_of_ty(ty: &Ty, types: &TypeTable) -> u64 {
    static_layout(ty, types).map(|(_s, a)| a).unwrap_or(8)
}

fn emit_spawn_tramp(out: &mut String, o_ty: &Ty, types: &TypeTable) {
    let suffix = mangle_o_for_tramp_with_types(o_ty, Some(types));
    let llvm_t = llvm_ty(o_ty, types);
    let align = align_of_ty(o_ty, types);
    // v0.0.4 Phase 2 Slice 2H: ctx layout has a u64 refcount at offset 0.
    // Fn pointer moved to offset 8; result slot to offset 16. After the
    // worker writes its result, it atomically decrements the refcount;
    // if it was the last reference (prev == 1), it frees `ctx`. The
    // refcount lets `JoinHandle::drop` switch from blocking-join to true
    // fire-and-forget detach without racing the worker.
    out.push_str(&format!(
        "define internal ptr @__cplus_thread_tramp_{suffix}(ptr %arg) {{\n"
    ));
    out.push_str("entry:\n");
    if matches!(o_ty, Ty::Unit) {
        out.push_str(
            "  %f = load ptr, ptr getelementptr inbounds (i8, ptr %arg, i64 8), align 8\n",
        );
        out.push_str("  call void %f()\n");
    } else if return_passes_by_sret_widened(o_ty, types) {
        let (sz, al) = static_layout(o_ty, types).expect("sret thread-spawn O has layout");
        out.push_str("  %fptr = getelementptr inbounds i8, ptr %arg, i64 8\n");
        out.push_str("  %f = load ptr, ptr %fptr, align 8\n");
        out.push_str("  %slot = getelementptr i8, ptr %arg, i64 16\n");
        out.push_str(&format!(
            "  call void %f(ptr sret({llvm_t}) noalias nonnull noundef writable dereferenceable({sz}) align {al} %slot)\n"
        ));
    } else {
        out.push_str("  %fptr = getelementptr inbounds i8, ptr %arg, i64 8\n");
        out.push_str("  %f = load ptr, ptr %fptr, align 8\n");
        out.push_str(&format!("  %r = call {llvm_t} %f()\n"));
        out.push_str("  %slot = getelementptr i8, ptr %arg, i64 16\n");
        out.push_str(&format!("  store {llvm_t} %r, ptr %slot, align {align}\n"));
    }
    // Atomic refcount decrement. AcqRel: release pairs with prior result
    // store; acquire on the decrement-from-1 path ensures the freeing
    // thread sees the parent's last writes (if any) before deallocation.
    // Prev == 1 means worker was the last reference; free the ctx.
    out.push_str("  %prev = atomicrmw sub ptr %arg, i64 1 acq_rel\n");
    out.push_str("  %was_last = icmp eq i64 %prev, 1\n");
    out.push_str("  br i1 %was_last, label %free_bb, label %ret_bb\n");
    out.push_str("free_bb:\n");
    out.push_str("  call void @free(ptr %arg)\n");
    out.push_str("  br label %ret_bb\n");
    out.push_str("ret_bb:\n");
    out.push_str("  ret ptr null\n");
    out.push_str("}\n");
}

fn emit_spawn_with_tramp(out: &mut String, idx: usize, i_ty: &Ty, o_ty: &Ty, types: &TypeTable) {
    let i_llvm = llvm_ty(i_ty, types);
    let o_llvm = llvm_ty(o_ty, types);
    let o_align = align_of_ty(o_ty, types);
    let i_align = align_of_ty(i_ty, types);
    let o_size = static_layout(o_ty, types).map(|(s, _)| s).unwrap_or(8);
    // v0.0.4 Phase 2 Slice 2H ctx layout:
    //   refcount: u64       @ 0
    //   fn_ptr:             @ 8
    //   result_slot:        @ 16
    //   input_slot:         @ 16 + size_of(O) (aligned to align_of(I))
    let input_off_unaligned = 16 + o_size;
    let input_off = (input_off_unaligned + i_align - 1) & !(i_align - 1);
    out.push_str(&format!(
        "define internal ptr @__cplus_thread_tramp_with_{idx}(ptr %arg) {{\n"
    ));
    out.push_str("entry:\n");
    out.push_str("  %fptr = getelementptr inbounds i8, ptr %arg, i64 8\n");
    out.push_str("  %f = load ptr, ptr %fptr, align 8\n");
    out.push_str(&format!(
        "  %input_slot = getelementptr i8, ptr %arg, i64 {input_off}\n"
    ));
    out.push_str(&format!(
        "  %i = load {i_llvm}, ptr %input_slot, align {i_align}\n"
    ));
    if matches!(o_ty, Ty::Unit) {
        out.push_str(&format!("  call void %f({i_llvm} %i)\n"));
    } else {
        out.push_str(&format!("  %r = call {o_llvm} %f({i_llvm} %i)\n"));
        out.push_str("  %result_slot = getelementptr i8, ptr %arg, i64 16\n");
        out.push_str(&format!(
            "  store {o_llvm} %r, ptr %result_slot, align {o_align}\n"
        ));
    }
    // Refcount dec + maybe-free, same shape as emit_spawn_tramp.
    out.push_str("  %prev = atomicrmw sub ptr %arg, i64 1 acq_rel\n");
    out.push_str("  %was_last = icmp eq i64 %prev, 1\n");
    out.push_str("  br i1 %was_last, label %free_bb, label %ret_bb\n");
    out.push_str("free_bb:\n");
    out.push_str("  call void @free(ptr %arg)\n");
    out.push_str("  br label %ret_bb\n");
    out.push_str("ret_bb:\n");
    out.push_str("  ret ptr null\n");
    out.push_str("}\n");
}

/// Slice 5B eligibility: O must be a primitive scalar whose mangled
/// suffix matches sema's `mangle_ty_for_name` output (so the runtime
/// `struct_by_name.get("JoinHandle__<suffix>")` lookup hits). Raw
/// pointers + fn pointers land in 5C alongside the cross-thread
/// move work — their mangled forms include the inner pointee type
/// and need a recursive name-builder that codegen doesn't expose
/// yet. Aggregates (structs/enums/arrays/slices) and `string` need
/// sret-aware trampolines and join paths — those land in 5C as well.
fn is_thread_spawn_eligible(ty: &Ty) -> bool {
    match ty {
        Ty::I8
        | Ty::I16
        | Ty::I32
        | Ty::I64
        | Ty::U8
        | Ty::U16
        | Ty::U32
        | Ty::U64
        | Ty::Isize
        | Ty::Usize
        | Ty::F16
        | Ty::F32
        | Ty::F64
        | Ty::Bool
        | Ty::Unit
        | Ty::String => true,
        // v0.0.4 Phase 1F: raw / fn / struct / enum / array O. The
        // trampoline path handles them via either the value-return or
        // sret-return branch (depending on `return_passes_by_sret_widened`),
        // and `mangle_o_for_tramp_with_types` builds matching symbols.
        Ty::RawPtr(_) | Ty::FnPtr { .. } | Ty::Struct(_) | Ty::Enum(_) | Ty::Array(_, _) => true,
        // Slice (`T[]`) is a fat pointer borrowing external storage; a
        // worker that returns one would hand the parent a dangling
        // pointer once the worker's stack unwinds. Reject explicitly.
        Ty::Slice(_) => false,
        // str: same hazard as Slice.
        Ty::Str => false,
        // v0.0.6 Slice 1B: SIMD vectors are Copy + register-sized; safe
        // to return from a worker just like an integer. Masks share
        // the same LLVM lowering, so the same trampoline path works.
        Ty::Simd { .. } | Ty::Mask { .. } => true,
        Ty::Param(_) | Ty::Error => false,
    }
}

/// v0.0.3 Phase 5 Slice 5E.3: find the `Future[T]` struct in the
/// type table given the inner `T`. Suffix-matches `Future__<mangle(T)>`
/// the same way `lookup_join_handle_ty` matches the JoinHandle name.
fn lookup_future_ty(inner: &Ty, types: &TypeTable) -> Ty {
    let target = format!(
        "Future__{}",
        mangle_o_for_tramp_with_types(inner, Some(types))
    );
    let dotted = format!(".{target}");
    for (idx, d) in types.struct_defs.iter().enumerate() {
        if d.name == target || d.name.ends_with(&dotted) {
            return Ty::Struct(StructId(idx as u32));
        }
    }
    Ty::Struct(StructId(0))
}

/// v0.0.4 Phase 4 Slice 4A: mirror of `lookup_future_ty` for `Iterator[T]`.
fn lookup_iterator_ty(inner: &Ty, types: &TypeTable) -> Ty {
    let target = format!(
        "Iterator__{}",
        mangle_o_for_tramp_with_types(inner, Some(types))
    );
    let dotted = format!(".{target}");
    for (idx, d) in types.struct_defs.iter().enumerate() {
        if d.name == target || d.name.ends_with(&dotted) {
            return Ty::Struct(StructId(idx as u32));
        }
    }
    Ty::Struct(StructId(0))
}

/// v0.0.3 Phase 5 Slice 5E.3: given the monomorphized struct name of
/// a `Future[U]` instantiation (e.g. `Future__i32`), recover U as a
/// `Ty`. Mirrors `mangle_o_for_tramp`'s naming — supports the same
/// scalar set the async return-type restriction allows.
fn ty_from_future_name(name: &str, types: &TypeTable) -> Ty {
    let suffix = name.rsplit_once("Future__").map(|(_, s)| s).unwrap_or(name);
    ty_from_suffix(suffix, types)
}

fn ty_from_suffix(suffix: &str, types: &TypeTable) -> Ty {
    if suffix == "i8" { return Ty::I8; }
    if suffix == "i16" { return Ty::I16; }
    if suffix == "i32" { return Ty::I32; }
    if suffix == "i64" { return Ty::I64; }
    if suffix == "u8" { return Ty::U8; }
    if suffix == "u16" { return Ty::U16; }
    if suffix == "u32" { return Ty::U32; }
    if suffix == "u64" { return Ty::U64; }
    if suffix == "isize" { return Ty::Isize; }
    if suffix == "usize" { return Ty::Usize; }
    if suffix == "f32" { return Ty::F32; }
    if suffix == "f64" { return Ty::F64; }
    if suffix == "bool" { return Ty::Bool; }
    if suffix == "unit" { return Ty::Unit; }
    if suffix == "str" { return Ty::Str; }
    if suffix == "string" { return Ty::String; }
    if suffix == "ERR" { return Ty::Error; }

    if let Some(inner_suffix) = suffix.strip_prefix("ptr_") {
        let inner_ty = ty_from_suffix(inner_suffix, types);
        if inner_ty == Ty::Error {
            return Ty::Error;
        }
        return Ty::RawPtr(Box::new(inner_ty));
    }

    if let Some(inner_suffix) = suffix.strip_prefix("slice_") {
        let inner_ty = ty_from_suffix(inner_suffix, types);
        if inner_ty == Ty::Error {
            return Ty::Error;
        }
        return Ty::Slice(Box::new(inner_ty));
    }

    if suffix.starts_with("arr") {
        if let Some(idx) = suffix.find('_') {
            if let Ok(n) = suffix[3..idx].parse::<usize>() {
                let elem_suffix = &suffix[idx + 1..];
                let elem_ty = ty_from_suffix(elem_suffix, types);
                if elem_ty == Ty::Error {
                    return Ty::Error;
                }
                return Ty::Array(Box::new(elem_ty), n.try_into().unwrap());
            }
        }
    }

    if let Some(idx) = suffix.rfind('x') {
        if let Ok(lanes) = suffix[idx + 1..].parse::<usize>() {
            let elem_suffix = &suffix[..idx];
            let elem_ty = ty_from_suffix(elem_suffix, types);
            if elem_ty != Ty::Error {
                return Ty::Simd {
                    elem: Box::new(elem_ty),
                    lanes: lanes.try_into().unwrap(),
                };
            }
        }
    }

    if let Some(inner_suffix) = suffix.strip_prefix("Param_") {
        return Ty::Param(inner_suffix.to_string());
    }

    let dotted = format!(".{suffix}");
    for (idx, d) in types.struct_defs.iter().enumerate() {
        if d.name == suffix || d.name.ends_with(&dotted) {
            return Ty::Struct(StructId(idx as u32));
        }
    }

    for (name, id) in &types.enum_by_name {
        if name == suffix || name.ends_with(&dotted) {
            return Ty::Enum(*id);
        }
    }

    Ty::Error
}

/// v0.0.3 Phase 5 Slice 5C input eligibility. Like
/// `is_thread_spawn_eligible` for scalars, but also accepts Copy
/// structs (the canonical `Range`-style input for the parallel-sum
/// recipe is a struct, not a scalar). Strings and Vec[T] are non-Copy
/// — sema's `check_arg_with_move` enforces the `move` at the call
/// site so the trampoline can read+pass them, but the typed store
/// into ctx works the same as for Copy.
fn is_thread_input_eligible(ty: &Ty, types: &TypeTable) -> bool {
    if is_thread_spawn_eligible(ty) {
        return true;
    }
    // v0.0.3 Slice 5D: raw pointers + fn pointers accepted as input —
    // they're Copy, 8-byte, LLVM `ptr`-typed. The concurrent-counter
    // recipe shares a `*u64` between worker threads via this path.
    if matches!(ty, Ty::RawPtr(_) | Ty::FnPtr { .. }) {
        return true;
    }
    // Copy structs (no Drop, all-Copy fields) lay out cleanly.
    if let Ty::Struct(id) = ty {
        let info = &types.struct_defs[id.0 as usize];
        if info.is_copy {
            return true;
        }
        // Non-Copy structs (e.g. Vec[T], string-shaped wrappers): the
        // worker takes ownership via the typed store, so this is
        // technically safe — but ownership transfer of non-Copy I
        // depends on the parent flipping its drop flag at the
        // spawn_with call site (sema's `move` machinery handles that),
        // and the worker actually freeing if the worker's f does not.
        // For 5C v1 we accept structs; richer support for Vec[T] /
        // string returns lands when we add sret-aware trampolines.
        return true;
    }
    // Accept strings + slices as moveable input by value (they're
    // fat-pointer aggregates; typed store/load works).
    matches!(ty, Ty::Str | Ty::String | Ty::Slice(_))
}

/// Build mode controls overflow checking on plain `+ - *`. Division-by-zero
/// trapping is emitted regardless of mode (per plan §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    /// Debug: insert `llvm.{sadd,ssub,smul}.with.overflow.i32` + `llvm.trap`
    /// around `+ - *`. Matches Rust's debug-mode arithmetic.
    Debug,
    /// Release: emit plain `add` / `sub` / `mul`. Wrapping is defined per §2.3.
    Release,
}

/// Generate LLVM IR for a sema-validated program. Caller must run sema first;
/// codegen will panic on unresolvable references that sema would have caught.
pub fn generate(program: &Program, mode: BuildMode) -> String {
    generate_inner(
        program,
        mode,
        true,
        None,
        None,
        &[],
        false,
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
}

/// v0.0.6 Slice 1A / v0.0.7 Slice 3.1: codegen entry point for the
/// post-monomorphize pipeline, taking sema's [`MonoInfo`] so
/// `include_bytes!` / `include_str!` calls can resolve to the bytes
/// sema read at type-check time.
pub fn generate_with_mono(
    program: &Program,
    mode: BuildMode,
    fp_contract: bool,
    debug_source: Option<&std::path::Path>,
    sanitizers: &[&str],
    is_lib: bool,
    mono: &crate::sema::MonoInfo,
) -> String {
    generate_inner(
        program,
        mode,
        fp_contract,
        None,
        debug_source,
        sanitizers,
        is_lib,
        &mono.compile_time_blobs,
        &mono.env_vars,
        &mono.statics,
        &mono.selectors,
        &mono.shader_blobs,
    )
}

/// Phase 5 Slice 5.B: generate IR for a library target. Non-`pub` items
/// get `internal` linkage so LTO can strip unused implementation detail
/// from the final `.dylib` / `.a`. `pub` items keep external linkage and
/// form the C-callable public ABI.
///
/// Distinct from `generate` so existing executable builds (and the
/// substring-pinned test suite that goes with them) keep their current
/// linkage exactly. Eventually the bin path can share this rule too —
/// internal linkage is correct everywhere — but ship + verify first.
pub fn generate_lib(program: &Program, mode: BuildMode) -> String {
    generate_inner(
        program,
        mode,
        true,
        None,
        None,
        &[],
        true,
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
}

/// Phase 11 polish (2026-05-13): emit LLVM IR with DWARF debug
/// metadata. v1 ships function-level info only — DICompileUnit, DIFile,
/// and one DISubprogram per function. Per-instruction `!DILocation`
/// (for line-by-line stepping) is a follow-up. lldb can still identify
/// function symbols in stack traces and set breakpoints by name.
pub fn generate_with_debug(
    program: &Program,
    mode: BuildMode,
    source_file: &std::path::Path,
) -> String {
    generate_inner(
        program,
        mode,
        true,
        None,
        Some(source_file),
        &[],
        false,
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
}

/// Phase 11 polish (2026-05-13): emit LLVM IR with sanitizer function
/// attributes (`sanitize_address`, `sanitize_thread`, `sanitize_memory`)
/// attached to every user-defined `define`. Required for clang's
/// sanitizer passes to instrument code that originates from a `.ll`
/// (the C path auto-attaches these; the IR path doesn't).
pub fn generate_with_options(
    program: &Program,
    mode: BuildMode,
    source_file: Option<&std::path::Path>,
    sanitizers: &[&str],
) -> String {
    generate_inner(
        program,
        mode,
        true,
        None,
        source_file,
        sanitizers,
        false,
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
}

/// Slice 5ATTR.4: emit a test-runner binary. The output IR contains every
/// `#[test]` function in the program plus a synthesized `main` that:
///   1. Zeroes `@cpc_test_failed` before each test,
///   2. Calls the test,
///   3. For `fn() -> i32` tests, ORs the return value into the failure check,
///   4. Prints `test <name> ... ok` / `... FAILED` (or one JSON line per
///      test when `json` is true),
///   5. Tracks pass / fail counts and returns the fail count.
///
/// `assert` statements throughout the project lower to a flag write instead
/// of `llvm.trap` so a failed assertion sets the flag, the test function
/// falls through (Phase 5 has no raw pointers — fall-through is safe), and
/// the driver reads the flag after the call.
///
/// The project's user-defined `fn main` (if any) is *not* emitted; the
/// synthesized driver replaces it.
pub fn generate_test_binary(
    program: &Program,
    mode: BuildMode,
    tests: &[crate::attrs::TestFn],
    json: bool,
    mono: &crate::sema::MonoInfo,
) -> String {
    generate_inner(
        program,
        mode,
        true,
        Some(TestDriverConfig { tests, json }),
        None,
        &[],
        false,
        &mono.compile_time_blobs,
        &mono.env_vars,
        &mono.statics,
        &mono.selectors,
        &mono.shader_blobs,
    )
}

struct TestDriverConfig<'a> {
    tests: &'a [crate::attrs::TestFn],
    json: bool,
}

fn generate_inner(
    program: &Program,
    mode: BuildMode,
    fp_contract: bool,
    test_cfg: Option<TestDriverConfig<'_>>,
    debug_source: Option<&std::path::Path>,
    sanitizers: &[&str],
    is_lib: bool,
    compile_time_blobs_map: &HashMap<crate::lexer::Span, crate::sema::CompileTimeBlobEntry>,
    env_vars_map: &HashMap<crate::lexer::Span, crate::sema::EnvVarEntry>,
    statics_map: &std::collections::BTreeMap<String, crate::sema::StaticInfo>,
    selectors_set: &std::collections::BTreeSet<String>,
    shader_blobs_map: &HashMap<crate::lexer::Span, Vec<u8>>,
) -> String {
    let types = collect_types(program);
    let sigs = collect_sigs(program, &types);
    let test_mode = test_cfg.is_some();
    let mut out = String::new();
    // Slice 1B: module-level `!range` metadata table. Allocated as we
    // codegen each function; flushed to `out` after every function body is
    // written and before DWARF (which has its own ID range).
    let md = ModuleMetadata::new();
    // B-10: record the fp-contraction policy before any function body emits.
    md.fp_contract.set(fp_contract);
    // v0.0.8 bench-gap fix C: compute the fastcc-eligible set once. A
    // user-defined function or method gets `fastcc` iff it has internal
    // linkage (non-`pub`, non-`main`, non-extern, non-drop) AND its
    // address is not taken anywhere in the program. Drop methods stay
    // on `preserve_nonecc` — fastcc can't compose with it. `main` keeps
    // C cc so the OS runtime can invoke it.
    {
        let address_taken = collect_address_taken_fns(program, &sigs);
        let mut fastcc = md.fastcc_funcs.borrow_mut();
        for item in &program.items {
            match &item.kind {
                ItemKind::Function(f) => {
                    if f.generic_params.is_empty()
                        && !f.is_pub
                        && !f.is_extern
                        && f.name.name != "main"
                        && !address_taken.contains(&f.name.name)
                    {
                        fastcc.insert(f.name.name.clone());
                    }
                }
                ItemKind::Impl(b) => {
                    let target_name = &b.target.name;
                    for m in &b.methods {
                        if m.generic_params.is_empty()
                            && !m.is_pub
                            && m.name.name != "drop"
                        {
                            fastcc.insert(mangle(target_name, &m.name.name));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let tramps = ThreadTrampolines::new();
    write_preamble(&mut out);
    if test_mode {
        // Shared per-test failure flag — written by `assert` in any function
        // called by a test, read by the driver `main` after each test call.
        out.push_str("@cpc_test_failed = global i32 0\n\n");
    }
    // Phase 8 slice 8.STR.1: collect every unique string literal in the
    // program, emit one `@.str.N` global per unique payload, build a
    // lookup table so gen_expr can resolve a literal to its global.
    let str_lits = collect_and_emit_str_lits(&mut out, program);
    // v0.0.6 Slice 1A / v0.0.7 Slice 3.1: emit one `@.bytes.N =
    // private unnamed_addr constant [N x i8] c"..."` global per
    // unique absolute path in sema's compile-time-blobs map. Populate
    // `md.compile_time_blobs` with the per-call-site lookup so
    // `gen_expr(ExprKind::IncludeBytes | ExprKind::IncludeStr)` can
    // emit the right shape (raw pointer or `str` fat-pointer).
    emit_compile_time_blob_globals(&mut out, compile_time_blobs_map, &md);
    emit_env_var_globals(&mut out, env_vars_map, &md);
    // v0.0.12 G-033 (llama.cplus G-032): struct type declarations must
    // precede static-global emission so `@NAME = global %S zeroinitializer`
    // (for `pub static NAME: S = #zero::[S]();`) lands in a context where
    // `%S` is already declared. Pre-fix `emit_statics` ran first and
    // clang's IR parser rejected the forward struct reference with
    // "invalid type for null constant". The selector + shader-blob
    // globals use primitive types only and stay where they are.
    write_struct_decls(&mut out, &types, program);
    // v0.0.9 Phase 4: emit one LLVM global per module-scope `static`.
    // Immutable statics → `@NAME = constant <ty> <lit>` (lives in
    // `.rodata`). Mutable statics → `@NAME = global <ty> <lit>` (lives
    // in `.data`). Populates `md.statics` so gen_expr / gen_assign
    // route Ident references through load/store against the symbol.
    emit_statics(&mut out, statics_map, &types, &md);
    // v0.0.10 Phase 4A: emit per-selector cached-pointer globals.
    emit_selector_globals(&mut out, selectors_set, &md);
    // v0.0.10 Phase 4C: emit per-call shader-blob globals.
    emit_shader_blob_globals(&mut out, shader_blobs_map, &md);
    // Phase 11 / ObjC interop: multiple `extern fn` declarations may share
    // a single linker symbol via `#[link_name = "..."]`. Track emitted
    // symbols so we never emit two `declare`s with the same name (LLVM
    // rejects that as a redefinition).
    let mut emitted_extern_symbols: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    // v0.0.3 Phase 5 Slice 5B: pre-seed with the symbols the preamble
    // declares so user-level `extern fn pthread_join(...)` style
    // declarations don't emit a duplicate `declare`. Same shape rule
    // applies to `malloc` / `free` / `memcpy` / `snprintf` / `printf`
    // which user code may legitimately re-declare to get a typed
    // handle.
    for sym in [
        "pthread_create",
        "pthread_join",
        "malloc",
        "free",
        "memcpy",
        "snprintf",
        "printf",
        // v0.0.4 Phase 3 Slice 3A.1: reactor preamble emits these as
        // internal definitions; pre-seed so stdlib/reactor.cplus's
        // `extern fn` re-declarations don't double-emit.
        "__cplus_reactor_get_state",
        "__cplus_reactor_set_state",
        "__cplus_coro_resume",
        "__cplus_coro_done",
    ] {
        emitted_extern_symbols.insert(sym.to_string());
    }
    // v0.0.10 Phase 4A/B: declare the ObjC runtime symbols we synthesize
    // calls to from `#selector` / `#msg_send` intrinsics. Pre-seed so
    // user-level `extern fn objc_msgSend(...)` / `extern fn sel_registerName(...)`
    // declarations don't double-emit. Always emitted when ANY selector is
    // present (the `#msg_send` declare costs nothing if unused — LLVM
    // strips unused declares).
    if !selectors_set.is_empty() {
        out.push_str("declare ptr @sel_registerName(ptr)\n");
        // CRITICAL: NOT variadic. On aarch64-apple-darwin, the ObjC
        // ABI requires `objc_msgSend` to be called with the exact
        // non-variadic signature — variadic-declared calls pass args
        // via the stack (per the AAPCS variadic rule) while libobjc
        // expects them in registers. Mismatching produces immediate
        // crashes in the trampoline. We emit a 2-arg non-variadic
        // declare; per-call sites emit `call <ret_ty> @objc_msgSend(...)`
        // with whatever typed arg list they need. Modern LLVM with
        // opaque pointers accepts the shape divergence between declare
        // and call (this is what the user-side `extern fn ... #[link_name = "objc_msgSend"]`
        // pattern relies on too).
        out.push_str("declare ptr @objc_msgSend(ptr, ptr)\n\n");
        emitted_extern_symbols.insert("sel_registerName".to_string());
        emitted_extern_symbols.insert("objc_msgSend".to_string());
    }
    for item in &program.items {
        match &item.kind {
            ItemKind::Function(f) => {
                // Test driver replaces the user's `main`. Other functions go
                // through unchanged so tests can call helpers, and so a
                // `#[test]` function's own body is emitted normally.
                if test_mode && f.name.name == "main" {
                    continue;
                }
                // Slice 7GEN.4: generic functions don't emit pre-monomorphization.
                // Slice 7GEN.5 will walk a work-queue of instantiations.
                if !f.generic_params.is_empty() {
                    continue;
                }
                gen_function(
                    &mut out,
                    f,
                    &sigs,
                    &types,
                    &str_lits,
                    mode,
                    test_mode,
                    &mut emitted_extern_symbols,
                    &md,
                    &tramps,
                    is_lib,
                );
            }
            ItemKind::Impl(b) => {
                if let Some(&id) = types.struct_by_name.get(&b.target.name) {
                    for m in &b.methods {
                        // Slice 7GEN.5e: generic methods are codegen-skipped
                        // pre-monomorphization. Their Ty::Param-bearing
                        // signatures and bodies are emitted as concrete
                        // copies by the monomorphize pass.
                        if !m.generic_params.is_empty() {
                            continue;
                        }
                        gen_method(
                            &mut out, id, m, &sigs, &types, &str_lits, mode, test_mode, &md,
                            &tramps, is_lib,
                        );
                    }
                } else if let Some(&enum_id) = types.enum_by_name.get(&b.target.name) {
                    // v0.0.5 Phase 2C: enum impl-method emission.
                    for m in &b.methods {
                        if !m.generic_params.is_empty() {
                            continue;
                        }
                        gen_enum_method(
                            &mut out, enum_id, m, &sigs, &types, &str_lits, mode, test_mode, &md,
                            &tramps,
                        );
                    }
                }
            }
            // Slice 7GEN.3: interface declarations have no runtime
            // presence — they're sema-time contracts. No IR emission.
            ItemKind::Interface(_) => {}
            // Phase 11 polish: type aliases are sema-only — resolved away
            // before codegen ever sees them.
            ItemKind::TypeAlias(_) => {}
            ItemKind::Enum(_) | ItemKind::Struct(_) => {
                // Enum types are erased to i32; struct types are declared
                // upfront in `write_struct_decls`. Nothing to emit per-item.
            }
            // v0.0.9 Phase 4: const items are lowered away by
            // `crate::lower::substitute_consts` before codegen runs.
            // Reaching this arm means lowering missed one — invariant
            // violation, but no IR to emit either way.
            ItemKind::Const(_) => {}
            // v0.0.9 Phase 4: static items are emitted as LLVM globals
            // in a dedicated pre-pass (`emit_statics`) just below.
            // Skip here so we don't double-emit.
            ItemKind::Static(_) => {}
            // v0.0.15: module-scope `#asm("...");` → LLVM `module asm "..."`.
            // A top-level entity, valid interspersed among the `define`s.
            // The template is emitted verbatim (escaped for the LLVM string
            // grammar); module asm has no operands, so `$` is not special.
            ItemKind::ModuleAsm(ma) => {
                out.push_str(&format!("module asm \"{}\"\n", escape_llvm_str(&ma.template)));
            }
        }
    }
    if let Some(cfg) = test_cfg {
        emit_test_driver_main(&mut out, cfg.tests, cfg.json);
    }
    // v0.0.3 Phase 5 Slice 5B: emit one `define internal ptr` trampoline
    // per unique O type registered during function-body codegen. Done
    // before metadata flushing so the trampoline bodies don't get
    // tangled with the metadata block.
    emit_thread_trampolines(&mut out, &tramps, &types);
    // Slice 1B: flush the accumulated `!N = !{...}` range metadata table
    // before DWARF (which writes its own metadata block). DWARF allocates
    // IDs starting at 0; our range table starts at 100_000 — disjoint.
    md.emit_into(&mut out);
    // Phase 11 polish (2026-05-13): DWARF debug metadata. v1 emits
    // module flags + DICompileUnit + DIFile + one DISubprogram per
    // function (named, line-numbered). Per-instruction DILocation is
    // a follow-up — for now lldb identifies function symbols in stack
    // traces and accepts `break <fn>` by name; line-stepping is
    // degenerate (lands at function entry).
    if let Some(path) = debug_source {
        let src = std::fs::read_to_string(path).ok();
        emit_dwarf_metadata(&mut out, program, path, src.as_deref());
    }
    if !sanitizers.is_empty() {
        attach_sanitizer_attrs(&mut out, sanitizers);
    }
    out
}

/// Phase 11 polish (2026-05-13): attach `sanitize_*` attributes to
/// every user-defined `define` line. clang's sanitizer passes only
/// instrument functions carrying these attributes; for source compiled
/// via clang the C frontend auto-attaches them, but cpc emits IR
/// directly so we do it here. Inline-attribute syntax keeps the IR
/// trivially diff-able without dragging in attribute groups.
fn attach_sanitizer_attrs(out: &mut String, sanitizers: &[&str]) {
    let attrs: Vec<&str> = sanitizers
        .iter()
        .filter_map(|s| match *s {
            "address" => Some("sanitize_address"),
            "thread" => Some("sanitize_thread"),
            "memory" => Some("sanitize_memory"),
            // UBSan doesn't gate on a function attribute — its checks are
            // inserted unconditionally by the pass.
            "undefined" => None,
            _ => None,
        })
        .collect();
    if attrs.is_empty() {
        return;
    }
    let attr_str = attrs.join(" ");
    let original = std::mem::take(out);
    for line in original.lines() {
        if line.starts_with("define ") {
            // Insert ` <attrs>` after the params' closing `)` and
            // before the `{`. v0.0.3 Slice 5D fix: track paren depth
            // so we don't land inside a nested `sret(%T)`-style
            // attribute. The function's param list opens at depth 1
            // and closes at depth 0; we attach right after that.
            let mut depth: i32 = 0;
            let mut close_idx: Option<usize> = None;
            for (i, c) in line.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close_idx = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close_paren) = close_idx {
                let after = &line[close_paren + 1..];
                let head = &line[..=close_paren];
                out.push_str(head);
                out.push(' ');
                out.push_str(&attr_str);
                out.push_str(after);
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
}

/// Emit module-flag + !llvm.dbg.cu + DICompileUnit + DIFile metadata,
/// plus one DISubprogram per program function. Post-processes the IR
/// to attach `!dbg !N` to each `define` line.
fn emit_dwarf_metadata(
    out: &mut String,
    program: &Program,
    source_file: &std::path::Path,
    src: Option<&str>,
) {
    let line_map = src.map(crate::diagnostics::LineMap::new);
    let abs = source_file
        .canonicalize()
        .unwrap_or_else(|_| source_file.to_path_buf());
    let filename = abs
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("source.cplus")
        .to_string();
    let directory = abs
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or(".")
        .to_string();
    // Reserve metadata ids:
    //   !0 = DwarfVersion flag
    //   !1 = DebugInfoVersion flag
    //   !2 = DICompileUnit
    //   !3 = DIFile
    //   !4 = DISubroutineType (shared "unknown type" placeholder)
    //   !5 = empty types array
    //   !6.. = pairs of (DISubprogram, DILocation) per fn
    let cu_id = 2u32;
    let file_id = 3u32;
    let sub_type_id = 4u32;
    let empty_types_id = 5u32;
    let mut next_md = 6u32;
    // First pass: collect function names + their start line.
    // Per function we allocate two metadata ids: a DISubprogram (the
    // `define ... !dbg !S` anchor) and a DILocation (the `!dbg !L`
    // attached to every call inside that function — required so clang
    // doesn't drop the whole DI block).
    struct FnDi {
        sub_id: u32,
        loc_id: u32,
        line: u32,
    }
    let mut fn_meta: std::collections::HashMap<String, FnDi> = std::collections::HashMap::new();
    let alloc_pair = |name: String,
                      line: u32,
                      fn_meta: &mut std::collections::HashMap<String, FnDi>,
                      next_md: &mut u32| {
        let sub_id = *next_md;
        let loc_id = *next_md + 1;
        *next_md += 2;
        fn_meta.insert(
            name,
            FnDi {
                sub_id,
                loc_id,
                line,
            },
        );
    };
    for item in &program.items {
        let ItemKind::Function(f) = &item.kind else {
            continue;
        };
        if !f.generic_params.is_empty() {
            continue;
        }
        let line = line_for_span(f.name.span, line_map.as_ref(), src);
        let user_name = if f.name.name == "main" {
            "main".to_string()
        } else {
            f.name.name.clone()
        };
        alloc_pair(user_name, line, &mut fn_meta, &mut next_md);
    }
    // Methods on impl blocks: emit one DISubprogram per method too.
    // The IR's `define` line uses the mangled `Type.method` name.
    for item in &program.items {
        let ItemKind::Impl(b) = &item.kind else {
            continue;
        };
        for m in &b.methods {
            if !m.generic_params.is_empty() {
                continue;
            }
            let mangled = format!("{}.{}", b.target.name, m.name.name);
            let line = line_for_span(m.name.span, line_map.as_ref(), src);
            alloc_pair(mangled, line, &mut fn_meta, &mut next_md);
        }
    }
    // Post-process the IR. Two attachments:
    //   1. `define ... !dbg !S { ... }` — attach the function's
    //      DISubprogram to the `define` line.
    //   2. `<call|invoke> ..., !dbg !L` — attach the function's
    //      DILocation to every call inside its body. clang rejects DI
    //      blocks where a call instruction inside a debug-info'd
    //      function lacks a `!dbg`.
    let original = std::mem::take(out);
    let mut current_loc_id: Option<u32> = None;
    for line in original.lines() {
        // Function definition line.
        if let Some(stripped) = line.strip_prefix("define ") {
            if let Some(at) = stripped.find('@') {
                let after_at = &stripped[at + 1..];
                if let Some(paren) = after_at.find('(') {
                    let raw_name = &after_at[..paren];
                    if let Some(meta) = fn_meta.get(raw_name) {
                        current_loc_id = Some(meta.loc_id);
                        if let Some(brace) = line.rfind('{') {
                            let (head, tail) = line.split_at(brace);
                            out.push_str(head.trim_end());
                            out.push_str(&format!(" !dbg !{} ", meta.sub_id));
                            out.push_str(tail);
                            out.push('\n');
                            continue;
                        }
                    }
                }
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // End of a function body: a sole `}` at column 0.
        if line == "}" {
            current_loc_id = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Inside a debug-info'd function: attach !dbg to any `call`
        // or `invoke` instruction so clang accepts the DI block.
        if let Some(loc) = current_loc_id {
            let trimmed = line.trim_start();
            let is_call = trimmed.starts_with("call ")
                || trimmed.starts_with("invoke ")
                // SSA-assignment form: `%v = call ...`.
                || (trimmed.starts_with('%')
                    && trimmed.contains("= call ")
                    && !trimmed.contains("!dbg"));
            if is_call && !line.contains("!dbg") {
                out.push_str(line);
                out.push_str(&format!(", !dbg !{loc}"));
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    // Emit the metadata block.
    out.push('\n');
    out.push_str("!llvm.module.flags = !{!0, !1}\n");
    out.push_str("!llvm.dbg.cu = !{!2}\n");
    out.push('\n');
    out.push_str("!0 = !{i32 2, !\"Dwarf Version\", i32 4}\n");
    out.push_str("!1 = !{i32 2, !\"Debug Info Version\", i32 3}\n");
    out.push_str(&format!(
        "!{cu_id} = distinct !DICompileUnit(language: DW_LANG_C99, file: !{file_id}, \
         producer: \"cpc\", isOptimized: false, runtimeVersion: 0, \
         emissionKind: FullDebug, splitDebugInlining: false)\n"
    ));
    out.push_str(&format!(
        "!{file_id} = !DIFile(filename: \"{}\", directory: \"{}\")\n",
        escape_dwarf_str(&filename),
        escape_dwarf_str(&directory)
    ));
    out.push_str(&format!(
        "!{sub_type_id} = !DISubroutineType(types: !{empty_types_id})\n"
    ));
    out.push_str(&format!("!{empty_types_id} = !{{null}}\n"));
    // Sort fn_meta entries by sub_id for stable output. Each entry
    // emits a DISubprogram immediately followed by its DILocation.
    let mut sorted: Vec<(String, u32, u32, u32)> = fn_meta
        .iter()
        .map(|(n, m)| (n.clone(), m.sub_id, m.loc_id, m.line))
        .collect();
    sorted.sort_by_key(|e| e.1);
    for (name, sub_id, loc_id, line) in sorted {
        out.push_str(&format!(
            "!{sub_id} = distinct !DISubprogram(name: \"{}\", linkageName: \"{}\", \
             scope: !{file_id}, file: !{file_id}, line: {line}, type: !{sub_type_id}, \
             scopeLine: {line}, spFlags: DISPFlagDefinition, unit: !{cu_id})\n",
            escape_dwarf_str(&name),
            escape_dwarf_str(&name),
        ));
        out.push_str(&format!(
            "!{loc_id} = !DILocation(line: {line}, column: 1, scope: !{sub_id})\n"
        ));
    }
}

fn line_for_span(
    span: crate::lexer::Span,
    line_map: Option<&crate::diagnostics::LineMap>,
    src: Option<&str>,
) -> u32 {
    match (line_map, src) {
        (Some(lm), Some(s)) => lm.position(span.start, s).line,
        _ => 1,
    }
}

fn escape_dwarf_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone)]
struct FnSig {
    /// Parameter info: type, `move_` flag, `mutable` flag.
    ///   - `move_` decides whether a Drop-bearing argument transfers ownership
    ///     across the call (caller flips its drop-flag, callee registers one).
    ///   - `mutable` paired with non-Copy struct type triggers the §2.9
    ///     exclusive-borrow ABI: callee receives a `ptr`, not a value copy,
    ///     so field writes propagate back to the caller (slice 5BC.codegen).
    ///   - `restrict` (4th flag) — v0.0.8 post-bench-gap: opt-in `noalias`
    ///     for raw-pointer (`*T`) params. Sema enforces `restrict` only
    ///     appears on `*T` types (E0411). codegen-side, the flag flips
    ///     the scalar-pointer attr from `noundef` to `noalias noundef`
    ///     at both def and call sites.
    params: Vec<(Ty, bool, bool, bool)>,
    return_type: Ty,
    /// Slice 10.FFI.4: variadic extern fn. Call sites for these emit
    /// `call ret_ty (fixed_types, ...) @name(args)` — the full
    /// function-type prefix is required by LLVM for varargs.
    is_variadic: bool,
    /// Phase 11 / ObjC interop: `#[link_name = "..."]` symbol alias.
    /// When `Some(s)`, codegen emits `declare ... @s(...)` and call
    /// sites use `@s` instead of `@<source_name>`. Only ever set on
    /// extern fns; sema rejects on non-extern.
    link_name: Option<String>,
    /// v0.0.12 G-027: `extern fn name(...)` import (no body). Distinguishes
    /// "this is a C-ABI import" from "this is an internal cpc fn". Drives
    /// sret application on the import-declaration and call-site paths so
    /// the AArch64-Darwin ABI for >16B struct returns matches what clang
    /// emits on the C side. `pub extern fn` definitions (which have
    /// bodies, exported with the C ABI) are also `true`; the call-site
    /// branch only cares for the import direction since exports define
    /// their own sret shape via the def-side path.
    is_extern: bool,
}

fn collect_sigs(p: &Program, types: &TypeTable) -> HashMap<String, FnSig> {
    let mut sigs = HashMap::new();
    // builtin: #println(i32) -> ()
    sigs.insert(
        "println".to_string(),
        FnSig {
            params: vec![(Ty::I32, false, false, false)],
            return_type: Ty::Unit,
            is_variadic: false,
            link_name: None,
            is_extern: false,
        },
    );
    for item in &p.items {
        let ItemKind::Function(f) = &item.kind else {
            continue;
        };
        // Slice 7GEN.4: generic fns are not emitted pre-monomorphization;
        // their signatures aren't part of the concrete call graph yet.
        if !f.generic_params.is_empty() {
            continue;
        }
        let params: Vec<(Ty, bool, bool, bool)> = f
            .params
            .iter()
            .map(|p| {
                let ty = ty_from(&p.ty, types);
                let mv = effective_move(p, &ty, types);
                (ty, mv, p.mutable, p.restrict)
            })
            .collect();
        let declared_ret = match &f.return_type {
            Some(t) => ty_from(t, types),
            None => Ty::Unit,
        };
        // v0.0.3 Phase 5 Slice 5E.3: `async fn foo() -> T` exposes
        // `Future[T]` at the call site. The body codegen path
        // (`gen_async_function`) uses the inner T directly; the sig
        // here drives call-site lowering, where the returned value
        // is the Future aggregate.
        let ret = if f.is_async {
            lookup_future_ty(&declared_ret, types)
        } else if f.is_gen {
            // v0.0.4 Phase 4 Slice 4A: gen fn exposes `Iterator[T]` at
            // the call site.
            lookup_iterator_ty(&declared_ret, types)
        } else {
            declared_ret
        };
        let link_name = f.attributes.iter().find_map(|a| {
            if a.path.name != "link_name" {
                return None;
            }
            match a.args.as_slice() {
                [AttrArg::Str(s, _)] => Some(s.clone()),
                _ => None,
            }
        });
        sigs.insert(
            f.name.name.clone(),
            FnSig {
                params,
                return_type: ret,
                is_variadic: f.is_variadic,
                link_name,
                is_extern: f.is_extern,
            },
        );
    }
    sigs
}

/// Codegen-side type registry. Mirrors sema's enum/struct numbering by walking
/// `program.items` in the same declaration order.
#[derive(Debug, Clone, Default)]
struct TypeTable {
    enum_by_name: HashMap<String, EnumId>,
    enum_defs: Vec<EnumInfo>,
    struct_by_name: HashMap<String, StructId>,
    struct_defs: Vec<StructInfo>,
}

#[derive(Debug, Clone)]
struct EnumInfo {
    /// Variant name → declaration-order index (the runtime tag value).
    variants: HashMap<String, u32>,
    /// Variants in declaration order, with payload type lists. Plain enums
    /// have all-empty payloads; tagged enums have at least one non-empty.
    variant_payloads: Vec<Vec<Ty>>,
    /// True iff at least one variant carries a payload. Codegen branches on
    /// this: plain enums stay bare `i32` (Phase-2A fast path); tagged enums
    /// use the `{ i32 tag, [N x i64] payload }` layout.
    is_tagged: bool,
    /// Number of 8-byte slots in the tagged-enum payload area. 0 for plain
    /// enums. For tagged enums, this is the max across variants of
    /// `payload.len()` — Phase 3 uses one 8-byte slot per payload value
    /// regardless of the value's actual size (simple, wastes some bytes;
    /// alignment is naturally 8 everywhere).
    payload_slots: u32,
    /// Mirror of sema's Copy fixpoint. Plain enums are always Copy; tagged
    /// enums are Copy iff every variant's payload type list is all-Copy.
    is_copy: bool,
    /// v0.0.5 Phase 2C: inherent methods declared via `impl EnumName`.
    /// Keyed by method name; same shape as `StructInfo::methods`.
    methods: HashMap<String, MethodInfo>,
}

#[derive(Debug, Clone)]
struct StructInfo {
    name: String,
    /// Fields in declaration order. The pair is (field name, field type).
    fields: Vec<(String, Ty)>,
    /// Methods declared in `impl` blocks for this struct.
    methods: HashMap<String, MethodInfo>,
    /// True iff this struct has a destructor — a method named `drop` with
    /// signature `fn drop(mut self)`. Sema validates the signature; codegen
    /// mirrors the flag to decide whether `let x: T = ...` registers a
    /// scope-exit drop call. See `docs/design/phase3-drop.md`.
    is_drop: bool,
    /// Mirror of sema's Copy fixpoint. A struct is Copy iff it has no Drop
    /// destructor and every field is Copy. Used by the §2.9 mutable-borrow
    /// ABI in `param_passes_by_ptr` — non-Copy `mut x: T` is pointer-passed
    /// so the callee's writes propagate back to the caller.
    is_copy: bool,
}

#[derive(Debug, Clone)]
struct MethodInfo {
    receiver: Option<Receiver>,
    /// Parameter info, excluding the receiver:
    /// `(ty, move_, mutable, restrict)`. `move_` drives call-site
    /// drop-flag flips; `mutable` drives the §2.9 pointer-pass ABI for
    /// non-Copy struct params (slice 5BC.codegen); `restrict` (v0.0.8
    /// post-bench-gap) opt-in `noalias` for raw-pointer params.
    params: Vec<(Ty, bool, bool, bool)>,
    return_type: Ty,
    /// v0.0.8 bench-gap fix D: if this method's body matches a known
    /// "trivial" pattern, the call-site emitter substitutes the
    /// pattern's IR directly instead of emitting a `call` instruction.
    /// LLVM's inliner would do this at `-O3` anyway — but inlining at
    /// cpc-emission time shrinks the IR clang has to optimize and lets
    /// the optimizer converge faster on the remaining work.
    trivial_inline: Option<TrivialInline>,
}

/// v0.0.8 bench-gap fix D: classify a method's body for cpc-side
/// inlining. Today: getter pattern only — `fn name(self) -> T { return
/// self.<field>; }`. Setter / pass-through / primitive-return patterns
/// can land in the same enum if they prove load-bearing on a future
/// benchmark.
#[derive(Debug, Clone)]
enum TrivialInline {
    /// `fn name(self) -> FieldTy { return self.<field>; }` — Read
    /// receiver, zero params, single-`return self.field` body. Inlines
    /// to `gep inbounds %S, ptr <recv>, i32 0, i32 <field_idx>` + load.
    GetField(String),
}

/// v0.0.8 fix D: detect the trivial-getter pattern. Returns
/// `Some(GetField(field_name))` iff the method's body is a single
/// `return self.<field>;` statement, the receiver is `self` (Read), the
/// param list is empty, and the method isn't gen / async / generic.
/// Drop methods are excluded by their `mut self` receiver. Returns
/// `None` for any other shape.
fn detect_trivial_inline(m: &Method) -> Option<TrivialInline> {
    if !matches!(m.receiver, Some(Receiver::Read)) {
        return None;
    }
    if !m.params.is_empty() || m.is_gen || m.is_async || !m.generic_params.is_empty() {
        return None;
    }
    // Body must be exactly one statement, with no tail expression.
    if m.body.stmts.len() != 1 || m.body.tail.is_some() {
        return None;
    }
    let StmtKind::Return(Some(ret_expr)) = &m.body.stmts[0].kind else {
        return None;
    };
    let ExprKind::Field { receiver, name } = &ret_expr.kind else {
        return None;
    };
    let ExprKind::Ident(recv_name) = &receiver.kind else {
        return None;
    };
    if recv_name != "self" {
        return None;
    }
    Some(TrivialInline::GetField(name.name.clone()))
}

impl StructInfo {
    fn field_index(&self, name: &str) -> u32 {
        self.fields
            .iter()
            .position(|(n, _)| n == name)
            .expect("sema validated") as u32
    }
    fn field_type(&self, name: &str) -> Ty {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
            .expect("sema validated")
    }
}

fn mangle(struct_name: &str, method_name: &str) -> String {
    format!("{}.{}", struct_name, method_name)
}

/// v0.0.5 Phase 3 Slice 3B: reconstruct a synthesized tuple struct's
/// mangled name from its element types. Must match sema's naming in
/// `synthesize_tuple_struct` (which uses `mangle_ty_for_name`).
fn tuple_struct_name(elem_tys: &[Ty], types: &TypeTable) -> String {
    let parts: Vec<String> = elem_tys
        .iter()
        .map(|t| tuple_elem_mangle(t, types))
        .collect();
    format!("__tuple_{}", parts.join("_"))
}

fn tuple_elem_mangle(ty: &Ty, types: &TypeTable) -> String {
    match ty {
        Ty::I8 => "i8".into(),
        Ty::I16 => "i16".into(),
        Ty::I32 => "i32".into(),
        Ty::I64 => "i64".into(),
        Ty::U8 => "u8".into(),
        Ty::U16 => "u16".into(),
        Ty::U32 => "u32".into(),
        Ty::U64 => "u64".into(),
        Ty::Isize => "isize".into(),
        Ty::Usize => "usize".into(),
        Ty::F16 => "f16".into(),
        Ty::F32 => "f32".into(),
        Ty::F64 => "f64".into(),
        Ty::Bool => "bool".into(),
        Ty::Unit => "unit".into(),
        Ty::Str => "str".into(),
        Ty::String => "string".into(),
        Ty::Slice(inner) => format!("slice_{}", tuple_elem_mangle(inner, types)),
        Ty::RawPtr(inner) => format!("ptr_{}", tuple_elem_mangle(inner, types)),
        Ty::FnPtr {
            params,
            return_type,
        } => {
            let mut s = String::from("fn");
            for p in params {
                s.push('_');
                s.push_str(&tuple_elem_mangle(p, types));
            }
            if !matches!(**return_type, Ty::Unit) {
                s.push_str("_ret_");
                s.push_str(&tuple_elem_mangle(return_type, types));
            }
            s
        }
        Ty::Struct(id) => types.struct_defs[id.0 as usize].name.clone(),
        Ty::Enum(id) => types
            .enum_by_name
            .iter()
            .find_map(|(n, eid)| if eid == id { Some(n.clone()) } else { None })
            .unwrap_or_else(|| "<enum>".to_string()),
        Ty::Array(elem, n) => format!("arr{}_{}", n, tuple_elem_mangle(elem, types)),
        Ty::Simd { elem, lanes } => format!("{}x{}", tuple_elem_mangle(elem, types), lanes),
        Ty::Mask { elem, lanes } => format!("mask{}x{}", tuple_elem_mangle(elem, types), lanes),
        Ty::Param(name) => format!("Param_{name}"),
        Ty::Error => "ERR".into(),
    }
}

fn collect_types(p: &Program) -> TypeTable {
    let mut t = TypeTable::default();
    // First pass: register names so struct field type resolution can refer
    // to other types declared anywhere in the program (forward refs).
    for item in &p.items {
        match &item.kind {
            ItemKind::Enum(e) => {
                if t.enum_by_name.contains_key(&e.name.name)
                    || t.struct_by_name.contains_key(&e.name.name)
                {
                    continue;
                }
                // Slice 7GEN.4: generic enum templates are not emitted
                // pre-monomorphization. Slice 7GEN.5 will register a
                // per-instantiation type entry as the work-queue drains.
                if !e.generic_params.is_empty() {
                    continue;
                }
                let id = EnumId(t.enum_defs.len() as u32);
                let mut variants = HashMap::new();
                let mut empty_payloads: Vec<Vec<Ty>> = Vec::new();
                for (idx, v) in e.variants.iter().enumerate() {
                    variants.entry(v.name.name.clone()).or_insert(idx as u32);
                    empty_payloads.push(Vec::new()); // resolved in pass 2 below
                }
                let is_tagged = e.variants.iter().any(|v| !v.payload.is_empty());
                t.enum_defs.push(EnumInfo {
                    variants,
                    variant_payloads: empty_payloads,
                    is_tagged,
                    payload_slots: 0, // computed in pass 2 below
                    // Plain enums are Copy unconditionally; tagged enums are
                    // resolved by the fixpoint in `compute_copy_flags`.
                    is_copy: !is_tagged,
                    methods: HashMap::new(),
                });
                t.enum_by_name.insert(e.name.name.clone(), id);
            }
            ItemKind::Struct(s) => {
                if t.enum_by_name.contains_key(&s.name.name)
                    || t.struct_by_name.contains_key(&s.name.name)
                {
                    continue;
                }
                // Slice 7GEN.4: generic struct templates are not emitted
                // pre-monomorphization. Slice 7GEN.5 lands the work-queue.
                if !s.generic_params.is_empty() {
                    continue;
                }
                let id = StructId(t.struct_defs.len() as u32);
                t.struct_defs.push(StructInfo {
                    name: s.name.name.clone(),
                    fields: Vec::new(),
                    methods: HashMap::new(),
                    is_drop: false,
                    is_copy: false, // computed in `compute_copy_flags`
                });
                t.struct_by_name.insert(s.name.name.clone(), id);
            }
            ItemKind::Function(_)
            | ItemKind::Impl(_)
            | ItemKind::Interface(_)
            | ItemKind::TypeAlias(_)
            | ItemKind::Const(_)
            | ItemKind::Static(_)
            | ItemKind::ModuleAsm(_) => {}
        }
    }
    // Second pass: resolve struct field types.
    for item in &p.items {
        let ItemKind::Struct(s) = &item.kind else {
            continue;
        };
        if !s.generic_params.is_empty() {
            continue;
        }
        let Some(&id) = t.struct_by_name.get(&s.name.name) else {
            continue;
        };
        let mut fields: Vec<(String, Ty)> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        for f in &s.fields {
            if seen.contains_key(&f.name.name) {
                continue;
            }
            seen.insert(f.name.name.clone(), ());
            let ty = ty_from(&f.ty, &t);
            fields.push((f.name.name.clone(), ty));
        }
        t.struct_defs[id.0 as usize].fields = fields;
    }
    // Second-and-a-half pass: resolve enum variant payload types now that
    // every struct and enum name is registered. Also compute payload_slots
    // for tagged enums.
    //
    // **v0.0.3 drop-tracking fix**: payload_slots used to be a COUNT of
    // payload types (1 slot per type, 8 bytes per slot). That broke for
    // any variant carrying an aggregate >8 bytes — e.g. `Result[Vec[u8], E]`
    // allocated only 1×i64 for the Ok payload, but `Vec[u8]` is 24 bytes.
    // Storing the Vec into the enum stomped past its allocation; loading
    // it back truncated to 8 bytes. We now compute slots from actual
    // payload byte size, rounded up to i64 alignment.
    for item in &p.items {
        let ItemKind::Enum(e) = &item.kind else {
            continue;
        };
        if !e.generic_params.is_empty() {
            continue;
        }
        let Some(&id) = t.enum_by_name.get(&e.name.name) else {
            continue;
        };
        let mut max_slots: u32 = 0;
        let mut payloads: Vec<Vec<Ty>> = Vec::with_capacity(e.variants.len());
        for v in &e.variants {
            let p: Vec<Ty> = v.payload.iter().map(|ty| ty_from(ty, &t)).collect();
            // Sum payload bytes; round up to i64-aligned slot count.
            let mut bytes: u64 = 0;
            for ty in &p {
                if let Some((sz, _al)) = static_layout(ty, &t) {
                    // Pad each value up to 8 bytes (i64-aligned slots).
                    bytes += (sz + 7) & !7;
                }
            }
            let slots = ((bytes + 7) / 8) as u32;
            max_slots = max_slots.max(slots);
            payloads.push(p);
        }
        t.enum_defs[id.0 as usize].variant_payloads = payloads;
        t.enum_defs[id.0 as usize].payload_slots = max_slots;
    }
    // Third pass: collect methods from impl blocks.
    for item in &p.items {
        let ItemKind::Impl(b) = &item.kind else {
            continue;
        };
        // v0.0.5 Phase 2C: route enum impls to enum_defs's method table.
        if let Some(&enum_id) = t.enum_by_name.get(&b.target.name) {
            for m in &b.methods {
                if t.enum_defs[enum_id.0 as usize]
                    .methods
                    .contains_key(&m.name.name)
                {
                    continue;
                }
                if !m.generic_params.is_empty() {
                    continue;
                }
                let params: Vec<(Ty, bool, bool, bool)> = m
                    .params
                    .iter()
                    .map(|p| {
                        let ty = ty_from(&p.ty, &t);
                        let mv = effective_move(p, &ty, &t);
                        (ty, mv, p.mutable, p.restrict)
                    })
                    .collect();
                let declared_ret = match &m.return_type {
                    Some(ty) => ty_from(ty, &t),
                    None => Ty::Unit,
                };
                let return_type = if m.is_gen {
                    lookup_iterator_ty(&declared_ret, &t)
                } else if m.is_async {
                    lookup_future_ty(&declared_ret, &t)
                } else {
                    declared_ret
                };
                let trivial_inline = detect_trivial_inline(m);
                t.enum_defs[enum_id.0 as usize].methods.insert(
                    m.name.name.clone(),
                    MethodInfo {
                        receiver: m.receiver,
                        params,
                        return_type,
                        trivial_inline,
                    },
                );
            }
            continue;
        }
        let Some(&id) = t.struct_by_name.get(&b.target.name) else {
            continue;
        };
        for m in &b.methods {
            if t.struct_defs[id.0 as usize]
                .methods
                .contains_key(&m.name.name)
            {
                continue;
            }
            // Slice 7GEN.5e: skip generic method templates in codegen
            // type collection. Monomorphized concrete copies will be
            // emitted via the monomorphize pass.
            if !m.generic_params.is_empty() {
                continue;
            }
            // v0.0.15: apply the move-by-default rule (`effective_move`) to
            // struct-method params, matching free functions (collect_sigs) and
            // enum methods above. Previously this site used the raw `p.move_`
            // flag, so a bare non-Copy aggregate param of a *struct method*
            // (notably `Vec[T]::push(x: T)`) was treated as a borrow: the caller
            // never `mark_moved` it, so a heap-owning argument was double-freed
            // (the vendor/json `elems.push(v)` use-after-free).
            let params: Vec<(Ty, bool, bool, bool)> = m
                .params
                .iter()
                .map(|p| {
                    let ty = ty_from(&p.ty, &t);
                    let mv = effective_move(p, &ty, &t);
                    (ty, mv, p.mutable, p.restrict)
                })
                .collect();
            let declared_ret = match &m.return_type {
                Some(ty) => ty_from(ty, &t),
                None => Ty::Unit,
            };
            // v0.0.5 Phase 2B: gen-method return wraps T → Iterator[T] at
            // the call site (mirror of `gen fn` free fn collect_sigs).
            // v0.0.5 Phase 4 Slice 4B: same for async methods → Future[T].
            let return_type = if m.is_gen {
                lookup_iterator_ty(&declared_ret, &t)
            } else if m.is_async {
                lookup_future_ty(&declared_ret, &t)
            } else {
                declared_ret
            };
            let trivial_inline = detect_trivial_inline(m);
            t.struct_defs[id.0 as usize].methods.insert(
                m.name.name.clone(),
                MethodInfo {
                    receiver: m.receiver,
                    params,
                    return_type,
                    trivial_inline,
                },
            );
            // Mirror sema's Drop detection so codegen knows which bindings
            // need scope-exit drop emission. Sema has already validated the
            // signature; we trust it here.
            if m.name.name == "drop" {
                t.struct_defs[id.0 as usize].is_drop = true;
            }
        }
    }
    // Fourth pass: fixpoint Copy resolution across structs and tagged enums.
    // Mirrors sema's `compute_struct_copy_flags` and `compute_enum_copy_flags`
    // — the answer must be identical so the borrow-checker's classification
    // (sema's `is_copy`) matches the ABI choice codegen makes here.
    compute_copy_flags(&mut t);
    t
}

/// Mirror of sema's Copy fixpoint, on codegen's own `TypeTable`.
/// A struct is Copy iff it has no `drop` destructor and every field is Copy.
/// A tagged enum is Copy iff every variant payload type is Copy.
/// Plain enums are pre-marked Copy at construction; primitives and arrays of
/// Copy elements are handled directly by `is_copy_ty`.
fn compute_copy_flags(t: &mut TypeTable) {
    loop {
        let mut changed = false;
        for i in 0..t.struct_defs.len() {
            if t.struct_defs[i].is_copy || t.struct_defs[i].is_drop {
                continue;
            }
            let all_fields_copy = t.struct_defs[i]
                .fields
                .iter()
                .all(|(_, ty)| is_copy_ty(ty, t));
            if all_fields_copy {
                t.struct_defs[i].is_copy = true;
                changed = true;
            }
        }
        for i in 0..t.enum_defs.len() {
            if t.enum_defs[i].is_copy {
                continue;
            }
            // Only tagged enums reach here (plain enums were pre-marked).
            let all_payloads_copy = t.enum_defs[i]
                .variant_payloads
                .iter()
                .all(|p| p.iter().all(|ty| is_copy_ty(ty, t)));
            if all_payloads_copy {
                t.enum_defs[i].is_copy = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// True iff `ty` is Copy under the current `TypeTable`. Primitives and unit
/// are Copy; arrays inherit element copy-ness; structs and enums consult the
/// pre-computed flags. Sema is the source of truth — this mirror exists so
/// codegen can answer the question without re-importing sema's state.
fn is_copy_ty(ty: &Ty, t: &TypeTable) -> bool {
    match ty {
        Ty::Unit
        | Ty::Bool
        | Ty::I8
        | Ty::I16
        | Ty::I32
        | Ty::I64
        | Ty::U8
        | Ty::U16
        | Ty::U32
        | Ty::U64
        | Ty::Usize
        | Ty::Isize
        | Ty::F16
        | Ty::F32
        | Ty::F64
        | Ty::Str
        | Ty::Slice(_)
        | Ty::RawPtr(_)
        | Ty::FnPtr { .. } => true,
        Ty::Array(elem, _) => is_copy_ty(elem, t),
        Ty::Struct(id) => t.struct_defs[id.0 as usize].is_copy,
        Ty::Enum(id) => t.enum_defs[id.0 as usize].is_copy,
        // v0.0.6 Slice 1B: SIMD types are Copy (lane-scalars are all
        // Copy primitives; the whole vector is a register-sized value).
        // Masks share the SIMD lowering and Copy semantics.
        Ty::Simd { .. } | Ty::Mask { .. } => true,
        // Phase 8 slice 8.STR.3: owned `string` is non-Copy + Drop.
        Ty::String => false,
        Ty::Error => false,
        // Slice 7GEN.4: generic type parameters never reach codegen
        // pre-monomorphization (slice 7GEN.5). Treat as non-Copy to
        // keep the helper total.
        Ty::Param(_) => false,
    }
}

/// §2.9 borrow-ABI choice for a parameter. Returns true when the LLVM signature
/// should use `ptr` for this parameter and the callee binds it directly (no
/// alloca, no initial store, no drop registration), so that the callee's
/// writes propagate back to the caller's place (for `mut`) or so the caller
/// avoids an aggregate byte-copy (for shared).
///
/// Fires on:
/// - `mut x: T` where T is a non-Copy struct (slice 5BC.codegen) — exclusive
///   borrow ABI; writes propagate back, paired with LLVM `noalias` in 6BC.codegen.
/// - `borrow x: T` where T is a non-Copy struct — shared borrow pointer-pass
///   paired with LLVM `readonly`. Eliminates the byte-copy at call sites of
///   large non-Copy aggregates while leaving ownership (and the drop) with the
///   caller.
///
/// Note: a *bare* `x: T` on a non-Copy struct no longer reaches the shared-borrow
/// branch — `effective_move` (below) rewrites it to `move_`, so it is value-passed
/// like an explicit `move x: T`. `move x: T` stays value-passed (the value is the
/// transfer; the caller's drop flag flip suppresses the caller-side drop).
fn param_passes_by_ptr(ty: &Ty, move_: bool, _mutable: bool, t: &TypeTable) -> bool {
    if move_ {
        return false;
    }
    matches!(ty, Ty::Struct(_)) && !is_copy_ty(ty, t)
}

/// v0.0.12: the v0.0.10 "non-Copy moves by default" rule, wired through to
/// codegen.
///
/// The borrow checker already treats a bare `x: T` on a non-Copy struct as a
/// move (use-after-move is E0335), but codegen historically lowered it like a
/// shared borrow: pointer-pass with `readonly`, the callee binding aliasing the
/// caller's storage, and the caller keeping the drop. That mismatch
/// double-freed whenever the callee duplicated the value back out —
/// `fn f(x: T) -> T { return x; }` then `let c = f(b);` dropped the one heap
/// allocation twice (caller's unconditional drop of `b` + new owner `c`'s drop).
///
/// Collapsing the bare case onto the existing, sound `move x: T` lowering fixes
/// it: value-pass, caller flips the arg's drop flag (`mark_moved`), and the
/// callee `register_drop`s the param so it is freed exactly once — by the callee
/// at scope exit, or by whoever it is forwarded to. `mut x` (exclusive borrow)
/// and `borrow x` (shared borrow) do not consume, so they keep their
/// by-pointer borrow ABI.
///
/// Covers `Ty::Struct` and `Ty::Enum` (v0.0.15): both are non-Copy aggregates
/// whose value-pass is a bitwise copy aliasing the caller's heap, so both need
/// the move lowering. Excluding `Ty::Enum` was the v0.0.14 vendor/json
/// double-free: a heap-owning enum (`Value::Array(Vec[Value])`) passed by
/// bare-ident (`elems.push(v)`, `Result::Ok(v)`) was borrow-copied without
/// `mark_moved`, so the caller's scope-exit drop freed heap the callee had
/// already stored — a use-after-free on the next read. The callee `register_drop`
/// path already handles `Ty::Enum` move params, so this routes enums through the
/// identical, sound machinery as structs. `Ty::String` / `Vec[T]` value params
/// still dodge the double-free via the auto-clone-on-return safety net
/// (`borrowed_params`), so they are left untouched here.
fn effective_move(p: &Param, ty: &Ty, t: &TypeTable) -> bool {
    if p.move_ {
        return true;
    }
    if p.mutable || p.borrow_ {
        return false;
    }
    matches!(ty, Ty::Struct(_) | Ty::Enum(_)) && !is_copy_ty(ty, t)
}

/// Slice 1A: full parameter attribute set (v0.0.2 LLVM information dividend).
///
/// Supersedes the original `param_attr_prefix` (which returned just
/// `noalias`/`readonly`). Composes every attribute the frontend has already
/// proven sound:
/// - **Pointer-passed (non-Copy struct, by §2.9 borrow ABI):**
///   - `noalias` (`mut`/`move`) or `readonly` (shared `self`/`x: T`) — the
///     borrow checker proves disjointness for the first, write-freeness for
///     the second.
///   - `nonnull` — C+ has no null in safe code (cross-ref
///     [feedback_cplus_no_null.md]); the address always comes from an
///     `alloca` or a previously-typed place.
///   - `noundef` — definite-assignment (sema slice 3J) guarantees the bytes
///     reachable through the pointer are fully defined.
///   - `dereferenceable(N)` + `align A` — `(N, A)` come from `static_layout`,
///     keyed on the slice-11.LAYOUT type table. Exact, not lower bounds.
/// - **Value-passed scalar (integers, bool, floats, raw `*T`, fn-pointer,
///   plain enum `iN'`):** `noundef` alone. Definite assignment justifies it
///   and LLVM's `-O2` uses `noundef` to fold redundant freeze/select
///   patterns.
/// - **Value-passed aggregate (`str`, `string`, `T[]`, Copy struct, tagged
///   enum):** no attributes. Padding bytes are LLVM-`poison` after
///   `insertvalue` construction, so `noundef` would be unsound at the
///   aggregate level.
///
/// Returned string has no trailing space; callers append a separator before
/// the SSA name (e.g. `"ptr {attrs} %{i}"`).
fn param_attrs(
    ty: &Ty,
    move_: bool,
    mutable: bool,
    restrict: bool,
    pointer_passed: bool,
    types: &TypeTable,
) -> String {
    if pointer_passed {
        let mut s = String::new();
        s.push_str(if move_ || mutable {
            "noalias"
        } else {
            "readonly"
        });
        s.push_str(" nonnull noundef");
        if let Some((sz, al)) = static_layout(ty, types) {
            if sz > 0 {
                let _ = write!(s, " dereferenceable({sz})");
            }
            let _ = write!(s, " align {al}");
        }
        s
    } else if is_scalar_ty(ty, types) {
        // v0.0.8 post-bench-gap: `restrict x: *T` on a raw-pointer
        // (scalar-passed) param promotes the attr set from bare
        // `noundef` to `noalias noundef`. Borrow-checked struct ptr
        // params already pick up `noalias` via the pointer_passed
        // branch above. Sema (E0411) gates `restrict` to `*T` shapes,
        // so the attribute is only emitted when it's sound.
        if restrict && matches!(ty, Ty::RawPtr(_)) {
            "noalias noundef".to_string()
        } else {
            "noundef".to_string()
        }
    } else {
        String::new()
    }
}

/// Slice 1D (v0.0.2): decide whether the return value should use the LLVM
/// `sret` calling convention instead of a value-returned aggregate.
///
/// The plan describes `sret` for "non-Copy structs, slices, owned strings,
/// or any aggregate exceeding a target-specific size threshold (start with
/// > 16 bytes)". This implementation ships the **narrow** version: only
/// owned `string` (24 bytes, has Drop, the canonical case where copy
/// elision matters most). Generic non-Copy struct sret is deferred — it
/// has substantial test-surface impact and the wins for small aggregates
/// (≤ 16 bytes) are negligible at -O2 because LLVM already lowers them
/// through ABI-appropriate registers.
///
/// `extern fn` boundaries are never sret-modified — those keep the C ABI
/// the user declared. The callers of this predicate check `is_extern`
/// before calling.
/// Phase 5 Slice 5.D: classify a `pub extern fn` parameter or return type
/// against the platform C ABI. Today we target aarch64-apple-darwin —
/// the AArch64 Procedure Call Standard — and treat all aggregates as
/// integer-class (no HFA detection; the plan defers float-class to v2).
///
/// The rule is:
/// - Scalar (primitive, raw `*T`, fn-ptr, plain enum): pass unchanged.
/// - Aggregate ≤ 8 bytes: coerce to `i64`. Caller packs into a single GPR;
///   callee re-interprets the bits via an alloca'd buffer.
/// - Aggregate 9..=16 bytes: coerce to `[2 x i64]`. Two GPRs.
/// - Aggregate > 16 bytes: pass indirectly via a pointer. Return via
///   `sret(<ty>)` to a caller-allocated slot.
///
/// `Indirect` returns are handled in tandem with Slice 1D's `sret` path —
/// the function signature drops the value return and gains a `ptr sret(...)`
/// first parameter. Indirect *args* are bare `ptr` (no `byval` on
/// aarch64-darwin; the caller-callee contract owns the memory layout).
#[derive(Debug, Clone, PartialEq)]
enum CAbiClass {
    /// Pass as-is; no coercion needed.
    Direct,
    /// Coerce to the given LLVM type. The coerced size is ≥ the original
    /// size and is required for the alloca's storage so the coerced
    /// store doesn't overflow into adjacent memory.
    Coerce {
        llvm_ty: String,
        size: u64,
        align: u64,
    },
    /// Pass indirectly via a hidden pointer (param side) / `sret` (return).
    Indirect,
}

/// AArch64 AAPCS64 §6.8.2: a Homogeneous Floating-point Aggregate (HFA) — an
/// aggregate whose fundamental members are all the *same* floating-point type
/// (`f32` or `f64`), four or fewer — is passed/returned in consecutive FP
/// registers (s0–s3 / d0–d3), NOT coerced to integer class or passed indirectly.
/// `NSPoint`/`NSSize` (2×f64) and `NSRect`/`NSEdgeInsets` (4×f64) are HFAs;
/// classifying them by raw byte size sends every geometry value to integer/
/// memory instead of the FP registers the callee reads (garbage coordinates).
/// Returns `Some((elem_llvm_ty, member_count))` for an HFA, else `None`.
fn hfa_members(ty: &Ty, types: &TypeTable) -> Option<(&'static str, u64)> {
    fn walk(ty: &Ty, types: &TypeTable, elem: &mut Option<&'static str>, count: &mut u64) -> bool {
        let this: &'static str = match ty {
            Ty::F32 => "float",
            Ty::F64 => "double",
            Ty::Struct(id) => {
                let info = &types.struct_defs[id.0 as usize];
                if info.fields.is_empty() {
                    return false;
                }
                for (_, fty) in &info.fields {
                    if !walk(fty, types, elem, count) {
                        return false;
                    }
                }
                return true;
            }
            Ty::Array(inner, n) => {
                for _ in 0..*n {
                    if !walk(inner, types, elem, count) {
                        return false;
                    }
                }
                return true;
            }
            _ => return false,
        };
        match *elem {
            None => *elem = Some(this),
            Some(prev) => {
                if prev != this {
                    return false;
                }
            }
        }
        *count += 1;
        *count <= 4
    }
    let mut elem: Option<&'static str> = None;
    let mut count: u64 = 0;
    if walk(ty, types, &mut elem, &mut count) && count >= 1 {
        return Some((elem.unwrap(), count));
    }
    None
}

fn classify_c_abi(ty: &Ty, types: &TypeTable) -> CAbiClass {
    // Aggregates only need ABI coercion. Everything else is a single
    // register class and passes through cleanly.
    let is_aggregate = match ty {
        // Plain enums lower to i32 (scalar). Tagged enums are aggregates —
        // but sema's 5.C predicate rejects them at the `pub extern fn`
        // boundary, so we never see one here. Defensively handle anyway:
        // a tagged enum reaching codegen for an extern fn would still
        // need coercion (and a future spec for the layout); treat as
        // aggregate-by-size.
        Ty::Enum(id) => types.enum_defs[id.0 as usize].is_tagged,
        Ty::Struct(_) | Ty::Array(_, _) | Ty::Str | Ty::String | Ty::Slice(_) => true,
        _ => false,
    };
    if !is_aggregate {
        return CAbiClass::Direct;
    }
    let Some((size, _align)) = static_layout(ty, types) else {
        return CAbiClass::Direct;
    };
    if size == 0 {
        return CAbiClass::Direct;
    }
    // AArch64 AAPCS64: HFAs (≤4 same-type floats/doubles) pass in FP registers
    // (d0–d3 / s0–s3), not integer-class or indirect. Coerce to `[N x <fp>]`,
    // which the AArch64 backend assigns to FP regs — without this, NSPoint/NSSize
    // (2×f64) coerce to `[2 x i64]` and NSRect (4×f64) goes indirect, landing
    // every geometry argument in the wrong registers (garbage coordinates, plus
    // a crash when a following pointer arg reads the spilled struct pointer).
    // Gated to aarch64 — the target AppKit's float-struct args run on; other
    // targets keep the size-based classification their tests exercise.
    if cfg!(target_arch = "aarch64") {
        if let Some((elem, n)) = hfa_members(ty, types) {
            return CAbiClass::Coerce {
                llvm_ty: format!("[{n} x {elem}]"),
                size,
                align: _align,
            };
        }
    }
    // Microsoft x64 (windows-msvc) ABI: an aggregate is passed/returned in a
    // single register only when its size is exactly 1, 2, 4, or 8 bytes;
    // every other size (3/5/6/7 and anything > 8) is passed indirectly via a
    // pointer to a caller-allocated copy. This is unlike x86_64-SysV (which
    // packs up to 16 bytes into two integer registers, `{ i64, i64 }`) and
    // AArch64-Darwin (`[2 x i64]`). Getting this wrong makes a 16-byte struct
    // arrive in two registers where the callee expects a pointer — an
    // immediate access violation on the first field read.
    if cfg!(all(target_arch = "x86_64", windows)) {
        let coerce = |ty: &str, n: u64| CAbiClass::Coerce {
            llvm_ty: ty.to_string(),
            size: n,
            align: n,
        };
        return match size {
            1 => coerce("i8", 1),
            2 => coerce("i16", 2),
            4 => coerce("i32", 4),
            8 => coerce("i64", 8),
            _ => CAbiClass::Indirect,
        };
    }
    // v0.0.3 Slice 3F: pick the per-platform coercion shape for 9..16-byte
    // aggregates. aarch64-darwin's AAPCS uses `[2 x i64]` (HFA-aware but
    // we treat all as integer-class). x86_64-sysv uses `{i64, i64}` —
    // distinct LLVM type at the IR level so clang's backend assigns each
    // member to its own register. >16 bytes go indirect on both.
    if size <= 8 {
        CAbiClass::Coerce {
            llvm_ty: "i64".to_string(),
            size: 8,
            align: 8,
        }
    } else if size <= 16 {
        let llvm_ty = if cfg!(target_arch = "x86_64") {
            "{ i64, i64 }".to_string()
        } else {
            "[2 x i64]".to_string()
        };
        CAbiClass::Coerce {
            llvm_ty,
            size: 16,
            align: 8,
        }
    } else {
        CAbiClass::Indirect
    }
}

fn return_passes_by_sret(ty: &Ty) -> bool {
    matches!(ty, Ty::String)
}

/// Like `return_passes_by_sret` but with access to struct flags so we can
/// widen to non-Copy structs without breaking C-ABI exports (which need
/// the byval/register coercion path, not sret) or small Copy POD returns.
///
/// **v0.0.3 Slice 1P** widens beyond v0.0.2's `Ty::String`-only path.
/// Without this, returning a `Vec[T]` (or any user-defined non-Copy
/// struct) across a module boundary triggers a drop-after-move: the
/// LLVM "first-class aggregate" return path copies the struct verbatim,
/// leaving the source's drop pointing at the same heap allocation as
/// the destination's. The sret path constructs the value in the caller's
/// slot directly, sidestepping the copy.
fn return_passes_by_sret_widened(ty: &Ty, types: &TypeTable) -> bool {
    if return_passes_by_sret(ty) {
        return true;
    }
    if let Ty::Struct(id) = ty {
        let def = &types.struct_defs[id.0 as usize];
        if !def.is_copy {
            return true;
        }
    }
    if let Ty::Enum(id) = ty {
        let def = &types.enum_defs[id.0 as usize];
        if !def.is_copy {
            return true;
        }
    }
    false
}

/// Compute static (size, align) in bytes. Matches LLVM's default natural
/// layout for the 64-bit targets C+ supports (x86_64, arm64).
///
/// Returns `None` only for non-codegen types (`Ty::Error`, `Ty::Param`) —
/// those should never reach codegen anyway. Callers can `.unwrap_or` the
/// dereferenceable/align attrs away when the layout is unknown.
/// Byte offset of payload value `pi` within a tagged-enum variant's payload
/// area `[N x i64]`: the sum of the i64-padded sizes of the values before it
/// (value 0 is at offset 0). Replaces the old slot-index GEP (`i64 pi`), which
/// assumed one 8-byte slot per value and corrupted layout when an earlier value
/// exceeded 8 bytes (a `string`/struct/enum payload before another). For
/// all-≤8-byte payloads the offset equals `pi * 8`, identical to the old GEP.
fn enum_payload_byte_offset(payload_tys: &[Ty], pi: usize, types: &TypeTable) -> u64 {
    let mut off: u64 = 0;
    for ty in payload_tys.iter().take(pi) {
        if let Some((sz, _al)) = static_layout(ty, types) {
            off += (sz + 7) & !7;
        }
    }
    off
}

fn static_layout(ty: &Ty, types: &TypeTable) -> Option<(u64, u64)> {
    fn align_up(off: u64, al: u64) -> u64 {
        (off + al - 1) & !(al - 1)
    }
    match ty {
        Ty::I8 | Ty::U8 | Ty::Bool => Some((1, 1)),
        Ty::I16 | Ty::U16 | Ty::F16 => Some((2, 2)),
        Ty::I32 | Ty::U32 | Ty::F32 => Some((4, 4)),
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::F64 => Some((8, 8)),
        Ty::RawPtr(_) | Ty::FnPtr { .. } => Some((8, 8)),
        // Fat pointers (Phase 8 / 11): { ptr, i64 } and { ptr, i64, i64 }.
        Ty::Str | Ty::Slice(_) => Some((16, 8)),
        Ty::String => Some((24, 8)),
        Ty::Unit => Some((0, 1)),
        Ty::Array(elem, n) => {
            let (esz, ea) = static_layout(elem, types)?;
            // No trailing pad on arrays — LLVM lays out [N x T] as N * size(T).
            Some((esz.saturating_mul(*n as u64), ea))
        }
        Ty::Struct(id) => {
            let info = &types.struct_defs[id.0 as usize];
            let mut off: u64 = 0;
            let mut max_al: u64 = 1;
            for (_, fty) in &info.fields {
                let (sz, al) = static_layout(fty, types)?;
                if al > max_al {
                    max_al = al;
                }
                off = align_up(off, al);
                off = off.saturating_add(sz);
            }
            // Pad to struct alignment.
            let total = if max_al == 0 {
                off
            } else {
                align_up(off, max_al)
            };
            Some((total, max_al.max(1)))
        }
        Ty::Enum(id) => {
            let info = &types.enum_defs[id.0 as usize];
            if !info.is_tagged {
                // Plain enum: bare i32.
                Some((4, 4))
            } else {
                // Tagged enum: { i32 tag, [N x i64] payload } — align 8.
                let payload_bytes = info.payload_slots as u64 * 8;
                // tag is i32, padded up to 8 before the array.
                let size = 8u64.saturating_add(payload_bytes);
                Some((size, 8))
            }
        }
        // v0.0.6 Slice 1B: fixed-width SIMD vector — `lanes` * size(elem),
        // aligned to the natural vector alignment (the total size, capped
        // by the lane scalar's alignment for short widths and rounded up
        // to a power of 2 otherwise). LLVM lays `<N x T>` exactly this way.
        Ty::Simd { elem, lanes } => {
            let (esz, ea) = static_layout(elem, types)?;
            let size = esz.saturating_mul(*lanes as u64);
            // Natural alignment for power-of-two-sized vectors equals the
            // size itself (4/8/16/32-byte alignment); otherwise the lane
            // alignment.
            let align = if size.is_power_of_two() { size } else { ea };
            Some((size, align))
        }
        // Masks lower to the same `<N x iN>` LLVM type as the matching
        // Simd — share the layout calculation verbatim.
        Ty::Mask { elem, lanes } => {
            let (esz, ea) = static_layout(elem, types)?;
            let size = esz.saturating_mul(*lanes as u64);
            let align = if size.is_power_of_two() { size } else { ea };
            Some((size, align))
        }
        Ty::Error | Ty::Param(_) => None,
    }
}

/// Slice 1C: scoped `!alias.scope` / `!noalias` metadata publication.
///
/// The borrow checker proves that for every pointer-passed `mut`/`move`
/// param (the ones that carried `noalias` from Slice 1A), no other live
/// pointer in the same function reaches the same memory. That fact is
/// already encoded as the `noalias` parameter attribute — but parameter
/// attrs degrade after inlining. Scoped alias metadata survives inlining
/// because the inliner imports the callee's scopes into the caller's
/// metadata universe.
///
/// This function does a single linear pass over the emitted function body
/// running a mini-dataflow:
///   1. Seed: each scoped param's SSA name (`%0`, `%1`, ...) → scope id.
///   2. On `getelementptr ..., ptr %src, ...` → propagate %src's scope
///      to the GEP's result.
///   3. On `load .., ptr %src` / `store .., ptr %src` → if %src has a
///      scope, append `, !alias.scope !L, !noalias !O` to the line.
///
/// Returns the rewritten body. `scope_idx_for_ssa` maps the param SSA name
/// to an index into `this_lists`/`other_lists`; both lists are indexed by
/// scope index so callers don't have to re-thread which scope is which.
fn annotate_alias_scope_metadata(
    body: &str,
    seed: &HashMap<String, usize>,
    this_lists: &[u32],
    other_lists: &[u32],
) -> String {
    let mut scope_map: HashMap<String, usize> = seed.clone();
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        out.push_str(&annotate_one_line(
            line,
            &mut scope_map,
            this_lists,
            other_lists,
        ));
        out.push('\n');
    }
    out
}

fn annotate_one_line(
    line: &str,
    scope_map: &mut HashMap<String, usize>,
    this_lists: &[u32],
    other_lists: &[u32],
) -> String {
    // Split on " = " to find an SSA def (load / GEP / etc.). Stores have
    // no LHS — handle separately below.
    let trimmed = line.trim_start();
    if let Some(eq_idx) = trimmed.find(" = ") {
        let lhs = &trimmed[..eq_idx];
        let rhs = &trimmed[eq_idx + 3..];
        if lhs.starts_with('%') {
            if rhs.starts_with("getelementptr ") {
                // GEP: find `, ptr %src` and propagate that scope to lhs.
                if let Some(src) = extract_ptr_operand(rhs) {
                    if let Some(&s) = scope_map.get(&src) {
                        scope_map.insert(lhs.to_string(), s);
                    }
                }
                return line.to_string();
            }
            if rhs.starts_with("load ") {
                if let Some(src) = extract_ptr_operand(rhs) {
                    if let Some(&s) = scope_map.get(&src) {
                        return format!(
                            "{line}, !alias.scope !{}, !noalias !{}",
                            this_lists[s], other_lists[s]
                        );
                    }
                }
                return line.to_string();
            }
            // bitcast / ptrtoint / inttoptr / select / phi etc. — could
            // propagate but the current language rarely generates these
            // for our scope-source ptrs. Leave conservative.
        }
    } else if trimmed.starts_with("store ") {
        if let Some(src) = extract_ptr_operand(trimmed) {
            if let Some(&s) = scope_map.get(&src) {
                return format!(
                    "{line}, !alias.scope !{}, !noalias !{}",
                    this_lists[s], other_lists[s]
                );
            }
        }
    }
    line.to_string()
}

/// Find the first `, ptr %X` operand in `s` and return `"%X"`. Used by
/// `annotate_one_line` to locate the address operand of load/store/GEP.
fn extract_ptr_operand(s: &str) -> Option<String> {
    let key = ", ptr ";
    let idx = s.find(key)?;
    let rest = &s[idx + key.len()..];
    if !rest.starts_with('%') {
        return None;
    }
    let end = rest
        .find(|c: char| c == ',' || c == ')' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// True iff `ty` lowers to a single LLVM scalar (one register class).
/// Aggregates (`str`, `string`, `T[]`, structs, tagged enums) are not
/// scalars even when small. Used to decide whether `noundef` is sound on a
/// value-passed parameter — aggregates carry `poison` padding so the
/// whole-value `noundef` would be unsound.
fn is_scalar_ty(ty: &Ty, types: &TypeTable) -> bool {
    match ty {
        Ty::I8
        | Ty::I16
        | Ty::I32
        | Ty::I64
        | Ty::U8
        | Ty::U16
        | Ty::U32
        | Ty::U64
        | Ty::Isize
        | Ty::Usize
        | Ty::F32
        | Ty::F64
        | Ty::Bool
        | Ty::RawPtr(_)
        | Ty::FnPtr { .. } => true,
        // Plain enums lower to `i32` (scalar); tagged enums to a struct.
        Ty::Enum(id) => !types.enum_defs[id.0 as usize].is_tagged,
        // v0.0.6 Slice 1C: SIMD vectors are register-passed scalars in
        // LLVM (no aggregate `poison` padding); `noundef` is sound at
        // the whole-value level.
        Ty::Simd { .. } => true,
        _ => false,
    }
}

/// Slice 6BC.opt move-scanning walker. Walks a function body collecting
/// the names of bindings used at any `move`-position argument or `move
/// self` receiver. Pure syntactic + callee-signature consultation —
/// doesn't reason about flow, so a binding moved inside an `if` arm
/// still counts as "moved somewhere." That's correct: if there's ANY
/// path that moves the binding, the drop flag must be runtime-checked.
fn scan_moves_in_block(
    b: &Block,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    set: &mut std::collections::HashSet<String>,
) {
    for s in &b.stmts {
        scan_moves_in_stmt(s, sigs, types, set);
    }
    if let Some(t) = &b.tail {
        // v0.0.5 Phase 1B: a block whose tail expression is a bare
        // `Ident(n)` moves that binding out of the block (its value
        // flows to whichever expression consumes the block). Pre-mark
        // the source so its drop_flag gets Runtime disposition; the
        // block-expr codegen flips it before pop_scope drops fire.
        if let ExprKind::Ident(n) = &t.kind {
            set.insert(n.clone());
        }
        scan_moves_in_expr(t, sigs, types, set);
    }
}

fn scan_moves_in_stmt(
    s: &Stmt,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    set: &mut std::collections::HashSet<String>,
) {
    match &s.kind {
        StmtKind::Let { init, .. } => {
            // v0.0.3 drop-tracking: `let v = some_ident;` moves the source
            // binding (for non-Copy types). Pre-register so codegen's
            // mark_moved has a flag to flip.
            if let Some(e) = init {
                if let ExprKind::Ident(n) = &e.kind {
                    set.insert(n.clone());
                }
                scan_moves_in_expr(e, sigs, types, set);
            }
        }
        StmtKind::Return(Some(e)) => {
            // v0.0.3 drop-tracking: `return <ident>;` for a non-Copy
            // binding moves the value out. Pre-mark so the runtime
            // drop flag gets allocated; codegen's mark_moved at the
            // Return site flips it before scope-exit drops fire.
            if let ExprKind::Ident(n) = &e.kind {
                set.insert(n.clone());
            }
            scan_moves_in_expr(e, sigs, types, set);
        }
        StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => {
            scan_moves_in_expr(e, sigs, types, set)
        }
        StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::While { cond, body, .. } => {
            scan_moves_in_expr(cond, sigs, types, set);
            scan_moves_in_block(body, sigs, types, set);
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                if let Some(i) = init.as_deref() {
                    scan_moves_in_stmt(i, sigs, types, set);
                }
                if let Some(c) = cond.as_ref() {
                    scan_moves_in_expr(c, sigs, types, set);
                }
                for u in update {
                    scan_moves_in_expr(u, sigs, types, set);
                }
                scan_moves_in_block(body, sigs, types, set);
            }
            ForLoop::Range { iter, body, .. } => {
                scan_moves_in_expr(iter, sigs, types, set);
                scan_moves_in_block(body, sigs, types, set);
            }
        },
        StmtKind::Loop(body, _) => scan_moves_in_block(body, sigs, types, set),
        // Lowered before codegen — should not appear here, but be safe.
        StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {}
    }
}

fn scan_moves_in_expr(
    e: &Expr,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    set: &mut std::collections::HashSet<String>,
) {
    match &e.kind {
        ExprKind::Call { callee, args, .. } => {
            // If callee is a known free function, consult sigs for
            // its move flags. Each move-arg of a plain `Ident` name
            // adds that binding to the moved set.
            if let ExprKind::Ident(fn_name) = &callee.kind {
                if let Some(sig) = sigs.get(fn_name) {
                    for (arg, (_pty, move_flag, _mut_flag, _restrict_flag)) in args.iter().zip(sig.params.iter()) {
                        if *move_flag {
                            if let ExprKind::Ident(n) = &arg.kind {
                                set.insert(n.clone());
                            }
                        }
                    }
                }
                // v0.0.3 Phase 5 Slice 5B: `__cplus_thread_join`
                // consumes its handle argument (frees the heap ctx
                // it points into). Treat it as a move so the
                // surrounding scope-exit drop is gated by a real
                // flag the intrinsic can flip.
                if fn_name == "__cplus_thread_join" {
                    if let Some(arg) = args.first() {
                        if let ExprKind::Ident(n) = &arg.kind {
                            set.insert(n.clone());
                        }
                    }
                }
                // v0.0.3 Phase 5 Slice 5C: `__cplus_thread_spawn_with`
                // moves its first value-arg (the input) into the
                // worker's context buffer. The trampoline reads it
                // before f runs, so the parent must relinquish
                // ownership at the spawn site.
                if fn_name == "__cplus_thread_spawn_with" {
                    if let Some(arg) = args.first() {
                        if let ExprKind::Ident(n) = &arg.kind {
                            set.insert(n.clone());
                        }
                    }
                }
            }
            // v0.0.3 drop-tracking: Call with a Path callee = associated-fn
            // call (enum variant construction `Result::Ok(v)`, or
            // struct assoc fn). Any non-Copy ident arg gets moved into the
            // constructed value; pre-register so codegen's `mark_moved`
            // call later has a drop flag to flip.
            if matches!(&callee.kind, ExprKind::Path { .. }) {
                for a in args {
                    if let ExprKind::Ident(n) = &a.kind {
                        set.insert(n.clone());
                    }
                }
            }
            // Method calls: `recv.method(args)` — when `method` has
            // `move self`, the receiver binding is moved. We don't
            // try to resolve method sigs from sigs; method-move is
            // detected via the type table lookup that codegen does
            // at the call site. Simplest conservative rule: if the
            // callee is a `Field` expression on an Ident receiver,
            // look up the method's receiver kind via types.
            if let ExprKind::Field { receiver, name: m } = &callee.kind {
                if let ExprKind::Ident(recv) = &receiver.kind {
                    // Need receiver's struct type to look up method
                    // sig. Codegen doesn't track binding types
                    // statically here; we walk all struct defs for
                    // a method matching `m.name`. Conservative —
                    // multiple matches just mean the name is added
                    // to the moved set, which is safe (no false
                    // optimization).
                    for sdef in &types.struct_defs {
                        if let Some(mi) = sdef.methods.get(&m.name) {
                            if matches!(mi.receiver, Some(crate::ast::Receiver::Move)) {
                                set.insert(recv.clone());
                            }
                        }
                    }
                }
                // G-027 fix: method calls also need to register
                // bare-Ident args at positions where the method's
                // param is `move`-marked. Pre-fix, codegen's
                // call-site mark_moved fired on the local but
                // find_drop_flag returned the `.unused` sentinel
                // (since the binding wasn't pre-registered as a
                // move source), producing an undefined-SSA store
                // that clang rejected. Same conservative walk as
                // the receiver path — multiple matches are safe.
                for sdef in &types.struct_defs {
                    if let Some(mi) = sdef.methods.get(&m.name) {
                        for (a, (_ty, move_flag, _mut_flag, _restr)) in args.iter().zip(mi.params.iter()) {
                            if *move_flag {
                                if let ExprKind::Ident(n) = &a.kind {
                                    set.insert(n.clone());
                                }
                            }
                        }
                    }
                }
            }
            scan_moves_in_expr(callee, sigs, types, set);
            for a in args {
                scan_moves_in_expr(a, sigs, types, set);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            scan_moves_in_expr(lhs, sigs, types, set);
            scan_moves_in_expr(rhs, sigs, types, set);
        }
        ExprKind::Unary { operand, .. } => scan_moves_in_expr(operand, sigs, types, set),
        ExprKind::Cast { expr, .. } => scan_moves_in_expr(expr, sigs, types, set),
        ExprKind::Field { receiver, .. } => scan_moves_in_expr(receiver, sigs, types, set),
        ExprKind::Index { receiver, index } => {
            scan_moves_in_expr(receiver, sigs, types, set);
            scan_moves_in_expr(index, sigs, types, set);
        }
        ExprKind::StructLit { fields, .. } | ExprKind::GenericStructLit { fields, .. } => {
            // G-023 fix: a field initialized from a bare-Ident source
            // (`Wrap { m: m }`) consumes that binding into the struct
            // slot. Pre-register so the runtime drop_flag gets allocated
            // and codegen's gen_struct_lit / gen_assign mark_moved flips
            // it before pop_scope drops fire. Without this the local
            // would be bitwise-copied into the field AND have its Drop
            // run, freeing inner heap storage the field aliases.
            for f in fields {
                if let ExprKind::Ident(n) = &f.value.kind {
                    set.insert(n.clone());
                }
                scan_moves_in_expr(&f.value, sigs, types, set);
            }
        }
        ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
            for el in elements {
                scan_moves_in_expr(el, sigs, types, set);
            }
        }
        ExprKind::Block(b) => scan_moves_in_block(b, sigs, types, set),
        ExprKind::Unsafe(b) => scan_moves_in_block(b, sigs, types, set),
        ExprKind::Await(inner) => scan_moves_in_expr(inner, sigs, types, set),
        ExprKind::Yield(inner) => scan_moves_in_expr(inner, sigs, types, set),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            scan_moves_in_expr(cond, sigs, types, set);
            scan_moves_in_block(then, sigs, types, set);
            if let Some(eb) = else_branch.as_deref() {
                scan_moves_in_expr(eb, sigs, types, set);
            }
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                scan_moves_in_expr(s, sigs, types, set);
            }
            if let Some(e) = end {
                scan_moves_in_expr(e, sigs, types, set);
            }
        }
        ExprKind::Assign { target, value, op } => {
            // G-023 fix: a plain `=` assignment whose RHS is a bare Ident
            // consumes that binding into the destination slot. Same
            // shape as Let-init-from-Ident or Return-Ident. Compound
            // assigns (`+=`, etc.) read+modify and don't transfer
            // ownership, so they don't qualify. The most-cited surface
            // is the raw-pointer store inside `unsafe { *p = val; }`
            // (used by `Box::new[T]`, `arena::alloc[T]`, and any
            // hand-rolled "copy into heap slot" helper).
            if matches!(op, AssignOp::Assign) {
                if let ExprKind::Ident(n) = &value.kind {
                    set.insert(n.clone());
                }
            }
            scan_moves_in_expr(target, sigs, types, set);
            scan_moves_in_expr(value, sigs, types, set);
        }
        ExprKind::Match { scrutinee, arms } => {
            // v0.0.14 enum-variant drop: matching an owned enum *consumes* it —
            // the payload is moved into the arm bindings and the scrutinee's
            // scope-exit drop is disarmed (gen_match calls `mark_moved`). Mark
            // a bare-Ident scrutinee here so it gets a Runtime drop flag the
            // disarm can flip. (A borrow-param scrutinee has no drop entry, so
            // this is a harmless no-op for it.)
            if let ExprKind::Ident(n) = &scrutinee.kind {
                set.insert(n.clone());
            }
            scan_moves_in_expr(scrutinee, sigs, types, set);
            for a in arms {
                // v0.0.14: a bare-`Ident` arm body (`Variant(s) => s`) moves
                // that binding out as the match value — the same shape as a
                // block-tail `Ident` (handled in scan_moves_in_block). Mark it
                // so an owning payload binding gets a Runtime drop flag the
                // move-out can disarm (else registering its drop would
                // double-free).
                if let ExprKind::Ident(n) = &a.body.kind {
                    set.insert(n.clone());
                }
                scan_moves_in_expr(&a.body, sigs, types, set);
            }
        }
        _ => {}
    }
}

fn write_struct_decls(out: &mut String, types: &TypeTable, _p: &Program) {
    let any_struct = !types.struct_defs.is_empty();
    let any_tagged_enum = types.enum_defs.iter().any(|e| e.is_tagged);
    if !any_struct && !any_tagged_enum {
        return;
    }
    // Struct named-type declarations (Phase 2B).
    for s in &types.struct_defs {
        let inner: Vec<String> = s.fields.iter().map(|(_, t)| llvm_ty(t, types)).collect();
        writeln!(out, "%{} = type {{ {} }}", s.name, inner.join(", ")).unwrap();
    }
    // Tagged-enum named-type declarations (Phase 3I). Layout is
    // `{ i32 tag, [N x i64] payload }` where N is the max payload-slot
    // count across variants. Each payload value occupies one i64-aligned
    // slot — Phase 3 simplification that wastes some bytes but guarantees
    // 8-byte alignment everywhere.
    for (i, info) in types.enum_defs.iter().enumerate() {
        if !info.is_tagged {
            continue;
        }
        let id = EnumId(i as u32);
        let name = enum_struct_name(id, types);
        writeln!(
            out,
            "%{} = type {{ i32, [{} x i64] }}",
            name, info.payload_slots
        )
        .unwrap();
    }
    out.push('\n');
}

fn ty_from(t: &Type, types: &TypeTable) -> Ty {
    let name = match &t.kind {
        TypeKind::Path(n) => n,
        TypeKind::Array { elem, len, .. } => {
            let elem_ty = ty_from(elem, types);
            return Ty::Array(Box::new(elem_ty), *len);
        }
        // Slice 6BC.5: region annotations are transparent at codegen
        // time. `borrow A T` lowers exactly like T — the region is
        // borrow-checker metadata, not a runtime construct.
        TypeKind::Borrowed { inner, .. } => return ty_from(inner, types),
        // Slice 7GEN.5c: monomorphize rewrites every `TypeKind::Generic`
        // to a concrete `TypeKind::Path(mangled_name)` before codegen.
        // If we reach here it means the rewrite missed a site.
        TypeKind::Generic { .. } => {
            panic!("codegen reached TypeKind::Generic — monomorphize did not rewrite this site")
        }
        // Slice 10.FFI.1: raw pointer lowers to LLVM `ptr` regardless
        // of pointee. Pointee info is sema-level only.
        TypeKind::RawPtr(inner) => {
            let inner_ty = ty_from(inner, types);
            return Ty::RawPtr(Box::new(inner_ty));
        }
        // Slice 11.FN_PTR: fn-ptr lowers to LLVM `ptr` regardless of signature.
        TypeKind::FnPtr {
            params,
            return_type,
        } => {
            let resolved_params: Vec<Ty> = params.iter().map(|p| ty_from(p, types)).collect();
            let resolved_ret = match return_type {
                Some(rt) => ty_from(rt, types),
                None => Ty::Unit,
            };
            return Ty::FnPtr {
                params: resolved_params,
                return_type: Box::new(resolved_ret),
            };
        }
        // Phase 11 polish (2026-05-14): slice type.
        TypeKind::Slice(inner) => {
            let inner_ty = ty_from(inner, types);
            return Ty::Slice(Box::new(inner_ty));
        }
        // v0.0.5 Phase 3 Slice 3B: tuple types are lowered to a
        // synthesized struct (`__tuple_N_<t1>_<t2>_...`) by sema +
        // monomorphize before codegen sees them. Reaching here means
        // a lowering site was missed.
        TypeKind::Tuple(_) => {
            panic!("codegen reached TypeKind::Tuple — sema/monomorphize did not lower this site")
        }
    };
    match name.as_str() {
        "i8" => Ty::I8,
        "i16" => Ty::I16,
        "i32" => Ty::I32,
        "i64" => Ty::I64,
        "u8" => Ty::U8,
        "u16" => Ty::U16,
        "u32" => Ty::U32,
        "u64" => Ty::U64,
        "isize" => Ty::Isize,
        "usize" => Ty::Usize,
        "f16" => Ty::F16,
        "f32" => Ty::F32,
        "f64" => Ty::F64,
        "bool" => Ty::Bool,
        "str" => Ty::Str,
        "string" => Ty::String,
        // v0.0.12 G-026: `()` is the unit type. Sema's `resolve_type`
        // has the same arm; codegen mirrors it because `collect_sigs`
        // walks the raw AST through this helper, not through sema.
        "()" => Ty::Unit,
        // v0.0.6 Slice 1B: SIMD type names. Mirror sema's resolve_type.
        "f32x4" => Ty::Simd {
            elem: Box::new(Ty::F32),
            lanes: 4,
        },
        "f64x2" => Ty::Simd {
            elem: Box::new(Ty::F64),
            lanes: 2,
        },
        "i32x4" => Ty::Simd {
            elem: Box::new(Ty::I32),
            lanes: 4,
        },
        "i64x2" => Ty::Simd {
            elem: Box::new(Ty::I64),
            lanes: 2,
        },
        "u64x2" => Ty::Simd {
            elem: Box::new(Ty::U64),
            lanes: 2,
        },
        "u32x4" => Ty::Simd {
            elem: Box::new(Ty::U32),
            lanes: 4,
        },
        "i8x16" => Ty::Simd {
            elem: Box::new(Ty::I8),
            lanes: 16,
        },
        "i16x8" => Ty::Simd {
            elem: Box::new(Ty::I16),
            lanes: 8,
        },
        "u8x16" => Ty::Simd {
            elem: Box::new(Ty::U8),
            lanes: 16,
        },
        "u16x8" => Ty::Simd {
            elem: Box::new(Ty::U16),
            lanes: 8,
        },
        // v0.0.12 SIMD Tier-1 (G-039a): 64-bit (sub-128) widths — the NEON
        // D-register family. The result of `i8x16::low/high` and the input
        // to `widen` / `combine`.
        "i8x8"   => Ty::Simd { elem: Box::new(Ty::I8),  lanes: 8 },
        "u8x8"   => Ty::Simd { elem: Box::new(Ty::U8),  lanes: 8 },
        "i16x4"  => Ty::Simd { elem: Box::new(Ty::I16), lanes: 4 },
        "u16x4"  => Ty::Simd { elem: Box::new(Ty::U16), lanes: 4 },
        "i32x2"  => Ty::Simd { elem: Box::new(Ty::I32), lanes: 2 },
        "u32x2"  => Ty::Simd { elem: Box::new(Ty::U32), lanes: 2 },
        "f32x2"  => Ty::Simd { elem: Box::new(Ty::F32), lanes: 2 },
        // v0.0.7 Slice 2.2: 256-bit widths.
        "f32x8"  => Ty::Simd { elem: Box::new(Ty::F32), lanes: 8  },
        "f64x4"  => Ty::Simd { elem: Box::new(Ty::F64), lanes: 4  },
        "i8x32"  => Ty::Simd { elem: Box::new(Ty::I8),  lanes: 32 },
        "u8x32"  => Ty::Simd { elem: Box::new(Ty::U8),  lanes: 32 },
        "i16x16" => Ty::Simd { elem: Box::new(Ty::I16), lanes: 16 },
        "u16x16" => Ty::Simd { elem: Box::new(Ty::U16), lanes: 16 },
        "i32x8"  => Ty::Simd { elem: Box::new(Ty::I32), lanes: 8  },
        "u32x8"  => Ty::Simd { elem: Box::new(Ty::U32), lanes: 8  },
        "i64x4"  => Ty::Simd { elem: Box::new(Ty::I64), lanes: 4  },
        "u64x4"  => Ty::Simd { elem: Box::new(Ty::U64), lanes: 4  },
        // v0.0.9 follow-up: mask types resolve to `Ty::Mask`, a
        // distinct sema-level type whose LLVM lowering matches the
        // width-equivalent signed-int SIMD. Codegen's `lty` treats
        // Ty::Mask and Ty::Simd identically.
        "mask8x16"  => Ty::Mask { elem: Box::new(Ty::I8),  lanes: 16 },
        "mask16x8"  => Ty::Mask { elem: Box::new(Ty::I16), lanes: 8  },
        "mask32x4"  => Ty::Mask { elem: Box::new(Ty::I32), lanes: 4  },
        "mask64x2"  => Ty::Mask { elem: Box::new(Ty::I64), lanes: 2  },
        "mask8x32"  => Ty::Mask { elem: Box::new(Ty::I8),  lanes: 32 },
        "mask16x16" => Ty::Mask { elem: Box::new(Ty::I16), lanes: 16 },
        "mask32x8"  => Ty::Mask { elem: Box::new(Ty::I32), lanes: 8  },
        "mask64x4"  => Ty::Mask { elem: Box::new(Ty::I64), lanes: 4  },
        _ => {
            if let Some(&id) = types.enum_by_name.get(name) {
                return Ty::Enum(id);
            }
            if let Some(&id) = types.struct_by_name.get(name) {
                return Ty::Struct(id);
            }
            Ty::Error
        }
    }
}

fn llvm_ty(ty: &Ty, types: &TypeTable) -> String {
    match ty {
        Ty::I8 | Ty::U8 => "i8".to_string(),
        Ty::I16 | Ty::U16 => "i16".to_string(),
        Ty::I32 | Ty::U32 => "i32".to_string(),
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize => "i64".to_string(),
        Ty::F16 => "half".to_string(),
        Ty::F32 => "float".to_string(),
        Ty::F64 => "double".to_string(),
        Ty::Bool => "i1".to_string(),
        Ty::Unit => "void".to_string(),
        Ty::Struct(id) => format!("%{}", types.struct_defs[id.0 as usize].name),
        Ty::Enum(id) => {
            let info = &types.enum_defs[id.0 as usize];
            // Plain enums (no variant has a payload) stay bare `i32` —
            // Phase-2A fast path. Tagged enums use a named struct type
            // emitted in the preamble: `%E = type { i32, [N x i64] }`.
            if info.is_tagged {
                format!("%{}", enum_struct_name(*id, types))
            } else {
                "i32".to_string()
            }
        }
        Ty::Array(elem, n) => format!("[{n} x {}]", llvm_ty(elem, types)),
        // v0.0.6 Slice 1B: SIMD vectors lower to LLVM `<N x T>` where T is
        // the lane scalar's IR type. LLVM understands these natively;
        // arithmetic, intrinsics (`llvm.fma.v4f32`), and shuffles all
        // operate directly on vector types. Masks share the same
        // lowering — the sema-level Mask/Simd distinction is erased
        // at the IR level for ABI compatibility.
        Ty::Simd { elem, lanes } | Ty::Mask { elem, lanes } => {
            format!("<{lanes} x {}>", llvm_ty(elem, types))
        }
        // Slice 10.FFI.1: raw pointers lower to LLVM `ptr` (opaque,
        // 8 bytes on 64-bit). Pointee info is sema-only.
        Ty::RawPtr(_) => "ptr".to_string(),
        // Slice 11.FN_PTR: fn pointers also lower to LLVM `ptr`. Sema
        // carries the param/return type info; codegen indirect-calls
        // know the call signature from the FnPtr Ty, not from the LLVM IR.
        Ty::FnPtr { .. } => "ptr".to_string(),
        // Phase 8 slice 8.STR.1: `str` is a fat pointer { ptr, len }.
        // 16 bytes on 64-bit platforms; passed by value.
        Ty::Str => "{ ptr, i64 }".to_string(),
        // Phase 8 slice 8.STR.3: owned `string` is { ptr, len, cap } —
        // 24 bytes on 64-bit. Passed by value; the ptr is the only field
        // codegen ever sees per-call, but the cap field is what `drop`
        // reads when freeing the buffer.
        Ty::String => "{ ptr, i64, i64 }".to_string(),
        // Phase 11 polish (2026-05-14): slice type `T[]` is a fat
        // pointer { ptr, len } — same shape as `str`. The element type
        // `T` is sema-only; LLVM sees just the pair.
        Ty::Slice(_) => "{ ptr, i64 }".to_string(),
        Ty::Error => panic!("codegen reached Ty::Error — sema should have rejected the program"),
        // Slice 7GEN.4: `Ty::Param` must not reach codegen. Until
        // monomorphization (slice 7GEN.5) lowers generic items, the
        // parser+sema admit generic surface but no generic item is
        // codegen-emitted — sema's reachability prevents calling a
        // generic from a concrete-typed context (its return type
        // would carry `Ty::Param`).
        Ty::Param(_) => {
            panic!("codegen reached Ty::Param — generics not yet monomorphized (slice 7GEN.5)")
        }
    }
}

/// LLVM type name for a tagged enum. We don't track the enum's source name
/// in `EnumInfo`, so synthesize a stable identifier from the EnumId. The
/// preamble emits `%enum.0 = type { ... }` etc.
fn enum_struct_name(id: EnumId, _types: &TypeTable) -> String {
    format!("enum.{}", id.0)
}

fn ty_bit_width(ty: &Ty) -> u32 {
    match ty {
        Ty::I8 | Ty::U8 => 8,
        Ty::I16 | Ty::U16 | Ty::F16 => 16,
        Ty::I32 | Ty::U32 | Ty::F32 | Ty::Enum(_) => 32,
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::F64 => 64,
        Ty::Bool => 1,
        _ => 0,
    }
}

/// LLVM IR for a Windows global constructor that switches stdout/stderr to
/// binary mode, so a printed '\n' stays a single LF byte instead of being
/// expanded to "\r\n" by the MSVC C runtime's text-mode translation. C+
/// follows the Rust/Go convention that '\n' is LF on every platform.
/// `_setmode(fd, _O_BINARY)` lives in the UCRT (`_O_BINARY` == 0x8000); fd 1
/// is stdout, fd 2 is stderr. The ctor is `internal` and `@llvm.global_ctors`
/// is `appending`, so emitting it in every module is safe (LLVM merges them)
/// and idempotent. Returns `""` on non-Windows hosts. Emitted by
/// `write_preamble` and injected into the frozen `hello.ll` demo so the
/// behavior is uniform across both codegen paths.
pub fn windows_binary_mode_ctor_ir() -> &'static str {
    if cfg!(windows) {
        "declare i32 @_setmode(i32, i32)\n\
         define internal void @__cpc_set_binary_mode() {\n\
         \x20 %1 = call i32 @_setmode(i32 1, i32 32768)\n\
         \x20 %2 = call i32 @_setmode(i32 2, i32 32768)\n\
         \x20 ret void\n\
         }\n\
         @llvm.global_ctors = appending global [1 x { i32, ptr, ptr }] \
         [{ i32, ptr, ptr } { i32 65535, ptr @__cpc_set_binary_mode, ptr null }]\n"
    } else {
        ""
    }
}

/// Whether `llvm.coro.end` returns `void` (LLVM ~22+) versus the older `i1`
/// (older LLVM, and Apple clang 21). The correct form depends on the *target
/// toolchain's* LLVM version — not the host `cpc` runs on — so `cpc` probes the
/// discovered clang once at build time and installs the answer here via
/// `set_coro_end_returns_void` before generating IR.
///
/// Defaults to `void` (the modern form). The `cpc` driver always sets the
/// probed value before codegen, and the IR-only unit tests never link, so the
/// default does not affect them.
static CORO_END_RETURNS_VOID: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Install the probed `llvm.coro.end` return-type form (see
/// `CORO_END_RETURNS_VOID`). Called by the driver before codegen.
pub fn set_coro_end_returns_void(returns_void: bool) {
    CORO_END_RETURNS_VOID.store(returns_void, std::sync::atomic::Ordering::Relaxed);
}

fn coro_end_returns_void() -> bool {
    CORO_END_RETURNS_VOID.load(std::sync::atomic::Ordering::Relaxed)
}

/// The `declare` for `llvm.coro.end`, in whichever return-type form the target
/// toolchain expects.
fn coro_end_decl_ir() -> String {
    if coro_end_returns_void() {
        "declare void @llvm.coro.end(ptr, i1, token)\n".to_string()
    } else {
        "declare i1 @llvm.coro.end(ptr, i1, token)\n".to_string()
    }
}

/// A `call` to `llvm.coro.end` on `%.coro.hdl`, matching the declared form. The
/// `i1` form binds (and discards) a result SSA value; the `void` form does not.
fn coro_end_call_ir() -> String {
    if coro_end_returns_void() {
        "  call void @llvm.coro.end(ptr %.coro.hdl, i1 false, token none)\n".to_string()
    } else {
        "  %.coro.end_token = call i1 @llvm.coro.end(ptr %.coro.hdl, i1 false, token none)\n"
            .to_string()
    }
}

fn write_preamble(out: &mut String) {
    out.push_str("; C+ Phase 1 codegen output\n");
    out.push_str("\n");
    out.push_str(windows_binary_mode_ctor_ir());
    // Format string used by `#println(i32)`. Module-private constant.
    out.push_str(
        "@.fmt_int_nl = private unnamed_addr constant [4 x i8] c\"%d\\0A\\00\", align 1\n",
    );
    // Phase 8 slice 8.STR.2: format string for `#println(str)`. Uses
    // `%.*s` so the pointer + length are passed verbatim (no NUL
    // assumption — strings may legitimately contain embedded NULs).
    out.push_str(
        "@.fmt_str_nl = private unnamed_addr constant [6 x i8] c\"%.*s\\0A\\00\", align 1\n",
    );
    out.push_str("\n");
    out.push_str("declare i32 @printf(ptr noundef, ...)\n");
    // Phase 8 slice 8.STR.3: byte-level string comparison.
    // v0.0.8 bench-gap fix B: memcmp's two pointers are read-only and
    // the function returns a deterministic value of the bytes — clang
    // declares this libc header with `readonly` so the optimizer can
    // hoist memcmp calls past intervening loads.
    out.push_str("declare i32 @memcmp(ptr noundef readonly, ptr noundef readonly, i64 noundef)\n");
    // Phase 8 slice 8.STR.3: owned `string` runtime. malloc + free for
    // construction + Drop; memcpy for clone. realloc reserved for future
    // mutation API (not used in v1).
    //
    // v0.0.8 bench-gap fix B: clang declares malloc with `noalias` on
    // the return (fresh allocations don't alias existing memory) plus
    // `noundef` on the size param + return. The `noalias` here is the
    // single biggest enabler for LLVM's alias analysis to disambiguate
    // heap regions against pre-existing pointers, which lets SROA /
    // mem2reg / GVN forward more loads.
    out.push_str("declare noalias noundef ptr @malloc(i64 noundef)\n");
    // v0.0.8 bench-gap fix B (finish): free is a no-capture deallocator
    // — `nocapture` says the function doesn't retain the pointer beyond
    // the call, so LLVM can keep deriving facts about prior pointers to
    // the same place across the free. `noundef` matches clang's emission.
    out.push_str("declare void @free(ptr nocapture noundef)\n");
    // v0.0.8 bench-gap fix B: memcpy's contract requires non-overlapping
    // src/dst (C99 7.21.2.1) — encode that as `noalias` on both pointer
    // params. `writeonly` on dst + `readonly` on src lets LLVM model the
    // call as not reading the destination and not writing through the
    // source, which is necessary for SROA / DSE around the copy.
    out.push_str("declare ptr @memcpy(ptr noalias noundef writeonly, ptr noalias noundef readonly, i64 noundef)\n");
    // Phase 8 slice 8.STR.6: snprintf for blessed `to_string()` on
    // numeric primitives. Returns the number of bytes that *would have*
    // been written (excluding NUL); we use that as the resulting
    // `string.len`. The 32-byte buffer comfortably covers every 64-bit
    // integer decimal plus a sign + the `%g` float format.
    //
    // v0.0.8 bench-gap fix B (finish): dst is writeonly, fmt is
    // readonly, and both carry noundef + the size param is noundef.
    // The two ptr params don't alias each other in any valid use.
    out.push_str(
        "declare i32 @snprintf(ptr noalias noundef writeonly, i64 noundef, \
         ptr noalias noundef readonly, ...)\n",
    );
    // Format strings the to_string intrinsics use.
    out.push_str("@.fmt_i64    = private unnamed_addr constant [5 x i8] c\"%lld\\00\", align 1\n");
    out.push_str("@.fmt_u64    = private unnamed_addr constant [5 x i8] c\"%llu\\00\", align 1\n");
    out.push_str("@.fmt_f64    = private unnamed_addr constant [3 x i8] c\"%g\\00\", align 1\n");
    out.push_str("@.bool_true  = private unnamed_addr constant [4 x i8] c\"true\", align 1\n");
    out.push_str("@.bool_false = private unnamed_addr constant [5 x i8] c\"false\", align 1\n");
    // Trap intrinsic — used for both overflow (debug) and divide-by-zero (always).
    out.push_str("declare void @llvm.trap()\n");
    // Slice 1B (v0.0.2): assume intrinsic — used to publish facts the
    // frontend has proven (bounds-check success, slice-length non-negative)
    // so `-O2`'s ConstraintElimination/InstCombine can elide downstream
    // redundant checks. At -O0 this is a no-op call.
    out.push_str("declare void @llvm.assume(i1 noundef)\n");
    // Phase 3A (v0.0.2): byte-swap intrinsics. Used by `bswap16/32/64`
    // and `htons`/`htonl`/`ntohs`/`ntohl` aliases. All declared so DCE
    // can strip the unused widths.
    out.push_str("declare i16 @llvm.bswap.i16(i16)\n");
    out.push_str("declare i32 @llvm.bswap.i32(i32)\n");
    out.push_str("declare i64 @llvm.bswap.i64(i64)\n");
    // Phase 5 Slice 5.D: memset intrinsic for zero-initializing C-ABI
    // return coercion slots. Used so tail bytes (beyond the original
    // struct's footprint) read as 0 instead of poison when packed into
    // the coerced integer-class return register.
    // v0.0.8 bench-gap fix B (finish): per LLVM's intrinsic contract,
    // memset's dst is `writeonly` (the call doesn't observe pre-existing
    // bytes through dst) and the isvolatile arg must be `immarg`. Add
    // `noalias noundef` on dst + `noundef` on the byte/length args to
    // give LLVM's alias analysis the same facts clang emits.
    out.push_str(
        "declare void @llvm.memset.p0.i64(ptr noalias noundef writeonly, \
         i8 noundef, i64 noundef, i1 immarg)\n",
    );
    // v0.0.12 G-031: per-arch spin-loop hint. Declared unconditionally
    // (DCE strips the unused one), used by `#cpu_relax()` codegen.
    if cfg!(target_arch = "aarch64") {
        out.push_str("declare void @llvm.aarch64.hint(i32 immarg)\n");
        // v0.0.12 SIMD Tier-1 (G-040): NEON `vqtbl1q` byte table lookup,
        // used by `i8x16/u8x16::table`. Out-of-range indices yield 0.
        out.push_str(
            "declare <16 x i8> @llvm.aarch64.neon.tbl1.v16i8(<16 x i8>, <16 x i8>)\n",
        );
    } else if cfg!(target_arch = "x86_64") {
        out.push_str("declare void @llvm.x86.sse2.pause()\n");
    }
    // v0.0.7 Slice 1.1: lifetime intrinsics. In release builds the
    // alloca helpers bracket each local's live range with these so
    // LLVM's SROA can reuse stack slots across non-overlapping
    // scopes. Disabled at `-O0` (debug builds skip the calls to keep
    // lldb's frame walker simple); declared unconditionally so the
    // emitted IR is identical-shape across modes.
    out.push_str("declare void @llvm.lifetime.start.p0(i64, ptr)\n");
    out.push_str("declare void @llvm.lifetime.end.p0(i64, ptr)\n");
    // v0.0.5: FMA intrinsics for the `a*b + c` peephole in gen_binary.
    // clang lowers source-level `a*b+c` to `llvm.fmuladd` directly at
    // `-ffp-contract=on` (default); cpc does the same so hot raytracer
    // loops match C's instruction count and FP-rounding behavior.
    out.push_str("declare float  @llvm.fmuladd.f32(float, float, float)\n");
    out.push_str("declare double @llvm.fmuladd.f64(double, double, double)\n");
    // v0.0.6 Slice 1B: SIMD intrinsic declarations. First cut covers
    // the f32x4 width; other widths land alongside their type names.
    out.push_str("declare <4 x float> @llvm.fma.v4f32(<4 x float>, <4 x float>, <4 x float>)\n");
    out.push_str("declare <4 x float> @llvm.sqrt.v4f32(<4 x float>)\n");
    out.push_str(
        "declare <2 x double> @llvm.fma.v2f64(<2 x double>, <2 x double>, <2 x double>)\n",
    );
    out.push_str("declare <2 x double> @llvm.sqrt.v2f64(<2 x double>)\n");
    out.push_str("declare <4 x float> @llvm.fabs.v4f32(<4 x float>)\n");
    out.push_str("declare <2 x double> @llvm.fabs.v2f64(<2 x double>)\n");
    out.push_str("declare <4 x float> @llvm.roundeven.v4f32(<4 x float>)\n");
    out.push_str("declare <2 x double> @llvm.roundeven.v2f64(<2 x double>)\n");
    out.push_str("declare <4 x i32> @llvm.abs.v4i32(<4 x i32>, i1)\n");
    out.push_str("declare <2 x i64> @llvm.abs.v2i64(<2 x i64>, i1)\n");
    out.push_str("declare <16 x i8> @llvm.abs.v16i8(<16 x i8>, i1)\n");
    out.push_str("declare <8 x i16> @llvm.abs.v8i16(<8 x i16>, i1)\n");
    out.push_str("declare <4 x float> @llvm.minnum.v4f32(<4 x float>, <4 x float>)\n");
    out.push_str("declare <4 x float> @llvm.maxnum.v4f32(<4 x float>, <4 x float>)\n");
    out.push_str("declare <2 x double> @llvm.minnum.v2f64(<2 x double>, <2 x double>)\n");
    out.push_str("declare <2 x double> @llvm.maxnum.v2f64(<2 x double>, <2 x double>)\n");
    out.push_str("declare <4 x i32> @llvm.smin.v4i32(<4 x i32>, <4 x i32>)\n");
    out.push_str("declare <4 x i32> @llvm.smax.v4i32(<4 x i32>, <4 x i32>)\n");
    out.push_str("declare <2 x i64> @llvm.smin.v2i64(<2 x i64>, <2 x i64>)\n");
    out.push_str("declare <2 x i64> @llvm.smax.v2i64(<2 x i64>, <2 x i64>)\n");
    // v0.0.7 Slice 2.2 audit: `u64x2` was the 1B gap among 128-bit
    // 8-byte-lane widths (only `i64x2` shipped). The umin/umax
    // intrinsics are the only per-width declarations it needs — every
    // other method on `u64x2` (add/sub/mul/div/and/or/xor/shl/shr/
    // splat/new/from_array/to_array/lane/with_lane/load/store) lowers
    // to native LLVM instructions with no intrinsic call.
    out.push_str("declare <2 x i64> @llvm.umin.v2i64(<2 x i64>, <2 x i64>)\n");
    out.push_str("declare <2 x i64> @llvm.umax.v2i64(<2 x i64>, <2 x i64>)\n");
    out.push_str("declare <4 x i32> @llvm.umin.v4i32(<4 x i32>, <4 x i32>)\n");
    out.push_str("declare <4 x i32> @llvm.umax.v4i32(<4 x i32>, <4 x i32>)\n");
    out.push_str("declare <16 x i8> @llvm.smin.v16i8(<16 x i8>, <16 x i8>)\n");
    out.push_str("declare <16 x i8> @llvm.smax.v16i8(<16 x i8>, <16 x i8>)\n");
    out.push_str("declare <16 x i8> @llvm.umin.v16i8(<16 x i8>, <16 x i8>)\n");
    out.push_str("declare <16 x i8> @llvm.umax.v16i8(<16 x i8>, <16 x i8>)\n");
    out.push_str("declare <8 x i16> @llvm.smin.v8i16(<8 x i16>, <8 x i16>)\n");
    out.push_str("declare <8 x i16> @llvm.smax.v8i16(<8 x i16>, <8 x i16>)\n");
    out.push_str("declare <8 x i16> @llvm.umin.v8i16(<8 x i16>, <8 x i16>)\n");
    out.push_str("declare <8 x i16> @llvm.umax.v8i16(<8 x i16>, <8 x i16>)\n");
    // v0.0.7 Slice 2.2: 256-bit SIMD intrinsics. AArch64 splits them
    // into two 128-bit ops at the LLVM backend; AVX2/SVE2 hosts use
    // native 256-bit vectors. Same intrinsic families as the 128-bit
    // widths — float fma/sqrt/fabs/minnum/maxnum, int abs/smin/smax/
    // umin/umax. No new method semantics; the method dispatch in
    // gen_simd_method_call is fully generic over (elem, lanes).
    out.push_str("declare <8 x float> @llvm.fma.v8f32(<8 x float>, <8 x float>, <8 x float>)\n");
    out.push_str("declare <8 x float> @llvm.sqrt.v8f32(<8 x float>)\n");
    out.push_str("declare <8 x float> @llvm.fabs.v8f32(<8 x float>)\n");
    out.push_str("declare <8 x float> @llvm.roundeven.v8f32(<8 x float>)\n");
    out.push_str("declare <8 x float> @llvm.minnum.v8f32(<8 x float>, <8 x float>)\n");
    out.push_str("declare <8 x float> @llvm.maxnum.v8f32(<8 x float>, <8 x float>)\n");
    out.push_str("declare <4 x double> @llvm.fma.v4f64(<4 x double>, <4 x double>, <4 x double>)\n");
    out.push_str("declare <4 x double> @llvm.sqrt.v4f64(<4 x double>)\n");
    out.push_str("declare <4 x double> @llvm.fabs.v4f64(<4 x double>)\n");
    out.push_str("declare <4 x double> @llvm.roundeven.v4f64(<4 x double>)\n");
    out.push_str("declare <4 x double> @llvm.minnum.v4f64(<4 x double>, <4 x double>)\n");
    out.push_str("declare <4 x double> @llvm.maxnum.v4f64(<4 x double>, <4 x double>)\n");
    // i8x32 / u8x32
    out.push_str("declare <32 x i8> @llvm.abs.v32i8(<32 x i8>, i1)\n");
    out.push_str("declare <32 x i8> @llvm.smin.v32i8(<32 x i8>, <32 x i8>)\n");
    out.push_str("declare <32 x i8> @llvm.smax.v32i8(<32 x i8>, <32 x i8>)\n");
    out.push_str("declare <32 x i8> @llvm.umin.v32i8(<32 x i8>, <32 x i8>)\n");
    out.push_str("declare <32 x i8> @llvm.umax.v32i8(<32 x i8>, <32 x i8>)\n");
    // i16x16 / u16x16
    out.push_str("declare <16 x i16> @llvm.abs.v16i16(<16 x i16>, i1)\n");
    out.push_str("declare <16 x i16> @llvm.smin.v16i16(<16 x i16>, <16 x i16>)\n");
    out.push_str("declare <16 x i16> @llvm.smax.v16i16(<16 x i16>, <16 x i16>)\n");
    out.push_str("declare <16 x i16> @llvm.umin.v16i16(<16 x i16>, <16 x i16>)\n");
    out.push_str("declare <16 x i16> @llvm.umax.v16i16(<16 x i16>, <16 x i16>)\n");
    // i32x8 / u32x8
    out.push_str("declare <8 x i32> @llvm.abs.v8i32(<8 x i32>, i1)\n");
    out.push_str("declare <8 x i32> @llvm.smin.v8i32(<8 x i32>, <8 x i32>)\n");
    out.push_str("declare <8 x i32> @llvm.smax.v8i32(<8 x i32>, <8 x i32>)\n");
    out.push_str("declare <8 x i32> @llvm.umin.v8i32(<8 x i32>, <8 x i32>)\n");
    out.push_str("declare <8 x i32> @llvm.umax.v8i32(<8 x i32>, <8 x i32>)\n");
    // i64x4 / u64x4
    out.push_str("declare <4 x i64> @llvm.abs.v4i64(<4 x i64>, i1)\n");
    out.push_str("declare <4 x i64> @llvm.smin.v4i64(<4 x i64>, <4 x i64>)\n");
    out.push_str("declare <4 x i64> @llvm.smax.v4i64(<4 x i64>, <4 x i64>)\n");
    out.push_str("declare <4 x i64> @llvm.umin.v4i64(<4 x i64>, <4 x i64>)\n");
    out.push_str("declare <4 x i64> @llvm.umax.v4i64(<4 x i64>, <4 x i64>)\n");
    // v0.0.7 Slice 2.1: vector reduction intrinsics. Used by
    // `sum` / `product` / `min_across` / `max_across` (numeric SIMD)
    // and `any` / `all` (mask SIMD via i1 vector intermediates).
    // Float reductions are seeded (sequential fp), int reductions are
    // bare. LLVM auto-instantiates these — declarations exist so the
    // emitted IR is self-contained for `cpc --emit-ll | clang`.
    let float_widths = [(4u32, "f32", "float"), (2, "f64", "double"),
                        (8, "f32", "float"), (4, "f64", "double")];
    for (n, suf, lty) in &float_widths {
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.fadd.v{n}{suf}({lty}, <{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.fmul.v{n}{suf}({lty}, <{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.fmin.v{n}{suf}(<{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.fmax.v{n}{suf}(<{n} x {lty}>)\n"));
    }
    let int_widths: &[(u32, &str)] = &[
        (16, "i8"), (32, "i8"),
        (8, "i16"), (16, "i16"),
        (4, "i32"), (8, "i32"),
        (2, "i64"), (4, "i64"),
    ];
    for (n, lty) in int_widths {
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.add.v{n}{lty}(<{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.mul.v{n}{lty}(<{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.smin.v{n}{lty}(<{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.smax.v{n}{lty}(<{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.umin.v{n}{lty}(<{n} x {lty}>)\n"));
        out.push_str(&format!("declare {lty} @llvm.vector.reduce.umax.v{n}{lty}(<{n} x {lty}>)\n"));
    }
    // `any` / `all` lower to or/and reductions on i1 vectors. One
    // per mask width.
    let i1_widths = [2u32, 4, 8, 16, 32];
    for n in &i1_widths {
        out.push_str(&format!("declare i1 @llvm.vector.reduce.or.v{n}i1(<{n} x i1>)\n"));
        out.push_str(&format!("declare i1 @llvm.vector.reduce.and.v{n}i1(<{n} x i1>)\n"));
    }
    // v0.0.3 Phase 5 Slice 5B: pthread externs for the thread::spawn /
    // JoinHandle::join intrinsics. pthread_t is opaque-pointer-sized
    // (8 bytes on every supported target); we model it as `i64` to
    // keep the calls platform-agnostic — both arm64-darwin and
    // x86_64-sysv pass an 8-byte integer in the same register class
    // as an 8-byte pointer for the ccc convention. On macOS pthread
    // is part of libSystem (linked by default); Linux callers need
    // `[link] libs = ["pthread"]` in their manifest (Phase 5C).
    out.push_str("declare i32 @pthread_create(ptr, ptr, ptr, ptr)\n");
    out.push_str("declare i32 @pthread_join(i64, ptr)\n");
    // v0.0.3 Phase 5 Slice 5E.3: LLVM coroutine intrinsics for the
    // `async fn` lowering. The `presplitcoroutine` function attribute
    // on each async fn triggers LLVM's CoroSplit pass during the
    // standard middle-end pipeline (also runs at -O0 because the
    // intrinsics are illegal in finalized IR — the pass *must* run).
    // We use the "promise" pattern for the per-coroutine result slot
    // so async fns returning T ≤ 8 bytes stash their result at a
    // known offset in the frame, retrievable by the poll loop.
    out.push_str("declare token @llvm.coro.id(i32, ptr, ptr, ptr)\n");
    out.push_str("declare ptr @llvm.coro.begin(token, ptr)\n");
    out.push_str("declare i64 @llvm.coro.size.i64()\n");
    out.push_str("declare i8 @llvm.coro.suspend(token, i1)\n");
    // `llvm.coro.end`'s return type differs by LLVM version: older LLVM (and
    // Apple clang 21) declare it returning `i1`; LLVM ~22+ changed it to
    // `void`, and each version's verifier rejects the other with "Intrinsic
    // has incorrect return type!". `cpc` probes the target clang and installs
    // the right form via `set_coro_end_returns_void` before codegen runs (see
    // `coro_end_call_ir`). The result value is never used either way.
    out.push_str(&coro_end_decl_ir());
    out.push_str("declare ptr @llvm.coro.free(token, ptr)\n");
    out.push_str("declare i1 @llvm.coro.done(ptr)\n");
    out.push_str("declare void @llvm.coro.resume(ptr)\n");
    out.push_str("declare void @llvm.coro.destroy(ptr)\n");
    out.push_str("declare ptr @llvm.coro.promise(ptr, i32, i1)\n");
    // pthread_detach is called from user code (stdlib/thread's Drop
    // impl) via `extern fn`, so its declare is emitted by the normal
    // extern-fn path. No preamble entry needed; emitting one would
    // collide.
    //
    // v0.0.4 Phase 3 Slice 3A.1: async reactor state. A single
    // process-global mutable pointer slot, plus tiny getter/setter
    // helpers. The actual reactor state struct (kqueue fd, waiter
    // arrays, pending-task queue) lives at the address this slot
    // points to and is allocated/managed by stdlib/reactor.cplus.
    // We emit the global + accessors here so that C+ source can
    // remain global-free; stdlib calls these as plain extern fns.
    out.push_str("\n; v0.0.4 Phase 3 Slice 3A.1: reactor state slot.\n");
    out.push_str("@__cplus_reactor_state = internal global ptr null, align 8\n");
    out.push_str(
        "define internal ptr @__cplus_reactor_get_state() {\n  \
         %p = load ptr, ptr @__cplus_reactor_state, align 8\n  \
         ret ptr %p\n\
         }\n",
    );
    out.push_str(
        "define internal void @__cplus_reactor_set_state(ptr %p) {\n  \
         store ptr %p, ptr @__cplus_reactor_state, align 8\n  \
         ret void\n\
         }\n",
    );
    // FFI-callable wrappers around the `llvm.coro.*` intrinsics so
    // stdlib/reactor.cplus can call them as plain extern fns. The
    // intrinsics themselves aren't FFI-callable directly (the LLVM
    // verifier rejects calls to them from non-coroutine functions
    // unless wrapped).
    out.push_str(
        "define internal void @__cplus_coro_resume(ptr %h) {\n  \
         call void @llvm.coro.resume(ptr %h)\n  \
         ret void\n\
         }\n",
    );
    out.push_str(
        "define internal i32 @__cplus_coro_done(ptr %h) {\n  \
         %d = call i1 @llvm.coro.done(ptr %h)\n  \
         %r = zext i1 %d to i32\n  \
         ret i32 %r\n\
         }\n",
    );
    // Checked-arithmetic intrinsics used in debug mode for signed integers
    // of every supported width. Always declared; LLVM drops unused ones.
    for op in ["sadd", "ssub", "smul"] {
        for bits in [8, 16, 32, 64] {
            out.push_str(&format!(
                "declare {{i{bits}, i1}} @llvm.{op}.with.overflow.i{bits}(i{bits}, i{bits})\n"
            ));
        }
    }
    out.push_str("\n");
}

/// Phase 8 slice 8.STR.1: walk the program, find every `ExprKind::StrLit`,
/// dedupe by content, and emit one `@.str.N = private unnamed_addr constant`
/// per unique literal. Returns a map from literal payload → (symbol, len).
/// `len` is the visible byte length, NOT counting the NUL terminator.
/// The NUL is appended in the IR so `to_cstring()` (slice 8.STR.4) can
/// hand the same buffer to C with no copy.
fn collect_and_emit_str_lits(out: &mut String, program: &Program) -> StrLitTable {
    let mut table: StrLitTable = HashMap::new();
    let mut next_id: u32 = 0;
    fn emit_str_global(s: &str, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        if table.contains_key(s) {
            return;
        }
        let symbol = format!("@.str.{}", *next_id);
        *next_id += 1;
        let len = s.len();
        let total = len + 1;
        let mut escaped = String::new();
        for byte in s.bytes() {
            if byte == b'"' || byte == b'\\' || !(0x20..0x7F).contains(&byte) {
                escaped.push_str(&format!("\\{byte:02X}"));
            } else {
                escaped.push(byte as char);
            }
        }
        escaped.push_str("\\00");
        out.push_str(&format!(
            "{symbol} = private unnamed_addr constant [{total} x i8] c\"{escaped}\", align 1\n"
        ));
        table.insert(s.to_string(), (symbol, len));
    }
    fn walk_expr(e: &Expr, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        match &e.kind {
            ExprKind::StrLit(s) => {
                emit_str_global(s, table, next_id, out);
            }
            ExprKind::CStrLit(s) => {
                // Reuse the str-lit machinery: the globals are already
                // NUL-terminated, so a c-string and a str of the same content
                // share one `@.str.N`. Only the use site differs (ptr vs
                // fat pointer).
                emit_str_global(s, table, next_id, out);
            }
            ExprKind::InterpStr { parts } => {
                // Phase 8 slice 8.STR.B: each Lit segment gets the same
                // @.str.N treatment as a plain StrLit so codegen at the
                // use site can reuse the existing fat-pointer machinery.
                for p in parts {
                    match p {
                        crate::ast::InterpStrPart::Lit(s) => {
                            emit_str_global(s, table, next_id, out)
                        }
                        crate::ast::InterpStrPart::Expr(e) => walk_expr(e, table, next_id, out),
                    }
                }
            }
            ExprKind::Block(b) => walk_block(b, table, next_id, out),
            ExprKind::Unsafe(b) => walk_block(b, table, next_id, out),
            ExprKind::Await(inner) => walk_expr(inner, table, next_id, out),
            ExprKind::Yield(inner) => walk_expr(inner, table, next_id, out),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                walk_expr(cond, table, next_id, out);
                walk_block(then, table, next_id, out);
                if let Some(eb) = else_branch {
                    walk_expr(eb, table, next_id, out);
                }
            }
            ExprKind::Call { callee, args, .. } => {
                walk_expr(callee, table, next_id, out);
                for a in args {
                    walk_expr(a, table, next_id, out);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                walk_expr(lhs, table, next_id, out);
                walk_expr(rhs, table, next_id, out);
            }
            ExprKind::Unary { operand, .. } => walk_expr(operand, table, next_id, out),
            ExprKind::Field { receiver, .. } => walk_expr(receiver, table, next_id, out),
            ExprKind::Index { receiver, index } => {
                walk_expr(receiver, table, next_id, out);
                walk_expr(index, table, next_id, out);
            }
            ExprKind::Assign { target, value, .. } => {
                walk_expr(target, table, next_id, out);
                walk_expr(value, table, next_id, out);
            }
            ExprKind::Cast { expr: inner, .. } => walk_expr(inner, table, next_id, out),
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    walk_expr(s, table, next_id, out);
                }
                if let Some(e) = end {
                    walk_expr(e, table, next_id, out);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                walk_expr(scrutinee, table, next_id, out);
                for a in arms {
                    walk_expr(&a.body, table, next_id, out);
                }
            }
            ExprKind::StructLit { fields, .. } | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    walk_expr(&f.value, table, next_id, out);
                }
            }
            ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
                for e in elements {
                    walk_expr(e, table, next_id, out);
                }
            }
            // v0.0.10 Phase 4: `#name(args...)` intrinsics may carry
            // arbitrary expressions in their arg list (e.g. `#msg_send`
            // forwards user args; `#compile_shader` takes path/target
            // string literals). Walk the args so any string literal in
            // there gets emitted to the @.str.N table.
            ExprKind::Intrinsic { args, .. } => {
                for a in args {
                    walk_expr(a, table, next_id, out);
                }
            }
            _ => {}
        }
    }
    fn walk_block(b: &Block, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        for s in &b.stmts {
            walk_stmt(s, table, next_id, out);
        }
        if let Some(t) = &b.tail {
            walk_expr(t, table, next_id, out);
        }
    }
    fn walk_stmt(s: &Stmt, table: &mut StrLitTable, next_id: &mut u32, out: &mut String) {
        match &s.kind {
            StmtKind::Let { init, .. } => {
                if let Some(e) = init {
                    walk_expr(e, table, next_id, out);
                }
            }
            StmtKind::Expr(e) | StmtKind::Assert(e) => walk_expr(e, table, next_id, out),
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    walk_expr(e, table, next_id, out);
                }
            }
            StmtKind::While { cond, body, .. } => {
                walk_expr(cond, table, next_id, out);
                walk_block(body, table, next_id, out);
            }
            StmtKind::For(forloop, _) => match forloop {
                crate::ast::ForLoop::Range { iter, body, .. } => {
                    walk_expr(iter, table, next_id, out);
                    walk_block(body, table, next_id, out);
                }
                crate::ast::ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(s) = init {
                        walk_stmt(s, table, next_id, out);
                    }
                    if let Some(c) = cond {
                        walk_expr(c, table, next_id, out);
                    }
                    for u in update {
                        walk_expr(u, table, next_id, out);
                    }
                    walk_block(body, table, next_id, out);
                }
            },
            StmtKind::Defer(e) => walk_expr(e, table, next_id, out),
            // Phase 3B follow-up (2026-05-15): plain `loop { ... }` blocks
            // were silently skipped by the str-literal pre-pass, so any
            // literal inside a `loop` body tripped a codegen `expect`. Walk
            // the body the same way as `while` / `for`.
            StmtKind::Loop(body, _) => walk_block(body, table, next_id, out),
            _ => {}
        }
    }
    for item in &program.items {
        match &item.kind {
            ItemKind::Function(f) if f.generic_params.is_empty() => {
                walk_block(&f.body, &mut table, &mut next_id, out);
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    if m.generic_params.is_empty() {
                        walk_block(&m.body, &mut table, &mut next_id, out);
                    }
                }
            }
            _ => {}
        }
    }
    out.push_str("\n");
    table
}

/// v0.0.8 bench-gap fix C: walk the program collecting names of free
/// functions whose address is taken anywhere (any `Ident(name)` outside
/// a `Call` callee position where `name` resolves to a fn). Used to
/// gate the `fastcc` calling convention — a function with `fastcc` cc
/// can't be safely called through a C-cc fn pointer, so any function
/// whose `&f` form appears in source must keep the default C cc.
///
/// "Free fn" here means a `sigs`-registered top-level function. Methods
/// (looked up via type + name) aren't take-the-address-able in C+ —
/// there's no `&T::method` syntax — so they're never added to the set.
fn collect_address_taken_fns(
    program: &Program,
    sigs: &HashMap<String, FnSig>,
) -> HashSet<String> {
    fn visit_expr(e: &Expr, sigs: &HashMap<String, FnSig>, taken: &mut HashSet<String>) {
        match &e.kind {
            ExprKind::Ident(name) => {
                if sigs.contains_key(name) {
                    taken.insert(name.clone());
                }
            }
            ExprKind::Call { callee, args, .. } => {
                // Direct call: `f(...)` where callee is a bare Ident is
                // NOT address-taking — that's the only case where an
                // Ident referring to a fn name is safe to skip. Any
                // other callee shape (parenthesized, indirect via a
                // local fn-ptr, etc.) gets recursed normally.
                if !matches!(callee.kind, ExprKind::Ident(_)) {
                    visit_expr(callee, sigs, taken);
                }
                for a in args {
                    visit_expr(a, sigs, taken);
                }
            }
            ExprKind::Block(b) | ExprKind::Unsafe(b) => visit_block(b, sigs, taken),
            ExprKind::Await(inner) | ExprKind::Yield(inner) => visit_expr(inner, sigs, taken),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                visit_expr(cond, sigs, taken);
                visit_block(then, sigs, taken);
                if let Some(eb) = else_branch {
                    visit_expr(eb, sigs, taken);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                visit_expr(lhs, sigs, taken);
                visit_expr(rhs, sigs, taken);
            }
            ExprKind::Unary { operand, .. } => visit_expr(operand, sigs, taken),
            ExprKind::Field { receiver, .. } => visit_expr(receiver, sigs, taken),
            ExprKind::Index { receiver, index } => {
                visit_expr(receiver, sigs, taken);
                visit_expr(index, sigs, taken);
            }
            ExprKind::Assign { target, value, .. } => {
                visit_expr(target, sigs, taken);
                visit_expr(value, sigs, taken);
            }
            ExprKind::Cast { expr: inner, .. } => visit_expr(inner, sigs, taken),
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    visit_expr(s, sigs, taken);
                }
                if let Some(e) = end {
                    visit_expr(e, sigs, taken);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                visit_expr(scrutinee, sigs, taken);
                for a in arms {
                    visit_expr(&a.body, sigs, taken);
                }
            }
            ExprKind::StructLit { fields, .. } | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    visit_expr(&f.value, sigs, taken);
                }
            }
            ExprKind::ArrayLit { elements }
            | ExprKind::TupleLit { elements }
            | ExprKind::GenericEnumCall { args: elements, .. } => {
                for e in elements {
                    visit_expr(e, sigs, taken);
                }
            }
            ExprKind::ArrayFill { fill, .. } => visit_expr(fill, sigs, taken),
            ExprKind::InterpStr { parts } => {
                for p in parts {
                    if let crate::ast::InterpStrPart::Expr(e) = p {
                        visit_expr(e, sigs, taken);
                    }
                }
            }
            // Leaves: literals, paths (Type::variant — not a free-fn
            // address), include_bytes/str compiler builtins.
            ExprKind::IntLit(_, _)
            | ExprKind::FloatLit(_, _)
            | ExprKind::BoolLit(_)
            | ExprKind::StrLit(_)
            | ExprKind::CStrLit(_)
            | ExprKind::Path { .. }
            | ExprKind::IncludeBytes { .. }
            | ExprKind::IncludeStr { .. }
            | ExprKind::EnvVar { .. } => {}
            ExprKind::Intrinsic { args, .. } => {
                for a in args {
                    visit_expr(a, sigs, taken);
                }
            }
            ExprKind::Asm { operands, .. } => {
                for op in operands {
                    visit_expr(&op.value, sigs, taken);
                }
            }
        }
    }
    fn visit_block(b: &Block, sigs: &HashMap<String, FnSig>, taken: &mut HashSet<String>) {
        for s in &b.stmts {
            visit_stmt(s, sigs, taken);
        }
        if let Some(t) = &b.tail {
            visit_expr(t, sigs, taken);
        }
    }
    fn visit_stmt(s: &Stmt, sigs: &HashMap<String, FnSig>, taken: &mut HashSet<String>) {
        match &s.kind {
            StmtKind::Let { init, .. } => {
                if let Some(e) = init {
                    visit_expr(e, sigs, taken);
                }
            }
            StmtKind::Expr(e) | StmtKind::Assert(e) | StmtKind::Defer(e) => {
                visit_expr(e, sigs, taken);
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    visit_expr(e, sigs, taken);
                }
            }
            StmtKind::While { cond, body, .. } => {
                visit_expr(cond, sigs, taken);
                visit_block(body, sigs, taken);
            }
            StmtKind::For(forloop, _) => match forloop {
                crate::ast::ForLoop::Range { iter, body, .. } => {
                    visit_expr(iter, sigs, taken);
                    visit_block(body, sigs, taken);
                }
                crate::ast::ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(s) = init {
                        visit_stmt(s, sigs, taken);
                    }
                    if let Some(c) = cond {
                        visit_expr(c, sigs, taken);
                    }
                    for u in update {
                        visit_expr(u, sigs, taken);
                    }
                    visit_block(body, sigs, taken);
                }
            },
            StmtKind::Loop(body, _) => visit_block(body, sigs, taken),
            _ => {}
        }
    }
    let mut taken: HashSet<String> = HashSet::new();
    for item in &program.items {
        match &item.kind {
            ItemKind::Function(f) if f.generic_params.is_empty() => {
                visit_block(&f.body, sigs, &mut taken);
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    if m.generic_params.is_empty() {
                        visit_block(&m.body, sigs, &mut taken);
                    }
                }
            }
            _ => {}
        }
    }
    taken
}

/// v0.0.6 Slice 1A / v0.0.7 Slice 3.1: emit one `@.bytes.N` global
/// per unique resolved path in sema's compile-time-blobs map and
/// populate `md.compile_time_blobs` with the per-call-site lookup.
///
/// Dedup is by canonicalized absolute path — sema's
/// `CompileTimeBlobEntry::abs_path` is the dedup key — so two
/// `include_bytes!("foo.bin")` calls (or one bytes + one str on the
/// same path) share one underlying `[N x i8]` global. The AST node
/// variant downstream decides whether to lower the call to a raw
/// pointer or to a `str` fat-pointer aggregate.
fn emit_compile_time_blob_globals(
    out: &mut String,
    map: &HashMap<crate::lexer::Span, crate::sema::CompileTimeBlobEntry>,
    md: &ModuleMetadata,
) {
    let mut path_to_sym: HashMap<std::path::PathBuf, (String, u32)> = HashMap::new();
    let mut next_id: u32 = 0;
    // Iterate in span order for stable output.
    let mut spans: Vec<&crate::lexer::Span> = map.keys().collect();
    spans.sort_by_key(|s| (s.start, s.end));
    for span in spans {
        let entry = &map[span];
        let (symbol, len) = match path_to_sym.get(&entry.abs_path) {
            Some(v) => v.clone(),
            None => {
                let symbol = format!("@.bytes.{}", next_id);
                next_id += 1;
                let len = entry.bytes.len() as u32;
                let mut escaped = String::new();
                for byte in &entry.bytes {
                    if *byte == b'"' || *byte == b'\\' || !(0x20..0x7F).contains(byte) {
                        escaped.push_str(&format!("\\{byte:02X}"));
                    } else {
                        escaped.push(*byte as char);
                    }
                }
                out.push_str(&format!(
                    "{symbol} = private unnamed_addr constant [{len} x i8] c\"{escaped}\", align 1\n"
                ));
                let v = (symbol, len);
                path_to_sym.insert(entry.abs_path.clone(), v.clone());
                v
            }
        };
        md.compile_time_blobs.borrow_mut().insert(*span, (symbol, len));
    }
    if !map.is_empty() {
        out.push_str("\n");
    }
}

/// v0.0.8 Phase 4: emit one `@.envvar.N` private constant per UNIQUE
/// env-var VALUE (two `env!("X")` calls always resolve to the same
/// value at sema time, so dedup-by-value is equivalent to dedup-by-name
/// — using value as the key is slightly cheaper and supports the
/// hypothetical case of two different vars holding the same string).
/// Populates `md.env_var_globals` so gen_expr for `ExprKind::EnvVar`
/// can build the `str` fat-pointer aggregate at the use site.
fn emit_env_var_globals(
    out: &mut String,
    map: &HashMap<crate::lexer::Span, crate::sema::EnvVarEntry>,
    md: &ModuleMetadata,
) {
    let mut value_to_sym: HashMap<String, (String, u32)> = HashMap::new();
    let mut next_id: u32 = 0;
    let mut spans: Vec<&crate::lexer::Span> = map.keys().collect();
    spans.sort_by_key(|s| (s.start, s.end));
    for span in spans {
        let entry = &map[span];
        let (symbol, len) = match value_to_sym.get(&entry.value) {
            Some(v) => v.clone(),
            None => {
                let symbol = format!("@.envvar.{}", next_id);
                next_id += 1;
                let len = entry.value.as_bytes().len() as u32;
                let mut escaped = String::new();
                for byte in entry.value.as_bytes() {
                    if *byte == b'"' || *byte == b'\\' || !(0x20..0x7F).contains(byte) {
                        escaped.push_str(&format!("\\{byte:02X}"));
                    } else {
                        escaped.push(*byte as char);
                    }
                }
                out.push_str(&format!(
                    "{symbol} = private unnamed_addr constant [{len} x i8] c\"{escaped}\", align 1\n"
                ));
                let v = (symbol, len);
                value_to_sym.insert(entry.value.clone(), v.clone());
                v
            }
        };
        md.env_var_globals.borrow_mut().insert(*span, (symbol, len));
    }
    if !map.is_empty() {
        out.push_str("\n");
    }
}

/// v0.0.10 Phase 4A: emit one cached-pointer global pair per unique
/// ObjC selector used by `#selector(...)` / `#msg_send(...)`. Layout per
/// selector "name":
///   - `@__cplus.sel.<n>.data   = private constant [L x i8] c"name\00"`
///     (NUL-terminated — `sel_registerName` is a C-string API).
///   - `@__cplus.sel.<n>.cached = private global ptr null` (the SEL
///     pointer, lazily filled by the first `#selector` call to that name).
///
/// Populates `md.selector_globals` so `gen_intrinsic` for `#selector`
/// and `#msg_send` can emit the load+branch+register pattern with the
/// right symbol names. Walks `selectors_set` in sorted order for
/// deterministic IR output.
fn emit_selector_globals(
    out: &mut String,
    selectors_set: &std::collections::BTreeSet<String>,
    md: &ModuleMetadata,
) {
    if selectors_set.is_empty() {
        return;
    }
    let mut globals = md.selector_globals.borrow_mut();
    for (idx, name) in selectors_set.iter().enumerate() {
        let data_sym = format!("@__cplus.sel.{idx}.data");
        let cached_sym = format!("@__cplus.sel.{idx}.cached");
        let payload_len = (name.as_bytes().len() + 1) as u32; // +1 for NUL
        let mut escaped = String::new();
        for byte in name.as_bytes() {
            if *byte == b'"' || *byte == b'\\' || !(0x20..0x7F).contains(byte) {
                escaped.push_str(&format!("\\{byte:02X}"));
            } else {
                escaped.push(*byte as char);
            }
        }
        escaped.push_str("\\00");
        out.push_str(&format!(
            "{data_sym} = private unnamed_addr constant [{payload_len} x i8] c\"{escaped}\", align 1\n"
        ));
        out.push_str(&format!(
            "{cached_sym} = private global ptr null, align 8\n"
        ));
        globals.insert(name.clone(), (data_sym, cached_sym, payload_len));
    }
    out.push_str("\n");
}

/// v0.0.10 Phase 4C: emit one private constant `[N x i8]` global per
/// `#compile_shader(...)` call site. Bytes already produced at sema time
/// (sema invoked `xcrun ... metallib` and stored the result in
/// `MonoInfo::shader_blobs`). Symbol naming: `@.shader.N`.
fn emit_shader_blob_globals(
    out: &mut String,
    map: &HashMap<crate::lexer::Span, Vec<u8>>,
    md: &ModuleMetadata,
) {
    if map.is_empty() {
        return;
    }
    let mut spans: Vec<&crate::lexer::Span> = map.keys().collect();
    spans.sort_by_key(|s| (s.start, s.end));
    let mut globals = md.shader_blob_globals.borrow_mut();
    for (idx, span) in spans.iter().enumerate() {
        let bytes = &map[span];
        let len = bytes.len() as u32;
        let symbol = format!("@.shader.{idx}");
        let mut escaped = String::new();
        for byte in bytes {
            if *byte == b'"' || *byte == b'\\' || !(0x20..0x7F).contains(byte) {
                escaped.push_str(&format!("\\{byte:02X}"));
            } else {
                escaped.push(*byte as char);
            }
        }
        out.push_str(&format!(
            "{symbol} = private unnamed_addr constant [{len} x i8] c\"{escaped}\", align 1\n"
        ));
        globals.insert(**span, (symbol, len));
    }
    out.push_str("\n");
}

/// v0.0.9 Phase 4: emit one LLVM global per module-scope `static`.
/// Iterates `statics_map` in sorted-name order for deterministic output.
/// For each entry, renders the literal initializer (already validated
/// by `lower::is_const_initializer` and type-checked by sema) into an
/// LLVM constant operand and writes:
///
///   - `@NAME = constant <ty> <lit>` when `info.is_mut == false`
///   - `@NAME = global   <ty> <lit>` when `info.is_mut == true`
///
/// `str`-typed statics are rejected with E0X35 (panic here since sema
/// should have caught) — string-fat-pointer initialization requires
/// two globals and v0.0.9 punts that; use `const FOO: str = "..."`
/// instead (which the lower pass substitutes literally).
///
/// Populates `md.statics` with the qualified-name → `Ty` map so
/// `gen_expr` / `gen_assign` can detect references to statics and
/// route them through load/store ops against the emitted symbol.
fn emit_statics(
    out: &mut String,
    statics_map: &std::collections::BTreeMap<String, crate::sema::StaticInfo>,
    types: &TypeTable,
    md: &ModuleMetadata,
) {
    if statics_map.is_empty() {
        return;
    }
    for (qname, info) in statics_map {
        // v0.0.9 follow-up: `str`-typed statics need a paired data
        // global. Emit `@<name>.bytes = constant [N x i8] c"..."` for
        // the payload and `@<name> = {constant|global} { ptr, i64 } { ptr @<name>.bytes, i64 N }`
        // for the fat-pointer header. Reads through gen_expr's Ident
        // path then load `{ptr, i64}` from `@<name>` as any str does.
        if matches!(info.ty, Ty::Str) {
            if let ExprKind::StrLit(s) = &info.init.kind {
                let bytes_sym = format!("{qname}.bytes");
                let bytes_len = emit_cstr(out, &bytes_sym, s);
                let str_len = bytes_len.saturating_sub(1); // emit_cstr adds NUL terminator
                let storage = if info.is_mut { "global" } else { "constant" };
                out.push_str(&format!(
                    "@{qname} = {storage} {{ ptr, i64 }} {{ ptr @{bytes_sym}, i64 {str_len} }}\n"
                ));
                md.statics.borrow_mut().insert(qname.clone(), info.ty.clone());
                continue;
            }
        }
        let lltype = llvm_ty(&info.ty, types);
        let llvalue = match render_static_literal(&info.init, &info.ty, types) {
            Some(s) => s,
            None => {
                // Defense-in-depth: lower + sema should have rejected
                // any non-literal initializer before reaching codegen.
                // Reaching this means a pass regression — emit a
                // poisoned global so the IR fails to assemble loudly
                // rather than silently miscompiling.
                "poison".to_string()
            }
        };
        let storage = if info.is_mut { "global" } else { "constant" };
        out.push_str(&format!("@{qname} = {storage} {lltype} {llvalue}\n"));
        md.statics.borrow_mut().insert(qname.clone(), info.ty.clone());
    }
    out.push('\n');
}

/// Render an AST literal as an LLVM constant operand suitable for a
/// global initializer. Mirrors the literal-shape acceptance rule from
/// `lower::is_const_initializer` plus the float-bit-pattern emission
/// from `gen_expr(ExprKind::FloatLit)`. Returns `None` on any unsupported
/// shape — the caller emits a `poison` operand in that case so the
/// failure surfaces at LLVM-assembly time rather than silently.
fn render_static_literal(e: &Expr, ty: &Ty, types: &TypeTable) -> Option<String> {
    use crate::lexer::NumSuffix;
    match &e.kind {
        // v0.0.12 G-043 (llama.cplus): array literal / fill as a static
        // initializer → LLVM constant aggregate `[N x T] [T v0, T v1, ...]`.
        // Each element is rendered with the *declared* element type (`elem`),
        // so bare int literals coerce to it (the static-position analog of
        // G-044). A zero fill collapses to `zeroinitializer` to keep large
        // tables out of the textual IR.
        ExprKind::ArrayLit { elements } => {
            let Ty::Array(elem, n) = ty else { return None; };
            if elements.len() as u64 != *n as u64 {
                return None;
            }
            let elem_ll = llvm_ty(elem, types);
            let mut parts: Vec<String> = Vec::with_capacity(elements.len());
            for el in elements {
                let v = render_static_literal(el, elem, types)?;
                parts.push(format!("{elem_ll} {v}"));
            }
            Some(format!("[{}]", parts.join(", ")))
        }
        ExprKind::ArrayFill { fill, count, .. } => {
            let Ty::Array(elem, n) = ty else { return None; };
            if *count as u64 != *n as u64 {
                return None;
            }
            let v = render_static_literal(fill, elem, types)?;
            if v == "0" || v == "0x0000000000000000" {
                return Some("zeroinitializer".to_string());
            }
            let elem_ll = llvm_ty(elem, types);
            let parts: Vec<String> = (0..*count).map(|_| format!("{elem_ll} {v}")).collect();
            Some(format!("[{}]", parts.join(", ")))
        }
        // v0.0.13 (G-043 second half): struct literal as a static initializer →
        // LLVM constant struct `%Name { T0 v0, T1 v1, ... }`. Fields are emitted
        // in the struct's *declared* order (matching the `%Name = type { ... }`
        // layout in `write_struct_decls`), so source field order is irrelevant.
        // Each field value is rendered with its declared field type, so a bare
        // `255` in an `f32` field and a `0` in a `bool` field coerce correctly,
        // and nested structs / arrays recurse. The ggml `sphere_t scene[]` case.
        ExprKind::StructLit { fields, .. } => {
            let Ty::Struct(id) = ty else { return None; };
            let info = &types.struct_defs[id.0 as usize];
            let mut parts: Vec<String> = Vec::with_capacity(info.fields.len());
            for (fname, fty) in &info.fields {
                let lit = fields.iter().find(|f| &f.name.name == fname)?;
                let v = render_static_literal(&lit.value, fty, types)?;
                let fty_ll = llvm_ty(fty, types);
                parts.push(format!("{fty_ll} {v}"));
            }
            Some(format!("{{ {} }}", parts.join(", ")))
        }
        ExprKind::IntLit(v, _) => Some(v.to_string()),
        ExprKind::BoolLit(b) => Some(if *b { "true".to_string() } else { "false".to_string() }),
        ExprKind::FloatLit(v, suf) => render_static_float(*v, *suf, ty),
        ExprKind::Unary { op: UnaryOp::Neg, operand } => match &operand.kind {
            ExprKind::IntLit(v, _) => Some(format!("-{v}")),
            ExprKind::FloatLit(v, suf) => render_static_float(-*v, *suf, ty),
            _ => None,
        },
        // str-typed statics need a paired data global; v0.0.9 punts.
        // Users should declare these as `const FOO: str = "..."` which
        // lower-substitutes the literal at every use site (no global
        // needed).
        ExprKind::StrLit(_) => None,
        // v0.0.12 G-033 (llama.cplus G-032): `#zero::[T]()` initializer.
        // LLVM's `zeroinitializer` lands the global in BSS (`.bss` /
        // Mach-O `__DATA,__bss`) with no runtime cost — same as C's
        // `static T name = {0};` / `static T name;`. Closes the
        // lookup-table and BSS-zero-struct cases the llama port hit
        // when porting `ggml_cpu_init`-owned globals into cpc.
        ExprKind::Intrinsic { name, args, type_args, .. }
            if name == "zero" && args.is_empty() && type_args.len() == 1 =>
        {
            Some("zeroinitializer".to_string())
        }
        _ => None,
    }
}

/// Render a float literal as an LLVM constant operand for a static initializer,
/// choosing the bit width from the *target* type (`ty`) first, then the
/// literal's own suffix. LLVM writes `float`/`double` constants as a hex form
/// of the value reinterpreted at `double` width (the 2026-05-17 raytracer fix:
/// decimal float forms don't round-trip), and `half` constants with the
/// 16-bit `0xH....` form. Using the field/element type means a bare `1.5` in an
/// `f32` array or struct field renders as the f32 value, not an f64 one.
fn render_static_float(v: f64, suf: crate::lexer::NumSuffix, ty: &Ty) -> Option<String> {
    use crate::lexer::NumSuffix;
    match ty {
        Ty::F16 => Some(format!("0xH{:04X}", f64_to_f16_bits(v))),
        Ty::F32 => Some(format!("0x{:016X}", (v as f32 as f64).to_bits())),
        Ty::F64 => Some(format!("0x{:016X}", v.to_bits())),
        // No declared float type to key off (e.g. the literal is the whole
        // static `static X: f32 = 1.5f32;` and `ty` is the scalar) — fall back
        // to the literal's suffix, matching the historical behavior.
        _ => {
            let bits = match suf {
                NumSuffix::F32 => (v as f32 as f64).to_bits(),
                _ => v.to_bits(),
            };
            Some(format!("0x{bits:016X}"))
        }
    }
}

/// Convert an `f64` to its IEEE-754 binary16 (`half`) bit pattern with
/// round-to-nearest-even, for emitting `half 0xH....` constants in static
/// initializers. Handles zero, subnormals, overflow-to-infinity, and NaN.
/// Mirrors the value `fptrunc double ... to half` would produce at runtime, so
/// a `static`-position `1.5f16` matches a runtime one.
fn f64_to_f16_bits(v: f64) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 48) & 0x8000) as u16;
    let exp = ((bits >> 52) & 0x7ff) as i64; // biased 11-bit exponent
    let mant = bits & 0x000f_ffff_ffff_ffff; // 52-bit mantissa

    if exp == 0x7ff {
        // Inf / NaN. Preserve NaN-ness with a non-zero payload.
        let nan = if mant != 0 { 0x0200 } else { 0 };
        return sign | 0x7c00 | nan;
    }
    // Unbias (1023) and rebias to half (15).
    let unbiased = exp - 1023;
    let half_exp = unbiased + 15;

    if half_exp >= 0x1f {
        // Overflow → infinity.
        return sign | 0x7c00;
    }
    if half_exp <= 0 {
        // Subnormal or underflow to zero. Build the full significand
        // (implicit leading 1 for normals) and shift it down into the
        // subnormal range, rounding to nearest even.
        // Build the full 53-bit significand (implicit leading 1) and shift it
        // into the half-subnormal grid, where the stored mantissa M satisfies
        // value == M * 2^-24. From value == significand * 2^(unbiased-52) and
        // unbiased == half_exp-15, that gives M == significand >> (43-half_exp).
        let significand = (1u64 << 52) | mant; // 53-bit, leading 1 explicit
        let shift = (43 - half_exp) as u32;
        let half_mant = round_shift(significand, shift);
        // Rounding can carry M up to 0x400, which is exactly the smallest
        // normal — `sign | 0x400` encodes that (exp field 1, mantissa 0).
        return sign | (half_mant as u16);
    }
    // Normal: keep the top 10 mantissa bits, round the dropped 42.
    let half_mant = round_shift(mant, 42);
    // Rounding the mantissa can carry into the exponent; the +half_mant
    // overflow naturally bumps the exponent field since they're adjacent.
    sign | ((half_exp as u16) << 10) | (half_mant as u16)
}

/// Right-shift `value` by `shift` bits with round-to-nearest, ties-to-even.
fn round_shift(value: u64, shift: u32) -> u64 {
    if shift == 0 {
        return value;
    }
    if shift >= 64 {
        return 0;
    }
    let dropped = value & ((1u64 << shift) - 1);
    let kept = value >> shift;
    let halfway = 1u64 << (shift - 1);
    if dropped > halfway || (dropped == halfway && (kept & 1) == 1) {
        kept + 1
    } else {
        kept
    }
}

/// Escape an inline-asm template for an LLVM `asm` string operand.
///
/// Two layers stack here. The outer one is LLVM IR string-constant escaping
/// (same convention as [`emit_cstr`]): `"`, `\`, and any non-printable byte
/// become `\XX` hex; printable ASCII passes through. The inner one is
/// inline-asm specific: `$` is LLVM's operand sigil, so a *literal* dollar in
/// the final assembly must be `$$`. Since `$` is printable it survives the IR
/// layer untouched, so we double it directly. (Tier 1 has no operands, but a
/// template may still legitimately contain a `$`.)
fn escape_asm_template(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'$' => out.push_str("$$"),
            b'"' | b'\\' => out.push_str(&format!("\\{byte:02X}")),
            0x20..=0x7E => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:02X}")),
        }
    }
    out
}

/// v0.0.15: escape a string for an LLVM module-level `module asm "..."`
/// directive. LLVM IR string literals escape `"`, `\`, and every
/// non-printable byte as `\NN` (two hex digits); other ASCII is verbatim.
/// Unlike `escape_asm_template`, `$` is *not* doubled — module asm performs
/// no operand substitution, so `$` is an ordinary character.
fn escape_llvm_str(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'"' | b'\\' => out.push_str(&format!("\\{byte:02X}")),
            0x20..=0x7E => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:02X}")),
        }
    }
    out
}

/// Emit a `private unnamed_addr constant` LLVM string literal with a NUL
/// terminator. Used by both `println` (the existing `@.fmt_int_nl`) and the
/// slice 5ATTR.4 test driver. Returns the byte length including the null
/// terminator (the `[N x i8]` length in the declaration).
fn emit_cstr(out: &mut String, name: &str, s: &str) -> usize {
    let mut escaped = String::new();
    let mut len: usize = 0;
    for byte in s.bytes() {
        if byte == b'"' || byte == b'\\' || !(0x20..0x7F).contains(&byte) {
            escaped.push_str(&format!("\\{byte:02X}"));
        } else {
            escaped.push(byte as char);
        }
        len += 1;
    }
    escaped.push_str("\\00");
    len += 1;
    out.push_str(&format!(
        "@{name} = private unnamed_addr constant [{len} x i8] c\"{escaped}\", align 1\n"
    ));
    len
}

/// Slice 5ATTR.4 — emit the synthesized test-driver `main`. Called only when
/// `generate_test_binary` is the entry point; the user's own `main` is
/// suppressed in `generate_inner` so this one is the linker's choice.
///
/// IR shape (per test, in source order):
///   - clear `@cpc_test_failed`
///   - call the test fn
///   - for `fn() -> i32` tests, fold the return into the failure check
///   - print one pass/fail line (human or JSON per `json`)
///   - bump a local pass/fail counter
/// Final block prints the summary and returns the fail count as the process
/// exit status (so `cpc test` can short-circuit on any failure).
fn emit_test_driver_main(out: &mut String, tests: &[crate::attrs::TestFn], json: bool) {
    out.push('\n');
    // Format strings. Use distinct names per mode to keep the IR readable.
    let (pass_fmt, fail_fmt, summary_fmt) = if json {
        (
            "{\"name\":\"%s\",\"result\":\"pass\"}\n",
            "{\"name\":\"%s\",\"result\":\"fail\"}\n",
            "{\"passed\":%d,\"failed\":%d}\n",
        )
    } else {
        (
            "test %s ... ok\n",
            "test %s ... FAILED\n",
            "\ntest result: %d passed; %d failed\n",
        )
    };
    let pass_fmt_len = emit_cstr(out, ".fmt_test_pass", pass_fmt);
    let fail_fmt_len = emit_cstr(out, ".fmt_test_fail", fail_fmt);
    let summary_fmt_len = emit_cstr(out, ".fmt_test_summary", summary_fmt);
    // Per-test display-name constant. Numbered by source order to match the
    // tests vec; codegen never reads it as a Rust value, only emits a printf
    // arg, so the index is the only required key.
    let mut name_lens: Vec<usize> = Vec::with_capacity(tests.len());
    for (i, t) in tests.iter().enumerate() {
        let n = emit_cstr(out, &format!(".tn_{i}"), &t.display_name);
        name_lens.push(n);
    }
    out.push('\n');
    out.push_str("define i32 @main() {\n");
    out.push_str("entry:\n");
    out.push_str("  %passed = alloca i32\n");
    out.push_str("  %failed = alloca i32\n");
    out.push_str("  store i32 0, ptr %passed\n");
    out.push_str("  store i32 0, ptr %failed\n");
    for (i, t) in tests.iter().enumerate() {
        let pass_lbl = format!("p{i}");
        let fail_lbl = format!("fl{i}");
        let next_lbl = format!("n{i}");
        out.push_str(&format!("\n  ; test {} {}\n", i, t.display_name));
        out.push_str("  store i32 0, ptr @cpc_test_failed\n");
        if t.returns_i32 {
            out.push_str(&format!("  %ret{i} = call i32 @{}()\n", t.qualified_name));
            out.push_str(&format!("  %flag{i} = load i32, ptr @cpc_test_failed\n"));
            out.push_str(&format!("  %combined{i} = or i32 %ret{i}, %flag{i}\n"));
            out.push_str(&format!("  %ok{i} = icmp eq i32 %combined{i}, 0\n"));
        } else {
            out.push_str(&format!("  call void @{}()\n", t.qualified_name));
            out.push_str(&format!("  %flag{i} = load i32, ptr @cpc_test_failed\n"));
            out.push_str(&format!("  %ok{i} = icmp eq i32 %flag{i}, 0\n"));
        }
        out.push_str(&format!(
            "  br i1 %ok{i}, label %{pass_lbl}, label %{fail_lbl}\n"
        ));
        out.push('\n');
        out.push_str(&format!("{pass_lbl}:\n"));
        out.push_str(&format!(
            "  %pcall{i} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_test_pass, ptr noundef @.tn_{i})\n"
        ));
        out.push_str(&format!("  %pold{i} = load i32, ptr %passed\n"));
        out.push_str(&format!("  %pnew{i} = add i32 %pold{i}, 1\n"));
        out.push_str(&format!("  store i32 %pnew{i}, ptr %passed\n"));
        out.push_str(&format!("  br label %{next_lbl}\n"));
        out.push('\n');
        out.push_str(&format!("{fail_lbl}:\n"));
        out.push_str(&format!(
            "  %fcall{i} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_test_fail, ptr noundef @.tn_{i})\n"
        ));
        out.push_str(&format!("  %fold{i} = load i32, ptr %failed\n"));
        out.push_str(&format!("  %fnew{i} = add i32 %fold{i}, 1\n"));
        out.push_str(&format!("  store i32 %fnew{i}, ptr %failed\n"));
        out.push_str(&format!("  br label %{next_lbl}\n"));
        out.push('\n');
        out.push_str(&format!("{next_lbl}:\n"));
    }
    out.push_str("\n  ; summary\n");
    out.push_str("  %final_passed = load i32, ptr %passed\n");
    out.push_str("  %final_failed = load i32, ptr %failed\n");
    out.push_str(
        "  %scall = call i32 (ptr, ...) @printf(ptr noundef @.fmt_test_summary, i32 %final_passed, i32 %final_failed)\n",
    );
    out.push_str("  ret i32 %final_failed\n");
    out.push_str("}\n");
    // Silence unused warnings if `tests` is empty — the length values are
    // useful when debugging IR layout but otherwise discarded.
    let _ = (pass_fmt_len, fail_fmt_len, summary_fmt_len, name_lens);
}

/// v0.0.13 (topic D): map a function/method's `#[inline]` attribute to the
/// LLVM function attribute to splice after the `define ... (...)` signature.
///
/// - `#[inline]`         → ` inlinehint` (raises the inliner's likelihood; only
///                          matters once the inliner runs, i.e. `--release`/-O2+)
/// - `#[inline(always)]` → ` alwaysinline` (forces inlining, including at -O0
///                          and past LLVM's cost threshold — the lever for hot
///                          SIMD/kernel wrappers that otherwise stay a `bl`)
/// - `#[inline(never)]`  → ` noinline`
///
/// Returns `""` when the attribute is absent. The leading space is included so
/// callers can splice it directly after the closing `)`. Arg-shape errors are
/// already reported by `attrs.rs`, so an unrecognized arg yields `""` here.
fn inline_fn_attr(attrs: &[Attribute]) -> &'static str {
    for a in attrs {
        if a.path.name != "inline" {
            continue;
        }
        return match a.args.as_slice() {
            [] => " inlinehint",
            [AttrArg::Ident(id)] if id.name == "always" => " alwaysinline",
            [AttrArg::Ident(id)] if id.name == "never" => " noinline",
            _ => "",
        };
    }
    ""
}

/// v0.0.14 inline asm Tier 3: a `#[naked]` function emits no prologue/epilogue —
/// its body is inline asm that handles the ABI and the return itself. The LLVM
/// `naked` attribute suppresses frame setup; `noinline` keeps the body intact.
fn has_naked_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path.name == "naked")
}

fn gen_function(
    out: &mut String,
    f: &Function,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    emitted_extern_symbols: &mut std::collections::HashSet<String>,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
    is_lib: bool,
) {
    // Builtin name: codegen never emits a definition for it; clang links printf.
    if f.name.name == "println" {
        return;
    }

    // v0.0.3 Phase 5 Slice 5E.3: `async fn` bodies route to the
    // coroutine codegen path. The lowered fn returns a `Future[T]`
    // wrapping an LLVM coroutine handle; the user's declared return
    // type T lands in the coroutine promise for the executor to read.
    if f.is_async {
        gen_async_function(out, f, sigs, types, str_lits, mode, test_mode, md, tramps);
        return;
    }
    // v0.0.4 Phase 4 Slice 4A: `gen fn` bodies are also coroutines, but
    // they produce `Iterator[T]` (multi-value sequence) instead of
    // `Future[T]` (single eventual value). `yield V` stashes V into the
    // coroutine promise and suspends; the iterator's `next()` reads the
    // promise + resumes.
    if f.is_gen {
        gen_gen_function(out, f, sigs, types, str_lits, mode, test_mode, md, tramps);
        return;
    }

    let sig = sigs.get(&f.name.name).expect("sig was collected");
    let return_ty = sig.return_type.clone();

    // Slice 10.FFI.1: extern fn declarations emit `declare TYPE @name(...)`
    // and no body. LLVM matches against the platform C ABI at link time.
    // Param attributes (noalias/readonly) are skipped — they're only sound
    // on C+ fns whose call sites the borrow checker has analyzed.
    //
    // Phase 5 Slice 5.C: `pub extern fn name(...) { body }` is the export
    // form (definition). Parser sets `is_pub` only on that shape. Fall
    // through to normal `define` emission for those — they're regular
    // function bodies that happen to commit to a stable C-callable
    // name. Slice 5.D will adjust the LLVM signature to match the
    // platform C ABI for value-passed aggregates.
    if f.is_extern && !f.is_pub {
        // Slice 10.FFI.4: some C symbols are already declared in the
        // codegen preamble (printf for `println`, memcmp for `str ==`).
        // Re-declaring them would clash at link time; skip if the
        // user's extern fn matches a preamble-emitted name. The sema
        // signature still flows through the call-site routing.
        // Phase 11 / ObjC interop: dedup also against the resolved
        // link_name (e.g. a user could declare `#[link_name = "printf"]
        // extern fn my_printf(...)` — same symbol, would clash).
        let resolved_symbol = sig.link_name.as_deref().unwrap_or(&f.name.name);
        // Preamble-declared libc symbols. Skip re-emission if a user's
        // `extern fn malloc/free/memcpy` shadows; we trust the user's
        // signature matches the preamble's (i64 args / ptr returns). The
        // preamble shapes are the ones the `string` runtime emits calls
        // against — a divergent user signature would mis-link anyway.
        if matches!(
            resolved_symbol,
            "printf" | "memcmp" | "malloc" | "free" | "memcpy"
        ) {
            return;
        }
        // Phase 11 / ObjC interop: multiple `extern fn` declarations
        // may share a single linker symbol via `#[link_name = "..."]`.
        // The codegen-side dedup prevents emitting the same `declare`
        // twice (which LLVM rejects).
        if emitted_extern_symbols.contains(resolved_symbol) {
            return;
        }
        emitted_extern_symbols.insert(resolved_symbol.to_string());
        // v0.0.12 G-027: apply the same C-ABI classification on extern
        // *imports* that `pub extern fn` *exports* already use. Previously
        // the import side emitted `declare %T @f(...)` for any return type
        // — the AArch64-Darwin ABI requires structs >16B to be returned
        // via a hidden `ptr sret(%T)` first arg, and clang on the C side
        // emits exactly that. The mismatch silently miscompiled call sites
        // (caller wrote args into x0 where the callee expected the sret
        // pointer → SIGSEGV on first call). Mirroring `classify_c_abi`
        // here makes the two halves of the ABI agree.
        let ret_abi = classify_c_abi(&return_ty, types);
        let uses_sret = matches!(ret_abi, CAbiClass::Indirect);
        let coerce_ret_ty: Option<String> = if let CAbiClass::Coerce { llvm_ty, .. } = &ret_abi {
            Some(llvm_ty.clone())
        } else {
            None
        };
        let sig_ret_ty: String = if uses_sret {
            "void".to_string()
        } else if let Some(t) = &coerce_ret_ty {
            t.clone()
        } else {
            llvm_ty(&return_ty, types)
        };
        write!(out, "declare {} @{}(", sig_ret_ty, resolved_symbol).unwrap();
        if uses_sret {
            let ret_inner = llvm_ty(&return_ty, types);
            let (sz, al) = static_layout(&return_ty, types)
                .expect("extern sret return type must have a known layout");
            write!(
                out,
                "ptr sret({}) noalias nonnull noundef writable dereferenceable({}) align {}",
                ret_inner, sz, al
            )
            .unwrap();
            if !f.params.is_empty() || f.is_variadic {
                out.push_str(", ");
            }
        }
        for (i, (_param, (pty, _move_flag, _mut_flag, _restrict_flag))) in
            f.params.iter().zip(sig.params.iter()).enumerate()
        {
            if i > 0 {
                out.push_str(", ");
            }
            // Classify each param too — symmetric with the export path.
            // For Indirect, the C ABI passes by pointer; for Coerce, an
            // integer-class type packs the aggregate; Direct passes the
            // type unchanged.
            match classify_c_abi(pty, types) {
                CAbiClass::Indirect => out.push_str("ptr"),
                CAbiClass::Coerce { llvm_ty, .. } => out.push_str(&llvm_ty),
                CAbiClass::Direct => out.push_str(&llvm_ty(pty, types)),
            }
        }
        // Slice 10.FFI.4: trailing `, ...` for variadic extern fns.
        if f.is_variadic {
            if !f.params.is_empty() {
                out.push_str(", ");
            }
            out.push_str("...");
        }
        out.push_str(")\n");
        return;
    }

    // Function header. Non-Copy `mut x: T` params lower to a `ptr noalias`
    // parameter (§2.9 exclusive borrow ABI, with 6BC.codegen's `noalias`
    // attribute proving uniqueness to LLVM). Non-Copy shared `x: T` params
    // lower to `ptr readonly` — pointer-pass avoids the byte-copy and the
    // callee provably can't write through the pointer (sema rejects).
    // Everything else stays value-passed.
    //
    // Slice 1D: when the return type triggers `return_passes_by_sret`
    // (currently: owned `string` only), rewrite the signature so the
    // result lands at a caller-provided slot. The sret pointer is the
    // first param (%0), and the user-declared params shift by one. The
    // function returns `void`.
    // Phase 5 Slice 5.D: classify return + params against the C ABI when
    // this is a `pub extern fn` export. Indirect returns flow through the
    // existing Slice 1D `sret` path; ≤16-byte aggregate returns coerce
    // to integer-class types; scalar returns pass through.
    let is_c_export = f.is_extern && f.is_pub;
    let ret_abi = if is_c_export {
        classify_c_abi(&return_ty, types)
    } else {
        CAbiClass::Direct
    };
    let param_abis: Vec<CAbiClass> = if is_c_export {
        sig.params
            .iter()
            .map(|(pty, _, _, _)| classify_c_abi(pty, types))
            .collect()
    } else {
        vec![CAbiClass::Direct; sig.params.len()]
    };

    // Existing Slice 1D path: owned `string` returns use sret. 5.D adds
    // sret for any C-export with an Indirect-class return (>16 bytes).
    // v0.0.3 Slice 1P widens to non-Copy structs for non-C-export fns
    // (cross-module heap-owning struct return drop-after-move).
    let uses_sret = if is_c_export {
        return_passes_by_sret(&return_ty) || matches!(ret_abi, CAbiClass::Indirect)
    } else {
        return_passes_by_sret_widened(&return_ty, types)
    };
    let coerce_ret_ty: Option<String> = if let CAbiClass::Coerce { llvm_ty, .. } = &ret_abi {
        Some(llvm_ty.clone())
    } else {
        None
    };
    let sig_return_ty: String = if uses_sret {
        "void".to_string()
    } else if let Some(t) = &coerce_ret_ty {
        t.clone()
    } else {
        llvm_ty(&return_ty, types)
    };
    let ret_ty_str = llvm_ty(&return_ty, types); // raw underlying type (e.g. for sret(...))
    let sret_param_offset: u32 = if uses_sret { 1 } else { 0 };
    // Phase 5 Slice 5.B: in library builds, non-`pub` items get
    // `internal` linkage so LTO can strip them out of the final
    // `.dylib` / `.a`. Executable builds keep external linkage (matches
    // pre-5.B behavior; the existing test substring assertions pin that).
    // `main` is the linker entry point and always external. `pub` items
    // form the public ABI and stay external in lib mode.
    // v0.0.3 Slice 3D: roll lib-mode internal linkage out to executable
    // builds. `main` and `pub` items stay external (linker entry +
    // public ABI); everything else is `internal` so LTO can strip
    // unused helpers. The `is_lib` parameter no longer gates this rule.
    let linkage = if f.name.name == "main" || f.is_pub {
        ""
    } else {
        "internal "
    };
    // v0.0.8 bench-gap fix C: `internal`-linkage functions whose
    // address isn't taken anywhere can use `fastcc` (LLVM picks its own
    // register-passing convention for the callee, skipping the C ABI's
    // caller-saved register set). The eligibility set was computed in
    // `generate_inner` from the address-taken pre-pass; callers that
    // reach this function via direct call also emit `call fastcc` so
    // the cc matches.
    let cc = if linkage == "internal " {
        md.fastcc_prefix(&f.name.name)
    } else {
        ""
    };
    write!(out, "define {}{}{} @{}(", linkage, cc, sig_return_ty, f.name.name).unwrap();
    if uses_sret {
        // sret slot: caller-allocated, callee-writable, exact size + align.
        let (sz, al) =
            static_layout(&return_ty, types).expect("sret return type must have a known layout");
        write!(
            out,
            "ptr sret({}) noalias nonnull noundef writable dereferenceable({}) align {} %0",
            ret_ty_str, sz, al
        )
        .unwrap();
        if !f.params.is_empty() {
            out.push_str(", ");
        }
    }
    for (i, (_param, (pty, move_flag, mut_flag, restrict_flag))) in
        f.params.iter().zip(sig.params.iter()).enumerate()
    {
        if i > 0 {
            out.push_str(", ");
        }
        let llvm_idx = i as u32 + sret_param_offset;
        // Phase 5 Slice 5.D: when this fn is a C-ABI export, override the
        // LLVM signature for value-passed aggregates per the platform PCS:
        //   ≤8 bytes → i64
        //   9..16   → [2 x i64]
        //   >16     → ptr (caller-allocated; no `byval` on aarch64-darwin)
        //
        // Pointer-passed `mut`/`move` params are not C-ABI exportable
        // anyway (sema 5.C rejects non-Copy aggregates that aren't
        // `#[repr(C)]` and rejects Drop entirely), so the `param_passes_by_ptr`
        // path doesn't co-occur with non-Direct ABI classes here.
        match &param_abis[i] {
            CAbiClass::Coerce { llvm_ty, .. } => {
                write!(out, "{} %{}", llvm_ty, llvm_idx).unwrap();
            }
            CAbiClass::Indirect => {
                // v0.0.3 Slice 3F: x86_64-sysv requires `byval(<ty>) align <A>`
                // on indirect args so the backend knows to materialize a
                // caller-side copy that the callee can mutate. aarch64-darwin
                // doesn't use byval — caller and callee implicitly share
                // the layout via the bare pointer.
                if cfg!(target_arch = "x86_64") {
                    let (_sz, al) = static_layout(pty, types).unwrap_or((8, 8));
                    let inner = llvm_ty(pty, types);
                    write!(out, "ptr byval({inner}) align {al} %{llvm_idx}").unwrap();
                } else {
                    write!(out, "ptr %{}", llvm_idx).unwrap();
                }
            }
            CAbiClass::Direct => {
                let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
                let attrs = param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, by_ptr, types);
                let base_ty = if by_ptr {
                    "ptr".to_string()
                } else {
                    llvm_ty(pty, types)
                };
                if attrs.is_empty() {
                    write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
                } else {
                    write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
                }
            }
        }
    }
    out.push_str(")");
    out.push_str(inline_fn_attr(&f.attributes));
    let is_naked = has_naked_attr(&f.attributes);
    if is_naked {
        out.push_str(" naked noinline");
    }
    out.push_str(" {\n");
    out.push_str("entry:\n");

    // Build the function body
    let mut state = FnState::new(
        return_ty.clone(),
        sigs,
        types,
        str_lits,
        mode,
        test_mode,
        md,
        tramps,
    );
    state.collect_moved_bindings(&f.body);
    // Slice 1E: record this fn's parameter types so the Return-statement
    // predicate can check musttail signature equality against the callee.
    state.enclosing_params = sig.params.iter().map(|(t, _, _, _)| t.clone()).collect();
    state.tail_call_eligible = true;
    // v0.0.8 fix C: musttail requires caller and callee to share a cc.
    // Record the enclosing fn's cc decision so the Return-stmt predicate
    // can match it against the callee's cc.
    state.enclosing_is_fastcc = cc == "fastcc ";
    // Slice 1D: if this fn uses sret, remember the slot's SSA name (%0) so
    // StmtKind::Return can store-into it before `ret void`.
    if uses_sret {
        state.sret_slot = Some("%0".to_string());
    }
    // Phase 5 Slice 5.D: coerced returns flow through StmtKind::Return.
    state.coerce_ret = coerce_ret_ty.clone();

    // Bind params. Pointer-passed params (`mut x: T` non-Copy) bind directly
    // to the SSA argument — no alloca, no initial store — exactly like
    // receivers. Value-passed params copy into an alloca; `move`-marked Drop
    // params register a scope-exit drop. Non-`move` value-passed params are
    // left unregistered to avoid double-free of the caller's value.
    //
    // Slice 1D: when sret is in effect, the user-declared params are at
    // SSA indices 1..N instead of 0..N-1 — the sret slot occupies %0.
    //
    // Phase 5 Slice 5.D: `pub extern fn` exports apply C-ABI param
    // coercions per `param_abis`:
    //   - Coerce: alloca a slot sized for the coerced type (≥ struct size,
    //     so the coerced store doesn't overflow), store the coerced SSA
    //     value into it, bind as the original struct type — subsequent
    //     field GEPs use the original-type's offsets and read valid bytes.
    //   - Indirect: the SSA arg IS a pointer to the C caller's slot.
    //     Bind directly; gen_field GEPs off it like any other place.
    // A `#[naked]` function materializes no params: the body is inline asm
    // that reads arguments straight from their ABI registers. Skipping the
    // prologue is the whole point (the SSA args stay unused, which is legal).
    for (i, (param, (pty, move_flag, mut_flag, restrict_flag))) in f
        .params
        .iter()
        .zip(sig.params.iter())
        .enumerate()
        .filter(|_| !is_naked)
    {
        let llvm_idx = i as u32 + sret_param_offset;
        // C-ABI coerced param: alloca with coerced size, store the coerced
        // value, bind as original struct type. The alloca uses the
        // coerced LLVM type because it dominates the size + align needed.
        if let CAbiClass::Coerce {
            llvm_ty: clty,
            align,
            ..
        } = &param_abis[i]
        {
            let slot = state.alloca_named_raw(&param.name.name, clty, *align);
            state
                .body
                .push_str(&format!("  store {} %{}, ptr {}\n", clty, llvm_idx, slot));
            state.bind(&param.name.name, slot, pty.clone());
            continue;
        }
        // C-ABI indirect param: the SSA arg is a pointer to the caller's
        // by-value slot. Bind directly; no alloca, no initial copy.
        if matches!(param_abis[i], CAbiClass::Indirect) {
            state.bind(&param.name.name, format!("%{llvm_idx}"), pty.clone());
            continue;
        }
        if param_passes_by_ptr(pty, *move_flag, *mut_flag, types) {
            state.bind(&param.name.name, format!("%{llvm_idx}"), pty.clone());
            // v0.0.5 Slice 1A: track params that share heap with the
            // caller (so the body cannot return the binding as-is
            // without a deep clone — both ends would Drop the same
            // heap). For pointer-passed non-Copy structs, the binding
            // IS the caller's pointer.
            state.borrowed_params.insert(param.name.name.clone());
            continue;
        }
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types),
            llvm_idx,
            slot
        ));
        state.bind(&param.name.name, slot.clone(), pty.clone());
        // v0.0.5 Slice 1A: value-passed non-`move` Drop param. The
        // aggregate is a bit-copy of the caller's; its heap pointer
        // (Ty::String → {ptr, len, cap}, Vec[T] → {ptr, len, cap},
        // etc.) ALIASES the caller's. Returning this binding without
        // a clone double-frees at scope exit. Track for the auto-
        // clone-on-return path (today only Ty::String returns are
        // auto-cloned — Vec[T] and similar need T::clone glue, their
        // own slice).
        if !*move_flag && matches!(pty, Ty::String) {
            state.borrowed_params.insert(param.name.name.clone());
        }
        if *move_flag {
            // v0.0.14 auto field-drop: a moved-in owning aggregate (struct with
            // owning fields, with or without an explicit `drop`, or a tagged
            // enum with owning payloads) is the callee's to tear down.
            match pty {
                Ty::Struct(id) if state.needs_drop(pty) => {
                    state.register_drop(&param.name.name, &slot, *id);
                }
                Ty::Enum(id) if state.needs_drop(pty) => {
                    state.register_drop_kind(&param.name.name, &slot, DropKind::Enum(*id));
                }
                _ => {}
            }
        }
    }

    // Emit body
    if is_naked {
        state.gen_naked_body(&f.body);
    } else {
        state.gen_body_block(&f.body);
    }

    // Ensure final terminator
    if !state.terminated {
        if is_naked {
            // The body's asm performs the real return; control never falls
            // through to here. `unreachable` is the valid IR terminator (a
            // `ret` would emit a stray epilogue-less return after the asm).
            state.emit_terminator("unreachable");
        } else {
            match &return_ty {
                Ty::Unit => state.emit_terminator("ret void"),
                // Sema guarantees a value; this is unreachable, but emit
                // `unreachable` so the IR validates if we slip through.
                _ => state.emit_terminator("unreachable"),
            }
        }
    }

    // Slice 1C: scoped alias metadata for noalias-shaped params. Run the
    // dataflow over `state.body` (allocas in `state.allocas` never touch
    // these ptrs — they're fresh slots, not derived from a param).
    //
    // v0.0.3 Slice 3C: extended to include non-Copy local allocas. Each
    // gets its own scope; the borrow checker proves locals are disjoint
    // from each other AND from noalias params (otherwise we'd have a
    // double-ownership E0335/E0370). After-inlining this metadata still
    // applies to the loads/stores it tags, which is exactly the case
    // where param attrs degrade.
    let noalias_params: Vec<u32> = f
        .params
        .iter()
        .zip(sig.params.iter())
        .enumerate()
        .filter_map(|(i, (_, (pty, mv, mu, _restrict_flag)))| {
            (param_passes_by_ptr(pty, *mv, *mu, types) && (*mv || *mu)).then_some(i as u32)
        })
        .collect();
    let local_slots = state.noalias_local_slots.clone();
    let total_scopes = noalias_params.len() + local_slots.len();
    if total_scopes >= 2 {
        let domain = md.register_alias_domain(&f.name.name);
        let mut scopes: Vec<u32> = Vec::with_capacity(total_scopes);
        for i in 0..noalias_params.len() {
            scopes.push(md.register_alias_scope(domain, &format!("p{i}")));
        }
        for i in 0..local_slots.len() {
            scopes.push(md.register_alias_scope(domain, &format!("l{i}")));
        }
        let this_lists: Vec<u32> = scopes
            .iter()
            .map(|&s| md.register_alias_scope_list(&[s]))
            .collect();
        let other_lists: Vec<u32> = (0..scopes.len())
            .map(|i| {
                let others: Vec<u32> = scopes
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, &s)| s)
                    .collect();
                md.register_alias_scope_list(&others)
            })
            .collect();
        let mut seed: HashMap<String, usize> = HashMap::new();
        for (idx, &param_ssa) in noalias_params.iter().enumerate() {
            seed.insert(format!("%{param_ssa}"), idx);
        }
        for (idx, slot) in local_slots.iter().enumerate() {
            seed.insert(slot.clone(), noalias_params.len() + idx);
        }
        state.body = annotate_alias_scope_metadata(&state.body, &seed, &this_lists, &other_lists);
    }

    // Glue: allocas first (in entry), then body
    for line in &state.allocas {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

/// v0.0.3 Phase 5 Slice 5E.3: emit an `async fn foo() -> T` as an
/// LLVM coroutine. The lowered function:
///   1. Allocates the coroutine frame via `malloc(@llvm.coro.size())`
///      and obtains a handle from `@llvm.coro.begin`.
///   2. Runs the user's body. `return X` is rewritten to "store X
///      into the coroutine promise, then `br final_suspend`".
///   3. Final-suspends (the standard switched-resume pattern with the
///      switch's `default` label returning the handle to the caller).
///   4. The cleanup path (taken when the executor calls
///      `coro.destroy`) frees the frame via `llvm.coro.free` + `free`.
///   5. Wraps the handle into a `Future[T]` aggregate and returns it.
///
/// Scope: v0.0.3 only handles primitive Copy `T` ≤ 8 bytes. Non-Copy
/// returns (string/Vec) need sret-aware promise sizing — same gap as
/// thread::spawn's Copy-only restriction and tracked alongside it.
/// Generic async fns work because monomorphization fires before
/// codegen (the async fn is a regular `Function` post-mono).
/// v0.0.5 Phase 2B: lower a generator method (e.g.
/// `pub gen fn iter(self) -> T`) to an LLVM coroutine returning
/// `Iterator[T]`. Mirrors `gen_gen_function`'s overall shape but
/// adapts to the method's receiver-prefix parameter layout.
/// v0.0.5 Phase 4 Slice 4B: lower an `async fn` method to an LLVM
/// coroutine that produces `Future[T]`. Structurally identical to
/// `gen_async_function` for the body (same coro.id/begin/suspend/end
/// pattern, same promise wiring) but with method-shaped signature
/// (receiver + params) and a mangled name. Mirror of `gen_gen_method`
/// for the async/Future side.
fn gen_async_method(
    out: &mut String,
    struct_id: StructId,
    m: &Method,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
) {
    let struct_name = types.struct_defs[struct_id.0 as usize].name.clone();
    let sig = types.struct_defs[struct_id.0 as usize]
        .methods
        .get(&m.name.name)
        .expect("sig was collected")
        .clone();
    let mangled = mangle(&struct_name, &m.name.name);

    let inner_ty = match &m.return_type {
        Some(t) => ty_from(t, types),
        None => Ty::Unit,
    };
    let inner_llvm = llvm_ty(&inner_ty, types);
    let (_inner_size, inner_align) = static_layout(&inner_ty, types).unwrap_or((8, 8));
    let future_ret_ty = sig.return_type.clone();
    let future_llvm = llvm_ty(&future_ret_ty, types);

    let linkage = if m.is_pub { "" } else { "internal " };
    // v0.0.8 fix C: non-pub async method → eligible for fastcc.
    let cc = if linkage == "internal " {
        md.fastcc_prefix(&mangled)
    } else {
        ""
    };

    // Function header: receiver + params, returns Future[T] (single-ptr
    // aggregate, fits in a register — no sret).
    write!(out, "define {}{}{} @{}(", linkage, cc, future_llvm, mangled).unwrap();
    let mut llvm_idx: u32 = 0;
    let mut first = true;
    let struct_ty = Ty::Struct(struct_id);
    if let Some(rcv) = sig.receiver {
        let (mv, mu) = match rcv {
            Receiver::Read => (false, false),
            Receiver::Mut => (false, true),
            Receiver::Move => (true, true),
        };
        if !first {
            out.push_str(", ");
        }
        // v0.0.8 fix A: same Copy+Read by-value rule as `gen_method` so
        // the call-site lowering in `gen_method_call` (which doesn't
        // distinguish async/gen/sync) matches this signature.
        let self_by_ptr =
            !is_copy_ty(&struct_ty, types) || !matches!(rcv, Receiver::Read);
        let attrs = param_attrs(&struct_ty, mv, mu, false, self_by_ptr, types);
        if self_by_ptr {
            if attrs.is_empty() {
                write!(out, "ptr %{llvm_idx}").unwrap();
            } else {
                write!(out, "ptr {} %{llvm_idx}", attrs).unwrap();
            }
        } else {
            let base_ty = llvm_ty(&struct_ty, types);
            if attrs.is_empty() {
                write!(out, "{} %{llvm_idx}", base_ty).unwrap();
            } else {
                write!(out, "{} {} %{llvm_idx}", base_ty, attrs).unwrap();
            }
        }
        llvm_idx += 1;
        first = false;
    }
    for (_param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        if !first {
            out.push_str(", ");
        }
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        let attrs = param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, by_ptr, types);
        let base_ty = if by_ptr {
            "ptr".to_string()
        } else {
            llvm_ty(pty, types)
        };
        if attrs.is_empty() {
            write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
        } else {
            write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    out.push_str(") presplitcoroutine {\nentry:\n");

    // Promise allocation. Unit-returning async methods use an i8 placeholder
    // so the promise pointer is addressable but never written through.
    let promise_ty: &str = if matches!(inner_ty, Ty::Unit) {
        "i8"
    } else {
        &inner_llvm
    };
    let promise_align: u64 = if matches!(inner_ty, Ty::Unit) {
        1
    } else {
        inner_align
    };
    out.push_str(&format!(
        "  %.coro.promise = alloca {promise_ty}, align {promise_align}\n"
    ));
    out.push_str(&format!(
        "  %.coro.id = call token @llvm.coro.id(i32 {promise_align}, ptr %.coro.promise, ptr null, ptr null)\n"
    ));
    out.push_str("  %.coro.size = call i64 @llvm.coro.size.i64()\n");
    out.push_str("  %.coro.mem = call ptr @malloc(i64 %.coro.size)\n");
    out.push_str("  %.coro.hdl = call ptr @llvm.coro.begin(token %.coro.id, ptr %.coro.mem)\n");

    let mut state = FnState::new(
        inner_ty.clone(),
        sigs,
        types,
        str_lits,
        mode,
        test_mode,
        md,
        tramps,
    );
    state.return_ty = inner_ty.clone();
    state.coro_promise = Some((
        ".coro.hdl".to_string(),
        inner_llvm.clone(),
        inner_align as u32,
    ));
    state.collect_moved_bindings(&m.body);

    let mut next_idx: u32 = 0;
    if let Some(rcv) = sig.receiver {
        let self_by_ptr =
            !is_copy_ty(&Ty::Struct(struct_id), types) || !matches!(rcv, Receiver::Read);
        if self_by_ptr {
            let recv_name = format!("%{}", next_idx);
            state.bind("self", recv_name, Ty::Struct(struct_id));
        } else {
            // v0.0.8 fix A: Copy `self` (Read) arrives by value; spill to
            // a slot so `self.x` field reads keep their place shape.
            let slot = state.alloca_named("self", Ty::Struct(struct_id));
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                llvm_ty(&Ty::Struct(struct_id), types),
                next_idx,
                slot
            ));
            state.bind("self", slot, Ty::Struct(struct_id));
        }
        next_idx += 1;
    }
    for (param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        if by_ptr {
            state.bind(&param.name.name, format!("%{}", next_idx), pty.clone());
        } else {
            let slot = state.alloca_named(&param.name.name, pty.clone());
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                llvm_ty(pty, types),
                next_idx,
                slot
            ));
            state.bind(&param.name.name, slot.clone(), pty.clone());
        }
        next_idx += 1;
    }

    let _body_value = state.gen_block_expr(&m.body);

    // Body fall-off: same handling as gen_async_function — Unit methods
    // fall through cleanly, non-Unit ones store undef (sema rejects
    // missing-return for non-Unit T, so this is the unit fall-off case).
    if !state.terminated {
        if !matches!(inner_ty, Ty::Unit) {
            let prom_ptr = state.next_tmp();
            state.emit(&format!(
                "{prom_ptr} = call ptr @llvm.coro.promise(ptr %.coro.hdl, i32 {promise_align}, i1 false)"
            ));
            state.emit(&format!(
                "store {inner_llvm} undef, ptr {prom_ptr}, align {promise_align}"
            ));
        }
        state.emit_terminator("br label %.coro.final_suspend");
    }

    // v0.0.5 Slice 4F: notify awaiter before final suspend.
    // See parallel comment in gen_async_function.
    state.body.push_str(".coro.final_suspend:\n");
    state
        .body
        .push_str("  call void @stdlib_reactor_notify_completed_v1(ptr %.coro.hdl)\n");
    state
        .body
        .push_str("  %.coro.fs = call i8 @llvm.coro.suspend(token none, i1 true)\n");
    state.body.push_str("  switch i8 %.coro.fs, label %.coro.ramp_return [i8 0, label %.coro.trap i8 1, label %.coro.cleanup]\n");
    state.body.push_str(".coro.ramp_return:\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.trap:\n");
    state.body.push_str("  call void @llvm.trap()\n");
    state.body.push_str("  unreachable\n");
    state.body.push_str(".coro.cleanup:\n");
    state.body.push_str(
        "  %.coro.mem_free = call ptr @llvm.coro.free(token %.coro.id, ptr %.coro.hdl)\n",
    );
    state
        .body
        .push_str("  call void @free(ptr %.coro.mem_free)\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.end:\n");
    state.body.push_str(
        &coro_end_call_ir(),
    );
    state.body.push_str(&format!(
        "  %.coro.future0 = insertvalue {future_llvm} undef, ptr %.coro.hdl, 0\n"
    ));
    state
        .body
        .push_str(&format!("  ret {future_llvm} %.coro.future0\n"));

    for a in &state.allocas {
        out.push_str("  ");
        out.push_str(a);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

fn gen_gen_method(
    out: &mut String,
    struct_id: StructId,
    m: &Method,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
) {
    let struct_name = types.struct_defs[struct_id.0 as usize].name.clone();
    let sig = types.struct_defs[struct_id.0 as usize]
        .methods
        .get(&m.name.name)
        .expect("sig was collected")
        .clone();
    let mangled = mangle(&struct_name, &m.name.name);

    let inner_ty = match &m.return_type {
        Some(t) => ty_from(t, types),
        None => Ty::Unit,
    };
    let inner_llvm = llvm_ty(&inner_ty, types);
    let (_inner_size, inner_align) = static_layout(&inner_ty, types).unwrap_or((8, 8));
    let iter_ret_ty = sig.return_type.clone();
    let iter_llvm = llvm_ty(&iter_ret_ty, types);

    let linkage = if m.is_pub { "" } else { "internal " };
    // v0.0.8 fix C: non-pub gen method → eligible for fastcc.
    let cc = if linkage == "internal " {
        md.fastcc_prefix(&mangled)
    } else {
        ""
    };

    // Function header: same receiver + param structure as a regular
    // method, but the return is `Iterator[T]` (one-ptr aggregate that
    // fits in a register, so no sret).
    write!(out, "define {}{}{} @{}(", linkage, cc, iter_llvm, mangled).unwrap();
    let mut llvm_idx: u32 = 0;
    let mut first = true;
    let struct_ty = Ty::Struct(struct_id);
    if let Some(rcv) = sig.receiver {
        let (mv, mu) = match rcv {
            Receiver::Read => (false, false),
            Receiver::Mut => (false, true),
            Receiver::Move => (true, true),
        };
        if !first {
            out.push_str(", ");
        }
        // v0.0.8 fix A: same Copy+Read by-value rule as `gen_method` so
        // the call-site lowering in `gen_method_call` (which doesn't
        // distinguish gen/sync) matches this signature.
        let self_by_ptr =
            !is_copy_ty(&struct_ty, types) || !matches!(rcv, Receiver::Read);
        let attrs = param_attrs(&struct_ty, mv, mu, false, self_by_ptr, types);
        if self_by_ptr {
            if attrs.is_empty() {
                write!(out, "ptr %{llvm_idx}").unwrap();
            } else {
                write!(out, "ptr {} %{llvm_idx}", attrs).unwrap();
            }
        } else {
            let base_ty = llvm_ty(&struct_ty, types);
            if attrs.is_empty() {
                write!(out, "{} %{llvm_idx}", base_ty).unwrap();
            } else {
                write!(out, "{} {} %{llvm_idx}", base_ty, attrs).unwrap();
            }
        }
        llvm_idx += 1;
        first = false;
    }
    for (_param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        if !first {
            out.push_str(", ");
        }
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        let attrs = param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, by_ptr, types);
        let base_ty = if by_ptr {
            "ptr".to_string()
        } else {
            llvm_ty(pty, types)
        };
        if attrs.is_empty() {
            write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
        } else {
            write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    out.push_str(") presplitcoroutine {\nentry:\n");

    let promise_ty: &str = if matches!(inner_ty, Ty::Unit) {
        "i8"
    } else {
        &inner_llvm
    };
    let promise_align: u64 = if matches!(inner_ty, Ty::Unit) {
        1
    } else {
        inner_align
    };
    out.push_str(&format!(
        "  %.coro.promise = alloca {promise_ty}, align {promise_align}\n"
    ));
    out.push_str(&format!(
        "  %.coro.id = call token @llvm.coro.id(i32 {promise_align}, ptr %.coro.promise, ptr null, ptr null)\n"
    ));
    out.push_str("  %.coro.size = call i64 @llvm.coro.size.i64()\n");
    out.push_str("  %.coro.mem = call ptr @malloc(i64 %.coro.size)\n");
    out.push_str("  %.coro.hdl = call ptr @llvm.coro.begin(token %.coro.id, ptr %.coro.mem)\n");

    let mut state = FnState::new(Ty::Unit, sigs, types, str_lits, mode, test_mode, md, tramps);
    state.return_ty = Ty::Unit;
    state.coro_promise = Some((
        ".coro.hdl".to_string(),
        inner_llvm.clone(),
        inner_align as u32,
    ));
    state.collect_moved_bindings(&m.body);

    let mut next_idx: u32 = 0;
    // Bind the receiver. Non-Copy receivers are pointer-passed; Copy
    // `self` (Read) is value-passed (v0.0.8 fix A) — spill to a slot so
    // `self.x` keeps its place shape.
    if let Some(rcv) = sig.receiver {
        let self_by_ptr =
            !is_copy_ty(&Ty::Struct(struct_id), types) || !matches!(rcv, Receiver::Read);
        if self_by_ptr {
            let recv_name = format!("%{}", next_idx);
            state.bind("self", recv_name, Ty::Struct(struct_id));
        } else {
            let slot = state.alloca_named("self", Ty::Struct(struct_id));
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                llvm_ty(&Ty::Struct(struct_id), types),
                next_idx,
                slot
            ));
            state.bind("self", slot, Ty::Struct(struct_id));
        }
        next_idx += 1;
    }
    // Bind params to allocas (value-passed) or pointer slots (pointer-passed).
    for (param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        if by_ptr {
            state.bind(&param.name.name, format!("%{}", next_idx), pty.clone());
        } else {
            let slot = state.alloca_named(&param.name.name, pty.clone());
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                llvm_ty(pty, types),
                next_idx,
                slot
            ));
            state.bind(&param.name.name, slot.clone(), pty.clone());
        }
        next_idx += 1;
    }

    let _body_value = state.gen_block_expr(&m.body);

    // gen-method body is Unit-typed (it `yield`s values rather than
    // returning one). Fall-off / `return;` both branch into final_suspend.
    if !state.terminated {
        state.emit_terminator("br label %.coro.final_suspend");
    }

    state.body.push_str(".coro.final_suspend:\n");
    state
        .body
        .push_str("  %.coro.fs = call i8 @llvm.coro.suspend(token none, i1 true)\n");
    state.body.push_str("  switch i8 %.coro.fs, label %.coro.ramp_return [i8 0, label %.coro.trap i8 1, label %.coro.cleanup]\n");
    state.body.push_str(".coro.ramp_return:\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.trap:\n");
    state.body.push_str("  call void @llvm.trap()\n");
    state.body.push_str("  unreachable\n");
    state.body.push_str(".coro.cleanup:\n");
    state.body.push_str(
        "  %.coro.mem_free = call ptr @llvm.coro.free(token %.coro.id, ptr %.coro.hdl)\n",
    );
    state
        .body
        .push_str("  call void @free(ptr %.coro.mem_free)\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.end:\n");
    state.body.push_str(
        &coro_end_call_ir(),
    );
    state.body.push_str(&format!(
        "  %.coro.iter0 = insertvalue {iter_llvm} undef, ptr %.coro.hdl, 0\n"
    ));
    state
        .body
        .push_str(&format!("  ret {iter_llvm} %.coro.iter0\n"));

    for a in &state.allocas {
        out.push_str("  ");
        out.push_str(a);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

/// v0.0.4 Phase 4 Slice 4A: lower a `gen fn` body to an LLVM coroutine
/// that produces `Iterator[T]`. Structurally identical to
/// `gen_async_function` — same coro.id/begin/suspend/end pattern — but
/// wraps the handle in Iterator instead of Future and doesn't write a
/// dummy promise value on body fall-off (gen body is Unit; the inner T
/// only appears at `yield` sites, which write the promise themselves).
fn gen_gen_function(
    out: &mut String,
    f: &Function,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
) {
    let sig = sigs.get(&f.name.name).expect("sig was collected");
    let inner_ty = match &f.return_type {
        Some(t) => ty_from(t, types),
        None => Ty::Unit,
    };
    let inner_llvm = llvm_ty(&inner_ty, types);
    let (_inner_size, inner_align) = static_layout(&inner_ty, types).unwrap_or((8, 8));
    let iter_ret_ty = sig.return_type.clone();

    let linkage = if f.name.name == "main" || f.is_pub {
        ""
    } else {
        "internal "
    };
    // v0.0.8 fix C: non-pub gen function → eligible for fastcc.
    let cc = if linkage == "internal " {
        md.fastcc_prefix(&f.name.name)
    } else {
        ""
    };
    let iter_llvm = llvm_ty(&iter_ret_ty, types);

    write!(out, "define {}{}{} @{}(", linkage, cc, iter_llvm, f.name.name).unwrap();
    for (i, (param, (pty, _move_flag, _mut_flag, _restrict_flag))) in
        f.params.iter().zip(sig.params.iter()).enumerate()
    {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{} %{}", llvm_ty(pty, types), i as u32).unwrap();
        let _ = param;
    }
    out.push_str(") presplitcoroutine {\nentry:\n");

    // Promise alloca holds the yielded value. Unit-typed gen fns are
    // unusual (a yield-less generator), but still need a valid promise
    // slot — use a 1-byte placeholder. Yield sites store the actual T.
    let promise_ty: &str = if matches!(inner_ty, Ty::Unit) {
        "i8"
    } else {
        &inner_llvm
    };
    let promise_align: u64 = if matches!(inner_ty, Ty::Unit) {
        1
    } else {
        inner_align
    };
    out.push_str(&format!(
        "  %.coro.promise = alloca {promise_ty}, align {promise_align}\n"
    ));
    out.push_str(&format!(
        "  %.coro.id = call token @llvm.coro.id(i32 {promise_align}, ptr %.coro.promise, ptr null, ptr null)\n"
    ));
    out.push_str("  %.coro.size = call i64 @llvm.coro.size.i64()\n");
    out.push_str("  %.coro.mem = call ptr @malloc(i64 %.coro.size)\n");
    out.push_str("  %.coro.hdl = call ptr @llvm.coro.begin(token %.coro.id, ptr %.coro.mem)\n");

    let mut state = FnState::new(Ty::Unit, sigs, types, str_lits, mode, test_mode, md, tramps);
    state.return_ty = Ty::Unit;
    state.coro_promise = Some((
        ".coro.hdl".to_string(),
        inner_llvm.clone(),
        inner_align as u32,
    ));
    state.collect_moved_bindings(&f.body);

    for (i, (param, (pty, _, _, _))) in f.params.iter().zip(sig.params.iter()).enumerate() {
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types),
            i as u32,
            slot
        ));
        state.bind(&param.name.name, slot.clone(), pty.clone());
    }

    let _body_value = state.gen_block_expr(&f.body);

    // Gen fn body is Unit-typed. Fall-off and explicit `return;` both
    // branch into the final-suspend block (the consumer's next() will
    // observe coro.done = true and return Option::None).
    if !state.terminated {
        state.emit_terminator("br label %.coro.final_suspend");
    }

    state.body.push_str(".coro.final_suspend:\n");
    state
        .body
        .push_str("  %.coro.fs = call i8 @llvm.coro.suspend(token none, i1 true)\n");
    state.body.push_str("  switch i8 %.coro.fs, label %.coro.ramp_return [i8 0, label %.coro.trap i8 1, label %.coro.cleanup]\n");
    state.body.push_str(".coro.ramp_return:\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.trap:\n");
    state.body.push_str("  call void @llvm.trap()\n");
    state.body.push_str("  unreachable\n");
    state.body.push_str(".coro.cleanup:\n");
    state.body.push_str(
        "  %.coro.mem_free = call ptr @llvm.coro.free(token %.coro.id, ptr %.coro.hdl)\n",
    );
    state
        .body
        .push_str("  call void @free(ptr %.coro.mem_free)\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.end:\n");
    state.body.push_str(
        &coro_end_call_ir(),
    );
    state.body.push_str(&format!(
        "  %.coro.iter0 = insertvalue {iter_llvm} undef, ptr %.coro.hdl, 0\n"
    ));
    state
        .body
        .push_str(&format!("  ret {iter_llvm} %.coro.iter0\n"));

    for a in &state.allocas {
        out.push_str("  ");
        out.push_str(a);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

fn gen_async_function(
    out: &mut String,
    f: &Function,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
) {
    let sig = sigs.get(&f.name.name).expect("sig was collected");
    // codegen's `collect_sigs` wraps async fn sigs to Future[T] for
    // call-site lowering. The body itself needs the inner T — re-derive
    // from the parsed return-type AST.
    let inner_ty = match &f.return_type {
        Some(t) => ty_from(t, types),
        None => Ty::Unit,
    };
    let inner_llvm = llvm_ty(&inner_ty, types);
    let (inner_size, inner_align) = static_layout(&inner_ty, types).unwrap_or((8, 8));
    let future_ret_ty = sig.return_type.clone();

    let linkage = if f.name.name == "main" || f.is_pub {
        ""
    } else {
        "internal "
    };
    // v0.0.8 fix C: non-pub async function → eligible for fastcc.
    let cc = if linkage == "internal " {
        md.fastcc_prefix(&f.name.name)
    } else {
        ""
    };
    let future_llvm = llvm_ty(&future_ret_ty, types);

    // Function signature. Async fns can't be C-exports (no extern
    // pub), so we don't need the C-ABI coercion paths. They also
    // can't use the sret return path because the return value
    // (Future[T] = { *u8 }) is just one ptr — fits in a register.
    write!(out, "define {}{}{} @{}(", linkage, cc, future_llvm, f.name.name).unwrap();
    for (i, (param, (pty, _move_flag, _mut_flag, _restrict_flag))) in
        f.params.iter().zip(sig.params.iter()).enumerate()
    {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{} %{}", llvm_ty(pty, types), i as u32).unwrap();
        let _ = param;
    }
    out.push_str(") presplitcoroutine {\nentry:\n");

    // v0.0.4 Phase 1E: properly allocate the coroutine promise.
    // Previously we passed `ptr null` as the promise arg to `coro.id`
    // and then called `coro.promise(...)` to write to it, which
    // returned undefined behavior for non-trivial inner types. For
    // primitive Copy returns the resulting OOB writes happened to land
    // inside the frame's slack; for `string` (24 B) and `Vec[T]` they
    // overflowed (ASan caught it on the chained-string async test).
    //
    // The LLVM coro intrinsic contract: pass an `alloca <T>` as the
    // promise arg + its alignment as the first i32. CoroSplit hoists
    // the alloca into the heap frame at a known offset; `coro.promise`
    // returns that in-frame pointer.
    // Unit-returning async fns have no value to stash; LLVM rejects
    // `alloca void`. Use a 1-byte placeholder so the promise pointer
    // is non-null + addressable but no bytes are read/written through
    // it (Unit-return bodies never `store {inner_llvm} value, ptr ...`).
    let promise_ty: &str = if matches!(inner_ty, Ty::Unit) {
        "i8"
    } else {
        &inner_llvm
    };
    let promise_align: u64 = if matches!(inner_ty, Ty::Unit) {
        1
    } else {
        inner_align
    };
    out.push_str(&format!(
        "  %.coro.promise = alloca {promise_ty}, align {promise_align}\n"
    ));
    out.push_str(&format!(
        "  %.coro.id = call token @llvm.coro.id(i32 {promise_align}, ptr %.coro.promise, ptr null, ptr null)\n"
    ));
    out.push_str("  %.coro.size = call i64 @llvm.coro.size.i64()\n");
    out.push_str("  %.coro.mem = call ptr @malloc(i64 %.coro.size)\n");
    out.push_str("  %.coro.hdl = call ptr @llvm.coro.begin(token %.coro.id, ptr %.coro.mem)\n");

    // Build a FnState configured for async-body emission. The body's
    // `return X` will store X to the coroutine promise and branch to
    // `%.coro.final_suspend`.
    let mut state = FnState::new(
        inner_ty.clone(),
        sigs,
        types,
        str_lits,
        mode,
        test_mode,
        md,
        tramps,
    );
    state.return_ty = inner_ty.clone();
    state.coro_promise = Some((
        ".coro.hdl".to_string(),
        inner_llvm.clone(),
        inner_align as u32,
    ));
    state.body.push_str(""); // body collects after entry
    state.collect_moved_bindings(&f.body);

    // Bind params to allocas (mirrors the non-async path's simple
    // value-passed handling).
    let body_start_offset = state.body.len();
    for (i, (param, (pty, _, _, _))) in f.params.iter().zip(sig.params.iter()).enumerate() {
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types),
            i as u32,
            slot
        ));
        state.bind(&param.name.name, slot.clone(), pty.clone());
    }

    let _body_value = state.gen_block_expr(&f.body);
    let _ = body_start_offset;

    // If the body fell off the end without an explicit `return`, that's
    // a unit-typed async fn — handle the same way: store an undef of
    // inner type then suspend. (Sema rejects async fns missing return
    // for non-unit Ty.)
    if !state.terminated {
        // Unit-returning async fns: no value to store; just fall into
        // final_suspend. (Sema rejects async fns missing `return` for
        // non-Unit T, so this is the unit fall-off case.)
        if !matches!(inner_ty, Ty::Unit) {
            let prom_ptr = state.next_tmp();
            state.emit(&format!(
                "{prom_ptr} = call ptr @llvm.coro.promise(ptr %.coro.hdl, i32 {promise_align}, i1 false)"
            ));
            state.emit(&format!(
                "store {inner_llvm} undef, ptr {prom_ptr}, align {promise_align}"
            ));
        }
        state.emit_terminator("br label %.coro.final_suspend");
    }

    // Emit the suspend / cleanup / end blocks. These live after the
    // user body — every `return X` branched here from inside.
    //
    // v0.0.5 Slice 4F: notify any registered awaiter that this coro
    // is about to complete. The call happens BEFORE the final suspend
    // so the awaiter is enqueued before control leaves this frame;
    // the next `drain_pending` round in `block_on` picks it up and
    // resumes it. By that point `coro.done(self_hdl)` reads true
    // (the final suspend has executed), so the awaiter's await loop
    // extracts cleanly.
    state.body.push_str(".coro.final_suspend:\n");
    state
        .body
        .push_str("  call void @stdlib_reactor_notify_completed_v1(ptr %.coro.hdl)\n");
    state
        .body
        .push_str("  %.coro.fs = call i8 @llvm.coro.suspend(token none, i1 true)\n");
    state.body.push_str("  switch i8 %.coro.fs, label %.coro.ramp_return [i8 0, label %.coro.trap i8 1, label %.coro.cleanup]\n");
    state.body.push_str(".coro.ramp_return:\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.trap:\n");
    state.body.push_str("  call void @llvm.trap()\n");
    state.body.push_str("  unreachable\n");
    state.body.push_str(".coro.cleanup:\n");
    state.body.push_str(
        "  %.coro.mem_free = call ptr @llvm.coro.free(token %.coro.id, ptr %.coro.hdl)\n",
    );
    state
        .body
        .push_str("  call void @free(ptr %.coro.mem_free)\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.end:\n");
    state.body.push_str(
        &coro_end_call_ir(),
    );
    // Wrap the handle in Future[T].
    state.body.push_str(&format!(
        "  %.coro.future0 = insertvalue {future_llvm} undef, ptr %.coro.hdl, 0\n"
    ));
    state
        .body
        .push_str(&format!("  ret {future_llvm} %.coro.future0\n"));

    // Allocas live in the function preamble before user body.
    for a in &state.allocas {
        out.push_str("  ");
        out.push_str(a);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
    let _ = inner_size; // silence unused
}

/// Emit a method as a regular LLVM function with a mangled name `@Type.method`.
/// Receivers compile to LLVM parameters:
/// - `self` (value): a struct-typed parameter, stored in an alloca
/// - `self` / `mut self`: a `ptr` parameter, bound directly (no alloca)
/// v0.0.5 Phase 2C: emit an inherent method declared on an enum type
/// (`impl EnumName { fn foo(self, ...) -> T { ... } }`). Mirror of
/// `gen_method` adapted for enum receivers — same coroutine dispatch
/// for `is_gen`, same calling-convention / linkage / sret rules.
/// Enums skip the destructor-special-case (drops are struct-only today,
/// gated by sema's E0338).
fn gen_enum_method(
    out: &mut String,
    enum_id: EnumId,
    m: &Method,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
) {
    // Recover the enum name via the reverse-lookup table — EnumInfo
    // doesn't carry it directly (Phase-1F-style reverse lookup, same
    // pattern other enum-aware codegen helpers use).
    let enum_name = types
        .enum_by_name
        .iter()
        .find_map(|(n, eid)| {
            if *eid == enum_id {
                Some(n.clone())
            } else {
                None
            }
        })
        .expect("enum has registered name");
    let sig = types.enum_defs[enum_id.0 as usize]
        .methods
        .get(&m.name.name)
        .expect("sig was collected")
        .clone();
    let mangled = mangle(&enum_name, &m.name.name);

    // is_gen path: same coroutine lowering as gen_gen_method but receiver
    // is the enum's address. Reuse the gen-method coroutine helper with
    // a struct_id stand-in if needed; for simplicity inline a small
    // mirror that's enum-aware.
    if m.is_gen {
        gen_gen_enum_method(
            out, enum_id, &enum_name, m, &sig, sigs, types, str_lits, mode, test_mode, md, tramps,
        );
        return;
    }

    let return_ty = sig.return_type.clone();
    let enum_ty = Ty::Enum(enum_id);

    let linkage = if m.is_pub { "" } else { "internal " };
    // v0.0.8 fix C: non-pub enum method → eligible for fastcc.
    let cc = if linkage == "internal " {
        md.fastcc_prefix(&mangled)
    } else {
        ""
    };
    let uses_sret = return_passes_by_sret_widened(&return_ty, types);
    let return_ty_str = if uses_sret {
        "void".to_string()
    } else {
        llvm_ty(&return_ty, types)
    };
    write!(out, "define {}{}{} @{}(", linkage, cc, return_ty_str, mangled).unwrap();
    let mut llvm_idx: u32 = 0;
    let mut first = true;
    if uses_sret {
        let (sz, al) = static_layout(&return_ty, types).expect("sret return type has layout");
        let ret_ty_inner = llvm_ty(&return_ty, types);
        write!(
            out,
            "ptr sret({}) noalias nonnull noundef writable dereferenceable({}) align {} %{}",
            ret_ty_inner, sz, al, llvm_idx
        )
        .unwrap();
        llvm_idx += 1;
        first = false;
    }
    if let Some(rcv) = sig.receiver {
        let (mv, mu) = match rcv {
            Receiver::Read => (false, false),
            Receiver::Mut => (false, true),
            Receiver::Move => (true, true),
        };
        if !first {
            out.push_str(", ");
        }
        let attrs = param_attrs(&enum_ty, mv, mu, false, true, types);
        if attrs.is_empty() {
            write!(out, "ptr %{llvm_idx}").unwrap();
        } else {
            write!(out, "ptr {} %{llvm_idx}", attrs).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    for (_param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        if !first {
            out.push_str(", ");
        }
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        let attrs = param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, by_ptr, types);
        let base_ty = if by_ptr {
            "ptr".to_string()
        } else {
            llvm_ty(pty, types)
        };
        if attrs.is_empty() {
            write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
        } else {
            write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    out.push_str(") {\n");
    out.push_str("entry:\n");

    let mut state = FnState::new(
        return_ty.clone(),
        sigs,
        types,
        str_lits,
        mode,
        test_mode,
        md,
        tramps,
    );
    state.collect_moved_bindings(&m.body);
    let mut next_idx: u32 = 0;
    if uses_sret {
        state.sret_slot = Some("%0".to_string());
        next_idx = 1;
    }
    if let Some(_rcv) = sig.receiver {
        let recv_name = format!("%{}", next_idx);
        state.bind("self", recv_name, Ty::Enum(enum_id));
        next_idx += 1;
    }
    for (param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        if by_ptr {
            state.bind(&param.name.name, format!("%{}", next_idx), pty.clone());
        } else {
            let slot = state.alloca_named(&param.name.name, pty.clone());
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                llvm_ty(pty, types),
                next_idx,
                slot
            ));
            state.bind(&param.name.name, slot.clone(), pty.clone());
        }
        next_idx += 1;
    }

    let _ = state.gen_block_expr(&m.body);

    // If the body is value-producing (non-Unit return), make sure we
    // emit a final ret. Otherwise fall through to ret void.
    if !state.terminated {
        if uses_sret {
            state.emit_terminator("ret void");
        } else if matches!(return_ty, Ty::Unit) {
            state.emit_terminator("ret void");
        } else {
            // Unreachable in well-formed bodies; sema enforces explicit
            // return for non-Unit. Trap to avoid UB on malformed input.
            state.emit_terminator("unreachable");
        }
    }

    for a in &state.allocas {
        out.push_str("  ");
        out.push_str(a);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

/// v0.0.5 Phase 2C: lower a `gen` method on an enum to an LLVM
/// coroutine returning `Iterator[T]`. Mirror of `gen_gen_method` for
/// struct receivers.
fn gen_gen_enum_method(
    out: &mut String,
    enum_id: EnumId,
    enum_name: &str,
    m: &Method,
    sig: &MethodInfo,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
) {
    let mangled = mangle(enum_name, &m.name.name);
    let inner_ty = match &m.return_type {
        Some(t) => ty_from(t, types),
        None => Ty::Unit,
    };
    let inner_llvm = llvm_ty(&inner_ty, types);
    let (_inner_size, inner_align) = static_layout(&inner_ty, types).unwrap_or((8, 8));
    let iter_ret_ty = sig.return_type.clone();
    let iter_llvm = llvm_ty(&iter_ret_ty, types);

    let linkage = if m.is_pub { "" } else { "internal " };
    // v0.0.8 fix C: non-pub gen enum method → eligible for fastcc.
    let cc = if linkage == "internal " {
        md.fastcc_prefix(&mangled)
    } else {
        ""
    };
    let enum_ty = Ty::Enum(enum_id);

    write!(out, "define {}{}{} @{}(", linkage, cc, iter_llvm, mangled).unwrap();
    let mut llvm_idx: u32 = 0;
    let mut first = true;
    if let Some(rcv) = sig.receiver {
        let (mv, mu) = match rcv {
            Receiver::Read => (false, false),
            Receiver::Mut => (false, true),
            Receiver::Move => (true, true),
        };
        if !first {
            out.push_str(", ");
        }
        let attrs = param_attrs(&enum_ty, mv, mu, false, true, types);
        if attrs.is_empty() {
            write!(out, "ptr %{llvm_idx}").unwrap();
        } else {
            write!(out, "ptr {} %{llvm_idx}", attrs).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    for (_param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        if !first {
            out.push_str(", ");
        }
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        let attrs = param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, by_ptr, types);
        let base_ty = if by_ptr {
            "ptr".to_string()
        } else {
            llvm_ty(pty, types)
        };
        if attrs.is_empty() {
            write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
        } else {
            write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    out.push_str(") presplitcoroutine {\nentry:\n");

    let promise_ty: &str = if matches!(inner_ty, Ty::Unit) {
        "i8"
    } else {
        &inner_llvm
    };
    let promise_align: u64 = if matches!(inner_ty, Ty::Unit) {
        1
    } else {
        inner_align
    };
    out.push_str(&format!(
        "  %.coro.promise = alloca {promise_ty}, align {promise_align}\n"
    ));
    out.push_str(&format!(
        "  %.coro.id = call token @llvm.coro.id(i32 {promise_align}, ptr %.coro.promise, ptr null, ptr null)\n"
    ));
    out.push_str("  %.coro.size = call i64 @llvm.coro.size.i64()\n");
    out.push_str("  %.coro.mem = call ptr @malloc(i64 %.coro.size)\n");
    out.push_str("  %.coro.hdl = call ptr @llvm.coro.begin(token %.coro.id, ptr %.coro.mem)\n");

    let mut state = FnState::new(Ty::Unit, sigs, types, str_lits, mode, test_mode, md, tramps);
    state.return_ty = Ty::Unit;
    state.coro_promise = Some((
        ".coro.hdl".to_string(),
        inner_llvm.clone(),
        inner_align as u32,
    ));
    state.collect_moved_bindings(&m.body);

    let mut next_idx: u32 = 0;
    if let Some(_rcv) = sig.receiver {
        state.bind("self", format!("%{}", next_idx), Ty::Enum(enum_id));
        next_idx += 1;
    }
    for (param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        if by_ptr {
            state.bind(&param.name.name, format!("%{}", next_idx), pty.clone());
        } else {
            let slot = state.alloca_named(&param.name.name, pty.clone());
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                llvm_ty(pty, types),
                next_idx,
                slot
            ));
            state.bind(&param.name.name, slot.clone(), pty.clone());
        }
        next_idx += 1;
    }

    let _ = state.gen_block_expr(&m.body);
    if !state.terminated {
        state.emit_terminator("br label %.coro.final_suspend");
    }
    state.body.push_str(".coro.final_suspend:\n");
    state
        .body
        .push_str("  %.coro.fs = call i8 @llvm.coro.suspend(token none, i1 true)\n");
    state.body.push_str("  switch i8 %.coro.fs, label %.coro.ramp_return [i8 0, label %.coro.trap i8 1, label %.coro.cleanup]\n");
    state
        .body
        .push_str(".coro.ramp_return:\n  br label %.coro.end\n");
    state
        .body
        .push_str(".coro.trap:\n  call void @llvm.trap()\n  unreachable\n");
    state.body.push_str(".coro.cleanup:\n");
    state.body.push_str(
        "  %.coro.mem_free = call ptr @llvm.coro.free(token %.coro.id, ptr %.coro.hdl)\n",
    );
    state
        .body
        .push_str("  call void @free(ptr %.coro.mem_free)\n");
    state.body.push_str("  br label %.coro.end\n");
    state.body.push_str(".coro.end:\n");
    state.body.push_str(
        &coro_end_call_ir(),
    );
    state.body.push_str(&format!(
        "  %.coro.iter0 = insertvalue {iter_llvm} undef, ptr %.coro.hdl, 0\n"
    ));
    state
        .body
        .push_str(&format!("  ret {iter_llvm} %.coro.iter0\n"));
    for a in &state.allocas {
        out.push_str("  ");
        out.push_str(a);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

fn gen_method(
    out: &mut String,
    struct_id: StructId,
    m: &Method,
    sigs: &HashMap<String, FnSig>,
    types: &TypeTable,
    str_lits: &StrLitTable,
    mode: BuildMode,
    test_mode: bool,
    md: &ModuleMetadata,
    tramps: &ThreadTrampolines,
    is_lib: bool,
) {
    // v0.0.5 Phase 2B: gen-method dispatch. Same lowering as
    // `gen_gen_function` for free fns but adapted to method receiver
    // + parameter shape.
    if m.is_gen {
        gen_gen_method(
            out, struct_id, m, sigs, types, str_lits, mode, test_mode, md, tramps,
        );
        return;
    }
    // v0.0.5 Phase 4 Slice 4B: async-method dispatch. Same coroutine
    // lowering as `gen_async_function` (Future[T] return, coro.id/begin/
    // suspend/end) but with method-shaped receiver + params.
    if m.is_async {
        gen_async_method(
            out, struct_id, m, sigs, types, str_lits, mode, test_mode, md, tramps,
        );
        return;
    }
    let struct_name = types.struct_defs[struct_id.0 as usize].name.clone();
    let sig = types.struct_defs[struct_id.0 as usize]
        .methods
        .get(&m.name.name)
        .expect("sig was collected")
        .clone();
    let mangled = mangle(&struct_name, &m.name.name);

    let return_ty = sig.return_type.clone();
    let struct_ty = Ty::Struct(struct_id);

    // Function header. Both `self` and `mut self` lower to a `ptr` parameter
    // (the struct's address). The receiver kind only affects sema-level
    // mutability checks, not the LLVM signature.
    //
    // Slice 1F (v0.0.2): destructors are compiler-synthesized cold paths.
    // Apply `preserve_nonecc` (no callee-save register saves at the call
    // boundary) plus `cold` (the optimizer biases hot paths away from
    // them). Drop runs once per object at scope exit — it's the canonical
    // cold helper. `preserve_nonecc` requires clang/LLVM 17+; macOS
    // shipped that in Xcode 15.3 (Feb 2024).
    let is_drop_method = m.name.name == "drop";
    // v0.0.8 bench-gap fix C: non-drop, non-pub methods can use
    // `fastcc` (LLVM-internal register-passing cc). Drop methods stay
    // `preserve_nonecc` — fastcc can't compose with it. `pub` methods
    // have external linkage and must keep C cc for the public ABI.
    let cc_prefix = if is_drop_method {
        "preserve_nonecc "
    } else if !m.is_pub && md.is_fastcc(&mangled) {
        "fastcc "
    } else {
        ""
    };
    // `cold` on drop glue, plus any `#[inline]` LLVM attribute. (A drop
    // method is never user-marked `#[inline]`, so these don't collide.)
    let fn_attrs = format!("{}{}", if is_drop_method { " cold" } else { "" }, inline_fn_attr(&m.attributes));
    let fn_attrs = fn_attrs.as_str();
    // Phase 5 Slice 5.B: in library builds, non-`pub` methods get
    // `internal` linkage. `drop` is compiler-synthesized infrastructure —
    // not part of the public C-ABI surface even when `pub`; always
    // internal in lib mode. Executable builds keep external linkage on
    // every method (matches pre-5.B behavior).
    // v0.0.3 Slice 3D: methods also pick up internal linkage in bin
    // builds. `pub` methods stay external; `drop` (synthesized) stays
    // internal regardless of `pub`-ness.
    let linkage = if m.is_pub && !is_drop_method {
        ""
    } else {
        "internal "
    };
    // v0.0.3 Slice 1P: method signatures use sret when their return type
    // is a non-Copy aggregate (matches gen_method_call's call-site logic).
    // Without this, the signature returns by value but call sites pass a
    // sret slot → ABI mismatch → SIGSEGV / wrong values at runtime.
    let uses_sret = return_passes_by_sret_widened(&return_ty, types);
    let return_ty_str = if uses_sret {
        "void".to_string()
    } else {
        llvm_ty(&return_ty, types)
    };
    write!(
        out,
        "define {}{}{} @{}(",
        linkage, cc_prefix, return_ty_str, mangled
    )
    .unwrap();
    let mut llvm_idx: u32 = 0;
    let mut first = true;
    if uses_sret {
        let (sz, al) = static_layout(&return_ty, types).expect("sret return type has layout");
        let ret_ty_inner = llvm_ty(&return_ty, types);
        write!(
            out,
            "ptr sret({}) noalias nonnull noundef writable dereferenceable({}) align {} %{}",
            ret_ty_inner, sz, al, llvm_idx
        )
        .unwrap();
        llvm_idx += 1;
        first = false;
    }
    if let Some(rcv) = sig.receiver {
        // Slice 1A: receiver gets the full pointer attr set. Map Receiver
        // kind onto (move_, mutable) for `param_attrs`:
        //   Read => (false, false) → readonly
        //   Mut  => (false, true)  → noalias
        //   Move => (true,  true)  → noalias (callee owns; exclusive)
        let (mv, mu) = match rcv {
            Receiver::Read => (false, false),
            Receiver::Mut => (false, true),
            Receiver::Move => (true, true),
        };
        if !first {
            out.push_str(", ");
        }
        // v0.0.8 bench-gap fix A: Copy `self` (Read) passes by value,
        // mirroring the rule for non-receiver Copy params
        // (`param_passes_by_ptr` returns false for Copy structs). Passing
        // `self` by pointer for a 12-byte Copy struct like V3 forces
        // alloca → store → pass-pointer at every call site, which
        // materializes the value into memory and blocks SROA + the SLP-
        // vectorizer from seeing field-parallel arithmetic.
        //
        // Restricted to `Read`: `mut self` / `move self` must stay
        // pointer-passed even on Copy types, because the language treats
        // mutations through `mut self` as write-through to the caller's
        // place (see `phase7_generic_typed_impl_mut_self_runs` e2e test:
        // `b.set(42); b.get()` must observe the write).
        let self_by_ptr =
            !is_copy_ty(&struct_ty, types) || !matches!(rcv, Receiver::Read);
        let attrs = param_attrs(&struct_ty, mv, mu, false, self_by_ptr, types);
        if self_by_ptr {
            if attrs.is_empty() {
                write!(out, "ptr %{llvm_idx}").unwrap();
            } else {
                write!(out, "ptr {} %{llvm_idx}", attrs).unwrap();
            }
        } else {
            let base_ty = llvm_ty(&struct_ty, types);
            if attrs.is_empty() {
                write!(out, "{} %{llvm_idx}", base_ty).unwrap();
            } else {
                write!(out, "{} {} %{llvm_idx}", base_ty, attrs).unwrap();
            }
        }
        llvm_idx += 1;
        first = false;
    }
    for (_param, (pty, move_flag, mut_flag, restrict_flag)) in m.params.iter().zip(sig.params.iter()) {
        if !first {
            out.push_str(", ");
        }
        let by_ptr = param_passes_by_ptr(pty, *move_flag, *mut_flag, types);
        let attrs = param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, by_ptr, types);
        let base_ty = if by_ptr {
            "ptr".to_string()
        } else {
            llvm_ty(pty, types)
        };
        if attrs.is_empty() {
            write!(out, "{} %{}", base_ty, llvm_idx).unwrap();
        } else {
            write!(out, "{} {} %{}", base_ty, attrs, llvm_idx).unwrap();
        }
        llvm_idx += 1;
        first = false;
    }
    out.push_str(")");
    out.push_str(fn_attrs);
    out.push_str(" {\n");
    out.push_str("entry:\n");

    let mut state = FnState::new(
        return_ty.clone(),
        sigs,
        types,
        str_lits,
        mode,
        test_mode,
        md,
        tramps,
    );
    state.collect_moved_bindings(&m.body);
    // Destructors don't auto-drop their receiver — we *are* the destructor.
    if m.name.name == "drop" {
        state.in_destructor = true;
    }
    // v0.0.3 Slice 1P: when the method uses sret, %0 is the sret slot and
    // the receiver shifts to %1.
    let mut next_idx: u32 = 0;
    if uses_sret {
        state.sret_slot = Some("%0".to_string());
        next_idx = 1;
    }

    // Bind the receiver. Non-Copy receivers are pointer-passed: `self`
    // resolves directly to the SSA pointer argument. Copy `self` (Read)
    // is value-passed (v0.0.8 fix A) — spill `%{idx}` to a named slot so
    // `self.x` keeps lowering as `gep slot, 0, fld`, the place shape the
    // field-access codegen expects. Mirror of the value-passed
    // non-receiver param path below. Must match the signature decision
    // above.
    if let Some(rcv) = sig.receiver {
        let self_by_ptr =
            !is_copy_ty(&struct_ty, types) || !matches!(rcv, Receiver::Read);
        if self_by_ptr {
            let recv_name = format!("%{}", next_idx);
            state.bind("self", recv_name.clone(), struct_ty.clone());
            // `move self` consumes the receiver: the method body owns it,
            // so we register a scope-exit drop for `self` (unless we *are*
            // the destructor — see `in_destructor` above). For `self` /
            // `mut self` the receiver is non-owning (post-§2.8a
            // pointer-pass), so no drop.
            if matches!(rcv, Receiver::Move) && !state.in_destructor {
                // v0.0.14 auto field-drop: extend beyond explicit-`drop` structs
                // to any owning aggregate (owning fields / owning enum payloads).
                match &struct_ty {
                    Ty::Struct(id) if state.needs_drop(&struct_ty) => {
                        state.register_drop("self", &recv_name, *id);
                    }
                    Ty::Enum(id) if state.needs_drop(&struct_ty) => {
                        state.register_drop_kind("self", &recv_name, DropKind::Enum(*id));
                    }
                    _ => {}
                }
            }
        } else {
            // Copy by-value: spill into a slot so `self.x` reads still work.
            // No drop registration: Copy and Drop are mutually exclusive.
            let slot = state.alloca_named("self", struct_ty.clone());
            state.body.push_str(&format!(
                "  store {} %{}, ptr {}\n",
                llvm_ty(&struct_ty, types),
                next_idx,
                slot
            ));
            state.bind("self", slot, struct_ty.clone());
        }
        next_idx += 1;
    }

    // Bind non-receiver params. Pointer-passed (`mut x: T` non-Copy) bind
    // directly to the SSA argument so writes propagate to the caller's
    // place. Value-passed params copy into an alloca; `move`-marked Drop
    // params register a scope-exit drop. Non-`move` value-passed params are
    // bit-duplicates of the caller's value, so codegen does NOT register a
    // drop for them (the caller still owns the original).
    for (i, (param, (pty, move_flag, mut_flag, restrict_flag))) in
        m.params.iter().zip(sig.params.iter()).enumerate()
    {
        let idx = next_idx + i as u32;
        if param_passes_by_ptr(pty, *move_flag, *mut_flag, types) {
            state.bind(&param.name.name, format!("%{idx}"), pty.clone());
            // v0.0.5 Slice 1A: track for auto-clone-on-return (see gen_function).
            state.borrowed_params.insert(param.name.name.clone());
            continue;
        }
        let slot = state.alloca_named(&param.name.name, pty.clone());
        state.body.push_str(&format!(
            "  store {} %{}, ptr {}\n",
            llvm_ty(pty, types),
            idx,
            slot
        ));
        state.bind(&param.name.name, slot.clone(), pty.clone());
        // v0.0.5 Slice 1A: value-passed non-`move` Ty::String shares heap.
        if !*move_flag && matches!(pty, Ty::String) {
            state.borrowed_params.insert(param.name.name.clone());
        }
        if *move_flag {
            // v0.0.14 auto field-drop: a moved-in owning aggregate (struct with
            // owning fields, with or without an explicit `drop`, or a tagged
            // enum with owning payloads) is the callee's to tear down.
            match pty {
                Ty::Struct(id) if state.needs_drop(pty) => {
                    state.register_drop(&param.name.name, &slot, *id);
                }
                Ty::Enum(id) if state.needs_drop(pty) => {
                    state.register_drop_kind(&param.name.name, &slot, DropKind::Enum(*id));
                }
                _ => {}
            }
        }
    }

    state.gen_body_block(&m.body);

    if !state.terminated {
        match &return_ty {
            Ty::Unit => state.emit_terminator("ret void"),
            _ => state.emit_terminator("unreachable"),
        }
    }

    // Slice 1C: scoped alias metadata. The receiver counts as a noalias
    // param when it's `mut self` or `move self`; `self` (Read) is
    // `readonly` and does NOT participate in the scope set (two shared
    // refs may alias).
    let mut noalias_ssas: Vec<u32> = Vec::new();
    if let Some(rcv) = sig.receiver {
        // Only `mut self` / `move self` participate in the alias-scope
        // set. `self` (Read) gets `readonly` and may legitimately alias
        // another shared borrow. (Read is also the only case where Copy
        // receivers become by-value — a by-value receiver isn't a pointer
        // anyway. So the Mut/Move match already excludes the by-value
        // path.)
        if matches!(rcv, Receiver::Mut | Receiver::Move) {
            noalias_ssas.push(0);
        }
    }
    for (i, (_, (pty, mv, mu, _restrict_flag))) in m.params.iter().zip(sig.params.iter()).enumerate() {
        let idx = next_idx + i as u32;
        if param_passes_by_ptr(pty, *mv, *mu, types) && (*mv || *mu) {
            noalias_ssas.push(idx);
        }
    }
    if noalias_ssas.len() >= 2 {
        let domain = md.register_alias_domain(&mangled);
        let scopes: Vec<u32> = noalias_ssas
            .iter()
            .enumerate()
            .map(|(i, _)| md.register_alias_scope(domain, &format!("p{i}")))
            .collect();
        let this_lists: Vec<u32> = scopes
            .iter()
            .map(|&s| md.register_alias_scope_list(&[s]))
            .collect();
        let other_lists: Vec<u32> = (0..scopes.len())
            .map(|i| {
                let others: Vec<u32> = scopes
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, &s)| s)
                    .collect();
                md.register_alias_scope_list(&others)
            })
            .collect();
        let mut seed: HashMap<String, usize> = HashMap::new();
        for (idx_in_set, &ssa_idx) in noalias_ssas.iter().enumerate() {
            seed.insert(format!("%{ssa_idx}"), idx_in_set);
        }
        state.body = annotate_alias_scope_metadata(&state.body, &seed, &this_lists, &other_lists);
    }

    for line in &state.allocas {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&state.body);
    out.push_str("}\n\n");
}

/// A Drop binding registered in some scope frame. At scope exit (or `return`)
/// codegen walks these in reverse-registration order and emits a conditional
/// call to `T::drop(value_slot)` gated on `flag_slot`. The flag is initialized
/// to `true` when the binding is created and flipped to `false` whenever the
/// binding is moved out (via a `move`-marked param or `move self` receiver).
#[derive(Debug, Clone)]
struct DropEntry {
    binding_name: String,
    value_slot: String,
    flag_slot: String,
    /// Phase 8 slice 8.STR.3 follow-up (2026-05-14): which kind of
    /// Drop are we registering? `Struct(id)` is the original case —
    /// emits `call @<type>.drop(ptr)`. `String` is the owned-string
    /// case — loads the `ptr` field and emits `call @free(ptr)`.
    kind: DropKind,
    /// Slice 6BC.opt: static drop-flag specialization. When `Always`,
    /// emit an unconditional drop call at scope exit (no flag-load, no
    /// branch). `Runtime` is the Phase-5 default — the load + branch on
    /// the per-binding flag handles the MaybePartial case where the
    /// binding may or may not have been moved on different paths.
    disposition: DropDisposition,
}

#[derive(Debug, Clone, Copy)]
enum DropKind {
    Struct(StructId),
    /// Phase 8 slice 8.STR.3 follow-up: owned `string`. The drop body
    /// loads the `ptr` field from the value slot and calls `@free(ptr)`.
    /// `@free(null)` is a libc no-op so `string::new()` (which stores
    /// `null`) drops cleanly without a separate null-check.
    String,
    /// v0.0.14 enum-variant drop: a tagged enum with owning payloads. The
    /// drop body switches on the tag and tears down the active payload.
    Enum(EnumId),
}

/// Slice 6BC.opt: per-Drop-binding lowering choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropDisposition {
    /// Binding is never moved on any path through the function — the
    /// drop flag would always be true at scope exit, so we elide both
    /// the flag and the conditional branch and emit a direct
    /// `call @T.drop(ptr)`. The bigger win on common code: most Drop
    /// bindings are "let it and drop it at end" without any moves.
    Always,
    /// Phase-5 default: emit the i1 alloca + flag-load + conditional
    /// drop. Needed when the binding may be moved on some paths.
    Runtime,
}

/// A scope-exit hook. Drop and `defer` share one LIFO stack per scope
/// (see `docs/design/phase3-drop.md` §4.4). At scope exit codegen walks
/// the frame in reverse-registration order and dispatches:
///   - `Drop` → conditional call gated on the drop flag
///   - `Defer` → re-emit the deferred expression (value discarded)
#[derive(Debug, Clone)]
enum ScopeExit {
    Drop(DropEntry),
    Defer(Expr),
}

/// Phase 8 slice 8.STR.1: registry of unique string literals collected
/// by a pre-pass and emitted as `@.str.N` globals. Maps literal payload
/// (decoded UTF-8 bytes) → `(global_symbol, byte_len_without_nul)`.
type StrLitTable = HashMap<String, (String, usize)>;

struct FnState<'a> {
    body: String,
    allocas: Vec<String>,
    scopes: Vec<HashMap<String, (String, Ty)>>,
    /// Phase 8 slice 8.STR.1: shared lookup of string-literal globals.
    str_lits: &'a StrLitTable,
    /// Slice 1B: module-level metadata table. Codegen registers `!range`
    /// nodes here; the table emits its accumulated definitions at module-end.
    md: &'a ModuleMetadata,
    /// Parallel stack to `scopes`. Each frame collects scope-exit hooks
    /// (Drop bindings + `defer` statements) in registration order. At scope
    /// close codegen walks the frame in reverse and dispatches each entry.
    scope_exits: Vec<Vec<ScopeExit>>,
    /// v0.0.7 Slice 1.1: parallel stack to `scopes`. Each frame holds
    /// the `(slot, size_bytes)` of every alloca registered in that
    /// scope, so `pop_scope` can emit one `llvm.lifetime.end` per
    /// binding in reverse registration order. Allocas created before
    /// any `push_scope` (e.g. param-copy slots at fn entry) live for
    /// the whole function — those skip the lifetime intrinsics
    /// entirely so LLVM treats them as fully live (the default).
    /// Only populated when `mode == BuildMode::Release`; debug builds
    /// skip lifetime intrinsics to keep the debugger's frame walker
    /// simple.
    scope_allocas: Vec<Vec<(String, u64)>>,
    return_ty: Ty,
    sigs: &'a HashMap<String, FnSig>,
    types: &'a TypeTable,
    mode: BuildMode,
    /// Slice 6BC.opt: precomputed set of binding names that ARE moved
    /// somewhere in this function body. Computed once at FnState
    /// construction. A binding name not in this set is provably never
    /// moved, so `register_drop` picks `DropDisposition::Always`.
    moved_bindings: std::collections::HashSet<String>,
    /// v0.0.3 Slice 3C: SSA slot names of `let mut` bindings holding
    /// non-Copy types. Each gets its own `!alias.scope` after body
    /// generation, paired with the param-shape scopes from Slice 1C.
    /// The borrow checker proves these locals are disjoint by virtue of
    /// being separate allocas with single-ownership lifetimes.
    noalias_local_slots: Vec<String>,
    tmp_counter: u32,
    block_counter: u32,
    terminated: bool,
    /// True iff we are currently emitting the body of a destructor (a method
    /// named `drop`). The receiver `self` of a destructor is *not* registered
    /// as a Drop binding — running drop at end of drop would recurse. Other
    /// local Drop bindings inside the destructor body still register normally.
    in_destructor: bool,
    /// Slice 5ATTR.4: `assert` lowering depends on whether we're emitting a
    /// `cpc test` binary. In test mode the trap is replaced by a write to
    /// `@cpc_test_failed` so the driver's `main` can read which test failed
    /// without unwinding. In normal builds (false), `assert` traps.
    test_mode: bool,
    /// Slice 4-end: stack of `(continue_label, break_label)` for the
    /// enclosing loops. `break` jumps to `break_label`; `continue` jumps
    /// to `continue_label` (the loop's back-edge / cond-check / increment
    /// trampoline). Pushed when entering a loop body, popped on exit.
    loop_labels: Vec<(String, String)>,
    /// Slice 1E (v0.0.2): single-use hint set by `StmtKind::Return` when the
    /// statement is `return foo(args);` and the call qualifies for
    /// `musttail` (return type matches enclosing fn; no Drop/defer entries
    /// pending; non-variadic; default CC). gen_named_call consults this
    /// flag, emits `musttail call`, and clears it.
    pending_musttail: bool,
    /// Slice 1E: enclosing function's parameter types in declaration order.
    /// `musttail` requires the *caller*'s parameter signature to match the
    /// *callee*'s — LLVM's verifier rejects musttail across mismatched
    /// arities. Filled in by `gen_function` before body codegen.
    enclosing_params: Vec<Ty>,
    /// Slice 1E: true iff this body is a free function (eligible for
    /// `musttail`). Methods carry a receiver in their LLVM signature, so
    /// even matching the call's arg list isn't enough — the receiver
    /// position would mismatch. Disable musttail in method bodies to keep
    /// the predicate simple. Set by `gen_function`; defaults false.
    tail_call_eligible: bool,
    /// v0.0.8 fix C: true iff the enclosing function itself is emitted
    /// with the `fastcc` calling convention. LLVM's musttail verifier
    /// requires caller and callee to share a cc — when this is set, the
    /// musttail predicate only fires if the callee is also fastcc;
    /// when unset, the callee must also be default-cc.
    enclosing_is_fastcc: bool,
    /// Slice 1D (v0.0.2): SSA name of this fn's sret parameter (the
    /// caller-allocated result slot) when `return_passes_by_sret` fires.
    /// `StmtKind::Return` consults it: `Some(slot)` → store the value to
    /// the slot and `ret void`; `None` → emit `ret <ty> <val>` as usual.
    sret_slot: Option<String>,
    /// Phase 5 Slice 5.D: when emitting a `pub extern fn` whose return
    /// type lowers to a coerced C-ABI integer class (≤8 → i64, 9..16 →
    /// `[2 x i64]`), `StmtKind::Return` packs the value through an
    /// alloca and emits `ret <coerced>` instead of `ret <original>`.
    /// `None` means no coercion is needed (Direct return).
    coerce_ret: Option<String>,
    /// v0.0.3 Phase 5 Slice 5B: shared registry of per-O trampolines
    /// requested by `__cplus_thread_spawn` call sites within this
    /// function's body. After all function bodies are emitted, the
    /// registry walks its set and writes one trampoline definition
    /// per unique O.
    tramps: &'a ThreadTrampolines,
    /// v0.0.3 Phase 5 Slice 5E.3: set when emitting an `async fn`
    /// body. Carries the coroutine handle SSA name, the inner T's
    /// LLVM type, and T's alignment. `StmtKind::Return` consults this:
    /// when present, `return X` lowers to "store X to coro.promise
    /// then `br .coro.final_suspend`" instead of the usual `ret X`.
    coro_promise: Option<(String, String, u32)>,
    /// v0.0.5 Slice 1A: names of parameters bound via shared-borrow ABI
    /// (passed as `ptr readonly` with the caller still owning the value).
    /// `StmtKind::Return` consults this to detect `return X` where X is
    /// borrowed-not-owned — the body would otherwise hand the caller's
    /// pointer back, and the caller would then double-free (caller's
    /// original binding + caller's result binding both Drop the same
    /// heap). Closes the long-open `fn echo(x: string) -> string { return x; }`
    /// runtime bug documented in plan.md Slice 1A. The fix: when this set
    /// contains the returned ident and the return type has heap-owned
    /// Drop semantics (currently `Ty::String`), emit a deep clone so the
    /// caller's result is an independent heap allocation.
    borrowed_params: std::collections::HashSet<String>,
    /// v0.0.8 bench-gap finding 1: per-expression field-read memo.
    /// Maps `(local_binding_name, field_name)` to the SSA name of the
    /// already-loaded field value. Hit when the same field appears
    /// twice in one expression (e.g. `v.x * v.x` in a Vec3 dot
    /// product). Cleared at every statement boundary and on any
    /// operation that could mutate a local (call, assignment) — the
    /// goal is to keep the cache valid only within a pure expression.
    ///
    /// Bench impact: closes the May 17 → today raytracer NEON 2-lane
    /// regression. Before the cache, `v.x * v.x` emitted two GEPs +
    /// two loads + one fmul; that interleaved-duplicate-load pattern
    /// defeated the SLP-vectorizer. With the cache it emits one GEP
    /// + one load + `fmul %v.x, %v.x` — the standard adjacent-load
    /// pattern the vectorizer recognizes.
    field_load_cache: std::collections::HashMap<(String, String), (String, Ty)>,
}

impl<'a> FnState<'a> {
    fn new(
        return_ty: Ty,
        sigs: &'a HashMap<String, FnSig>,
        types: &'a TypeTable,
        str_lits: &'a StrLitTable,
        mode: BuildMode,
        test_mode: bool,
        md: &'a ModuleMetadata,
        tramps: &'a ThreadTrampolines,
    ) -> Self {
        Self {
            body: String::new(),
            allocas: Vec::new(),
            scopes: vec![HashMap::new()],
            scope_exits: vec![Vec::new()],
            scope_allocas: vec![Vec::new()],
            return_ty,
            sigs,
            types,
            str_lits,
            md,
            mode,
            moved_bindings: std::collections::HashSet::new(),
            noalias_local_slots: Vec::new(),
            tmp_counter: 0,
            block_counter: 0,
            terminated: false,
            in_destructor: false,
            test_mode,
            loop_labels: Vec::new(),
            pending_musttail: false,
            enclosing_params: Vec::new(),
            tail_call_eligible: false,
            enclosing_is_fastcc: false,
            sret_slot: None,
            coerce_ret: None,
            tramps,
            coro_promise: None,
            borrowed_params: std::collections::HashSet::new(),
            field_load_cache: std::collections::HashMap::new(),
        }
    }

    /// v0.0.8 bench-gap finding 1: invalidate the per-expression
    /// field-read memo. Called at statement boundaries and before
    /// every potentially-mutating operation (function call,
    /// assignment, method call). The cache only ever holds reads
    /// since the last invalidation, so a stale value cannot leak
    /// past one of these boundaries.
    fn invalidate_field_load_cache(&mut self) {
        self.field_load_cache.clear();
    }

    /// Slice 6BC.opt: scan the function body for `move`-position
    /// argument bindings. Returns a set of every binding name that is
    /// moved somewhere in the body. Used by `register_drop` to pick
    /// `Always` (never-moved) vs `Runtime` (may-be-moved) drop
    /// lowering. The walk is purely syntactic — it doesn't need
    /// type information because the callee's signature tells us
    /// which arg positions are `move`. We consult `sigs` to know
    /// each callee's `move_` flags.
    fn collect_moved_bindings(&mut self, body: &Block) {
        let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
        scan_moves_in_block(body, self.sigs, self.types, &mut set);
        self.moved_bindings = set;
    }

    fn lty(&self, ty: &Ty) -> String {
        llvm_ty(ty, self.types)
    }

    // ---- counters ----

    fn next_tmp(&mut self) -> String {
        self.tmp_counter += 1;
        format!("%t{}", self.tmp_counter)
    }

    fn next_block_label(&mut self) -> String {
        self.block_counter += 1;
        format!("bb{}", self.block_counter)
    }

    // ---- block / instruction emission ----

    fn emit(&mut self, s: &str) {
        if self.terminated {
            return;
        }
        self.body.push_str("  ");
        self.body.push_str(s);
        self.body.push('\n');
    }

    fn emit_terminator(&mut self, s: &str) {
        if self.terminated {
            return;
        }
        self.body.push_str("  ");
        self.body.push_str(s);
        self.body.push('\n');
        self.terminated = true;
    }

    fn open_block(&mut self, label: &str) {
        // Ensure the previous block has a terminator. Connect by `br` if not.
        if !self.terminated {
            self.body.push_str(&format!("  br label %{label}\n"));
        }
        self.body.push('\n');
        self.body.push_str(&format!("{label}:\n"));
        self.terminated = false;
        // v0.0.8 bench-gap finding 1 follow-up: SSA values are
        // basic-block-local in the dominance sense. An SSA name
        // defined in one block does not dominate a use in a parallel
        // arm of an if-expression or after a `br` merge — LLVM
        // rejects the IR with "Instruction does not dominate all
        // uses!". The field-read memo only ever stores names defined
        // in the previously-open block, so clearing it at every
        // block-open call keeps the cache strictly intra-block.
        self.invalidate_field_load_cache();
    }

    fn alloca_named(&mut self, name_hint: &str, ty: Ty) -> String {
        // Uniquify across the function so the same source-level name in
        // different scopes (e.g. a function param `s` and a match-arm
        // payload binding `s`) gets distinct LLVM SSA names. Bump the
        // anonymous counter for the suffix to keep names deterministic.
        self.tmp_counter += 1;
        let slot = format!("%{}.addr{}", sanitize(name_hint), self.tmp_counter);
        self.allocas
            .push(format!("{slot} = alloca {}", self.lty(&ty)));
        self.bracket_lifetime(&slot, &ty);
        slot
    }

    fn alloca_anon(&mut self, ty: Ty) -> String {
        self.tmp_counter += 1;
        let slot = format!("%a{}", self.tmp_counter);
        self.allocas
            .push(format!("{slot} = alloca {}", self.lty(&ty)));
        self.bracket_lifetime(&slot, &ty);
        slot
    }

    /// Phase 5 Slice 5.D: alloca a slot whose LLVM type is given as a raw
    /// string (e.g. `i64`, `[2 x i64]`) with an explicit alignment. Used
    /// by C-ABI param coercion where the alloca's size + align must
    /// match the *coerced* type (which is at least as large as the
    /// original struct), even though the binding's logical type is the
    /// original C+ struct.
    fn alloca_named_raw(&mut self, name_hint: &str, llvm_ty_str: &str, align: u64) -> String {
        self.tmp_counter += 1;
        let slot = format!("%{}.addr{}", sanitize(name_hint), self.tmp_counter);
        self.allocas
            .push(format!("{slot} = alloca {llvm_ty_str}, align {align}"));
        // Raw-LLVM-typed allocas (C-ABI param coercion slots) carry no
        // Ty we can size statically, so they skip the lifetime
        // bracketing. They live for the full function anyway — the
        // coerced bits are read once at fn entry and never revisited.
        slot
    }

    /// v0.0.7 Slice 1.2: emit a `load` instruction with the right TBAA
    /// tag for `ty`. Centralized so primitive-typed loads pick up the
    /// `!tbaa !N` access tag, which lets LLVM's alias analysis prove
    /// that a load of `*i32` and a load of `*f32` (e.g. in a Vec3 vs
    /// Sphere mixed-field hot loop) don't alias. Aggregate types get
    /// no tag — `tbaa_tag_for` returns `None` for those and LLVM
    /// falls back to may-alias-anything, the conservative default.
    fn gen_load(&mut self, tmp: &str, ty: &Ty, ptr: &str) {
        let lty = self.lty(ty);
        match self.md.tbaa_tag_for(ty, self.types) {
            Some(id) => self.emit(&format!("{tmp} = load {lty}, ptr {ptr}, !tbaa !{id}")),
            None => self.emit(&format!("{tmp} = load {lty}, ptr {ptr}")),
        }
    }

    /// v0.0.7 Slice 1.2: emit a `store` instruction with the right
    /// TBAA tag for `ty`. Mirror of `gen_load`.
    fn gen_store(&mut self, ty: &Ty, val: &str, ptr: &str) {
        let lty = self.lty(ty);
        match self.md.tbaa_tag_for(ty, self.types) {
            Some(id) => self.emit(&format!("store {lty} {val}, ptr {ptr}, !tbaa !{id}")),
            None => self.emit(&format!("store {lty} {val}, ptr {ptr}")),
        }
    }

    /// v0.0.7 Slice 1.1: emit `llvm.lifetime.start` inline at the
    /// alloca's source position and register the slot for matching
    /// `lifetime.end` at scope close. Gated on `BuildMode::Release`
    /// (debug builds skip lifetime intrinsics to keep lldb's frame
    /// walker simple) and on being inside a non-bottom scope frame
    /// (allocas before the body's first `push_scope` — e.g. param
    /// copy slots — live for the whole function and need no
    /// brackets, which is the default LLVM behavior).
    fn bracket_lifetime(&mut self, slot: &str, ty: &Ty) {
        if !matches!(self.mode, BuildMode::Release) {
            return;
        }
        if self.scope_allocas.len() <= 1 {
            // Bottom (function-wide) frame: leave the alloca live
            // for the whole function.
            return;
        }
        let Some((size, _)) = static_layout(ty, self.types) else {
            return;
        };
        if size == 0 {
            return;
        }
        self.body.push_str(&format!(
            "  call void @llvm.lifetime.start.p0(i64 {size}, ptr {slot})\n"
        ));
        self.scope_allocas
            .last_mut()
            .unwrap()
            .push((slot.to_string(), size));
    }

    // ---- locals / scopes ----

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.scope_exits.push(Vec::new());
        self.scope_allocas.push(Vec::new());
    }

    /// Close the innermost scope, emitting scope-exit hooks (Drop calls and
    /// `defer` expressions) for everything registered in this scope, in
    /// reverse registration order. If the current block is already terminated
    /// (e.g. by an early `return`) the hooks are skipped — the `return` path
    /// already emitted them.
    fn pop_scope(&mut self) {
        if !self.terminated {
            let frame = self.scope_exits.last().cloned().unwrap_or_default();
            for entry in frame.iter().rev() {
                self.emit_scope_exit(entry);
            }
            // v0.0.7 Slice 1.1: emit `llvm.lifetime.end` for each alloca
            // registered in this scope, in reverse registration order.
            // Runs *after* drop hooks (which read the slot's stored
            // value) so the lifetime.end doesn't poison those reads.
            // On the terminated branch the `ret` follows immediately and
            // the stack frame is destroyed wholesale — lifetime.end is
            // unnecessary there.
            let allocas = self.scope_allocas.last().cloned().unwrap_or_default();
            for (slot, size) in allocas.iter().rev() {
                self.body.push_str(&format!(
                    "  call void @llvm.lifetime.end.p0(i64 {size}, ptr {slot})\n"
                ));
            }
        }
        self.scopes.pop();
        self.scope_exits.pop();
        self.scope_allocas.pop();
    }

    fn bind(&mut self, name: &str, slot: String, ty: Ty) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (slot, ty));
    }

    fn lookup(&self, name: &str) -> Option<&(String, Ty)> {
        for scope in self.scopes.iter().rev() {
            if let Some(entry) = scope.get(name) {
                return Some(entry);
            }
        }
        None
    }

    // ---- Drop registration + emission ----

    /// Register a Drop binding in the current scope. Slice 6BC.opt
    /// picks the lowering disposition based on whether the binding is
    /// moved anywhere in the function:
    ///   - **Always** (binding NOT in `moved_bindings`): no flag at
    ///     all. Scope-exit emits a direct unconditional drop call.
    ///     Saves the alloca, the initial store, the flag-load, the
    ///     conditional branch, and the post-drop label.
    ///   - **Runtime** (binding IS in `moved_bindings`): allocate the
    ///     `i1` drop flag, init to `true`, record for runtime check.
    ///
    /// Returns the flag slot string. For `Always` dispositions the
    /// returned string is a placeholder that's never used (callers
    /// that flip the flag via `find_drop_flag` would skip dropped
    /// bindings naturally — but the precondition is no moves happen).
    fn register_drop(
        &mut self,
        binding_name: &str,
        value_slot: &str,
        struct_id: StructId,
    ) -> String {
        self.register_drop_kind(binding_name, value_slot, DropKind::Struct(struct_id))
    }

    /// v0.0.14 auto field-drop: register a scope-exit drop for a binding of any
    /// owning type — `string`, a struct with owning fields (with or without an
    /// explicit `drop`), or a tagged enum with owning payloads. No-op for
    /// trivially-droppable types. Used where the binding's full `Ty` is known
    /// (lets); the param/self sites that predate enums inline their own match.
    fn register_value_drop(&mut self, binding_name: &str, value_slot: &str, ty: &Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let kind = match ty {
            Ty::String => DropKind::String,
            Ty::Struct(id) => DropKind::Struct(*id),
            Ty::Enum(id) => DropKind::Enum(*id),
            // Array locals aren't drop-registered (pre-existing gap); array
            // *fields* are reached via struct field recursion at drop time.
            _ => return,
        };
        self.register_drop_kind(binding_name, value_slot, kind);
    }

    fn register_drop_kind(
        &mut self,
        binding_name: &str,
        value_slot: &str,
        kind: DropKind,
    ) -> String {
        let disposition = if self.moved_bindings.contains(binding_name) {
            DropDisposition::Runtime
        } else {
            DropDisposition::Always
        };
        let flag_slot = match disposition {
            DropDisposition::Runtime => {
                // Uniquify like `alloca_named`: the same source name in two
                // sibling scopes (e.g. `out` in two match arms) must not share
                // one hoisted `.drop_flag` alloca. `find_drop_flag` returns the
                // stored slot, so the suffix doesn't affect lookup.
                self.tmp_counter += 1;
                let s = format!("%{}.drop_flag{}", sanitize(binding_name), self.tmp_counter);
                self.allocas.push(format!("{s} = alloca i1"));
                // v0.0.7 Slice 1.2: drop-flag init store — bool leaf.
                self.gen_store(&Ty::Bool, "true", &s);
                s
            }
            DropDisposition::Always => {
                format!("%{}.drop_flag.unused", sanitize(binding_name))
            }
        };
        self.scope_exits
            .last_mut()
            .unwrap()
            .push(ScopeExit::Drop(DropEntry {
                binding_name: binding_name.to_string(),
                value_slot: value_slot.to_string(),
                flag_slot: flag_slot.clone(),
                kind,
                disposition,
            }));
        flag_slot
    }

    /// Register a `defer EXPR;` hook in the current scope. The expression
    /// fires at scope exit (lexical), in LIFO order with surrounding Drop
    /// calls.
    fn register_defer(&mut self, expr: Expr) {
        self.scope_exits
            .last_mut()
            .unwrap()
            .push(ScopeExit::Defer(expr));
    }

    /// Look up a Drop binding's flag slot by binding name. Walks scope
    /// frames from innermost to outermost (matches `lookup` semantics).
    fn find_drop_flag(&self, name: &str) -> Option<String> {
        for frame in self.scope_exits.iter().rev() {
            for entry in frame.iter().rev() {
                if let ScopeExit::Drop(d) = entry {
                    if d.binding_name == name {
                        return Some(d.flag_slot.clone());
                    }
                }
            }
        }
        None
    }

    /// Flip a Drop binding's flag to `false`, suppressing its scope-exit
    /// drop. Called when codegen emits a `move`-marked argument or a
    /// `move self` receiver and the source is a plain Ident.
    /// v0.0.5 Phase 1C: emit the drop call for the value at `*p_val`
    /// when T has Drop. No-op for Copy / non-Drop types. Mirrors the
    /// drop-dispatch logic in `emit_conditional_drop`'s body.
    ///
    /// Recognized Drop kinds:
    ///   - `Ty::String` → inline free of the `{ptr, len, cap}` aggregate's
    ///     `ptr` field (string's heap buffer).
    ///   - `Ty::Struct(id)` where the struct has an explicit `fn drop`
    ///     method → call `<mangled-struct-name>.drop(p_val)` with
    ///     `preserve_nonecc` to match the callee's CC.
    ///   - Anything else → no-op (Copy types and structs without Drop
    ///     don't need teardown).
    /// Emit a pointer to payload value `pi` of a tagged enum at `enum_ptr`,
    /// using byte offsets (not slot indices) so multi-payload variants with a
    /// value larger than 8 bytes lay out correctly. Used by construction, match
    /// extraction, and enum-variant drop so all three agree on layout.
    fn payload_slot_ptr(
        &mut self,
        llvm_enum: &str,
        enum_ptr: &str,
        payload_tys: &[Ty],
        pi: usize,
    ) -> String {
        let off = enum_payload_byte_offset(payload_tys, pi, self.types);
        let base = self.next_tmp();
        self.emit(&format!(
            "{base} = getelementptr inbounds {llvm_enum}, ptr {enum_ptr}, i32 0, i32 1, i64 0"
        ));
        if off == 0 {
            return base;
        }
        let slot = self.next_tmp();
        self.emit(&format!(
            "{slot} = getelementptr inbounds i8, ptr {base}, i64 {off}"
        ));
        slot
    }

    /// v0.0.14 auto field-drop: does a value of this type need teardown?
    /// Mirrors sema's `ty_carries_drop` so codegen registers/emits drops for
    /// exactly the types the front end treats as owning. Recursive and
    /// cycle-safe (by-value containment is acyclic; Vec/Box break recursion
    /// via raw-pointer fields).
    fn needs_drop(&self, ty: &Ty) -> bool {
        match ty {
            Ty::String => true,
            Ty::Struct(id) => {
                let def = &self.types.struct_defs[id.0 as usize];
                def.is_drop || def.fields.iter().any(|f| self.needs_drop(&f.1))
            }
            Ty::Enum(id) => {
                let info = &self.types.enum_defs[id.0 as usize];
                info.is_tagged
                    && info
                        .variant_payloads
                        .iter()
                        .any(|p| p.iter().any(|t| self.needs_drop(t)))
            }
            Ty::Array(elem, _) => self.needs_drop(elem),
            _ => false,
        }
    }

    /// Emit the teardown for a value of type `ty` stored at pointer `p_val`.
    ///
    /// - `string` → free its heap buffer (`ptr` field of `{ptr,len,cap}`).
    /// - `Struct` → run the user `drop` (if any) first, then auto-drop owning
    ///   fields in **reverse declaration order** (construct forward, tear down
    ///   backward).
    /// - tagged `Enum` → switch on the tag and drop the active variant's owning
    ///   payload values (v0.0.14 enum-variant drop; was E0344-forbidden).
    /// - `Array` → drop each element.
    ///
    /// Container *elements* behind a raw pointer (a `Vec`'s heap buffer) are the
    /// container's own `drop` responsibility, not auto field-drop's — a `Vec[T]`
    /// field is dropped by calling `Vec::drop` (frees the buffer); dropping each
    /// `T` element is a separate Vec enhancement.
    fn gen_drop_in_place(&mut self, ty: &Ty, p_val: &str) {
        match ty {
            Ty::String => {
                let pp = self.next_tmp();
                self.emit(&format!(
                    "{pp} = getelementptr inbounds {{ ptr, i64, i64 }}, ptr {p_val}, i32 0, i32 0"
                ));
                let pv = self.next_tmp();
                self.gen_load(&pv, &Ty::RawPtr(Box::new(Ty::Unit)), &pp);
                self.emit(&format!("call void @free(ptr {pv})"));
            }
            Ty::Struct(id) => {
                let struct_def = &self.types.struct_defs[id.0 as usize];
                // 1. Run the user destructor first, while fields are still
                //    live and readable inside it.
                if struct_def.methods.contains_key("drop") {
                    let struct_name = struct_def.name.clone();
                    self.emit(&format!(
                        "call preserve_nonecc void @{struct_name}.drop(ptr {p_val})"
                    ));
                }
                // 2. Auto-drop owning fields, reverse declaration order.
                let llvm_ty = self.lty(ty);
                let fields: Vec<(usize, Ty)> = self.types.struct_defs[id.0 as usize]
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (i, f.1.clone()))
                    .collect();
                for (i, fty) in fields.into_iter().rev() {
                    if !self.needs_drop(&fty) {
                        continue;
                    }
                    let fp = self.next_tmp();
                    self.emit(&format!(
                        "{fp} = getelementptr inbounds {llvm_ty}, ptr {p_val}, i32 0, i32 {i}"
                    ));
                    self.gen_drop_in_place(&fty, &fp);
                }
            }
            Ty::Enum(id) => {
                let info = self.types.enum_defs[id.0 as usize].clone();
                if !info.is_tagged {
                    return;
                }
                // Variants (by tag) whose payload needs any teardown.
                let drop_variants: Vec<(usize, Vec<Ty>)> = info
                    .variant_payloads
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.iter().any(|t| self.needs_drop(t)))
                    .map(|(v, p)| (v, p.clone()))
                    .collect();
                if drop_variants.is_empty() {
                    return;
                }
                let llvm_enum = self.lty(ty);
                // Load the tag (field 0).
                let tag_ptr = self.next_tmp();
                self.emit(&format!(
                    "{tag_ptr} = getelementptr inbounds {llvm_enum}, ptr {p_val}, i32 0, i32 0"
                ));
                let tag_val = self.next_tmp();
                self.gen_load(&tag_val, &Ty::I32, &tag_ptr);
                // switch tag -> per-variant drop block; default -> merge.
                let merge_lbl = self.next_block_label();
                let mut blocks: Vec<String> = Vec::with_capacity(drop_variants.len());
                let mut cases = String::new();
                for (v, _) in &drop_variants {
                    let lbl = self.next_block_label();
                    cases.push_str(&format!("    i32 {v}, label %{lbl}\n"));
                    blocks.push(lbl);
                }
                self.emit_terminator(&format!(
                    "switch i32 {tag_val}, label %{merge_lbl} [\n{cases}  ]"
                ));
                for ((_, payload), lbl) in drop_variants.iter().zip(blocks.iter()) {
                    self.open_block(lbl);
                    for (pi, pty) in payload.iter().enumerate() {
                        if !self.needs_drop(pty) {
                            continue;
                        }
                        // Byte-offset GEP (shared with construct/match) so a
                        // payload value after a >8-byte one is dropped at its
                        // real location.
                        let slot_ptr = self.payload_slot_ptr(&llvm_enum, p_val, payload, pi);
                        self.gen_drop_in_place(pty, &slot_ptr);
                    }
                    self.emit_terminator(&format!("br label %{merge_lbl}"));
                }
                self.open_block(&merge_lbl);
            }
            Ty::Array(elem, n) => {
                if !self.needs_drop(elem) {
                    return;
                }
                let llvm_ty = self.lty(ty);
                for i in 0..*n {
                    let ep = self.next_tmp();
                    self.emit(&format!(
                        "{ep} = getelementptr inbounds {llvm_ty}, ptr {p_val}, i32 0, i32 {i}"
                    ));
                    self.gen_drop_in_place(elem, &ep);
                }
            }
            _ => {}
        }
    }

    fn mark_moved(&mut self, name: &str) {
        if let Some(flag) = self.find_drop_flag(name) {
            // v0.0.7 Slice 1.2: drop-flag write — bool leaf.
            self.gen_store(&Ty::Bool, "false", &flag);
        }
        // If there's no flag, the binding isn't Drop — nothing to do.
    }

    /// v0.0.5 Slice 1A: deep-clone a `string` aggregate value (`{ ptr, i64
    /// len, i64 cap }`). Allocates `len` bytes on the heap, memcpies the
    /// source bytes in, returns a fresh aggregate `{ new_ptr, len, len }`.
    /// `cap` is set to `len` (tighter than the source) — the result is
    /// only ever read or freed, never grown, so the saved bytes are pure
    /// win. Free-of-null is a libc no-op, so `len == 0` is safe even
    /// though `malloc(0)` is implementation-defined.
    ///
    /// Used by `StmtKind::Return` when the returned ident is a shared-
    /// borrow parameter: the caller still owns the original; the result
    /// must be an independent allocation or Drop double-frees.
    fn clone_string_aggregate(&mut self, src: &str) -> String {
        let src_ptr = self.next_tmp();
        self.emit(&format!(
            "{src_ptr} = extractvalue {{ ptr, i64, i64 }} {src}, 0"
        ));
        let len = self.next_tmp();
        self.emit(&format!(
            "{len} = extractvalue {{ ptr, i64, i64 }} {src}, 1"
        ));
        let new_ptr = self.next_tmp();
        self.emit(&format!("{new_ptr} = call ptr @malloc(i64 {len})"));
        // `len == 0` → malloc may return null or a unique sentinel; either
        // way memcpy(_, _, 0) is a no-op, free(null) is a no-op. Skip the
        // zero-length branch — keeps the IR flat and matches what the
        // existing string constructors emit. libc `memcpy` is already
        // declared in the preamble; cheaper than introducing the LLVM
        // memcpy intrinsic here.
        let _dummy = self.next_tmp();
        self.emit(&format!(
            "{_dummy} = call ptr @memcpy(ptr {new_ptr}, ptr {src_ptr}, i64 {len})"
        ));
        let t1 = self.next_tmp();
        self.emit(&format!(
            "{t1} = insertvalue {{ ptr, i64, i64 }} undef, ptr {new_ptr}, 0"
        ));
        let t2 = self.next_tmp();
        self.emit(&format!(
            "{t2} = insertvalue {{ ptr, i64, i64 }} {t1}, i64 {len}, 1"
        ));
        let t3 = self.next_tmp();
        self.emit(&format!(
            "{t3} = insertvalue {{ ptr, i64, i64 }} {t2}, i64 {len}, 2"
        ));
        t3
    }

    /// Emit a drop call for a Drop binding at scope-exit. Slice
    /// 6BC.opt's disposition decides the lowering:
    /// - **Always**: direct unconditional `call @T.drop(ptr)`. No
    ///   flag-load, no conditional, no extra basic blocks. Most
    ///   common case — bindings that get let-and-dropped.
    /// - **Runtime**: load the flag, branch on it, drop in the true
    ///   arm, fall through. The Phase-5 default for bindings that
    ///   may have been moved on some paths.
    fn emit_conditional_drop(&mut self, entry: &DropEntry) {
        // Build the drop-body emitter once; the disposition switch only
        // decides whether to gate it on the flag.
        // All drop kinds route through `gen_drop_in_place`, the single source
        // of teardown logic: a struct runs its user `drop` then auto-drops
        // owning fields; `string` frees its buffer; a tagged enum switches on
        // the tag and drops the active payload.
        let drop_ty = match entry.kind {
            DropKind::Struct(id) => Ty::Struct(id),
            DropKind::String => Ty::String,
            DropKind::Enum(id) => Ty::Enum(id),
        };
        let value_slot = entry.value_slot.clone();
        let body = |state: &mut Self| {
            state.gen_drop_in_place(&drop_ty, &value_slot);
        };
        match entry.disposition {
            DropDisposition::Always => {
                body(self);
            }
            DropDisposition::Runtime => {
                let flag_val = self.next_tmp();
                self.gen_load(&flag_val, &Ty::Bool, &entry.flag_slot);
                let drop_lbl = self.next_block_label();
                let skip_lbl = self.next_block_label();
                self.emit_terminator(&format!(
                    "br i1 {flag_val}, label %{drop_lbl}, label %{skip_lbl}"
                ));
                self.open_block(&drop_lbl);
                body(self);
                self.open_block(&skip_lbl);
            }
        }
    }

    /// Dispatch one scope-exit entry: Drop → conditional call;
    /// Defer → re-emit the expression (value discarded).
    fn emit_scope_exit(&mut self, entry: &ScopeExit) {
        match entry {
            ScopeExit::Drop(d) => self.emit_conditional_drop(d),
            ScopeExit::Defer(e) => {
                // Re-emit the deferred expression as a discard-value
                // expression statement. Side effects fire; the result
                // (if any) is dropped on the floor.
                let _ = self.gen_expr(e);
            }
        }
    }

    /// Emit scope-exit hooks for *every* live scope (all frames, innermost
    /// first, each frame reverse-registered). Used by `return` so all
    /// destructors + defers run on the early-exit path before `ret`.
    fn emit_all_scope_exits(&mut self) {
        let frames: Vec<Vec<ScopeExit>> = self.scope_exits.iter().rev().cloned().collect();
        for frame in &frames {
            for entry in frame.iter().rev() {
                if self.terminated {
                    return;
                }
                self.emit_scope_exit(entry);
            }
        }
    }

    // ---- function body ----

    /// v0.0.14 inline asm Tier 3: emit a `#[naked]` body — just the inline-asm
    /// statements (and an optional asm tail), with no implicit `ret`. The asm
    /// performs the real return; `gen_function` caps the block with
    /// `unreachable`. Sema (`check_naked`) has verified the body is asm-only.
    fn gen_naked_body(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(t) = &b.tail {
                let _ = self.gen_expr(t);
            }
        }
        self.pop_scope();
    }

    fn gen_body_block(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
        if !self.terminated {
            // Implicit function-exit path: evaluate the optional tail first
            // (post-§2.8a, function bodies must use explicit `return`, but
            // Unit-returning bodies are allowed to fall off the end), then
            // run scope-exit drops, then ret.
            match &b.tail {
                Some(t) => {
                    let val = self.gen_expr(t);
                    self.emit_all_scope_exits();
                    if !self.terminated {
                        match self.return_ty {
                            Ty::Unit => self.emit_terminator("ret void"),
                            _ => {
                                let (v, _) = val.expect("non-Unit fn requires tail value");
                                self.emit_terminator(&format!(
                                    "ret {} {}",
                                    self.lty(&self.return_ty),
                                    v
                                ));
                            }
                        }
                    }
                }
                None => {
                    if self.return_ty == Ty::Unit {
                        self.emit_all_scope_exits();
                        if !self.terminated {
                            self.emit_terminator("ret void");
                        }
                    }
                    // For non-Unit returns sema requires an explicit
                    // `return`; the last stmt already terminated and ran
                    // drops via the StmtKind::Return path.
                }
            }
        }
        self.pop_scope();
    }

    // ---- statements ----

    fn gen_stmt(&mut self, s: &Stmt) {
        // v0.0.8 bench-gap finding 1: statement boundaries are
        // cache-flush points. Any statement can mutate a local
        // (`let`, `=`, function call as `Expr` stmt, etc.) so the
        // safe model is to drop the field-read memo at every entry.
        self.invalidate_field_load_cache();
        match &s.kind {
            StmtKind::Let { name, ty, init, .. } => {
                // Resolve declared type up front (always present for the
                // uninitialized case — sema enforced that).
                let var_ty = match (ty, init) {
                    (Some(t), _) => ty_from(t, self.types),
                    (None, Some(init_expr)) => {
                        // Inferred from the init expression's type.
                        let (val, val_ty) =
                            self.gen_expr(init_expr).expect("let init produces a value");
                        let slot = self.alloca_named(&name.name, val_ty.clone());
                        self.emit(&format!(
                            "store {} {}, ptr {}",
                            self.lty(&val_ty),
                            val,
                            slot
                        ));
                        // v0.0.3 Slice 3C: non-Copy local allocas get
                        // their own alias scope after body generation.
                        if !is_copy_ty(&val_ty, self.types) {
                            self.noalias_local_slots.push(slot.clone());
                        }
                        // v0.0.3 drop-tracking: `let v = some_local;` for a
                        // non-Copy type moves the value into the new binding.
                        if !is_copy_ty(&val_ty, self.types) {
                            if let ExprKind::Ident(src) = &init_expr.kind {
                                self.mark_moved(src);
                            }
                        }
                        self.register_value_drop(&name.name, &slot, &val_ty);
                        self.bind(&name.name, slot, val_ty);
                        return;
                    }
                    (None, None) => unreachable!("sema rejected uninit `let` without annotation"),
                };
                let slot = self.alloca_named(&name.name, var_ty.clone());
                // v0.0.3 Slice 3C: non-Copy local allocas get their own
                // alias scope after body generation, regardless of whether
                // the binding is initialized at let or assigned later.
                if !is_copy_ty(&var_ty, self.types) {
                    self.noalias_local_slots.push(slot.clone());
                }
                if let Some(init_expr) = init {
                    // G-044: when the destination is a typed array, build the
                    // literal with the declared element type so the aggregate's
                    // type matches the slot (else `[N x i32]` vs `[N x i64]`).
                    let (val, _) = match (&init_expr.kind, &var_ty) {
                        (ExprKind::ArrayLit { elements }, Ty::Array(elem, _)) => {
                            self.gen_array_lit(elements, Some((**elem).clone()))
                        }
                        (ExprKind::ArrayFill { fill, count, .. }, Ty::Array(elem, _)) => {
                            self.gen_array_fill(fill, *count, Some((**elem).clone()))
                        }
                        _ => self.gen_expr(init_expr).expect("let init produces a value"),
                    };
                    self.emit(&format!(
                        "store {} {}, ptr {}",
                        self.lty(&var_ty),
                        val,
                        slot
                    ));
                    // v0.0.3 drop-tracking: `let v = some_local;` for a
                    // non-Copy type moves the value into the new binding.
                    // Disarm the source's drop so it doesn't fire on the
                    // shared heap allocation at scope exit.
                    if !is_copy_ty(&var_ty, self.types) {
                        if let ExprKind::Ident(src) = &init_expr.kind {
                            self.mark_moved(src);
                        }
                    }
                }
                // If the type carries a destructor, register a scope-exit
                // drop hook before binding the name (so the flag exists by
                // the time anything references this binding). For an
                // uninitialized Drop binding this is currently safe because
                // sema rejects any path that would read it before it's
                // assigned — so drop only runs after assignment.
                self.register_value_drop(&name.name, &slot, &var_ty);
                self.bind(&name.name, slot, var_ty);
            }
            StmtKind::Return(value) => {
                let ret_ty = self.return_ty.clone();
                // Slice 1E (v0.0.2): musttail eligibility for direct calls.
                // The statement must be `return foo(args);` where:
                //   - foo is a known named function (Ident callee in `sigs`)
                //   - foo's return type matches the enclosing fn's return type
                //   - foo is non-variadic (musttail demands matching arity)
                //   - no Drop/defer entries are pending — musttail requires
                //     the ret to immediately follow the call, with nothing
                //     between (the LLVM verifier rejects otherwise)
                //   - foo is not a builtin (`println` lowers to printf)
                // Methods, indirect (FnPtr) calls, and assoc-fn calls are not
                // currently handled (small surface; revisit if measured).
                if self.tail_call_eligible {
                    if let Some(e) = value {
                        if let ExprKind::Call {
                            callee, args: _, ..
                        } = &e.kind
                        {
                            if let ExprKind::Ident(name) = &callee.kind {
                                // v0.0.9 Phase 3 audit: `pending_drops` is
                                // a conservative "any Drop entries registered"
                                // check. It blocks musttail in cases where
                                // every registered drop is provably going to
                                // be skipped at runtime (the binding got
                                // moved into the call's args, flipping its
                                // flag to false). Producing tighter analysis
                                // here — peeking at the call's arg list and
                                // declaring "all drops in scope will be
                                // flipped before the call" — is a measured
                                // optimization (recursive-fn tail-call
                                // performance) that wants property-test
                                // coverage, not a directed fix. Deferred to
                                // v0.0.10. Today's behavior: correct (just
                                // a regular `call`, not `musttail call`).
                                let pending_drops =
                                    self.scope_exits.iter().any(|frame| !frame.is_empty());
                                if !pending_drops {
                                    if let Some(sig) = self.sigs.get(name) {
                                        let callee_params: Vec<&Ty> =
                                            sig.params.iter().map(|(t, _, _, _)| t).collect();
                                        let enclosing: Vec<&Ty> =
                                            self.enclosing_params.iter().collect();
                                        // v0.0.8 fix C: musttail requires
                                        // caller and callee to share a
                                        // calling convention.
                                        let callee_is_fastcc = self.md.is_fastcc(name);
                                        let cc_matches = callee_is_fastcc
                                            == self.enclosing_is_fastcc;
                                        // Target guard: x86-64 cannot guarantee
                                        // a tail call that returns a by-value
                                        // aggregate wider than the 16-byte
                                        // System V register-return window — the
                                        // value comes back in memory and LLVM's
                                        // backend aborts with "failed to perform
                                        // tail call elimination on a call site
                                        // marked musttail". (sret returns go
                                        // through a separate void-return path and
                                        // are unaffected.) arm64/AArch64 — the
                                        // macOS target — has no such limit, so we
                                        // only relax `musttail` on x86-64 and
                                        // leave the macOS code path byte-for-byte
                                        // identical. `musttail` is a TCO
                                        // optimization here, never a correctness
                                        // requirement, so falling back to a plain
                                        // `call` is always safe.
                                        let tail_return_ok = if cfg!(target_arch = "x86_64")
                                            && !return_passes_by_sret_widened(
                                                &self.return_ty,
                                                self.types,
                                            ) {
                                            static_layout(&self.return_ty, self.types)
                                                .map(|(sz, _)| sz <= 16)
                                                .unwrap_or(true)
                                        } else {
                                            true
                                        };
                                        if !sig.is_variadic
                                            && sig.return_type == self.return_ty
                                            && callee_params == enclosing
                                            && name != "println"
                                            && cc_matches
                                            && tail_return_ok
                                        {
                                            self.pending_musttail = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Evaluate the return value first so any moves it triggers
                // (e.g. `return f(move_x)`) flip drop flags before scope drops.
                //
                // Slice 1D: when `gen_expr` lowers a musttail+sret call, it
                // emits `ret void` itself (the caller's sret slot is the
                // callee's sret slot, so the value has already landed by
                // the time control returns) and sets `self.terminated`.
                // Don't `.expect` a value in that case — gen_expr returned
                // None *because* it terminated the block early.
                let ret_val = match value {
                    Some(e) => {
                        let v = self.gen_expr(e);
                        if self.terminated {
                            return;
                        }
                        // v0.0.3 drop-tracking: `return <ident>;` moves the
                        // named binding out of the function. Without this
                        // mark, the scope-exit drop chain would free the
                        // heap allocation while the SSA value still holds
                        // the (now-dangling) pointer that gets stored into
                        // the caller's sret slot.
                        if let ExprKind::Ident(name) = &e.kind {
                            self.mark_moved(name);
                        }
                        // v0.0.12 G-026: `return CALL();` where CALL returns
                        // `()` (e.g. a generic body monomorphized with O=())
                        // produces no SSA value. Fall through to the Unit
                        // return path — emit drops, then `ret void`.
                        if v.is_none() && matches!(ret_ty, Ty::Unit) {
                            self.emit_all_scope_exits();
                            if !self.terminated {
                                self.emit_terminator("ret void");
                            }
                            return;
                        }
                        let raw = v.expect("non-Unit return value").0;
                        // v0.0.5 Slice 1A: auto-clone-on-return-of-borrowed.
                        // `fn echo(x: string) -> string { return x; }` lifts
                        // the caller's pointer into the result slot; the
                        // caller's source binding stays live → both Drop the
                        // same heap → double-free at exit.
                        //
                        // When the returned expression is a bare Ident bound
                        // to a shared-borrow parameter AND the return type
                        // is `string` (the only currently-supported heap-
                        // owning Drop type), emit a deep clone so the result
                        // is an independent allocation. Other heap-owning
                        // generic containers (Vec[T], HashMap[K,V]) still
                        // need explicit `move` — their element-level clone
                        // needs T::clone glue which is its own slice.
                        let cloned = match (&e.kind, &ret_ty) {
                            (ExprKind::Ident(name), Ty::String)
                                if self.borrowed_params.contains(name) =>
                            {
                                Some(self.clone_string_aggregate(&raw))
                            }
                            _ => None,
                        };
                        Some(cloned.unwrap_or(raw))
                    }
                    None => None,
                };
                // Defensive: clear the flag in case gen_expr didn't reach
                // gen_named_call (e.g. ExprKind::Call routed through a
                // different lowering path).
                self.pending_musttail = false;
                // Run destructors for all live Drop bindings in every scope
                // before the `ret`. The conditional drop respects each
                // binding's flag, so values moved into the return expr are
                // not double-dropped.
                self.emit_all_scope_exits();
                if self.terminated {
                    return;
                }
                match (ret_val, &ret_ty) {
                    (Some(v), _) => {
                        // v0.0.3 Phase 5 Slice 5E.3: inside an `async fn`
                        // body, `return X` stashes X in the coroutine
                        // promise and branches to the final-suspend
                        // block — the ramp will return the handle, and
                        // the executor will read X out via
                        // `llvm.coro.promise` after `coro.done` flips.
                        if let Some((hdl, inner_lty, align)) = self.coro_promise.clone() {
                            let prom_ptr = self.next_tmp();
                            self.emit(&format!(
                                "{prom_ptr} = call ptr @llvm.coro.promise(ptr %{hdl}, i32 {align}, i1 false)"
                            ));
                            self.emit(&format!(
                                "store {inner_lty} {v}, ptr {prom_ptr}, align {align}"
                            ));
                            self.emit_terminator("br label %.coro.final_suspend");
                            return;
                        }
                        // Slice 1D: when this fn uses sret, the result
                        // lands in the caller-provided slot — store the
                        // value there and return void.
                        // Phase 5 Slice 5.D: when this fn is a C-ABI export
                        // with a coerced return (≤16 byte aggregate
                        // packed into i64 / [2 x i64]), stage the value
                        // through a temp alloca and reload as the coerced
                        // type before returning.
                        if let Some(slot) = self.sret_slot.clone() {
                            // v0.0.7 Slice 1.2: TBAA-tagged sret store.
                            // sret is aggregate-typed (struct-by-pointer),
                            // so gen_store falls through untagged — which
                            // is the conservative correct default.
                            self.gen_store(&ret_ty, &v, &slot);
                            self.emit_terminator("ret void");
                        } else if let Some(coerced) = self.coerce_ret.clone() {
                            // Stage the original-typed value through a
                            // stack slot, then reload via the coerced
                            // LLVM type. The slot must be sized for the
                            // coerced type (which is ≥ the original) so
                            // the wide load doesn't read OOB. Bytes
                            // beyond the original size are caller-side
                            // undefined per the C ABI — `0`-initializing
                            // them keeps the load deterministic for the
                            // common scalar-output case.
                            let lty = self.lty(&ret_ty);
                            // Use coerce_ret's name to size the alloca.
                            // Convention: i64 → 8 bytes align 8; [2 x i64] → 16/8.
                            let (sz, al) = if coerced == "i64" {
                                (8u64, 8u64)
                            } else if coerced.contains("[2 x i64]") {
                                (16, 8)
                            } else {
                                (8, 8)
                            };
                            let tmp = self.alloca_named_raw("ret.coerce", &coerced, al);
                            // Zero-initialize so unused tail bytes are 0,
                            // not poison. memset via i8 store is cheap;
                            // -O2 will fold it together with the user store.
                            self.emit(&format!(
                                "call void @llvm.memset.p0.i64(ptr {tmp}, i8 0, i64 {sz}, i1 false)"
                            ));
                            // v0.0.7 Slice 1.2: ret-coerce stage store /
                            // reload. The store uses the user's Ty (TBAA-
                            // tagged when primitive); the reload reads the
                            // bits through the coerced LLVM type
                            // (alloca_named_raw owns this slot — raw LLVM
                            // type — so we can't gen_load it from a Ty).
                            self.gen_store(&ret_ty, &v, &tmp);
                            let _ = lty;
                            let coerced_v = self.next_tmp();
                            self.emit(&format!("{coerced_v} = load {coerced}, ptr {tmp}"));
                            self.emit_terminator(&format!("ret {coerced} {coerced_v}"));
                        } else {
                            self.emit_terminator(&format!("ret {} {}", self.lty(&ret_ty), v));
                        }
                    }
                    (None, &Ty::Unit) => {
                        // v0.0.4 Phase 3 Slice 3A.2: Unit-returning
                        // async fns (`async fn yield_now() { ... }`)
                        // exit via final_suspend rather than `ret void`.
                        // No promise store — Unit has no value.
                        if self.coro_promise.is_some() {
                            self.emit_terminator("br label %.coro.final_suspend");
                        } else {
                            self.emit_terminator("ret void");
                        }
                    }
                    (None, _) => {
                        unreachable!("sema should reject return-without-value for non-Unit")
                    }
                }
            }
            StmtKind::While { cond, body, attributes } => self.gen_while(cond, body, attributes),
            StmtKind::For(fl, attributes) => self.gen_for(fl, attributes),
            StmtKind::Expr(e) => {
                let _ = self.gen_expr(e);
            }
            StmtKind::Defer(e) => {
                // Lexical defer: register the expression to run at the
                // enclosing scope's exit, LIFO with surrounding Drop calls.
                // Sema has already type-checked the expression; we clone the
                // AST node into the scope-exit frame and re-emit it later.
                self.register_defer(e.clone());
            }
            // Lowering replaces these with match-using forms before sema
            // and codegen ever see them.
            StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {
                panic!("codegen saw an un-lowered if-let/guard-let/while-let — driver must call crate::lower before codegen");
            }
            StmtKind::Break => {
                let (_, break_lbl) = self
                    .loop_labels
                    .last()
                    .expect("sema rejects `break` outside a loop (E0353)")
                    .clone();
                self.emit_terminator(&format!("br label %{break_lbl}"));
            }
            StmtKind::Continue => {
                let (cont_lbl, _) = self
                    .loop_labels
                    .last()
                    .expect("sema rejects `continue` outside a loop (E0353)")
                    .clone();
                self.emit_terminator(&format!("br label %{cont_lbl}"));
            }
            StmtKind::Loop(body, attributes) => self.gen_loop(body, attributes),
            // Phase 5 slice 5ATTR.3: `assert EXPR;` — branch on the bool
            // and trap on the false path. Sema guarantees the expression
            // type is bool. Phase-5 trap-only behavior; slice 5ATTR.4 will
            // replace the trap with a per-test failure-flag write inside
            // synthesized test-driver builds.
            StmtKind::Assert(e) => self.gen_assert(e),
        }
    }

    fn gen_assert(&mut self, cond: &Expr) {
        let (v, _) = self.gen_expr(cond).expect("assert cond is a bool value");
        let fail_lbl = self.next_block_label();
        let ok_lbl = self.next_block_label();
        self.emit_terminator(&format!("br i1 {v}, label %{ok_lbl}, label %{fail_lbl}"));
        self.open_block(&fail_lbl);
        if self.test_mode {
            // Slice 5ATTR.4: under `cpc test`, an `assert` failure sets the
            // shared `@cpc_test_failed` flag and falls through. The driver's
            // `main` reads the flag after each test's call to decide pass/fail.
            // We do *not* return early — Phase 5 has no raw pointers, so
            // continuing past a failed assertion can't segfault, and a flag
            // write is cheaper than synthesizing a return-of-default per type.
            // v0.0.7 Slice 1.2: test-driver flag write — i32 leaf.
            self.gen_store(&Ty::I32, "1", "@cpc_test_failed");
            self.emit_terminator(&format!("br label %{ok_lbl}"));
        } else {
            self.emit("call void @llvm.trap()");
            self.emit_terminator("unreachable");
        }
        self.open_block(&ok_lbl);
    }

    /// `loop { body }` — emit:
    ///   head:
    ///     body            ; may `br exit` (break) or `br head` (continue) or fall through
    ///     br head
    ///   exit:
    fn gen_loop(&mut self, body: &Block, attributes: &[Attribute]) {
        let head = self.next_block_label();
        let exit = self.next_block_label();
        self.emit_terminator(&format!("br label %{head}"));
        self.open_block(&head);
        // `continue` in a `loop` jumps back to `head`; `break` jumps to `exit`.
        self.loop_labels.push((head.clone(), exit.clone()));
        self.push_scope();
        for s in &body.stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(tail) = &body.tail {
                let _ = self.gen_expr(tail);
            }
            // v0.0.7 Slice 1.3: attach `!llvm.loop` metadata to the
            // back-edge branch if loop-hint attributes are present.
            let md = self.loop_metadata_for(attributes);
            self.emit_terminator(&format!("br label %{head}{md}"));
        }
        self.pop_scope();
        self.loop_labels.pop();
        self.open_block(&exit);
    }

    fn gen_while(&mut self, cond: &Expr, body: &Block, attributes: &[Attribute]) {
        let head = self.next_block_label();
        let loop_body = self.next_block_label();
        let exit = self.next_block_label();

        self.emit_terminator(&format!("br label %{head}"));
        self.open_block(&head);
        let (cond_v, _) = self.gen_expr(cond).expect("while cond produces bool");
        self.emit_terminator(&format!(
            "br i1 {cond_v}, label %{loop_body}, label %{exit}"
        ));

        self.open_block(&loop_body);
        // `continue` re-evaluates the cond → branches to `head`. `break`
        // exits to `exit`. Slice 4-end.
        self.loop_labels.push((head.clone(), exit.clone()));
        self.push_scope();
        for s in &body.stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(tail) = &body.tail {
                // value discarded
                let _ = self.gen_expr(tail);
            }
            // v0.0.7 Slice 1.3: `!llvm.loop` on the back-edge branch.
            let md = self.loop_metadata_for(attributes);
            self.emit_terminator(&format!("br label %{head}{md}"));
        }
        self.pop_scope();
        self.loop_labels.pop();

        self.open_block(&exit);
    }

    /// v0.0.7 Slice 1.3: build the `, !llvm.loop !N` suffix from a list
    /// of loop-hint attributes. Returns an empty string when no
    /// recognized attributes are present so the emitted branch is
    /// byte-identical to the no-attr case (preserves existing IR test
    /// fixtures). Recognized: `#[unroll(N)]` →
    /// `!{!"llvm.loop.unroll.count", i32 N}`; `#[vectorize_width(N)]`
    /// → `!{!"llvm.loop.vectorize.width", i32 N}`.
    fn loop_metadata_for(&self, attributes: &[Attribute]) -> String {
        if attributes.is_empty() {
            return String::new();
        }
        let mut child_ids: Vec<u32> = Vec::new();
        for a in attributes {
            let Some(n) = loop_attr_int_value(a) else {
                continue;
            };
            let key = match a.path.name.as_str() {
                "unroll" => "llvm.loop.unroll.count",
                "vectorize_width" => "llvm.loop.vectorize.width",
                _ => continue,
            };
            let id = self.md.next_id.get();
            self.md.next_id.set(id + 1);
            self.md
                .nodes
                .borrow_mut()
                .push(format!("!{id} = !{{!\"{key}\", i32 {n}}}"));
            child_ids.push(id);
        }
        if child_ids.is_empty() {
            return String::new();
        }
        // The outer node is the `!llvm.loop` group: self-referential
        // distinct, followed by the per-hint child nodes.
        let loop_id = self.md.next_id.get();
        self.md.next_id.set(loop_id + 1);
        let mut items = vec![format!("!{loop_id}")];
        for c in &child_ids {
            items.push(format!("!{c}"));
        }
        self.md.nodes.borrow_mut().push(format!(
            "!{loop_id} = distinct !{{{}}}",
            items.join(", ")
        ));
        format!(", !llvm.loop !{loop_id}")
    }

    /// v0.0.4 Phase 4 Slice 4C: lower `for var in iter_expr { body }`
    /// when `iter_expr` produces an `Iterator[T]`. Inline shape:
    ///   __hdl  = extractvalue Iterator, 0
    ///   loop:
    ///     done = coro.done(__hdl)
    ///     if done -> exit
    ///     prom = coro.promise(__hdl)
    ///     var  = load T, prom
    ///     coro.resume(__hdl)
    ///     <body>
    ///     br loop
    ///   exit:
    ///     coro.destroy(__hdl)
    /// Cleaner than constructing `Option[T]` per iteration; the next()
    /// path remains for explicit pull-style consumers.
    fn gen_for_iterator(&mut self, var: &crate::ast::Ident, iter: &Expr, body: &crate::ast::Block) {
        let (iter_v, iter_ty) = self.gen_expr(iter).expect("for-iter has value");
        let elem_ty = self
            .unwrap_iterator_ty(&iter_ty)
            .expect("sema validated iter is Iterator[T]");
        let iter_llvm = self.lty(&iter_ty);
        let elem_llvm = self.lty(&elem_ty);
        let elem_align = match static_layout(&elem_ty, self.types) {
            Some((_, a)) => a,
            None => 8,
        };
        let hdl = self.next_tmp();
        self.emit(&format!("{hdl} = extractvalue {iter_llvm} {iter_v}, 0"));

        self.push_scope();
        let var_slot = self.alloca_named(&var.name, elem_ty.clone());
        self.bind(&var.name, var_slot.clone(), elem_ty.clone());

        let head = self.next_block_label();
        let body_lbl = self.next_block_label();
        let exit = self.next_block_label();

        self.emit_terminator(&format!("br label %{head}"));
        self.open_block(&head);
        let done = self.next_tmp();
        self.emit(&format!("{done} = call i1 @llvm.coro.done(ptr {hdl})"));
        self.emit_terminator(&format!("br i1 {done}, label %{exit}, label %{body_lbl}"));

        self.open_block(&body_lbl);
        let prom_ptr = self.next_tmp();
        self.emit(&format!(
            "{prom_ptr} = call ptr @llvm.coro.promise(ptr {hdl}, i32 {elem_align}, i1 false)"
        ));
        let val = self.next_tmp();
        // v0.0.7 Slice 1.2: coroutine yield-value load + var store.
        // gen_load doesn't accept an explicit align argument, but the
        // coroutine promise pointer needs `align {elem_align}` because
        // it's an arbitrary stack address from `llvm.coro.promise`.
        // Emit the load directly here and the store via gen_store
        // (which picks up TBAA for primitive `elem_ty`).
        self.emit(&format!(
            "{val} = load {elem_llvm}, ptr {prom_ptr}, align {elem_align}"
        ));
        self.gen_store(&elem_ty, &val, &var_slot);
        self.emit(&format!("call void @llvm.coro.resume(ptr {hdl})"));

        // `continue` should jump back to head; `break` to exit.
        self.loop_labels.push((head.clone(), exit.clone()));
        self.push_scope();
        for s in &body.stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(tail) = &body.tail {
                let _ = self.gen_expr(tail);
            }
            self.emit_terminator(&format!("br label %{head}"));
        }
        self.pop_scope();
        self.loop_labels.pop();

        self.open_block(&exit);
        // Destroy the iterator's frame so the malloc'd coroutine state
        // is freed once iteration completes.
        self.emit(&format!("call void @llvm.coro.destroy(ptr {hdl})"));
        self.pop_scope();
    }

    fn gen_for(&mut self, fl: &ForLoop, attributes: &[Attribute]) {
        match fl {
            ForLoop::Range { var, iter, body } => {
                // v0.0.4 Phase 4 Slice 4C: two for-in shapes — closed-range
                // (`for x in 0..n`) and iterator (`for x in some_gen_fn()`).
                // Iterator form lowers inline to coro.done/resume/promise
                // (avoids materializing an Option per iteration).
                let (start_e, end_e, inclusive) = match &iter.kind {
                    ExprKind::Range {
                        start: Some(s),
                        end: Some(e),
                        inclusive,
                    } => (s.as_ref(), e.as_ref(), *inclusive),
                    _ => {
                        self.gen_for_iterator(var, iter, body);
                        return;
                    }
                };
                self.push_scope();
                let i_slot = self.alloca_named(&var.name, Ty::I32);
                self.bind(&var.name, i_slot.clone(), Ty::I32);
                let end_slot = self.alloca_anon(Ty::I32);

                let (start_v, _) = self.gen_expr(start_e).expect("range start");
                // v0.0.7 Slice 1.2: for-range loop counter — i32 leaf.
                self.gen_store(&Ty::I32, &start_v, &i_slot);
                let (end_v, _) = self.gen_expr(end_e).expect("range end");
                self.gen_store(&Ty::I32, &end_v, &end_slot);

                let head = self.next_block_label();
                let body_lbl = self.next_block_label();
                let step = self.next_block_label();
                let exit = self.next_block_label();

                self.emit_terminator(&format!("br label %{head}"));
                self.open_block(&head);
                let i_v = self.next_tmp();
                self.gen_load(&i_v, &Ty::I32, &i_slot);
                let e_v = self.next_tmp();
                self.gen_load(&e_v, &Ty::I32, &end_slot);
                let cond_v = self.next_tmp();
                let cmp = if inclusive { "sle" } else { "slt" };
                self.emit(&format!("{cond_v} = icmp {cmp} i32 {i_v}, {e_v}"));
                self.emit_terminator(&format!("br i1 {cond_v}, label %{body_lbl}, label %{exit}"));

                self.open_block(&body_lbl);
                // `continue` in a for-range loop must run the increment;
                // route it through `step`, not back to `head`. Slice 4-end.
                self.loop_labels.push((step.clone(), exit.clone()));
                self.push_scope();
                for s in &body.stmts {
                    if self.terminated {
                        break;
                    }
                    self.gen_stmt(s);
                }
                if !self.terminated {
                    if let Some(tail) = &body.tail {
                        let _ = self.gen_expr(tail);
                    }
                    self.emit_terminator(&format!("br label %{step}"));
                }
                self.pop_scope();
                self.loop_labels.pop();

                // Step block: increment then back to head.
                self.open_block(&step);
                let cur_i = self.next_tmp();
                self.gen_load(&cur_i, &Ty::I32, &i_slot);
                let next_i = self.next_tmp();
                self.emit(&format!("{next_i} = add i32 {cur_i}, 1"));
                self.gen_store(&Ty::I32, &next_i, &i_slot);
                // v0.0.7 Slice 1.3: back-edge gets `!llvm.loop`.
                let md = self.loop_metadata_for(attributes);
                self.emit_terminator(&format!("br label %{head}{md}"));

                self.pop_scope();
                self.open_block(&exit);
            }
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                self.push_scope();
                if let Some(init) = init {
                    self.gen_stmt(init);
                }

                let head = self.next_block_label();
                let body_lbl = self.next_block_label();
                let step = self.next_block_label();
                let exit = self.next_block_label();

                self.emit_terminator(&format!("br label %{head}"));
                self.open_block(&head);
                let cond_v = match cond {
                    Some(c) => self.gen_expr(c).expect("for-cond produces bool").0,
                    None => "true".to_string(),
                };
                self.emit_terminator(&format!("br i1 {cond_v}, label %{body_lbl}, label %{exit}"));

                self.open_block(&body_lbl);
                // `continue` in a C-style for must run the update list;
                // route through `step`. Slice 4-end.
                self.loop_labels.push((step.clone(), exit.clone()));
                self.push_scope();
                for s in &body.stmts {
                    if self.terminated {
                        break;
                    }
                    self.gen_stmt(s);
                }
                if !self.terminated {
                    if let Some(tail) = &body.tail {
                        let _ = self.gen_expr(tail);
                    }
                    self.emit_terminator(&format!("br label %{step}"));
                }
                self.pop_scope();
                self.loop_labels.pop();

                // Step block: run update list, branch back to head.
                self.open_block(&step);
                for u in update {
                    let _ = self.gen_expr(u);
                }
                // v0.0.7 Slice 1.3: back-edge gets `!llvm.loop`.
                let md = self.loop_metadata_for(attributes);
                self.emit_terminator(&format!("br label %{head}{md}"));

                self.pop_scope();
                self.open_block(&exit);
            }
        }
    }

    // ---- expressions ----

    /// Generate IR for an expression. Returns Some((value, type)) for value-
    /// producing expressions, None for diverging or Unit-typed expressions
    /// where the caller can't use a value.
    fn gen_expr(&mut self, e: &Expr) -> Option<(String, Ty)> {
        match &e.kind {
            ExprKind::IntLit(v, suf) => {
                use crate::lexer::NumSuffix;
                // Honor the literal's numeric suffix so downstream consumers
                // (array literals, binary arithmetic, anything that builds a
                // typed SSA temporary) emit the right LLVM width. Without
                // this, `[10u8, 20u8]` becomes `[N x i32]` and `1u64 + 2u64`
                // computes in i32 — both produce invalid IR when their
                // results meet a typed destination.
                let ty = match suf {
                    NumSuffix::I8 => Ty::I8,
                    NumSuffix::I16 => Ty::I16,
                    NumSuffix::I32 => Ty::I32,
                    NumSuffix::I64 => Ty::I64,
                    NumSuffix::U8 => Ty::U8,
                    NumSuffix::U16 => Ty::U16,
                    NumSuffix::U32 => Ty::U32,
                    NumSuffix::U64 => Ty::U64,
                    NumSuffix::Isize => Ty::Isize,
                    NumSuffix::Usize => Ty::Usize,
                    // Unsuffixed integer literal: default to i32. Sema-driven
                    // declared types still flow correctly because `let x: u64
                    // = 42` emits `store i64 42` (LLVM accepts width-agnostic
                    // numeric literals in the textual operand position).
                    NumSuffix::None | NumSuffix::F16 | NumSuffix::F32 | NumSuffix::F64 => Ty::I32,
                };
                Some((v.to_string(), ty))
            }
            ExprKind::BoolLit(b) => Some((if *b { "true" } else { "false" }.to_string(), Ty::Bool)),
            ExprKind::IncludeBytes { .. } => {
                // v0.0.6 Slice 1A: lower to the byte global's address.
                // The pre-pass (`emit_compile_time_blob_globals`)
                // populates `md.compile_time_blobs` keyed by this
                // expression's span.
                let span = e.span;
                let (symbol, len) = {
                    let table = self.md.compile_time_blobs.borrow();
                    table
                        .get(&span)
                        .expect(
                            "include_bytes!: span not in module table — \
                         sema must have produced a MonoInfo::compile_time_blobs \
                         entry for every ExprKind::IncludeBytes node",
                        )
                        .clone()
                };
                Some((
                    symbol,
                    Ty::RawPtr(Box::new(Ty::Array(Box::new(Ty::U8), len))),
                ))
            }
            ExprKind::IncludeStr { .. } => {
                // v0.0.7 Slice 3.1: lower to a `str` fat-pointer
                // aggregate `{ ptr, i64 }` pointing at the shared
                // `[N x i8]` global emitted by
                // `emit_compile_time_blob_globals`. UTF-8 has already
                // been validated at sema time (E0875), so the bytes
                // here are guaranteed valid.
                let span = e.span;
                let (symbol, len) = {
                    let table = self.md.compile_time_blobs.borrow();
                    table
                        .get(&span)
                        .expect(
                            "include_str!: span not in module table — \
                         sema must have produced a MonoInfo::compile_time_blobs \
                         entry for every ExprKind::IncludeStr node",
                        )
                        .clone()
                };
                let t1 = self.next_tmp();
                let t2 = self.next_tmp();
                self.body.push_str(&format!(
                    "  {t1} = insertvalue {{ ptr, i64 }} undef, ptr {symbol}, 0\n"
                ));
                self.body.push_str(&format!(
                    "  {t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {len}, 1\n"
                ));
                Some((t2, Ty::Str))
            }
            ExprKind::EnvVar { .. } => {
                // v0.0.8 Phase 4: lower to a `str` fat-pointer aggregate
                // pointing at the shared `[N x i8]` global emitted by
                // `emit_env_var_globals`. Value was read at sema time;
                // E0876 already fired if the var was missing, so by the
                // time we reach codegen the value is guaranteed present.
                let span = e.span;
                let (symbol, len) = {
                    let table = self.md.env_var_globals.borrow();
                    table
                        .get(&span)
                        .expect(
                            "env!: span not in module table — sema must \
                             have produced a MonoInfo::env_vars entry for \
                             every ExprKind::EnvVar node",
                        )
                        .clone()
                };
                let t1 = self.next_tmp();
                let t2 = self.next_tmp();
                self.body.push_str(&format!(
                    "  {t1} = insertvalue {{ ptr, i64 }} undef, ptr {symbol}, 0\n"
                ));
                self.body.push_str(&format!(
                    "  {t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {len}, 1\n"
                ));
                Some((t2, Ty::Str))
            }
            ExprKind::StrLit(s) => {
                // Phase 8 slice 8.STR.1: lower a string literal to a fat-pointer
                // value `{ ptr, i64 }`. The bytes live in a `@.str.N` global
                // emitted by the pre-pass; we just look up the symbol + length
                // and build the struct via `insertvalue`.
                let (symbol, len) = self
                    .str_lits
                    .get(s)
                    .expect("str literal not in table")
                    .clone();
                let t1 = self.next_tmp();
                let t2 = self.next_tmp();
                self.body.push_str(&format!(
                    "  {t1} = insertvalue {{ ptr, i64 }} undef, ptr {symbol}, 0\n"
                ));
                self.body.push_str(&format!(
                    "  {t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {len}, 1\n"
                ));
                Some((t2, Ty::Str))
            }
            ExprKind::CStrLit(s) => {
                // The NUL-terminated bytes already live in `@.str.N` (shared
                // with str literals); a c-string is just that pointer as `*u8`.
                let (symbol, _len) = self
                    .str_lits
                    .get(s)
                    .expect("c-string literal not in table")
                    .clone();
                Some((symbol, Ty::RawPtr(Box::new(Ty::U8))))
            }
            ExprKind::InterpStr { parts } => Some(self.gen_interp_str(parts)),
            ExprKind::FloatLit(v, suf) => {
                use crate::lexer::NumSuffix;
                if matches!(suf, NumSuffix::F16) {
                    // f16 literal: round the f64 value straight to `half` via a
                    // single `fptrunc` — no double-rounding, no hand-computed
                    // half bit pattern. (LLVM rejects a decimal half constant,
                    // so we truncate from the f64-hex form.)
                    let bits = v.to_bits();
                    let t = self.next_tmp();
                    self.body.push_str(&format!(
                        "  {t} = fptrunc double 0x{bits:016X} to half\n"
                    ));
                    return Some((t, Ty::F16));
                }
                let ty = match suf {
                    NumSuffix::F32 => Ty::F32,
                    _ => Ty::F64,
                };
                // v0.0.4 bug fix (raytracer port, 2026-05-17): LLVM rejects
                // `float 0.1` because decimal-form float constants require
                // exact representability; `0.1` is not f32-exact. The decimal
                // form is also strict for `double` (e.g. `1e-8` lacks the
                // mandatory decimal point in the mantissa, breaking parses).
                // Emit hex form: `0x` + the bit pattern of the *f64*
                // representation. For f32 we narrow first (so the f64 hex
                // we emit, when re-narrowed to float by LLVM, round-trips
                // to the exact f32 the user wrote).
                //
                // v0.0.8 fix E (closed, no work): obs.md claimed cpc
                // emits `0x3FD99999A0000000` while C produces
                // `0x3ECCCCCC` for `0.4f32` (a "double-rounding bug").
                // Both halves of that claim are false: cpc and clang
                // emit the same hex (`0x3FD99999A0000000`), and the
                // correctly-rounded f32 for 0.4 IS `0x3ECCCCCD` (one ULP
                // higher than the bad alternative). The (*v as f32 as
                // f64) chain below is the canonical round-trip and
                // produces bit-identical IR to clang. See test
                // `fix_e_f32_literal_matches_clang_bit_pattern`.
                let bits: u64 = match suf {
                    NumSuffix::F32 => (*v as f32 as f64).to_bits(),
                    _ => v.to_bits(),
                };
                Some((format!("0x{bits:016X}"), ty))
            }

            ExprKind::Ident(name) => {
                // Slice 11.FN_PTR: bare-ident referring to a fn (sema
                // coerced it via the expected-FnPtr context) produces
                // the symbol's address as a `ptr` SSA value. Use the
                // link_name if `#[link_name = "..."]` was set; otherwise
                // the source-level name.
                if let Some(sig) = self.sigs.get(name).cloned() {
                    let symbol: String = sig.link_name.clone().unwrap_or_else(|| name.to_string());
                    let params: Vec<Ty> = sig.params.iter().map(|(t, _, _, _)| t.clone()).collect();
                    let ty = Ty::FnPtr {
                        params,
                        return_type: Box::new(sig.return_type.clone()),
                    };
                    return Some((format!("@{symbol}"), ty));
                }
                // v0.0.9 Phase 4: module-scope `static` read. Sema
                // already gated `static mut` reads behind `unsafe`;
                // codegen unconditionally emits a load against the
                // global symbol. Routes before the local-lookup path
                // so a local binding that shadows a static (which
                // sema would have prevented via E0301) doesn't shadow
                // here either.
                let static_ty: Option<Ty> = self.md.statics.borrow().get(name).cloned();
                if let Some(ty) = static_ty {
                    let v = self.next_tmp();
                    self.gen_load(&v, &ty, &format!("@{name}"));
                    return Some((v, ty));
                }
                let (slot, ty) = self.lookup(name).expect("sema validated").clone();
                let v = self.next_tmp();
                // v0.0.7 Slice 1.2: ident-lookup load — the densest
                // single TBAA migration target. Primitive bindings
                // pick up `!tbaa !N` so LLVM's alias analysis can
                // hoist past disjoint-type accesses in hot loops.
                self.gen_load(&v, &ty, &slot);
                Some((v, ty))
            }

            ExprKind::Block(b) => self.gen_block_expr(b),
            // Slice 10.FFI.3: `unsafe { ... }` is a marker for sema;
            // codegen treats it as a regular block.
            ExprKind::Unsafe(b) => self.gen_block_expr(b),

            // v0.0.3 Phase 5 Slice 5E.3: `await EXPR`. EXPR evaluates
            // to a `Future[U]`; we drive it to completion in a
            // resume-loop. The loop:
            //
            //   1. Check inner.done. If true, fall through to extract.
            //   2. Otherwise self-suspend (normal suspend, i1 false).
            //      Default switch label = ramp return (first time we
            //      reach this point during the outer's body); i8 0 =
            //      resumed by executor → call inner.resume + re-check.
            //   3. Once inner.done, extract via `llvm.coro.promise`,
            //      destroy the inner coroutine, branch out with the
            //      loaded value.
            //
            // Only valid inside an `async fn` body — sema rejects
            // otherwise (E0901).
            ExprKind::Await(inner_expr) => self.gen_await_expr(inner_expr),

            // v0.0.4 Phase 4 Slice 4A: `yield EXPR` inside a `gen fn`
            // body. Lowering: store EXPR to the coroutine promise,
            // suspend (non-final), resume falls through. The actual
            // function lowering (`gen_gen_function`) emits the
            // surrounding coroutine setup; this handler just stamps in
            // the per-yield store + suspend.
            ExprKind::Yield(inner_expr) => self.gen_yield_expr(inner_expr),

            ExprKind::If {
                cond,
                then,
                else_branch,
            } => self.gen_if(cond, then, else_branch.as_deref()),

            ExprKind::Call {
                callee,
                args,
                type_args,
            } => self.gen_call(callee, args, type_args),

            ExprKind::Binary { op, lhs, rhs } => Some(self.gen_binary(*op, lhs, rhs)),

            ExprKind::Unary { op, operand } => Some(self.gen_unary(*op, operand)),

            ExprKind::Assign { target, value, op } => {
                self.gen_assign(*op, target, value);
                None
            }

            ExprKind::Cast { expr, ty } => Some(self.gen_cast(expr, ty)),
            ExprKind::Path { segments } => Some(self.gen_path(segments)),
            ExprKind::StructLit { name, fields } => Some(self.gen_struct_lit(name, fields)),
            // Slice 7GEN.5c: GenericStructLit must not reach codegen —
            // monomorphize rewrites every instance to a regular StructLit
            // with the mangled name. If we get here, that pass missed.
            ExprKind::GenericStructLit { .. } => {
                panic!("codegen reached GenericStructLit — monomorphize did not rewrite this site")
            }
            // v0.0.5 Phase 3 Slice 3B: tuple literal `(a, b, ...)`.
            // Element types come from each gen_expr; combine them to
            // re-derive the synthesized tuple struct's mangled name
            // (matches sema's `synthesize_tuple_struct` naming), then
            // emit the same alloca + per-field store + final load
            // pattern that gen_struct_lit uses.
            ExprKind::TupleLit { elements } => Some(self.gen_tuple_lit(elements)),
            ExprKind::Field { receiver, name } => Some(self.gen_field(receiver, name)),
            ExprKind::ArrayLit { elements } | ExprKind::GenericEnumCall { args: elements, .. } => {
                Some(self.gen_array_lit(elements, None))
            }
            ExprKind::ArrayFill { fill, count, .. } => Some(self.gen_array_fill(fill, *count, None)),
            ExprKind::Index { receiver, index } => Some(self.gen_index(receiver, index)),
            ExprKind::Range { .. } => {
                unreachable!("sema rejects ranges outside `for ... in`")
            }
            ExprKind::Match { scrutinee, arms } => self.gen_match(scrutinee, arms),
            ExprKind::Intrinsic { name, type_args, args, ret_ty } => {
                self.gen_intrinsic(name, type_args, args, ret_ty.as_ref(), e.span)
            }
            ExprKind::Asm { template, operands, clobbers } => {
                self.gen_asm(template, operands, clobbers);
                None
            }
        }
    }

    fn gen_array_lit(&mut self, elements: &[Expr], expected_elem: Option<Ty>) -> (String, Ty) {
        // G-044: prefer the declared/expected element type when the
        // destination is typed (`let a: [i64; 4] = [1, 2, 3, 4]`). Bare int
        // literals report `i32` from `gen_expr` but emit width-agnostic
        // constant strings, so storing them through an `i64`-typed GEP is
        // correct — and it keeps the built aggregate's type equal to the
        // destination slot's, avoiding the `[4 x i32]` vs `[4 x i64]` LLVM
        // mismatch. Sema has already verified each element matches the
        // expected element type, so non-literal elements are the right type.
        // With no expected type (untyped `let a = [...]`), infer from the
        // first element as before.
        let (first_val, inferred_elem) = self.gen_expr(&elements[0]).expect("array lit element");
        let elem_ty = expected_elem.unwrap_or(inferred_elem);
        let len = elements.len() as u32;
        let array_ty = Ty::Array(Box::new(elem_ty.clone()), len);
        let llvm_arr = self.lty(&array_ty);
        let _ = self.lty(&elem_ty);
        let slot = self.alloca_anon(array_ty.clone());
        // Store first element.
        let p0 = self.next_tmp();
        self.emit(&format!(
            "{p0} = getelementptr inbounds {llvm_arr}, ptr {slot}, i32 0, i32 0"
        ));
        // v0.0.7 Slice 1.2: array literal init — per-element store
        // through GEP. gen_store picks up the element-type TBAA leaf.
        self.gen_store(&elem_ty, &first_val, &p0);
        // Store the rest.
        for (i, e) in elements.iter().enumerate().skip(1) {
            let (v, _) = self.gen_expr(e).expect("array lit element");
            let p = self.next_tmp();
            self.emit(&format!(
                "{p} = getelementptr inbounds {llvm_arr}, ptr {slot}, i32 0, i32 {i}"
            ));
            self.gen_store(&elem_ty, &v, &p);
        }
        let v = self.next_tmp();
        // Whole-array load is aggregate-shaped; gen_load skips TBAA.
        self.gen_load(&v, &array_ty, &slot);
        (v, array_ty)
    }

    /// v0.0.11 Phase 3: lower `[EXPR; N]` fill-array literal. For the
    /// common case `[0u8; N]` (zero-byte fill) we use `llvm.memset.p0.i64`
    /// which is a single instruction LLVM lowers to a libc memset call
    /// or a tight SIMD store loop. For other shapes we emit an N-iteration
    /// LLVM loop — small N could be unrolled by an inliner pass but isn't
    /// here. The result mirrors `gen_array_lit`'s aggregate-load tail.
    fn gen_array_fill(&mut self, fill: &Expr, count: u32, expected_elem: Option<Ty>) -> (String, Ty) {
        // G-044: same expected-element-type rule as `gen_array_lit` — a typed
        // destination (`let a: [i64; N] = [1; N]`) coerces the bare fill
        // literal to the declared element type.
        let (fill_val, inferred_elem) = self.gen_expr(fill).expect("array fill element");
        let elem_ty = expected_elem.unwrap_or(inferred_elem);
        let array_ty = Ty::Array(Box::new(elem_ty.clone()), count);
        let llvm_arr = self.lty(&array_ty);
        let _ = self.lty(&elem_ty);
        let slot = self.alloca_anon(array_ty.clone());

        // Fast path: zero-byte fill (`[0u8; N]`) lowers to llvm.memset.
        // This is the hot case for static-arena's buffer init — N can be
        // 16K, 64K, etc., and emitting that many enumerated stores would
        // be absurd both at codegen time and in the resulting IR size.
        let zero_byte_fill =
            matches!(elem_ty, Ty::U8 | Ty::I8) && fill_val == "0";
        if zero_byte_fill {
            // `@llvm.memset.p0.i64` is already declared in the module
            // preamble (see `write_preamble`), so no extra decl needed.
            self.emit(&format!(
                "call void @llvm.memset.p0.i64(ptr {slot}, i8 0, i64 {count}, i1 false)"
            ));
        } else {
            // General path: N-iteration store loop. Counter lives in an
            // alloca so we can phi-free; LLVM's mem2reg promotes it.
            let i_slot = self.alloca_anon(Ty::I64);
            let head_lbl = self.next_block_label();
            let body_lbl = self.next_block_label();
            let exit_lbl = self.next_block_label();
            self.emit(&format!("store i64 0, ptr {i_slot}, align 8"));
            self.emit_terminator(&format!("br label %{head_lbl}"));
            self.open_block(&head_lbl);
            let i_val = self.next_tmp();
            self.emit(&format!("{i_val} = load i64, ptr {i_slot}, align 8"));
            let cmp = self.next_tmp();
            self.emit(&format!("{cmp} = icmp ult i64 {i_val}, {count}"));
            self.emit_terminator(&format!(
                "br i1 {cmp}, label %{body_lbl}, label %{exit_lbl}"
            ));
            self.open_block(&body_lbl);
            let p = self.next_tmp();
            self.emit(&format!(
                "{p} = getelementptr inbounds {llvm_arr}, ptr {slot}, i32 0, i64 {i_val}"
            ));
            self.gen_store(&elem_ty, &fill_val, &p);
            let next = self.next_tmp();
            self.emit(&format!("{next} = add i64 {i_val}, 1"));
            self.emit(&format!("store i64 {next}, ptr {i_slot}, align 8"));
            self.emit_terminator(&format!("br label %{head_lbl}"));
            self.open_block(&exit_lbl);
        }

        let v = self.next_tmp();
        self.gen_load(&v, &array_ty, &slot);
        (v, array_ty)
    }

    fn gen_index(&mut self, receiver: &Expr, index: &Expr) -> (String, Ty) {
        let (recv_ptr, recv_ty) = self.gen_place(receiver);
        // Slice 10.FFI.2: raw-pointer indexing is unchecked pointer
        // arithmetic — no bounds check, no array-style outer GEP.
        if let Ty::RawPtr(inner_box) = recv_ty.clone() {
            let inner = (*inner_box).clone();
            let loaded_ptr = self.next_tmp();
            // v0.0.7 Slice 1.2: raw-pointer dereference indexing —
            // ptr-load via the "ptr" TBAA leaf, then element load via
            // the inner type's leaf.
            self.gen_load(&loaded_ptr, &recv_ty, &recv_ptr);
            let (idx_val, _) = self.gen_expr(index).expect("index has value");
            let inner_lt = self.lty(&inner);
            let ptr = self.next_tmp();
            self.emit(&format!(
                "{ptr} = getelementptr inbounds {inner_lt}, ptr {loaded_ptr}, i64 {idx_val}"
            ));
            let v = self.next_tmp();
            self.gen_load(&v, &inner, &ptr);
            return (v, inner);
        }
        let Ty::Array(elem, n) = recv_ty.clone() else {
            unreachable!("sema validated");
        };
        let (idx_val, _) = self.gen_expr(index).expect("index has value");
        let llvm_arr = self.lty(&recv_ty);
        let llvm_elem = self.lty(&elem);
        // Bounds check: `icmp uge i64 idx, N` → branch to trap.
        let bound = self.next_tmp();
        self.emit(&format!("{bound} = icmp uge i64 {idx_val}, {n}"));
        let trap_lbl = self.next_block_label();
        let ok_lbl = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {bound}, label %{trap_lbl}, label %{ok_lbl}"
        ));
        self.open_block(&trap_lbl);
        self.emit("call void @llvm.trap()");
        self.emit_terminator("unreachable");
        self.open_block(&ok_lbl);
        // Slice 1B: publish the post-bounds-check fact `idx < N` via
        // `llvm.assume`. -O2's ConstraintElimination uses this to drop
        // redundant checks on subsequent uses of `idx` against `n`.
        let in_bounds = self.next_tmp();
        self.emit(&format!("{in_bounds} = icmp ult i64 {idx_val}, {n}"));
        self.emit(&format!("call void @llvm.assume(i1 {in_bounds})"));
        // GEP and load.
        let ptr = self.next_tmp();
        self.emit(&format!(
            "{ptr} = getelementptr inbounds {llvm_arr}, ptr {recv_ptr}, i64 0, i64 {idx_val}"
        ));
        let v = self.next_tmp();
        // v0.0.7 Slice 1.2: array element load — element type's TBAA leaf.
        let _ = llvm_elem;
        self.gen_load(&v, &elem, &ptr);
        (v, (*elem).clone())
    }

    /// Build a struct literal: alloca a slot for the new value, store each
    /// field via GEP, load the whole struct as the SSA value. mem2reg
    /// promotes this to PHI/aggregate construction at -O2.
    /// v0.0.5 Phase 3 Slice 3B: lower a tuple literal to a struct
    /// aggregate. Reconstructs the synthesized tuple struct's name
    /// from element types (mirrors sema's `synthesize_tuple_struct`
    /// naming via `mangle_ty_for_tuple` below) and uses the same
    /// per-field store pattern `gen_struct_lit` uses.
    fn gen_tuple_lit(&mut self, elements: &[Expr]) -> (String, Ty) {
        let parts: Vec<(String, Ty)> = elements
            .iter()
            .map(|e| self.gen_expr(e).expect("tuple element has value"))
            .collect();
        let mangled = tuple_struct_name(
            &parts.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(),
            self.types,
        );
        let id =
            *self.types.struct_by_name.get(&mangled).unwrap_or_else(|| {
                panic!("sema should have synthesized tuple struct `{}`", mangled)
            });
        let struct_ty = Ty::Struct(id);
        let llvm_struct = self.lty(&struct_ty);
        let slot = self.alloca_anon(struct_ty.clone());
        for (i, (val, t)) in parts.iter().enumerate() {
            let ptr = self.next_tmp();
            self.emit(&format!(
                "{ptr} = getelementptr inbounds {llvm_struct}, ptr {slot}, i32 0, i32 {i}"
            ));
            // v0.0.7 Slice 1.2: TBAA-tagged tuple-struct field write.
            self.gen_store(t, val, &ptr);
        }
        let v = self.next_tmp();
        // v0.0.7 Slice 1.2: tuple-struct value load — aggregate, no TBAA.
        let _ = llvm_struct;
        self.gen_load(&v, &struct_ty, &slot);
        (v, struct_ty)
    }

    fn gen_struct_lit(&mut self, name: &Ident, fields: &[StructLitField]) -> (String, Ty) {
        let id = *self
            .types
            .struct_by_name
            .get(&name.name)
            .expect("sema validated");
        let info = self.types.struct_defs[id.0 as usize].clone();
        let struct_ty = Ty::Struct(id);
        let llvm_struct = self.lty(&struct_ty);

        let slot = self.alloca_anon(struct_ty.clone());
        for f in fields {
            let (val, _val_ty) = self.gen_expr(&f.value).expect("field init has value");
            let idx = info.field_index(&f.name.name);
            let field_ty = info.field_type(&f.name.name);
            let ptr = self.next_tmp();
            self.emit(&format!(
                "{ptr} = getelementptr inbounds {llvm_struct}, ptr {slot}, i32 0, i32 {idx}"
            ));
            // v0.0.7 Slice 1.2: TBAA-tagged struct field write at
            // struct-literal initialization. Primitive-typed fields
            // carry their per-type leaf; aggregate fields stay
            // untagged (gen_store handles both via tbaa_tag_for).
            self.gen_store(&field_ty, &val, &ptr);
            // G-023 fix: if the field value was a bare-Ident source,
            // ownership transferred into the field — flip the source's
            // drop_flag so the scope-exit drop doesn't free inner heap
            // storage the field now aliases. mark_moved is a no-op for
            // non-Drop bindings (no flag allocated).
            if let ExprKind::Ident(n) = &f.value.kind {
                self.mark_moved(n);
            }
        }
        let v = self.next_tmp();
        // v0.0.7 Slice 1.2: struct-lit value load — aggregate, no TBAA.
        let _ = llvm_struct;
        self.gen_load(&v, &struct_ty, &slot);
        (v, struct_ty)
    }

    /// v0.0.10 Phase 4 (GPU binding-layer wedge): lower `#selector(...)`,
    /// `#msg_send(...)`, and `#compile_shader(...)` intrinsics. Sema has
    /// already validated each call (string-literal args, unsafe context,
    /// etc.) and populated the per-module tables (`selectors`,
    /// `shader_blobs`) consumed by the pre-passes. Codegen's job is the
    /// per-call lowering.
    fn gen_intrinsic(
        &mut self,
        name: &str,
        type_args: &[crate::ast::Type],
        args: &[Expr],
        ret_ty: Option<&crate::ast::Type>,
        span: crate::lexer::Span,
    ) -> Option<(String, Ty)> {
        match name {
            "selector" => Some(self.gen_intrinsic_selector(args)),
            "msg_send" => self.gen_intrinsic_msg_send(args, ret_ty),
            "compile_shader" => Some(self.gen_intrinsic_compile_shader(span)),
            // v0.0.11 Phase 4: intrinsic-spelling migration.
            "addr_of" => Some(self.gen_intrinsic_addr_of(args)),
            "include_bytes" => Some(self.gen_intrinsic_include_bytes(span)),
            "include_str" => Some(self.gen_intrinsic_include_str(span)),
            "env" => Some(self.gen_intrinsic_env(span)),
            "size_of" => Some(self.gen_intrinsic_size_of(type_args)),
            "align_of" => Some(self.gen_intrinsic_align_of(type_args)),
            // v0.0.12 G-028: `#zero::[T]()` — alloca a fresh T-sized slot,
            // memset to zero, load aggregate value. For zero-sized types
            // the memset is skipped (LLVM is fine with size=0, but it's
            // cleaner to avoid emitting the call entirely).
            "zero" => Some(self.gen_intrinsic_zero(type_args)),
            // v0.0.12 G-031: `#cpu_relax()` — spin-loop hint, per-arch.
            // Returns no value (caller-side: dropped on the floor).
            "cpu_relax" => {
                self.gen_intrinsic_cpu_relax();
                None
            }
            // `#println(x)` — void primitive print (its own arm; `ffi_builtin_cg`
            // is value-only).
            "println" => {
                self.gen_println(args);
                None
            }
            // `#asm(...)` never reaches here: the parser routes it to
            // `ExprKind::Asm`, lowered by `gen_asm`.
            _ => {
                // FFI/raw + byte-swap builtins (`#str_ptr`, `#slice_ptr`,
                // `#bswap32`, …) — shared with the bare-call path.
                if let Some(r) = self.ffi_builtin_cg(name, args) {
                    return Some(r);
                }
                panic!(
                    "codegen: unknown `#{name}` intrinsic — sema should have rejected this with E0905"
                )
            }
        }
    }

    /// v0.0.16: lower `#println(x)` — the type-dispatched primitive print.
    /// Void (returns no SSA value), so it has its own `gen_intrinsic` arm rather
    /// than going through `ffi_builtin_cg` (whose `None` means "not a builtin").
    fn gen_println(&mut self, args: &[Expr]) {
        let (av, aty) = self.gen_expr(&args[0]).expect("println arg");
        let v = self.next_tmp();
        match aty {
            Ty::Str => {
                let ptr_tmp = self.next_tmp();
                let len_tmp = self.next_tmp();
                self.emit(&format!("{ptr_tmp} = extractvalue {{ ptr, i64 }} {av}, 0"));
                self.emit(&format!("{len_tmp} = extractvalue {{ ptr, i64 }} {av}, 1"));
                let len_i32 = self.next_tmp();
                self.emit(&format!("{len_i32} = trunc i64 {len_tmp} to i32"));
                self.emit(&format!(
                    "{v} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_str_nl, i32 {len_i32}, ptr {ptr_tmp})"
                ));
            }
            _ => {
                self.emit(&format!(
                    "{v} = call i32 (ptr, ...) @printf(ptr noundef @.fmt_int_nl, i32 {av})"
                ));
            }
        }
    }

    /// v0.0.16: lower the `#`-sigil FFI/raw + byte-swap builtin intrinsics.
    /// `Some((value, ty))` if `name` is one of them, else `None`. Shared by the
    /// `#name` dispatch (`gen_intrinsic`) and the bare-call path during
    /// migration. Sema (`ffi_builtin_ty`) has already validated arg shapes.
    fn ffi_builtin_cg(&mut self, name: &str, args: &[Expr]) -> Option<(String, Ty)> {
        if name == "str_ptr" {
            let (av, _) = self.gen_expr(&args[0]).expect("str_ptr arg");
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 0"));
            return Some((r, Ty::RawPtr(Box::new(Ty::U8))));
        }
        if name == "str_len" {
            let (av, _) = self.gen_expr(&args[0]).expect("str_len arg");
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 1"));
            return Some((r, Ty::Usize));
        }
        if name == "str_from_raw_parts" {
            let (p_val, _) = self.gen_expr(&args[0]).expect("str_from_raw_parts ptr");
            let (n_val, _) = self.gen_expr(&args[1]).expect("str_from_raw_parts len");
            let t1 = self.next_tmp();
            let t2 = self.next_tmp();
            self.emit(&format!("{t1} = insertvalue {{ ptr, i64 }} undef, ptr {p_val}, 0"));
            self.emit(&format!("{t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {n_val}, 1"));
            return Some((t2, Ty::Str));
        }
        if name == "slice_ptr" {
            let (av, ty) = self.gen_expr(&args[0]).expect("slice_ptr arg");
            let elem_ty = match ty {
                Ty::Slice(inner) => *inner,
                _ => unreachable!("sema validated slice_ptr arg type"),
            };
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 0"));
            return Some((r, Ty::RawPtr(Box::new(elem_ty))));
        }
        if name == "slice_len" {
            let (av, _) = self.gen_expr(&args[0]).expect("slice_len arg");
            let r = self.next_tmp();
            self.emit(&format!("{r} = extractvalue {{ ptr, i64 }} {av}, 1"));
            // Publish the proven non-negative invariant via `llvm.assume`
            // (`!range` is illegal on `extractvalue`).
            let nn = self.next_tmp();
            self.emit(&format!("{nn} = icmp sge i64 {r}, 0"));
            self.emit(&format!("call void @llvm.assume(i1 {nn})"));
            return Some((r, Ty::Usize));
        }
        if name == "slice_from_raw_parts" {
            let (p_val, p_ty) = self.gen_expr(&args[0]).expect("slice_from_raw_parts ptr");
            let (n_val, _) = self.gen_expr(&args[1]).expect("slice_from_raw_parts len");
            let elem_ty = match p_ty {
                Ty::RawPtr(inner) => *inner,
                _ => unreachable!("sema validated slice_from_raw_parts ptr type"),
            };
            let t1 = self.next_tmp();
            let t2 = self.next_tmp();
            self.emit(&format!("{t1} = insertvalue {{ ptr, i64 }} undef, ptr {p_val}, 0"));
            self.emit(&format!("{t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {n_val}, 1"));
            return Some((t2, Ty::Slice(Box::new(elem_ty))));
        }
        if let Some((bits, ret_ty)) = match name {
            "bswap16" | "htons" | "ntohs" => Some((16u32, Ty::U16)),
            "bswap32" | "htonl" | "ntohl" => Some((32u32, Ty::U32)),
            "bswap64" => Some((64u32, Ty::U64)),
            _ => None,
        } {
            let (av, _) = self.gen_expr(&args[0]).expect("bswap arg");
            let r = self.next_tmp();
            self.emit(&format!("{r} = call i{bits} @llvm.bswap.i{bits}(i{bits} {av})"));
            return Some((r, ret_ty));
        }
        None
    }

    /// v0.0.11 Phase 4: `#addr_of(expr)` — pointer to a place.
    ///
    /// v0.0.12 G-025: extended to any place expression — `Ident`, `Field`,
    /// `Index`, `Deref` and chains. `gen_place` already produces the right
    /// GEP for each shape (e.g. `(*o).b` walks Deref → field-GEP on the
    /// pointed-to struct), so the codegen here is unchanged from the
    /// bare-ident slice; only sema's gate was loosened.
    fn gen_intrinsic_addr_of(&mut self, args: &[Expr]) -> (String, Ty) {
        let (slot, ty) = self.gen_place(&args[0]);
        (slot, Ty::RawPtr(Box::new(ty)))
    }

    /// v0.0.11 Phase 4: `#include_bytes("path")` — address of a private
    /// `[N x i8]` global emitted by `emit_compile_time_blob_globals`.
    fn gen_intrinsic_include_bytes(&mut self, span: crate::lexer::Span) -> (String, Ty) {
        let (symbol, len) = {
            let table = self.md.compile_time_blobs.borrow();
            table
                .get(&span)
                .expect("#include_bytes: span not in module table — sema must have populated compile_time_blobs_table")
                .clone()
        };
        (
            symbol,
            Ty::RawPtr(Box::new(Ty::Array(Box::new(Ty::U8), len))),
        )
    }

    /// v0.0.11 Phase 4: `#include_str("path")` — `str` fat-pointer over
    /// the same private `[N x i8]` global emitted by
    /// `emit_compile_time_blob_globals`. UTF-8 validated at sema time.
    fn gen_intrinsic_include_str(&mut self, span: crate::lexer::Span) -> (String, Ty) {
        let (symbol, len) = {
            let table = self.md.compile_time_blobs.borrow();
            table
                .get(&span)
                .expect("#include_str: span not in module table — sema must have populated compile_time_blobs_table")
                .clone()
        };
        let t1 = self.next_tmp();
        let t2 = self.next_tmp();
        self.body.push_str(&format!(
            "  {t1} = insertvalue {{ ptr, i64 }} undef, ptr {symbol}, 0\n"
        ));
        self.body.push_str(&format!(
            "  {t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {len}, 1\n"
        ));
        (t2, Ty::Str)
    }

    /// v0.0.11 Phase 4: `#env("NAME")` — `str` fat-pointer over the
    /// `[N x i8]` global emitted by `emit_env_var_globals`. Value was
    /// read at sema time.
    fn gen_intrinsic_env(&mut self, span: crate::lexer::Span) -> (String, Ty) {
        let (symbol, len) = {
            let table = self.md.env_var_globals.borrow();
            table
                .get(&span)
                .expect("#env: span not in module table — sema must have populated env_vars_table")
                .clone()
        };
        let t1 = self.next_tmp();
        let t2 = self.next_tmp();
        self.body.push_str(&format!(
            "  {t1} = insertvalue {{ ptr, i64 }} undef, ptr {symbol}, 0\n"
        ));
        self.body.push_str(&format!(
            "  {t2} = insertvalue {{ ptr, i64 }} {t1}, i64 {len}, 1\n"
        ));
        (t2, Ty::Str)
    }

    /// v0.0.11 Phase 4: `#size_of::[T]()` — GEP-null trick, folded to a
    /// constant at -O1+.
    fn gen_intrinsic_size_of(&mut self, type_args: &[crate::ast::Type]) -> (String, Ty) {
        let t = ty_from(&type_args[0], &self.types);
        let llvm_t = llvm_ty(&t, &self.types);
        let ptr_tmp = self.next_tmp();
        let int_tmp = self.next_tmp();
        self.emit(&format!(
            "{ptr_tmp} = getelementptr {llvm_t}, ptr null, i64 1"
        ));
        self.emit(&format!("{int_tmp} = ptrtoint ptr {ptr_tmp} to i64"));
        (int_tmp, Ty::Usize)
    }

    /// v0.0.11 Phase 4: `#align_of::[T]()` — GEP-null on `{ i1, T }` to
    /// extract T's alignment offset. Folded to a constant at -O1+.
    fn gen_intrinsic_align_of(&mut self, type_args: &[crate::ast::Type]) -> (String, Ty) {
        let t = ty_from(&type_args[0], &self.types);
        let llvm_t = llvm_ty(&t, &self.types);
        let ptr_tmp = self.next_tmp();
        let int_tmp = self.next_tmp();
        self.emit(&format!(
            "{ptr_tmp} = getelementptr {{ i1, {llvm_t} }}, ptr null, i64 0, i32 1"
        ));
        self.emit(&format!("{int_tmp} = ptrtoint ptr {ptr_tmp} to i64"));
        (int_tmp, Ty::Usize)
    }

    /// v0.0.12 G-028: `#zero::[T]()` — a zeroed value of type `T`.
    /// Allocates a T-sized slot on the stack, memsets to zero, loads
    /// the aggregate back as a value. `llvm.memset.p0.i64` is already
    /// declared in the preamble (see `write_preamble`).
    fn gen_intrinsic_zero(&mut self, type_args: &[crate::ast::Type]) -> (String, Ty) {
        let t = ty_from(&type_args[0], &self.types);
        let slot = self.alloca_anon(t.clone());
        if let Some((sz, _al)) = static_layout(&t, self.types) {
            if sz > 0 {
                self.emit(&format!(
                    "call void @llvm.memset.p0.i64(ptr {slot}, i8 0, i64 {sz}, i1 false)"
                ));
            }
        }
        let v = self.next_tmp();
        // Aggregate load — gen_load skips TBAA for aggregates.
        self.gen_load(&v, &t, &slot);
        (v, t)
    }

    /// v0.0.14 inline asm (Tier 1 + Tier 2): emit a side-effecting `call asm`.
    ///
    /// Lowering, following LLVM's inline-asm operand model:
    ///   - operand `$N` numbering: outputs (`out`/`inout`) first in declaration
    ///     order, then pure inputs (`in`); `{name}` in the template is rewritten
    ///     to `$N`.
    ///   - constraint string: `=r`/`={reg}` per output, `r`/`{reg}` per input,
    ///     a tied-input number per `inout` (its output index), then `~{reg}` per
    ///     clobber.
    ///   - return type: void (0 outputs), the scalar type (1), or an anonymous
    ///     struct unpacked with `extractvalue` (N>1). Each output's result is
    ///     stored back into its place.
    /// `sideeffect` is always set so a side-effecting, output-free asm (a fence)
    /// isn't deleted. Tier 1 (`#asm("dmb ish")`) is the no-operand degenerate
    /// case → `call void asm sideeffect "dmb ish", ""()`.
    fn gen_asm(&mut self, template: &str, operands: &[AsmOperand], clobbers: &[String]) {
        let outs: Vec<&AsmOperand> = operands
            .iter()
            .filter(|o| matches!(o.dir, AsmDir::Out | AsmDir::InOut))
            .collect();
        let ins: Vec<&AsmOperand> = operands
            .iter()
            .filter(|o| matches!(o.dir, AsmDir::In))
            .collect();

        // `$N` index per operand name: outputs first, then pure inputs.
        let mut index_of: HashMap<&str, usize> = HashMap::new();
        for (k, op) in outs.iter().enumerate() {
            index_of.insert(op.name.as_str(), k);
        }
        for (j, op) in ins.iter().enumerate() {
            index_of.insert(op.name.as_str(), outs.len() + j);
        }

        // Template: escape literal specials, then `{name}` -> `$N`.
        let mut tmpl = escape_asm_template(template);
        for op in operands {
            let n = index_of[op.name.as_str()];
            tmpl = tmpl.replace(&format!("{{{}}}", op.name), &format!("${n}"));
        }

        // Constraint string.
        let mut cons: Vec<String> = Vec::new();
        for op in &outs {
            cons.push(match &op.reg {
                AsmReg::Any => "=r".to_string(),
                AsmReg::Explicit(r) => format!("={{{r}}}"),
            });
        }
        for op in &ins {
            cons.push(match &op.reg {
                AsmReg::Any => "r".to_string(),
                AsmReg::Explicit(r) => format!("{{{r}}}"),
            });
        }
        for (k, op) in outs.iter().enumerate() {
            if op.dir == AsmDir::InOut {
                cons.push(k.to_string()); // tied input -> output index
            }
        }
        for c in clobbers {
            cons.push(format!("~{{{c}}}"));
        }
        let constraints = cons.join(",");

        // Argument values: pure inputs, then each inout's current value.
        let mut args: Vec<(String, String)> = Vec::new();
        for op in &ins {
            let (v, ty) = self
                .gen_expr(&op.value)
                .expect("asm `in` operand has a value");
            args.push((llvm_ty(&ty, self.types), v));
        }
        for op in &outs {
            if op.dir == AsmDir::InOut {
                let (v, ty) = self
                    .gen_expr(&op.value)
                    .expect("asm `inout` operand reads its place");
                args.push((llvm_ty(&ty, self.types), v));
            }
        }

        // Output places (slot + llvm type) to store results back into.
        let out_slots: Vec<(String, String)> = outs
            .iter()
            .map(|op| {
                let (slot, ty) = self.gen_place(&op.value);
                (slot, llvm_ty(&ty, self.types))
            })
            .collect();

        let args_str = args
            .iter()
            .map(|(t, v)| format!("{t} {v}"))
            .collect::<Vec<_>>()
            .join(", ");

        if out_slots.is_empty() {
            self.emit(&format!(
                "call void asm sideeffect \"{tmpl}\", \"{constraints}\"({args_str})"
            ));
            return;
        }

        let ret_ty_str = if out_slots.len() == 1 {
            out_slots[0].1.clone()
        } else {
            format!(
                "{{ {} }}",
                out_slots
                    .iter()
                    .map(|(_, t)| t.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let res = self.next_tmp();
        self.emit(&format!(
            "{res} = call {ret_ty_str} asm sideeffect \"{tmpl}\", \"{constraints}\"({args_str})"
        ));
        if out_slots.len() == 1 {
            let (slot, ty) = &out_slots[0];
            self.emit(&format!("store {ty} {res}, ptr {slot}"));
        } else {
            for (k, (slot, ty)) in out_slots.iter().enumerate() {
                let ev = self.next_tmp();
                self.emit(&format!("{ev} = extractvalue {ret_ty_str} {res}, {k}"));
                self.emit(&format!("store {ty} {ev}, ptr {slot}"));
            }
        }
    }

    /// v0.0.12 G-031: `#cpu_relax()` — per-arch spin-loop hint.
    ///
    /// * aarch64 → `call void @llvm.aarch64.hint(i32 1)` (YIELD)
    /// * x86_64  → `call void @llvm.x86.sse2.pause()`
    /// * other   → no instruction emitted (the hint is correctness-
    ///   irrelevant; the C convention treats unknown targets as a no-op)
    ///
    /// Declarations are added once per module via the preamble. The
    /// codegen module's preamble already declares many libc/llvm
    /// intrinsics; these two are added there as well.
    fn gen_intrinsic_cpu_relax(&mut self) {
        if cfg!(target_arch = "aarch64") {
            self.emit("call void @llvm.aarch64.hint(i32 1)");
        } else if cfg!(target_arch = "x86_64") {
            self.emit("call void @llvm.x86.sse2.pause()");
        }
        // else: emit nothing — the hint is a power optimization, not
        // a correctness requirement.
    }

    /// `#selector("name") -> *u8`. Look up the cached-pointer pair
    /// emitted by `emit_selector_globals`, then emit the lazy-init
    /// load+branch+register pattern. Per-call cost: one load + one
    /// branch (predicted-taken after the first call to that selector).
    fn gen_intrinsic_selector(&mut self, args: &[Expr]) -> (String, Ty) {
        let name = match &args[0].kind {
            ExprKind::StrLit(s) => s.clone(),
            _ => panic!("codegen: #selector arg should be a string literal (sema invariant)"),
        };
        let (data_sym, cached_sym, _len) = {
            let table = self.md.selector_globals.borrow();
            table
                .get(&name)
                .expect("#selector: name not in module table — sema must populate selectors_set")
                .clone()
        };
        let cached_val = self.next_tmp();
        let is_null = self.next_tmp();
        let registered = self.next_tmp();
        let result = self.next_tmp();
        let register_lbl = self.next_block_label();
        let done_lbl = self.next_block_label();
        // Avoid `phi` (no current-block tracking in codegen state) by
        // re-loading the cached pointer at the merge. The second load is
        // free for LLVM to fold given the global's `nonnull` after the
        // register path, and it dodges the need to thread the entry-block
        // label through the helper.
        self.emit(&format!("{cached_val} = load ptr, ptr {cached_sym}, align 8"));
        self.emit(&format!("{is_null} = icmp eq ptr {cached_val}, null"));
        self.emit_terminator(&format!(
            "br i1 {is_null}, label %{register_lbl}, label %{done_lbl}"
        ));
        self.open_block(&register_lbl);
        self.emit(&format!(
            "{registered} = call ptr @sel_registerName(ptr {data_sym})"
        ));
        self.emit(&format!("store ptr {registered}, ptr {cached_sym}, align 8"));
        self.emit_terminator(&format!("br label %{done_lbl}"));
        self.open_block(&done_lbl);
        self.emit(&format!("{result} = load ptr, ptr {cached_sym}, align 8"));
        (result, Ty::RawPtr(Box::new(Ty::U8)))
    }

    /// `#msg_send(recv, "selector", args...) -> T`. Synthesizes a typed
    /// call to the variadic `@objc_msgSend` declaration emitted in
    /// `generate_inner`'s pre-pass. The selector is looked up via the
    /// same machinery as `#selector(...)`.
    fn gen_intrinsic_msg_send(
        &mut self,
        args: &[Expr],
        ret_ty_ast: Option<&crate::ast::Type>,
    ) -> Option<(String, Ty)> {
        // args[0] = receiver expression; args[1] = selector string literal;
        // args[2..] = forwarded arguments.
        let (recv_val, _recv_ty) = self.gen_expr(&args[0]).expect("#msg_send receiver");
        // Selector: reuse the #selector(...) lowering by faking a one-arg
        // intrinsic call against args[1].
        let sel_args = std::slice::from_ref(&args[1]);
        let (sel_val, _) = self.gen_intrinsic_selector(sel_args);
        // Type-check the forwarded args (sema already did this; gen_expr
        // is what produces the LLVM values).
        let mut typed_args: Vec<(String, Ty)> = Vec::with_capacity(args.len().saturating_sub(2));
        for a in &args[2..] {
            let (v, t) = self.gen_expr(a).expect("#msg_send forwarded arg");
            typed_args.push((v, t));
        }
        // Resolve the return type. Sema accepts the `-> T` ascription as
        // an AST `Type`; resolve it against the type tables here.
        let ret_ty = match ret_ty_ast {
            Some(rt) => ty_from(rt, self.types),
            None => Ty::Unit,
        };
        let ret_llvm = if matches!(ret_ty, Ty::Unit) {
            "void".to_string()
        } else {
            self.lty(&ret_ty)
        };
        // Build the call. Format:
        //   %r = call <ret_llvm> (ptr, ptr, ...) @objc_msgSend(ptr %recv, ptr %sel, <typed args...>)
        let mut argstr = format!("ptr {recv_val}, ptr {sel_val}");
        for (v, t) in &typed_args {
            argstr.push_str(", ");
            argstr.push_str(&format!("{} {}", self.lty(t), v));
        }
        // CRITICAL: NOT variadic on aarch64-darwin (see emit_intrinsic_runtime_decls
        // comment in generate_inner). Per-call non-variadic signature; LLVM
        // accepts shape divergence between this and the 2-arg declare with
        // opaque pointers.
        if matches!(ret_ty, Ty::Unit) {
            self.emit(&format!("call void @objc_msgSend({argstr})"));
            None
        } else {
            let r = self.next_tmp();
            self.emit(&format!("{r} = call {ret_llvm} @objc_msgSend({argstr})"));
            Some((r, ret_ty))
        }
    }

    /// `#compile_shader("path", "target") -> *[u8; N]`. Bytes are already
    /// produced at sema time (sema invoked `xcrun ... metallib` and
    /// stored the result in `MonoInfo::shader_blobs`). Codegen looks up
    /// the global emitted by `emit_shader_blob_globals` and returns its
    /// address. Mirrors `ExprKind::IncludeBytes`.
    fn gen_intrinsic_compile_shader(&mut self, span: crate::lexer::Span) -> (String, Ty) {
        let (symbol, len) = {
            let table = self.md.shader_blob_globals.borrow();
            table
                .get(&span)
                .expect(
                    "#compile_shader: span not in module table — \
                     sema must produce a MonoInfo::shader_blobs entry for every call",
                )
                .clone()
        };
        (
            symbol,
            Ty::RawPtr(Box::new(Ty::Array(Box::new(Ty::U8), len))),
        )
    }

    /// Read a field. The receiver may be a place (`p.x`), in which case we
    /// keep the address chain as long as possible (one GEP off the local's
    /// alloca), or a value (`make().x`), in which case we stash the value
    /// in a temporary alloca first.
    fn gen_field(&mut self, receiver: &Expr, name: &Ident) -> (String, Ty) {
        // v0.0.8 bench-gap finding 1: if the receiver is a bare
        // local-binding ident, check the per-expression field-read
        // cache. Repeated reads of `v.x` in one expression
        // (e.g. `v.x * v.x` in a Vec3 dot product) reuse the same
        // SSA value, which restores the adjacent-load pattern LLVM's
        // SLP-vectorizer keys on. Nested receivers (`v.inner.x`) and
        // place expressions through pointer arithmetic skip the
        // cache to keep the invariants simple.
        if let ExprKind::Ident(local) = &receiver.kind {
            let key = (local.clone(), name.name.clone());
            if let Some(cached) = self.field_load_cache.get(&key) {
                return cached.clone();
            }
        }
        let (slot, struct_ty) = self.gen_place(receiver);
        let Ty::Struct(id) = struct_ty else {
            unreachable!("sema validated");
        };
        let info = self.types.struct_defs[id.0 as usize].clone();
        let llvm_struct = self.lty(&struct_ty);
        let idx = info.field_index(&name.name);
        let field_ty = info.field_type(&name.name);
        let ptr = self.next_tmp();
        self.emit(&format!(
            "{ptr} = getelementptr inbounds {llvm_struct}, ptr {slot}, i32 0, i32 {idx}"
        ));
        let v = self.next_tmp();
        // v0.0.7 Slice 1.2: TBAA-tagged struct field load. Primitive
        // fields get the per-type leaf; aggregate fields fall through
        // to the v0.0.8 aggregate-leaf path.
        self.gen_load(&v, &field_ty, &ptr);
        // v0.0.8 bench-gap finding 1: memoize for the rest of the
        // current expression.
        if let ExprKind::Ident(local) = &receiver.kind {
            self.field_load_cache.insert(
                (local.clone(), name.name.clone()),
                (v.clone(), field_ty.clone()),
            );
        }
        (v, field_ty)
    }

    /// Compute a (slot-pointer, type) for a place expression. For an Ident
    /// the slot is the local's alloca. For a Field chain we GEP through.
    /// For arbitrary value-producing expressions, materialize into a temp
    /// alloca so we can address it.
    fn gen_place(&mut self, e: &Expr) -> (String, Ty) {
        match &e.kind {
            ExprKind::Ident(name) => {
                // v0.0.9 Phase 4: module-scope `static mut` write. The
                // global symbol IS the place — no alloca, no load.
                // Sema rejected writes to immutable statics (E0305)
                // and gated `static mut` writes behind `unsafe`
                // (E0X34), so reaching here means a permitted write.
                let static_ty: Option<Ty> = self.md.statics.borrow().get(name).cloned();
                if let Some(ty) = static_ty {
                    return (format!("@{name}"), ty);
                }
                let (slot, ty) = self.lookup(name).expect("sema validated").clone();
                (slot, ty)
            }
            ExprKind::Field { receiver, name } => {
                let (recv_slot, recv_ty) = self.gen_place(receiver);
                let Ty::Struct(id) = recv_ty.clone() else {
                    unreachable!("sema validated");
                };
                let info = self.types.struct_defs[id.0 as usize].clone();
                let llvm_struct = self.lty(&recv_ty);
                let idx = info.field_index(&name.name);
                let field_ty = info.field_type(&name.name);
                let ptr = self.next_tmp();
                self.emit(&format!(
                    "{ptr} = getelementptr inbounds {llvm_struct}, ptr {recv_slot}, i32 0, i32 {idx}"
                ));
                (ptr, field_ty)
            }
            ExprKind::Index { receiver, index } => {
                let (recv_slot, recv_ty) = self.gen_place(receiver);
                // Slice 10.FFI.2: indexing on raw pointers is unchecked
                // pointer arithmetic. `p[i]` loads the pointer value from
                // its slot, then GEPs by i64 offset — no bounds check
                // (the pointer's length is unknown).
                if let Ty::RawPtr(inner_box) = recv_ty.clone() {
                    let inner = (*inner_box).clone();
                    let loaded_ptr = self.next_tmp();
                    // v0.0.7 Slice 1.2: place-side raw-pointer load.
                    self.gen_load(&loaded_ptr, &recv_ty, &recv_slot);
                    let (idx_val, _) = self.gen_expr(index).expect("index has value");
                    let inner_lt = self.lty(&inner);
                    let ptr = self.next_tmp();
                    self.emit(&format!(
                        "{ptr} = getelementptr inbounds {inner_lt}, ptr {loaded_ptr}, i64 {idx_val}"
                    ));
                    return (ptr, inner);
                }
                let Ty::Array(elem, n) = recv_ty.clone() else {
                    unreachable!("sema validated");
                };
                let (idx_val, _) = self.gen_expr(index).expect("index has value");
                let llvm_arr = self.lty(&recv_ty);
                // Bounds check.
                let bound = self.next_tmp();
                self.emit(&format!("{bound} = icmp uge i64 {idx_val}, {n}"));
                let trap_lbl = self.next_block_label();
                let ok_lbl = self.next_block_label();
                self.emit_terminator(&format!(
                    "br i1 {bound}, label %{trap_lbl}, label %{ok_lbl}"
                ));
                self.open_block(&trap_lbl);
                self.emit("call void @llvm.trap()");
                self.emit_terminator("unreachable");
                self.open_block(&ok_lbl);
                // Slice 1B: publish post-check fact `idx < N` via assume.
                let in_bounds = self.next_tmp();
                self.emit(&format!("{in_bounds} = icmp ult i64 {idx_val}, {n}"));
                self.emit(&format!("call void @llvm.assume(i1 {in_bounds})"));
                let ptr = self.next_tmp();
                self.emit(&format!("{ptr} = getelementptr inbounds {llvm_arr}, ptr {recv_slot}, i64 0, i64 {idx_val}"));
                (ptr, (*elem).clone())
            }
            // Slice 10.FFI.2: `*p` as an assignment target. `gen_place`
            // returns the pointer value itself (which IS the slot to
            // store into); the pointee type comes from the unwrapped
            // RawPtr.
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => {
                let (v, ty) = self.gen_expr(operand).expect("deref target has value");
                let inner = match ty {
                    Ty::RawPtr(i) => (*i).clone(),
                    _ => unreachable!("sema validated operand is RawPtr"),
                };
                (v, inner)
            }
            _ => {
                // Value expression: stash in a temp alloca and address that.
                let (val, ty) = self.gen_expr(e).expect("place fallback expects a value");
                let slot = self.alloca_anon(ty.clone());
                // v0.0.7 Slice 1.2: TBAA-tagged spill store.
                self.gen_store(&ty, &val, &slot);
                (slot, ty)
            }
        }
    }

    /// B-10: fast-math flag prefix for floating-point arithmetic.
    /// Returns `"contract "` when fp-contraction is on (the default,
    /// matching clang's `-ffp-contract=on`) and `""` under
    /// `--fp-contract=off`. Insert directly before the type in a float
    /// `fadd`/`fsub`/`fmul`/`fdiv` so the optimizer is (or isn't)
    /// licensed to fuse `a*b+c` into an FMA.
    fn fmf(&self) -> &'static str {
        if self.md.fp_contract.get() {
            "contract "
        } else {
            ""
        }
    }

    /// FMA peephole. Returns Some when `lhs OP rhs` matches one of:
    /// - `(a * b) + c`   → `llvm.fmuladd.fN(a, b, c)`
    /// - `c + (a * b)`   → `llvm.fmuladd.fN(a, b, c)`
    /// - `(a * b) - c`   → `llvm.fmuladd.fN(a, b, -c)` via fneg
    /// - `c - (a * b)`   → `llvm.fmuladd.fN(-a, b, c)` (negate one factor)
    /// All on a float type. Sides must agree on type.
    fn try_emit_fmuladd(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> Option<(String, Ty)> {
        // Helper: when an Expr is a `Binary(Mul, a, b)`, return (a, b).
        fn as_mul<'e>(e: &'e Expr) -> Option<(&'e Expr, &'e Expr)> {
            if let ExprKind::Binary {
                op: BinOp::Mul,
                lhs,
                rhs,
            } = &e.kind
            {
                Some((lhs, rhs))
            } else {
                None
            }
        }
        // Detect which side is the Mul; evaluation order is left-to-right
        // (same as gen_binary's default). For `c +/- a*b`, we still
        // evaluate c before a, b — matches both source order and the
        // existing non-FMA path.
        let (a_e, b_e, c_e, c_first) = match op {
            BinOp::Add => {
                if let Some((a, b)) = as_mul(lhs) {
                    (a, b, rhs, false)
                } else if let Some((a, b)) = as_mul(rhs) {
                    (a, b, lhs, true)
                } else {
                    return None;
                }
            }
            BinOp::Sub => {
                // (a*b) - c  → fmuladd(a, b, -c)
                // c - (a*b)  → fmuladd(-a, b, c)
                if let Some((a, b)) = as_mul(lhs) {
                    (a, b, rhs, false)
                } else if let Some((a, b)) = as_mul(rhs) {
                    (a, b, lhs, true)
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        // Evaluate the c operand first if it appears first in source order,
        // else evaluate a, b first. This preserves the side-effect ordering
        // that the non-FMA path has.
        let (c_val, c_ty, a_val, a_ty, b_val, b_ty) = if c_first {
            let (c, ct) = self.gen_expr(c_e)?;
            let (a, at) = self.gen_expr(a_e)?;
            let (b, bt) = self.gen_expr(b_e)?;
            (c, ct, a, at, b, bt)
        } else {
            let (a, at) = self.gen_expr(a_e)?;
            let (b, bt) = self.gen_expr(b_e)?;
            let (c, ct) = self.gen_expr(c_e)?;
            (c, ct, a, at, b, bt)
        };
        // All three operands must agree on float type.
        if !(a_ty.is_float() && a_ty == b_ty && a_ty == c_ty) {
            // Operands not all float / not same float type — fall back to
            // the regular path. Emit the operands we already generated;
            // re-evaluating in the caller would duplicate side effects.
            // Easiest: reconstruct via plain fmul+fadd/fsub here.
            // (Reached only if sema let through a mismatch, which it
            // shouldn't.)
            return None;
        }
        let lty = self.lty(&a_ty);
        let intrin = match a_ty {
            Ty::F32 => "@llvm.fmuladd.f32",
            Ty::F64 => "@llvm.fmuladd.f64",
            _ => return None,
        };
        // For (a*b) - c, negate c. For c - (a*b), negate a.
        let (a_final, c_final) = match op {
            BinOp::Add => (a_val, c_val),
            BinOp::Sub => {
                if c_first {
                    // c - (a*b) → fmuladd(-a, b, c)
                    let neg = self.next_tmp();
                    self.emit(&format!("{neg} = fneg contract {lty} {a_val}"));
                    (neg, c_val)
                } else {
                    // (a*b) - c → fmuladd(a, b, -c)
                    let neg = self.next_tmp();
                    self.emit(&format!("{neg} = fneg contract {lty} {c_val}"));
                    (a_val, neg)
                }
            }
            _ => unreachable!(),
        };
        let v = self.next_tmp();
        self.emit(&format!(
            "{v} = call contract {lty} {intrin}({lty} {a_final}, {lty} {b_val}, {lty} {c_final})"
        ));
        Some((v, a_ty))
    }

    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> (String, Ty) {
        // Short-circuit evaluation for && and ||.
        match op {
            BinOp::And => return self.gen_short_circuit(lhs, rhs, true),
            BinOp::Or => return self.gen_short_circuit(lhs, rhs, false),
            _ => {}
        }
        // FMA peephole: `(a * b) + c` / `c + (a * b)` / `(a * b) - c` /
        // `c - (a * b)` on a float type lower to `llvm.fmuladd` (single
        // instruction with one rounding). Matches clang's `-ffp-contract=on`
        // default, which contracts source-level a*b+c to fmuladd at IR-build
        // time. Without this, raytracer-style hot loops were ~50% slower
        // than C even after adding `contract` to fmul/fadd, because the
        // explicit intrinsic conveys more information than fast-math flags.
        // B-10: only contract `a*b+c` into `llvm.fmuladd` when fp-contraction
        // is enabled. Under `--fp-contract=off` we fall through to plain
        // fmul + fadd so float output is bit-identical to C built with
        // `-ffp-contract=off`.
        if self.md.fp_contract.get() && matches!(op, BinOp::Add | BinOp::Sub) {
            if let Some(out) = self.try_emit_fmuladd(op, lhs, rhs) {
                return out;
            }
        }
        let (l, lt) = self.gen_expr(lhs).expect("binary lhs has value");
        let (r, rt) = self.gen_expr(rhs).expect("binary rhs has value");
        // Slice 10.FFI.2: pointer arithmetic `p + n` / `p - n`.
        // Lowers to `getelementptr inbounds T, ptr %p, i64 %n` where
        // T is the pointee. Subtract negates the index first.
        if let Ty::RawPtr(inner_box) = lt.clone() {
            if matches!(op, BinOp::Add | BinOp::Sub) {
                let inner = (*inner_box).clone();
                let inner_lt = self.lty(&inner);
                let idx = if matches!(op, BinOp::Sub) {
                    let neg = self.next_tmp();
                    self.emit(&format!("{neg} = sub i64 0, {r}"));
                    neg
                } else {
                    r.clone()
                };
                let ptr = self.next_tmp();
                self.emit(&format!(
                    "{ptr} = getelementptr inbounds {inner_lt}, ptr {l}, i64 {idx}"
                ));
                return (ptr, lt);
            }
        }
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                if lt.is_float() {
                    let v = self.next_tmp();
                    let fop = match op {
                        BinOp::Add => "fadd",
                        BinOp::Sub => "fsub",
                        BinOp::Mul => "fmul",
                        _ => unreachable!(),
                    };
                    // `contract` enables LLVM to fuse fmul+fadd into fmadd
                    // (one rounding instead of two). Matches clang's default
                    // `-ffp-contract=on`; without it cpc raytracer-style code
                    // ran ~50% slower than the C equivalent because every
                    // dot/scale/madd pair stayed as discrete instructions.
                    let cf = self.fmf();
                    self.emit(&format!("{v} = {fop} {cf}{} {l}, {r}", self.lty(&lt)));
                    return (v, lt);
                }
                // Integer: signed gets debug overflow checks, unsigned wraps.
                if lt.is_signed_int() && self.mode == BuildMode::Debug {
                    return (self.arith_with_overflow_check(op, &lt, &l, &r), lt);
                }
                let v = self.next_tmp();
                let iop = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    BinOp::Mul => "mul",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {} {l}, {r}", self.lty(&lt)));
                (v, lt)
            }
            BinOp::Div => {
                if lt.is_float() {
                    let v = self.next_tmp();
                    let cf = self.fmf();
                    self.emit(&format!("{v} = fdiv {cf}{} {l}, {r}", self.lty(&lt)));
                    return (v, lt);
                }
                (self.divide_with_zero_check(op, &lt, &l, &r), lt)
            }
            BinOp::Mod => {
                // Sema rejects float `%`; only integer reaches here.
                (self.divide_with_zero_check(op, &lt, &l, &r), lt)
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                // Phase 8 slice 8.STR.3: byte-level equality for `str`.
                // Lowering: (len_a == len_b) && (memcmp(p_a, p_b, len_a) == 0).
                // We pre-check length so unequal lengths short-circuit
                // without touching the bytes — same alloca+branch shape
                // used by gen_short_circuit.
                if matches!(lt, Ty::Str) && matches!(op, BinOp::Eq | BinOp::Ne) {
                    let result_slot = self.alloca_anon(Ty::Bool);
                    let lp = self.next_tmp();
                    let ll = self.next_tmp();
                    let rp = self.next_tmp();
                    let rl = self.next_tmp();
                    self.emit(&format!("{lp} = extractvalue {{ ptr, i64 }} {l}, 0"));
                    self.emit(&format!("{ll} = extractvalue {{ ptr, i64 }} {l}, 1"));
                    self.emit(&format!("{rp} = extractvalue {{ ptr, i64 }} {r}, 0"));
                    self.emit(&format!("{rl} = extractvalue {{ ptr, i64 }} {r}, 1"));
                    let len_eq = self.next_tmp();
                    self.emit(&format!("{len_eq} = icmp eq i64 {ll}, {rl}"));
                    let cmp_lbl = self.next_block_label();
                    let unequal_lbl = self.next_block_label();
                    let merge_lbl = self.next_block_label();
                    self.emit_terminator(&format!(
                        "br i1 {len_eq}, label %{cmp_lbl}, label %{unequal_lbl}"
                    ));
                    self.open_block(&cmp_lbl);
                    let mc = self.next_tmp();
                    self.emit(&format!(
                        "{mc} = call i32 @memcmp(ptr {lp}, ptr {rp}, i64 {ll})"
                    ));
                    let mc_eq = self.next_tmp();
                    self.emit(&format!("{mc_eq} = icmp eq i32 {mc}, 0"));
                    // v0.0.7 Slice 1.2: str-eq result store — bool leaf.
                    self.gen_store(&Ty::Bool, &mc_eq, &result_slot);
                    self.emit_terminator(&format!("br label %{merge_lbl}"));
                    self.open_block(&unequal_lbl);
                    self.gen_store(&Ty::Bool, "false", &result_slot);
                    self.emit_terminator(&format!("br label %{merge_lbl}"));
                    self.open_block(&merge_lbl);
                    let v = self.next_tmp();
                    self.gen_load(&v, &Ty::Bool, &result_slot);
                    if matches!(op, BinOp::Ne) {
                        let inv = self.next_tmp();
                        self.emit(&format!("{inv} = xor i1 {v}, true"));
                        return (inv, Ty::Bool);
                    }
                    return (v, Ty::Bool);
                }
                let v = self.next_tmp();
                let cmp = cmp_op_for_type(op, &lt);
                let inst = if lt.is_float() { "fcmp" } else { "icmp" };
                self.emit(&format!("{v} = {inst} {cmp} {} {l}, {r}", self.lty(&lt)));
                (v, Ty::Bool)
            }
            BinOp::AddWrap | BinOp::SubWrap | BinOp::MulWrap => {
                // Wrapping operators emit plain integer `add/sub/mul`
                // regardless of build mode: documents intent and gives
                // predictable wrap behavior in debug too. Sema has already
                // restricted these to integer operands.
                let v = self.next_tmp();
                let iop = match op {
                    BinOp::AddWrap => "add",
                    BinOp::SubWrap => "sub",
                    BinOp::MulWrap => "mul",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {} {l}, {r}", self.lty(&lt)));
                (v, lt)
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                // Phase 3A: plain LLVM `and` / `or` / `xor` on integers.
                // No overflow / range checks — bit ops can't overflow.
                let v = self.next_tmp();
                let iop = match op {
                    BinOp::BitAnd => "and",
                    BinOp::BitOr => "or",
                    BinOp::BitXor => "xor",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {} {l}, {r}", self.lty(&lt)));
                (v, lt)
            }
            BinOp::Shl | BinOp::Shr => {
                // Phase 3A: `shl` for left shift; right shift picks
                // `ashr` for signed (arithmetic — preserves sign bit) or
                // `lshr` for unsigned (logical — fills with zero).
                //
                // Shift count: sema allows any integer type. LLVM
                // requires both operands to have the same type, so we
                // truncate / zero-extend the RHS to the LHS width here
                // using the real evaluated RHS type.
                let lhs_t = self.lty(&lt);
                let coerced_r = self.coerce_int_to_width(&r, &rt, &lt);
                let v = self.next_tmp();
                let iop = match (op, lt.is_signed_int()) {
                    (BinOp::Shl, _) => "shl",
                    (BinOp::Shr, true) => "ashr",
                    (BinOp::Shr, false) => "lshr",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {lhs_t} {l}, {coerced_r}"));
                (v, lt)
            }
        }
    }

    /// Phase 3A: coerce an SSA integer `val` of type `from_ty` to the width
    /// of `to_ty`. Used by shift codegen, which lets the RHS be any
    /// integer type but LLVM `shl/lshr/ashr` requires same-width operands.
    /// Zero-extends widening, truncates narrowing. Returns the SSA name of
    /// the coerced value (the original `val` when widths already match).
    fn coerce_int_to_width(&mut self, val: &str, from_ty: &Ty, to_ty: &Ty) -> String {
        let from_bits = ty_bit_width(from_ty);
        let to_bits = ty_bit_width(to_ty);
        if from_bits == to_bits {
            return val.to_string();
        }
        let from_lt = self.lty(from_ty);
        let to_lt = self.lty(to_ty);
        let r = self.next_tmp();
        // Shift counts are inherently unsigned. zext is the right widening
        // even for signed sema types: i8 -> i64 with zext keeps the count
        // semantically equal (shift amounts >= 0 in valid programs).
        let op = if from_bits < to_bits { "zext" } else { "trunc" };
        self.emit(&format!("{r} = {op} {from_lt} {val} to {to_lt}"));
        r
    }

    /// Emit a debug-mode checked signed `+ - *` using the
    /// `llvm.{sadd,ssub,smul}.with.overflow.iN` intrinsic, where N is chosen
    /// from the operand type. On overflow, trap and `unreachable`; otherwise
    /// extract the result.
    fn arith_with_overflow_check(&mut self, op: BinOp, ty: &Ty, l: &str, r: &str) -> String {
        let intrinsic = match op {
            BinOp::Add => "sadd",
            BinOp::Sub => "ssub",
            BinOp::Mul => "smul",
            _ => unreachable!(),
        };
        let llvm_t = self.lty(&ty);
        let bits = ty_bit_width(&ty);
        let pair = self.next_tmp();
        self.emit(&format!(
            "{pair} = call {{{llvm_t}, i1}} @llvm.{intrinsic}.with.overflow.i{bits}({llvm_t} {l}, {llvm_t} {r})"
        ));
        let overflow_bit = self.next_tmp();
        self.emit(&format!(
            "{overflow_bit} = extractvalue {{{llvm_t}, i1}} {pair}, 1"
        ));
        let trap_lbl = self.next_block_label();
        let cont_lbl = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {overflow_bit}, label %{trap_lbl}, label %{cont_lbl}"
        ));
        self.open_block(&trap_lbl);
        self.emit("call void @llvm.trap()");
        self.emit_terminator("unreachable");
        self.open_block(&cont_lbl);
        let result = self.next_tmp();
        self.emit(&format!(
            "{result} = extractvalue {{{llvm_t}, i1}} {pair}, 0"
        ));
        result
    }

    /// Emit a divide-by-zero check before `sdiv` / `udiv` / `srem` / `urem`.
    /// Trap and `unreachable` on zero (always — both modes per §2.3).
    fn divide_with_zero_check(&mut self, op: BinOp, ty: &Ty, l: &str, r: &str) -> String {
        let llvm_op = match (op, ty.is_signed_int()) {
            (BinOp::Div, true) => "sdiv",
            (BinOp::Div, false) => "udiv",
            (BinOp::Mod, true) => "srem",
            (BinOp::Mod, false) => "urem",
            _ => unreachable!(),
        };
        let llvm_t = self.lty(&ty);
        let zero_check = self.next_tmp();
        self.emit(&format!("{zero_check} = icmp eq {llvm_t} {r}, 0"));
        let trap_lbl = self.next_block_label();
        let ok_lbl = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {zero_check}, label %{trap_lbl}, label %{ok_lbl}"
        ));
        self.open_block(&trap_lbl);
        self.emit("call void @llvm.trap()");
        self.emit_terminator("unreachable");
        self.open_block(&ok_lbl);
        let result = self.next_tmp();
        self.emit(&format!("{result} = {llvm_op} {llvm_t} {l}, {r}"));
        result
    }

    fn gen_short_circuit(&mut self, lhs: &Expr, rhs: &Expr, is_and: bool) -> (String, Ty) {
        // `a && b`:   if a then b else false
        // `a || b`:   if a then true else b
        let result_slot = self.alloca_anon(Ty::Bool);
        let (lv, _) = self.gen_expr(lhs).expect("lhs of && / ||");
        let then_lbl = self.next_block_label();
        let else_lbl = self.next_block_label();
        let merge_lbl = self.next_block_label();
        self.emit_terminator(&format!("br i1 {lv}, label %{then_lbl}, label %{else_lbl}"));

        self.open_block(&then_lbl);
        // v0.0.7 Slice 1.2: short-circuit result stores — bool leaf.
        let (v_then, v_else) = if is_and {
            let (rv, _) = self.gen_expr(rhs).expect("rhs of &&");
            self.gen_store(&Ty::Bool, &rv, &result_slot);
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            self.open_block(&else_lbl);
            self.gen_store(&Ty::Bool, "false", &result_slot);
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            ("rhs".to_string(), "false".to_string())
        } else {
            self.gen_store(&Ty::Bool, "true", &result_slot);
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            self.open_block(&else_lbl);
            let (rv, _) = self.gen_expr(rhs).expect("rhs of ||");
            self.gen_store(&Ty::Bool, &rv, &result_slot);
            self.emit_terminator(&format!("br label %{merge_lbl}"));
            ("true".to_string(), "rhs".to_string())
        };
        let _ = (v_then, v_else);

        self.open_block(&merge_lbl);
        let v = self.next_tmp();
        self.gen_load(&v, &Ty::Bool, &result_slot);
        (v, Ty::Bool)
    }

    fn gen_unary(&mut self, op: UnaryOp, operand: &Expr) -> (String, Ty) {
        // v0.0.12 G-023: const-fold `-LIT` on an unsuffixed numeric literal
        // into the textual constant string. This mirrors how the unsuffixed
        // literal itself flows ("100" is emitted as textual `100` and LLVM
        // accepts it at any int width), so `let x: i64 = -100;` works the
        // same as `let x: i64 = 100;`. With a suffix the literal already
        // pins the type, so the regular SSA path below is correct.
        if let UnaryOp::Neg = op {
            if let ExprKind::IntLit(v, crate::lexer::NumSuffix::None) = &operand.kind {
                return (format!("-{v}"), Ty::I32);
            }
            if let ExprKind::FloatLit(v, crate::lexer::NumSuffix::None) = &operand.kind {
                // Emit the negated value as an LLVM hex-float constant.
                // `format!("-{v}")` would print e.g. `-5` (Rust's `Display`
                // drops the `.0` for a whole f64), and LLVM rejects `double -5`
                // with "integer constant must have integer type". The positive
                // `FloatLit` path emits hex for exactly this reason; mirror it
                // here (via `render_static_float`).
                if let Some(s) = render_static_float(-*v, crate::lexer::NumSuffix::None, &Ty::F64) {
                    return (s, Ty::F64);
                }
            }
        }
        let (v, ty) = self.gen_expr(operand).expect("unary operand has value");
        let r = self.next_tmp();
        match op {
            UnaryOp::Neg => {
                if ty.is_float() {
                    self.emit(&format!("{r} = fneg {} {v}", self.lty(&ty)));
                } else {
                    // Sema only allows signed integers and floats for `-`.
                    // Signed integer negation: in debug, INT_MIN cannot be negated;
                    // we emit `sub` and rely on Phase-3 hardening for that case.
                    self.emit(&format!("{r} = sub {} 0, {v}", self.lty(&ty)));
                }
                (r, ty)
            }
            UnaryOp::Not => {
                self.emit(&format!("{r} = xor i1 {v}, true"));
                (r, Ty::Bool)
            }
            UnaryOp::Deref => {
                // Slice 10.FFI.2: `*p` lowers to a `load` from the
                // pointer. The pointee type comes from the operand's
                // `Ty::RawPtr` payload, NOT from `ty` (which IS the
                // raw-pointer type for the operand). `gen_expr`
                // returned `(v, RawPtr(inner))`; we load `<inner>` from
                // the pointer.
                let inner = match &ty {
                    Ty::RawPtr(i) => (**i).clone(),
                    _ => unreachable!("sema validated operand is RawPtr"),
                };
                // v0.0.7 Slice 1.2: `*p` dereference load — inner type's TBAA leaf.
                self.gen_load(&r, &inner, &v);
                (r, inner)
            }
            UnaryOp::BitNot => {
                // Phase 3A: `~v` lowers to `xor v, -1` (all-bits-set
                // constant), the standard LLVM idiom. Works on every
                // integer width; LLVM picks the right `-1` from the
                // operand type.
                self.emit(&format!("{r} = xor {} {v}, -1", self.lty(&ty)));
                (r, ty)
            }
            _ => unreachable!("sema rejects & / &mut in Phase 1"),
        }
    }

    /// Lower `EnumName::Variant` to its integer literal value (the variant's
    /// declaration index, 0-based). Phase 2A always emits as `i32`.
    fn gen_path(&mut self, segments: &[Ident]) -> (String, Ty) {
        debug_assert_eq!(segments.len(), 2, "Phase 2A paths are 2 segments");
        let enum_name = &segments[0].name;
        let variant_name = &segments[1].name;
        let id = *self
            .types
            .enum_by_name
            .get(enum_name)
            .expect("sema validated enum name");
        let info = &self.types.enum_defs[id.0 as usize];
        let idx = info
            .variants
            .get(variant_name)
            .copied()
            .expect("sema validated variant name");
        // Plain enum (no payloads anywhere): bare i32 tag — Phase 2A path.
        if !info.is_tagged {
            return (idx.to_string(), Ty::Enum(id));
        }
        // Tagged enum, payload-less variant (e.g. `Maybe::None`): construct
        // the full tagged-enum value with the tag set and the payload area
        // left undefined. Result is the loaded aggregate.
        self.gen_tagged_construct(id, idx, &[])
    }

    /// Build a tagged-enum value. Strategy:
    ///   1. alloca `%enum.N` (the named tagged-enum struct).
    ///   2. GEP to field 0 (the i32 tag), store the variant index.
    ///   3. For each payload value, GEP to the payload byte array's i64 slot
    ///      at index k, bitcast to a pointer of the payload type, store.
    ///   4. Load the aggregate to produce the SSA value for the result.
    ///
    /// Payload slot 0 lives at field 1 of the enum struct, then GEP'd by i64
    /// index. Each payload value occupies one i64-aligned slot regardless
    /// of its actual width (Phase 3 simplification — see `write_struct_decls`).
    fn gen_tagged_construct(
        &mut self,
        id: EnumId,
        tag: u32,
        args: &[(String, Ty)],
    ) -> (String, Ty) {
        let enum_ty = Ty::Enum(id);
        let llvm_enum = self.lty(&enum_ty);
        let slot = self.alloca_anon(enum_ty.clone());
        // Store tag at field 0. v0.0.7 Slice 1.2: tag is i32 → i32 leaf.
        let tag_ptr = self.next_tmp();
        self.emit(&format!(
            "{tag_ptr} = getelementptr inbounds {llvm_enum}, ptr {slot}, i32 0, i32 0"
        ));
        self.gen_store(&Ty::I32, &tag.to_string(), &tag_ptr);
        // Store each payload value at its byte offset (shared layout with
        // match extraction + enum-variant drop).
        let ptys: Vec<Ty> = args.iter().map(|(_, t)| t.clone()).collect();
        for (i, (val, ty)) in args.iter().enumerate() {
            let slot_ptr = self.payload_slot_ptr(&llvm_enum, &slot, &ptys, i);
            // v0.0.7 Slice 1.2: payload store — primitive payload types
            // pick up their TBAA leaf via gen_store; aggregate payloads
            // (struct/enum/string/etc.) fall through untagged.
            self.gen_store(ty, val, &slot_ptr);
        }
        // Load the aggregate value (whole enum — gen_load skips TBAA on aggregates).
        let v = self.next_tmp();
        self.gen_load(&v, &enum_ty, &slot);
        (v, enum_ty)
    }

    /// Materialize a tagged-enum value as a pointer (for match destructuring).
    /// If the scrutinee is already a place expression (Ident, Field, Index),
    /// return its slot directly. Otherwise compute the value into a temp
    /// alloca and return that pointer.
    fn enum_scrutinee_ptr(&mut self, scrutinee: &Expr) -> (String, EnumId) {
        // Try a place-form first.
        match &scrutinee.kind {
            ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => {
                let (ptr, ty) = self.gen_place(scrutinee);
                let Ty::Enum(id) = ty else {
                    unreachable!("sema validated")
                };
                (ptr, id)
            }
            _ => {
                let (val, ty) = self.gen_expr(scrutinee).expect("match scrutinee has value");
                let Ty::Enum(id) = ty.clone() else {
                    unreachable!("sema validated")
                };
                let slot = self.alloca_anon(ty.clone());
                // v0.0.7 Slice 1.2: match scrutinee spill — aggregate enum.
                self.gen_store(&ty, &val, &slot);
                (slot, id)
            }
        }
    }

    fn gen_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> Option<(String, Ty)> {
        // v0.0.14 enum-variant drop: if the scrutinee is an owned binding (one
        // with a scope-exit drop registered), matching it *consumes* it — the
        // payload is moved into the arm bindings, so we disarm the scrutinee's
        // drop and register each owning payload binding for its own drop. A
        // borrow-param scrutinee has no drop entry, so `consumed` is false and
        // payload bindings stay borrows (no double-free, no transfer).
        let scrutinee_name = match &scrutinee.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            _ => None,
        };
        let (scr_ptr, enum_id) = self.enum_scrutinee_ptr(scrutinee);
        let consumed = scrutinee_name
            .as_ref()
            .map(|n| self.find_drop_flag(n).is_some())
            .unwrap_or(false);
        let info = self.types.enum_defs[enum_id.0 as usize].clone();
        let llvm_enum = self.lty(&Ty::Enum(enum_id));

        // The result slot is allocated lazily: when the first arm body
        // produces an SSA value, we observe its type and alloca a slot for
        // the match result. All subsequent value-producing arms store into
        // the same slot. (`alloca` lives in entry block regardless of where
        // we emit the request, so creating it mid-function is fine.)
        let mut result_slot: Option<(String, Ty)> = None;

        // Load the tag once. Slice 1B: publish the tag's `[0, N)` range
        // metadata so `-O2`'s switch-simplifier and ConstraintElimination
        // can drop the default arm when sema's exhaustiveness check
        // already covered every variant.
        let n_variants = info.variants.len() as i64;
        let range_md = self.md.register_range(0, n_variants, "i32");
        let tag_val = {
            if info.is_tagged {
                let tag_ptr = self.next_tmp();
                self.emit(&format!(
                    "{tag_ptr} = getelementptr inbounds {llvm_enum}, ptr {scr_ptr}, i32 0, i32 0"
                ));
                let v = self.next_tmp();
                self.emit(&format!(
                    "{v} = load i32, ptr {tag_ptr}, !range !{range_md}"
                ));
                v
            } else {
                // Plain enum: scrutinee is already an i32 tag value.
                let v = self.next_tmp();
                self.emit(&format!(
                    "{v} = load i32, ptr {scr_ptr}, !range !{range_md}"
                ));
                v
            }
        };

        // v0.0.14: disarm the consumed scrutinee's scope-exit drop. Emitted in
        // this pre-switch block (which dominates every arm) and path-sensitive
        // via the runtime flag — if the match isn't reached, the flag stays set
        // and the scrutinee drops normally.
        if consumed {
            if let Some(n) = &scrutinee_name {
                self.mark_moved(n);
            }
        }

        // Build labels per arm + a merge label.
        let merge_lbl = self.next_block_label();
        let mut arm_labels: Vec<String> = Vec::with_capacity(arms.len());
        for _ in arms {
            arm_labels.push(self.next_block_label());
        }
        let default_lbl = self.next_block_label();

        // Find the catch-all arm (Wildcard or Binding) — its label becomes
        // the switch default. If absent, point default at `unreachable`.
        // Sema's exhaustiveness check has already verified the match covers
        // every variant or has a catch-all.
        let catchall_idx = arms.iter().position(|a| {
            matches!(
                a.pattern.kind,
                PatternKind::Wildcard | PatternKind::Binding(_)
            )
        });
        let switch_default = match catchall_idx {
            Some(i) => arm_labels[i].clone(),
            None => default_lbl.clone(),
        };

        // Emit switch: one case per concrete variant arm.
        let mut cases = String::new();
        for (i, arm) in arms.iter().enumerate() {
            if let PatternKind::Variant { variant_name, .. } = &arm.pattern.kind {
                let tag = info
                    .variants
                    .get(&variant_name.name)
                    .copied()
                    .expect("sema validated variant");
                cases.push_str(&format!("    i32 {tag}, label %{}\n", arm_labels[i]));
            }
        }
        self.emit_terminator(&format!(
            "switch i32 {tag_val}, label %{switch_default} [\n{cases}  ]"
        ));

        // Emit each arm body.
        for (i, arm) in arms.iter().enumerate() {
            self.open_block(&arm_labels[i]);
            self.push_scope();
            // Bind pattern values into the arm scope.
            match &arm.pattern.kind {
                PatternKind::Wildcard => {}
                PatternKind::Binding(name) => {
                    // Bind the whole scrutinee to `name`. For an enum that's
                    // a load of the aggregate from the slot we already have.
                    let v = self.next_tmp();
                    let enum_ty = Ty::Enum(enum_id);
                    // v0.0.7 Slice 1.2: enum is aggregate — gen_load/gen_store skip TBAA.
                    let _ = llvm_enum;
                    self.gen_load(&v, &enum_ty, &scr_ptr);
                    let local_slot = self.alloca_named(&name.name, enum_ty.clone());
                    self.gen_store(&enum_ty, &v, &local_slot);
                    // A whole-enum catch-all binding rebinds the (consumed)
                    // scrutinee; the new binding is itself a normal owned local
                    // and is drop-registered through the usual let/bind path, so
                    // nothing extra to do here.
                    self.bind(&name.name, local_slot, enum_ty);
                }
                PatternKind::Variant {
                    variant_name,
                    payload,
                    ..
                } => {
                    let tag = info
                        .variants
                        .get(&variant_name.name)
                        .copied()
                        .expect("sema validated variant");
                    let variant_payload_tys = info
                        .variant_payloads
                        .get(tag as usize)
                        .cloned()
                        .unwrap_or_default();
                    for (pi, pp) in payload.iter().enumerate() {
                        if let PatternKind::Binding(name) = &pp.kind {
                            let pty = variant_payload_tys.get(pi).cloned().unwrap_or(Ty::I32);
                            // Byte-offset GEP (shared with construct/drop) so a
                            // payload after a >8-byte one reads its real bytes.
                            let slot_ptr =
                                self.payload_slot_ptr(&llvm_enum, &scr_ptr, &variant_payload_tys, pi);
                            let v = self.next_tmp();
                            // v0.0.7 Slice 1.2: match-arm payload load/store —
                            // primitive payloads pick up their TBAA leaf;
                            // aggregate payloads (struct, string) fall through.
                            self.gen_load(&v, &pty, &slot_ptr);
                            let local_slot = self.alloca_named(&name.name, pty.clone());
                            self.gen_store(&pty, &v, &local_slot);
                            // v0.0.14: a consumed scrutinee's owning payload is
                            // now re-registered for drop here, closing the leak
                            // when the binding is bound but NOT moved out
                            // (`Owned(s) => { ... }`). The scrutinee's whole-enum
                            // drop is disarmed above, so this binding is the sole
                            // owner of the payload bytes. If the arm DOES move it
                            // out (`=> s`, `=> Wrap(s)`, `=> consume(s)`),
                            // scan_moves marked the name moved → Runtime drop flag
                            // → the move site disarms it (no double-free). If not
                            // moved, the flag stays armed → scope-exit drop fires
                            // (no leak). A non-drop payload registers nothing.
                            self.bind(&name.name, local_slot.clone(), pty.clone());
                            if consumed {
                                self.register_value_drop(&name.name, &local_slot, &pty);
                            }
                        }
                        // Wildcard payload patterns bind nothing.
                    }
                }
            }
            // Emit the arm body. If it produces a value, lazily allocate
            // the result slot (on first value) and store the arm's value.
            let body_val = self.gen_expr(&arm.body);
            if let Some((v, ty)) = body_val {
                if result_slot.is_none() {
                    let s = self.alloca_anon(ty.clone());
                    result_slot = Some((s, ty.clone()));
                }
                let (rs, rt) = result_slot.clone().unwrap();
                // v0.0.7 Slice 1.2: match arm result store.
                self.gen_store(&rt, &v, &rs);
            }
            // v0.0.14: a bare-`Ident` arm body (`=> s`) moves that binding out
            // as the match value — its bytes are now in the result slot, so
            // disarm its scope-exit drop (mirrors gen_block_expr's block-tail
            // handling). gen_expr on a bare Ident doesn't mark_moved itself.
            if let ExprKind::Ident(n) = &arm.body.kind {
                self.mark_moved(n);
            }
            self.pop_scope();
            if !self.terminated {
                self.emit_terminator(&format!("br label %{merge_lbl}"));
            }
        }

        // Default block (only reachable if no catch-all). Sema rejects
        // non-exhaustive matches, so this is dead code — emit `unreachable`
        // for completeness.
        if catchall_idx.is_none() {
            self.open_block(&default_lbl);
            self.emit_terminator("unreachable");
        }

        // Merge: load result if there is one.
        self.open_block(&merge_lbl);
        match &result_slot {
            Some((rs, rt)) => {
                let v = self.next_tmp();
                // v0.0.7 Slice 1.2: match merge result reload.
                self.gen_load(&v, rt, rs);
                Some((v, rt.clone()))
            }
            None => None,
        }
    }

    fn gen_cast(&mut self, expr: &Expr, target: &Type) -> (String, Ty) {
        let (v, from_actual) = self.gen_expr(expr).expect("cast operand has value");
        let to_actual = ty_from(target, self.types);
        // Enums lower to i32 at LLVM level. For cast instruction selection,
        // treat enum operands as their underlying i32 form. Sema disallows
        // int → enum, so we only need to handle the source side.
        let from = if from_actual.is_enum() {
            Ty::I32
        } else {
            from_actual
        };
        let to = to_actual.clone();
        if from == to {
            return (v, to_actual);
        }
        let from_t = self.lty(&from);
        let to_t = self.lty(&to);
        let r = self.next_tmp();
        let inst: &'static str = match (&from, &to) {
            // int → int, same/diff width
            (a, b) if a.is_int() && b.is_int() => {
                let aw = ty_bit_width(a);
                let bw = ty_bit_width(b);
                if bw == aw {
                    // No-op (signed/unsigned reinterpret); emit a bitcast for IR validity.
                    self.emit(&format!("{r} = bitcast {from_t} {v} to {to_t}"));
                    return (r, to);
                } else if bw < aw {
                    "trunc"
                } else if a.is_signed_int() {
                    "sext"
                } else {
                    "zext"
                }
            }
            // bool → int
            (Ty::Bool, b) if b.is_int() => "zext",
            // int → float
            (a, b) if a.is_signed_int() && b.is_float() => "sitofp",
            (a, b) if a.is_unsigned_int() && b.is_float() => "uitofp",
            // float → int
            (a, b) if a.is_float() && b.is_signed_int() => "fptosi",
            (a, b) if a.is_float() && b.is_unsigned_int() => "fptoui",
            // float → float (different widths)
            (a, b) if a.is_float() && b.is_float() => {
                if ty_bit_width(b) > ty_bit_width(a) {
                    "fpext"
                } else {
                    "fptrunc"
                }
            }
            // Phase 11: raw-pointer → raw-pointer reinterpretation.
            // Both ends lower to LLVM `ptr` (opaque pointer model), so the
            // cast is a no-op at the IR level — the SSA value is identical.
            // Return the existing value unchanged with the new Ty.
            (Ty::RawPtr(_), Ty::RawPtr(_)) => {
                return (v, to);
            }
            // Phase 11 / P3: integer → raw pointer. Sema gates on `unsafe`.
            // If the source integer is narrower than i64, zero-extend it first
            // (`inttoptr` requires its operand to match the target pointer
            // width, which is i64 on our supported 64-bit targets).
            (a, Ty::RawPtr(_)) if a.is_int() => {
                let aw = ty_bit_width(a);
                let widened: String = if aw < 64 {
                    let w = self.next_tmp();
                    let zext_inst = if a.is_signed_int() { "sext" } else { "zext" };
                    self.emit(&format!("{w} = {zext_inst} {from_t} {v} to i64"));
                    w
                } else {
                    v.clone()
                };
                self.emit(&format!("{r} = inttoptr i64 {widened} to {to_t}"));
                return (r, to);
            }
            // v0.0.9 Phase 6 (cpc-gaps G-016): raw-pointer → 64-bit integer.
            // Sema's `cast_allowed` only admits `usize` / `u64` / `isize` /
            // `i64` as targets and `check_cast` gates on `unsafe`. Lowers
            // to LLVM `ptrtoint`. All four target widths produce `i64` at
            // the IR level (every C+ 64-bit-int type lowers to LLVM `i64`).
            (Ty::RawPtr(_), b) if matches!(b, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                self.emit(&format!("{r} = ptrtoint {from_t} {v} to {to_t}"));
                return (r, to);
            }
            _ => unreachable!("sema rejects unsupported casts: {:?} → {:?}", from, to),
        };
        self.emit(&format!("{r} = {inst} {from_t} {v} to {to_t}"));
        (r, to)
    }

    fn gen_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
    ) -> Option<(String, Ty)> {
        // Slice 11.FN_PTR: detect indirect calls. Two shapes:
        //   1. Callee is an Ident bound to a local of FnPtr type — load
        //      the pointer from the local's slot, then `call ret %ptr(args)`.
        //   2. Callee is a Field expression where the field's resolved type
        //      is FnPtr — load the pointer from the field address, then
        //      indirect-call. Same struct-of-callbacks pattern.
        // For both, the FnPtr's params/return give us the call signature.
        // Direct named calls (callee is an Ident matching a sig) fall through
        // to the existing gen_named_call path.
        if let ExprKind::Ident(name) = &callee.kind {
            if let Some((slot, ty)) = self.lookup(name).cloned() {
                if let Ty::FnPtr {
                    params,
                    return_type,
                } = ty
                {
                    let v = self.next_tmp();
                    // v0.0.7 Slice 1.2: fn-ptr load — ptr leaf.
                    let fnptr_ty = Ty::RawPtr(Box::new(Ty::Unit));
                    self.gen_load(&v, &fnptr_ty, &slot);
                    return self.gen_indirect_call(&v, &params, &return_type, args);
                }
            }
        }
        if let ExprKind::Field { receiver, name } = &callee.kind {
            // Check if the field type is FnPtr — if so, indirect call.
            // gen_place returns the address; we load the pointer value.
            let (recv_addr, recv_ty) = self.gen_place(receiver);
            if let Ty::Struct(id) = recv_ty {
                let info = &self.types.struct_defs[id.0 as usize];
                if let Some((idx, ft)) = info
                    .fields
                    .iter()
                    .enumerate()
                    .find(|(_, (fname, _))| fname == &name.name)
                    .map(|(i, (_, t))| (i as u32, t.clone()))
                {
                    if matches!(ft, Ty::FnPtr { .. }) {
                        let Ty::FnPtr {
                            params,
                            return_type,
                        } = ft
                        else {
                            unreachable!()
                        };
                        let llvm_struct = llvm_ty(&Ty::Struct(id), self.types);
                        let field_ptr = self.next_tmp();
                        self.emit(&format!(
                            "{field_ptr} = getelementptr inbounds {llvm_struct}, ptr {recv_addr}, i32 0, i32 {idx}"
                        ));
                        let fn_val = self.next_tmp();
                        // v0.0.7 Slice 1.2: fn-ptr field load — ptr leaf.
                        let fnptr_ty = Ty::RawPtr(Box::new(Ty::Unit));
                        self.gen_load(&fn_val, &fnptr_ty, &field_ptr);
                        return self.gen_indirect_call(&fn_val, &params, &return_type, args);
                    }
                }
            }
        }
        match &callee.kind {
            ExprKind::Ident(name) => self.gen_named_call(name, args, type_args),
            ExprKind::Field { receiver, name } => self.gen_method_call(receiver, name, args),
            ExprKind::Path { segments } => self.gen_assoc_call(segments, args),
            _ => unreachable!("sema validates callee shape"),
        }
    }

    /// Slice 11.FN_PTR: indirect call through an SSA pointer value.
    /// Lowers each arg, emits `call <retty> <ptr>(<args>)`. Mirrors the
    /// shape of `gen_named_call` for the per-arg lowering but uses the
    /// callee's `%v` SSA name instead of `@<name>`.
    fn gen_indirect_call(
        &mut self,
        callee_val: &str,
        params: &[Ty],
        return_type: &Ty,
        args: &[Expr],
    ) -> Option<(String, Ty)> {
        // Evaluate each arg to a value. FnPtr params are always value-passed
        // (no `mut`/`move` machinery at the call site — those are call-site
        // contracts, not type-level facts; the fn-pointer abstraction
        // erases them). All callable signatures here are C-ABI (ccc).
        let mut arg_vals: Vec<(String, String)> = Vec::with_capacity(args.len());
        for (a, pty) in args.iter().zip(params.iter()) {
            let (v, _) = self.gen_expr(a).expect("indirect call arg has value");
            arg_vals.push((v, self.lty(pty)));
        }
        let mut arg_str = String::new();
        for (i, (v, t)) in arg_vals.iter().enumerate() {
            if i > 0 {
                arg_str.push_str(", ");
            }
            arg_str.push_str(&format!("{t} {v}"));
        }
        match return_type {
            Ty::Unit => {
                self.emit(&format!("call void {callee_val}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!(
                    "{v} = call {} {callee_val}({arg_str})",
                    self.lty(ret)
                ));
                Some((v, ret.clone()))
            }
        }
    }

    fn gen_named_call(
        &mut self,
        name: &str,
        args: &[Expr],
        type_args: &[Type],
    ) -> Option<(String, Ty)> {
        // v0.0.8 bench-gap finding 1: a call may mutate any local
        // bound by `mut x: T` / `move x: T` in the callee's
        // signature. Drop the field-read memo before evaluating
        // arguments so cached values can't outlive a mutation.
        self.invalidate_field_load_cache();
        // Special case: #println(i32) → call printf with our %d\n format.
        // Phase 8 slice 8.STR.2: also handle #println(str) by extracting
        // (ptr, len) from the fat-pointer value and passing both to
        // printf with the `%.*s\n` format string.
        // v0.0.16: FFI/raw + byte-swap builtins are `#name(...)` intrinsics,
        // lowered by `gen_intrinsic` -> `ffi_builtin_cg`; a bare call never
        // reaches codegen (sema rejects it with a fix-it).
        // v0.0.3 Phase 5 Slice 5A: atomic intrinsics
        // (`__cplus_atomic_<op>_<ty>_<ord>`). Lowered directly to LLVM's
        // `load atomic` / `store atomic` / `atomicrmw` / `cmpxchg`. Sema
        // validated arg count + types + the surrounding `unsafe`; codegen
        // is mechanical.
        if let Some(spec) = crate::atomic::parse_atomic_intrinsic(name) {
            return self.gen_atomic_intrinsic(&spec, args);
        }
        // v0.0.12 G-030 (llama.cplus G-029): standalone memory fence.
        // LLVM's `fence` instruction accepts acquire / release / acq_rel /
        // seq_cst (not monotonic — that would be rejected). The stdlib
        // wrapper passes a `relaxed` arg through as a no-op for parity
        // with C's `atomic_thread_fence(memory_order_relaxed)`.
        if let Some(ord) = crate::atomic::parse_atomic_fence(name) {
            let llvm_ord = match ord {
                "relaxed" => {
                    // No instruction emitted — matches C's behavior.
                    return None;
                }
                "acquire" => "acquire",
                "release" => "release",
                "acqrel"  => "acq_rel",
                "seqcst"  => "seq_cst",
                _ => unreachable!("parse_atomic_fence already validated"),
            };
            self.emit(&format!("fence {llvm_ord}"));
            return None;
        }
        // v0.0.3 Phase 5 Slice 5B: thread spawn/join intrinsics. Sema
        // validated args + JoinHandle[O] shape + the surrounding
        // `unsafe`; codegen mallocs the context, registers a per-O
        // trampoline, and lowers to pthread_create / pthread_join +
        // load/store of the result slot.
        if name == "__cplus_thread_spawn" {
            return self.gen_thread_spawn(args, type_args);
        }
        if name == "__cplus_thread_spawn_with" {
            return self.gen_thread_spawn_with(args, type_args);
        }
        if name == "__cplus_thread_join" {
            return self.gen_thread_join(args, type_args);
        }
        if name == "__cplus_block_on" {
            return self.gen_block_on(args, type_args);
        }
        // v0.0.4 Phase 3 Slice 3A.1: async I/O suspension intrinsic.
        // `__cplus_reactor_wait_read(fd)` — inside an async fn, register
        // the current coroutine handle with the reactor for read-readiness
        // on `fd`, then suspend self. Reactor wakes us when the fd is
        // ready; control returns from the intrinsic call.
        if name == "__cplus_reactor_wait_read" {
            return self.gen_reactor_wait_read(args);
        }
        // v0.0.4 Phase 3 Slice 3A.3: write-side counterpart to wait_read.
        // Suspends until `fd` is write-ready (EVFILT_WRITE).
        if name == "__cplus_reactor_wait_write" {
            return self.gen_reactor_wait_write(args);
        }
        // v0.0.5 Phase 4 Slice 4A: timer-side counterpart. Registers a
        // one-shot EVFILT_TIMER for `ms` ms, then suspends self.
        if name == "__cplus_reactor_wait_timer" {
            return self.gen_reactor_wait_timer(args);
        }
        // v0.0.4 Phase 3 Slice 3A.2: `__cplus_reactor_spawn_local(fut)`
        // pushes a Future's handle onto the reactor's task queue.
        if name == "__cplus_reactor_spawn_local" {
            return self.gen_reactor_spawn_local(args);
        }
        // v0.0.4 Phase 3 Slice 3A.2: `__cplus_reactor_yield_now()`
        // enqueues self + suspends, giving the executor a round-trip
        // to drive other queued tasks.
        if name == "__cplus_reactor_yield_now" {
            return self.gen_reactor_yield_now();
        }
        // v0.0.5 Phase 1C: `__cplus_drop_in_place::[T](p: *T)` lowers to
        // a call to the monomorphized `T::drop(p)` when T has Drop,
        // or to nothing (no-op) when T has no Drop. Used by stdlib
        // containers to invoke inner-T Drop before freeing storage.
        if name == "__cplus_drop_in_place" {
            let t = ty_from(&type_args[0], &self.types);
            let (p_val, _) = self.gen_expr(&args[0]).expect("drop_in_place ptr arg");
            self.gen_drop_in_place(&t, &p_val);
            return None;
        }
        let sig = self
            .sigs
            .get(name)
            .unwrap_or_else(|| panic!("sema validated function exists: missing `{name}`"))
            .clone();
        // v0.0.8 (post-bench-gap): capture-and-clear `pending_musttail`
        // *before* evaluating args. Otherwise any nested Call within an
        // arg expression consumes the flag and emits a spurious
        // `musttail call` for a call that isn't truly in tail position
        // — clang rejects with "musttail call must precede a ret with
        // an optional bitcast" or "cannot guarantee tail call due to
        // mismatched return types". The flag is set by `StmtKind::Return`
        // for the OUTER call shape `return f(args);`; only this OUTER
        // call should pick it up. Sub-call args run with the flag
        // cleared.
        let want_musttail = self.pending_musttail;
        self.pending_musttail = false;
        // Per-arg lowering. `arg_vals[i]` is (ssa-value, llvm-type-string).
        // For pointer-passed `mut x: T` params we take the address of the
        // source place; for value-passed params we evaluate the value and
        // flip the source's drop flag on a `move`.
        let mut arg_vals: Vec<(String, String)> = Vec::with_capacity(args.len());
        // Fixed (declared) params first.
        // v0.0.8 fix B (finish): mirror the callee's param attrs at the
        // call site. clang emits the same `noalias`/`readonly nonnull
        // noundef dereferenceable(N) align A` set on both sides; this
        // helps LLVM's inter-procedural analysis before inlining and
        // matches the IR shape clang produces.
        for (a, (pty, move_flag, mut_flag, restrict_flag)) in args.iter().zip(sig.params.iter()) {
            if param_passes_by_ptr(pty, *move_flag, *mut_flag, self.types) {
                let (addr, _) = self.gen_place(a);
                let attrs =
                    param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, true, self.types);
                let ty_str = if attrs.is_empty() {
                    "ptr".to_string()
                } else {
                    format!("ptr {attrs}")
                };
                arg_vals.push((addr, ty_str));
            } else {
                // v0.0.12 G-034 (llama.cplus G-033): mirror G-027 on the
                // param side. For extern imports, struct-by-value args must
                // honor the AArch64-Darwin / x86_64-sysv C ABI just like
                // the declaration. The import-decl path classifies via
                // `classify_c_abi` (≤8B → Coerce i64, ≤16B → Coerce
                // [2 x i64], >16B → Indirect ptr); the call site previously
                // passed the raw `%T` aggregate, silently mismatching the
                // callee's coerced/indirect signature → SIGSEGV on the
                // first real call.
                if sig.is_extern {
                    match classify_c_abi(pty, self.types) {
                        CAbiClass::Coerce { llvm_ty, size, align } => {
                            let (v, _) = self.gen_expr(a).expect("call arg is a value");
                            let pty_lty = self.lty(pty);
                            let slot = self.alloca_named_raw("arg.coerce", &llvm_ty, align);
                            // Store the struct verbatim, then reload through
                            // the coerced LLVM type. The alloca was sized
                            // for the coerced type (≥ struct size), so
                            // store/load slop falls within the slot.
                            let (_, struct_al) = static_layout(pty, self.types)
                                .unwrap_or((size, align));
                            self.emit(&format!(
                                "store {pty_lty} {v}, ptr {slot}, align {struct_al}"
                            ));
                            let coerced = self.next_tmp();
                            self.emit(&format!(
                                "{coerced} = load {llvm_ty}, ptr {slot}, align {align}"
                            ));
                            arg_vals.push((coerced, llvm_ty));
                            if *move_flag {
                                if let ExprKind::Ident(name) = &a.kind {
                                    self.mark_moved(name);
                                }
                            }
                            continue;
                        }
                        CAbiClass::Indirect => {
                            let (v, _) = self.gen_expr(a).expect("call arg is a value");
                            let pty_lty = self.lty(pty);
                            let (_, al) = static_layout(pty, self.types)
                                .expect("indirect arg has layout");
                            let slot = self.alloca_anon(pty.clone());
                            self.emit(&format!(
                                "store {pty_lty} {v}, ptr {slot}, align {al}"
                            ));
                            // aarch64-darwin doesn't use `byval` here (the
                            // caller-allocated slot is implicitly shared);
                            // x86_64-sysv would — matching the import-decl
                            // side which mirrors the same convention.
                            arg_vals.push((slot, "ptr".to_string()));
                            if *move_flag {
                                if let ExprKind::Ident(name) = &a.kind {
                                    self.mark_moved(name);
                                }
                            }
                            continue;
                        }
                        CAbiClass::Direct => {}
                    }
                }
                let (v, _) = self.gen_expr(a).expect("call arg is a value");
                // v0.0.8 post-bench-gap: mirror `restrict *T` at the
                // scalar-arg call site so it matches the callee's
                // `ptr noalias noundef` signature.
                let ty_str = if *restrict_flag && matches!(pty, Ty::RawPtr(_)) {
                    format!("{} noalias noundef", self.lty(pty))
                } else {
                    self.lty(pty)
                };
                arg_vals.push((v, ty_str));
                if *move_flag {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
            }
        }
        // Slice 10.FFI.4: variadic tail args. Each tail arg evaluated
        // at its natural type and passed by value (the C varargs ABI).
        // No `move` semantics — varargs are inherently bit-copies.
        if sig.is_variadic {
            for a in args.iter().skip(sig.params.len()) {
                let (v, ty) = self.gen_expr(a).expect("varargs tail arg has value");
                arg_vals.push((v, self.lty(&ty)));
            }
        }
        let mut arg_str = String::new();
        for (i, (v, ty)) in arg_vals.iter().enumerate() {
            if i > 0 {
                arg_str.push_str(", ");
            }
            arg_str.push_str(&format!("{ty} {v}"));
        }
        // Slice 10.FFI.4: LLVM requires the full function type for
        // variadic call sites. `call retty (fixed_types, ...) @name(args)`.
        let type_prefix = if sig.is_variadic {
            let mut s = String::from(" (");
            for (i, (pty, _, _, _)) in sig.params.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&self.lty(pty));
            }
            if !sig.params.is_empty() {
                s.push_str(", ");
            }
            s.push_str("...)");
            s
        } else {
            String::new()
        };
        // Phase 11 / ObjC interop: `#[link_name = "..."]` aliases the
        // linker symbol. Call sites use the link_name when present so the
        // call resolves to the same C symbol as the user wrote in the
        // attribute, not the C+ source-level name.
        let symbol: &str = sig.link_name.as_deref().unwrap_or(name);
        // Slice 1E: tail-call optimization. `pending_musttail` was set by
        // `StmtKind::Return` when this call is the last expression
        // before a return and the signatures match. LLVM's verifier
        // rejects `musttail` IR that doesn't truly qualify (e.g.
        // variadic mismatch), so the predicate in StmtKind::Return is
        // conservative. The capture-and-clear now happens at the top of
        // this function (see `want_musttail` above) so nested
        // arg-evaluation Calls can't steal the flag.
        //
        // Slice 1D: detect sret callee. Only applies when the callee is
        // user-defined (sig has no link_name pointing at a C symbol and
        // the callee is non-variadic) and the return type triggers the
        // predicate. The current narrow predicate fires only for `string`
        // returns (24-byte aggregate with Drop).
        // v0.0.3 Slice 1P: widen sret to non-Copy struct returns from
        // non-extern user functions, matching the widened predicate in
        // emit_function_signature. Variadic callees stay on the value-
        // return path.
        //
        // v0.0.12 G-027: for extern *imports*, classify the return via
        // the C ABI rules and use sret when Indirect (>16B aggregate on
        // aarch64-darwin). Previously the call site emitted a direct
        // struct return for any extern callee, which silently mismatched
        // the sret shape clang emitted on the C side → SIGSEGV. The
        // import-declaration path was just updated to emit the matching
        // sret signature; this is the call-side companion.
        let extern_sret = sig.is_extern
            && !sig.is_variadic
            && matches!(classify_c_abi(&sig.return_type, self.types), CAbiClass::Indirect);
        let uses_sret = !sig.is_variadic
            && ((sig.link_name.is_none()
                && !sig.is_extern
                && return_passes_by_sret_widened(&sig.return_type, self.types))
                || extern_sret);
        if uses_sret {
            // musttail + sret would require the caller's own sret slot to
            // be forwarded as the callee's sret arg. Supported when caller
            // and callee both use sret with matching types; the predicate
            // in StmtKind::Return already verified return-type equality.
            let ret = sig.return_type.clone();
            let lty = self.lty(&ret);
            if want_musttail {
                if let Some(caller_slot) = self.sret_slot.clone() {
                    // Forward caller's sret slot into the callee. After
                    // `musttail call void @foo(ptr %caller_slot, ...)` the
                    // function's `ret void` will see the value already
                    // landed at the caller's caller's slot.
                    //
                    // v0.0.4 Phase 1A: LLVM's musttail verifier requires the
                    // call-site sret attribute (and inner type) to match the
                    // callee's declaration. Forwarding bare `ptr %slot`
                    // tripped "mismatched ABI impacting function attributes."
                    // Mirror the attribute string used at the callee's
                    // declaration site ([codegen.rs] sret in
                    // emit_function_signature and emit_method_signature).
                    let (sret_sz, sret_al) =
                        static_layout(&ret, self.types).expect("sret return type has layout");
                    let sret_inner = self.lty(&ret);
                    let sret_attrs = format!(
                        "ptr sret({}) noalias nonnull noundef writable dereferenceable({}) align {} {}",
                        sret_inner, sret_sz, sret_al, caller_slot
                    );
                    let mut head = sret_attrs;
                    if !arg_str.is_empty() {
                        head.push_str(", ");
                        head.push_str(&arg_str);
                    }
                    // v0.0.8 fix C: musttail requires the call-site cc
                    // to match the callee's exactly. If the callee is
                    // fastcc, emit `musttail call fastcc ...`.
                    let cc = self.md.fastcc_prefix(symbol);
                    self.emit(&format!(
                        "musttail call {cc}void{type_prefix} @{symbol}({head})"
                    ));
                    // Return type signaled to upstream — but musttail in
                    // tail position is always followed by `ret void`
                    // emitted by StmtKind::Return. We must NOT supply a
                    // value; emit the terminator now and return None so
                    // StmtKind::Return's value path becomes a no-op.
                    // (StmtKind::Return reads `ret_val` only — passing it
                    // None lands in the (None, Ty::Unit) arm... but
                    // ret_ty is `string`, not Unit. Simpler: emit the
                    // terminator + signal terminated.)
                    self.emit_terminator("ret void");
                    return None;
                }
                // No caller sret — can't forward. Fall through to
                // non-musttail sret call.
            }
            let slot = self.alloca_anon(ret.clone());
            let mut head = format!("ptr {slot}");
            if !arg_str.is_empty() {
                head.push_str(", ");
                head.push_str(&arg_str);
            }
            // v0.0.8 fix C: sret call site picks up the callee's cc too.
            let cc = self.md.fastcc_prefix(symbol);
            self.emit(&format!("call {cc}void{type_prefix} @{symbol}({head})"));
            let v = self.next_tmp();
            // v0.0.7 Slice 1.2: sret-call result reload. `ret` is
            // typically an aggregate (the whole reason for sret), so
            // gen_load skips TBAA — conservative correct default.
            let _ = lty;
            self.gen_load(&v, &ret, &slot);
            return Some((v, ret));
        }
        let call_kind = if want_musttail {
            "musttail call"
        } else {
            "call"
        };
        // v0.0.8 fix C: mirror the callee's cc at the call site. The
        // `fastcc_prefix` lookup keys on the cpc-internal symbol; extern
        // / runtime names lookup-miss and stay default-cc.
        let cc = self.md.fastcc_prefix(symbol);
        match sig.return_type {
            Ty::Unit => {
                self.emit(&format!(
                    "{call_kind} {cc}void{type_prefix} @{symbol}({arg_str})"
                ));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!(
                    "{v} = {call_kind} {cc}{}{type_prefix} @{symbol}({arg_str})",
                    self.lty(&ret)
                ));
                Some((v, ret))
            }
        }
    }

    /// v0.0.3 Phase 5 Slice 5A: lower `__cplus_atomic_*` intrinsics.
    ///
    /// - Load:    `%r = load atomic <ty> ptr <p> <ord>, align <a>`
    /// - Store:   `store atomic <ty> <v>, ptr <p> <ord>, align <a>`
    /// - Xchg/fetch_*: `%r = atomicrmw <op> ptr <p>, <ty> <v> <ord>`
    /// - Cmpxchg: `%pair = cmpxchg ptr <p>, <ty> <e>, <ty> <d> <ord> <ord>`
    ///   then `extractvalue` for the previous-value field.
    ///
    /// Alignment is set to the natural alignment of the operand type
    /// (= operand byte width on every supported width 1/2/4/8). LLVM
    /// requires `align` on atomic load/store; `atomicrmw`/`cmpxchg`
    /// derive alignment from the pointer's pointee type.
    fn gen_atomic_intrinsic(
        &mut self,
        spec: &crate::atomic::AtomicSpec,
        args: &[Expr],
    ) -> Option<(String, Ty)> {
        use crate::atomic::AtomicOp;
        let (p_val, _) = self.gen_expr(&args[0]).expect("atomic ptr arg");
        let llvm_ty = self.lty(&spec.ty);
        let align = spec.bits / 8;
        let ord = spec.llvm_ordering;
        match spec.op {
            AtomicOp::Load => {
                let r = self.next_tmp();
                self.emit(&format!(
                    "{r} = load atomic {llvm_ty}, ptr {p_val} {ord}, align {align}"
                ));
                Some((r, spec.ty.clone()))
            }
            AtomicOp::Store => {
                let (v_val, _) = self.gen_expr(&args[1]).expect("atomic store val arg");
                self.emit(&format!(
                    "store atomic {llvm_ty} {v_val}, ptr {p_val} {ord}, align {align}"
                ));
                None
            }
            AtomicOp::Xchg
            | AtomicOp::FetchAdd
            | AtomicOp::FetchSub
            | AtomicOp::FetchAnd
            | AtomicOp::FetchOr
            | AtomicOp::FetchXor => {
                let opcode = spec.op.rmw_opcode();
                let (v_val, _) = self.gen_expr(&args[1]).expect("atomicrmw val arg");
                let r = self.next_tmp();
                self.emit(&format!(
                    "{r} = atomicrmw {opcode} ptr {p_val}, {llvm_ty} {v_val} {ord}"
                ));
                Some((r, spec.ty.clone()))
            }
            AtomicOp::Cmpxchg => {
                let (e_val, _) = self.gen_expr(&args[1]).expect("cmpxchg expected arg");
                let (d_val, _) = self.gen_expr(&args[2]).expect("cmpxchg desired arg");
                // Failure ordering: LLVM forbids the failure ordering
                // from being stronger than the success ordering AND
                // forbids `release`/`acq_rel` as failure orderings.
                // Map each success ordering to the strongest legal
                // matching failure ordering.
                let fail_ord = match ord {
                    "release" => "monotonic",
                    "acq_rel" => "acquire",
                    other => other,
                };
                let pair = self.next_tmp();
                self.emit(&format!(
                    "{pair} = cmpxchg ptr {p_val}, {llvm_ty} {e_val}, {llvm_ty} {d_val} {ord} {fail_ord}"
                ));
                let prev = self.next_tmp();
                self.emit(&format!(
                    "{prev} = extractvalue {{ {llvm_ty}, i1 }} {pair}, 0"
                ));
                Some((prev, spec.ty.clone()))
            }
        }
    }

    /// v0.0.3 Phase 5 Slice 5B: lower `__cplus_thread_spawn(f)` to:
    ///   1. malloc(8 + size_of(O)) → ctx
    ///   2. store f at ctx[0]
    ///   3. malloc(8) → tid_slot (pthread_create needs writable storage)
    ///   4. pthread_create(tid_slot, NULL, @__cplus_thread_tramp_<O>, ctx)
    ///   5. load tid_slot → tid; free(tid_slot)
    ///   6. insertvalue { i64, ptr } { tid, ctx }
    ///
    /// Returns the `JoinHandle[O]` aggregate value. O is restricted to
    /// Copy types ≤ 8 bytes; non-Copy lands in Slice 5C.
    fn gen_thread_spawn(&mut self, args: &[Expr], type_args: &[Type]) -> Option<(String, Ty)> {
        let o_ty = ty_from(&type_args[0], self.types);
        let (f_val, _f_ty) = self.gen_expr(&args[0]).expect("thread_spawn fn arg");
        if !is_thread_spawn_eligible(&o_ty) {
            // Diagnostic surfaces in sema; here we still must produce
            // some IR to keep the build flow alive for `--emit-ll`
            // smoke paths. Use poison and a JoinHandle-shaped value.
            return Some(("undef".to_string(), self.lookup_join_handle_ty(&o_ty)));
        }
        let tramp_sym = self.tramps.register_spawn(&o_ty, self.types);
        let (size, _) = static_layout(&o_ty, self.types).unwrap_or((8, 8));
        // v0.0.4 Phase 2 Slice 2H ctx layout:
        //   refcount: u64       @ 0   (initialized to 2: parent + worker)
        //   fn_ptr:             @ 8
        //   result_slot:        @ 16
        let total_size = 16 + size;
        let ctx = self.next_tmp();
        self.emit(&format!("{ctx} = call ptr @malloc(i64 {total_size})"));
        // refcount = 2 (plain store — not yet shared with the worker).
        self.emit(&format!("store i64 2, ptr {ctx}, align 8"));
        // fn_ptr at offset 8.
        let fn_slot = self.next_tmp();
        self.emit(&format!(
            "{fn_slot} = getelementptr inbounds i8, ptr {ctx}, i64 8"
        ));
        self.emit(&format!("store ptr {f_val}, ptr {fn_slot}, align 8"));
        let tid_slot = self.next_tmp();
        self.emit(&format!("{tid_slot} = call ptr @malloc(i64 8)"));
        let err = self.next_tmp();
        self.emit(&format!(
            "{err} = call i32 @pthread_create(ptr {tid_slot}, ptr null, ptr @{tramp_sym}, ptr {ctx})"
        ));
        // pthread_create returns 0 on success. Trap on failure so the
        // user doesn't get a zero'd tid silently. v0.0.3 has no
        // unwind/Result story for thread errors per the locked plan
        // decision; aborting is the honest behaviour.
        let ok = self.next_tmp();
        let trap_bb = self.next_block_label();
        let cont_bb = self.next_block_label();
        self.emit(&format!("{ok} = icmp eq i32 {err}, 0"));
        self.emit_terminator(&format!("br i1 {ok}, label %{cont_bb}, label %{trap_bb}"));
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body.push_str("  call void @llvm.trap()\n");
        self.body.push_str("  unreachable\n");
        self.body.push_str(&format!("{cont_bb}:\n"));
        self.terminated = false;
        let tid = self.next_tmp();
        self.emit(&format!("{tid} = load i64, ptr {tid_slot}, align 8"));
        self.emit(&format!("call void @free(ptr {tid_slot})"));
        // Build the JoinHandle[O] aggregate. The stdlib defines its
        // shape as { tid: u64, opaque ctx: *u8 } — sema-resolved Ty is what
        // we ask the type table for the LLVM struct name.
        let handle_ty = self.lookup_join_handle_ty(&o_ty);
        let handle_llvm = self.lty(&handle_ty);
        let agg0 = self.next_tmp();
        let agg1 = self.next_tmp();
        self.emit(&format!(
            "{agg0} = insertvalue {handle_llvm} undef, i64 {tid}, 0"
        ));
        self.emit(&format!(
            "{agg1} = insertvalue {handle_llvm} {agg0}, ptr {ctx}, 1"
        ));
        Some((agg1, handle_ty))
    }

    /// v0.0.3 Phase 5 Slice 5B: lower `__cplus_thread_join(h)` to:
    ///   1. extract tid (field 0) and ctx (field 1) from h
    ///   2. pthread_join(tid, NULL)
    ///   3. load result from ctx + 8
    ///   4. free(ctx)
    ///   5. return the loaded result
    ///
    /// Restricted to Copy O ≤ 8 bytes (matches spawn's eligibility).
    /// v0.0.3 Phase 5 Slice 5C: lower `__cplus_thread_spawn_with::[I, O](input, f)`
    /// to malloc(`8 + size_of(O) + size_of(I)`) → store f at offset 0 →
    /// store input at offset `8 + size_of(O)` → pthread_create with the
    /// per-(I, O) trampoline → return `JoinHandle[O]`. Sharing the
    /// fixed-offset-8 result slot with spawn keeps the join intrinsic
    /// single-shape.
    fn gen_thread_spawn_with(&mut self, args: &[Expr], type_args: &[Type]) -> Option<(String, Ty)> {
        let i_ty = ty_from(&type_args[0], self.types);
        let o_ty = ty_from(&type_args[1], self.types);
        let (input_val, _input_actual_ty) = self.gen_expr(&args[0]).expect("spawn_with input arg");
        // If the input was a named binding being moved into the worker,
        // flip its drop flag so the parent's scope-exit drop doesn't
        // run on memory the worker now owns.
        if let ExprKind::Ident(name) = &args[0].kind {
            self.mark_moved(name);
        }
        let (f_val, _f_ty) = self.gen_expr(&args[1]).expect("spawn_with fn arg");
        if !is_thread_spawn_eligible(&o_ty) || !is_thread_input_eligible(&i_ty, self.types) {
            return Some(("undef".to_string(), self.lookup_join_handle_ty(&o_ty)));
        }
        let tramp_sym = self.tramps.register_spawn_with(&i_ty, &o_ty);
        let (o_size, _) = static_layout(&o_ty, self.types).unwrap_or((8, 8));
        let (i_size, i_align) = static_layout(&i_ty, self.types).unwrap_or((8, 8));
        // v0.0.4 Phase 2 Slice 2H ctx layout:
        //   refcount: u64       @ 0   (initialized to 2: parent + worker)
        //   fn_ptr:             @ 8
        //   result_slot:        @ 16
        //   input_slot:         @ 16 + size_of(O), aligned to align_of(I)
        let input_off_unaligned = 16 + o_size;
        let input_off = (input_off_unaligned + i_align - 1) & !(i_align - 1);
        let total_size = input_off + i_size;
        let ctx = self.next_tmp();
        self.emit(&format!("{ctx} = call ptr @malloc(i64 {total_size})"));
        // refcount = 2 (plain store — not yet shared with the worker).
        self.emit(&format!("store i64 2, ptr {ctx}, align 8"));
        // fn_ptr at offset 8.
        let fn_slot = self.next_tmp();
        self.emit(&format!(
            "{fn_slot} = getelementptr inbounds i8, ptr {ctx}, i64 8"
        ));
        self.emit(&format!("store ptr {f_val}, ptr {fn_slot}, align 8"));
        let input_slot = self.next_tmp();
        self.emit(&format!(
            "{input_slot} = getelementptr i8, ptr {ctx}, i64 {input_off}"
        ));
        let i_llvm = self.lty(&i_ty);
        self.emit(&format!(
            "store {i_llvm} {input_val}, ptr {input_slot}, align {i_align}"
        ));
        let tid_slot = self.next_tmp();
        self.emit(&format!("{tid_slot} = call ptr @malloc(i64 8)"));
        let err = self.next_tmp();
        self.emit(&format!(
            "{err} = call i32 @pthread_create(ptr {tid_slot}, ptr null, ptr @{tramp_sym}, ptr {ctx})"
        ));
        let ok = self.next_tmp();
        let trap_bb = self.next_block_label();
        let cont_bb = self.next_block_label();
        self.emit(&format!("{ok} = icmp eq i32 {err}, 0"));
        self.emit_terminator(&format!("br i1 {ok}, label %{cont_bb}, label %{trap_bb}"));
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body.push_str("  call void @llvm.trap()\n");
        self.body.push_str("  unreachable\n");
        self.body.push_str(&format!("{cont_bb}:\n"));
        self.terminated = false;
        let tid = self.next_tmp();
        self.emit(&format!("{tid} = load i64, ptr {tid_slot}, align 8"));
        self.emit(&format!("call void @free(ptr {tid_slot})"));
        let handle_ty = self.lookup_join_handle_ty(&o_ty);
        let handle_llvm = self.lty(&handle_ty);
        let agg0 = self.next_tmp();
        let agg1 = self.next_tmp();
        self.emit(&format!(
            "{agg0} = insertvalue {handle_llvm} undef, i64 {tid}, 0"
        ));
        self.emit(&format!(
            "{agg1} = insertvalue {handle_llvm} {agg0}, ptr {ctx}, 1"
        ));
        Some((agg1, handle_ty))
    }

    /// v0.0.3 Phase 5 Slice 5E.3: lower `await EXPR` inside an `async fn`.
    /// EXPR evaluates to a `Future[U]`; drive it to completion via a
    /// resume-loop, then extract the result from the coroutine promise.
    /// Requires `self.coro_promise.is_some()` (sema's E0901 enforces).
    /// v0.0.4 Phase 4 Slice 4A: lower `yield EXPR` inside a `gen fn`
    /// body. Stash the value in the coroutine promise, then suspend
    /// (non-final). Resume falls through; ramp exits the gen fn.
    fn gen_yield_expr(&mut self, inner_expr: &Expr) -> Option<(String, Ty)> {
        // Evaluate the yielded value.
        let (val, vty) = self.gen_expr(inner_expr).expect("yield value has SSA");
        let vty_llvm = self.lty(&vty);
        let (_size, align) = match static_layout(&vty, self.types) {
            Some((s, a)) => (s, a),
            None => (8u64, 8u64),
        };
        // Store the value into the coroutine promise. The promise slot
        // is owned by the surrounding gen fn (allocated in gen_gen_function).
        let prom_ptr = self.next_tmp();
        self.emit(&format!(
            "{prom_ptr} = call ptr @llvm.coro.promise(ptr %.coro.hdl, i32 {align}, i1 false)"
        ));
        self.emit(&format!(
            "store {vty_llvm} {val}, ptr {prom_ptr}, align {align}"
        ));
        // Suspend self (non-final). Switched-resume pattern matches
        // wait_read / yield_now: default → ramp-return out of the gen
        // fn; 0 → continue with next statement; 1 → cleanup path.
        let suspend_v = self.next_tmp();
        let resume_bb = self.next_block_label();
        let ramp_bb = self.next_block_label();
        let trap_bb = self.next_block_label();
        self.emit(&format!(
            "{suspend_v} = call i8 @llvm.coro.suspend(token none, i1 false)"
        ));
        self.emit_terminator(&format!(
            "switch i8 {suspend_v}, label %{ramp_bb} [i8 0, label %{resume_bb} i8 1, label %{trap_bb}]"
        ));
        // Ramp-return path: branch to the gen fn's .coro.end (emitted by
        // gen_gen_function's epilogue).
        self.body.push_str(&format!("{ramp_bb}:\n"));
        self.body.push_str("  br label %.coro.end\n");
        // Trap branch: should be unreachable.
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body
            .push_str("  call void @llvm.trap()\n  unreachable\n");
        // Normal-resume branch: continue with the body.
        self.body.push_str(&format!("{resume_bb}:\n"));
        self.terminated = false;
        None
    }

    fn gen_await_expr(&mut self, inner_expr: &Expr) -> Option<(String, Ty)> {
        let (inner_v, inner_ty) = self.gen_expr(inner_expr).expect("await on void expr");
        // Inner must be Future[U]. Pull U out via the struct's generic origin.
        let u_ty = match &inner_ty {
            Ty::Struct(id) => {
                let def = &self.types.struct_defs[id.0 as usize];
                // Codegen StructInfo doesn't carry generic_origin; rely
                // on the convention that Future[U] is `{ ptr }` and U
                // comes from the surrounding expectation. Easiest path:
                // ask sema-side via the resolver's TypeKind, but we
                // don't have that here. Fall back to the field type +
                // expected-type from context. For v0.0.3, await
                // expressions always appear in contexts where the
                // outer's body's coro_promise tells us the type we
                // ultimately stash — but that's the OUTER's T, not
                // the inner's U. Instead, look up by struct name
                // ending with `Future__<u>` and parse the suffix.
                let inner_struct_name = &def.name;
                ty_from_future_name(inner_struct_name, self.types)
            }
            _ => panic!("await of non-Future at codegen — sema should have rejected"),
        };
        let u_llvm = llvm_ty(&u_ty, self.types);
        let u_align = match static_layout(&u_ty, self.types) {
            Some((_, a)) => a,
            None => 8,
        };

        // The Future[U] aggregate is `{ ptr }`. extractvalue needs the
        // full named-struct type (LLVM doesn't unify structurally).
        let inner_hdl = self.next_tmp();
        let future_llvm = self.lty(&inner_ty);
        self.emit(&format!(
            "{inner_hdl} = extractvalue {future_llvm} {inner_v}, 0"
        ));

        let loop_bb = self.next_block_label();
        let resume_bb = self.next_block_label();
        let extract_bb = self.next_block_label();
        let ramp_bb = self.next_block_label();
        let trap_bb = self.next_block_label();
        let done_bb = self.next_block_label();

        // Loop entry — check inner.done.
        self.emit_terminator(&format!("br label %{loop_bb}"));
        self.body.push_str(&format!("{loop_bb}:\n"));
        self.terminated = false;
        let done = self.next_tmp();
        self.emit(&format!(
            "{done} = call i1 @llvm.coro.done(ptr {inner_hdl})"
        ));
        self.emit_terminator(&format!(
            "br i1 {done}, label %{extract_bb}, label %{resume_bb}"
        ));

        // Resume branch — for v0.0.3 (no reactor), inner is already
        // done after the first ramp call (no real suspension). If
        // we ever land here, suspend self normally so the executor
        // can advance us; on resume, drive inner forward then loop
        // back.
        //
        // v0.0.5 Slice 4F: before suspending, register self as an
        // awaiter of inner. When inner reaches its final_suspend
        // epilogue, it'll call `notify_completed(inner_hdl)` which
        // looks up this mapping and enqueues self on the pending
        // queue. Without this, awaiters of deep-nested coroutines
        // stall when their target suspends and later completes —
        // `block_on` only re-resumes the *outermost* future on each
        // loop pass.
        self.body.push_str(&format!("{resume_bb}:\n"));
        self.body.push_str(&format!(
            "  call void @stdlib_reactor_register_awaiter_v1(ptr {inner_hdl}, ptr %.coro.hdl)\n"
        ));
        let suspend_v = self.next_tmp();
        self.body.push_str(&format!(
            "  {suspend_v} = call i8 @llvm.coro.suspend(token none, i1 false)\n"
        ));
        self.body.push_str(&format!(
            "  switch i8 {suspend_v}, label %{ramp_bb} [i8 0, label %{loop_bb} i8 1, label %{trap_bb}]\n"
        ));

        // Ramp-return path — exited via the surrounding fn's coro.end.
        // We branch to .coro.final_suspend? No — we need to return
        // the outer's handle as Pending. The standard pattern is to
        // branch to a "self ramp return" path that exits to the
        // outer's `.coro.end`. Since the outer's coroutine ramp is
        // already wired to return via final_suspend, we just need
        // to fall through to that. Simplest correct: br to a label
        // that branches to `.coro.end`. But `.coro.end` lives in
        // the outer ramp emit. Use the existing `.coro.end` label.
        self.body.push_str(&format!("{ramp_bb}:\n"));
        self.body.push_str("  br label %.coro.end\n");

        // Trap branch — shouldn't be reachable; coro.destroy on the
        // outer would mean the executor gave up.
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body
            .push_str("  call void @llvm.trap()\n  unreachable\n");

        // Extract branch — read the inner promise, destroy inner.
        // v0.0.5 Slice 4A fix: when U is Unit, the promise has no
        // payload to load — emitting `load void, ...` is illegal LLVM.
        // Skip the load and produce the canonical unit value instead.
        self.body.push_str(&format!("{extract_bb}:\n"));
        let result = if matches!(u_ty, Ty::Unit) {
            self.body.push_str(&format!(
                "  call void @llvm.coro.destroy(ptr {inner_hdl})\n"
            ));
            self.body.push_str(&format!("  br label %{done_bb}\n"));
            self.body.push_str(&format!("{done_bb}:\n"));
            self.terminated = false;
            return None;
        } else {
            let inner_prom = self.next_tmp();
            self.body.push_str(&format!(
                "  {inner_prom} = call ptr @llvm.coro.promise(ptr {inner_hdl}, i32 {u_align}, i1 false)\n"
            ));
            let r = self.next_tmp();
            self.body.push_str(&format!(
                "  {r} = load {u_llvm}, ptr {inner_prom}, align {u_align}\n"
            ));
            r
        };
        self.body.push_str(&format!(
            "  call void @llvm.coro.destroy(ptr {inner_hdl})\n"
        ));
        self.body.push_str(&format!("  br label %{done_bb}\n"));

        self.body.push_str(&format!("{done_bb}:\n"));
        self.terminated = false;
        Some((result, u_ty))
    }

    /// v0.0.3 Phase 5 Slice 5E.5: lower `__cplus_block_on::[T](future)`.
    /// Synchronously drives the coroutine to completion via
    /// `coro.resume` until `coro.done`, then reads T from the
    /// coroutine promise, destroys the frame, and returns T.
    fn gen_block_on(&mut self, args: &[Expr], type_args: &[Type]) -> Option<(String, Ty)> {
        let t_ty = ty_from(&type_args[0], self.types);
        let t_llvm = llvm_ty(&t_ty, self.types);
        let t_align = static_layout(&t_ty, self.types)
            .map(|(_, a)| a)
            .unwrap_or(8);
        let (future_v, future_ty) = self.gen_expr(&args[0]).expect("block_on future arg");
        let future_llvm = self.lty(&future_ty);
        let hdl = self.next_tmp();
        self.emit(&format!("{hdl} = extractvalue {future_llvm} {future_v}, 0"));
        // v0.0.4 Phase 3 Slice 3A.1: reactor-integrated drive loop.
        //
        //   loop:
        //     if done(future_hdl): goto extract
        //     resume(future_hdl)
        //     if done(future_hdl): goto extract
        //     drain pending tasks (spawn_local'd, yield_now'd)
        //     if waiter_count() > 0: poll_one_event (blocks on kevent)
        //     goto loop
        //
        // Check done BEFORE resume — async fns run their body eagerly
        // from the call site (no initial_suspend), so the handle may
        // already be done before block_on ever sees it. Resuming a
        // done handle is undefined behavior in LLVM coroutines.
        let loop_bb = self.next_block_label();
        let extract_bb = self.next_block_label();
        let resume_bb = self.next_block_label();
        let drive_bb = self.next_block_label();
        self.emit_terminator(&format!("br label %{loop_bb}"));
        self.body.push_str(&format!("{loop_bb}:\n"));
        self.terminated = false;
        let pre_done = self.next_tmp();
        self.emit(&format!("{pre_done} = call i1 @llvm.coro.done(ptr {hdl})"));
        self.emit_terminator(&format!(
            "br i1 {pre_done}, label %{extract_bb}, label %{resume_bb}"
        ));
        self.body.push_str(&format!("{resume_bb}:\n"));
        self.terminated = false;
        self.body
            .push_str(&format!("  call void @llvm.coro.resume(ptr {hdl})\n"));
        let post_done = self.next_tmp();
        self.emit(&format!("{post_done} = call i1 @llvm.coro.done(ptr {hdl})"));
        self.emit_terminator(&format!(
            "br i1 {post_done}, label %{extract_bb}, label %{drive_bb}"
        ));
        // Drive: drain pending queue, then kevent_wait if there are
        // waiters. Loop back to check done + maybe resume outer.
        self.body.push_str(&format!("{drive_bb}:\n"));
        self.terminated = false;
        self.body
            .push_str("  call i32 @stdlib_reactor_drain_pending_v1()\n");
        self.body
            .push_str("  %.bo.nw = call i32 @stdlib_reactor_waiter_count_v1()\n");
        self.body
            .push_str("  %.bo.has_waiters = icmp sgt i32 %.bo.nw, 0\n");
        let poll_bb = self.next_block_label();
        let loop_skip = self.next_block_label();
        self.body.push_str(&format!(
            "  br i1 %.bo.has_waiters, label %{poll_bb}, label %{loop_skip}\n"
        ));
        self.body.push_str(&format!("{poll_bb}:\n"));
        self.body
            .push_str("  %.bo.poll = call i32 @stdlib_reactor_poll_one_event_v1()\n");
        self.body.push_str(&format!("  br label %{loop_skip}\n"));
        self.body.push_str(&format!("{loop_skip}:\n"));
        self.body.push_str(&format!("  br label %{loop_bb}\n"));
        // Extract path: read the outer's promise, destroy the frame.
        self.body.push_str(&format!("{extract_bb}:\n"));
        self.terminated = false;
        let prom = self.next_tmp();
        self.emit(&format!(
            "{prom} = call ptr @llvm.coro.promise(ptr {hdl}, i32 {t_align}, i1 false)"
        ));
        let result = self.next_tmp();
        self.emit(&format!(
            "{result} = load {t_llvm}, ptr {prom}, align {t_align}"
        ));
        self.emit(&format!("call void @llvm.coro.destroy(ptr {hdl})"));
        Some((result, t_ty))
    }

    /// v0.0.4 Phase 3 Slice 3A.1: `__cplus_reactor_wait_read(fd)` —
    /// register fd + current coroutine handle with the reactor, then
    /// suspend self via `llvm.coro.suspend`. When the reactor wakes us,
    /// fall through. Returns Unit.
    ///
    /// Only valid inside an async fn body (sema enforces). The
    /// coroutine handle comes from the enclosing fn's `%.coro.hdl`
    /// which the async-fn lowering bound during prologue setup.
    fn gen_reactor_wait_read(&mut self, args: &[Expr]) -> Option<(String, Ty)> {
        let (fd_val, _) = self.gen_expr(&args[0]).expect("wait_read fd arg");
        // Use the enclosing coroutine's handle. gen_async_function
        // binds this as `%.coro.hdl` at prologue time.
        self.emit(&format!(
            "call void @stdlib_reactor_register_read_v1(i32 {fd_val}, ptr %.coro.hdl)"
        ));
        // Suspend self. Switched-resume pattern (same shape as await
        // and async-fn final-suspend).
        let suspend_v = self.next_tmp();
        let resume_bb = self.next_block_label();
        let ramp_bb = self.next_block_label();
        let trap_bb = self.next_block_label();
        self.emit(&format!(
            "{suspend_v} = call i8 @llvm.coro.suspend(token none, i1 false)"
        ));
        self.emit_terminator(&format!(
            "switch i8 {suspend_v}, label %{ramp_bb} [i8 0, label %{resume_bb} i8 1, label %{trap_bb}]"
        ));
        // Ramp-return path: yield Pending up to the outer awaiter (or
        // block_on). Falls into the standard `.coro.end` block emitted
        // by gen_async_function's epilogue.
        self.body.push_str(&format!("{ramp_bb}:\n"));
        self.body.push_str("  br label %.coro.end\n");
        // Trap: should be unreachable in a healthy program.
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body
            .push_str("  call void @llvm.trap()\n  unreachable\n");
        // Normal resume: continue with the body.
        self.body.push_str(&format!("{resume_bb}:\n"));
        self.terminated = false;
        None
    }

    /// v0.0.4 Phase 3 Slice 3A.3: `__cplus_reactor_wait_write(fd)` —
    /// mirror of wait_read for write-readiness. Same control-flow shape;
    /// the only difference is registering with EVFILT_WRITE via the
    /// `register_write_v1` stable export.
    fn gen_reactor_wait_write(&mut self, args: &[Expr]) -> Option<(String, Ty)> {
        let (fd_val, _) = self.gen_expr(&args[0]).expect("wait_write fd arg");
        self.emit(&format!(
            "call void @stdlib_reactor_register_write_v1(i32 {fd_val}, ptr %.coro.hdl)"
        ));
        let suspend_v = self.next_tmp();
        let resume_bb = self.next_block_label();
        let ramp_bb = self.next_block_label();
        let trap_bb = self.next_block_label();
        self.emit(&format!(
            "{suspend_v} = call i8 @llvm.coro.suspend(token none, i1 false)"
        ));
        self.emit_terminator(&format!(
            "switch i8 {suspend_v}, label %{ramp_bb} [i8 0, label %{resume_bb} i8 1, label %{trap_bb}]"
        ));
        self.body.push_str(&format!("{ramp_bb}:\n"));
        self.body.push_str("  br label %.coro.end\n");
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body
            .push_str("  call void @llvm.trap()\n  unreachable\n");
        self.body.push_str(&format!("{resume_bb}:\n"));
        self.terminated = false;
        None
    }

    /// v0.0.5 Phase 4 Slice 4A: `__cplus_reactor_wait_timer(ms: u64)` —
    /// register a one-shot EVFILT_TIMER with the reactor for `ms`
    /// milliseconds, then suspend self. Reactor's `poll_one_event` reads
    /// the kevent ident back as `%.coro.hdl` (we set it that way in
    /// register_timer) and resumes us directly.
    fn gen_reactor_wait_timer(&mut self, args: &[Expr]) -> Option<(String, Ty)> {
        let (ms_val, _) = self.gen_expr(&args[0]).expect("wait_timer ms arg");
        self.emit(&format!(
            "call void @stdlib_reactor_register_timer_v1(i64 {ms_val}, ptr %.coro.hdl)"
        ));
        let suspend_v = self.next_tmp();
        let resume_bb = self.next_block_label();
        let ramp_bb = self.next_block_label();
        let trap_bb = self.next_block_label();
        self.emit(&format!(
            "{suspend_v} = call i8 @llvm.coro.suspend(token none, i1 false)"
        ));
        self.emit_terminator(&format!(
            "switch i8 {suspend_v}, label %{ramp_bb} [i8 0, label %{resume_bb} i8 1, label %{trap_bb}]"
        ));
        self.body.push_str(&format!("{ramp_bb}:\n"));
        self.body.push_str("  br label %.coro.end\n");
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body
            .push_str("  call void @llvm.trap()\n  unreachable\n");
        self.body.push_str(&format!("{resume_bb}:\n"));
        self.terminated = false;
        None
    }

    /// v0.0.4 Phase 3 Slice 3A.2: `__cplus_reactor_spawn_local(fut)` —
    /// push a Future's coroutine handle onto the reactor's task queue.
    /// The Future is consumed (its handle propagates to the reactor).
    /// Returns Unit.
    fn gen_reactor_spawn_local(&mut self, args: &[Expr]) -> Option<(String, Ty)> {
        let (fut_val, fut_ty) = self.gen_expr(&args[0]).expect("spawn_local future arg");
        // Future[T] is `{ ptr }`. Extract the handle and enqueue.
        let fut_llvm = self.lty(&fut_ty);
        let hdl = self.next_tmp();
        self.emit(&format!("{hdl} = extractvalue {fut_llvm} {fut_val}, 0"));
        self.emit(&format!(
            "call void @stdlib_reactor_enqueue_pending_v1(ptr {hdl})"
        ));
        None
    }

    /// v0.0.4 Phase 3 Slice 3A.2: `__cplus_reactor_yield_now()` —
    /// enqueue self + suspend. Lets the executor round-trip through
    /// the pending queue before resuming us.
    fn gen_reactor_yield_now(&mut self) -> Option<(String, Ty)> {
        self.emit("call void @stdlib_reactor_enqueue_pending_v1(ptr %.coro.hdl)");
        let suspend_v = self.next_tmp();
        let resume_bb = self.next_block_label();
        let ramp_bb = self.next_block_label();
        let trap_bb = self.next_block_label();
        self.emit(&format!(
            "{suspend_v} = call i8 @llvm.coro.suspend(token none, i1 false)"
        ));
        self.emit_terminator(&format!(
            "switch i8 {suspend_v}, label %{ramp_bb} [i8 0, label %{resume_bb} i8 1, label %{trap_bb}]"
        ));
        self.body.push_str(&format!("{ramp_bb}:\n"));
        self.body.push_str("  br label %.coro.end\n");
        self.body.push_str(&format!("{trap_bb}:\n"));
        self.body
            .push_str("  call void @llvm.trap()\n  unreachable\n");
        self.body.push_str(&format!("{resume_bb}:\n"));
        self.terminated = false;
        None
    }

    fn gen_thread_join(&mut self, args: &[Expr], type_args: &[Type]) -> Option<(String, Ty)> {
        let o_ty = ty_from(&type_args[0], self.types);
        let (h_val, h_ty) = self.gen_expr(&args[0]).expect("thread_join handle arg");
        // The intrinsic consumes the handle (frees ctx). If the source
        // expression is a plain Ident bound as a Drop value, flip its
        // drop flag so the scope-exit destructor doesn't double-free.
        if let ExprKind::Ident(name) = &args[0].kind {
            self.mark_moved(name);
        }
        let handle_llvm = self.lty(&h_ty);
        let tid = self.next_tmp();
        let ctx = self.next_tmp();
        self.emit(&format!("{tid} = extractvalue {handle_llvm} {h_val}, 0"));
        self.emit(&format!("{ctx} = extractvalue {handle_llvm} {h_val}, 1"));
        let _err = self.next_tmp();
        self.emit(&format!(
            "{_err} = call i32 @pthread_join(i64 {tid}, ptr null)"
        ));
        // v0.0.4 Phase 2 Slice 2H: refcounted ctx. After pthread_join
        // returns, the worker has finished and atomically decremented
        // the refcount (down to 1, since parent's ref is still live).
        // Parent reads the result from offset 16, then atomically
        // decrements its own ref; the parent will always observe prev==1
        // here (worker finished its dec before pthread_join returned),
        // so parent does the free.
        if !is_thread_spawn_eligible(&o_ty) || matches!(o_ty, Ty::Unit) {
            // Eligibility-failed: codegen errored at spawn site. Best
            // effort: dec refcount + maybe-free so the IR still validates.
            self.emit_refcount_dec_and_maybe_free(&ctx);
            if matches!(o_ty, Ty::Unit) {
                return None;
            }
            return Some(("undef".to_string(), o_ty));
        }
        let llvm_t = self.lty(&o_ty);
        let align = match &o_ty {
            Ty::I8 | Ty::U8 | Ty::Bool => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            _ => 8,
        };
        let slot = self.next_tmp();
        let result = self.next_tmp();
        // Result slot at offset 16 (after refcount@0 + fn_ptr@8).
        self.emit(&format!("{slot} = getelementptr i8, ptr {ctx}, i64 16"));
        self.emit(&format!(
            "{result} = load {llvm_t}, ptr {slot}, align {align}"
        ));
        self.emit_refcount_dec_and_maybe_free(&ctx);
        Some((result, o_ty))
    }

    /// v0.0.4 Phase 2 Slice 2H helper: atomically decrement the u64
    /// refcount at offset 0 of `ctx`. If the previous value was 1, this
    /// thread held the last reference — free `ctx`.
    fn emit_refcount_dec_and_maybe_free(&mut self, ctx: &str) {
        let prev = self.next_tmp();
        self.emit(&format!("{prev} = atomicrmw sub ptr {ctx}, i64 1 acq_rel"));
        let was_last = self.next_tmp();
        self.emit(&format!("{was_last} = icmp eq i64 {prev}, 1"));
        let free_bb = self.next_block_label();
        let cont_bb = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {was_last}, label %{free_bb}, label %{cont_bb}"
        ));
        self.body.push_str(&format!("{free_bb}:\n"));
        self.body
            .push_str(&format!("  call void @free(ptr {ctx})\n"));
        self.body.push_str(&format!("  br label %{cont_bb}\n"));
        self.body.push_str(&format!("{cont_bb}:\n"));
        self.terminated = false;
    }

    /// Find the `JoinHandle[O]` struct Ty in the type table. Looks
    /// up by the monomorphizer's mangled suffix (`JoinHandle__<suffix>`);
    /// the resolver prefixes that with a per-file path, so we
    /// suffix-match against `.JoinHandle__<suffix>` (or the bare name
    /// for unqualified contexts).
    fn lookup_join_handle_ty(&self, o_ty: &Ty) -> Ty {
        let target = format!(
            "JoinHandle__{}",
            mangle_o_for_tramp_with_types(o_ty, Some(self.types))
        );
        let dotted = format!(".{target}");
        for (idx, d) in self.types.struct_defs.iter().enumerate() {
            if d.name == target || d.name.ends_with(&dotted) {
                return Ty::Struct(StructId(idx as u32));
            }
        }
        // Unreachable when sema is happy — but during sema-error
        // recovery codegen may still be called. Fall back to id 0
        // so the rest of the function compiles.
        Ty::Struct(StructId(0))
    }

    fn gen_method_call(
        &mut self,
        receiver: &Expr,
        name: &Ident,
        args: &[Expr],
    ) -> Option<(String, Ty)> {
        // v0.0.8 bench-gap finding 1: method calls may mutate the
        // receiver (`fn inc(mut self)`) or any `mut`-arg-bound local.
        // Drop the field-read memo before lowering so a cached read
        // can't outlive a mutation. (See `gen_named_call` for the
        // same rationale.)
        self.invalidate_field_load_cache();
        // Phase 8 slice 8.STR.6: blessed `to_string()` on primitives + `str`.
        // The receiver is a primitive value, not a place — handle before
        // gen_place (which expects a place-producing expression).
        if name.name == "to_string" && args.is_empty() {
            let (rv, rt) = self
                .gen_expr(receiver)
                .expect("to_string receiver has value");
            if Self::is_blessed_to_string_receiver_codegen(&rt) {
                return Some(self.gen_to_string_intrinsic(&rv, &rt));
            }
        }
        // v0.0.12 G-045: blessed `to_bits()` on a float scalar → LLVM
        // `bitcast` to the same-width unsigned int. Bit-preserving, zero-cost.
        if name.name == "to_bits" && args.is_empty() {
            let (rv, rt) = self.gen_expr(receiver).expect("to_bits receiver has value");
            let uty = match rt {
                Ty::F16 => Some(Ty::U16),
                Ty::F32 => Some(Ty::U32),
                Ty::F64 => Some(Ty::U64),
                _ => None,
            };
            if let Some(u) = uty {
                let r = self.next_tmp();
                self.emit(&format!(
                    "{r} = bitcast {} {rv} to {}",
                    self.lty(&rt),
                    self.lty(&u)
                ));
                return Some((r, u));
            }
        }
        // v0.0.4 Phase 4 Slice 4B: blessed `next()` on `Iterator[T]`.
        // Inline lowering: check coro.done → if done return None; else
        // read promise into v, resume coroutine, return Some(v).
        if name.name == "next" && args.is_empty() {
            let (rv, rt) = self.gen_expr(receiver).expect("iter.next receiver value");
            if let Some(elem) = self.unwrap_iterator_ty(&rt) {
                return Some(self.gen_iter_next_intrinsic(&rv, &rt, &elem));
            }
        }
        // v0.0.4 Phase 3 Slice 3B.5: blessed `hash()` for primitive + str
        // receivers. Routes to `gen_hash_intrinsic` which emits FNV-1a on
        // the underlying bytes (str) or a multiplicative mixer (integers).
        if name.name == "hash" && args.is_empty() {
            let (rv, rt) = self.gen_expr(receiver).expect("hash receiver has value");
            if Self::is_blessed_hash_receiver_codegen(&rt) {
                return Some(self.gen_hash_intrinsic(&rv, &rt));
            }
        }
        // v0.0.4 Phase 3 Slice 3B.5: blessed `eq(other)` for primitive +
        // str receivers. Lowers to the same icmp / memcmp shape as `==`.
        if name.name == "eq" && args.len() == 1 {
            let (lv, lt) = self.gen_expr(receiver).expect("eq receiver");
            if Self::is_blessed_eq_receiver_codegen(&lt) {
                let (rv, _) = self.gen_expr(&args[0]).expect("eq arg");
                return Some(self.gen_eq_intrinsic(&lv, &rv, &lt));
            }
        }
        // v0.0.12 G-024: blessed `is_null()` / `is_not_null()` on raw
        // pointers. Single `icmp eq/ne ptr %p, null` — no memory access,
        // safe in any context. Sema rejected the call on any non-pointer
        // receiver, so reaching here with a non-pointer is impossible.
        if (name.name == "is_null" || name.name == "is_not_null") && args.is_empty() {
            let (pv, pt) = self.gen_expr(receiver).expect("is_null receiver");
            if matches!(pt, Ty::RawPtr(_)) {
                let r = self.next_tmp();
                let cmp = if name.name == "is_null" { "eq" } else { "ne" };
                self.emit(&format!("{r} = icmp {cmp} ptr {pv}, null"));
                return Some((r, Ty::Bool));
            }
        }
        // v0.0.12 G-028: blessed `write_zeroed()` on `*T` — memset the
        // T-many bytes the pointer refers to. Companion to `#zero::[T]()`.
        // Uses `llvm.memset.p0.i64` (already declared in the preamble).
        if name.name == "write_zeroed" && args.is_empty() {
            let (pv, pt) = self.gen_expr(receiver).expect("write_zeroed receiver");
            if let Ty::RawPtr(inner) = pt {
                if let Some((sz, _al)) = static_layout(&inner, self.types) {
                    if sz > 0 {
                        self.emit(&format!(
                            "call void @llvm.memset.p0.i64(ptr {pv}, i8 0, i64 {sz}, i1 false)"
                        ));
                    }
                }
                return None;
            }
        }
        // Materialize the receiver as a place (pointer) — works for Ident,
        // Field chains, and value-producing temporaries (gen_place handles each).
        let (recv_ptr, recv_ty) = self.gen_place(receiver);
        // Phase 8 slice 8.STR.3: blessed methods on `string` are
        // intrinsic — no MethodSig lookup, no mangled-name call.
        if matches!(recv_ty, Ty::String) {
            return Some(self.gen_string_method_call(&recv_ptr, &name.name, args));
        }
        // v0.0.6 Slice 1B: SIMD instance methods. Load the vector value
        // from the receiver's slot and dispatch. v0.0.9 follow-up: also
        // catch `Ty::Mask` — masks share the SIMD LLVM lowering and go
        // through the same method-codegen path (with sema having
        // already restricted which methods make sense).
        if matches!(&recv_ty, Ty::Simd { .. } | Ty::Mask { .. }) {
            let lty = self.lty(&recv_ty);
            let load_align = simd_align_for(&recv_ty);
            let v = self.next_tmp();
            self.body.push_str(&format!(
                "  {v} = load {lty}, ptr {recv_ptr}, align {load_align}\n"
            ));
            let (rv, rt) = self.gen_simd_method_call(&v, &recv_ty, &name.name, args);
            if matches!(rt, Ty::Unit) {
                return None;
            }
            return Some((rv, rt));
        }
        // v0.0.5 Phase 2C: enum receivers route through the enum
        // method-table (`enum_defs[id].methods`). Same call shape as
        // structs — `ptr` for the receiver followed by the value args.
        if let Ty::Enum(eid) = recv_ty {
            let enum_name = self
                .types
                .enum_by_name
                .iter()
                .find_map(|(n, id)| if *id == eid { Some(n.clone()) } else { None })
                .expect("enum name registered");
            let info = self.types.enum_defs[eid.0 as usize]
                .methods
                .get(&name.name)
                .expect("sema validated")
                .clone();
            // Move-receiver flip mirrors the struct path below.
            if matches!(info.receiver, Some(Receiver::Move)) {
                if let ExprKind::Ident(n) = &receiver.kind {
                    self.mark_moved(n);
                }
            }
            let (v, ret) =
                self.gen_enum_method_call_inner(&recv_ptr, &enum_name, &name.name, &info, args);
            if matches!(ret, Ty::Unit) {
                return None;
            }
            return Some((v, ret));
        }
        let Ty::Struct(id) = recv_ty else {
            unreachable!("sema validated")
        };
        let struct_name = self.types.struct_defs[id.0 as usize].name.clone();
        let info = self.types.struct_defs[id.0 as usize]
            .methods
            .get(&name.name)
            .expect("sema validated")
            .clone();
        let rcv = info.receiver.expect("sema validated instance call");
        let mangled = mangle(&struct_name, &name.name);

        // v0.0.8 fix D: cpc-side inlining for trivial method bodies.
        // When the method matches a known shape (getter, today), emit
        // the equivalent IR directly at the call site instead of going
        // through a `call` instruction. Saves the inliner pass cost +
        // shrinks the IR clang sees. `recv_ptr` is already a pointer to
        // the receiver's place (`gen_place` materialized it above), so
        // the inline reuses the same address regardless of whether the
        // receiver is Copy (by-value at the call ABI) or non-Copy
        // (pointer-passed) — the place pointer is uniform.
        if let Some(TrivialInline::GetField(field_name)) = &info.trivial_inline {
            let struct_info = &self.types.struct_defs[id.0 as usize];
            let field_idx = struct_info.field_index(field_name);
            let field_ty = struct_info.field_type(field_name);
            let llvm_struct = self.lty(&recv_ty);
            let gep = self.next_tmp();
            self.emit(&format!(
                "{gep} = getelementptr inbounds {llvm_struct}, ptr {recv_ptr}, i32 0, i32 {field_idx}"
            ));
            let v = self.next_tmp();
            self.gen_load(&v, &field_ty, &gep);
            return Some((v, field_ty));
        }

        // Build the LLVM call argument list. v0.0.8 fix A: Copy `self`
        // (Read) is passed by value to match `gen_method`'s by-value
        // signature. `mut self` / `move self` stay pointer-passed even on
        // Copy types so writes propagate to the caller's place.
        // v0.0.8 fix B (finish): on the pointer-passed receiver, mirror
        // the callee's receiver attrs at the call site.
        let recv_by_value =
            is_copy_ty(&recv_ty, self.types) && matches!(rcv, Receiver::Read);
        let recv_arg = if recv_by_value {
            let v = self.next_tmp();
            self.gen_load(&v, &recv_ty, &recv_ptr);
            format!("{} {v}", self.lty(&recv_ty))
        } else {
            let (mv, mu) = match rcv {
                Receiver::Read => (false, false),
                Receiver::Mut => (false, true),
                Receiver::Move => (true, true),
            };
            let attrs = param_attrs(&recv_ty, mv, mu, false, true, self.types);
            if attrs.is_empty() {
                format!("ptr {recv_ptr}")
            } else {
                format!("ptr {attrs} {recv_ptr}")
            }
        };
        let mut arg_parts: Vec<String> = vec![recv_arg];
        for (a, (pty, move_flag, mut_flag, restrict_flag)) in args.iter().zip(info.params.iter()) {
            if param_passes_by_ptr(pty, *move_flag, *mut_flag, self.types) {
                let (addr, _) = self.gen_place(a);
                let attrs =
                    param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, true, self.types);
                if attrs.is_empty() {
                    arg_parts.push(format!("ptr {addr}"));
                } else {
                    arg_parts.push(format!("ptr {attrs} {addr}"));
                }
            } else {
                let (v, _) = self.gen_expr(a).expect("call arg has value");
                // v0.0.8 post-bench-gap: mirror `restrict *T` noalias
                // at the scalar-arg call site to match the callee's
                // `ptr noalias noundef` signature.
                let lty = self.lty(pty);
                if *restrict_flag && matches!(pty, Ty::RawPtr(_)) {
                    arg_parts.push(format!("{lty} noalias noundef {v}"));
                } else {
                    arg_parts.push(format!("{lty} {v}"));
                }
                if *move_flag {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
            }
        }
        let arg_str = arg_parts.join(", ");

        // `move self` consumes the receiver: flip its drop flag if the
        // receiver expression was a plain Ident bound as a Drop value.
        if matches!(rcv, Receiver::Move) {
            if let ExprKind::Ident(name) = &receiver.kind {
                self.mark_moved(name);
            }
        }

        // v0.0.3 Slice 1P: method-call sret. Same logic as gen_named_call
        // — non-Copy struct returns flow through a caller-allocated slot
        // so cross-module returns don't double-drop the heap buffer.
        // v0.0.8 fix C: mirror the callee's cc at the call site.
        let cc = self.md.fastcc_prefix(&mangled);
        if return_passes_by_sret_widened(&info.return_type, self.types) {
            let ret = info.return_type.clone();
            let _ = self.lty(&ret);
            let slot = self.alloca_anon(ret.clone());
            let mut head = format!("ptr {slot}");
            if !arg_str.is_empty() {
                head.push_str(", ");
                head.push_str(&arg_str);
            }
            self.emit(&format!("call {cc}void @{mangled}({head})"));
            let v = self.next_tmp();
            // v0.0.7 Slice 1.2: method-call sret reload — aggregate ret.
            self.gen_load(&v, &ret, &slot);
            return Some((v, ret));
        }
        match info.return_type {
            Ty::Unit => {
                self.emit(&format!("call {cc}void @{mangled}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!(
                    "{v} = call {cc}{} @{mangled}({arg_str})",
                    self.lty(&ret)
                ));
                Some((v, ret))
            }
        }
    }

    /// v0.0.5 Phase 2C: emit an enum method call. Same shape as the
    /// struct method-call path — pointer receiver + value/pointer args
    /// per `param_passes_by_ptr`, sret-aware return handling for
    /// non-Copy aggregate returns.
    fn gen_enum_method_call_inner(
        &mut self,
        recv_ptr: &str,
        enum_name: &str,
        method_name: &str,
        info: &MethodInfo,
        args: &[Expr],
    ) -> (String, Ty) {
        let mangled = mangle(enum_name, method_name);
        // v0.0.8 fix B (finish): mirror the callee's receiver + param
        // attrs at the call site (clang emits the same set on both sides).
        let enum_id = *self
            .types
            .enum_by_name
            .get(enum_name)
            .expect("enum name registered");
        let enum_ty = Ty::Enum(enum_id);
        let recv_arg = match info.receiver {
            Some(rcv) => {
                let (mv, mu) = match rcv {
                    Receiver::Read => (false, false),
                    Receiver::Mut => (false, true),
                    Receiver::Move => (true, true),
                };
                let attrs = param_attrs(&enum_ty, mv, mu, false, true, self.types);
                if attrs.is_empty() {
                    format!("ptr {recv_ptr}")
                } else {
                    format!("ptr {attrs} {recv_ptr}")
                }
            }
            None => format!("ptr {recv_ptr}"),
        };
        let mut arg_parts: Vec<String> = vec![recv_arg];
        for (a, (pty, move_flag, mut_flag, restrict_flag)) in args.iter().zip(info.params.iter()) {
            if param_passes_by_ptr(pty, *move_flag, *mut_flag, self.types) {
                let (addr, _) = self.gen_place(a);
                let attrs =
                    param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, true, self.types);
                if attrs.is_empty() {
                    arg_parts.push(format!("ptr {addr}"));
                } else {
                    arg_parts.push(format!("ptr {attrs} {addr}"));
                }
            } else {
                let (v, _) = self.gen_expr(a).expect("call arg has value");
                // v0.0.8 post-bench-gap: mirror `restrict *T` noalias
                // at the scalar-arg call site to match the callee's
                // `ptr noalias noundef` signature.
                let lty = self.lty(pty);
                if *restrict_flag && matches!(pty, Ty::RawPtr(_)) {
                    arg_parts.push(format!("{lty} noalias noundef {v}"));
                } else {
                    arg_parts.push(format!("{lty} {v}"));
                }
                if *move_flag {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
            }
        }
        let arg_str = arg_parts.join(", ");
        // v0.0.8 fix C: mirror callee's cc.
        let cc = self.md.fastcc_prefix(&mangled);
        if return_passes_by_sret_widened(&info.return_type, self.types) {
            let ret = info.return_type.clone();
            let _ = self.lty(&ret);
            let slot = self.alloca_anon(ret.clone());
            let mut head = format!("ptr {slot}");
            if !arg_str.is_empty() {
                head.push_str(", ");
                head.push_str(&arg_str);
            }
            self.emit(&format!("call {cc}void @{mangled}({head})"));
            let v = self.next_tmp();
            // v0.0.7 Slice 1.2: sret reload — aggregate ret.
            self.gen_load(&v, &ret, &slot);
            return (v, ret);
        }
        match &info.return_type {
            Ty::Unit => {
                self.emit(&format!("call {cc}void @{mangled}({arg_str})"));
                // gen_method_call's signature is Option<(String, Ty)>;
                // returning a placeholder isn't quite right but the
                // caller treats Unit specially. Use a fresh undef tmp.
                ("undef".to_string(), Ty::Unit)
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!(
                    "{v} = call {cc}{} @{mangled}({arg_str})",
                    self.lty(ret)
                ));
                (v, ret.clone())
            }
        }
    }

    fn gen_assoc_call(&mut self, segments: &[Ident], args: &[Expr]) -> Option<(String, Ty)> {
        // Sema verified `Type::method` is either an associated function
        // (struct path) or a tagged-enum variant constructor (enum path).
        // Dispatch on the type-segment's kind.
        let type_name = &segments[0].name;
        let method_name = &segments[1].name;
        // Phase 8 slice 8.STR.3: blessed string assoc fns.
        if type_name == "string" {
            return Some(self.gen_string_assoc_call(method_name, args));
        }
        // v0.0.12 G-045: blessed `fN::from_bits(uN)` → LLVM `bitcast` from the
        // unsigned int to the float. Bit-preserving, zero-cost. Pairs with
        // `.to_bits()`.
        if method_name == "from_bits" {
            let fty = match type_name.as_str() {
                "f16" => Some(Ty::F16),
                "f32" => Some(Ty::F32),
                "f64" => Some(Ty::F64),
                _ => None,
            };
            if let Some(f) = fty {
                let (uv, ut) = self.gen_expr(&args[0]).expect("from_bits arg has value");
                let r = self.next_tmp();
                self.emit(&format!(
                    "{r} = bitcast {} {uv} to {}",
                    self.lty(&ut),
                    self.lty(&f)
                ));
                return Some((r, f));
            }
        }
        // v0.0.6 Slice 1B: SIMD associated functions — `f32x4::splat`,
        // `f32x4::new`, `f32x4::from_array`.
        if let Some(simd_ty) = codegen_simd_ty_from_name(type_name) {
            return Some(self.gen_simd_assoc_call(&simd_ty, method_name, args));
        }
        if let Some(&enum_id) = self.types.enum_by_name.get(type_name) {
            // Tagged-enum variant construction with payload.
            let info = &self.types.enum_defs[enum_id.0 as usize];
            let tag = *info
                .variants
                .get(method_name)
                .expect("sema validated variant");
            let mut payload_vals: Vec<(String, Ty)> = Vec::new();
            for a in args {
                let (v, t) = self.gen_expr(a).expect("variant payload has value");
                // v0.0.3 drop-tracking fix: when a non-Copy value is consumed
                // by a variant constructor (`Result::Ok(local_vec)`), the
                // source binding's drop must be disarmed — the new enum value
                // now owns the heap allocation. Without this, both the
                // source local and the enum's payload free at scope exit.
                if !is_copy_ty(&t, self.types) {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
                payload_vals.push((v, t));
            }
            let (v, ty) = self.gen_tagged_construct(enum_id, tag, &payload_vals);
            return Some((v, ty));
        }
        let id = *self
            .types
            .struct_by_name
            .get(type_name)
            .expect("sema validated");
        let info = self.types.struct_defs[id.0 as usize]
            .methods
            .get(method_name)
            .expect("sema validated")
            .clone();
        let mangled = mangle(type_name, method_name);

        // v0.0.8 fix B (finish): mirror the callee's param attrs at the
        // call site (associated functions have no receiver — just args).
        let mut arg_parts: Vec<String> = Vec::new();
        for (a, (pty, move_flag, mut_flag, restrict_flag)) in args.iter().zip(info.params.iter()) {
            if param_passes_by_ptr(pty, *move_flag, *mut_flag, self.types) {
                let (addr, _) = self.gen_place(a);
                let attrs =
                    param_attrs(pty, *move_flag, *mut_flag, *restrict_flag, true, self.types);
                if attrs.is_empty() {
                    arg_parts.push(format!("ptr {addr}"));
                } else {
                    arg_parts.push(format!("ptr {attrs} {addr}"));
                }
            } else {
                let (v, _) = self.gen_expr(a).expect("call arg has value");
                // v0.0.8 post-bench-gap: mirror `restrict *T` noalias
                // at the scalar-arg call site to match the callee's
                // `ptr noalias noundef` signature.
                let lty = self.lty(pty);
                if *restrict_flag && matches!(pty, Ty::RawPtr(_)) {
                    arg_parts.push(format!("{lty} noalias noundef {v}"));
                } else {
                    arg_parts.push(format!("{lty} {v}"));
                }
                if *move_flag {
                    if let ExprKind::Ident(name) = &a.kind {
                        self.mark_moved(name);
                    }
                }
            }
        }
        let arg_str = arg_parts.join(", ");
        // v0.0.3 Slice 1P: static (assoc-fn) calls also need sret-aware
        // dispatch when the method's return type is non-Copy aggregate.
        // The method's `define` signature uses sret (per gen_method's
        // uses_sret branch); the call must match.
        // v0.0.8 fix C: mirror callee's cc.
        let cc = self.md.fastcc_prefix(&mangled);
        if return_passes_by_sret_widened(&info.return_type, self.types) {
            let ret = info.return_type.clone();
            let lty = self.lty(&ret);
            let slot = self.alloca_anon(ret.clone());
            let mut head = format!("ptr {slot}");
            if !arg_str.is_empty() {
                head.push_str(", ");
                head.push_str(&arg_str);
            }
            self.emit(&format!("call {cc}void @{mangled}({head})"));
            let v = self.next_tmp();
            // v0.0.7 Slice 1.2: sret reload — aggregate ret.
            let _ = lty;
            self.gen_load(&v, &ret, &slot);
            return Some((v, ret));
        }
        match info.return_type {
            Ty::Unit => {
                self.emit(&format!("call {cc}void @{mangled}({arg_str})"));
                None
            }
            ret => {
                let v = self.next_tmp();
                self.emit(&format!(
                    "{v} = call {cc}{} @{mangled}({arg_str})",
                    self.lty(&ret)
                ));
                Some((v, ret))
            }
        }
    }

    fn gen_if(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_branch: Option<&Expr>,
    ) -> Option<(String, Ty)> {
        let (cond_v, _) = self.gen_expr(cond).expect("if cond is bool");
        // v0.0.15: no predictor. The result slot is allocated lazily from the
        // `Ty` `gen_expr` actually returns for whichever branch first yields a
        // value (mirrors `gen_match`), killing the drift between a hand-kept
        // type predictor and real codegen. `alloca_anon` pushes to the entry
        // alloca block, so a slot first created inside `then`/`else` still
        // dominates the `merge` load.
        let mut result_slot: Option<(String, Ty)> = None;

        let then_lbl = self.next_block_label();
        let else_lbl = self.next_block_label();
        let merge_lbl = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {cond_v}, label %{then_lbl}, label %{else_lbl}"
        ));

        self.open_block(&then_lbl);
        self.gen_block_into_slot(then, &mut result_slot, &merge_lbl);

        self.open_block(&else_lbl);
        match else_branch {
            Some(eb) => match &eb.kind {
                ExprKind::Block(b) => self.gen_block_into_slot(b, &mut result_slot, &merge_lbl),
                ExprKind::If { .. } => {
                    let v = self.gen_expr(eb);
                    if !self.terminated {
                        if let Some((rv, rt)) = &v {
                            // v0.0.7 Slice 1.2: if-else-if chain result store.
                            self.store_into_result_slot(&mut result_slot, rv, rt);
                        }
                        self.emit_terminator(&format!("br label %{merge_lbl}"));
                    }
                }
                _ => unreachable!("else branch is Block or If per parser"),
            },
            None => {
                self.emit_terminator(&format!("br label %{merge_lbl}"));
            }
        }

        self.open_block(&merge_lbl);
        match result_slot {
            Some((slot, ty)) => {
                let v = self.next_tmp();
                // v0.0.7 Slice 1.2: if-expr merge result reload.
                self.gen_load(&v, &ty, &slot);
                Some((v, ty))
            }
            None => None,
        }
    }

    fn gen_block_into_slot(
        &mut self,
        b: &Block,
        result_slot: &mut Option<(String, Ty)>,
        merge_lbl: &str,
    ) {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
        if !self.terminated {
            if let Some(tail) = &b.tail {
                let v = self.gen_expr(tail);
                if let Some((rv, rt)) = &v {
                    // v0.0.7 Slice 1.2: block-tail value store.
                    self.store_into_result_slot(result_slot, rv, rt);
                    // v0.0.15 double-free fix: a bare-`Ident` block tail used as
                    // an `if`/`else` branch value moves that binding into the
                    // shared result slot (the same move `gen_block_expr` already
                    // disarms for standalone blocks). Flip its drop flag so the
                    // moved-out value isn't freed again at scope exit. `mark_moved`
                    // emits the flag store inside THIS branch's basic block, so it
                    // is runtime-correct for a conditional move: the binding still
                    // drops on the branch that does not move it. Without this, a
                    // `match … { Ok(v) => if c { … } else { v } }` (the vendor/json
                    // `parse` shape) double-freed `v`'s nested heap.
                    if !is_copy_ty(rt, self.types) {
                        if let ExprKind::Ident(name) = &tail.kind {
                            self.mark_moved(name);
                        }
                    }
                }
            }
            self.emit_terminator(&format!("br label %{merge_lbl}"));
        }
        self.pop_scope();
    }

    /// v0.0.15: lazily allocate the shared if/else result slot from the actual
    /// `Ty` a branch produced (the type `gen_expr` returned), then store the
    /// branch value into it. The first value-producing branch fixes the slot
    /// type; sema has already proven both branches agree, so a later branch
    /// reuses the same slot. A `Unit`-typed branch value contributes no slot
    /// (an if-expr used only for effect has no result to merge).
    fn store_into_result_slot(
        &mut self,
        result_slot: &mut Option<(String, Ty)>,
        val: &str,
        ty: &Ty,
    ) {
        if *ty == Ty::Unit {
            return;
        }
        if result_slot.is_none() {
            *result_slot = Some((self.alloca_anon(ty.clone()), ty.clone()));
        }
        let (slot, slot_ty) = result_slot.clone().unwrap();
        self.gen_store(&slot_ty, val, &slot);
    }

    fn gen_block_expr(&mut self, b: &Block) -> Option<(String, Ty)> {
        self.push_scope();
        for s in &b.stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
        let result = if self.terminated {
            None
        } else {
            match &b.tail {
                Some(t) => {
                    let r = self.gen_expr(t);
                    // v0.0.5 Phase 1B: a block whose tail expression is a
                    // bare `Ident(name)` of a non-Copy binding is moving
                    // that value out of the block. The pop_scope below
                    // would otherwise drop the local, freeing the buffer
                    // before the loaded value reaches the caller's slot —
                    // and the caller's drop then fires on a dangling ptr.
                    // Flip the local's drop flag to disarm the scope-exit
                    // drop. (Plain `let b: T = a;` already gets this
                    // disarm via gen_let's Ident-RHS detection; the gap
                    // was the block-tail-as-value form.)
                    if let Some((_, ref rty)) = r {
                        if !is_copy_ty(rty, self.types) {
                            if let ExprKind::Ident(name) = &t.kind {
                                self.mark_moved(name);
                            }
                        }
                    }
                    r
                }
                None => None,
            }
        };
        self.pop_scope();
        result
    }

    fn gen_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr) {
        // v0.0.8 bench-gap finding 1: a compound or plain assignment
        // mutates the target. Drop the field-read memo so any reads
        // after the assignment see the new value, not the cached one.
        self.invalidate_field_load_cache();

        // v0.0.8 bench-gap finding 2: `place = StructLit{...}` fast
        // path. Bypass the intermediate-alloca-and-aggregate-copy
        // dance by storing each RHS field directly into the
        // destination's slot. Closes the hashmap-insert hot path
        // (`table[idx] = Entry { key, val };`) and any similar shape.
        //
        // Limitations:
        //   - Plain `=` only; compound assigns need the LHS read first.
        //   - The destination must lower to a struct of the same type
        //     as the literal (sema enforces this).
        //   - Generic struct literals share the field-list shape, so
        //     the same fast path applies after monomorphization.
        if matches!(op, AssignOp::Assign) {
            let lit_fields: Option<(&Ident, &[StructLitField])> = match &value.kind {
                ExprKind::StructLit { name, fields } => Some((name, fields.as_slice())),
                ExprKind::GenericStructLit { name, fields, .. } => Some((name, fields.as_slice())),
                _ => None,
            };
            if let Some((name, fields)) = lit_fields {
                if let Some(&id) = self.types.struct_by_name.get(&name.name) {
                    let info = self.types.struct_defs[id.0 as usize].clone();
                    let struct_ty = Ty::Struct(id);
                    let llvm_struct = self.lty(&struct_ty);
                    let (dest_slot, _dest_ty) = self.gen_place(target);
                    for f in fields {
                        let (val, _) = self
                            .gen_expr(&f.value)
                            .expect("struct-literal field init has value");
                        let idx = info.field_index(&f.name.name);
                        let field_ty = info.field_type(&f.name.name);
                        let ptr = self.next_tmp();
                        self.emit(&format!(
                            "{ptr} = getelementptr inbounds {llvm_struct}, ptr {dest_slot}, i32 0, i32 {idx}"
                        ));
                        self.gen_store(&field_ty, &val, &ptr);
                        // G-023 fix: same mark_moved as gen_struct_lit.
                        if let ExprKind::Ident(n) = &f.value.kind {
                            self.mark_moved(n);
                        }
                    }
                    return;
                }
            }
        }

        // Compute the place slot (Ident or Field chain). gen_place returns
        // a pointer that we can store to directly.
        let (slot, target_ty) = self.gen_place(target);
        let (rhs_v, _) = self.gen_expr(value).expect("assigned value");
        let _ = self.lty(&target_ty);
        // v0.0.3 Slice 3A: compound assigns. For `a OP= b`, lower as
        // load + binary op + store. Plain `=` is just store.
        // v0.0.7 Slice 1.2: assignment store is the HOTTEST primitive
        // store site in a typical function — drives raytracer perf.
        let to_store = if matches!(op, AssignOp::Assign) {
            rhs_v
        } else {
            let cur = self.next_tmp();
            self.gen_load(&cur, &target_ty, &slot);
            self.gen_compound_op(op, &target_ty, &cur, &rhs_v)
        };
        self.gen_store(&target_ty, &to_store, &slot);
        // G-023 fix: a plain `=` whose RHS was a bare-Ident source
        // consumes that binding into the destination slot. The most-
        // cited surface is the raw-pointer store inside
        // `unsafe { *p = val; }` (Box::new[T], arena::alloc[T]); also
        // covers plain `x = y` between local bindings. Compound
        // assigns read+modify and don't transfer ownership — they
        // skip this. mark_moved is a no-op for non-Drop bindings.
        if matches!(op, AssignOp::Assign) {
            if let ExprKind::Ident(n) = &value.kind {
                self.mark_moved(n);
            }
        }
    }

    /// Lower one compound-assign binary op given pre-evaluated SSA values.
    /// `+=`/`-=`/`*=` use the same debug-overflow path as the plain
    /// `+`/`-`/`*` binary ops; `/=`/`%=` use the zero-check path; bitwise
    /// + shift assigns lower to single LLVM instructions.
    fn gen_compound_op(&mut self, op: AssignOp, ty: &Ty, l: &str, r: &str) -> String {
        let lty = self.lty(ty);
        match op {
            AssignOp::AddAssign | AssignOp::SubAssign | AssignOp::MulAssign => {
                if ty.is_float() {
                    let v = self.next_tmp();
                    let fop = match op {
                        AssignOp::AddAssign => "fadd",
                        AssignOp::SubAssign => "fsub",
                        AssignOp::MulAssign => "fmul",
                        _ => unreachable!(),
                    };
                    let cf = self.fmf();
                    self.emit(&format!("{v} = {fop} {cf}{lty} {l}, {r}"));
                    return v;
                }
                let bin = match op {
                    AssignOp::AddAssign => BinOp::Add,
                    AssignOp::SubAssign => BinOp::Sub,
                    AssignOp::MulAssign => BinOp::Mul,
                    _ => unreachable!(),
                };
                if ty.is_signed_int() && self.mode == BuildMode::Debug {
                    return self.arith_with_overflow_check(bin, ty, l, r);
                }
                let v = self.next_tmp();
                let iop = match op {
                    AssignOp::AddAssign => "add",
                    AssignOp::SubAssign => "sub",
                    AssignOp::MulAssign => "mul",
                    _ => unreachable!(),
                };
                self.emit(&format!("{v} = {iop} {lty} {l}, {r}"));
                v
            }
            AssignOp::DivAssign => {
                if ty.is_float() {
                    let v = self.next_tmp();
                    let cf = self.fmf();
                    self.emit(&format!("{v} = fdiv {cf}{lty} {l}, {r}"));
                    return v;
                }
                self.divide_with_zero_check(BinOp::Div, ty, l, r)
            }
            AssignOp::ModAssign => self.divide_with_zero_check(BinOp::Mod, ty, l, r),
            AssignOp::BitAndAssign => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = and {lty} {l}, {r}"));
                v
            }
            AssignOp::BitOrAssign => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = or {lty} {l}, {r}"));
                v
            }
            AssignOp::BitXorAssign => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = xor {lty} {l}, {r}"));
                v
            }
            AssignOp::ShlAssign => {
                let v = self.next_tmp();
                self.emit(&format!("{v} = shl {lty} {l}, {r}"));
                v
            }
            AssignOp::ShrAssign => {
                let v = self.next_tmp();
                let op_str = if ty.is_signed_int() { "ashr" } else { "lshr" };
                self.emit(&format!("{v} = {op_str} {lty} {l}, {r}"));
                v
            }
            AssignOp::Assign => unreachable!("plain assign handled in gen_assign"),
        }
    }

    // ---------- Phase 8 slice 8.STR.B.2: interpolation codegen ----------
    //
    // Lowers `"hello ${name}, n is ${n}"` to:
    //   1. For each Lit part: lookup the @.str.N global, take (ptr, len).
    //   2. For each Expr part: evaluate. If primitive/str, invoke the
    //      blessed to_string intrinsic to produce a `string`. If
    //      already `string`, use as-is.
    //   3. Compute total length = sum of all part lengths.
    //   4. malloc(total).
    //   5. memcpy each part's bytes into the buffer at the running offset.
    //   6. Build the result aggregate `{ buf, total, total }`.
    //
    // v1 leak: any Expr-derived `string`'s buffer is malloc'd inside
    // to_string and never freed. Matches the broader 8.STR.3 leak
    // policy; Drop integration is a follow-up.

    fn gen_interp_str(&mut self, parts: &[crate::ast::InterpStrPart]) -> (String, Ty) {
        use crate::ast::InterpStrPart;
        // First pass: produce a (ptr, len) pair per part.
        let mut piece_ptrs: Vec<String> = Vec::with_capacity(parts.len());
        let mut piece_lens: Vec<String> = Vec::with_capacity(parts.len());
        // Per-segment conversion buffers (int/float/bool/str → string) are
        // freshly malloc'd just to be copied into the output buffer below.
        // Collect them so we can free them once the copy is done — otherwise
        // each `${non_string}` segment leaks its scratch buffer. A
        // `Ty::String` operand is excluded: that value is owned by its
        // binding (dropped at scope exit), so freeing it here would
        // double-free.
        let mut temps_to_free: Vec<String> = Vec::new();
        for p in parts {
            match p {
                InterpStrPart::Lit(s) => {
                    let (symbol, len) = self
                        .str_lits
                        .get(s)
                        .expect("interp lit part missing from str_lits table")
                        .clone();
                    piece_ptrs.push(symbol);
                    piece_lens.push(format!("{len}"));
                }
                InterpStrPart::Expr(e) => {
                    let (v, t) = self.gen_expr(e).expect("interp expr has value");
                    // Convert to a string aggregate. Every arm except the
                    // `Ty::String` passthrough allocates a fresh buffer.
                    let (sv, is_temp) = match t {
                        Ty::String => (v, false),
                        Ty::Str => (self.gen_to_string_str(&v).0, true),
                        Ty::Bool => (self.gen_to_string_bool(&v).0, true),
                        Ty::F32 | Ty::F64 => (self.gen_to_string_float(&v, &t).0, true),
                        ref rt if rt.is_signed_int() => {
                            (self.gen_to_string_signed(&v, rt).0, true)
                        }
                        ref rt if rt.is_unsigned_int() => {
                            (self.gen_to_string_unsigned(&v, rt).0, true)
                        }
                        _ => unreachable!("sema validated interp expr type"),
                    };
                    // Extract ptr+len.
                    let pp = self.next_tmp();
                    self.emit(&format!("{pp} = extractvalue {{ ptr, i64, i64 }} {sv}, 0"));
                    let lp = self.next_tmp();
                    self.emit(&format!("{lp} = extractvalue {{ ptr, i64, i64 }} {sv}, 1"));
                    if is_temp {
                        temps_to_free.push(pp.clone());
                    }
                    piece_ptrs.push(pp);
                    piece_lens.push(lp);
                }
            }
        }
        // Compute total length via accumulating adds.
        let mut total = String::from("0");
        for l in &piece_lens {
            let next = self.next_tmp();
            self.emit(&format!("{next} = add i64 {total}, {l}"));
            total = next;
        }
        // Allocate the output buffer.
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 {total})"));
        // memcpy each piece at the running offset.
        let mut offset = String::from("0");
        for (ptr, len) in piece_ptrs.iter().zip(piece_lens.iter()) {
            let dst = self.next_tmp();
            self.emit(&format!(
                "{dst} = getelementptr i8, ptr {buf}, i64 {offset}"
            ));
            let _cpy = self.next_tmp();
            self.emit(&format!(
                "{_cpy} = call ptr @memcpy(ptr {dst}, ptr {ptr}, i64 {len})"
            ));
            let next_off = self.next_tmp();
            self.emit(&format!("{next_off} = add i64 {offset}, {len}"));
            offset = next_off;
        }
        // All segment bytes are now copied into `buf`; release the scratch
        // conversion buffers (String operands were never collected here).
        for tmp in &temps_to_free {
            self.emit(&format!("call void @free(ptr {tmp})"));
        }
        let v = self.string_aggregate(&buf, &total, &total);
        (v, Ty::String)
    }

    // ---------- Phase 8 slice 8.STR.6: blessed `to_string()` ----------

    fn is_blessed_to_string_receiver_codegen(ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::I8
                | Ty::I16
                | Ty::I32
                | Ty::I64
                | Ty::Isize
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::Usize
                | Ty::F32
                | Ty::F64
                | Ty::Bool
                | Ty::Str
        )
    }

    /// v0.0.4 Phase 4 Slice 4B: given a `Ty::Struct(id)` whose name
    /// matches `Iterator__<U>`, recover U. Returns None for non-Iterator
    /// types.
    fn unwrap_iterator_ty(&self, ty: &Ty) -> Option<Ty> {
        let Ty::Struct(id) = ty else {
            return None;
        };
        let name = self.types.struct_defs[id.0 as usize].name.clone();
        // G-026 fix: the inner T's mangled name can contain `.` (e.g.
        // `Iterator__src.main.Value` when Value lives in src/main).
        // Don't naively split on the rightmost `.` — find the
        // `Iterator__` marker anywhere in the qualified name and take
        // everything after it. Prefer the LAST occurrence so a type
        // literally named `Iterator__Iterator__T` would resolve to its
        // outermost Iterator wrap.
        let suffix = if let Some(idx) = name.rfind("Iterator__") {
            &name[idx + "Iterator__".len()..]
        } else if let Some(rest) = name.strip_prefix("Iterator__") {
            rest
        } else {
            return None;
        };
        // Reuse the future-name-decoder; the suffix grammar is identical.
        let synthetic = format!("Future__{suffix}");
        Some(ty_from_future_name(&synthetic, self.types))
    }

    /// v0.0.4 Phase 4 Slice 4B: emit IR for `it.next()`. Returns
    /// `Option[T]`. Algorithm:
    ///   1. Extract handle from the Iterator aggregate.
    ///   2. If coro.done(hdl) → return Option::None.
    ///   3. Else read T from the coroutine promise.
    ///   4. coro.resume(hdl) to advance for the next call.
    ///   5. Wrap T in Option::Some and return.
    fn gen_iter_next_intrinsic(&mut self, rv: &str, rt: &Ty, elem: &Ty) -> (String, Ty) {
        // The Iterator aggregate is `{ ptr }`. Extract the handle.
        let iter_llvm = self.lty(rt);
        let hdl = self.next_tmp();
        self.emit(&format!("{hdl} = extractvalue {iter_llvm} {rv}, 0"));

        // Resolve the concrete `Option[T]` enum type that sema instantiated.
        let option_ty = self.lookup_option_ty(elem);
        let option_llvm = self.lty(&option_ty);
        let option_align = match static_layout(&option_ty, self.types) {
            Some((_, a)) => a,
            None => 8,
        };
        let elem_align = match static_layout(elem, self.types) {
            Some((_, a)) => a,
            None => 8,
        };
        let elem_llvm = self.lty(elem);

        // Result slot for the Option[T] aggregate we'll return.
        let result_slot = self.alloca_anon(option_ty.clone());

        let done = self.next_tmp();
        self.emit(&format!("{done} = call i1 @llvm.coro.done(ptr {hdl})"));
        let none_bb = self.next_block_label();
        let some_bb = self.next_block_label();
        let join_bb = self.next_block_label();
        self.emit_terminator(&format!("br i1 {done}, label %{none_bb}, label %{some_bb}"));

        // None arm: discriminant 1 (Option's variants are Some=0, None=1
        // by declaration order in stdlib/option.cplus).
        self.open_block(&none_bb);
        let none_agg = self.build_option_none_aggregate(&option_llvm);
        self.emit(&format!(
            "store {option_llvm} {none_agg}, ptr {result_slot}, align {option_align}"
        ));
        self.emit_terminator(&format!("br label %{join_bb}"));

        // Some arm: read promise, resume, build Some(v).
        self.open_block(&some_bb);
        let prom_ptr = self.next_tmp();
        self.emit(&format!(
            "{prom_ptr} = call ptr @llvm.coro.promise(ptr {hdl}, i32 {elem_align}, i1 false)"
        ));
        let val = self.next_tmp();
        self.emit(&format!(
            "{val} = load {elem_llvm}, ptr {prom_ptr}, align {elem_align}"
        ));
        self.emit(&format!("call void @llvm.coro.resume(ptr {hdl})"));
        let some_agg = self.build_option_some_aggregate(&option_ty, elem, &val);
        self.emit(&format!(
            "store {option_llvm} {some_agg}, ptr {result_slot}, align {option_align}"
        ));
        self.emit_terminator(&format!("br label %{join_bb}"));

        self.open_block(&join_bb);
        let result = self.next_tmp();
        self.emit(&format!(
            "{result} = load {option_llvm}, ptr {result_slot}, align {option_align}"
        ));
        (result, option_ty)
    }

    /// Build an `Option::None` aggregate value (just the tag set to 1).
    fn build_option_none_aggregate(&mut self, option_llvm: &str) -> String {
        // Option[T] lowers as `%enum.<id> = type { i32, [N x i64] }` per
        // the v0.0.2 tagged-enum scheme. None = tag 1, payload undef.
        let t1 = self.next_tmp();
        self.emit(&format!("{t1} = insertvalue {option_llvm} undef, i32 1, 0"));
        t1
    }

    /// Build an `Option::Some(v)` aggregate by writing the tag (0) then
    /// the payload value into the payload slot. Uses an alloca + store
    /// to handle arbitrary T layout; reloads as the option aggregate.
    fn build_option_some_aggregate(&mut self, option_ty: &Ty, elem: &Ty, val: &str) -> String {
        let option_llvm = self.lty(option_ty);
        let option_align = match static_layout(option_ty, self.types) {
            Some((_, a)) => a,
            None => 8,
        };
        let elem_llvm = self.lty(elem);
        let elem_align = match static_layout(elem, self.types) {
            Some((_, a)) => a,
            None => 8,
        };
        // Alloca for the option aggregate; write tag=0, payload=val.
        let slot = self.alloca_anon(option_ty.clone());
        // Zero-init the slot so the payload bytes past T are clean (the
        // payload array is sized for the worst-case variant).
        self.emit(&format!(
            "call void @llvm.memset.p0.i64(ptr {slot}, i8 0, i64 ptrtoint (ptr getelementptr ({option_llvm}, ptr null, i64 1) to i64), i1 false)"
        ));
        // Tag at offset 0 (i32).
        self.emit(&format!("store i32 0, ptr {slot}, align {option_align}"));
        // Payload starts at offset 8 (i32 tag + 4 bytes padding to align
        // for the 8-byte payload field). Get a payload pointer via GEP
        // into the aggregate's payload member (field 1, index 0).
        let pay_ptr = self.next_tmp();
        self.emit(&format!(
            "{pay_ptr} = getelementptr inbounds {option_llvm}, ptr {slot}, i32 0, i32 1, i32 0"
        ));
        // Cast pay_ptr to ptr-to-elem and store val.
        // (LLVM opaque pointers — no bitcast needed.)
        let pay_elem_ptr = pay_ptr.clone();
        self.emit(&format!(
            "store {elem_llvm} {val}, ptr {pay_elem_ptr}, align {elem_align}"
        ));
        // Reload as the option aggregate.
        let loaded = self.next_tmp();
        self.emit(&format!(
            "{loaded} = load {option_llvm}, ptr {slot}, align {option_align}"
        ));
        loaded
    }

    /// Look up the concrete `Option[T]` enum type table entry for an
    /// element type T. Mirrors `lookup_future_ty`'s suffix-match scheme.
    fn lookup_option_ty(&self, inner: &Ty) -> Ty {
        let target = format!(
            "Option__{}",
            mangle_o_for_tramp_with_types(inner, Some(self.types))
        );
        let dotted = format!(".{target}");
        for (name, id) in &self.types.enum_by_name {
            if name == &target || name.ends_with(&dotted) {
                return Ty::Enum(*id);
            }
        }
        Ty::Enum(EnumId(0))
    }

    fn is_blessed_hash_receiver_codegen(ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::I8
                | Ty::I16
                | Ty::I32
                | Ty::I64
                | Ty::Isize
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::Usize
                | Ty::Str
        )
    }

    fn is_blessed_eq_receiver_codegen(ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::I8
                | Ty::I16
                | Ty::I32
                | Ty::I64
                | Ty::Isize
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::Usize
                | Ty::Bool
                | Ty::Str
        )
    }

    /// v0.0.4 Phase 3 Slice 3B.5: emit a u64 hash for an arbitrary
    /// primitive or str receiver. Strategy:
    ///   - integer: widen to i64 / zext / sext, multiply by a constant
    ///     (FNV-1a prime 0x100000001b3) XOR'd with the FNV offset basis.
    ///     One mix step is sufficient for hashtable bucket dispersion;
    ///     it's not a cryptographic hash.
    ///   - str: extract ptr + len, FNV-1a over the bytes via a 4-block
    ///     loop. Length-prefixed so "ab" and "ba" hash differently.
    fn gen_hash_intrinsic(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        if matches!(rt, Ty::Str) {
            return self.gen_hash_str(rv);
        }
        // Integer: widen to u64, then mix with FNV-1a prime.
        let widened = match rt {
            Ty::I64 | Ty::Isize | Ty::U64 | Ty::Usize => rv.to_string(),
            _ if rt.is_signed_int() => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = sext {} {rv} to i64", self.lty(rt)));
                w
            }
            _ => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = zext {} {rv} to i64", self.lty(rt)));
                w
            }
        };
        // FNV-1a mix: h = (offset XOR v) * prime.
        let xored = self.next_tmp();
        self.emit(&format!(
            "{xored} = xor i64 {widened}, -3750763034362895579"
        ));
        let mixed = self.next_tmp();
        self.emit(&format!("{mixed} = mul i64 {xored}, 1099511628211"));
        (mixed, Ty::U64)
    }

    fn gen_hash_str(&mut self, rv: &str) -> (String, Ty) {
        // Extract ptr + len from the fat-pointer aggregate.
        let p = self.next_tmp();
        let n = self.next_tmp();
        self.emit(&format!("{p} = extractvalue {{ ptr, i64 }} {rv}, 0"));
        self.emit(&format!("{n} = extractvalue {{ ptr, i64 }} {rv}, 1"));
        // Inline FNV-1a byte loop. Allocate stack slots for the running
        // hash + counter so we can re-load through the loop.
        let h_slot = self.alloca_anon(Ty::U64);
        let i_slot = self.alloca_anon(Ty::I64);
        // v0.0.7 Slice 1.2: FNV-1a hash inner loop — all primitive
        // loads/stores get their TBAA leaf, lighting up LLVM's alias
        // analysis on the inner-loop hot path.
        self.gen_store(&Ty::U64, "-3750763034362895579", &h_slot);
        self.gen_store(&Ty::I64, "0", &i_slot);
        let loop_bb = self.next_block_label();
        let body_bb = self.next_block_label();
        let done_bb = self.next_block_label();
        self.emit_terminator(&format!("br label %{loop_bb}"));
        self.body.push_str(&format!("{loop_bb}:\n"));
        self.terminated = false;
        let i_cur = self.next_tmp();
        self.gen_load(&i_cur, &Ty::I64, &i_slot);
        let cmp = self.next_tmp();
        self.emit(&format!("{cmp} = icmp slt i64 {i_cur}, {n}"));
        self.emit_terminator(&format!("br i1 {cmp}, label %{body_bb}, label %{done_bb}"));
        self.body.push_str(&format!("{body_bb}:\n"));
        self.terminated = false;
        let byte_p = self.next_tmp();
        self.emit(&format!(
            "{byte_p} = getelementptr inbounds i8, ptr {p}, i64 {i_cur}"
        ));
        let byte = self.next_tmp();
        self.gen_load(&byte, &Ty::I8, &byte_p);
        let byte_w = self.next_tmp();
        self.emit(&format!("{byte_w} = zext i8 {byte} to i64"));
        let h_cur = self.next_tmp();
        self.gen_load(&h_cur, &Ty::U64, &h_slot);
        let xored = self.next_tmp();
        self.emit(&format!("{xored} = xor i64 {h_cur}, {byte_w}"));
        let mixed = self.next_tmp();
        self.emit(&format!("{mixed} = mul i64 {xored}, 1099511628211"));
        self.gen_store(&Ty::U64, &mixed, &h_slot);
        let i_next = self.next_tmp();
        self.emit(&format!("{i_next} = add i64 {i_cur}, 1"));
        self.gen_store(&Ty::I64, &i_next, &i_slot);
        self.emit_terminator(&format!("br label %{loop_bb}"));
        self.body.push_str(&format!("{done_bb}:\n"));
        self.terminated = false;
        let h_final = self.next_tmp();
        self.gen_load(&h_final, &Ty::U64, &h_slot);
        (h_final, Ty::U64)
    }

    /// v0.0.4 Phase 3 Slice 3B.5: emit a bool from an `.eq(other)` call
    /// on a primitive / str receiver. Same lowering as `==`.
    fn gen_eq_intrinsic(&mut self, lv: &str, rv: &str, lt: &Ty) -> (String, Ty) {
        if matches!(lt, Ty::Str) {
            return self.gen_eq_str(lv, rv);
        }
        // Bool + integer: icmp eq.
        let r = self.next_tmp();
        self.emit(&format!("{r} = icmp eq {} {lv}, {rv}", self.lty(lt)));
        (r, Ty::Bool)
    }

    fn gen_eq_str(&mut self, lv: &str, rv: &str) -> (String, Ty) {
        let result_slot = self.alloca_anon(Ty::Bool);
        let lp = self.next_tmp();
        let ll = self.next_tmp();
        let rp = self.next_tmp();
        let rl = self.next_tmp();
        self.emit(&format!("{lp} = extractvalue {{ ptr, i64 }} {lv}, 0"));
        self.emit(&format!("{ll} = extractvalue {{ ptr, i64 }} {lv}, 1"));
        self.emit(&format!("{rp} = extractvalue {{ ptr, i64 }} {rv}, 0"));
        self.emit(&format!("{rl} = extractvalue {{ ptr, i64 }} {rv}, 1"));
        let len_eq = self.next_tmp();
        self.emit(&format!("{len_eq} = icmp eq i64 {ll}, {rl}"));
        let cmp_lbl = self.next_block_label();
        let unequal_lbl = self.next_block_label();
        let merge_lbl = self.next_block_label();
        self.emit_terminator(&format!(
            "br i1 {len_eq}, label %{cmp_lbl}, label %{unequal_lbl}"
        ));
        self.open_block(&cmp_lbl);
        let mc = self.next_tmp();
        self.emit(&format!(
            "{mc} = call i32 @memcmp(ptr {lp}, ptr {rp}, i64 {ll})"
        ));
        let mc_eq = self.next_tmp();
        self.emit(&format!("{mc_eq} = icmp eq i32 {mc}, 0"));
        // v0.0.7 Slice 1.2: string-eq result store — bool leaf.
        self.gen_store(&Ty::Bool, &mc_eq, &result_slot);
        self.emit_terminator(&format!("br label %{merge_lbl}"));
        self.open_block(&unequal_lbl);
        self.gen_store(&Ty::Bool, "false", &result_slot);
        self.emit_terminator(&format!("br label %{merge_lbl}"));
        self.open_block(&merge_lbl);
        let result = self.next_tmp();
        self.gen_load(&result, &Ty::Bool, &result_slot);
        (result, Ty::Bool)
    }

    /// Emit IR that produces a `string` aggregate for an arbitrary
    /// primitive (or `str`) receiver value. Strategy:
    ///   - signed int: sign-extend to i64, snprintf("%lld") into a
    ///     32-byte malloc'd buffer, take snprintf's returned length.
    ///   - unsigned int: zero-extend to i64, snprintf("%llu").
    ///   - f32: fpext to f64, snprintf("%g"). f64: direct snprintf.
    ///   - bool: branch on the i1, malloc 4/5 bytes, memcpy "true"/"false".
    ///   - str: extract ptr+len from the fat-pointer, malloc(len),
    ///     memcpy. The result owns the bytes; old bytes untouched.
    fn gen_to_string_intrinsic(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        match rt {
            Ty::Bool => self.gen_to_string_bool(rv),
            Ty::Str => self.gen_to_string_str(rv),
            Ty::F32 | Ty::F64 => self.gen_to_string_float(rv, rt),
            _ if rt.is_signed_int() => self.gen_to_string_signed(rv, rt),
            _ if rt.is_unsigned_int() => self.gen_to_string_unsigned(rv, rt),
            _ => unreachable!("sema validated to_string receiver: {:?}", rt),
        }
    }

    fn gen_to_string_signed(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        // Widen to i64 for the format spec.
        let widened = match rt {
            Ty::I64 | Ty::Isize => rv.to_string(),
            _ => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = sext {} {rv} to i64", self.lty(rt)));
                w
            }
        };
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 32)"));
        let written = self.next_tmp();
        self.emit(&format!(
            "{written} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 32, ptr @.fmt_i64, i64 {widened})"
        ));
        let len = self.next_tmp();
        self.emit(&format!("{len} = sext i32 {written} to i64"));
        let v = self.string_aggregate(&buf, &len, "32");
        (v, Ty::String)
    }

    fn gen_to_string_unsigned(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        let widened = match rt {
            Ty::U64 | Ty::Usize => rv.to_string(),
            _ => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = zext {} {rv} to i64", self.lty(rt)));
                w
            }
        };
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 32)"));
        let written = self.next_tmp();
        self.emit(&format!(
            "{written} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 32, ptr @.fmt_u64, i64 {widened})"
        ));
        let len = self.next_tmp();
        self.emit(&format!("{len} = sext i32 {written} to i64"));
        let v = self.string_aggregate(&buf, &len, "32");
        (v, Ty::String)
    }

    fn gen_to_string_float(&mut self, rv: &str, rt: &Ty) -> (String, Ty) {
        // Widen f32 → f64 for "%g".
        let widened = match rt {
            Ty::F64 => rv.to_string(),
            _ => {
                let w = self.next_tmp();
                self.emit(&format!("{w} = fpext float {rv} to double"));
                w
            }
        };
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 32)"));
        let written = self.next_tmp();
        self.emit(&format!(
            "{written} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 32, ptr @.fmt_f64, double {widened})"
        ));
        let len = self.next_tmp();
        self.emit(&format!("{len} = sext i32 {written} to i64"));
        let v = self.string_aggregate(&buf, &len, "32");
        (v, Ty::String)
    }

    fn gen_to_string_bool(&mut self, rv: &str) -> (String, Ty) {
        // Avoid the branch entirely: select between the two static
        // pointers and lengths. The buffer must still be owned (callers
        // can later `free` it), so unconditionally malloc 5 bytes
        // (covers both `"true"` and `"false"`), pick which static blob
        // and how many bytes to copy via `select`, then memcpy.
        let len = self.next_tmp();
        self.emit(&format!("{len} = select i1 {rv}, i64 4, i64 5"));
        let src = self.next_tmp();
        self.emit(&format!(
            "{src} = select i1 {rv}, ptr @.bool_true, ptr @.bool_false"
        ));
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 {len})"));
        let _cpy = self.next_tmp();
        self.emit(&format!(
            "{_cpy} = call ptr @memcpy(ptr {buf}, ptr {src}, i64 {len})"
        ));
        let v = self.string_aggregate(&buf, &len, &len);
        (v, Ty::String)
    }

    fn gen_to_string_str(&mut self, rv: &str) -> (String, Ty) {
        // Extract ptr+len from the str fat-pointer, malloc(len), memcpy.
        let src_ptr = self.next_tmp();
        self.emit(&format!("{src_ptr} = extractvalue {{ ptr, i64 }} {rv}, 0"));
        let len = self.next_tmp();
        self.emit(&format!("{len} = extractvalue {{ ptr, i64 }} {rv}, 1"));
        let buf = self.next_tmp();
        self.emit(&format!("{buf} = call ptr @malloc(i64 {len})"));
        let _cpy = self.next_tmp();
        self.emit(&format!(
            "{_cpy} = call ptr @memcpy(ptr {buf}, ptr {src_ptr}, i64 {len})"
        ));
        let v = self.string_aggregate(&buf, &len, &len);
        (v, Ty::String)
    }

    // ---------- Phase 8 slice 8.STR.3: owned `string` intrinsics ----------
    //
    // The `string` runtime value is a 24-byte struct `{ ptr, i64, i64 }` —
    // (data pointer, length in bytes, capacity in bytes). Stored by value
    // in locals; produced as an LLVM aggregate via `insertvalue` so the
    // call site doesn't need to allocate a separate slot.
    //
    // Drop is NOT integrated in this initial cut: `string` locals leak
    // their buffer at scope exit. Wiring Drop alongside the existing
    // struct-Drop machinery is a follow-up slice — see plan.md resolved-log.

    /// Build a `string` SSA value from three components.
    fn string_aggregate(&mut self, ptr: &str, len: &str, cap: &str) -> String {
        let v0 = self.next_tmp();
        self.emit(&format!(
            "{v0} = insertvalue {{ ptr, i64, i64 }} undef, ptr {ptr}, 0"
        ));
        let v1 = self.next_tmp();
        self.emit(&format!(
            "{v1} = insertvalue {{ ptr, i64, i64 }} {v0}, i64 {len}, 1"
        ));
        let v2 = self.next_tmp();
        self.emit(&format!(
            "{v2} = insertvalue {{ ptr, i64, i64 }} {v1}, i64 {cap}, 2"
        ));
        v2
    }

    /// `string::new()` — empty string. `ptr=null, len=0, cap=0`. No heap.
    /// `string::with_capacity(n)` — `malloc(n)` buffer, `len=0, cap=n`.
    fn gen_string_assoc_call(&mut self, method: &str, args: &[Expr]) -> (String, Ty) {
        match method {
            "new" => {
                let _ = args;
                let v = self.string_aggregate("null", "0", "0");
                (v, Ty::String)
            }
            "with_capacity" => {
                let (n, _) = self
                    .gen_expr(&args[0])
                    .expect("with_capacity arg has value");
                let buf = self.next_tmp();
                self.emit(&format!("{buf} = call ptr @malloc(i64 {n})"));
                let v = self.string_aggregate(&buf, "0", &n);
                (v, Ty::String)
            }
            _ => unreachable!("sema validated method `string::{method}`"),
        }
    }

    /// Methods on a `string` receiver. The receiver is materialized as a
    /// `ptr` to the local slot (24-byte aggregate); we load whichever
    /// fields the method needs via `getelementptr`/`load`.
    fn gen_string_method_call(
        &mut self,
        recv_ptr: &str,
        method: &str,
        args: &[Expr],
    ) -> (String, Ty) {
        let _ = args; // every v1 method is zero-arg
        match method {
            "len" => {
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr inbounds {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                // v0.0.7 Slice 1.2: string fat-pointer len field — usize leaf.
                self.gen_load(&lv, &Ty::Usize, &lp);
                (lv, Ty::Usize)
            }
            "is_empty" => {
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr inbounds {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                self.gen_load(&lv, &Ty::Usize, &lp);
                let cmp = self.next_tmp();
                self.emit(&format!("{cmp} = icmp eq i64 {lv}, 0"));
                (cmp, Ty::Bool)
            }
            "as_str" => {
                // Extract ptr + len; package as `str` fat-pointer `{ ptr, i64 }`.
                let pp = self.next_tmp();
                self.emit(&format!("{pp} = getelementptr inbounds {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 0"));
                let pv = self.next_tmp();
                // v0.0.7 Slice 1.2: string ptr field — ptr leaf.
                self.gen_load(&pv, &Ty::RawPtr(Box::new(Ty::Unit)), &pp);
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr inbounds {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                self.gen_load(&lv, &Ty::Usize, &lp);
                let s0 = self.next_tmp();
                self.emit(&format!(
                    "{s0} = insertvalue {{ ptr, i64 }} undef, ptr {pv}, 0"
                ));
                let s1 = self.next_tmp();
                self.emit(&format!(
                    "{s1} = insertvalue {{ ptr, i64 }} {s0}, i64 {lv}, 1"
                ));
                (s1, Ty::Str)
            }
            "clone" => {
                // Load len, malloc a fresh buffer of size len (cap = len in
                // the clone), memcpy bytes, build a new aggregate.
                let pp = self.next_tmp();
                self.emit(&format!("{pp} = getelementptr inbounds {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 0"));
                let pv = self.next_tmp();
                // v0.0.7 Slice 1.2: string.clone() — ptr + len reads.
                self.gen_load(&pv, &Ty::RawPtr(Box::new(Ty::Unit)), &pp);
                let lp = self.next_tmp();
                self.emit(&format!("{lp} = getelementptr inbounds {{ ptr, i64, i64 }}, ptr {recv_ptr}, i32 0, i32 1"));
                let lv = self.next_tmp();
                self.gen_load(&lv, &Ty::Usize, &lp);
                let buf = self.next_tmp();
                self.emit(&format!("{buf} = call ptr @malloc(i64 {lv})"));
                let _cpy = self.next_tmp();
                self.emit(&format!(
                    "{_cpy} = call ptr @memcpy(ptr {buf}, ptr {pv}, i64 {lv})"
                ));
                let v = self.string_aggregate(&buf, &lv, &lv);
                (v, Ty::String)
            }
            _ => unreachable!("sema validated `string.{method}`"),
        }
    }

    // ---- v0.0.6 Slice 1B: SIMD codegen ----

    /// `f32x4::splat(s)` / `f32x4::new(a,b,c,d)` / `f32x4::from_array(a)`.
    fn gen_simd_assoc_call(&mut self, recv: &Ty, method: &str, args: &[Expr]) -> (String, Ty) {
        let Ty::Simd { elem, lanes } = recv else {
            unreachable!("sema validated SIMD recv");
        };
        let lty = self.lty(recv);
        let elem_lty = self.lty(elem);
        match method {
            "splat" => {
                let (v, _) = self.gen_expr(&args[0]).expect("splat arg has value");
                // Build `<N x T>` by inserting v at lane 0 then shuffling
                // with `zeroinitializer` to broadcast across every lane.
                let t1 = self.next_tmp();
                self.emit(&format!(
                    "{t1} = insertelement {lty} undef, {elem_lty} {v}, i32 0"
                ));
                let t2 = self.next_tmp();
                self.emit(&format!(
                    "{t2} = shufflevector {lty} {t1}, {lty} undef, <{lanes} x i32> zeroinitializer"
                ));
                (t2, recv.clone())
            }
            "new" => {
                let mut prev = "undef".to_string();
                for (i, a) in args.iter().enumerate() {
                    let (v, _) = self.gen_expr(a).expect("new arg has value");
                    let t = self.next_tmp();
                    self.emit(&format!(
                        "{t} = insertelement {lty} {prev}, {elem_lty} {v}, i32 {i}"
                    ));
                    prev = t;
                }
                (prev, recv.clone())
            }
            "from_array" => {
                // LLVM rejects `bitcast [N x T] -> <N x T>` (vector vs.
                // aggregate). Type-pun via a stack slot: store the array,
                // load as a vector. SROA collapses the slot at -O1+.
                let (v, _) = self.gen_expr(&args[0]).expect("from_array arg");
                let arr_lty = format!("[{lanes} x {elem_lty}]");
                let slot = self.alloca_anon(recv.clone());
                let align = simd_align_for(recv);
                self.emit(&format!("store {arr_lty} {v}, ptr {slot}, align {align}"));
                let out = self.next_tmp();
                self.emit(&format!("{out} = load {lty}, ptr {slot}, align {align}"));
                (out, recv.clone())
            }
            "load" => {
                // `f32x4::load(p)` — emit a vector load through the raw
                // pointer. Use the lane scalar's alignment (caller's
                // minimum contract). Misaligned addresses are UB; sema
                // gated the call via the `unsafe { ... }` requirement.
                let (p, _) = self.gen_expr(&args[0]).expect("load arg");
                let align = simd_lane_align_for(elem);
                let out = self.next_tmp();
                self.emit(&format!("{out} = load {lty}, ptr {p}, align {align}"));
                (out, recv.clone())
            }
            // G-037: `TARGET::reinterpret(v)` → `bitcast`. Same total width
            // (sema-checked); a same-type reinterpret is a legal no-op bitcast.
            "reinterpret" => {
                let (v, src_ty) = self.gen_expr(&args[0]).expect("reinterpret arg");
                let src_lty = self.lty(&src_ty);
                if src_lty == lty {
                    return (v, recv.clone());
                }
                let out = self.next_tmp();
                self.emit(&format!("{out} = bitcast {src_lty} {v} to {lty}"));
                (out, recv.clone())
            }
            // G-038a: `FLOATxN::from_int(v)` → `sitofp`/`uitofp` (signedness of
            // the source lane type).
            "from_int" => {
                let (v, src_ty) = self.gen_expr(&args[0]).expect("from_int arg");
                let src_lty = self.lty(&src_ty);
                let op = match &src_ty {
                    Ty::Simd { elem: se, .. } if se.is_unsigned_int() => "uitofp",
                    _ => "sitofp",
                };
                let out = self.next_tmp();
                self.emit(&format!("{out} = {op} {src_lty} {v} to {lty}"));
                (out, recv.clone())
            }
            // G-038a: `INTxN::from_float(v)` → `fptosi`/`fptoui` (signedness of
            // the integer target lane type). Truncates toward zero.
            "from_float" => {
                let (v, src_ty) = self.gen_expr(&args[0]).expect("from_float arg");
                let src_lty = self.lty(&src_ty);
                let op = if elem.is_unsigned_int() { "fptoui" } else { "fptosi" };
                let out = self.next_tmp();
                self.emit(&format!("{out} = {op} {src_lty} {v} to {lty}"));
                (out, recv.clone())
            }
            _ => unreachable!("sema validated `{}::{method}`", "<simd>"),
        }
    }

    /// SIMD instance methods. Receiver value `recv` is the already-loaded
    /// `<N x T>` SSA value. Accepts both `Ty::Simd` and `Ty::Mask` (they
    /// share the LLVM lowering; sema has restricted which methods make
    /// sense per kind). `to_bits` / `to_mask` are recognised here as
    /// no-op type relabels.
    fn gen_simd_method_call(
        &mut self,
        recv: &str,
        recv_ty: &Ty,
        method: &str,
        args: &[Expr],
    ) -> (String, Ty) {
        // v0.0.9 follow-up: `to_bits` / `to_mask` are zero-cost
        // sema-only conversions — return the loaded vector unchanged
        // but relabel its `Ty`. Sema enforced direction validity.
        if method == "to_bits" {
            if let Ty::Mask { elem, lanes } = recv_ty {
                return (
                    recv.to_string(),
                    Ty::Simd { elem: elem.clone(), lanes: *lanes },
                );
            }
        }
        if method == "to_mask" {
            if let Ty::Simd { elem, lanes } = recv_ty {
                return (
                    recv.to_string(),
                    Ty::Mask { elem: elem.clone(), lanes: *lanes },
                );
            }
        }
        let (elem, lanes) = match recv_ty {
            Ty::Simd { elem, lanes } | Ty::Mask { elem, lanes } => (elem, lanes),
            _ => unreachable!("sema validated"),
        };
        let lty = self.lty(recv_ty);
        let elem_lty = self.lty(elem);
        let elem_suffix = simd_intrinsic_suffix(elem, *lanes);
        match method {
            "add" | "sub" | "mul" | "div" => {
                let (b, _) = self.gen_expr(&args[0]).expect("simd binop arg");
                let op = match (method, elem.is_float()) {
                    ("add", true) => "fadd",
                    ("sub", true) => "fsub",
                    ("mul", true) => "fmul",
                    ("div", true) => "fdiv",
                    ("add", false) => "add",
                    ("sub", false) => "sub",
                    ("mul", false) => "mul",
                    ("div", false) => {
                        if elem.is_signed_int() {
                            "sdiv"
                        } else {
                            "udiv"
                        }
                    }
                    _ => unreachable!(),
                };
                // B-10: float lanes carry the `contract` fast-math flag only
                // when fp-contraction is on; int lanes never do.
                let cf = if elem.is_float() { self.fmf() } else { "" };
                let t = self.next_tmp();
                self.emit(&format!("{t} = {op} {cf}{lty} {recv}, {b}"));
                (t, recv_ty.clone())
            }
            "fma" => {
                let (b, _) = self.gen_expr(&args[0]).expect("fma arg b");
                let (c, _) = self.gen_expr(&args[1]).expect("fma arg c");
                let t = self.next_tmp();
                self.emit(&format!(
                    "{t} = call {lty} @llvm.fma.{elem_suffix}({lty} {recv}, {lty} {b}, {lty} {c})"
                ));
                (t, recv_ty.clone())
            }
            "sqrt" => {
                let t = self.next_tmp();
                self.emit(&format!(
                    "{t} = call {lty} @llvm.sqrt.{elem_suffix}({lty} {recv})"
                ));
                (t, recv_ty.clone())
            }
            // G-042: round each lane to the nearest integer, ties to even
            // (matches AArch64 FCVTNS / `vcvtnq_s32_f32` and IEEE default).
            // Result stays float; pair with `INTxN::from_float(...)` to get
            // a rounded integer SIMD (the quantizer pattern).
            "round" => {
                let t = self.next_tmp();
                self.emit(&format!(
                    "{t} = call {lty} @llvm.roundeven.{elem_suffix}({lty} {recv})"
                ));
                (t, recv_ty.clone())
            }
            "abs" => {
                let t = self.next_tmp();
                if elem.is_float() {
                    self.emit(&format!(
                        "{t} = call {lty} @llvm.fabs.{elem_suffix}({lty} {recv})"
                    ));
                } else {
                    // `llvm.abs.<vN>` takes an extra i1 `is_int_min_poison`
                    // arg; pass `false` so INT_MIN abs is defined as
                    // INT_MIN (matches the wrapping arithmetic semantics
                    // of the `*%` family).
                    self.emit(&format!(
                        "{t} = call {lty} @llvm.abs.{elem_suffix}({lty} {recv}, i1 false)"
                    ));
                }
                (t, recv_ty.clone())
            }
            "store" => {
                // `v.store(p)` — write the vector through the raw pointer.
                // Same alignment contract as `load`. Sema enforced
                // `unsafe { ... }`.
                let (p, _) = self.gen_expr(&args[0]).expect("store arg");
                let align = simd_lane_align_for(elem);
                self.emit(&format!("store {lty} {recv}, ptr {p}, align {align}"));
                // store returns unit; pick a placeholder value the
                // caller discards (the gen_method_call wrapper only
                // returns it when not Ty::Unit, but mirroring other
                // store-shaped helpers — they return unit via 0/Unit).
                return ("0".to_string(), Ty::Unit);
            }
            "and" | "or" | "xor" => {
                let (b, _) = self.gen_expr(&args[0]).expect("simd bitwise arg");
                let t = self.next_tmp();
                self.emit(&format!("{t} = {method} {lty} {recv}, {b}"));
                (t, recv_ty.clone())
            }
            "not" => {
                // LLVM has no `not` instruction; emit `xor v, -1` (all-ones).
                let t = self.next_tmp();
                self.emit(&format!("{t} = xor {lty} {recv}, splat ({elem_lty} -1)"));
                (t, recv_ty.clone())
            }
            "shl" | "shr" => {
                let count = simd_lane_literal(&args[0]).expect("sema validated shift literal");
                let op = match (method, elem.is_signed_int()) {
                    ("shl", _) => "shl",
                    ("shr", true) => "ashr",
                    ("shr", false) => "lshr",
                    _ => unreachable!(),
                };
                let t = self.next_tmp();
                self.emit(&format!(
                    "{t} = {op} {lty} {recv}, splat ({elem_lty} {count})"
                ));
                (t, recv_ty.clone())
            }
            "min" | "max" => {
                let (b, _) = self.gen_expr(&args[0]).expect("simd min/max arg");
                let intrinsic = match (method, elem.is_float(), elem.is_signed_int()) {
                    ("min", true, _) => "minnum",
                    ("max", true, _) => "maxnum",
                    ("min", false, true) => "smin",
                    ("max", false, true) => "smax",
                    ("min", false, false) => "umin",
                    ("max", false, false) => "umax",
                    _ => unreachable!(),
                };
                let t = self.next_tmp();
                self.emit(&format!(
                    "{t} = call {lty} @llvm.{intrinsic}.{elem_suffix}({lty} {recv}, {lty} {b})"
                ));
                (t, recv_ty.clone())
            }
            "lane" => {
                // Sema validated index is a literal in range. Extract it
                // again here so we can emit `extractelement` with a
                // constant operand.
                let idx = simd_lane_literal(&args[0]).expect("sema validated lane literal");
                let t = self.next_tmp();
                self.emit(&format!("{t} = extractelement {lty} {recv}, i32 {idx}"));
                (t, (**elem).clone())
            }
            "with_lane" => {
                let idx = simd_lane_literal(&args[0]).expect("sema validated lane literal");
                let (xv, _) = self.gen_expr(&args[1]).expect("with_lane value arg");
                let t = self.next_tmp();
                self.emit(&format!(
                    "{t} = insertelement {lty} {recv}, {elem_lty} {xv}, i32 {idx}"
                ));
                (t, recv_ty.clone())
            }
            "to_array" => {
                // LLVM forbids `bitcast <N x T> -> [N x T]`. Type-pun
                // via a stack slot: store as vector, load as array. SROA
                // collapses the slot at -O1+.
                let arr_lty = format!("[{lanes} x {elem_lty}]");
                let slot = self.alloca_anon(recv_ty.clone());
                let align = simd_align_for(recv_ty);
                self.emit(&format!("store {lty} {recv}, ptr {slot}, align {align}"));
                let t = self.next_tmp();
                self.emit(&format!("{t} = load {arr_lty}, ptr {slot}, align {align}"));
                (t, Ty::Array(elem.clone(), *lanes))
            }
            // v0.0.7 Slice 2.1: lane-wise comparison ops.
            //   1. fcmp/icmp produces `<N x i1>`.
            //   2. sext to the mask shape (signed-int SIMD of the
            //      bit-width-matched lane size).
            "lt" | "le" | "gt" | "ge" | "eq" | "ne" => {
                let (b, _) = self.gen_expr(&args[0]).expect("simd cmp arg");
                let op_kind = if elem.is_float() {
                    "fcmp"
                } else if elem.is_signed_int() {
                    "icmp"
                } else {
                    "icmp"
                };
                let pred = match (method, elem.is_float(), elem.is_signed_int()) {
                    ("eq", true, _) => "oeq",
                    ("ne", true, _) => "one",
                    ("lt", true, _) => "olt",
                    ("le", true, _) => "ole",
                    ("gt", true, _) => "ogt",
                    ("ge", true, _) => "oge",
                    ("eq", false, _) => "eq",
                    ("ne", false, _) => "ne",
                    ("lt", false, true) => "slt",
                    ("le", false, true) => "sle",
                    ("gt", false, true) => "sgt",
                    ("ge", false, true) => "sge",
                    ("lt", false, false) => "ult",
                    ("le", false, false) => "ule",
                    ("gt", false, false) => "ugt",
                    ("ge", false, false) => "uge",
                    _ => unreachable!(),
                };
                let cmp_i1 = self.next_tmp();
                self.emit(&format!("{cmp_i1} = {op_kind} {pred} {lty} {recv}, {b}"));
                // Mask shape: <N x iN> where iN matches the bit width.
                let mask_elem = match **elem {
                    Ty::I8 | Ty::U8 | Ty::Bool => Ty::I8,
                    Ty::I16 | Ty::U16 => Ty::I16,
                    Ty::I32 | Ty::U32 | Ty::F32 => Ty::I32,
                    _ => Ty::I64,
                };
                let mask_ty = Ty::Simd {
                    elem: Box::new(mask_elem.clone()),
                    lanes: *lanes,
                };
                let mask_lty = self.lty(&mask_ty);
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = sext <{lanes} x i1> {cmp_i1} to {mask_lty}"
                ));
                (result, mask_ty)
            }
            // v0.0.7 Slice 2.1: select(true_v, false_v) on a mask
            // receiver. Convert mask <N x iN> back to <N x i1> via
            // `icmp ne 0`, then LLVM `select`.
            "select" => {
                let (t_v, t_ty) = self.gen_expr(&args[0]).expect("select true arg");
                let (f_v, _) = self.gen_expr(&args[1]).expect("select false arg");
                let mask_i1 = self.next_tmp();
                self.emit(&format!(
                    "{mask_i1} = icmp ne {lty} {recv}, zeroinitializer"
                ));
                let result = self.next_tmp();
                let t_lty = self.lty(&t_ty);
                self.emit(&format!(
                    "{result} = select <{lanes} x i1> {mask_i1}, {t_lty} {t_v}, {t_lty} {f_v}"
                ));
                (result, t_ty)
            }
            // v0.0.7 Slice 2.1: mask reductions. `any` = OR-reduce
            // (i1 result); `all` = AND-reduce. Convert mask to i1
            // vector first so the OR/AND reduction widths are the
            // smallest possible.
            "any" | "all" => {
                let mask_i1 = self.next_tmp();
                self.emit(&format!(
                    "{mask_i1} = icmp ne {lty} {recv}, zeroinitializer"
                ));
                let intrinsic = if method == "any" {
                    "or"
                } else {
                    "and"
                };
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = call i1 @llvm.vector.reduce.{intrinsic}.v{lanes}i1(<{lanes} x i1> {mask_i1})"
                ));
                (result, Ty::Bool)
            }
            // v0.0.7 Slice 2.1: horizontal sum / product. Float uses
            // the sequential-fp reduction with a seed (0.0 / 1.0);
            // int uses the integer reduction (no seed).
            "sum" | "product" => {
                let elem_suffix = simd_intrinsic_suffix(elem, *lanes);
                let result = self.next_tmp();
                if elem.is_float() {
                    let (intrinsic, seed) = if method == "sum" {
                        ("fadd", "0.0")
                    } else {
                        ("fmul", "1.0")
                    };
                    self.emit(&format!(
                        "{result} = call {elem_lty} @llvm.vector.reduce.{intrinsic}.{elem_suffix}({elem_lty} {seed}, {lty} {recv})"
                    ));
                } else {
                    let intrinsic = if method == "sum" { "add" } else { "mul" };
                    self.emit(&format!(
                        "{result} = call {elem_lty} @llvm.vector.reduce.{intrinsic}.{elem_suffix}({lty} {recv})"
                    ));
                }
                (result, (**elem).clone())
            }
            // v0.0.7 Slice 2.1: horizontal min/max. Same int-vs-float
            // split as the lane-wise `min`/`max`.
            "min_across" | "max_across" => {
                let elem_suffix = simd_intrinsic_suffix(elem, *lanes);
                let intrinsic = match (method, elem.is_float(), elem.is_signed_int()) {
                    ("min_across", true, _) => "fmin",
                    ("max_across", true, _) => "fmax",
                    ("min_across", false, true) => "smin",
                    ("max_across", false, true) => "smax",
                    ("min_across", false, false) => "umin",
                    ("max_across", false, false) => "umax",
                    _ => unreachable!(),
                };
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = call {elem_lty} @llvm.vector.reduce.{intrinsic}.{elem_suffix}({lty} {recv})"
                ));
                (result, (**elem).clone())
            }
            // v0.0.7 Slice 2.1: reverse all lanes — shufflevector
            // with a constant descending mask.
            "reverse" => {
                let mask_parts: Vec<String> = (0..*lanes)
                    .rev()
                    .map(|i| format!("i32 {i}"))
                    .collect();
                let mask = mask_parts.join(", ");
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = shufflevector {lty} {recv}, {lty} undef, <{lanes} x i32> <{mask}>"
                ));
                (result, recv_ty.clone())
            }
            // v0.0.7 Slice 2.1: swizzle — per-lane permutation by a
            // constant `[u32; N]` array literal.
            "swizzle" => {
                let indices = simd_swizzle_indices(&args[0], *lanes)
                    .expect("sema validated swizzle arg shape");
                let mask_parts: Vec<String> =
                    indices.iter().map(|i| format!("i32 {i}")).collect();
                let mask = mask_parts.join(", ");
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = shufflevector {lty} {recv}, {lty} undef, <{lanes} x i32> <{mask}>"
                ));
                (result, recv_ty.clone())
            }
            // v0.0.7 Slice 2.1: interleave_lo / interleave_hi —
            // shufflevector picking even pairs from the lower / upper
            // halves of (recv, arg).
            "interleave_lo" | "interleave_hi" => {
                let (b, _) = self.gen_expr(&args[0]).expect("interleave arg");
                let half = *lanes / 2;
                let mut mask_parts: Vec<String> = Vec::with_capacity(*lanes as usize);
                let base: u32 = if method == "interleave_lo" { 0 } else { half };
                for i in 0..half {
                    let idx_a = base + i;
                    let idx_b = base + i + *lanes; // second operand starts at `lanes`
                    mask_parts.push(format!("i32 {idx_a}"));
                    mask_parts.push(format!("i32 {idx_b}"));
                }
                let mask = mask_parts.join(", ");
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = shufflevector {lty} {recv}, {lty} {b}, <{lanes} x i32> <{mask}>"
                ));
                (result, recv_ty.clone())
            }
            // G-039b: low / high half via shufflevector. Result has half the
            // lanes; the mask selects the bottom or top `lanes/2` indices.
            "low" | "high" => {
                let half = *lanes / 2;
                let base: u32 = if method == "high" { half } else { 0 };
                let mask: Vec<String> =
                    (0..half).map(|i| format!("i32 {}", base + i)).collect();
                let target = Ty::Simd { elem: elem.clone(), lanes: half };
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = shufflevector {lty} {recv}, {lty} poison, <{half} x i32> <{}>",
                    mask.join(", ")
                ));
                (result, target)
            }
            // G-039b: combine two half-width vectors into a full-width one.
            // recv fills the low lanes (0..lanes), the arg the high lanes.
            "combine" => {
                let (b, _) = self.gen_expr(&args[0]).expect("combine arg");
                let total = *lanes * 2;
                let mask: Vec<String> = (0..total).map(|i| format!("i32 {i}")).collect();
                let target = Ty::Simd { elem: elem.clone(), lanes: total };
                let result = self.next_tmp();
                self.emit(&format!(
                    "{result} = shufflevector {lty} {recv}, {lty} {b}, <{total} x i32> <{}>",
                    mask.join(", ")
                ));
                (result, target)
            }
            // G-038b: widen each integer lane to the next size up. Signed
            // lanes sign-extend, unsigned zero-extend; lane count unchanged.
            "widen" => {
                let welem = match elem.as_ref() {
                    Ty::I8 => Ty::I16,
                    Ty::I16 => Ty::I32,
                    Ty::I32 => Ty::I64,
                    Ty::U8 => Ty::U16,
                    Ty::U16 => Ty::U32,
                    Ty::U32 => Ty::U64,
                    _ => unreachable!("sema validated widen lane type"),
                };
                let target = Ty::Simd { elem: Box::new(welem), lanes: *lanes };
                let tlty = self.lty(&target);
                let op = if elem.is_unsigned_int() { "zext" } else { "sext" };
                let result = self.next_tmp();
                self.emit(&format!("{result} = {op} {lty} {recv} to {tlty}"));
                (result, target)
            }
            // G-038b: narrow each integer lane to the next size down by
            // truncation; lane count unchanged.
            "narrow" => {
                let nelem = match elem.as_ref() {
                    Ty::I16 => Ty::I8,
                    Ty::I32 => Ty::I16,
                    Ty::I64 => Ty::I32,
                    Ty::U16 => Ty::U8,
                    Ty::U32 => Ty::U16,
                    Ty::U64 => Ty::U32,
                    _ => unreachable!("sema validated narrow lane type"),
                };
                let target = Ty::Simd { elem: Box::new(nelem), lanes: *lanes };
                let tlty = self.lty(&target);
                let result = self.next_tmp();
                self.emit(&format!("{result} = trunc {lty} {recv} to {tlty}"));
                (result, target)
            }
            // G-040: byte table lookup. On aarch64 this is a single
            // `vqtbl1q` (out-of-range index -> 0). Elsewhere, a per-lane
            // gather with the same out-of-range-zeroing semantics.
            "table" => {
                let (idx, _) = self.gen_expr(&args[0]).expect("table idx arg");
                let result = self.next_tmp();
                if cfg!(target_arch = "aarch64") {
                    self.emit(&format!(
                        "{result} = call <16 x i8> @llvm.aarch64.neon.tbl1.v16i8(<16 x i8> {recv}, <16 x i8> {idx})"
                    ));
                    (result, recv_ty.clone())
                } else {
                    // Portable fallback: extract each index lane, bounds-check
                    // (unsigned < 16), gather from the table, zero if out of
                    // range, insert into the result.
                    let mut cur = "undef".to_string();
                    for i in 0..16u32 {
                        let ei = self.next_tmp();
                        self.emit(&format!("{ei} = extractelement <16 x i8> {idx}, i32 {i}"));
                        let inb = self.next_tmp();
                        self.emit(&format!("{inb} = icmp ult i8 {ei}, 16"));
                        let safe = self.next_tmp();
                        self.emit(&format!("{safe} = select i1 {inb}, i8 {ei}, i8 0"));
                        let val = self.next_tmp();
                        self.emit(&format!("{val} = extractelement <16 x i8> {recv}, i8 {safe}"));
                        let val2 = self.next_tmp();
                        self.emit(&format!("{val2} = select i1 {inb}, i8 {val}, i8 0"));
                        let nxt = self.next_tmp();
                        self.emit(&format!("{nxt} = insertelement <16 x i8> {cur}, i8 {val2}, i32 {i}"));
                        cur = nxt;
                    }
                    (cur, recv_ty.clone())
                }
            }
            _ => unreachable!("sema validated SIMD method `{method}`"),
        }
    }
}

// ---- helpers ----

/// v0.0.6 Slice 1B: free-fn alias of sema's `simd_ty_from_name` so codegen
/// can recognize the same source names without cross-module imports.
/// Update both whenever a new SIMD width is added.
/// v0.0.7 Slice 2.1: parse the `[u32; N]` array literal that drives
/// `swizzle` into its constant indices. Sema validated the shape;
/// this helper just walks the AST to surface the integer values.
fn simd_swizzle_indices(e: &Expr, lanes: u32) -> Option<Vec<u32>> {
    let ExprKind::ArrayLit { elements } = &e.kind else {
        return None;
    };
    if elements.len() as u32 != lanes {
        return None;
    }
    let mut out = Vec::with_capacity(lanes as usize);
    for el in elements {
        let v = simd_lane_literal(el)?;
        out.push(v as u32);
    }
    Some(out)
}

/// v0.0.7 Slice 1.3: extract the int payload from a one-arg loop-hint
/// attribute (`#[unroll(N)]` / `#[vectorize_width(N)]`). Returns
/// `None` for any other shape — the validator in `attrs.rs` rejects
/// those at the boundary, so reaching here with a non-int means
/// sema/attrs already produced a diagnostic and codegen should skip
/// silently.
fn loop_attr_int_value(a: &Attribute) -> Option<i64> {
    if a.args.len() != 1 {
        return None;
    }
    match &a.args[0] {
        AttrArg::Int(v, _) => Some(*v),
        _ => None,
    }
}

fn codegen_simd_ty_from_name(name: &str) -> Option<Ty> {
    match name {
        "f32x4" => Some(Ty::Simd {
            elem: Box::new(Ty::F32),
            lanes: 4,
        }),
        "f64x2" => Some(Ty::Simd {
            elem: Box::new(Ty::F64),
            lanes: 2,
        }),
        "i32x4" => Some(Ty::Simd {
            elem: Box::new(Ty::I32),
            lanes: 4,
        }),
        "i64x2" => Some(Ty::Simd {
            elem: Box::new(Ty::I64),
            lanes: 2,
        }),
        "u64x2" => Some(Ty::Simd {
            elem: Box::new(Ty::U64),
            lanes: 2,
        }),
        "u32x4" => Some(Ty::Simd {
            elem: Box::new(Ty::U32),
            lanes: 4,
        }),
        "i8x16" => Some(Ty::Simd {
            elem: Box::new(Ty::I8),
            lanes: 16,
        }),
        "i16x8" => Some(Ty::Simd {
            elem: Box::new(Ty::I16),
            lanes: 8,
        }),
        "u8x16" => Some(Ty::Simd {
            elem: Box::new(Ty::U8),
            lanes: 16,
        }),
        "u16x8" => Some(Ty::Simd {
            elem: Box::new(Ty::U16),
            lanes: 8,
        }),
        // v0.0.12 SIMD Tier-1 (G-039a): 64-bit (sub-128) widths.
        "i8x8"   => Some(Ty::Simd { elem: Box::new(Ty::I8),  lanes: 8 }),
        "u8x8"   => Some(Ty::Simd { elem: Box::new(Ty::U8),  lanes: 8 }),
        "i16x4"  => Some(Ty::Simd { elem: Box::new(Ty::I16), lanes: 4 }),
        "u16x4"  => Some(Ty::Simd { elem: Box::new(Ty::U16), lanes: 4 }),
        "i32x2"  => Some(Ty::Simd { elem: Box::new(Ty::I32), lanes: 2 }),
        "u32x2"  => Some(Ty::Simd { elem: Box::new(Ty::U32), lanes: 2 }),
        "f32x2"  => Some(Ty::Simd { elem: Box::new(Ty::F32), lanes: 2 }),
        // v0.0.7 Slice 2.2: 256-bit widths.
        "f32x8"  => Some(Ty::Simd { elem: Box::new(Ty::F32), lanes: 8  }),
        "f64x4"  => Some(Ty::Simd { elem: Box::new(Ty::F64), lanes: 4  }),
        "i8x32"  => Some(Ty::Simd { elem: Box::new(Ty::I8),  lanes: 32 }),
        "u8x32"  => Some(Ty::Simd { elem: Box::new(Ty::U8),  lanes: 32 }),
        "i16x16" => Some(Ty::Simd { elem: Box::new(Ty::I16), lanes: 16 }),
        "u16x16" => Some(Ty::Simd { elem: Box::new(Ty::U16), lanes: 16 }),
        "i32x8"  => Some(Ty::Simd { elem: Box::new(Ty::I32), lanes: 8  }),
        "u32x8"  => Some(Ty::Simd { elem: Box::new(Ty::U32), lanes: 8  }),
        "i64x4"  => Some(Ty::Simd { elem: Box::new(Ty::I64), lanes: 4  }),
        "u64x4"  => Some(Ty::Simd { elem: Box::new(Ty::U64), lanes: 4  }),
        // v0.0.7 Slice 2.1: mask types alias the matching signed-int SIMD.
        "mask8x16"  => Some(Ty::Simd { elem: Box::new(Ty::I8),  lanes: 16 }),
        "mask16x8"  => Some(Ty::Simd { elem: Box::new(Ty::I16), lanes: 8  }),
        "mask32x4"  => Some(Ty::Simd { elem: Box::new(Ty::I32), lanes: 4  }),
        "mask64x2"  => Some(Ty::Simd { elem: Box::new(Ty::I64), lanes: 2  }),
        "mask8x32"  => Some(Ty::Simd { elem: Box::new(Ty::I8),  lanes: 32 }),
        "mask16x16" => Some(Ty::Simd { elem: Box::new(Ty::I16), lanes: 16 }),
        "mask32x8"  => Some(Ty::Simd { elem: Box::new(Ty::I32), lanes: 8  }),
        "mask64x4"  => Some(Ty::Simd { elem: Box::new(Ty::I64), lanes: 4  }),
        _ => None,
    }
}

/// Minimum alignment for the *lane scalar* of a SIMD vector. Used by
/// `load`/`store` on raw pointers — the caller's contract is that the
/// pointer is at least lane-aligned; LLVM can then emit aligned moves
/// where supported and unaligned where required.
fn simd_lane_align_for(elem: &Ty) -> u64 {
    match elem {
        Ty::I8 | Ty::U8 | Ty::Bool => 1,
        Ty::I16 | Ty::U16 => 2,
        Ty::I32 | Ty::U32 | Ty::F32 => 4,
        Ty::I64 | Ty::U64 | Ty::F64 | Ty::Isize | Ty::Usize => 8,
        _ => 1,
    }
}

/// Natural alignment for a SIMD vector in bytes (lane size × lane count,
/// rounded to the natural pow-of-2). Mirrors `static_layout`.
fn simd_align_for(ty: &Ty) -> u64 {
    let Ty::Simd { elem, lanes } = ty else {
        return 1;
    };
    let elem_sz = match **elem {
        Ty::I8 | Ty::U8 | Ty::Bool => 1,
        Ty::I16 | Ty::U16 => 2,
        Ty::I32 | Ty::U32 | Ty::F32 => 4,
        Ty::I64 | Ty::U64 | Ty::F64 | Ty::Isize | Ty::Usize => 8,
        _ => 1,
    };
    let sz = elem_sz * (*lanes as u64);
    if sz.is_power_of_two() {
        sz
    } else {
        elem_sz
    }
}

/// Per-element LLVM intrinsic suffix — `v4f32`, `v2f64`, `v4i32`, etc.
fn simd_intrinsic_suffix(elem: &Ty, lanes: u32) -> String {
    let elem_part = match elem {
        Ty::F32 => "f32",
        Ty::F64 => "f64",
        Ty::I8 | Ty::U8 => "i8",
        Ty::I16 | Ty::U16 => "i16",
        Ty::I32 | Ty::U32 => "i32",
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize => "i64",
        _ => "unknown",
    };
    format!("v{lanes}{elem_part}")
}

/// Extract a SIMD lane index literal. Sema validated this; the helper
/// just re-reads the literal so codegen can emit the constant operand.
fn simd_lane_literal(e: &Expr) -> Option<u64> {
    match &e.kind {
        ExprKind::IntLit(v, _) => Some(*v),
        ExprKind::Cast { expr, .. } => {
            if let ExprKind::IntLit(v, _) = &expr.kind {
                Some(*v)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn cmp_op_for_type(op: BinOp, ty: &Ty) -> &'static str {
    if ty.is_float() {
        // Ordered comparisons (NaN comparisons are false). Bool eq/ne handled via i1 icmp.
        return match op {
            BinOp::Eq => "oeq",
            BinOp::Ne => "one",
            BinOp::Lt => "olt",
            BinOp::Le => "ole",
            BinOp::Gt => "ogt",
            BinOp::Ge => "oge",
            _ => unreachable!(),
        };
    }
    match op {
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => {
            if ty.is_unsigned_int() {
                "ult"
            } else {
                "slt"
            }
        }
        BinOp::Le => {
            if ty.is_unsigned_int() {
                "ule"
            } else {
                "sle"
            }
        }
        BinOp::Gt => {
            if ty.is_unsigned_int() {
                "ugt"
            } else {
                "sgt"
            }
        }
        BinOp::Ge => {
            if ty.is_unsigned_int() {
                "uge"
            } else {
                "sge"
            }
        }
        _ => unreachable!(),
    }
}

fn sanitize(s: &str) -> String {
    // LLVM names accept a wide set; identifiers from C+ (ASCII alnum + _) are fine.
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema;
    use std::path::PathBuf;

    fn gen_src(src: &str) -> String {
        gen_src_with(src, BuildMode::Debug)
    }

    fn gen_src_with(src: &str, mode: BuildMode) -> String {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(diags.is_empty(), "sema errors: {diags:#?}");
        generate(&prog, mode)
    }

    /// The `llvm.coro.end` declare + calls follow the probed return-type form
    /// (`i1` for older LLVM / Apple clang 21, `void` for LLVM ~22+). A
    /// regression guard for the Windows-port fix: emitting one form
    /// unconditionally broke the other toolchain ("Intrinsic has incorrect
    /// return type!"). The driver probes the real clang; here we drive the flag
    /// directly. Both forms must be internally consistent (declare matches call).
    #[test]
    fn coro_end_emit_follows_probed_return_type() {
        // i1 form (older LLVM / Apple clang 21): declare + call both i1, and the
        // call binds a (discarded) result SSA value.
        set_coro_end_returns_void(false);
        assert_eq!(
            coro_end_decl_ir(),
            "declare i1 @llvm.coro.end(ptr, i1, token)\n"
        );
        assert!(
            coro_end_call_ir()
                .contains("%.coro.end_token = call i1 @llvm.coro.end(ptr %.coro.hdl, i1 false, token none)"),
            "i1 mode call: {}",
            coro_end_call_ir()
        );

        // void form (LLVM ~22+): declare + call both void, no result value.
        set_coro_end_returns_void(true);
        assert_eq!(
            coro_end_decl_ir(),
            "declare void @llvm.coro.end(ptr, i1, token)\n"
        );
        assert_eq!(
            coro_end_call_ir(),
            "  call void @llvm.coro.end(ptr %.coro.hdl, i1 false, token none)\n"
        );

        // Restore the default for any other test that generates async IR.
        set_coro_end_returns_void(true);
    }

    /// Regression: a negative float literal (`-5.0`) was const-folded to the
    /// textual `-5` (Rust `Display` drops the `.0` for whole f64s), which LLVM
    /// rejects for `double` ("integer constant must have integer type"). It must
    /// emit the hex-float form, like the positive literal path. -5.0's bit
    /// pattern is 0xC014000000000000.
    #[test]
    fn negative_float_literal_emits_hex_not_int_constant() {
        let ir = gen_src(
            "fn main() -> i32 { let n: f64 = -5.0; if n < 0.0 { return 1; } return 0; }",
        );
        assert!(
            !ir.contains("double -5"),
            "negative float literal emitted as an integer-form constant:\n{ir}"
        );
        assert!(
            ir.contains("0xC014000000000000"),
            "expected the hex-float form of -5.0:\n{ir}"
        );
    }

    /// `let _ = expr;` is a discard binding: it parses, type-checks, and lowers
    /// (evaluating — and dropping — its initializer). Multiple `let _` in one
    /// scope must not collide (each gets a unique synthesized name).
    #[test]
    fn let_underscore_is_a_discard_binding() {
        let ir = gen_src(
            "fn f() -> i32 { return 7; }\n\
             fn main() -> i32 { let _ = f(); let _ = f(); return 0; }",
        );
        assert!(ir.contains("define i32 @main"), "expected main in IR:\n{ir}");
    }

    /// v0.0.3 Phase 5 Slice 5B: gen_src + monomorphize. Required for
    /// codegen IR tests of intrinsics whose return type is a generic
    /// struct (e.g. `__cplus_thread_spawn` returning `JoinHandle[O]`).
    /// Mirrors `cpc/src/main.rs::run_monomorphize`.
    fn gen_src_mono(src: &str) -> String {
        use crate::ast::ItemKind;
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let file_path = PathBuf::from("test.cplus");
        let mut files: std::collections::BTreeMap<String, (PathBuf, String)> =
            std::collections::BTreeMap::new();
        files.insert(
            "test.cplus".to_string(),
            (file_path.clone(), src.to_string()),
        );
        let (diags, mono) = sema::check_multi_with_mono(&prog, file_path.clone(), src, files);
        assert!(diags.is_empty(), "sema errors: {diags:#?}");
        // Build name lookup over (mono-extended) struct/enum tables.
        let mut struct_names: Vec<String> = Vec::new();
        let mut enum_names: Vec<String> = Vec::new();
        for item in &prog.items {
            match &item.kind {
                ItemKind::Struct(s) if s.generic_params.is_empty() => {
                    struct_names.push(s.name.name.clone())
                }
                ItemKind::Enum(e) if e.generic_params.is_empty() => {
                    enum_names.push(e.name.name.clone())
                }
                _ => {}
            }
        }
        for info in mono.struct_instantiations.values() {
            let slot = info.id as usize;
            if struct_names.len() <= slot {
                struct_names.resize(slot + 1, String::from("?"));
            }
            struct_names[slot] = info.mangled_name.clone();
        }
        for info in mono.enum_instantiations.values() {
            let slot = info.id as usize;
            if enum_names.len() <= slot {
                enum_names.resize(slot + 1, String::from("?"));
            }
            enum_names[slot] = info.mangled_name.clone();
        }
        let name_of = move |ty: &sema::Ty| -> String {
            match ty {
                sema::Ty::Struct(id) => struct_names
                    .get(id.0 as usize)
                    .cloned()
                    .unwrap_or_else(|| "?".into()),
                sema::Ty::Enum(id) => enum_names
                    .get(id.0 as usize)
                    .cloned()
                    .unwrap_or_else(|| "?".into()),
                other => other.name().to_string(),
            }
        };
        let post = crate::monomorphize::monomorphize(prog, &mono, &name_of);
        // v0.0.11 Phase 0: route through `generate_with_mono` so the
        // MonoInfo (compile_time_blobs / env_vars / statics / selectors /
        // shader_blobs) reaches codegen. Plain `generate()` would default
        // every table to empty and tests of `#selector` / `#msg_send` /
        // `#compile_shader` would panic in their pre-pass lookups.
        generate_with_mono(&post, BuildMode::Debug, true, None, &[], false, &mono)
    }

    #[test]
    fn preamble_includes_intrinsics() {
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(ir.contains("declare i32 @printf(ptr noundef, ...)"));
        assert!(ir.contains("@.fmt_int_nl"));
        assert!(ir.contains("declare void @llvm.trap()"));
        assert!(ir.contains("declare {i32, i1} @llvm.sadd.with.overflow.i32"));
        assert!(ir.contains("declare {i32, i1} @llvm.ssub.with.overflow.i32"));
        assert!(ir.contains("declare {i32, i1} @llvm.smul.with.overflow.i32"));
    }

    #[test]
    fn main_returns_int_literal() {
        let ir = gen_src("fn main() -> i32 { return 42; }");
        assert!(ir.contains("i32 @main()"));
        assert!(ir.contains("ret i32 42"));
    }

    #[test]
    fn if_arm_payload_enum_ctor_forwards_value_to_match_slot() {
        // v0.0.14 regression: a match arm whose body is an `if` building a
        // payload-carrying enum ctor (`Out::Hi(7)`, lowered as Call{Path})
        // must forward the if's value into the match-result slot. Pre-fix,
        // `expr_value_ty_with_bindings` didn't recognize the Call{Path} enum
        // ctor, so `gen_if` allocated no result slot and each if-branch built
        // its value then dropped it (the if-merge block was a bare `br`) —
        // the match then read an uninitialized slot. We assert the fix at the
        // IR level: both if-branches and the direct arm must emit an aggregate
        // `store %enum` of their value (so the merge sees a written slot on
        // every path). Pre-fix there were only 2 such stores (direct arm +
        // the let reload); the fix raises it to >= 4.
        let ir = gen_src(
            "enum Tag { A, B }\n\
             enum Out { Hi(i32), Lo(i32) }\n\
             fn pick(t: Tag, flag: bool) -> Out {\n\
                 let r: Out = match t {\n\
                     Tag::A => { if flag { Out::Hi(7) } else { Out::Lo(8) } }\n\
                     Tag::B => Out::Lo(30),\n\
                 };\n\
                 return r;\n\
             }\n\
             fn main() -> i32 { return 0; }\n",
        );
        let pick = ir
            .split("@pick")
            .nth(1)
            .expect("pick defined")
            .split("\n}")
            .next()
            .expect("pick body");
        let agg_stores = pick.matches("store %enum").count();
        assert!(
            agg_stores >= 4,
            "if-arm enum-ctor value dropped: expected >=4 aggregate enum stores \
             in @pick (both if-branches + merge + direct arm), got {agg_stores}.\n{pick}"
        );
    }

    #[test]
    fn debug_arithmetic_uses_overflow_intrinsics() {
        let ir = gen_src_with(
            "fn main() -> i32 { return 1 + 2 * 3 - 4; }",
            BuildMode::Debug,
        );
        assert!(ir.contains("call {i32, i1} @llvm.sadd.with.overflow.i32"));
        assert!(ir.contains("call {i32, i1} @llvm.ssub.with.overflow.i32"));
        assert!(ir.contains("call {i32, i1} @llvm.smul.with.overflow.i32"));
        assert!(ir.contains("call void @llvm.trap()"));
        assert!(ir.contains("unreachable"));
    }

    // ---- v0.0.7 Slice 1.3: loop-hint attributes ----

    #[test]
    fn unroll_attribute_emits_llvm_loop_metadata() {
        let ir = gen_src(
            "fn main() -> i32 { \
                let mut i: i32 = 0; \
                #[unroll(4)] while i < 10 { i = i + 1; } \
                return 0; \
            }",
        );
        assert!(
            ir.contains("!\"llvm.loop.unroll.count\", i32 4"),
            "expected unroll.count metadata node; IR:\n{ir}"
        );
        let backedge_md = ir
            .lines()
            .any(|l| l.contains("br label %") && l.contains(", !llvm.loop !"));
        assert!(
            backedge_md,
            "expected `!llvm.loop` on a back-edge branch; IR:\n{ir}"
        );
    }

    #[test]
    fn vectorize_width_attribute_on_for_loop() {
        let ir = gen_src(
            "fn main() -> i32 { \
                let mut sum: i32 = 0; \
                #[vectorize_width(8)] for i in 0..16 { sum = sum + i; } \
                return 0; \
            }",
        );
        assert!(
            ir.contains("!\"llvm.loop.vectorize.width\", i32 8"),
            "expected vectorize.width metadata; IR:\n{ir}"
        );
    }

    #[test]
    fn loop_without_attribute_omits_llvm_loop_metadata() {
        // Regression guard: a plain `while` keeps the existing
        // back-edge shape (no trailing `, !llvm.loop !N`).
        let ir = gen_src(
            "fn main() -> i32 { \
                let mut i: i32 = 0; \
                while i < 10 { i = i + 1; } \
                return 0; \
            }",
        );
        assert!(
            !ir.contains("!llvm.loop"),
            "expected no `!llvm.loop` references for an unannotated loop; IR:\n{ir}"
        );
    }

    // ---- v0.0.7 Slice 1.2: TBAA metadata ----

    #[test]
    fn tbaa_tags_appear_on_primitive_ident_load() {
        // A primitive-typed binding read goes through `gen_load`,
        // which appends `, !tbaa !N` for the i32 leaf. The TBAA root
        // + leaf definitions live at module-end (lazy-allocated, ID
        // band shared with `register_range`).
        let ir = gen_src("fn main() -> i32 { let x: i32 = 7; return x; }");
        // Tree definitions at module end.
        assert!(
            ir.contains("!{!\"C+ TBAA Root\"}"),
            "missing TBAA root; IR:\n{ir}"
        );
        assert!(
            ir.lines().any(|l| l.contains("!{!\"i32\",") && l.contains("i64 0}")),
            "missing i32 TBAA leaf; IR:\n{ir}"
        );
        // The load picked up the tag.
        let tagged_loads = ir
            .lines()
            .filter(|l| l.contains(" = load i32,") && l.contains("!tbaa "))
            .count();
        assert!(
            tagged_loads >= 1,
            "expected at least one TBAA-tagged i32 load; IR:\n{ir}"
        );
    }

    #[test]
    fn tbaa_distinct_leaves_for_disjoint_primitives() {
        // The whole point of TBAA — `i32` and `f64` get distinct leaf
        // IDs, so LLVM's alias analysis can prove that a `*i32` load
        // can't alias a `*f64` store. Verified at the metadata-tree
        // level rather than via downstream pass output (which depends
        // on optimizer version).
        let ir = gen_src(
            "fn main() -> i32 { \
                let a: i32 = 1; \
                let b: f64 = 2.0; \
                if a > 0 { return 0; } \
                if b > 0.0 { return 1; } \
                return 2; \
            }",
        );
        let i32_leaf_line = ir
            .lines()
            .find(|l| l.contains("!{!\"i32\",") && l.contains("i64 0}"))
            .expect("i32 leaf");
        let f64_leaf_line = ir
            .lines()
            .find(|l| l.contains("!{!\"f64\",") && l.contains("i64 0}"))
            .expect("f64 leaf");
        // The leading `!N` IDs differ.
        let i32_id: &str = i32_leaf_line.split(" = ").next().unwrap();
        let f64_id: &str = f64_leaf_line.split(" = ").next().unwrap();
        assert_ne!(i32_id, f64_id, "i32 and f64 must use distinct TBAA leaves");
    }

    #[test]
    fn tbaa_tags_aggregate_struct_loads_with_distinct_leaf() {
        // v0.0.8 bench-gap finding 4: aggregate loads/stores now get
        // a per-type TBAA leaf so a `*Entry` access doesn't alias a
        // `*Sphere` access under LLVM's analysis. The whole-struct
        // load on `let p: Pt = Pt { x: 1, y: 2 };` carries the
        // `struct.Pt` leaf, distinct from the `i32` leaf used by the
        // field load.
        let ir = gen_src(
            "struct Pt { x: i32, y: i32 }\n\
             fn main() -> i32 { let p: Pt = Pt { x: 1, y: 2 }; return p.x; }",
        );

        // Every whole-struct load carries `!tbaa !N`.
        let aggregate_load_lines: Vec<&str> = ir
            .lines()
            .filter(|l| l.contains(" = load %Pt,"))
            .collect();
        assert!(
            !aggregate_load_lines.is_empty(),
            "expected at least one whole-struct load; IR:\n{ir}"
        );
        for l in &aggregate_load_lines {
            assert!(
                l.contains("!tbaa "),
                "whole-struct loads must carry TBAA tag: {l}"
            );
        }

        // The struct leaf is distinct from the i32 leaf.
        let struct_leaf_line = ir
            .lines()
            .find(|l| l.contains("!\"struct.Pt\""))
            .unwrap_or_else(|| panic!("expected struct.Pt TBAA leaf; IR:\n{ir}"));
        let i32_leaf_line = ir
            .lines()
            .find(|l| l.contains("!\"i32\""))
            .unwrap_or_else(|| panic!("expected i32 TBAA leaf; IR:\n{ir}"));
        let struct_id = struct_leaf_line.split(" = ").next().unwrap();
        let i32_id = i32_leaf_line.split(" = ").next().unwrap();
        assert_ne!(
            struct_id, i32_id,
            "struct.Pt and i32 must use distinct TBAA leaves"
        );

        // And the i32 field load still carries its primitive tag.
        let field_load_tagged = ir
            .lines()
            .any(|l| l.contains(" = load i32,") && l.contains("!tbaa "));
        assert!(
            field_load_tagged,
            "expected at least one TBAA-tagged i32 field load; IR:\n{ir}"
        );
    }

    // ---- v0.0.8 bench-gap fixes ----

    #[test]
    fn field_read_caching_dedups_v_x_in_one_expression() {
        // v0.0.8 bench-gap finding 1: `v.x * v.x` in one expression
        // should emit ONE GEP + ONE load for v.x, then `fmul X, X`.
        // Before the cache, two GEP+loads were emitted and the SLP-
        // vectorizer's adjacent-load pattern recognition broke.
        let ir = gen_src(
            "struct Vec3 { x: f32, y: f32, z: f32 }\n\
             fn dot(v: Vec3) -> f32 {\n\
                 return v.x * v.x + v.y * v.y + v.z * v.z;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        // Count GEPs into Vec3 inside @dot.
        let dot_body: Vec<&str> = ir
            .lines()
            .skip_while(|l| !l.contains("define internal fastcc float @dot"))
            .take_while(|l| !l.starts_with("}"))
            .collect();
        let gep_count = dot_body
            .iter()
            .filter(|l| l.contains("getelementptr inbounds %Vec3"))
            .count();
        // Expected: one GEP per field (x, y, z) = 3. Without the
        // cache it was 6 (one per source-level field read, with
        // `v.x` appearing twice etc.).
        assert_eq!(
            gep_count, 3,
            "expected 3 field GEPs (one per field) in dot; got {gep_count}. IR:\n{}",
            dot_body.join("\n")
        );
    }

    #[test]
    fn field_read_cache_invalidates_across_basic_blocks() {
        // v0.0.8 bench-gap finding 1 follow-up: the field-read memo
        // must be cleared at every basic-block boundary or the cached
        // SSA name from a previous block fails LLVM's dominance check
        // ("Instruction does not dominate all uses!"). Concretely:
        // `let s = v.x * v.x; if cond { ... }; return s * v.x;` —
        // the `v.x` after the if-return must not reuse the SSA name
        // from before the if, because the if's terminator opened a
        // fresh block.
        let ir = gen_src(
            "struct V { x: f32, y: f32, z: f32 }\n\
             fn lookup(v: V, cond: bool) -> f32 {\n\
                 let x_squared: f32 = v.x * v.x;\n\
                 if cond { return v.x; }\n\
                 return x_squared * v.x;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        // The IR must parse — gen_src panics if cpc rejected the
        // module or emitted invalid IR.
        assert!(ir.contains("@lookup"), "lookup fn must be emitted: {ir}");
        // And there must be at least 3 GEPs into V — one for the
        // initial `v.x * v.x` (cached → one GEP), one in the
        // then-branch's `v.x` (fresh block), one in the merge
        // block's `v.x` (fresh block). Caching only fires within a
        // block.
        let geps = ir.lines().filter(|l| l.contains("getelementptr inbounds %V")).count();
        assert!(
            geps >= 3,
            "expected at least 3 GEPs across the if's three basic blocks; got {geps}. IR:\n{ir}"
        );
    }

    #[test]
    fn struct_literal_assignment_skips_intermediate_alloca() {
        // v0.0.8 bench-gap finding 2: `place = StructLit{...}` should
        // store each field directly to the destination's GEP slot.
        // Before the fast path, codegen built the literal in a fresh
        // alloca, loaded the whole struct, and stored the aggregate
        // value through SSA — three extra IR ops per assignment.
        let ir = gen_src(
            "struct Entry { key: i32, val: i32 }\n\
             extern fn malloc(n: usize) -> *u8;\n\
             fn main() -> i32 {\n\
                 let buf: *u8 = unsafe { malloc(8 as usize) };\n\
                 let t: *Entry = unsafe { buf as *Entry };\n\
                 unsafe { t[0 as usize] = Entry { key: 7, val: 42 }; }\n\
                 return 0;\n\
             }",
        );
        // No aggregate `load %Entry` should appear — the old path
        // emitted one to lift the literal out of its temp alloca.
        let aggregate_loads = ir
            .lines()
            .filter(|l| l.contains(" = load %Entry,"))
            .count();
        // And no aggregate `store %Entry` either — the old path
        // stored the lifted value to the destination.
        let aggregate_stores = ir
            .lines()
            .filter(|l| l.contains("store %Entry "))
            .count();
        assert_eq!(
            aggregate_loads, 0,
            "expected zero aggregate `load %Entry` in main; got {aggregate_loads}. IR:\n{ir}"
        );
        assert_eq!(
            aggregate_stores, 0,
            "expected zero aggregate `store %Entry` in main; got {aggregate_stores}. IR:\n{ir}"
        );
        // And the field stores are tagged with the i32 leaf as before.
        let i32_field_stores_tagged = ir
            .lines()
            .filter(|l| l.contains("store i32 ") && l.contains("!tbaa "))
            .count();
        assert!(
            i32_field_stores_tagged >= 2,
            "expected at least 2 TBAA-tagged i32 field stores; got {i32_field_stores_tagged}. IR:\n{ir}"
        );
    }

    // ---- v0.0.7 Slice 1.1: lifetime intrinsics ----

    #[test]
    fn release_emits_lifetime_bracketed_locals() {
        // A `let x: i32` inside a nested block at release mode must
        // get a matching `lifetime.start` / `lifetime.end` pair, and
        // the `end` must fire before the block's closing `br` so SROA
        // can reuse the slot across non-overlapping scopes.
        let ir = gen_src_with(
            "fn main() -> i32 { { let x: i32 = 7; let y: i32 = 8; } return 0; }",
            BuildMode::Release,
        );
        assert!(
            ir.contains("declare void @llvm.lifetime.start.p0(i64, ptr)"),
            "lifetime.start declaration missing from preamble:\n{ir}"
        );
        assert!(
            ir.contains("declare void @llvm.lifetime.end.p0(i64, ptr)"),
            "lifetime.end declaration missing from preamble:\n{ir}"
        );
        // Both bindings bracketed by lifetime calls.
        let start_calls = ir
            .lines()
            .filter(|l| l.contains("call void @llvm.lifetime.start.p0(i64 4,"))
            .count();
        let end_calls = ir
            .lines()
            .filter(|l| l.contains("call void @llvm.lifetime.end.p0(i64 4,"))
            .count();
        assert_eq!(start_calls, 2, "expected 2 lifetime.start calls; IR:\n{ir}");
        assert_eq!(end_calls, 2, "expected 2 lifetime.end calls; IR:\n{ir}");
    }

    #[test]
    fn debug_omits_lifetime_intrinsics_in_bodies() {
        // Debug mode skips the intrinsics — lldb's frame walker stays
        // simple. Declarations stay in the preamble (cheap, harmless).
        let ir = gen_src_with(
            "fn main() -> i32 { { let x: i32 = 7; } return 0; }",
            BuildMode::Debug,
        );
        assert!(
            ir.contains("declare void @llvm.lifetime.start.p0(i64, ptr)"),
            "lifetime.start declaration must still be in preamble at debug; IR:\n{ir}"
        );
        assert!(
            !ir.contains("call void @llvm.lifetime.start.p0"),
            "no lifetime.start *calls* expected at debug; IR:\n{ir}"
        );
        assert!(
            !ir.contains("call void @llvm.lifetime.end.p0"),
            "no lifetime.end *calls* expected at debug; IR:\n{ir}"
        );
    }

    #[test]
    fn release_lifetime_end_in_reverse_order() {
        // Spec: pop_scope walks the per-frame alloca list in reverse,
        // so `let x; let y;` produces `end(y) ... end(x)`. Critical
        // for stack-slot reuse: a forward-order end would let SROA
        // reuse x's slot for y while y is still live.
        let ir = gen_src_with(
            "fn main() -> i32 { { let x: i32 = 1; let y: i32 = 2; } return 0; }",
            BuildMode::Release,
        );
        // Find positions of end calls for x.addr and y.addr.
        let y_end_pos = ir
            .find("call void @llvm.lifetime.end.p0(i64 4, ptr %y.addr")
            .expect("expected lifetime.end for y");
        let x_end_pos = ir
            .find("call void @llvm.lifetime.end.p0(i64 4, ptr %x.addr")
            .expect("expected lifetime.end for x");
        assert!(
            y_end_pos < x_end_pos,
            "lifetime.end for y must come before x (reverse registration order); IR:\n{ir}"
        );
    }

    #[test]
    fn release_skips_lifetime_for_function_wide_allocas() {
        // The function body's bottom scope holds param-copy slots
        // (and any allocas created before the first `push_scope`).
        // Those live for the whole function — emitting lifetime
        // intrinsics for them would be a no-op at best and a footgun
        // at worst. Verify: a function with only a param (no inner
        // blocks) emits no lifetime calls.
        let ir = gen_src_with(
            "fn id(x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return id(7); }",
            BuildMode::Release,
        );
        assert!(
            !ir.contains("call void @llvm.lifetime.start.p0"),
            "param-only function must not bracket the param-copy slot; IR:\n{ir}"
        );
    }

    #[test]
    fn release_arithmetic_uses_plain_ops() {
        let ir = gen_src_with(
            "fn main() -> i32 { return 1 + 2 * 3 - 4; }",
            BuildMode::Release,
        );
        // Plain ops, no intrinsic calls in arithmetic body.
        assert!(ir.contains(" = add i32 "));
        assert!(ir.contains(" = sub i32 "));
        assert!(ir.contains(" = mul i32 "));
        // No sadd intrinsic *call* (declarations remain in preamble).
        assert!(!ir.contains("call {i32, i1} @llvm.sadd.with.overflow"));
        assert!(!ir.contains("call {i32, i1} @llvm.ssub.with.overflow"));
        assert!(!ir.contains("call {i32, i1} @llvm.smul.with.overflow"));
    }

    #[test]
    fn division_always_traps_on_zero() {
        // Both modes emit the zero-check.
        for mode in [BuildMode::Debug, BuildMode::Release] {
            let ir = gen_src_with("fn main() -> i32 { return 10 / 2; }", mode);
            assert!(ir.contains("icmp eq i32"), "mode={mode:?}: {ir}");
            assert!(ir.contains(" = sdiv i32 "), "mode={mode:?}");
            assert!(ir.contains("call void @llvm.trap()"), "mode={mode:?}");
        }
    }

    #[test]
    fn modulo_always_traps_on_zero() {
        let ir = gen_src("fn main() -> i32 { return 10 % 3; }");
        assert!(ir.contains("icmp eq i32"));
        assert!(ir.contains(" = srem i32 "));
    }

    #[test]
    fn let_emits_alloca_and_store() {
        let ir = gen_src("fn main() -> i32 { let x: i32 = 7; return x; }");
        assert!(ir.contains("alloca i32"));
        assert!(ir.contains("store i32 7, ptr"));
        assert!(ir.contains("load i32, ptr"));
    }

    #[test]
    fn comparison_emits_icmp() {
        let ir = gen_src("fn main() -> i32 { return if 1 < 2 { 1 } else { 0 }; }");
        assert!(ir.contains("icmp slt i32"));
        assert!(ir.contains("br i1"));
    }

    #[test]
    fn while_loop_has_header_and_exit() {
        let ir = gen_src(
            "fn main() -> i32 { let mut i: i32 = 0; while i < 5 { i = i + 1; } return i; }",
        );
        assert!(ir.contains("br label %bb"));
        assert!(ir.contains("icmp slt"));
    }

    #[test]
    fn for_range_inclusive_uses_sle() {
        let ir = gen_src(
            "fn main() -> i32 { let mut s: i32 = 0; for i in 0..=3 { s = s + i; } return s; }",
        );
        assert!(ir.contains("icmp sle i32"));
    }

    #[test]
    fn for_range_exclusive_uses_slt() {
        let ir = gen_src(
            "fn main() -> i32 { let mut s: i32 = 0; for i in 0..3 { s = s + i; } return s; }",
        );
        assert!(ir.contains("icmp slt i32"));
    }

    #[test]
    fn function_call_emits_call() {
        let ir = gen_src(
            "fn double(x: i32) -> i32 { return x + x; }\nfn main() -> i32 { return double(21); }",
        );
        // v0.0.8 fix C: non-pub `double` gets internal linkage + fastcc.
        // Both the define and the call site emit the cc.
        assert!(ir.contains("i32 @double"));
        assert!(ir.contains("call fastcc i32 @double"));
    }

    #[test]
    fn println_lowers_to_printf() {
        let ir = gen_src("fn main() -> i32 { #println(42); return 0; }");
        assert!(ir.contains("call i32 (ptr, ...) @printf(ptr noundef @.fmt_int_nl, i32 42"));
    }

    // ---- Phase 8 slice 8.STR.1–.3: strings ----

    #[test]
    fn str_literal_emits_global_constant() {
        // Each unique literal gets a `@.str.N = private unnamed_addr constant`.
        let ir = gen_src("fn main() -> i32 { let s: str = \"hi\"; return 0; }");
        assert!(
            ir.contains("@.str.0 = private unnamed_addr constant"),
            "expected @.str.0 global, got:\n{ir}"
        );
        // Bytes plus NUL: 2 + 1 = 3.
        assert!(
            ir.contains("[3 x i8] c\"hi\\00\""),
            "expected NUL-terminated payload, got:\n{ir}"
        );
    }

    #[test]
    fn str_literal_inside_loop_block_collected() {
        // Phase 3B regression guard (2026-05-15): the str-literal pre-pass
        // used to skip plain `loop { ... }` statements, so any literal
        // inside one tripped a codegen `expect` at use time. Walk it.
        let ir = gen_src("fn main() -> i32 { loop { let s: str = \"x\"; break; } return 0; }");
        assert!(
            ir.contains("@.str.0 = private unnamed_addr constant"),
            "expected @.str.0 to be emitted for the loop-body literal, got:\n{ir}"
        );
    }

    #[test]
    fn str_literal_dedupes_by_content() {
        // Two uses of the same literal share one global.
        let ir = gen_src("fn main() -> i32 { let a: str = \"x\"; let b: str = \"x\"; return 0; }");
        let count = ir.matches("@.str.0 = private unnamed_addr").count();
        assert_eq!(count, 1, "expected one @.str.0 declaration");
        // No @.str.1 should appear from the second use of the same literal.
        assert!(
            !ir.contains("@.str.1 = private unnamed_addr"),
            "expected dedup, second literal not to allocate a new symbol"
        );
    }

    #[test]
    fn str_value_builds_fat_pointer() {
        // The literal expression's SSA value is an `insertvalue` chain
        // into `{ ptr, i64 }`.
        let ir = gen_src("fn main() -> i32 { let s: str = \"ab\"; return 0; }");
        assert!(ir.contains("insertvalue { ptr, i64 } undef, ptr @.str.0, 0"));
        assert!(ir.contains("insertvalue { ptr, i64 }"));
        // Length stored is 2 (bytes), not 3 (including NUL).
        assert!(ir.contains("i64 2, 1"));
    }

    #[test]
    fn println_str_uses_dotstar_format() {
        // Slice 8.STR.2: `#println(str)` lowers to printf with `%.*s\n`.
        let ir = gen_src("fn main() -> i32 { #println(\"hi\"); return 0; }");
        assert!(ir.contains("@.fmt_str_nl"));
        assert!(ir.contains("call i32 (ptr, ...) @printf(ptr noundef @.fmt_str_nl, i32"));
    }

    #[test]
    fn str_equality_uses_memcmp() {
        // Slice 8.STR.3: `==` on `str` lowers to a length-prechecked
        // memcmp call. v0.0.8 fix B: the declaration carries the
        // `readonly noundef` parameter attributes — see
        // `libc_declarations_carry_noalias_and_readonly_attrs`.
        let ir = gen_src("fn main() -> i32 { if \"a\" == \"a\" { return 0; } return 1; }");
        assert!(ir.contains("declare i32 @memcmp("));
        assert!(ir.contains("call i32 @memcmp(ptr"));
    }

    #[test]
    fn call_site_mirrors_callee_ptr_attrs() {
        // v0.0.8 fix B (finish): when a callee declares `mut t: Tag`
        // (non-Copy struct ptr-passed) the call site should mirror the
        // full attribute set, not just bare `ptr <addr>`. clang emits
        // these attrs at the call site too — it helps inter-procedural
        // analysis before inlining and matches the IR shape clang
        // produces.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }",
        );
        // Definition still has the attrs (pre-existing behavior).
        assert!(
            ir.contains("void @bump(ptr noalias nonnull noundef dereferenceable(4) align 4 %0)"),
            "bump definition missing param attrs, got:\n{ir}"
        );
        // Call site now mirrors the same attrs. v0.0.8 fix C: `bump`
        // is non-pub so the call site also picks up `fastcc`.
        assert!(
            ir.contains(
                "call fastcc void @bump(ptr noalias nonnull noundef dereferenceable(4) align 4 "
            ),
            "bump call site missing param attrs (or fastcc), got:\n{ir}"
        );
    }

    #[test]
    fn shared_param_call_site_mirrors_readonly_attrs() {
        // v0.0.8 fix B (finish): shared borrow `borrow t: Tag` ptr-passed param
        // gets `readonly` (not `noalias`) at both def and call site.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn peek(borrow t: Tag) -> i32 { return t.v; }\n\
             fn main() -> i32 { let x: Tag = Tag { v: 1 }; return peek(x); }",
        );
        assert!(
            ir.contains("i32 @peek(ptr readonly nonnull noundef dereferenceable(4) align 4 %0)"),
            "peek definition missing readonly attr set, got:\n{ir}"
        );
        assert!(
            ir.contains(
                "call fastcc i32 @peek(ptr readonly nonnull noundef dereferenceable(4) align 4 "
            ),
            "peek call site missing readonly attr set (or fastcc), got:\n{ir}"
        );
        // And NOT `noalias` at the call site — shared borrow can alias.
        assert!(
            !ir.contains("call fastcc i32 @peek(ptr noalias"),
            "shared call site must not get `noalias`, got:\n{ir}"
        );
    }

    #[test]
    fn fix_d_trivial_getter_is_inlined_at_call_site() {
        // v0.0.8 bench-gap fix D: a method whose body is exactly
        // `return self.<field>;` (no params, Read receiver, no
        // gen/async/generic) is inlined at the call site to a
        // `getelementptr inbounds` + `load`, skipping the call. The
        // method definition is still emitted (clang's DCE strips
        // unreferenced internals at -O3).
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.get(); }",
        );
        // No `call ...@P.get` in main's body — the getter was inlined.
        let main_start = ir.find("@main()").expect("@main present");
        let main_end = ir[main_start..].find("\n}").expect("@main close");
        let main_body = &ir[main_start..main_start + main_end];
        assert!(
            !main_body.contains("call fastcc i32 @P.get"),
            "trivial getter must be inlined at call site, got:\n{main_body}"
        );
        // The inlined IR is GEP + load of field 0.
        assert!(
            main_body.contains("getelementptr inbounds %P,"),
            "expected inlined GEP into P, got:\n{main_body}"
        );
    }

    #[test]
    fn fix_d_getter_with_extra_param_is_not_inlined() {
        // Negative pin: the detector requires zero params. Adding any
        // param (here, an unused `_unused: i32`) busts the trivial
        // pattern and the call site keeps the `call` instruction.
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn get(self, _unused: i32) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.get(0); }",
        );
        assert!(
            ir.contains("call fastcc i32 @P.get("),
            "getter with extra params must NOT be inlined, got:\n{ir}"
        );
    }

    #[test]
    fn fix_d_mut_self_getter_is_not_inlined() {
        // Negative pin: only `self` (Read) receivers trigger the
        // inliner. `mut self` / `move self` keep the call (writes /
        // ownership transfers aren't a getter shape).
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn get(mut self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let mut p: P = P { x: 7 }; return p.get(); }",
        );
        assert!(
            ir.contains("call fastcc i32 @P.get("),
            "mut-self getter must NOT be inlined, got:\n{ir}"
        );
    }

    #[test]
    fn nested_call_arg_does_not_steal_musttail_flag() {
        // v0.0.8 post-bench-gap: `return outer(... nested(...) ...)` —
        // `pending_musttail` was being consumed by the FIRST nested
        // Call encountered during arg evaluation, even though only the
        // OUTER call is in tail position. Resulted in clang rejecting
        // the IR ("musttail call must precede a ret" or "cannot
        // guarantee tail call due to mismatched return types").
        //
        // The fix moved the capture-and-clear of `pending_musttail` to
        // the top of `gen_named_call`, so nested arg-evaluation calls
        // see `false`.
        let ir = gen_src(
            "struct V3 { x: f32, y: f32, z: f32 }\n\
             fn sub(a: V3, b: V3) -> V3 { return V3 { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z }; }\n\
             fn scale(a: V3, s: f32) -> V3 { return V3 { x: a.x * s, y: a.y * s, z: a.z * s }; }\n\
             fn dot(a: V3, b: V3) -> f32 { return a.x*b.x + a.y*b.y + a.z*b.z; }\n\
             fn reflect(v: V3, n: V3) -> V3 {\n\
                 return sub(v, scale(n, 2.0f32 * dot(v, n)));\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        // Only the outer `sub` call gets musttail (it's in tail
        // position, returns V3 like reflect).
        assert!(
            ir.contains("musttail call fastcc %V3 @sub("),
            "expected outer `sub` to get musttail, got:\n{ir}"
        );
        // The nested `dot` (returns float, NOT V3) must NOT be
        // musttail'd — that's the bug.
        assert!(
            !ir.contains("musttail call fastcc float @dot"),
            "nested `dot` arg must NOT be musttail (wrong return type), got:\n{ir}"
        );
        // Same for nested `scale` (its result is consumed by `sub`,
        // not immediately returned).
        assert!(
            !ir.contains("musttail call fastcc %V3 @scale"),
            "nested `scale` arg must NOT be musttail (not in tail position), got:\n{ir}"
        );
    }

    #[test]
    fn restrict_raw_pointer_param_emits_noalias_at_def() {
        // v0.0.8 post-bench-gap: `restrict p: *T` opt-in noalias for
        // raw-pointer params. Lowers to `ptr noalias noundef` at the
        // function definition.
        let ir = gen_src(
            "fn axpy(n: usize, restrict x: *f32, restrict y: *f32) { return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            ir.contains("@axpy(i64 noundef %0, ptr noalias noundef %1, ptr noalias noundef %2)"),
            "expected restrict params to lower to `ptr noalias noundef`, got:\n{ir}"
        );
    }

    #[test]
    fn restrict_raw_pointer_arg_emits_noalias_at_call_site() {
        // The call site must mirror the callee's `noalias noundef` on
        // a `restrict *T` param so LLVM's verifier sees a consistent
        // attribute set on both sides.
        let ir = gen_src(
            "fn axpy(n: usize, restrict x: *f32, restrict y: *f32) { return; }\n\
             fn main() -> i32 {\n\
                 let p: *f32 = unsafe { 0 as *f32 };\n\
                 let q: *f32 = unsafe { 0 as *f32 };\n\
                 axpy(0 as usize, p, q);\n\
                 return 0;\n\
             }",
        );
        assert!(
            ir.contains("call fastcc void @axpy(i64 ")
                && ir.contains("ptr noalias noundef "),
            "expected call site to mirror restrict noalias on both ptr args, got:\n{ir}"
        );
    }

    #[test]
    fn restrict_without_marker_emits_only_noundef() {
        // Negative pin: a `*T` param without `restrict` keeps the bare
        // `noundef` attr set — restrict isn't a default.
        let ir = gen_src(
            "fn touch(p: *f32) { return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            ir.contains("@touch(ptr noundef %0)"),
            "expected bare `noundef` on non-restrict ptr param, got:\n{ir}"
        );
        assert!(
            !ir.contains("@touch(ptr noalias"),
            "non-restrict ptr param must NOT get noalias, got:\n{ir}"
        );
    }

    #[test]
    fn fix_e_f32_literal_matches_clang_bit_pattern() {
        // obs.md fix E claimed cpc emits `float 0x3FD99999A0000000` for
        // `0.4f32` while C / clang produces `0x3ECCCCCC` — implying a
        // double-rounding bug. **The premise is falsified**:
        //
        //   1. Both cpc AND clang emit `float 0x3FD99999A0000000` for
        //      `0.4f` (verified against /tmp/bench-inspect/rt-c.ll vs
        //      rt-cplus.ll on the raytracer benchmark).
        //   2. The IEEE-754 round-to-nearest-even f32 closest to 0.4 IS
        //      0x3ECCCCCD (distance 5.96e-9), not 0x3ECCCCCC (distance
        //      2.38e-8). cpc + clang are both correct.
        //   3. The proposed "fix" hex `0x3FD9999A00000000` is itself
        //      malformed — that's a non-canonical f64 encoding of an f32
        //      value. The canonical f64 form of f32 0x3ECCCCCD is exactly
        //      `0x3FD99999A0000000` (mantissa shifted up by 29 bits, the
        //      f32→f64 promotion).
        //
        // This pin guards against a well-intentioned but incorrect "fix"
        // regressing the f32 emission to a non-canonical or
        // bit-divergent form. Any change to f32 literal lowering must
        // continue to emit `0x3FD99999A0000000` for `0.4f32`.
        let ir = gen_src(
            "fn main() -> i32 { let _x: f32 = 0.4f32; return 0; }",
        );
        assert!(
            ir.contains("float 0x3FD99999A0000000"),
            "expected canonical f32 0.4 hex (matches clang's emission), got:\n{ir}"
        );
        // And NOT the malformed "fix" hex.
        assert!(
            !ir.contains("float 0x3FD9999A00000000"),
            "non-canonical f64 encoding must not be emitted, got:\n{ir}"
        );
    }

    #[test]
    fn fix_c_pub_fn_keeps_default_cc() {
        // v0.0.8 fix C: `pub fn` has external linkage and must keep C
        // cc so its callers (other modules, the C ABI) can invoke it.
        let ir = gen_src(
            "pub fn pub_api(x: i32) -> i32 { return x +% x; }\n\
             fn main() -> i32 { return 0; }",
        );
        // No `fastcc` after `define ` on the pub fn.
        assert!(
            ir.contains("define i32 @pub_api(i32 noundef %0)"),
            "pub fn must keep default cc, got:\n{ir}"
        );
        assert!(
            !ir.contains("define fastcc i32 @pub_api"),
            "pub fn must NOT be fastcc, got:\n{ir}"
        );
    }

    #[test]
    fn fix_c_main_keeps_default_cc() {
        // v0.0.8 fix C: `main` is the OS-runtime entry point and must
        // keep C cc no matter what.
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(
            ir.contains("define i32 @main()"),
            "main must keep default cc, got:\n{ir}"
        );
        assert!(
            !ir.contains("define fastcc i32 @main"),
            "main must NOT be fastcc, got:\n{ir}"
        );
    }

    #[test]
    fn fix_c_address_taken_fn_keeps_default_cc() {
        // v0.0.8 fix C: if `&f` (i.e. the fn name as a value, not as a
        // Call's callee) appears anywhere, the address-taken pre-pass
        // adds `f` to the address-taken set and the eligibility check
        // drops it from fastcc. Otherwise calls through a C-cc fn
        // pointer would mismatch the fastcc callee.
        let ir = gen_src(
            "fn target() -> i32 { return 7; }\n\
             fn main() -> i32 {\n\
                 let fp: fn() -> i32 = target;\n\
                 return fp();\n\
             }",
        );
        assert!(
            ir.contains("define internal i32 @target()"),
            "address-taken target must keep default cc, got:\n{ir}"
        );
        assert!(
            !ir.contains("define internal fastcc i32 @target"),
            "address-taken target must NOT be fastcc, got:\n{ir}"
        );
    }

    #[test]
    fn fix_c_internal_fn_gets_fastcc() {
        // v0.0.8 fix C: a non-pub, non-extern, non-main, not-address-
        // taken function gets `internal fastcc`.
        let ir = gen_src(
            "fn helper(x: i32) -> i32 { return x +% x; }\n\
             fn main() -> i32 { return helper(7); }",
        );
        assert!(
            ir.contains("define internal fastcc i32 @helper("),
            "internal helper must get fastcc, got:\n{ir}"
        );
        // Direct call also picks up fastcc.
        assert!(
            ir.contains("call fastcc i32 @helper("),
            "call site must mirror fastcc, got:\n{ir}"
        );
    }

    #[test]
    fn auto_field_drop_recurses_struct_fields() {
        // v0.0.14: a struct with a Drop field but no own `drop` auto-drops the
        // field at scope exit (here: a moved-in owning param).
        let ir = gen_src(
            "struct Inner { opaque p: *u8 }\n\
             impl Inner { fn drop(mut self) { return; } }\n\
             struct Outer { inner: Inner }\n\
             fn consume(move o: Outer) -> i32 { return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            ir.contains("call preserve_nonecc void @Inner.drop"),
            "auto field-drop must invoke the field's destructor, got:\n{ir}"
        );
    }

    #[test]
    fn enum_variant_drop_switches_and_drops_payload() {
        // v0.0.14 enum-variant drop: dropping an owning enum switches on the
        // tag and tears down the active variant's payload.
        let ir = gen_src(
            "struct Inner { opaque p: *u8 }\n\
             impl Inner { fn drop(mut self) { return; } }\n\
             enum E { Has(Inner), None }\n\
             fn consume(move e: E) -> i32 { return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
        // The drop path (not a match) emits a tag switch + the payload drop.
        assert!(
            ir.contains("switch i32") && ir.contains("@Inner.drop"),
            "enum-variant drop must switch on the tag and drop the payload, got:\n{ir}"
        );
    }

    #[test]
    fn inline_asm_tier1_emits_sideeffect_call() {
        // v0.0.14 inline-asm Tier 1: a bare template lowers to an operand-free,
        // side-effecting asm call (`sideeffect` so DCE can't drop it).
        let ir = gen_src("fn main() -> i32 { unsafe { #asm(\"dmb ish\"); } return 0; }");
        assert!(
            ir.contains("call void asm sideeffect \"dmb ish\", \"\"()"),
            "expected operand-free sideeffect asm call, got:\n{ir}"
        );
    }

    #[test]
    fn inline_asm_tier2_emits_operands_and_constraints() {
        // `out(reg) s, in(reg) a, in(reg) b` -> output `=r`, two inputs `r`,
        // `{name}` placeholders rewritten to `$0/$1/$2`, result stored back.
        let ir = gen_src(
            "fn add(a: i64, b: i64) -> i64 {\n\
                 let mut s: i64 = 0;\n\
                 unsafe { #asm(\"add {s}, {a}, {b}\", s = out(reg) s, a = in(reg) a, b = in(reg) b); }\n\
                 return s;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            ir.contains("asm sideeffect \"add $0, $1, $2\", \"=r,r,r\""),
            "expected operand-numbered template + constraints, got:\n{ir}"
        );
        // Single output returns the scalar and is stored back.
        assert!(
            ir.contains("= call i64 asm sideeffect"),
            "expected i64-returning asm call, got:\n{ir}"
        );
    }

    #[test]
    fn inline_asm_tier2_inout_ties_input_to_output() {
        // `inout(reg) v` -> one output `=r` plus a tied input `0`.
        let ir = gen_src(
            "fn inc(x: i64) -> i64 {\n\
                 let mut v: i64 = x;\n\
                 unsafe { #asm(\"add {v}, {v}, #1\", v = inout(reg) v); }\n\
                 return v;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            ir.contains("asm sideeffect \"add $0, $0, #1\", \"=r,0\""),
            "expected tied inout constraint `=r,0`, got:\n{ir}"
        );
    }

    #[test]
    fn escape_asm_template_handles_specials() {
        // IR string-constant escaping for quote / backslash / non-printable,
        // plus the inline-asm `$` -> `$$` operand-sigil doubling.
        assert_eq!(escape_asm_template("dmb ish"), "dmb ish");
        assert_eq!(escape_asm_template("a\"b"), "a\\22b");
        assert_eq!(escape_asm_template("a\\b"), "a\\5Cb");
        assert_eq!(escape_asm_template("a\tb"), "a\\09b");
        assert_eq!(escape_asm_template("a$b"), "a$$b");
    }

    // v0.0.15: module-scope `#asm("...")` -> LLVM `module asm "..."`.
    #[test]
    fn module_asm_emits_module_asm_directive() {
        let ir = gen_src(
            "#asm(\".globl cplus_marker\");\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            ir.contains("module asm \".globl cplus_marker\""),
            "expected a module-level asm directive, got:\n{ir}"
        );
    }

    #[test]
    fn escape_llvm_str_handles_specials_without_dollar_doubling() {
        // Same IR string escaping as inline asm for quote / backslash /
        // non-printable, but `$` is *not* doubled: module asm has no operand
        // substitution, so `$` is an ordinary character.
        assert_eq!(escape_llvm_str(".text"), ".text");
        assert_eq!(escape_llvm_str("a\"b"), "a\\22b");
        assert_eq!(escape_llvm_str("a\\b"), "a\\5Cb");
        assert_eq!(escape_llvm_str("a\tb"), "a\\09b");
        assert_eq!(escape_llvm_str("a$b"), "a$b");
    }

    #[test]
    fn libc_declarations_carry_noalias_and_readonly_attrs() {
        // v0.0.8 bench-gap fix B: libc declarations match clang's
        // emission so LLVM's alias analysis can disambiguate heap
        // allocations and non-overlapping byte copies. Trigger an IR
        // emission that includes the libc preamble.
        let ir = gen_src("fn main() -> i32 { return 0; }");
        // malloc: noalias on the return + noundef everywhere.
        assert!(
            ir.contains("declare noalias noundef ptr @malloc(i64 noundef)"),
            "malloc declaration missing noalias/noundef attrs, got:\n{ir}"
        );
        // memcpy: noalias on both ptr params, writeonly on dst, readonly on src.
        assert!(
            ir.contains(
                "declare ptr @memcpy(ptr noalias noundef writeonly, \
                 ptr noalias noundef readonly, i64 noundef)"
            ),
            "memcpy declaration missing noalias/writeonly/readonly attrs, got:\n{ir}"
        );
        // memcmp: readonly on both ptr params, noundef everywhere.
        assert!(
            ir.contains(
                "declare i32 @memcmp(ptr noundef readonly, \
                 ptr noundef readonly, i64 noundef)"
            ),
            "memcmp declaration missing readonly/noundef attrs, got:\n{ir}"
        );
    }

    #[test]
    fn str_escape_sequences_in_global() {
        // `\n` in source becomes a real newline byte in the global blob,
        // encoded in the IR as `\0A`.
        let ir = gen_src("fn main() -> i32 { #println(\"a\\nb\"); return 0; }");
        assert!(
            ir.contains("\\0A"),
            "expected newline byte (\\0A) in IR, got:\n{ir}"
        );
    }

    #[test]
    fn negation_emits_sub_zero() {
        let ir = gen_src("fn main() -> i32 { let x: i32 = 5; return -x; }");
        assert!(ir.contains("sub i32 0,"));
    }

    #[test]
    fn logical_not_uses_xor() {
        let ir = gen_src("fn main() -> i32 { return if !(1 < 2) { 1 } else { 0 }; }");
        assert!(ir.contains("xor i1"));
    }

    #[test]
    fn factorial_compiles_to_ir() {
        let src = include_str!("../../docs/examples/factorial.cplus");
        let ir = gen_src(src);
        assert!(ir.contains("i32 @factorial(i32"));
        assert!(ir.contains("i32 @main()"));
    }

    #[test]
    fn fibonacci_compiles_to_ir() {
        let src = include_str!("../../docs/examples/fibonacci.cplus");
        let ir = gen_src(src);
        assert!(ir.contains("i32 @fib(i32"));
    }

    #[test]
    fn sum_range_compiles_to_ir() {
        let src = include_str!("../../docs/examples/sum_range.cplus");
        let _ir = gen_src(src);
    }

    #[test]
    fn c_for_compiles_to_ir() {
        let src = include_str!("../../docs/examples/c_for.cplus");
        let _ir = gen_src(src);
    }

    // ---- Phase 2 slice 1 codegen ----

    #[test]
    fn preamble_declares_overflow_intrinsics_for_all_widths() {
        let ir = gen_src("fn main() -> i32 { return 0; }");
        for op in ["sadd", "ssub", "smul"] {
            for bits in [8, 16, 32, 64] {
                let needle = format!("declare {{i{bits}, i1}} @llvm.{op}.with.overflow.i{bits}");
                assert!(ir.contains(&needle), "missing {needle} in: {ir}");
            }
        }
    }

    #[test]
    fn i64_arithmetic_uses_64bit_overflow_intrinsic_in_debug() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: i64 = 5; let _b: i64 = a + a; return 0; }",
            BuildMode::Debug,
        );
        assert!(ir.contains("call {i64, i1} @llvm.sadd.with.overflow.i64"));
    }

    #[test]
    fn i8_arithmetic_uses_8bit_overflow_intrinsic_in_debug() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: i8 = 1; let _b: i8 = a + a; return 0; }",
            BuildMode::Debug,
        );
        assert!(ir.contains("call {i8, i1} @llvm.sadd.with.overflow.i8"));
    }

    #[test]
    fn unsigned_arithmetic_uses_plain_op_no_overflow_check() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: u32 = 5; let _b: u32 = a + a; return 0; }",
            BuildMode::Debug,
        );
        // Unsigned overflow is well-defined wrapping; no intrinsic *call*.
        // (Declarations in preamble are fine.)
        assert!(!ir.contains("call {i32, i1}"));
        assert!(ir.contains(" = add i32 "));
    }

    #[test]
    fn unsigned_division_uses_udiv_with_zero_check() {
        let ir = gen_src_with(
            "fn main() -> i32 { let a: u32 = 10; let b: u32 = 2; let _c: u32 = a / b; return 0; }",
            BuildMode::Debug,
        );
        assert!(ir.contains(" = udiv i32 "));
        assert!(ir.contains("icmp eq i32"));
    }

    fn count(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn float_arithmetic_uses_fadd_etc() {
        // Use independent operands so the FMA peephole doesn't fire — this
        // test pins the plain fadd/fmul lowering path.
        let ir = gen_src("fn main() -> i32 { let a: f64 = 1.0; let b: f64 = 2.0; let c: f64 = 3.0; let _x: f64 = a + b; let _y: f64 = a * c; return 0; }");
        // `contract` fast-math flag lets LLVM fuse fmul+fadd into fmadd
        // (matches clang's `-ffp-contract=on` default).
        assert!(ir.contains(" = fadd contract double "));
        assert!(ir.contains(" = fmul contract double "));
        // No overflow-intrinsic *call* (the declaration in preamble is fine).
        assert_eq!(
            count(&ir, "call {"),
            0,
            "no checked-arith calls expected for float ops"
        );
    }

    /// B-10 helper: emit IR with floating-point contraction disabled, the
    /// `--fp-contract=off` path. Mirrors `gen_src_with` but routes through
    /// `generate_inner` with `fp_contract = false`.
    fn gen_src_no_fp_contract(src: &str) -> String {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(diags.is_empty(), "sema errors: {diags:#?}");
        generate_inner(
            &prog,
            BuildMode::Debug,
            false,
            None,
            None,
            &[],
            false,
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
    }

    #[test]
    fn fp_contract_off_drops_fmuladd_and_contract_flag() {
        // B-10: `a*b+c` contracts to `llvm.fmuladd` by default.
        let on = gen_src("fn f(a: f64, b: f64, c: f64) -> f64 { return a * b + c; }\n\
                          fn main() -> i32 { return 0; }");
        assert!(
            on.contains("call contract double @llvm.fmuladd.f64"),
            "default must contract to fmuladd, got:\n{on}"
        );

        // With fp-contraction off: plain fmul + fadd, no `contract` flag, no
        // fmuladd *call* in the body.
        let off = gen_src_no_fp_contract(
            "fn f(a: f64, b: f64, c: f64) -> f64 { return a * b + c; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            !off.contains("call contract double @llvm.fmuladd.f64"),
            "fp-contract=off must not emit an fmuladd call, got:\n{off}"
        );
        assert!(
            off.contains(" = fmul double ") && off.contains(" = fadd double "),
            "fp-contract=off must keep separate fmul + fadd, got:\n{off}"
        );
        assert!(
            !off.contains("fmul contract") && !off.contains("fadd contract"),
            "fp-contract=off must drop the `contract` fast-math flag, got:\n{off}"
        );
    }

    #[test]
    fn float_division_no_zero_check() {
        let ir = gen_src("fn main() -> i32 { let a: f64 = 1.0; let b: f64 = 2.0; let _c: f64 = a / b; return 0; }");
        assert!(ir.contains(" = fdiv contract double "));
        // Float div doesn't trap; no zero check.
        // (Other code paths may still have icmp eq for integer divs; assert
        // the fdiv lacks a preceding zero-check on a float.)
        let lines: Vec<&str> = ir.lines().collect();
        let fdiv_line = lines.iter().position(|l| l.contains(" = fdiv ")).unwrap();
        let preceding = &lines[fdiv_line.saturating_sub(3)..fdiv_line];
        for line in preceding {
            assert!(
                !line.contains("icmp eq double"),
                "float div should not have a zero-check"
            );
        }
    }

    #[test]
    fn float_negation_uses_fneg() {
        let ir = gen_src("fn main() -> i32 { let a: f64 = 5.0; let _b: f64 = -a; return 0; }");
        assert!(ir.contains(" = fneg double "));
    }

    #[test]
    fn signed_comparison_uses_signed_predicates() {
        let ir = gen_src("fn main() -> i32 { let a: i64 = 1; let b: i64 = 2; return if a < b { 0 } else { 1 }; }");
        assert!(ir.contains(" = icmp slt i64 "));
    }

    #[test]
    fn unsigned_comparison_uses_unsigned_predicates() {
        let ir = gen_src("fn main() -> i32 { let a: u64 = 1; let b: u64 = 2; return if a < b { 0 } else { 1 }; }");
        assert!(ir.contains(" = icmp ult i64 "));
    }

    #[test]
    fn float_comparison_uses_ordered_predicates() {
        let ir = gen_src("fn main() -> i32 { let a: f64 = 1.0; let b: f64 = 2.0; return if a < b { 0 } else { 1 }; }");
        assert!(ir.contains(" = fcmp olt double "));
    }

    #[test]
    fn cast_int_widen_uses_sext() {
        let ir = gen_src("fn main() -> i32 { let a: i8 = 5; let _b: i32 = a as i32; return 0; }");
        assert!(ir.contains(" = sext i8 "));
    }

    #[test]
    fn cast_uint_widen_uses_zext() {
        let ir = gen_src("fn main() -> i32 { let a: u8 = 5; let _b: u32 = a as u32; return 0; }");
        assert!(ir.contains(" = zext i8 "));
    }

    #[test]
    fn cast_int_narrow_uses_trunc() {
        let ir = gen_src("fn main() -> i32 { let a: i64 = 5; let _b: i8 = a as i8; return 0; }");
        assert!(ir.contains(" = trunc i64 "));
    }

    #[test]
    fn cast_int_to_float_uses_sitofp_or_uitofp() {
        let ir1 = gen_src("fn main() -> i32 { let a: i32 = 5; let _b: f64 = a as f64; return 0; }");
        assert!(ir1.contains(" = sitofp "));
        let ir2 = gen_src("fn main() -> i32 { let a: u32 = 5; let _b: f64 = a as f64; return 0; }");
        assert!(ir2.contains(" = uitofp "));
    }

    #[test]
    fn cast_float_to_int_uses_fptosi_or_fptoui() {
        let ir1 =
            gen_src("fn main() -> i32 { let a: f64 = 1.5; let _b: i32 = a as i32; return 0; }");
        assert!(ir1.contains(" = fptosi "));
        let ir2 =
            gen_src("fn main() -> i32 { let a: f64 = 1.5; let _b: u32 = a as u32; return 0; }");
        assert!(ir2.contains(" = fptoui "));
    }

    #[test]
    fn cast_float_widths_uses_fpext_or_fptrunc() {
        let ir1 =
            gen_src("fn main() -> i32 { let a: f32 = 1.0; let _b: f64 = a as f64; return 0; }");
        assert!(ir1.contains(" = fpext "));
        let ir2 =
            gen_src("fn main() -> i32 { let a: f64 = 1.0; let _b: f32 = a as f32; return 0; }");
        assert!(ir2.contains(" = fptrunc "));
    }

    #[test]
    fn cast_bool_to_int_uses_zext() {
        let ir = gen_src("fn main() -> i32 { let _b: i32 = true as i32; return 0; }");
        assert!(ir.contains(" = zext i1 "));
    }

    #[test]
    fn cast_signed_to_unsigned_same_width_is_bitcast() {
        let ir = gen_src("fn main() -> i32 { let a: i32 = 5; let _b: u32 = a as u32; return 0; }");
        // Same-width int cast is a no-op; use bitcast for IR validity.
        assert!(ir.contains(" = bitcast i32 "));
    }

    #[test]
    fn phase2_samples_compile_to_ir() {
        for name in [
            "mixed_ints.cplus",
            "float_arith.cplus",
            "unsigned.cplus",
            "direction.cplus",
        ] {
            let path = format!("{}/../docs/examples/{name}", env!("CARGO_MANIFEST_DIR"));
            let src = std::fs::read_to_string(path).unwrap();
            let _ir = gen_src(&src);
        }
    }

    // ---- Phase 2 slice 2A: enums + paths ----

    #[test]
    fn enum_path_lowers_to_int_constant() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { return Color::Green as i32; }",
        );
        // Green is index 1; the cast is enum→i32 which is a no-op.
        // The ret should reference the constant `1`.
        assert!(ir.contains("ret i32 1"), "expected `ret i32 1`, got: {ir}");
    }

    #[test]
    fn enum_equality_uses_icmp_eq_i32() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let c: Color = Color::Red; return if c == Color::Green { 1 } else { 0 }; }"
        );
        assert!(ir.contains("icmp eq i32"));
    }

    #[test]
    fn enum_typed_local_is_i32_alloca() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let _c: Color = Color::Red; return 0; }",
        );
        // Should have an i32 alloca for the Color local.
        assert!(ir.contains("alloca i32"));
    }

    #[test]
    fn enum_passed_as_argument_uses_i32() {
        let ir = gen_src(include_str!("../../docs/examples/direction.cplus"));
        assert!(ir.contains("i32 @opposite(i32"));
    }

    // ---- Phase 2 slice 2B: structs ----

    #[test]
    fn struct_decl_emits_named_type() {
        let ir = gen_src("struct Point { x: i32, y: i32 }\nfn main() -> i32 { return 0; }");
        assert!(
            ir.contains("%Point = type { i32, i32 }"),
            "expected struct decl in IR: {ir}"
        );
    }

    #[test]
    fn struct_literal_emits_alloca_and_per_field_store() {
        let ir = gen_src(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let _p: Point = Point { x: 1, y: 2 }; return 0; }",
        );
        assert!(ir.contains("alloca %Point"), "expected struct alloca: {ir}");
        assert!(
            ir.contains("getelementptr inbounds %Point"),
            "expected GEP into struct: {ir}"
        );
        assert!(ir.contains("store i32 1, ptr"));
        assert!(ir.contains("store i32 2, ptr"));
    }

    #[test]
    fn struct_field_read_uses_gep_load() {
        let ir = gen_src(
            "struct Point { x: i32, y: i32 }\n\
             fn first(p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(ir.contains("getelementptr inbounds %Point"));
        assert!(ir.contains("load i32, ptr"));
    }

    #[test]
    fn struct_field_write_uses_gep_store() {
        let ir = gen_src(
            "struct Counter { count: i32 }\n\
             fn main() -> i32 { let mut c: Counter = Counter { count: 0 }; c.count = 5; return 0; }"
        );
        assert!(ir.contains("getelementptr inbounds %Counter"));
        assert!(ir.contains("store i32 5, ptr"));
    }

    #[test]
    fn struct_passed_by_value_in_signature() {
        let ir = gen_src(include_str!("../../docs/examples/point.cplus"));
        assert!(ir.contains("i32 @distance_squared(%Point"));
    }

    #[test]
    fn nested_struct_chain_uses_chained_gep() {
        let ir = gen_src(include_str!("../../docs/examples/nested.cplus"));
        // The struct has fields { from: Point, to: Point }; the load chain
        // should GEP twice (Line.to then Point.x / Point.y).
        let geps = ir.matches("getelementptr").count();
        assert!(
            geps >= 4,
            "expected several GEPs in nested struct access; got {geps}: {ir}"
        );
    }

    #[test]
    fn empty_struct_emits_empty_named_type() {
        let ir =
            gen_src("struct Empty {}\nfn main() -> i32 { let _e: Empty = Empty {}; return 0; }");
        assert!(
            ir.contains("%Empty = type {  }"),
            "expected empty struct type: {ir}"
        );
    }

    #[test]
    fn phase2b_samples_compile_to_ir() {
        for name in ["point.cplus", "mutable_struct.cplus", "nested.cplus"] {
            let path = format!("{}/../docs/examples/{name}", env!("CARGO_MANIFEST_DIR"));
            let src = std::fs::read_to_string(path).unwrap();
            let _ir = gen_src(&src);
        }
    }

    // ---- Phase 2 slice 2C: methods + impl blocks ----

    #[test]
    fn method_name_is_mangled() {
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn new(x: i32) -> P { return P { x: x }; } }\n\
             fn main() -> i32 { let _p: P = P::new(5); return 0; }",
        );
        assert!(ir.contains("%P @P.new(i32 "), "expected mangled name: {ir}");
        // v0.0.8 fix C: `P.new` is non-pub → fastcc at the call site.
        assert!(
            ir.contains("call fastcc %P @P.new("),
            "expected mangled call: {ir}"
        );
    }

    #[test]
    fn read_self_on_copy_struct_takes_value_param() {
        // v0.0.8 bench-gap fix A: Copy receivers pass by value, so the
        // optimizer sees `self` as a register-resident aggregate and can
        // SROA / inline / vectorize the body. `P` has no Drop, so it's
        // Copy.
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.get(); }",
        );
        assert!(
            ir.contains("i32 @P.get(%P "),
            "expected by-value %P param for Copy self: {ir}"
        );
    }

    #[test]
    fn mut_self_on_copy_struct_stays_pointer_passed() {
        // v0.0.8 fix A scope: only Copy `self` (Read) goes by-value.
        // `mut self` on a Copy type stays pointer-passed so writes
        // observed by `self.x = v` propagate to the caller's place — the
        // language treats `mut self` as write-through, see
        // `phase7_generic_typed_impl_mut_self_runs` (e2e): a follow-up
        // `b.get()` call must observe the mutation.
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn set(mut self, v: i32) { self.x = v; } }\n\
             fn main() -> i32 { let mut p: P = P { x: 0 }; p.set(5); return 0; }",
        );
        assert!(
            ir.contains("void @P.set(ptr "),
            "mut self on Copy must stay pointer-passed: {ir}"
        );
        // Body should store through the ptr (GEP then store).
        assert!(ir.contains("getelementptr inbounds %P"));
    }

    #[test]
    fn instance_call_on_copy_passes_value() {
        // Body is `self.x +% 0` (not a bare `self.x`) so the trivial-
        // getter inliner from v0.0.8 fix D doesn't fire — this test
        // still exercises the call-site path. (See
        // `trivial_getter_is_inlined_at_call_site` for the inline path.)
        let ir = gen_src(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x +% 0; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; return p.get(); }",
        );
        // Call site loads the Copy receiver and passes by value.
        // v0.0.8 fix C: non-pub method → fastcc at call site.
        assert!(ir.contains("call fastcc i32 @P.get(%P "));
    }

    #[test]
    fn read_self_on_noncopy_struct_still_takes_ptr_param() {
        // Negative pin: the by-value lowering is gated on Copy. A
        // non-Copy struct (here, made non-Copy by an `impl Drop`) keeps
        // the borrow-ABI pointer shape. Body is `self.x +% 0` (not a
        // bare `self.x`) so v0.0.8 fix D's trivial-getter inliner
        // doesn't fire — this test still exercises the call-site path.
        let ir = gen_src(
            "struct Q { x: i32 }\n\
             impl Q { fn drop(mut self) { return; } fn get(self) -> i32 { return self.x +% 0; } }\n\
             fn main() -> i32 { let q: Q = Q { x: 7 }; let r: i32 = q.get(); return r; }",
        );
        assert!(
            ir.contains("i32 @Q.get(ptr "),
            "non-Copy receiver must stay pointer-passed: {ir}"
        );
        // v0.0.8 fix C: non-pub Q.get → fastcc at the call site.
        assert!(
            ir.contains("call fastcc i32 @Q.get(ptr "),
            "non-Copy call site must stay pointer-passed: {ir}"
        );
    }

    #[test]
    fn methods_sample_compiles_to_ir() {
        let _ir = gen_src(include_str!("../../docs/examples/methods.cplus"));
    }

    // ---- Phase 2 slice 2D: fixed-size arrays ----

    #[test]
    fn array_type_lowers_to_llvm_array() {
        let ir = gen_src("fn main() -> i32 { let _xs: [i32; 5] = [1, 2, 3, 4, 5]; return 0; }");
        assert!(
            ir.contains("alloca [5 x i32]"),
            "expected alloca for array: {ir}"
        );
        // Five stores (one per element).
        assert_eq!(
            ir.matches("store i32").count() >= 5,
            true,
            "expected ≥5 stores: {ir}"
        );
    }

    #[test]
    fn array_index_emits_bounds_check() {
        let ir =
            gen_src("fn main() -> i32 { let xs: [i32; 3] = [10, 20, 30]; return xs[0 as usize]; }");
        // Bounds check pattern: icmp uge i64 ..., 3
        assert!(
            ir.contains("icmp uge i64"),
            "expected bounds-check icmp: {ir}"
        );
        assert!(
            ir.contains("call void @llvm.trap()"),
            "expected trap branch: {ir}"
        );
        // GEP into the array.
        assert!(ir.contains("getelementptr inbounds [3 x i32]"));
    }

    #[test]
    fn array_indexed_assign_uses_gep_store() {
        let ir = gen_src(
            "fn main() -> i32 { let mut xs: [i32; 3] = [0, 0, 0]; xs[1 as usize] = 7; return 0; }",
        );
        assert!(ir.contains("getelementptr inbounds [3 x i32]"));
        assert!(ir.contains("store i32 7, ptr"));
    }

    #[test]
    fn array_as_param_uses_llvm_array_type() {
        let ir = gen_src(
            "fn first(xs: [i32; 3]) -> i32 { return xs[0 as usize]; }\n\
             fn main() -> i32 { return first([1, 2, 3]); }",
        );
        assert!(ir.contains("i32 @first([3 x i32]"));
    }

    #[test]
    fn array_samples_compile_to_ir() {
        for name in ["array_sum.cplus", "array_struct.cplus"] {
            let path = format!("{}/../docs/examples/{name}", env!("CARGO_MANIFEST_DIR"));
            let src = std::fs::read_to_string(path).unwrap();
            let _ir = gen_src(&src);
        }
    }

    #[test]
    fn function_body_terminates() {
        let ir = gen_src("fn f() { }\nfn main() -> i32 { return 0; }");
        assert!(ir.contains("ret void"));
        assert!(ir.contains("ret i32 0"));
    }

    #[test]
    fn wrapping_ops_use_plain_arithmetic_in_debug() {
        // Even in Debug mode, `+%`/`-%`/`*%` must NOT emit overflow-check
        // intrinsics — that's the whole point of the wrapping operators.
        let ir = gen_src_with(
            "fn main() -> i32 { return 1 +% 2 -% 3 *% 4; }",
            BuildMode::Debug,
        );
        assert!(ir.contains(" = add i32 "), "expected plain add, got: {ir}");
        assert!(ir.contains(" = sub i32 "));
        assert!(ir.contains(" = mul i32 "));
        // No checked-arithmetic call for the wrapping body. (The preamble
        // still declares the intrinsics for plain ops elsewhere, so we
        // can't just grep for "with.overflow" anywhere in the IR — instead
        // check that the body of `main` doesn't *call* the intrinsic.)
        let main_body_start = ir.find("i32 @main()").unwrap();
        let main_body = &ir[main_body_start..];
        assert!(
            !main_body.contains("call {i32, i1} @llvm.sadd.with.overflow"),
            "wrapping op leaked an overflow-check intrinsic into @main"
        );
    }

    #[test]
    fn wrapping_op_on_u32_uses_plain_add() {
        let ir = gen_src_with(
            "fn main() -> i32 { let x: u32 = 4000000000u32; let _y: u32 = x +% 1u32; return 0; }",
            BuildMode::Debug,
        );
        assert!(
            ir.contains(" = add i32 "),
            "expected plain add i32, got: {ir}"
        );
    }

    // Regression: gen_expr used to return Ty::I32 for every integer literal
    // regardless of suffix, which produced invalid LLVM IR for typed
    // destinations (array literals of non-i32 element types; arithmetic on
    // suffixed non-i32 literals).

    #[test]
    fn u8_array_literal_lowers_with_u8_element_type() {
        let ir = gen_src(
            "fn main() -> i32 { let a: [u8; 4] = [10u8, 20u8, 30u8, 40u8]; return a[0 as usize] as i32; }",
        );
        // The array's alloca must use i8 element type, not i32.
        assert!(
            ir.contains("alloca [4 x i8]"),
            "expected `alloca [4 x i8]` for the array literal, got: {ir}"
        );
        // And the per-element store must store an i8 value, not i32.
        assert!(
            ir.contains("store i8 "),
            "expected `store i8 ...` for each element, got: {ir}"
        );
    }

    #[test]
    fn suffixed_u64_arithmetic_uses_i64() {
        let ir = gen_src("fn main() -> i32 { let x: u64 = 1u64 +% 2u64; return x as i32; }");
        // u64 wrapping add must emit `add i64`, never `add i32`.
        assert!(
            ir.contains(" = add i64 "),
            "expected `add i64` for u64 wrapping add, got: {ir}"
        );
        assert!(
            !ir.contains(" = add i32 "),
            "u64 add must not lower to i32, got: {ir}"
        );
    }

    // ---- Phase 5 slice 5BC.codegen — §2.9 mut-borrow pointer ABI ----
    //
    // `mut x: T` on a non-Copy struct is an exclusive borrow per §2.9: the
    // callee's writes must propagate back to the caller's place. Codegen
    // lowers the parameter to a `ptr` and the call site takes the source's
    // address, so the callee operates on the caller's storage directly.
    //
    // Copy types, `move`-marked params, and shared (`x: T`) params are
    // unaffected — they stay value-passed.

    #[test]
    fn mut_param_noncopy_struct_lowers_to_ptr_noalias() {
        // Slice 6BC.codegen: Drop forces non-Copy. `bump` takes
        // `mut t: Tag` as `ptr noalias` — the borrow checker proves
        // uniqueness, so LLVM gets the strong promise.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }",
        );
        assert!(
            ir.contains("void @bump(ptr noalias "),
            "expected `mut t: Tag` to lower to `ptr noalias` param, got: {ir}"
        );
        // Call site still passes a pointer, not a struct value.
        // v0.0.8 fix C: non-pub `bump` → fastcc at the call.
        assert!(
            ir.contains("call fastcc void @bump(ptr "),
            "expected call site to pass ptr for non-Copy mut arg, got: {ir}"
        );
    }

    #[test]
    fn shared_param_noncopy_struct_lowers_to_ptr_readonly() {
        // Slice 6BC.codegen: a non-Copy shared borrow `borrow x: T` is
        // pointer-passed (avoids the byte-copy) and tagged
        // `readonly` (callee provably can't write). `noalias` would
        // be unsound — two shared args can be the same place.
        // (Bare `x: T` now *moves* — see `bare_noncopy_param_moves_value_passed`.)
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn peek(borrow t: Tag) -> i32 { return t.v; }\n\
             fn main() -> i32 { let x: Tag = Tag { v: 7 }; return peek(x); }",
        );
        assert!(
            ir.contains("i32 @peek(ptr readonly "),
            "expected `borrow t: Tag` to lower to `ptr readonly` param, got: {ir}"
        );
        // v0.0.8 fix C: non-pub `peek` → fastcc at the call.
        assert!(
            ir.contains("call fastcc i32 @peek(ptr "),
            "expected call site to pass ptr for non-Copy shared arg, got: {ir}"
        );
    }

    #[test]
    fn bare_noncopy_param_moves_value_passed() {
        // v0.0.12 fix: the v0.0.10 "non-Copy moves by default" rule, wired
        // through to codegen. A *bare* `x: T` on a non-Copy struct now lowers
        // like an explicit `move x: T` — struct-by-value, callee drop, caller
        // drop-flag flip — NOT the old `ptr readonly` borrow shape. The old
        // lowering double-freed when the value was forwarded back out
        // (`fn f(x: T) -> T { return x; }`): the caller's unconditional drop
        // and the new owner's drop both fired on the same heap.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn forward(x: Tag) -> Tag { return x; }\n\
             fn main() -> i32 { let b: Tag = Tag { v: 7 }; let c: Tag = forward(b); return c.v; }",
        );
        // Bare param is value-passed, like `move x: Tag`. (`forward` returns a
        // struct, so arg 0 is the sret pointer and the moved value is the
        // trailing `%Tag` by-value param.)
        assert!(
            ir.contains(", %Tag %"),
            "expected bare `x: Tag` to move (struct-by-value), got: {ir}"
        );
        assert!(
            !ir.contains("@forward(ptr readonly"),
            "bare `x: Tag` must NOT use the shared-borrow `ptr readonly` shape, got: {ir}"
        );
        // The caller emits a drop flag so it does not double-drop the moved `b`.
        assert!(
            ir.contains("b.drop_flag"),
            "expected caller drop-flag for the moved bare param, got: {ir}"
        );
    }

    #[test]
    fn move_param_noncopy_struct_stays_value_passed() {
        // `move x: T` transfers ownership; the by-value LLVM signature is
        // correct (callee owns the bytes and registers a scope-exit drop;
        // caller's drop-flag is flipped at the call site).
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn take(move t: Tag) -> i32 { return t.v; }\n\
             fn main() -> i32 { let x: Tag = Tag { v: 9 }; return take(x); }",
        );
        assert!(
            ir.contains("i32 @take(%Tag "),
            "expected `move t: Tag` to stay struct-by-value, got: {ir}"
        );
    }

    #[test]
    fn mut_param_copy_struct_stays_value_passed() {
        // Copy structs: `mut p: P` is local mutability per §2.9, not an
        // exclusive borrow. The LLVM signature must remain by-value so the
        // caller's storage is unaffected by the callee's writes.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             fn bump(mut p: P) -> i32 { p.v = p.v + 1; return p.v; }\n\
             fn main() -> i32 { let q: P = P { v: 5 }; return bump(q); }",
        );
        assert!(
            ir.contains("i32 @bump(%P "),
            "expected `mut p: P` on Copy struct to stay struct-by-value, got: {ir}"
        );
    }

    #[test]
    fn mut_param_noncopy_struct_no_alloca_in_callee() {
        // Pointer-passed params must NOT be re-alloca'd in the callee —
        // we bind directly to the SSA argument so writes hit the caller's
        // storage. Search the function body for an alloca of %Tag (would
        // indicate a stray re-allocation).
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 0 }; bump(x); return x.v; }",
        );
        // Find the @bump body and confirm it has no `alloca %Tag` inside.
        let body_start = ir.find("void @bump(").expect("@bump must be emitted");
        let body_tail = &ir[body_start..];
        let body_end = body_tail.find("\n}\n").expect("function close");
        let bump_body = &body_tail[..body_end];
        assert!(
            !bump_body.contains("alloca %Tag"),
            "ptr-passed `mut t: Tag` must not re-alloca in callee, got: {bump_body}"
        );
    }

    #[test]
    fn mut_param_noncopy_struct_no_double_drop() {
        // The callee bound a pointer to a non-Copy `mut` param must NOT
        // register a scope-exit drop — only the caller owns the value.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 0 }; bump(x); return x.v; }",
        );
        let body_start = ir.find("void @bump(").expect("@bump must be emitted");
        let body_tail = &ir[body_start..];
        let body_end = body_tail.find("\n}\n").expect("function close");
        let bump_body = &body_tail[..body_end];
        // @bump must NOT call @Tag.drop on the ptr — the caller will.
        assert!(
            !bump_body.contains("@Tag.drop"),
            "callee must not drop a non-Copy mut-borrow param, got: {bump_body}"
        );
    }

    // ---- Phase 5 slice 5ATTR.3: `assert EXPR;` ----

    #[test]
    fn assert_emits_conditional_trap() {
        let ir = gen_src("fn main() -> i32 { assert 1 == 1; return 0; }");
        // Branch on the bool, trap on the false path.
        assert!(ir.contains("br i1 "), "expected branch on i1: {ir}");
        assert!(
            ir.contains("call void @llvm.trap()"),
            "expected trap on false path: {ir}"
        );
        assert!(
            ir.contains("unreachable"),
            "expected unreachable after trap: {ir}"
        );
    }

    #[test]
    fn assert_in_test_fn_compiles_clean() {
        // A `#[test]` fn with `assert` lowers like any other fn — no
        // special test-driver synthesis yet (that's slice 5ATTR.4).
        let ir = gen_src(
            "#[test] fn ok() { assert 2 == 2; return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(ir.contains("void @ok("), "expected @ok defined: {ir}");
        assert!(ir.contains("call void @llvm.trap()"));
    }

    #[test]
    fn mut_param_noncopy_struct_via_method_call() {
        // Same borrow-ABI rule applies to non-receiver method params:
        // non-Copy `mut t: Tag` lowers as a `ptr noalias ...` parameter.
        // v0.0.8 fix A: `Tool` is Copy (no Drop), so its receiver is
        // value-passed; only the second param keeps the ptr shape.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             struct Tool {}\n\
             impl Tool { fn poke(self, mut t: Tag) { t.v = t.v + 1; return; } }\n\
             fn main() -> i32 {\n\
                 let mut x: Tag = Tag { v: 1 };\n\
                 let tool: Tool = Tool {};\n\
                 tool.poke(x);\n\
                 return x.v;\n\
             }",
        );
        // Tool.poke signature: Copy receiver by value, non-Copy mut param ptr.
        assert!(
            ir.contains("void @Tool.poke(%Tool "),
            "expected Copy receiver by value, got: {ir}"
        );
        assert!(
            ir.contains(", ptr noalias "),
            "expected `mut t: Tag` to lower to `ptr noalias`, got: {ir}"
        );
        // Call site: by-value receiver, ptr for the mut param.
        // v0.0.8 fix C: non-pub Tool.poke → fastcc at the call.
        assert!(
            ir.contains("call fastcc void @Tool.poke(%Tool "),
            "expected call to pass Copy receiver by value, got: {ir}"
        );
    }

    // ---- Phase v0.0.2 Slice 1A: LLVM information dividend ----
    //
    // Verifies that every fact the frontend has already proven is published
    // as an LLVM parameter attribute: noalias/readonly (existing), nonnull,
    // noundef, dereferenceable(N), align A on pointer-passed params; noundef
    // on value-passed scalar primitives.

    #[test]
    fn static_layout_primitives() {
        let t = TypeTable::default();
        assert_eq!(static_layout(&Ty::I8, &t), Some((1, 1)));
        assert_eq!(static_layout(&Ty::U8, &t), Some((1, 1)));
        assert_eq!(static_layout(&Ty::Bool, &t), Some((1, 1)));
        assert_eq!(static_layout(&Ty::I16, &t), Some((2, 2)));
        assert_eq!(static_layout(&Ty::U32, &t), Some((4, 4)));
        assert_eq!(static_layout(&Ty::F32, &t), Some((4, 4)));
        assert_eq!(static_layout(&Ty::I64, &t), Some((8, 8)));
        assert_eq!(static_layout(&Ty::Usize, &t), Some((8, 8)));
        assert_eq!(static_layout(&Ty::F64, &t), Some((8, 8)));
        assert_eq!(
            static_layout(&Ty::RawPtr(Box::new(Ty::U8)), &t),
            Some((8, 8))
        );
        // Fat pointers.
        assert_eq!(static_layout(&Ty::Str, &t), Some((16, 8)));
        assert_eq!(
            static_layout(&Ty::Slice(Box::new(Ty::I32)), &t),
            Some((16, 8))
        );
        assert_eq!(static_layout(&Ty::String, &t), Some((24, 8)));
        // Fixed-size array.
        assert_eq!(
            static_layout(&Ty::Array(Box::new(Ty::I32), 4), &t),
            Some((16, 4))
        );
    }

    #[test]
    fn static_layout_struct_with_padding() {
        // struct S { a: i8, b: i32, c: i8 } → size 12, align 4.
        // Layout: a at 0, pad to 4, b at 4..8, c at 8, pad to align 4 → 12.
        let src = "struct S { a: i8, b: i32, c: i8 }\nfn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["S"];
        assert_eq!(static_layout(&Ty::Struct(id), &types), Some((12, 4)));
    }

    #[test]
    fn is_scalar_ty_distinguishes_scalars_from_aggregates() {
        let t = TypeTable::default();
        assert!(is_scalar_ty(&Ty::I32, &t));
        assert!(is_scalar_ty(&Ty::Bool, &t));
        assert!(is_scalar_ty(&Ty::RawPtr(Box::new(Ty::U8)), &t));
        assert!(!is_scalar_ty(&Ty::Str, &t));
        assert!(!is_scalar_ty(&Ty::String, &t));
        assert!(!is_scalar_ty(&Ty::Slice(Box::new(Ty::I32)), &t));
        assert!(!is_scalar_ty(&Ty::Array(Box::new(Ty::I32), 4), &t));
        // Plain enum (no payloads) is scalar (i32); tagged enum is aggregate.
        let src = "enum Plain { A, B, C }\nenum Tagged { S(i32), N }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let plain = Ty::Enum(types.enum_by_name["Plain"]);
        let tagged = Ty::Enum(types.enum_by_name["Tagged"]);
        assert!(is_scalar_ty(&plain, &types));
        assert!(!is_scalar_ty(&tagged, &types));
    }

    #[test]
    fn primitive_value_param_gets_noundef() {
        // Definite-assignment + scalar → noundef.
        let ir = gen_src(
            "fn double(x: i32) -> i32 { return x + x; }\n\
             fn main() -> i32 { return double(21); }",
        );
        assert!(
            ir.contains("i32 @double(i32 noundef %0)"),
            "expected i32 param to carry noundef, got:\n{ir}"
        );
    }

    #[test]
    fn aggregate_value_param_does_not_get_noundef() {
        // `str` is an aggregate ({ ptr, i64 }) — noundef at aggregate level
        // would be unsound because padding/components may carry poison
        // through `insertvalue` chains. Skip noundef on aggregates.
        let ir = gen_src(
            "fn echo(s: str) -> str { return s; }\n\
             fn main() -> i32 { let r: str = echo(\"hi\"); return 0; }",
        );
        assert!(
            !ir.contains("{ ptr, i64 } noundef"),
            "value-passed aggregate must not carry noundef, got:\n{ir}"
        );
    }

    #[test]
    fn mut_param_noncopy_struct_emits_full_attr_set() {
        // `mut t: Tag` (non-Copy) gets the full pointer-attribute set:
        // noalias nonnull noundef dereferenceable(N) align A.
        // Tag = { i32 v } → size 4, align 4.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }",
        );
        assert!(
            ir.contains("void @bump(ptr noalias nonnull noundef dereferenceable(4) align 4 %0)"),
            "expected full attr set on mut ptr param, got:\n{ir}"
        );
    }

    #[test]
    fn shared_param_noncopy_struct_emits_readonly_attr_set() {
        // Shared borrow `borrow t: Tag` (non-Copy) gets readonly (not noalias)
        // plus the rest. Two shared params may legally point at the same place,
        // so noalias would be unsound.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn peek(borrow t: Tag) -> i32 { return t.v; }\n\
             fn main() -> i32 { let x: Tag = Tag { v: 7 }; return peek(x); }",
        );
        assert!(
            ir.contains("i32 @peek(ptr readonly nonnull noundef dereferenceable(4) align 4 %0)"),
            "expected readonly+rest on shared ptr param, got:\n{ir}"
        );
    }

    #[test]
    fn method_receiver_emits_receiver_attrs() {
        // Self / mut self / move self map to readonly / noalias / noalias.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn read(self) -> i32 { return self.v; }\n\
               fn bump(mut self) { self.v = self.v + 1; return; }\n\
               fn into(move self) -> i32 { return self.v; }\n\
             }\n\
             fn main() -> i32 {\n\
               let mut a: T = T { v: 1 }; a.bump();\n\
               let b: T = T { v: 2 }; let _r: i32 = b.read();\n\
               let c: T = T { v: 3 }; let _n: i32 = c.into();\n\
               return 0;\n\
             }",
        );
        // `self` (Read)  → readonly
        assert!(
            ir.contains("i32 @T.read(ptr readonly nonnull noundef dereferenceable(4) align 4 %0)"),
            "T.read receiver attrs missing, got:\n{ir}"
        );
        // `mut self` (Mut) → noalias
        assert!(
            ir.contains("void @T.bump(ptr noalias nonnull noundef dereferenceable(4) align 4 %0)"),
            "T.bump receiver attrs missing, got:\n{ir}"
        );
        // `move self` (Move) → noalias (callee owns; exclusive)
        assert!(
            ir.contains("i32 @T.into(ptr noalias nonnull noundef dereferenceable(4) align 4 %0)"),
            "T.into receiver attrs missing, got:\n{ir}"
        );
    }

    #[test]
    fn dereferenceable_size_matches_type_layout() {
        // Struct with mixed-size fields: { i8, i32 } → size 8, align 4.
        // (i8 at 0, padding to 4, i32 at 4..8.)
        let ir = gen_src(
            "struct Big { tag: i8, n: i32 }\n\
             impl Big { fn drop(mut self) { return; } }\n\
             fn use_it(borrow b: Big) -> i32 { return b.n; }\n\
             fn main() -> i32 { let x: Big = Big { tag: 1, n: 42 }; return use_it(x); }",
        );
        assert!(
            ir.contains("ptr readonly nonnull noundef dereferenceable(8) align 4"),
            "expected dereferenceable(8) align 4 for Big, got:\n{ir}"
        );
    }

    #[test]
    fn raw_pointer_param_intptr_does_not_get_nonnull() {
        // Slice 1A negative: a `*T` value-passed param (Copy, by-value) is
        // a scalar — it gets `noundef` but NOT `nonnull`/`dereferenceable`.
        // Raw pointers may be null in `unsafe` (slice 11.INTPTR via `0 as *T`).
        let ir = gen_src(
            "fn take(p: *u8) -> i32 { return 0; }\n\
             fn main() -> i32 { return take(unsafe { 0 as *u8 }); }",
        );
        // Look for the @take signature line; it must say `ptr noundef` but
        // not the pointer-target attribute set.
        let line = ir
            .lines()
            .find(|l| l.contains("i32 @take("))
            .expect("@take must be emitted");
        assert!(
            line.contains("ptr noundef"),
            "expected noundef on *u8 param: {line}"
        );
        assert!(
            !line.contains("nonnull"),
            "*u8 param must not carry nonnull: {line}"
        );
        assert!(
            !line.contains("dereferenceable"),
            "*u8 param must not carry dereferenceable: {line}"
        );
    }

    #[test]
    fn copy_struct_value_param_no_aggregate_noundef() {
        // Copy struct stays value-passed; aggregate value → no noundef.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             fn use_p(p: P) -> i32 { return p.v; }\n\
             fn main() -> i32 { let q: P = P { v: 5 }; return use_p(q); }",
        );
        let line = ir
            .lines()
            .find(|l| l.contains("i32 @use_p("))
            .expect("@use_p must be emitted");
        assert!(line.contains("%P "), "expected by-value P param: {line}");
        assert!(
            !line.contains("noundef"),
            "value-passed aggregate must not carry noundef: {line}"
        );
    }

    // ---- Phase v0.0.2 Slice 1B: !range / llvm.assume publication ----
    //
    // The borrow checker, exhaustiveness check, and bounds-check lowering
    // already prove tag-in-range / length-non-negative / index-in-bounds.
    // Slice 1B publishes those facts to LLVM as `!range` metadata and
    // `llvm.assume` calls so `-O2`'s ConstraintElimination / InstCombine
    // can fold redundant checks downstream.

    #[test]
    fn preamble_declares_assume_intrinsic() {
        // Used by both slice-len and bounds-check publication. Declared in
        // the preamble so unused programs drop it via DCE.
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(
            ir.contains("declare void @llvm.assume(i1 noundef)"),
            "missing llvm.assume declaration in preamble:\n{ir}"
        );
    }

    #[test]
    fn enum_tag_load_carries_range_metadata() {
        let ir = gen_src(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 {\n\
               let c: Color = Color::Green;\n\
               let r: i32 = match c { Color::Red => 1, Color::Green => 2, Color::Blue => 3 };\n\
               return r;\n\
             }",
        );
        // The tag is loaded with `, !range !N`.
        assert!(
            ir.contains("load i32, ptr") && ir.contains(", !range !"),
            "expected tag load with !range, got:\n{ir}"
        );
        // The metadata node is `!{i32 0, i32 3}` for the 3-variant enum.
        assert!(
            ir.contains("= !{i32 0, i32 3}"),
            "expected !{{i32 0, i32 3}} for 3-variant enum, got:\n{ir}"
        );
    }

    #[test]
    fn tagged_enum_tag_load_carries_range_metadata() {
        // Tagged enum (Option-like): match dispatch on the payload-bearing
        // variant. The tag GEP + load pattern is different from a plain
        // enum, but the `!range` attachment should still happen.
        let ir = gen_src(
            "enum Opt { Some(i32), None }\n\
             fn main() -> i32 {\n\
               let o: Opt = Opt::Some(7);\n\
               let r: i32 = match o { Opt::Some(v) => v, Opt::None => 0 };\n\
               return r;\n\
             }",
        );
        assert!(
            ir.contains("load i32, ptr") && ir.contains(", !range !"),
            "expected tagged-enum tag load with !range, got:\n{ir}"
        );
        assert!(
            ir.contains("= !{i32 0, i32 2}"),
            "expected !{{i32 0, i32 2}} for 2-variant Opt, got:\n{ir}"
        );
    }

    #[test]
    fn slice_len_emits_nonneg_assume() {
        // `slice_len` extractvalue gets a paired `icmp sge + llvm.assume`
        // because `!range` doesn't apply to extractvalue. -O2 propagates
        // the assume into range metadata downstream.
        let ir = gen_src(
            "extern fn malloc(n: usize) -> *u8;\n\
             fn main() -> i32 {\n\
               let buf: *u8 = unsafe { malloc(16 as usize) };\n\
               let p: *i32 = unsafe { buf as *i32 };\n\
               let s: i32[] = unsafe { #slice_from_raw_parts(p, 3 as usize) };\n\
               let n: usize = #slice_len(s);\n\
               return n as i32;\n\
             }",
        );
        assert!(
            ir.contains("icmp sge i64") && ir.contains("call void @llvm.assume(i1"),
            "expected slice_len followed by assume(sge ..., 0), got:\n{ir}"
        );
    }

    #[test]
    fn bounds_check_emits_in_bounds_assume() {
        // After the bounds-check branch lands in the ok-block, codegen
        // emits `assume(idx < N)` so -O2 can drop downstream redundant
        // checks. The IR contains BOTH the trap path and the assume.
        let ir = gen_src(
            "fn main() -> i32 {\n\
               let arr: [i32; 3] = [10, 20, 30];\n\
               let i: usize = 1 as usize;\n\
               return arr[i];\n\
             }",
        );
        // Trap path preserved.
        assert!(
            ir.contains("icmp uge i64"),
            "expected trap-side uge, got:\n{ir}"
        );
        assert!(
            ir.contains("call void @llvm.trap()"),
            "expected trap, got:\n{ir}"
        );
        // Assume on the ok side.
        assert!(
            ir.contains("icmp ult i64"),
            "expected ok-side ult, got:\n{ir}"
        );
        assert!(
            ir.contains("call void @llvm.assume(i1"),
            "expected llvm.assume after bounds check, got:\n{ir}"
        );
    }

    #[test]
    fn module_metadata_ids_start_at_high_offset() {
        // The range MD table uses `!100000+` to avoid colliding with
        // DWARF's `!0..!5` reserved + `!6..` function block. A program
        // that has any !range emission should have `!100000` defined.
        let ir = gen_src(
            "enum E { A, B }\n\
             fn main() -> i32 { let e: E = E::A; let _r: i32 = match e { E::A => 0, E::B => 1 }; return 0; }"
        );
        assert!(
            ir.contains("!100000 = !{"),
            "expected !100000 range MD node, got:\n{ir}"
        );
    }

    #[test]
    fn no_range_metadata_when_no_match_or_slice_or_index() {
        // Negative: a trivial program that never matches, never indexes
        // an array, never queries slice_len shouldn't emit any !range or
        // assume calls. (The `declare void @llvm.assume(...)` lives in
        // the preamble unconditionally — DCE handles it.)
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(
            !ir.contains("!range "),
            "trivial program must not carry !range, got:\n{ir}"
        );
        assert!(
            !ir.contains("call void @llvm.assume("),
            "trivial program must not call assume, got:\n{ir}"
        );
    }

    #[test]
    fn range_metadata_cache_reuses_node_id() {
        // Two matches on the same 3-variant enum should share one MD
        // node (the `register_range` cache key is (lo, hi, ty)).
        let ir = gen_src(
            "enum E { A, B, C }\n\
             fn main() -> i32 {\n\
               let a: E = E::A;\n\
               let b: E = E::B;\n\
               let r1: i32 = match a { E::A => 1, E::B => 2, E::C => 3 };\n\
               let r2: i32 = match b { E::A => 4, E::B => 5, E::C => 6 };\n\
               return r1 + r2;\n\
             }",
        );
        // Both match loads reference the same MD id (!100000).
        let occurrences = ir.matches(", !range !100000").count();
        assert!(
            occurrences >= 2,
            "expected MD id to be shared across two matches on same enum, got {occurrences}\n{ir}"
        );
        // Only one definition of !100000.
        let defs = ir.matches("!100000 = !{").count();
        assert_eq!(
            defs, 1,
            "expected one definition of !100000, got {defs}\n{ir}"
        );
    }

    // ---- Phase v0.0.2 Slice 1C: scoped !alias.scope / !noalias ----
    //
    // Borrowck proves that for every pointer-passed `mut`/`move` non-Copy
    // param, no other live pointer in the same function reaches the same
    // memory. Slice 1A encodes this as the `noalias` param attribute —
    // which degrades after inlining. Scoped alias metadata survives
    // inlining and feeds the loop vectorizer.

    #[test]
    fn two_mut_noncopy_params_emit_domain_and_scopes() {
        // `swap(mut a: Tag, mut b: Tag)` has two noalias-shaped pointers.
        // The function gets one domain MD node and two scope nodes.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn swap(mut a: Tag, mut b: Tag) {\n\
               let ta: i32 = a.v;\n\
               let tb: i32 = b.v;\n\
               a.v = tb;\n\
               b.v = ta;\n\
               return;\n\
             }\n\
             fn main() -> i32 {\n\
               let mut x: Tag = Tag { v: 1 };\n\
               let mut y: Tag = Tag { v: 2 };\n\
               swap(x, y);\n\
               return x.v;\n\
             }",
        );
        // Domain (self-referential, labeled with fn name).
        assert!(
            ir.contains("distinct !{") && ir.contains("\"swap\""),
            "expected swap domain MD node, got:\n{ir}"
        );
        // Two scopes labeled `p0` and `p1`.
        assert!(ir.contains("\"p0\""), "expected scope p0, got:\n{ir}");
        assert!(ir.contains("\"p1\""), "expected scope p1, got:\n{ir}");
        // Loads through the params carry alias.scope+noalias.
        assert!(
            ir.contains(", !alias.scope ") && ir.contains(", !noalias !"),
            "expected alias-scope annotated loads/stores, got:\n{ir}"
        );
    }

    #[test]
    fn scope_propagates_through_gep_to_field_loads() {
        // A direct field read on a `mut` non-Copy param GEPs off the param's
        // SSA, then loads. The post-pass should propagate the scope from the
        // GEP source to the load.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             impl P { fn drop(mut self) { return; } }\n\
             fn pair(mut a: P, mut b: P) -> i32 { return a.v + b.v; }\n\
             fn main() -> i32 {\n\
               let p: P = P { v: 1 };\n\
               let q: P = P { v: 2 };\n\
               return pair(p, q);\n\
             }",
        );
        // Both loads (one per param) must be annotated.
        let load_count = ir
            .lines()
            .filter(|l| l.contains("load i32") && l.contains("!alias.scope"))
            .count();
        assert!(
            load_count >= 2,
            "expected >=2 scope-tagged loads, got {load_count}:\n{ir}"
        );
    }

    #[test]
    fn single_mut_param_does_not_emit_scope_metadata() {
        // With only one noalias-shaped param, no aliasing-pair exists, so
        // there's nothing useful to publish — skip the metadata to keep IR
        // small.
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }",
        );
        // The function body must not carry alias.scope on its loads.
        let body_start = ir.find("void @bump(").expect("@bump emitted");
        let body_end = ir[body_start..].find("\n}\n").expect("@bump close");
        let body = &ir[body_start..body_start + body_end];
        assert!(
            !body.contains("!alias.scope"),
            "single-mut-param fn should not carry alias.scope, got:\n{body}"
        );
    }

    #[test]
    fn shared_params_do_not_participate_in_scope_set() {
        // Shared (`x: T`, non-Copy) is `readonly` — two shared params may
        // legally alias each other (§2.9). They MUST NOT show up as
        // alias-scope sources.
        let ir = gen_src(
            "struct P { v: i32 }\n\
             impl P { fn drop(mut self) { return; } }\n\
             fn both_shared(a: P, b: P) -> i32 { return a.v + b.v; }\n\
             fn main() -> i32 {\n\
               let p: P = P { v: 1 };\n\
               let q: P = P { v: 2 };\n\
               return both_shared(p, q);\n\
             }",
        );
        let body_start = ir.find("i32 @both_shared(").expect("@both_shared emitted");
        let body_end = ir[body_start..].find("\n}\n").expect("@both_shared close");
        let body = &ir[body_start..body_start + body_end];
        assert!(
            !body.contains("!alias.scope"),
            "shared (readonly) params must not get alias.scope, got:\n{body}"
        );
    }

    #[test]
    fn method_receiver_and_mut_param_participate_in_scope_set() {
        // `mut self` is pointer-passed-and-exclusive; combined with a
        // separate `mut other: T` param, we have two scopes.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn merge(mut self, mut other: T) {\n\
                 self.v = self.v + other.v;\n\
                 other.v = 0;\n\
                 return;\n\
               }\n\
             }\n\
             fn main() -> i32 {\n\
               let mut a: T = T { v: 10 };\n\
               let mut b: T = T { v: 20 };\n\
               a.merge(b);\n\
               return a.v;\n\
             }",
        );
        assert!(
            ir.contains("\"T.merge\""),
            "expected T.merge domain MD, got:\n{ir}"
        );
        // Both p0 (self) and p1 (other) scopes present.
        assert!(
            ir.contains("\"p0\""),
            "expected p0 (self) scope, got:\n{ir}"
        );
        assert!(
            ir.contains("\"p1\""),
            "expected p1 (other) scope, got:\n{ir}"
        );
        // Annotated load+store pairs in body.
        assert!(
            ir.lines()
                .any(|l| l.contains("load") && l.contains("!alias.scope")),
            "expected scope-tagged load in T.merge, got:\n{ir}"
        );
        assert!(
            ir.lines()
                .any(|l| l.contains("store") && l.contains("!alias.scope")),
            "expected scope-tagged store in T.merge, got:\n{ir}"
        );
    }

    #[test]
    fn shared_self_receiver_does_not_get_scope() {
        // `self` (Read) is readonly, NOT noalias. A method with `self` +
        // a mut param has only one noalias-shaped pointer → no metadata.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn read_and_bump(self, mut other: T) -> i32 {\n\
                 other.v = self.v;\n\
                 return self.v;\n\
               }\n\
             }\n\
             fn main() -> i32 {\n\
               let a: T = T { v: 5 };\n\
               let mut b: T = T { v: 0 };\n\
               return a.read_and_bump(b);\n\
             }",
        );
        let body_start = ir.find("i32 @T.read_and_bump(").expect("emitted");
        let body_end = ir[body_start..].find("\n}\n").expect("close");
        let body = &ir[body_start..body_start + body_end];
        assert!(
            !body.contains("!alias.scope"),
            "self (Read) + one mut param = one noalias only → no scope metadata, got:\n{body}"
        );
    }

    #[test]
    fn extract_ptr_operand_basic() {
        // Hand the helper a synthetic instruction; verify it pulls out
        // the pointer operand.
        let p = extract_ptr_operand("load i32, ptr %t5, align 4");
        assert_eq!(p.as_deref(), Some("%t5"));
        let p = extract_ptr_operand("getelementptr inbounds %T, ptr %0, i32 0, i32 1");
        assert_eq!(p.as_deref(), Some("%0"));
        let p = extract_ptr_operand("store i32 7, ptr %dst");
        assert_eq!(p.as_deref(), Some("%dst"));
        // No `, ptr` operand → None.
        let p = extract_ptr_operand("add i32 %a, %b");
        assert!(p.is_none());
    }

    #[test]
    fn alias_scope_dataflow_propagates_through_chained_gep() {
        // GEPs feed off other GEPs (nested struct access). The dataflow
        // should propagate the scope along the chain so the final load is
        // annotated.
        let ir = gen_src(
            "struct Inner { x: i32 }\n\
             struct Outer { inner: Inner, tag: i32 }\n\
             impl Outer { fn drop(mut self) { return; } }\n\
             fn touch_both(mut a: Outer, mut b: Outer) -> i32 {\n\
               return a.inner.x + b.tag;\n\
             }\n\
             fn main() -> i32 {\n\
               let p: Outer = Outer { inner: Inner { x: 7 }, tag: 1 };\n\
               let q: Outer = Outer { inner: Inner { x: 3 }, tag: 2 };\n\
               return touch_both(p, q);\n\
             }",
        );
        // Nested load (a.inner.x) must carry alias.scope.
        assert!(
            ir.lines()
                .any(|l| l.contains("load i32") && l.contains("!alias.scope")),
            "expected nested-load scope annotation, got:\n{ir}"
        );
    }

    // ---- Phase v0.0.2 Slice 1F: cold + preserve_nonecc on drop glue ----
    //
    // Destructors are compiler-synthesized cold-path helpers. Marking them
    // `preserve_nonecc cold` lets the optimizer skip callee-save register
    // saves at the call boundary and biases hot paths away from drops.

    #[test]
    fn drop_method_emits_cold_and_preserve_none_cc() {
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             fn main() -> i32 { let r: R = R { v: 7 }; return r.v; }",
        );
        // `define [internal ]preserve_nonecc void @R.drop(...) cold {`
        // After Slice 3D, drop methods get `internal` linkage in exe mode.
        assert!(
            ir.contains("preserve_nonecc void @R.drop("),
            "expected preserve_nonecc on drop definition, got:\n{ir}"
        );
        // The `cold` attribute lands after the param list, before `{`.
        let drop_line = ir
            .lines()
            .find(|l| l.contains("@R.drop("))
            .expect("drop definition emitted");
        assert!(
            drop_line.ends_with(") cold {"),
            "drop definition must carry `cold`, got: {drop_line}"
        );
    }

    #[test]
    fn drop_call_sites_match_callee_cc() {
        // LLVM rejects IR where the call site's CC disagrees with the
        // callee's. The Always-disposition path emits the call.
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             fn main() -> i32 { let r: R = R { v: 7 }; return r.v; }",
        );
        assert!(
            ir.contains("call preserve_nonecc void @R.drop("),
            "drop call site must match preserve_nonecc CC, got:\n{ir}"
        );
    }

    #[test]
    fn non_drop_methods_keep_default_cc() {
        // Only `drop` methods get the cold CC. Regular methods continue
        // to use the default C calling convention.
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R {\n\
               fn drop(mut self) { return; }\n\
               fn bump(mut self) -> i32 { self.v = self.v + 1; return self.v; }\n\
             }\n\
             fn main() -> i32 { let mut r: R = R { v: 0 }; return r.bump(); }",
        );
        // R.bump must NOT have preserve_nonecc or `) cold {`.
        let bump_line = ir
            .lines()
            .find(|l| l.contains("@R.bump("))
            .expect("@R.bump emitted");
        assert!(
            !bump_line.contains("preserve_nonecc"),
            "non-drop method must not get preserve_nonecc, got: {bump_line}"
        );
        assert!(
            !bump_line.contains("cold"),
            "non-drop method must not get cold, got: {bump_line}"
        );
    }

    #[test]
    fn non_drop_call_sites_use_fastcc_not_preserve_nonecc() {
        // Slice 1F: only `drop` methods get `preserve_nonecc` (the cold
        // cc that skips callee-save register saves). Other methods stay
        // out of that path.
        //
        // v0.0.8 fix C: non-pub, non-drop methods now get `fastcc` (a
        // different cc, but distinct from `preserve_nonecc`). Verify
        // both: `R.drop` uses preserve_nonecc; `R.bump` (non-pub
        // non-drop) uses fastcc.
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R {\n\
               fn drop(mut self) { return; }\n\
               fn bump(mut self) -> i32 { self.v = self.v + 1; return self.v; }\n\
             }\n\
             fn main() -> i32 { let mut r: R = R { v: 0 }; return r.bump(); }",
        );
        // R.bump is fastcc; R.drop is preserve_nonecc — the two never
        // share a cc, but both call sites carry SOME cc tag now.
        assert!(
            ir.contains("call fastcc i32 @R.bump("),
            "expected fastcc call to R.bump, got:\n{ir}"
        );
        assert!(
            !ir.contains("call fastcc preserve_nonecc")
                && !ir.contains("call preserve_nonecc fastcc"),
            "drop's preserve_nonecc and bump's fastcc must not collide, got:\n{ir}"
        );
        // Drop call still emits preserve_nonecc.
        assert!(
            ir.contains("call preserve_nonecc void @R.drop("),
            "expected preserve_nonecc call to R.drop, got:\n{ir}"
        );
    }

    // ---- Phase v0.0.2 Slice 1E: musttail on tail-position direct calls ----
    //
    // `return foo(args);` where caller and callee have matching signature
    // can be a guaranteed tail call. LLVM's verifier rejects musttail when
    // the param-count/type signature doesn't match exactly, so the
    // predicate is conservative.

    #[test]
    fn recursive_tail_call_uses_musttail() {
        // `sum_to(n, acc)` recurses with `return sum_to(n-1, acc+n)`. The
        // caller and callee have identical signatures, so musttail fires.
        let ir = gen_src(
            "fn sum_to(n: i32, acc: i32) -> i32 {\n\
               if n == 0 { return acc; }\n\
               return sum_to(n - 1, acc + n);\n\
             }\n\
             fn main() -> i32 { return sum_to(10, 0); }",
        );
        // The recursive call must be musttail. v0.0.8 fix C: non-pub
        // `sum_to` → fastcc; musttail call site must mirror the cc.
        let line = ir
            .lines()
            .find(|l| l.contains("call fastcc i32 @sum_to") && l.contains("musttail"))
            .expect("expected musttail recursive call");
        assert!(
            line.contains("musttail call fastcc i32 @sum_to"),
            "got: {line}"
        );
    }

    #[test]
    fn entry_call_with_mismatched_signature_does_not_use_musttail() {
        // `main() -> i32` returning `sum_to(args) -> i32` doesn't qualify:
        // caller has 0 params, callee has 2. LLVM would reject; the
        // predicate must bail.
        let ir = gen_src(
            "fn sum_to(n: i32, acc: i32) -> i32 {\n\
               if n == 0 { return acc; }\n\
               return sum_to(n - 1, acc + n);\n\
             }\n\
             fn main() -> i32 { return sum_to(10, 0); }",
        );
        // The main's call must be a plain `call`, not `musttail`.
        let main_start = ir.find("i32 @main()").expect("@main emitted");
        let main_end = ir[main_start..].find("\n}\n").expect("@main close");
        let main_body = &ir[main_start..main_start + main_end];
        assert!(
            main_body.contains("call fastcc i32 @sum_to"),
            "expected call to sum_to in main: {main_body}"
        );
        assert!(
            !main_body.contains("musttail"),
            "main → sum_to (mismatched sig) must not be musttail: {main_body}"
        );
    }

    #[test]
    fn mismatched_return_type_does_not_use_musttail() {
        // Caller returns i32, callee returns i64 → no musttail.
        let ir = gen_src(
            "fn long_id(n: i64) -> i64 { return n; }\n\
             fn short_id(n: i32) -> i32 { return n; }\n\
             fn caller(n: i32) -> i32 { return short_id(n); }\n\
             fn main() -> i32 { return caller(0); }",
        );
        // caller → short_id: same signature, same return → musttail.
        // v0.0.8 fix C: non-pub `short_id` → fastcc at the call.
        assert!(
            ir.contains("musttail call fastcc i32 @short_id"),
            "expected matching-sig musttail, got:\n{ir}"
        );
        // long_id is never tail-called.
        assert!(
            !ir.contains("musttail call fastcc i64 @long_id"),
            "i64 fn must not appear as musttail from i32-returning caller, got:\n{ir}"
        );
    }

    #[test]
    fn return_with_no_call_does_not_set_musttail_flag() {
        // Plain `return x;` (not a call) should produce a plain `ret`.
        let ir = gen_src("fn id(n: i32) -> i32 { return n + 1; }");
        assert!(
            !ir.contains("musttail"),
            "no call → no musttail, got:\n{ir}"
        );
        assert!(ir.contains("ret i32"), "expected ret i32: {ir}");
    }

    #[test]
    fn methods_do_not_emit_musttail() {
        // Method bodies carry an implicit receiver, so even if the body
        // does `return helper()`, the receiver-vs-no-receiver mismatch
        // would make musttail invalid. The eligibility flag suppresses.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             fn helper() -> i32 { return 1; }\n\
             impl T {\n\
               fn get(self) -> i32 { return helper(); }\n\
             }\n\
             fn main() -> i32 { let t: T = T { v: 0 }; return t.get(); }",
        );
        // T.get must NOT use musttail.
        let m_start = ir.find("i32 @T.get(").expect("T.get emitted");
        let m_end = ir[m_start..].find("\n}\n").expect("T.get close");
        let m_body = &ir[m_start..m_start + m_end];
        assert!(
            !m_body.contains("musttail"),
            "method body must not emit musttail (receiver shape would mismatch): {m_body}"
        );
    }

    #[test]
    fn return_drop_value_does_not_use_musttail() {
        // A Drop-bound local creates a pending scope-exit. musttail
        // requires the ret to immediately follow the call (no drop
        // emission between), so the predicate must bail.
        let ir = gen_src(
            "struct R { v: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             fn id(n: i32) -> i32 { return n; }\n\
             fn caller(n: i32) -> i32 {\n\
               let r: R = R { v: 1 };\n\
               return id(n);\n\
             }\n\
             fn main() -> i32 { return caller(5); }",
        );
        // caller's body has a Drop binding, so its `return id(n)` cannot
        // be musttail (drop runs between the call and the ret).
        let c_start = ir.find("i32 @caller(").expect("caller emitted");
        let c_end = ir[c_start..].find("\n}\n").expect("caller close");
        let c_body = &ir[c_start..c_start + c_end];
        assert!(
            !c_body.contains("musttail"),
            "pending Drop must suppress musttail in caller: {c_body}"
        );
    }

    // ---- Phase v0.0.2 Slice 1D: sret for owned-string returns ----
    //
    // Owned `string` is the canonical sret case: 24-byte aggregate with
    // Drop. Returning by value forces LLVM to copy through registers +
    // memory; `sret` collapses to a single write into the caller's slot.
    // The narrow scope (string only) bounds the ABI-change blast radius.

    #[test]
    fn return_passes_by_sret_predicate() {
        // Today only Ty::String triggers sret; primitives and slices stay
        // value-returned. Generic non-Copy struct sret is deferred.
        assert!(return_passes_by_sret(&Ty::String));
        assert!(!return_passes_by_sret(&Ty::I32));
        assert!(!return_passes_by_sret(&Ty::Str));
        assert!(!return_passes_by_sret(&Ty::Slice(Box::new(Ty::I32))));
        assert!(!return_passes_by_sret(&Ty::Unit));
    }

    #[test]
    fn string_returning_fn_uses_sret_definition() {
        let ir = gen_src(
            "fn greet() -> string { return \"hi\".to_string(); }\n\
             fn main() -> i32 { let s: string = greet(); return 0; }",
        );
        // The function returns void and takes a sret pointer as %0.
        assert!(
            ir.contains("void @greet(ptr sret({ ptr, i64, i64 }) noalias nonnull noundef writable dereferenceable(24) align 8 %0)"),
            "expected sret definition, got:\n{ir}"
        );
        // The body stores into %0 then returns void.
        assert!(
            ir.contains("store { ptr, i64, i64 }") && ir.contains(", ptr %0"),
            "expected store-to-sret-slot, got:\n{ir}"
        );
    }

    #[test]
    fn string_returning_fn_call_site_uses_sret_slot() {
        // The caller allocates a 24-byte slot, passes it as the sret
        // arg, and loads the result back for value-semantics consumers.
        let ir = gen_src(
            "fn greet() -> string { return \"hi\".to_string(); }\n\
             fn main() -> i32 { let s: string = greet(); return 0; }",
        );
        // v0.0.8 fix C: non-pub `greet` → fastcc at the call.
        assert!(
            ir.contains("call fastcc void @greet(ptr "),
            "expected void-returning call to greet, got:\n{ir}"
        );
        // After the call, the caller loads the value back from the slot.
        assert!(
            ir.contains("load { ptr, i64, i64 }, ptr"),
            "expected load-from-slot after sret call, got:\n{ir}"
        );
    }

    #[test]
    fn extern_fn_returning_large_aggregate_uses_sret_g027() {
        // v0.0.12 G-027: extern fn returning a >16-byte aggregate MUST
        // emit sret on the import declaration AND at every call site,
        // because the C ABI on aarch64-darwin (and x86_64-sysv) requires
        // the caller to allocate the return slot and pass a hidden first
        // pointer. Pre-fix this declared `declare %T @f(...)` and called
        // with a direct struct return — the call wrote args into x0
        // where the callee expected the sret pointer → SIGSEGV.
        //
        // `string` is a 24-byte aggregate (ptr, len, cap); any 24B
        // `#[repr(C)]` struct would hit the same path.
        let ir = gen_src(
            "extern fn make_str() -> string;\n\
             fn main() -> i32 { let s: string = unsafe { make_str() }; return 0; }",
        );
        assert!(
            ir.contains("declare void @make_str(ptr sret"),
            "extern fn returning 24B aggregate must declare with sret, got:\n{ir}"
        );
        assert!(
            ir.contains("call void @make_str(ptr "),
            "extern fn returning 24B aggregate must call with sret slot, got:\n{ir}"
        );
    }

    #[test]
    fn extern_fn_taking_8b_struct_coerces_to_i64_g034() {
        // v0.0.12 G-034 (llama.cplus G-033): ≤8B struct-by-value param to
        // an extern import must be coerced to i64 at the call site,
        // matching the declaration. Pre-fix: passed raw `%T` aggregate.
        let ir = gen_src(
            "#[repr(C)] struct S8 { a: i64 }\n\
             extern fn take_s8(s: S8) -> i64;\n\
             fn main() -> i32 {\n\
                 let v: S8 = S8 { a: 1 as i64 };\n\
                 let _r: i64 = unsafe { take_s8(v) };\n\
                 return 0;\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @take_s8(i64)"),
            "≤8B struct param must declare as i64, got:\n{ir}"
        );
        assert!(
            ir.contains("= call i64 @take_s8(i64 "),
            "≤8B struct call site must pass i64 (coerced), not %S8, got:\n{ir}"
        );
    }

    #[test]
    fn extern_fn_taking_16b_struct_coerces_to_array_g034() {
        // v0.0.12 G-034: 9..16B struct → coerce to `[2 x i64]` on
        // aarch64-darwin (or `{ i64, i64 }` on x86_64-sysv).
        let ir = gen_src(
            "#[repr(C)] struct S16 { a: i64, b: i64 }\n\
             extern fn take_s16(s: S16) -> i64;\n\
             fn main() -> i32 {\n\
                 let v: S16 = S16 { a: 1 as i64, b: 2 as i64 };\n\
                 let _r: i64 = unsafe { take_s16(v) };\n\
                 return 0;\n\
             }",
        );
        // Windows (Microsoft x64) passes a 16-byte aggregate indirectly (bare
        // `ptr`), not coerced into a register pair like SysV/aarch64-darwin.
        if cfg!(all(target_arch = "x86_64", windows)) {
            assert!(
                ir.contains("declare i64 @take_s16(ptr)"),
                "16B struct param must declare as ptr (indirect) on Win64, got:\n{ir}"
            );
            assert!(
                ir.contains("= call i64 @take_s16(ptr "),
                "16B struct call site must pass ptr (indirect) on Win64, got:\n{ir}"
            );
            return;
        }
        let coerced = if cfg!(target_arch = "x86_64") {
            "{ i64, i64 }"
        } else {
            "[2 x i64]"
        };
        assert!(
            ir.contains(&format!("declare i64 @take_s16({coerced})")),
            "16B struct param must declare as {coerced}, got:\n{ir}"
        );
        assert!(
            ir.contains(&format!("= call i64 @take_s16({coerced} ")),
            "16B struct call site must pass {coerced} (coerced), not %S16, got:\n{ir}"
        );
    }

    #[test]
    fn extern_fn_taking_large_struct_passes_indirect_g034() {
        // v0.0.12 G-034: >16B struct → indirect (bare ptr) on both
        // aarch64-darwin and x86_64-sysv. The call site allocates the
        // struct on the caller's frame and passes its address.
        let ir = gen_src(
            "#[repr(C)] struct S24 { a: i64, b: i64, c: i64 }\n\
             extern fn take_s24(s: S24) -> i64;\n\
             fn main() -> i32 {\n\
                 let v: S24 = S24 { a: 1 as i64, b: 2 as i64, c: 3 as i64 };\n\
                 let _r: i64 = unsafe { take_s24(v) };\n\
                 return 0;\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @take_s24(ptr)"),
            ">16B struct param must declare as ptr (indirect), got:\n{ir}"
        );
        assert!(
            ir.contains("= call i64 @take_s24(ptr "),
            ">16B struct call site must pass ptr to caller-alloca'd slot, got:\n{ir}"
        );
    }

    #[test]
    fn primitive_returning_fn_keeps_value_abi() {
        // Slice 1D narrow scope: only `string` triggers sret. i32 returns
        // continue to use the value form.
        let ir = gen_src(
            "fn add(a: i32, b: i32) -> i32 { return a + b; }\n\
             fn main() -> i32 { return add(2, 3); }",
        );
        assert!(
            ir.contains("i32 @add("),
            "primitive return must keep value form, got:\n{ir}"
        );
        assert!(
            !ir.contains("sret"),
            "no sret on primitive-return fn, got:\n{ir}"
        );
    }

    #[test]
    fn string_sret_with_args_shifts_param_indices() {
        // With sret, the user-declared params live at %1, %2, ... rather
        // than %0, %1, .... Verify the body references the shifted SSA.
        let ir = gen_src(
            "fn pick(n: i32) -> string { return \"x\".to_string(); }\n\
             fn main() -> i32 { let s: string = pick(7); return 0; }",
        );
        // Definition: %0 is sret, %1 is `n`.
        assert!(
            ir.contains("void @pick(ptr sret") && ir.contains(", i32 noundef %1)"),
            "expected sret-then-i32 param indices, got:\n{ir}"
        );
    }

    #[test]
    fn musttail_with_sret_forwards_caller_slot() {
        // Caller's sret slot can be forwarded to callee on a tail call.
        // `caller() -> string` returning `helper()` (both sret) should
        // not need an intermediate slot+load — forward the caller's slot.
        let ir = gen_src(
            "fn helper() -> string { return \"hi\".to_string(); }\n\
             fn caller() -> string { return helper(); }\n\
             fn main() -> i32 { let s: string = caller(); return 0; }",
        );
        // Caller's body must musttail-call helper using its own sret slot.
        // v0.0.4 Phase 1A: the call-site sret attribute must match the
        // callee's declaration or LLVM's musttail verifier rejects the IR.
        let c_start = ir.find("void @caller(").expect("caller emitted");
        let c_end = ir[c_start..].find("\n}\n").expect("caller close");
        let c_body = &ir[c_start..c_start + c_end];
        // v0.0.8 fix C: non-pub helper → fastcc; the musttail call site
        // must mirror it.
        assert!(
            c_body.contains("musttail call fastcc void @helper(ptr sret(")
                && c_body
                    .contains(") noalias nonnull noundef writable dereferenceable(24) align 8 %0)"),
            "expected musttail call forwarding caller's sret slot with sret attrs, got:\n{c_body}"
        );
    }

    // ---- Phase 3A: bitwise + shift + byte-swap ----

    #[test]
    fn bitand_emits_llvm_and() {
        let ir = gen_src("fn main() -> i32 { return 0xff & 0x0f; }");
        assert!(ir.contains(" = and i32 "), "expected `and i32`, got:\n{ir}");
    }

    #[test]
    fn bitor_emits_llvm_or() {
        let ir = gen_src("fn main() -> i32 { return 0xff | 0x0f; }");
        assert!(ir.contains(" = or i32 "), "expected `or i32`, got:\n{ir}");
    }

    #[test]
    fn bitxor_emits_llvm_xor() {
        let ir = gen_src("fn main() -> i32 { return 0xff ^ 0x0f; }");
        assert!(ir.contains(" = xor i32 "), "expected `xor i32`, got:\n{ir}");
    }

    #[test]
    fn bit_not_emits_xor_minus_one() {
        // Phase 3A: `~x` lowers to `xor x, -1` per LLVM idiom.
        let ir = gen_src("fn main() -> i32 { let x: i32 = 5; return ~x; }");
        assert!(
            ir.contains("xor i32 ") && ir.contains(", -1"),
            "expected `xor i32 ..., -1`, got:\n{ir}"
        );
    }

    #[test]
    fn shl_emits_llvm_shl() {
        let ir = gen_src("fn main() -> i32 { return 1 << 3; }");
        assert!(ir.contains(" = shl i32 "), "expected `shl i32`, got:\n{ir}");
    }

    #[test]
    fn signed_shr_emits_arithmetic_shift() {
        // i32 is signed → `ashr` (preserves sign bit).
        let ir = gen_src("fn main() -> i32 { let x: i32 = -8; return x >> 2; }");
        assert!(
            ir.contains(" = ashr i32 "),
            "expected `ashr` for signed shift right, got:\n{ir}"
        );
    }

    #[test]
    fn unsigned_shr_emits_logical_shift() {
        // u32 is unsigned → `lshr` (zero-fill).
        let ir = gen_src(
            "fn main() -> i32 { let x: u32 = 8 as u32; let y: u32 = x >> (2 as u32); return 0; }",
        );
        assert!(
            ir.contains(" = lshr i32 "),
            "expected `lshr` for unsigned shift right, got:\n{ir}"
        );
    }

    #[test]
    fn shift_count_different_width_gets_coerced() {
        // `i64 << u8` — RHS gets zext from i8 to i64 before the shift.
        let ir = gen_src(
            "fn main() -> i32 {\n\
               let x: i64 = 1 as i64;\n\
               let n: u8 = 3 as u8;\n\
               let y: i64 = x << n;\n\
               return 0;\n\
             }",
        );
        // zext from i8 to i64 of the count, followed by shl i64.
        assert!(
            ir.contains(" = zext i8 ") && ir.contains(" to i64"),
            "expected zext i8 -> i64, got:\n{ir}"
        );
        assert!(ir.contains(" = shl i64 "), "expected `shl i64`, got:\n{ir}");
    }

    #[test]
    fn bswap16_emits_intrinsic_call() {
        let ir = gen_src(
            "fn main() -> i32 { let p: u16 = 0x1234 as u16; let q: u16 = #bswap16(p); return 0; }",
        );
        assert!(
            ir.contains("call i16 @llvm.bswap.i16(i16 "),
            "expected llvm.bswap.i16 call, got:\n{ir}"
        );
    }

    #[test]
    fn bswap32_emits_intrinsic_call() {
        let ir = gen_src("fn main() -> i32 { let p: u32 = 0x12345678 as u32; let q: u32 = #bswap32(p); return 0; }");
        assert!(
            ir.contains("call i32 @llvm.bswap.i32(i32 "),
            "expected llvm.bswap.i32 call, got:\n{ir}"
        );
    }

    #[test]
    fn bswap64_emits_intrinsic_call() {
        let ir = gen_src(
            "fn main() -> i32 { let p: u64 = 1 as u64; let q: u64 = #bswap64(p); return 0; }",
        );
        assert!(
            ir.contains("call i64 @llvm.bswap.i64(i64 "),
            "expected llvm.bswap.i64 call, got:\n{ir}"
        );
    }

    #[test]
    fn htons_aliases_bswap16() {
        // htons/htonl/ntohs/ntohl are aliases that lower to bswap on LE.
        let ir = gen_src(
            "fn main() -> i32 { let p: u16 = 8080 as u16; let q: u16 = #htons(p); return 0; }",
        );
        assert!(
            ir.contains("call i16 @llvm.bswap.i16(i16 "),
            "expected htons to lower to llvm.bswap.i16, got:\n{ir}"
        );
    }

    #[test]
    fn htonl_aliases_bswap32() {
        let ir =
            gen_src("fn main() -> i32 { let p: u32 = 1 as u32; let q: u32 = #htonl(p); return 0; }");
        assert!(
            ir.contains("call i32 @llvm.bswap.i32(i32 "),
            "expected htonl to lower to llvm.bswap.i32, got:\n{ir}"
        );
    }

    #[test]
    fn preamble_declares_bswap_intrinsics() {
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(ir.contains("declare i16 @llvm.bswap.i16(i16)"));
        assert!(ir.contains("declare i32 @llvm.bswap.i32(i32)"));
        assert!(ir.contains("declare i64 @llvm.bswap.i64(i64)"));
    }

    // ---- v0.0.3 Phase 5 Slice 5A: atomic intrinsics ----

    const ATOMIC_PRELUDE: &str = "extern fn malloc(n: usize) -> *u8;\n";

    fn atomic_test_src(body: &str) -> String {
        format!("{ATOMIC_PRELUDE}fn main() -> i32 {{ unsafe {{ {body} }} return 0; }}")
    }

    #[test]
    fn atomic_load_emits_load_atomic_seqcst() {
        let ir = gen_src(&atomic_test_src(
            "let p: *i32 = malloc(4 as usize) as *i32; let _v: i32 = __cplus_atomic_load_i32_seqcst(p);",
        ));
        assert!(
            ir.contains("load atomic i32, ptr"),
            "expected load atomic, got:\n{ir}"
        );
        assert!(
            ir.contains("seq_cst, align 4"),
            "expected seq_cst align 4, got:\n{ir}"
        );
    }

    #[test]
    fn atomic_store_emits_store_atomic_release() {
        let ir = gen_src(&atomic_test_src(
            "let p: *i64 = malloc(8 as usize) as *i64; __cplus_atomic_store_i64_release(p, 42 as i64);",
        ));
        assert!(
            ir.contains("store atomic i64"),
            "expected store atomic, got:\n{ir}"
        );
        assert!(
            ir.contains("release, align 8"),
            "expected release align 8, got:\n{ir}"
        );
    }

    #[test]
    fn atomic_fetch_add_emits_atomicrmw_add() {
        let ir = gen_src(&atomic_test_src(
            "let p: *u64 = malloc(8 as usize) as *u64; let _r: u64 = __cplus_atomic_fetch_add_u64_seqcst(p, 1 as u64);",
        ));
        assert!(
            ir.contains("atomicrmw add ptr"),
            "expected atomicrmw add, got:\n{ir}"
        );
        assert!(
            ir.contains("seq_cst"),
            "expected seq_cst ordering, got:\n{ir}"
        );
    }

    #[test]
    fn atomic_fetch_or_relaxed_uses_monotonic_keyword() {
        let ir = gen_src(&atomic_test_src(
            "let p: *u32 = malloc(4 as usize) as *u32; let _r: u32 = __cplus_atomic_fetch_or_u32_relaxed(p, 1 as u32);",
        ));
        assert!(
            ir.contains("atomicrmw or ptr"),
            "expected atomicrmw or, got:\n{ir}"
        );
        assert!(
            ir.contains("monotonic"),
            "relaxed should lower to monotonic, got:\n{ir}"
        );
    }

    #[test]
    fn atomic_xchg_emits_atomicrmw_xchg() {
        let ir = gen_src(&atomic_test_src(
            "let p: *i32 = malloc(4 as usize) as *i32; let _r: i32 = __cplus_atomic_xchg_i32_acquire(p, 5 as i32);",
        ));
        assert!(
            ir.contains("atomicrmw xchg ptr"),
            "expected atomicrmw xchg, got:\n{ir}"
        );
        assert!(ir.contains("acquire"));
    }

    #[test]
    fn atomic_cmpxchg_emits_cmpxchg_and_extracts_prev() {
        let ir = gen_src(&atomic_test_src(
            "let p: *i32 = malloc(4 as usize) as *i32; let _r: i32 = __cplus_atomic_cmpxchg_i32_seqcst(p, 0 as i32, 1 as i32);",
        ));
        assert!(ir.contains("cmpxchg ptr"), "expected cmpxchg, got:\n{ir}");
        assert!(
            ir.contains("seq_cst seq_cst"),
            "expected success+failure seq_cst, got:\n{ir}"
        );
        assert!(
            ir.contains("extractvalue { i32, i1 }"),
            "expected previous-value extract, got:\n{ir}"
        );
    }

    #[test]
    fn atomic_cmpxchg_release_uses_monotonic_failure_ord() {
        // Failure ordering must not be stronger than success and cannot be
        // release/acq_rel; release success → monotonic failure.
        let ir = gen_src(&atomic_test_src(
            "let p: *i32 = malloc(4 as usize) as *i32; let _r: i32 = __cplus_atomic_cmpxchg_i32_release(p, 0 as i32, 1 as i32);",
        ));
        assert!(
            ir.contains("release monotonic"),
            "expected release+monotonic, got:\n{ir}"
        );
    }

    #[test]
    fn atomic_load_outside_unsafe_is_rejected() {
        let src = "extern fn malloc(n: usize) -> *u8; fn main() -> i32 { let p: *i32 = unsafe { malloc(4 as usize) as *i32 }; let _v: i32 = __cplus_atomic_load_i32_seqcst(p); return 0; }";
        let toks = crate::lexer::tokenize(src).expect("lex");
        let prog = crate::parser::parse(toks).expect("parse");
        let diags = crate::sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(
            diags.iter().any(|d| d.code.0 == "E0801"),
            "expected E0801 (unsafe required), got:\n{:?}",
            diags
        );
    }

    #[test]
    fn atomic_load_wrong_ptr_type_is_rejected() {
        let src = "extern fn malloc(n: usize) -> *u8; fn main() -> i32 { unsafe { let p: *i32 = malloc(4 as usize) as *i32; let _v: i64 = __cplus_atomic_load_i64_seqcst(p); } return 0; }";
        let toks = crate::lexer::tokenize(src).expect("lex");
        let prog = crate::parser::parse(toks).expect("parse");
        let diags = crate::sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(
            diags.iter().any(|d| d.code.0 == "E0302"),
            "expected E0302 (type mismatch on *T), got:\n{:?}",
            diags
        );
    }

    // ---- v0.0.3 Phase 5 Slice 5B: thread::spawn + JoinHandle::join ----

    const THREAD_PRELUDE: &str =
        "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } fn worker() -> i64 { return 7 as i64; } ";

    #[test]
    fn thread_spawn_emits_pthread_create_and_trampoline() {
        let src = format!(
            "{THREAD_PRELUDE}fn main() -> i32 {{ \
             let h: JoinHandle[i64] = unsafe {{ __cplus_thread_spawn::[i64](worker) }}; \
             return 0; \
             }}"
        );
        let ir = gen_src_mono(&src);
        assert!(
            ir.contains("call i32 @pthread_create(ptr "),
            "expected pthread_create call, got:\n{ir}"
        );
        assert!(
            ir.contains("@__cplus_thread_tramp_i64"),
            "expected trampoline reference, got:\n{ir}"
        );
        assert!(
            ir.contains("define internal ptr @__cplus_thread_tramp_i64(ptr %arg)"),
            "expected i64 trampoline definition, got:\n{ir}"
        );
        assert!(
            ir.contains("call i64 %f()"),
            "trampoline must call the user's fn with the i64 return type, got:\n{ir}"
        );
    }

    #[test]
    fn thread_join_emits_pthread_join_load_free() {
        let src = format!(
            "{THREAD_PRELUDE}fn main() -> i32 {{ \
             let h: JoinHandle[i64] = unsafe {{ __cplus_thread_spawn::[i64](worker) }}; \
             let r: i64 = unsafe {{ __cplus_thread_join::[i64](h) }}; \
             return 0; \
             }}"
        );
        let ir = gen_src_mono(&src);
        assert!(
            ir.contains("call i32 @pthread_join(i64 "),
            "expected pthread_join call, got:\n{ir}"
        );
        assert!(
            ir.contains("getelementptr i8, ptr"),
            "expected GEP into ctx for result slot, got:\n{ir}"
        );
        assert!(
            ir.contains("load i64, ptr"),
            "expected result load, got:\n{ir}"
        );
        assert!(
            ir.contains("call void @free(ptr "),
            "expected free of ctx, got:\n{ir}"
        );
    }

    // Note: `()` as a turbofish type argument doesn't parse today
    // (the parser expects a type-name starting token). The Unit-returning
    // trampoline code path is exercised by the `gen_atomic_intrinsic`-like
    // code in `emit_thread_trampolines`; a regression test for the IR
    // shape would need parser support for `[()]`. Skipping for v0.0.3.

    #[test]
    fn thread_spawn_for_bool_uses_align_1() {
        let src = "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } \
                   fn flag() -> bool { return true; } \
                   fn main() -> i32 { \
                       let h: JoinHandle[bool] = unsafe { __cplus_thread_spawn::[bool](flag) }; \
                       return 0; \
                   }";
        let ir = gen_src_mono(src);
        let tramp_idx = ir.find("@__cplus_thread_tramp_bool(").unwrap();
        let tramp = &ir[tramp_idx..(tramp_idx + 400).min(ir.len())];
        assert!(
            tramp.contains("store i1 %r, ptr %slot, align 1"),
            "bool trampoline must store the i1 result with align 1, got:\n{tramp}"
        );
    }

    #[test]
    fn thread_spawn_outside_unsafe_is_rejected() {
        let src = format!(
            "{THREAD_PRELUDE}fn main() -> i32 {{ \
             let h: JoinHandle[i64] = __cplus_thread_spawn::[i64](worker); \
             return 0; \
             }}"
        );
        let toks = crate::lexer::tokenize(&src).expect("lex");
        let prog = crate::parser::parse(toks).expect("parse");
        let diags = crate::sema::check(&prog, PathBuf::from("test.cplus"), &src);
        assert!(
            diags.iter().any(|d| d.code.0 == "E0801"),
            "expected E0801 (unsafe required), got:\n{:?}",
            diags
        );
    }

    #[test]
    fn thread_join_outside_unsafe_is_rejected() {
        let src = format!(
            "{THREAD_PRELUDE}fn main() -> i32 {{ \
             let h: JoinHandle[i64] = unsafe {{ __cplus_thread_spawn::[i64](worker) }}; \
             let r: i64 = __cplus_thread_join::[i64](h); \
             return 0; \
             }}"
        );
        let toks = crate::lexer::tokenize(&src).expect("lex");
        let prog = crate::parser::parse(toks).expect("parse");
        let diags = crate::sema::check(&prog, PathBuf::from("test.cplus"), &src);
        assert!(
            diags.iter().any(|d| d.code.0 == "E0801"),
            "expected E0801 (unsafe required), got:\n{:?}",
            diags
        );
    }

    #[test]
    fn thread_spawn_without_turbofish_is_rejected() {
        let src = format!(
            "{THREAD_PRELUDE}fn main() -> i32 {{ \
             let h: JoinHandle[i64] = unsafe {{ __cplus_thread_spawn(worker) }}; \
             return 0; \
             }}"
        );
        let toks = crate::lexer::tokenize(&src).expect("lex");
        let prog = crate::parser::parse(toks).expect("parse");
        let diags = crate::sema::check(&prog, PathBuf::from("test.cplus"), &src);
        assert!(
            diags.iter().any(|d| d.code.0 == "E0501"),
            "expected E0501 (missing turbofish type arg), got:\n{:?}",
            diags
        );
    }

    #[test]
    fn thread_pthread_create_declared_in_preamble() {
        let ir = gen_src("fn main() -> i32 { return 0; }");
        assert!(ir.contains("declare i32 @pthread_create(ptr, ptr, ptr, ptr)"));
        assert!(ir.contains("declare i32 @pthread_join(i64, ptr)"));
    }

    // ---- v0.0.3 Phase 5 Slice 5C: thread::spawn_with[I, O] ----

    #[test]
    fn thread_spawn_with_emits_pthread_create_and_indexed_trampoline() {
        let src = "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } \
                   fn double(x: i32) -> i32 { return x +% x; } \
                   fn main() -> i32 { \
                       let h: JoinHandle[i32] = unsafe { __cplus_thread_spawn_with::[i32, i32](21 as i32, double) }; \
                       return 0; \
                   }";
        let ir = gen_src_mono(src);
        assert!(
            ir.contains("call i32 @pthread_create(ptr "),
            "expected pthread_create call, got:\n{ir}"
        );
        assert!(
            ir.contains("@__cplus_thread_tramp_with_0"),
            "expected spawn_with trampoline reference, got:\n{ir}"
        );
        assert!(
            ir.contains("define internal ptr @__cplus_thread_tramp_with_0(ptr %arg)"),
            "expected spawn_with trampoline definition, got:\n{ir}"
        );
        assert!(
            ir.contains("call i32 %f(i32 %i)"),
            "trampoline must call f(<i32 input>), got:\n{ir}"
        );
    }

    #[test]
    fn thread_spawn_with_input_stored_after_result_slot() {
        // v0.0.4 Phase 2 Slice 2H ctx layout:
        //   refcount:    @ 0  (u64, 8 bytes)
        //   fn_ptr:      @ 8
        //   result_slot: @ 16 (i32, 4 bytes — slot ends at 20)
        //   input_slot:  @ 20 (i32, aligned)
        let src = "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } \
                   fn double(x: i32) -> i32 { return x +% x; } \
                   fn main() -> i32 { \
                       let h: JoinHandle[i32] = unsafe { __cplus_thread_spawn_with::[i32, i32](7 as i32, double) }; \
                       return 0; \
                   }";
        let ir = gen_src_mono(src);
        let tramp_idx = ir.find("@__cplus_thread_tramp_with_0(").unwrap();
        let tramp = &ir[tramp_idx..(tramp_idx + 500).min(ir.len())];
        assert!(
            tramp.contains("getelementptr i8, ptr %arg, i64 20"),
            "expected input slot at offset 20, got:\n{tramp}"
        );
    }

    #[test]
    fn thread_spawn_with_for_i64_input_lives_at_offset_16() {
        // i64 result is 8 bytes; result slot at offset 8..16. i64
        // input aligned to 8 = offset 16.
        let src = "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } \
                   fn negate(x: i64) -> i64 { return (0 as i64) -% x; } \
                   fn main() -> i32 { \
                       let h: JoinHandle[i64] = unsafe { __cplus_thread_spawn_with::[i64, i64](5 as i64, negate) }; \
                       return 0; \
                   }";
        let ir = gen_src_mono(src);
        let tramp_idx = ir.find("@__cplus_thread_tramp_with_0(").unwrap();
        let tramp = &ir[tramp_idx..(tramp_idx + 500).min(ir.len())];
        assert!(
            tramp.contains("getelementptr i8, ptr %arg, i64 16"),
            "expected input slot at offset 16, got:\n{tramp}"
        );
        assert!(
            tramp.contains("call i64 %f(i64 %i)"),
            "trampoline must call f(i64 i), got:\n{tramp}"
        );
    }

    #[test]
    fn thread_spawn_with_distinct_io_pairs_get_distinct_trampolines() {
        let src = "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } \
                   fn d32(x: i32) -> i32 { return x; } \
                   fn d64(x: i64) -> i64 { return x; } \
                   fn main() -> i32 { \
                       let h1: JoinHandle[i32] = unsafe { __cplus_thread_spawn_with::[i32, i32](1 as i32, d32) }; \
                       let h2: JoinHandle[i64] = unsafe { __cplus_thread_spawn_with::[i64, i64](2 as i64, d64) }; \
                       return 0; \
                   }";
        let ir = gen_src_mono(src);
        assert!(
            ir.contains("@__cplus_thread_tramp_with_0"),
            "expected first trampoline, got:\n{ir}"
        );
        assert!(
            ir.contains("@__cplus_thread_tramp_with_1"),
            "expected second trampoline (distinct (I, O) pair), got:\n{ir}"
        );
    }

    #[test]
    fn thread_spawn_with_outside_unsafe_is_rejected() {
        let src = "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } \
                   fn double(x: i32) -> i32 { return x +% x; } \
                   fn main() -> i32 { \
                       let h: JoinHandle[i32] = __cplus_thread_spawn_with::[i32, i32](21 as i32, double); \
                       return 0; \
                   }";
        let toks = crate::lexer::tokenize(src).expect("lex");
        let prog = crate::parser::parse(toks).expect("parse");
        let diags = crate::sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(
            diags.iter().any(|d| d.code.0 == "E0801"),
            "expected E0801 (unsafe required), got:\n{:?}",
            diags
        );
    }

    #[test]
    fn thread_spawn_with_wrong_type_arg_count_is_rejected() {
        let src = "pub struct JoinHandle[O] { tid: u64, opaque ctx: *u8 } \
                   fn double(x: i32) -> i32 { return x +% x; } \
                   fn main() -> i32 { \
                       let h: JoinHandle[i32] = unsafe { __cplus_thread_spawn_with::[i32](21 as i32, double) }; \
                       return 0; \
                   }";
        let toks = crate::lexer::tokenize(src).expect("lex");
        let prog = crate::parser::parse(toks).expect("parse");
        let diags = crate::sema::check(&prog, PathBuf::from("test.cplus"), src);
        assert!(
            diags.iter().any(|d| d.code.0 == "E0501"),
            "expected E0501 (2 type args required), got:\n{:?}",
            diags
        );
    }

    // ---- Phase 5 Slice 5.D: C-ABI aggregate coercion ----

    #[test]
    fn classify_c_abi_scalars_pass_direct() {
        let t = TypeTable::default();
        assert_eq!(classify_c_abi(&Ty::I32, &t), CAbiClass::Direct);
        assert_eq!(classify_c_abi(&Ty::U64, &t), CAbiClass::Direct);
        assert_eq!(classify_c_abi(&Ty::Bool, &t), CAbiClass::Direct);
        assert_eq!(classify_c_abi(&Ty::F32, &t), CAbiClass::Direct);
        assert_eq!(
            classify_c_abi(&Ty::RawPtr(Box::new(Ty::U8)), &t),
            CAbiClass::Direct
        );
    }

    #[test]
    fn classify_c_abi_small_struct_coerces_to_i64() {
        // `#[repr(C)] struct Point { x: i32, y: i32 }` is 8 bytes.
        let src = "#[repr(C)] struct Point { x: i32, y: i32 }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["Point"];
        let abi = classify_c_abi(&Ty::Struct(id), &types);
        match abi {
            CAbiClass::Coerce {
                llvm_ty,
                size,
                align,
            } => {
                assert_eq!(llvm_ty, "i64");
                assert_eq!(size, 8);
                assert_eq!(align, 8);
            }
            _ => panic!("expected Coerce(i64), got {abi:?}"),
        }
    }

    #[test]
    fn classify_c_abi_mid_struct_coerces_to_array_i64() {
        let src = "#[repr(C)] struct Pair { a: i64, b: i64 }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["Pair"];
        // A 16-byte two-eightbyte aggregate coerces to the target's
        // register-pair type: `[2 x i64]` on aarch64-darwin, `{ i64, i64 }`
        // on x86_64-sysv (see the `cfg!(target_arch)` split in classify_c_abi).
        // On Windows (Microsoft x64) a 16-byte aggregate is passed indirectly.
        if cfg!(all(target_arch = "x86_64", windows)) {
            assert_eq!(classify_c_abi(&Ty::Struct(id), &types), CAbiClass::Indirect);
            return;
        }
        let expected = if cfg!(target_arch = "x86_64") {
            "{ i64, i64 }"
        } else {
            "[2 x i64]"
        };
        match classify_c_abi(&Ty::Struct(id), &types) {
            CAbiClass::Coerce { llvm_ty, size, .. } => {
                assert_eq!(llvm_ty, expected);
                assert_eq!(size, 16);
            }
            other => panic!("expected Coerce({expected}), got {other:?}"),
        }
    }

    #[test]
    fn classify_c_abi_large_struct_passes_indirect() {
        let src = "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
                   fn main() -> i32 { return 0; }";
        let toks = tokenize(src).unwrap();
        let prog = parse(toks).unwrap();
        let diags = sema::check(&prog, PathBuf::from("t.cplus"), src);
        assert!(diags.is_empty());
        let types = collect_types(&prog);
        let id = types.struct_by_name["Triple"];
        assert_eq!(classify_c_abi(&Ty::Struct(id), &types), CAbiClass::Indirect);
    }

    #[test]
    fn pub_extern_fn_with_small_struct_param_coerces_to_i64() {
        // Codegen-level: the LLVM signature must use `i64` for the
        // 8-byte `Point` param (matching clang's aarch64-darwin output).
        let ir = gen_src(
            "#[repr(C)] struct Point { x: i32, y: i32 }\n\
             pub extern fn square(p: Point) -> i32 { return p.x * p.x + p.y * p.y; }",
        );
        // Look for the @square define with coerced i64 param.
        assert!(
            ir.contains("i32 @square(i64"),
            "expected `define i32 @square(i64 ...)`, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_small_struct_return_coerces_to_i64() {
        let ir = gen_src(
            "#[repr(C)] struct Point { x: i32, y: i32 }\n\
             pub extern fn make(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }",
        );
        // 8-byte struct return: `define i64 @make(...)` with packed coerce on ret.
        assert!(
            ir.contains("define i64 @make("),
            "expected `define i64 @make(...)`, got:\n{ir}"
        );
        // The Return statement should stage through alloca + load-as-i64.
        assert!(
            ir.contains("load i64, ptr") && ir.contains("ret i64"),
            "expected coerce-on-return path emitted, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_mid_struct_param_coerces_to_array_i64() {
        let ir = gen_src(
            "#[repr(C)] struct Pair { a: i64, b: i64 }\n\
             pub extern fn sum(p: Pair) -> i64 { return p.a + p.b; }",
        );
        // 16-byte two-eightbyte param coerces to the target's register-pair
        // type: `[2 x i64]` on aarch64-darwin, `{ i64, i64 }` on x86_64-sysv.
        // On Windows (Microsoft x64) it is passed indirectly (`ptr` byval).
        let expected = if cfg!(all(target_arch = "x86_64", windows)) {
            "define i64 @sum(ptr"
        } else if cfg!(target_arch = "x86_64") {
            "define i64 @sum({ i64, i64 }"
        } else {
            "define i64 @sum([2 x i64]"
        };
        assert!(
            ir.contains(expected),
            "expected `{expected} ...)`, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_large_struct_param_passes_indirect() {
        let ir = gen_src(
            "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
             pub extern fn sum(t: Triple) -> i64 { return t.a + t.b + t.c; }",
        );
        // >16 byte struct param: bare `ptr` (no byval on aarch64-darwin).
        assert!(
            ir.contains("define i64 @sum(ptr"),
            "expected `define i64 @sum(ptr ...)`, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_large_struct_return_uses_sret() {
        // >16-byte aggregate returns go through Slice 1D's sret path —
        // generalized in 5.D from `Ty::String` only to any Indirect class.
        let ir = gen_src(
            "#[repr(C)] struct Triple { a: i64, b: i64, c: i64 }\n\
             pub extern fn make() -> Triple { return Triple { a: 1 as i64, b: 2 as i64, c: 3 as i64 }; }"
        );
        assert!(
            ir.contains("void @make(ptr sret("),
            "expected sret-form return for >16 byte struct, got:\n{ir}"
        );
        assert!(
            ir.contains("ret void"),
            "expected `ret void` for sret path, got:\n{ir}"
        );
    }

    #[test]
    fn non_extern_fn_unaffected_by_5d() {
        // Regression guard: 5.D coercion fires ONLY on `pub extern fn`.
        // A regular C+ fn `fn use_p(p: Point) -> i32` keeps the C+ ABI
        // (LLVM first-class aggregate, `%Point %0`).
        let ir = gen_src(
            "#[repr(C)] struct Point { x: i32, y: i32 }\n\
             fn use_p(p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { let q: Point = Point { x: 1, y: 2 }; return use_p(q); }",
        );
        // The non-extern path keeps `%Point %0` (Copy struct, by-value).
        assert!(
            ir.contains("i32 @use_p(%Point"),
            "non-extern fn must keep C+ ABI, got:\n{ir}"
        );
    }

    #[test]
    fn pub_extern_fn_with_scalar_args_no_coercion() {
        // Pure-scalar signatures don't trigger any coercion machinery —
        // the C ABI and C+ ABI agree on scalar passing.
        let ir = gen_src("pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }");
        assert!(
            ir.contains("i32 @add(i32 noundef") && !ir.contains("@add(i64"),
            "scalar-only export must not coerce, got:\n{ir}"
        );
    }

    #[test]
    fn existing_substring_checks_still_match() {
        // Backward-compat: pre-1A tests assert on substrings like
        // `define void @bump(ptr noalias ` — confirm those still hold after
        // the attr set widened (the noalias prefix is still left-anchored).
        let ir = gen_src(
            "struct Tag { v: i32 }\n\
             impl Tag { fn drop(mut self) { return; } }\n\
             fn bump(mut t: Tag) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: Tag = Tag { v: 1 }; bump(x); return x.v; }",
        );
        assert!(ir.contains("void @bump(ptr noalias "));
    }

    // ---- Phase v0.0.2 Slice 1C: scoped !alias.scope / !noalias ----
    //
    // For every pointer-passed `mut`/`move` param (the ones already
    // carrying `noalias` from Slice 1A), publish a unique scope inside a
    // per-function domain. Loads/stores derived from each param carry
    // `!alias.scope` of their own scope and `!noalias` of all other
    // function-local scopes. Survives -O2 inlining where `noalias` would
    // be lost.

    #[test]
    fn two_mut_params_get_distinct_alias_scopes() {
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T { fn drop(mut self) { return; } }\n\
             fn swap_bump(mut a: T, mut b: T) {\n\
               let tmp: i32 = a.v;\n\
               a.v = b.v;\n\
               b.v = tmp;\n\
               return;\n\
             }\n\
             fn main() -> i32 {\n\
               let mut x: T = T { v: 1 };\n\
               let mut y: T = T { v: 2 };\n\
               swap_bump(x, y);\n\
               return x.v + y.v;\n\
             }",
        );
        // One domain, two scopes for the function. Match by label
        // rather than literal node IDs — IDs shift when other
        // module-level metadata (TBAA, range, etc.) is allocated
        // earlier in the pass.
        assert!(
            ir.lines()
                .any(|l| l.starts_with("!") && l.contains("distinct") && l.contains("!\"swap_bump\"")),
            "expected swap_bump domain definition, got:\n{ir}"
        );
        assert!(
            ir.contains("!\"p0\"}") && ir.contains("!\"p1\"}"),
            "expected p0 and p1 scopes for the params, got:\n{ir}"
        );
        // Loads/stores through both params carry alias.scope + noalias.
        let scope_lines = ir
            .lines()
            .filter(|l| l.contains("!alias.scope") && l.contains("!noalias"))
            .count();
        assert!(scope_lines >= 4,
            "expected at least 4 annotated load/store lines (2 loads + 2 stores), got {scope_lines}:\n{ir}");
    }

    #[test]
    fn single_mut_param_no_alias_scope() {
        // With only one noalias-capable param, there's nothing to be
        // disjoint *from*; emitting alias.scope wastes IR space without
        // a payoff. Confirm the optimization is gated on count >= 2.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T { fn drop(mut self) { return; } }\n\
             fn bump(mut t: T) { t.v = t.v + 1; return; }\n\
             fn main() -> i32 { let mut x: T = T { v: 1 }; bump(x); return x.v; }",
        );
        assert!(
            !ir.contains("!alias.scope"),
            "single mut param shouldn't trigger alias.scope, got:\n{ir}"
        );
    }

    #[test]
    fn non_copy_locals_get_alias_scope() {
        // v0.0.3 Slice 3C: two `let mut` non-Copy locals in a function
        // produce two `!alias.scope` annotations on their loads/stores.
        // Domain is per-function, scopes are `l0`/`l1` (locals only,
        // no noalias params in main).
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T { fn drop(mut self) { return; } }\n\
             fn main() -> i32 {\n\
               let mut x: T = T { v: 1 };\n\
               let mut y: T = T { v: 2 };\n\
               x.v = 3;\n\
               y.v = 4;\n\
               return x.v + y.v;\n\
             }",
        );
        assert!(
            ir.contains("!alias.scope"),
            "non-Copy locals should publish alias.scope metadata, got:\n{ir}"
        );
    }

    #[test]
    fn shared_params_do_not_participate_in_alias_scope() {
        // Two `t: T` shared params get `readonly`, not `noalias`. The
        // borrow checker doesn't prove they're disjoint, so the *sum*
        // function itself doesn't publish alias.scope on its params.
        // (Locals + mut/move params still get scopes per Slice 1C/3C —
        // we scope this test to the sum function's IR.)
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T { fn drop(mut self) { return; } }\n\
             fn sum(a: T, b: T) -> i32 { return a.v + b.v; }\n\
             fn main() -> i32 {\n\
               let x: T = T { v: 1 };\n\
               let y: T = T { v: 2 };\n\
               return sum(x, y);\n\
             }",
        );
        // Pull out @sum's body and verify no alias.scope appears INSIDE it.
        let sum_start = ir.find("@sum(").expect("@sum defined");
        let body = &ir[sum_start..];
        let sum_end = body
            .find("\n}\n")
            .map(|i| sum_start + i + 2)
            .unwrap_or(ir.len());
        let sum_body = &ir[sum_start..sum_end];
        assert!(
            !sum_body.contains("!alias.scope"),
            "two readonly shared params must not trigger alias.scope in their function, got:\n{sum_body}"
        );
    }

    #[test]
    fn method_mut_self_plus_mut_param_get_scopes() {
        // `mut self` (Receiver::Mut → noalias-shaped, idx 0) and a
        // non-Copy mut param both participate.
        let ir = gen_src(
            "struct T { v: i32 }\n\
             impl T {\n\
               fn drop(mut self) { return; }\n\
               fn merge(mut self, mut other: T) {\n\
                 self.v = self.v + other.v;\n\
                 other.v = 0;\n\
                 return;\n\
               }\n\
             }\n\
             fn main() -> i32 {\n\
               let mut a: T = T { v: 1 };\n\
               let mut b: T = T { v: 2 };\n\
               a.merge(b);\n\
               return a.v;\n\
             }",
        );
        // Method-mangled domain.
        assert!(
            ir.contains("distinct !{") && ir.contains("\"T.merge\"}"),
            "expected T.merge domain, got:\n{ir}"
        );
        // Loads/stores annotated.
        let scope_lines = ir
            .lines()
            .filter(|l| l.contains("!alias.scope") && l.contains("!noalias"))
            .count();
        assert!(
            scope_lines >= 2,
            "expected at least 2 annotated load/store lines in T.merge, got {scope_lines}:\n{ir}"
        );
    }

    #[test]
    fn alias_scope_propagates_through_gep_chain() {
        // gen_field GEPs off the param's slot. The post-pass dataflow
        // should propagate the scope through the GEP so the eventual
        // load carries it.
        let ir = gen_src(
            "struct Inner { n: i32 }\n\
             struct Outer { inner: Inner, tag: i32 }\n\
             impl Outer { fn drop(mut self) { return; } }\n\
             fn touch(mut a: Outer, mut b: Outer) -> i32 {\n\
               a.inner.n = b.tag;\n\
               return a.inner.n + b.tag;\n\
             }\n\
             fn main() -> i32 {\n\
               let mut x: Outer = Outer { inner: Inner { n: 0 }, tag: 7 };\n\
               let mut y: Outer = Outer { inner: Inner { n: 0 }, tag: 9 };\n\
               return touch(x, y);\n\
             }",
        );
        // Two-level GEP chain (Outer → Inner → n) — both loads/stores
        // through the chain should carry scope metadata.
        let touched = ir
            .lines()
            .filter(|l| l.contains("!alias.scope") && l.contains("!noalias"))
            .count();
        assert!(touched >= 2,
            "expected at least 2 scope-annotated loads/stores through GEP chains, got {touched}:\n{ir}");
    }

    // ---- v0.0.4 raytracer-port bug fixes (2026-05-17) ----

    #[test]
    fn let_struct_eq_if_expression_does_not_panic() {
        // Regression: `let r: V = if cond { a } else { b };` panicked
        // codegen with "let init produces a value" because `expr_value_ty`
        // didn't resolve Ident expressions to their binding type. Fixed
        // by `expr_value_ty_with_bindings` which consults the binding
        // table.
        let ir = gen_src(
            "struct V { x: i32, y: i32 }\n\
             fn main() -> i32 {\n\
                 let cond: bool = true;\n\
                 let a: V = V { x: 1, y: 2 };\n\
                 let b: V = V { x: 10, y: 20 };\n\
                 let r: V = if cond { a } else { b };\n\
                 return r.x;\n\
             }",
        );
        // The if-merge slot must exist and the result must be loaded back.
        assert!(
            ir.contains("alloca %V"),
            "expected V alloca for if-result slot:\n{ir}"
        );
    }

    #[test]
    fn let_str_eq_if_expression_does_not_panic() {
        // Regression: `let v: str = if cond { "a" } else { "b" };` panicked
        // codegen with "let init produces a value" because `expr_value_ty`
        // didn't handle string literals, so `gen_if` allocated no result slot
        // and returned None. (The struct case was already fixed; fat-pointer
        // `str`/`string` arms were the residual.) Fixed by adding StrLit/
        // InterpStr to `expr_value_ty`.
        let ir = gen_src(
            "fn main() -> i32 {\n\
                 let cond: bool = true;\n\
                 let v: str = if cond { \"aaa\" } else { \"bb\" };\n\
                 return #str_len(v) as i32;\n\
             }",
        );
        assert!(
            ir.contains("@main"),
            "expected codegen to complete without panic:\n{ir}"
        );
    }

    #[test]
    fn f32_literal_emits_hex_form() {
        // Regression: `0.1f32` emitted `float 0.1` which LLVM rejects
        // (not f32-exact). All f32 literals now emit hex form. The hex
        // is the f64 bit pattern of the f32-narrowed value.
        let ir = gen_src("fn main() -> i32 { let a: f32 = 0.1f32; return 0; }");
        assert!(
            ir.contains("float 0x"),
            "expected hex-form f32 literal:\n{ir}"
        );
        assert!(
            !ir.contains("float 0.1,"),
            "decimal-form f32 literal should be gone:\n{ir}"
        );
    }

    #[test]
    fn f64_literal_emits_hex_form() {
        // f64 literals also use hex form for round-trippable determinism.
        let ir = gen_src("fn main() -> i32 { let a: f64 = 1e-8; return 0; }");
        assert!(
            ir.contains("double 0x"),
            "expected hex-form f64 literal:\n{ir}"
        );
    }

    #[test]
    fn f32_literal_parses_directly_without_double_rounding() {
        // Regression: `0.4f32` previously parsed decimal → f64 → fptrunc-to-f32,
        // double-rounding to 0x3ECCCCCD via 0x3FD999999999999A. After fix:
        // decimal parses directly to f32 (0x3ECCCCCD is the IEEE-correct
        // round-to-nearest f32 of 0.4) and the IR carries its lossless f64
        // bit pattern 0x3FD99999A0000000 — what LLVM expects for `float 0x...`.
        let ir = gen_src("fn main() -> i32 { let _a: f32 = 0.4f32; return 0; }");
        // 0.4f32 → f32 bits 0x3ECCCCCD → lossless f64 widen → 0x3FD99999A0000000.
        assert!(
            ir.contains("float 0x3FD99999A0000000"),
            "expected float 0x3FD99999A0000000 (lossless f64 of 0.4f32):\n{ir}"
        );
        // 0.1f32 directly: f32 bits 0x3DCCCCCD → lossless f64 widen → 0x3FB99999A0000000.
        let ir2 = gen_src("fn main() -> i32 { let _a: f32 = 0.1f32; return 0; }");
        assert!(
            ir2.contains("float 0x3FB99999A0000000"),
            "expected float 0x3FB99999A0000000 (lossless f64 of 0.1f32):\n{ir2}"
        );
    }

    #[test]
    fn float_arith_emits_contract_fast_math_flag() {
        // Regression: cpc's IR didn't emit any fast-math flags, so LLVM at
        // -O2 couldn't fuse `fmul; fadd` into `fmadd` and the raytracer
        // benchmark ran ~50% slower than the C equivalent (which gets
        // `-ffp-contract=on` by default). Now every float arith carries
        // `contract`, recovering the missing fmadd codegen.
        //
        // Operands are deliberately split across multiple lets so the FMA
        // peephole (which fires on `(a*b) + c` shapes) doesn't lower them
        // to `llvm.fmuladd` — this test pins the plain-op path.
        let ir = gen_src("fn main() -> i32 { let a: f32 = 1.0f32; let b: f32 = 2.0f32; let c: f32 = 3.0f32; let _w: f32 = a + b; let _x: f32 = a - b; let _y: f32 = a * c; let _z: f32 = a / c; return 0; }");
        assert!(
            ir.contains(" = fmul contract float "),
            "expected fmul contract:\n{ir}"
        );
        assert!(
            ir.contains(" = fadd contract float "),
            "expected fadd contract:\n{ir}"
        );
        assert!(
            ir.contains(" = fsub contract float "),
            "expected fsub contract:\n{ir}"
        );
        assert!(
            ir.contains(" = fdiv contract float "),
            "expected fdiv contract:\n{ir}"
        );
    }

    #[test]
    fn fma_peephole_emits_fmuladd_for_mul_add_pattern() {
        // Regression: cpc's raytracer ran ~50% slower than C even with
        // `contract` flags because LLVM didn't always fuse fmul+fadd into
        // fmadd. clang's frontend lowers source-level `a*b+c` directly to
        // `llvm.fmuladd` at `-ffp-contract=on`; cpc now does the same in
        // gen_binary's FMA peephole.
        let ir = gen_src("fn dot(a: f32, b: f32, c: f32) -> f32 { return a * b + c; } fn main() -> i32 { return 0; }");
        assert!(
            ir.contains("call contract float @llvm.fmuladd.f32"),
            "expected fmuladd lowering for `a * b + c`:\n{ir}"
        );

        // `c + a * b` (mul on the right) — same intrinsic.
        let ir2 = gen_src("fn dot(a: f32, b: f32, c: f32) -> f32 { return c + a * b; } fn main() -> i32 { return 0; }");
        assert!(
            ir2.contains("call contract float @llvm.fmuladd.f32"),
            "expected fmuladd for `c + a*b`:\n{ir2}"
        );

        // `(a*b) - c` lowers via fmuladd(a, b, -c) — there should be one
        // fneg of c plus the fmuladd call.
        let ir3 = gen_src("fn dot(a: f32, b: f32, c: f32) -> f32 { return a * b - c; } fn main() -> i32 { return 0; }");
        assert!(
            ir3.contains("call contract float @llvm.fmuladd.f32"),
            "expected fmuladd for `a*b - c`:\n{ir3}"
        );
        assert!(
            ir3.contains(" = fneg contract float "),
            "expected fneg of c in `a*b - c`:\n{ir3}"
        );

        // f64 form selects the f64 intrinsic.
        let ir4 = gen_src("fn dot(a: f64, b: f64, c: f64) -> f64 { return a * b + c; } fn main() -> i32 { return 0; }");
        assert!(
            ir4.contains("call contract double @llvm.fmuladd.f64"),
            "expected f64 fmuladd:\n{ir4}"
        );
    }

    #[test]
    fn extract_ptr_operand_finds_address_arg() {
        // White-box: confirm the parser used by annotate_one_line picks
        // off the `, ptr %X` operand from load/store/GEP forms.
        assert_eq!(
            extract_ptr_operand("load i32, ptr %t2, align 4").as_deref(),
            Some("%t2"),
        );
        assert_eq!(
            extract_ptr_operand("getelementptr inbounds %T, ptr %0, i32 0, i32 0").as_deref(),
            Some("%0"),
        );
        assert_eq!(
            extract_ptr_operand("store i32 7, ptr %slot").as_deref(),
            Some("%slot"),
        );
        // No address operand → None.
        assert_eq!(extract_ptr_operand("add i32 1, 2"), None);
    }

    // ---- v0.0.9 Phase 3: mixed-if-arm panic regressions ----

    #[test]
    fn mixed_if_arm_with_field_tail_no_panic() {
        // Repro from Phase 3 — `let v = if cond { a.x } else { b.x }`
        // panicked in `expr_value_ty_with_bindings` because Field
        // wasn't covered (returned None → "let init produces a value").
        let ir = gen_src(
            "struct V3 { x: f32, y: f32, z: f32 } \
             fn main() -> i32 { \
                let cond: bool = true; \
                let a: V3 = V3 { x: 1.0f32, y: 2.0f32, z: 3.0f32 }; \
                let b: V3 = V3 { x: 9.0f32, y: 8.0f32, z: 7.0f32 }; \
                let x: f32 = if cond { a.x } else { b.x }; \
                return x as i32; \
             }",
        );
        // Smoke check that we got an if-merge phi (or equivalent
        // result-slot store path) without panicking the helper.
        assert!(
            ir.contains("phi float") || ir.contains("store float"),
            "expected if-merge result handling in IR; got:\n{ir}"
        );
    }

    #[test]
    fn mixed_if_arm_with_index_tail_no_panic() {
        let ir = gen_src(
            "fn main() -> i32 { \
                let cond: bool = true; \
                let arr: [i32; 4] = [10, 20, 30, 40]; \
                let v: i32 = if cond { arr[1] } else { arr[2] }; \
                return v; \
             }",
        );
        assert!(
            ir.contains("getelementptr"),
            "expected element-pointer GEP in IR; got:\n{ir}"
        );
    }

    #[test]
    fn mixed_if_arm_with_unsafe_block_tail_no_panic() {
        let ir = gen_src(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                let cond: bool = true; \
                let p: *u8 = if cond { \
                    unsafe { malloc(8 as usize) } \
                } else { \
                    unsafe { 0 as *u8 } \
                }; \
                let _addr: usize = unsafe { p as usize }; \
                return 0; \
             }",
        );
        assert!(
            ir.contains("call ptr @malloc"),
            "expected the unsafe-block tail's malloc call in IR; got:\n{ir}"
        );
    }

    #[test]
    fn mixed_if_arm_with_cast_tail_no_panic() {
        // Cast as tail expression — sema computes the result type from
        // the cast target, but the helper needed to know that too so
        // gen_if could pre-allocate the result slot.
        let ir = gen_src(
            "fn main() -> i32 { \
                let cond: bool = true; \
                let v: i64 = if cond { 5 as i64 } else { 10 as i64 }; \
                return v as i32; \
             }",
        );
        assert!(
            ir.contains("phi i64") || ir.contains("store i64"),
            "expected i64-result handling in IR; got:\n{ir}"
        );
    }

    #[test]
    fn mixed_if_arm_with_match_tail_no_panic() {
        // `match` as tail — used in chained-fallible-result code.
        let ir = gen_src(
            "enum Opt { Some(i32), None } \
             fn pick() -> Opt { return Opt::Some(7); } \
             fn main() -> i32 { \
                let cond: bool = true; \
                let v: i32 = if cond { \
                    match pick() { \
                        Opt::Some(n) => n, \
                        Opt::None    => 0, \
                    } \
                } else { \
                    42 \
                }; \
                return v; \
             }",
        );
        assert!(
            !ir.is_empty(),
            "expected non-empty IR (no panic); got:\n{ir}"
        );
    }

    // ---- v0.0.11 Phase 0: codegen for `#selector` / `#msg_send` / `#compile_shader` ----

    #[test]
    fn intrinsic_selector_emits_cached_globals_and_lazy_init() {
        let src = "fn main() -> i32 {\n\
                     let s: *u8 = #selector(\"length\");\n\
                     return 0;\n\
                 }";
        let ir = gen_src_mono(src);
        // The pre-pass should have emitted both the NUL-terminated data
        // global and the lazy-cached pointer slot.
        assert!(
            ir.contains("@__cplus.sel.0.data = private unnamed_addr constant [7 x i8] c\"length\\00\""),
            "missing selector data global; IR:\n{ir}"
        );
        assert!(
            ir.contains("@__cplus.sel.0.cached = private global ptr null"),
            "missing selector cached slot; IR:\n{ir}"
        );
        // Runtime declares emitted once, non-variadic objc_msgSend (the
        // aarch64-darwin ABI requirement — variadic and non-variadic
        // pass args via different storage).
        assert!(
            ir.contains("declare ptr @sel_registerName(ptr)"),
            "missing sel_registerName declare; IR:\n{ir}"
        );
        assert!(
            ir.contains("declare ptr @objc_msgSend(ptr, ptr)"),
            "missing objc_msgSend declare (must be non-variadic); IR:\n{ir}"
        );
        // The call site lazy-init pattern: load cached, branch on null,
        // sel_registerName + store on the slow path, re-load on the
        // merge to get the (now-populated) value.
        assert!(
            ir.contains("load ptr, ptr @__cplus.sel.0.cached"),
            "missing cached load; IR:\n{ir}"
        );
        assert!(
            ir.contains("call ptr @sel_registerName(ptr @__cplus.sel.0.data)"),
            "missing sel_registerName call site; IR:\n{ir}"
        );
        assert!(
            ir.contains("store ptr") && ir.contains("@__cplus.sel.0.cached"),
            "missing store-back into cached slot; IR:\n{ir}"
        );
    }

    #[test]
    fn intrinsic_selector_dedupes_repeated_names() {
        // Two `#selector("length")` calls in the same module share one
        // global pair (BTreeSet collapses the entry; codegen emits once).
        let src = "fn main() -> i32 {\n\
                     let s1: *u8 = #selector(\"length\");\n\
                     let s2: *u8 = #selector(\"length\");\n\
                     let s3: *u8 = #selector(\"alloc\");\n\
                     return 0;\n\
                 }";
        let ir = gen_src_mono(src);
        // Exactly two distinct data globals (one per unique name).
        assert!(
            ir.contains("@__cplus.sel.0.data"),
            "missing first selector global; IR:\n{ir}"
        );
        assert!(
            ir.contains("@__cplus.sel.1.data"),
            "missing second selector global; IR:\n{ir}"
        );
        assert!(
            !ir.contains("@__cplus.sel.2.data"),
            "should not have emitted a third global for the duplicate `length`; IR:\n{ir}"
        );
    }

    #[test]
    fn intrinsic_msg_send_typed_return() {
        let src = "fn main() -> i32 {\n\
                     unsafe {\n\
                         let obj: *u8 = 0 as *u8;\n\
                         let n: u64 = #msg_send(obj, \"length\") -> u64;\n\
                     }\n\
                     return 0;\n\
                 }";
        let ir = gen_src_mono(src);
        // Call uses the typed return (i64 — `u64` in C+ lowers to LLVM i64)
        // and a non-variadic call site. The `(ptr, ptr, ...)` shape would
        // be wrong on aarch64-darwin.
        assert!(
            ir.contains("call i64 @objc_msgSend("),
            "missing typed i64 msg_send call; IR:\n{ir}"
        );
        assert!(
            !ir.contains("call i64 (ptr, ptr, ...) @objc_msgSend"),
            "msg_send call must NOT be variadic-shaped (aarch64-darwin ABI); IR:\n{ir}"
        );
    }

    #[test]
    fn intrinsic_msg_send_void_return() {
        let src = "fn main() -> i32 {\n\
                     unsafe {\n\
                         let obj: *u8 = 0 as *u8;\n\
                         #msg_send(obj, \"release\");\n\
                     }\n\
                     return 0;\n\
                 }";
        let ir = gen_src_mono(src);
        // No `-> T` ascription → void return → no assignment, just a bare call.
        assert!(
            ir.contains("call void @objc_msgSend("),
            "missing void msg_send call; IR:\n{ir}"
        );
    }

    #[test]
    fn intrinsic_msg_send_forwards_extra_args() {
        let src = "fn main() -> i32 {\n\
                     unsafe {\n\
                         let obj: *u8 = 0 as *u8;\n\
                         let s: *u8 = #str_ptr(\"x\");\n\
                         let r: *u8 = #msg_send(obj, \"stringWithUTF8String:\", s) -> *u8;\n\
                     }\n\
                     return 0;\n\
                 }";
        let ir = gen_src_mono(src);
        // The call site must include all three args: recv, sel, and the
        // forwarded `s`. We check for the pattern `call ptr @objc_msgSend(ptr ..., ptr ..., ptr ...)`.
        let call_idx = ir
            .find("call ptr @objc_msgSend(")
            .expect(&format!("missing msg_send call; IR:\n{ir}"));
        let call_line_end = ir[call_idx..].find('\n').unwrap();
        let call_line = &ir[call_idx..call_idx + call_line_end];
        let ptr_count = call_line.matches("ptr").count();
        assert!(
            ptr_count >= 3,
            "expected at least 3 ptr args (recv + sel + forwarded); got line:\n{call_line}"
        );
    }

    // ---- v0.0.13 (G-043 second half): struct-literal statics ----

    #[test]
    fn struct_literal_static_emits_constant_aggregate() {
        let ir = gen_src_mono(
            "struct P { x: i32, y: f32, ok: bool }\n\
             pub static S: P = P { x: 7, y: 1.5f32, ok: true };\n\
             fn main() -> i32 { return S.x; }",
        );
        // The global is a constant struct in declared field order, with the
        // f32 field rendered as the f32 hex bit pattern and bool as i1.
        assert!(
            ir.contains("@S = constant %P { i32 7, float 0x3FF8000000000000, i1 true }"),
            "expected constant struct aggregate; IR:\n{ir}"
        );
    }

    #[test]
    fn struct_literal_static_renders_fields_in_declared_order() {
        // Source field order is reversed; output must follow the declared order.
        let ir = gen_src_mono(
            "struct P { a: i32, b: i32 }\n\
             pub static S: P = P { b: 2, a: 1 };\n\
             fn main() -> i32 { return S.a; }",
        );
        assert!(
            ir.contains("@S = constant %P { i32 1, i32 2 }"),
            "expected declared-order fields; IR:\n{ir}"
        );
    }

    #[test]
    fn nested_struct_literal_static_recurses() {
        let ir = gen_src_mono(
            "struct Inner { a: i32 }\n\
             struct Outer { i: Inner, n: i32 }\n\
             pub static O: Outer = Outer { i: Inner { a: 5 }, n: 6 };\n\
             fn main() -> i32 { return O.n; }",
        );
        assert!(
            ir.contains("@O = constant %Outer { %Inner { i32 5 }, i32 6 }"),
            "expected nested constant struct; IR:\n{ir}"
        );
    }

    #[test]
    fn f16_struct_field_static_emits_half_constant() {
        let ir = gen_src_mono(
            "struct W { a: f16, b: f16 }\n\
             pub static G: W = W { a: 1.0f16, b: 1.5f16 };\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(
            ir.contains("@G = constant %W { half 0xH3C00, half 0xH3E00 }"),
            "expected half constants; IR:\n{ir}"
        );
    }

    // ---- v0.0.13 (topic D): `#[inline]` LLVM function attributes ----

    #[test]
    fn inline_attrs_on_functions_emit_llvm_attrs() {
        let ir = gen_src(
            "#[inline] fn a(x: i32) -> i32 { return x +% 1; }\n\
             #[inline(always)] fn b(x: i32) -> i32 { return x +% 2; }\n\
             #[inline(never)] fn c(x: i32) -> i32 { return x +% 3; }\n\
             fn main() -> i32 { return a(0) +% b(0) +% c(0); }",
        );
        assert!(ir.contains("@a(i32 noundef %0) inlinehint {"), "IR:\n{ir}");
        assert!(ir.contains("@b(i32 noundef %0) alwaysinline {"), "IR:\n{ir}");
        assert!(ir.contains("@c(i32 noundef %0) noinline {"), "IR:\n{ir}");
    }

    #[test]
    fn inline_attr_on_method_emits_llvm_attr() {
        let ir = gen_src(
            "struct P { v: i32 }\n\
             impl P { #[inline(always)] fn get(self) -> i32 { return self.v; } }\n\
             fn main() -> i32 { let p: P = P { v: 5 }; return p.get(); }",
        );
        assert!(ir.contains("@P.get(%P %0) alwaysinline {"), "IR:\n{ir}");
    }

    #[test]
    fn no_inline_attr_emits_no_llvm_attr() {
        let ir = gen_src("fn plain(x: i32) -> i32 { return x; }\nfn main() -> i32 { return plain(1); }");
        // The signature closes straight into the body with no inline attribute.
        assert!(ir.contains("@plain(i32 noundef %0) {"), "IR:\n{ir}");
        assert!(!ir.contains("inlinehint"), "IR:\n{ir}");
        assert!(!ir.contains("alwaysinline"), "IR:\n{ir}");
    }

    #[test]
    fn f64_to_f16_bits_known_values() {
        assert_eq!(f64_to_f16_bits(0.0), 0x0000); // +0
        assert_eq!(f64_to_f16_bits(1.0), 0x3C00); // 1.0
        assert_eq!(f64_to_f16_bits(1.5), 0x3E00); // 1.5
        assert_eq!(f64_to_f16_bits(2.0), 0x4000); // 2.0
        assert_eq!(f64_to_f16_bits(-2.0), 0xC000); // sign bit set
        assert_eq!(f64_to_f16_bits(65504.0), 0x7BFF); // largest finite half
        assert_eq!(f64_to_f16_bits(65536.0), 0x7C00); // overflow → +inf
        assert_eq!(f64_to_f16_bits(0.5), 0x3800); // 0.5
        // Smallest normal half: 2^-14.
        assert_eq!(f64_to_f16_bits(6.103515625e-05), 0x0400);
        // A subnormal half: 2^-24 (smallest positive subnormal).
        assert_eq!(f64_to_f16_bits(5.960464477539063e-08), 0x0001);
    }
}
