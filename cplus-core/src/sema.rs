//! Semantic analysis: name resolution + type checking, single pass.
//!
//! Phase 1 scope: only `i32` and `bool` types. Reports every Phase-1 rejection
//! case from `docs/design/phase1-grammar.md` §7.2.
//!
//! Error code allocation:
//! - E0300: undefined name
//! - E0301: duplicate function definition
//! - E0302: type mismatch
//! - E0303: unknown type name
//! - E0304: condition must be `bool`
//! - E0305: assignment to immutable binding
//! - E0306: block produces no value but one is required
//! - E0307: `return` without a value when function returns non-`Unit`
//! - E0308: wrong number of arguments
//! - E0309: `main` must have signature `fn main() -> i32`
//! - E0310: float literals not supported in Phase 1
//! - E0311: non-`i32` integer suffix not supported in Phase 1
//! - E0312: feature parsed but not yet supported in Phase 1
//! - E0313: assignment target is not a place expression
//! - E0334: parameter has both `mut` and `move` (mutually exclusive)
//! - E0335: use of moved value
//! - E0337: cannot move out of non-binding place (partial moves deferred)
//! - E0338: destructor `drop` has wrong signature
//! - E0340: non-exhaustive `match` (variant not covered)
//! - E0341: pattern type doesn't match scrutinee
//! - E0342: wrong number of payload patterns / construction args for variant
//! - E0343: literal pattern not supported in Phase 3
//! - E0344: tagged-enum variant payload is `Drop` (Phase 3 doesn't synthesize variant drop)
//! - E0345: use of possibly-unassigned binding (definite-assignment failure)
//! - E0346: uninitialized `let` requires a type annotation

use crate::ast::*;
use crate::diagnostics::{DiagCode, DiagSink, Diagnostic, LineMap, Severity};
use crate::lexer::{NumSuffix, Span as ByteSpan};
use std::collections::HashMap;
use std::path::PathBuf;

/// Stable identifier for a user-defined enum. Indices into `SemaCx::enums`,
/// assigned in declaration order. Codegen rebuilds the same numbering by
/// walking `program.items` in the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnumId(pub u32);

/// Stable identifier for a user-defined struct. Same indexing convention
/// as `EnumId`, but in a separate index space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ty {
    // Signed integers
    I8, I16, I32, I64,
    // Unsigned integers
    U8, U16, U32, U64,
    // Pointer-sized
    Isize, Usize,
    // Floats
    F32, F64,
    // Other
    Bool,
    Unit,
    /// Phase 8 slice 8.STR.1: built-in string-view type. Lowered to a
    /// fat pointer `{ptr: *u8, len: usize}`. Copy semantics (a view,
    /// not an owner). String literals (`"hello"`) have this type.
    Str,
    /// Phase 8 slice 8.STR.3: owned heap-backed string. Lowered to
    /// `{ptr: *u8, len: usize, cap: usize}` — 24 bytes on 64-bit.
    /// Non-Copy, has Drop (frees the buffer via libc `free`). Built by
    /// `string::new()`, `string::with_capacity(n)`, and (slice 8.STR.B)
    /// the `"...${expr}..."` interpolation literal.
    String,
    /// Phase 11 polish (2026-05-14): slice type `T[]` — fat-pointer
    /// view `{ptr: *T, len: usize}`. 16 bytes on 64-bit. Copy semantics
    /// (a view, not an owner). The pointee `T` is preserved at the
    /// sema level (used by indexing + `slice_ptr` return type); LLVM
    /// sees only `{ ptr, i64 }` since `ptr` is opaque.
    Slice(Box<Ty>),
    /// Slice 10.FFI.1: raw pointer `*T`. Lowered to LLVM `ptr` (opaque,
    /// 8 bytes on 64-bit). Copy semantics. No borrow checking; the
    /// caller is responsible for lifetime and aliasing. Deref / index /
    /// arithmetic land in 10.FFI.2.
    RawPtr(Box<Ty>),
    /// Slice 11.FN_PTR: function pointer — `fn(T1, T2) -> R`. Lowered
    /// to LLVM `ptr`. Copy semantics. No environment capture (no
    /// closures in C+); the pointer is just the symbol's address.
    /// Calling convention is always ccc. Two FnPtr values are equal
    /// iff their parameter types and return type are equal.
    FnPtr { params: Vec<Ty>, return_type: Box<Ty> },
    Enum(EnumId),
    Struct(StructId),
    /// Fixed-size array: element type + length.
    Array(Box<Ty>, u32),
    /// Slice 7GEN.4: a generic type parameter, identified by name. Appears
    /// inside the body of a generic fn / method / struct / enum or inside an
    /// `interface` / `impl Interface for ...` block (where `Self` is
    /// represented as `Param("Self")`). Two `Ty::Param` values are equal
    /// iff their names match — the surrounding signature gives them meaning.
    /// Substitution at instantiation time (slice 7GEN.5) replaces each
    /// `Param` with a concrete type.
    Param(String),
    Error,   // sentinel for recovery; matches anything
}

impl Ty {
    /// Human-readable type name. For enums and structs we render a generic
    /// kind name; SemaCx has the actual table if higher-fidelity names are
    /// needed in a diagnostic message.
    pub fn name(&self) -> &'static str {
        match self {
            Ty::I8 => "i8", Ty::I16 => "i16", Ty::I32 => "i32", Ty::I64 => "i64",
            Ty::U8 => "u8", Ty::U16 => "u16", Ty::U32 => "u32", Ty::U64 => "u64",
            Ty::Isize => "isize", Ty::Usize => "usize",
            Ty::F32 => "f32", Ty::F64 => "f64",
            Ty::Bool => "bool",
            Ty::Unit => "()",
            Ty::Str => "str",
            Ty::String => "string",
            Ty::Slice(_) => "slice",
            Ty::RawPtr(_) => "raw-pointer",
            Ty::FnPtr { .. } => "fn-pointer",
            Ty::Enum(_) => "enum",
            Ty::Struct(_) => "struct",
            Ty::Array(_, _) => "array",
            Ty::Param(_) => "type-param",
            Ty::Error => "<error>",
        }
    }

    pub fn is_signed_int(&self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize)
    }
    pub fn is_unsigned_int(&self) -> bool {
        matches!(self, Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Usize)
    }
    pub fn is_int(&self) -> bool { self.is_signed_int() || self.is_unsigned_int() }
    pub fn is_float(&self) -> bool { matches!(self, Ty::F32 | Ty::F64) }
    pub fn is_numeric(&self) -> bool { self.is_int() || self.is_float() }
    pub fn is_enum(&self) -> bool { matches!(self, Ty::Enum(_)) }
    pub fn is_struct(&self) -> bool { matches!(self, Ty::Struct(_)) }
    pub fn is_array(&self) -> bool { matches!(self, Ty::Array(_, _)) }

    /// Phase 3 conservative `Copy` rule: primitives, `bool`, `()`, and plain
    /// Atomic `Copy` rule: types whose `Copy`-ness is fixed by the type itself,
    /// not by its components. Primitives, `bool`, `()`, and the `Error`
    /// sentinel (treated as Copy to avoid cascading move diagnostics on
    /// already-broken code). For composite types (`Array`, `Struct`,
    /// `Enum`) call `SemaCx::is_copy(&ty)` instead — the answer depends on
    /// the type table (a tagged enum is Copy iff every payload is Copy).
    pub fn is_atomic_copy(&self) -> bool {
        match self {
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64
            | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
            | Ty::Isize | Ty::Usize
            | Ty::F32 | Ty::F64
            | Ty::Bool | Ty::Unit
            | Ty::Str
            | Ty::Slice(_)
            | Ty::RawPtr(_)
            | Ty::FnPtr { .. }
            | Ty::Error => true,
            // Phase 8 slice 8.STR.3: owned `string` is non-Copy and Drop.
            Ty::Struct(_) | Ty::Array(_, _) | Ty::Enum(_) | Ty::String => false,
            // Slice 7GEN.4: a generic type parameter's Copy-ness is
            // determined by the concrete substitution. Conservatively
            // treat as non-Copy at the abstract level — bound-aware
            // Copy derivation (e.g. `T: Copy`) is Phase-7 follow-up
            // work that lands when the `Copy` interface itself does.
            Ty::Param(_) => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<EnumVariantDef>,
    /// Cached `Copy` flag: true iff every variant's payload type is `Copy`.
    /// Plain enums (no payloads) are always Copy. Computed in
    /// `compute_enum_copy_flags` (mirrors the struct case).
    pub is_copy: bool,
    /// True iff at least one variant has a payload. Plain enums stay
    /// payload-less and keep their Phase-2A bare-`i32` lowering; tagged
    /// enums get the `{ i32 tag, [N x i8] payload }` layout. Cached so
    /// codegen doesn't recompute the test.
    pub is_tagged: bool,
    /// Source-level generic name when this EnumDef was synthesized
    /// from a generic-enum instantiation (slice 7GEN.5d). For
    /// `enum Option[T] { ... }` instantiated as `Option[i32]`, this is
    /// `Some("Option")`. Used by `check_pattern` to accept unqualified
    /// `Option::Some(v)` patterns against `Option[i32]` scrutinees
    /// (type-directed resolution). `None` for plain (non-generic) enums.
    pub generic_base: Option<String>,
    /// Records `(template_name, concrete_args)` when this EnumDef was
    /// synthesized from a generic-enum instantiation. Used by `subst_ty`
    /// to recurse through nested generics: when substituting `T` inside
    /// a return type like `Result[T, Err]`, we walk the args, substitute,
    /// and re-instantiate. `None` for non-generic enums.
    pub generic_origin: Option<(String, Vec<Ty>)>,
}

#[derive(Debug, Clone)]
pub struct EnumVariantDef {
    pub name: String,
    /// Positional payload types. Empty for payload-less variants.
    pub payload: Vec<Ty>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    /// Field name → (declaration order index, field type, is_pub). Order
    /// matters for codegen to compute correct GEP indices. `is_pub` is
    /// honored by sema's cross-file field-access check (E0403/Field,
    /// slice 4C); same-file access ignores it.
    pub fields: Vec<(String, Ty, bool)>,
    /// Methods declared in any `impl` block for this struct.
    pub methods: HashMap<String, MethodSig>,
    /// Cached `Copy` flag — structural auto-derive: true iff every field type
    /// is `Copy` AND the type is not `Drop`. Computed by
    /// `compute_struct_copy_flags` after field types and Drop status are
    /// resolved. See `docs/design/phase3-copy-derivation.md`.
    pub is_copy: bool,
    /// True iff this struct has a destructor (a method named `drop` with the
    /// signature `fn drop(mut self)`). Drop types are always non-`Copy` —
    /// see `docs/design/phase3-drop.md`. Set by `collect_methods`.
    pub is_drop: bool,
    /// Slice 10.FFI.5: true when the struct carries `#[repr(C)]`.
    /// Promises a C-compatible layout for FFI passing — fields stored
    /// in declaration order with the platform's C ABI padding rules.
    /// Today's default emission already matches this on x86_64 for
    /// primitive-typed fields; the flag is the *stability* commitment
    /// that this won't change under future codegen optimizations.
    pub is_repr_c: bool,
    /// File this struct was declared in, derived from the qualified name
    /// at collection time (e.g. struct `src.math.Point` lives in file
    /// `src.math`). `None` for items without a qualified name (single-file
    /// mode). Used to gate field-pub checks against the access site.
    pub origin_file: Option<String>,
    /// Records `(template_name, concrete_args)` when this StructDef was
    /// synthesized from a generic-struct instantiation. Used by `subst_ty`
    /// to recurse through nested generics: substituting `T` inside a
    /// return type like `Box[T]` walks the args, substitutes, and
    /// re-instantiates. `None` for non-generic structs.
    pub generic_origin: Option<(String, Vec<Ty>)>,
}

/// Type + ownership marker for a single parameter. The `move_` flag indicates
/// the parameter was declared `move x: T` and consumes its argument (when the
/// argument's type is non-Copy). The `mutable` flag (`mut x: T`) is recorded
/// for completeness but is body-internal — call sites don't care.
#[derive(Debug, Clone)]
pub struct ParamSig {
    pub ty: Ty,
    pub mutable: bool,
    pub move_: bool,
}

#[derive(Debug, Clone)]
pub struct MethodSig {
    pub receiver: Option<Receiver>,
    /// Parameter signatures *excluding* the receiver.
    pub params: Vec<ParamSig>,
    pub return_type: Ty,
    /// Slice 7GEN.5e: method-level generic parameter names. Empty for
    /// non-generic methods. When non-empty, `params` and `return_type`
    /// may contain `Ty::Param(name)` placeholders; call sites must
    /// substitute via inference or an explicit turbofish.
    pub generic_params: Vec<String>,
    /// Slice 7GEN.5e step 4: bounds parallel to `generic_params`.
    /// Empty list per param when unbounded.
    pub generic_bounds: Vec<Vec<String>>,
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<(u32, Ty)> {
        self.fields.iter().enumerate().find_map(|(i, (n, t, _))| {
            (n == name).then(|| (i as u32, t.clone()))
        })
    }

    /// Like `field` but also returns the field's `pub` flag.
    pub fn field_with_pub(&self, name: &str) -> Option<(u32, Ty, bool)> {
        self.fields.iter().enumerate().find_map(|(i, (n, t, p))| {
            (n == name).then(|| (i as u32, t.clone(), *p))
        })
    }
}

#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<ParamSig>,
    pub return_type: Ty,
    /// Slice 10.FFI.4: variadic extern fn. When true, callers may
    /// pass any number of extra args after the fixed `params`. Only
    /// set on extern fns; C+-defined fns are never variadic.
    pub is_variadic: bool,
    /// Phase 11 / ObjC interop: `#[link_name = "..."]` aliases the
    /// linker symbol so multiple typed `extern fn` declarations can
    /// resolve to the same C symbol. None means "use the fn's source
    /// name as the symbol." Only meaningful on extern fns; sema rejects
    /// the attribute on non-extern fns.
    pub link_name: Option<String>,
}

/// Slice 7GEN.5a: a generic-fn signature with its type-parameter
/// names captured alongside the (potentially `Ty::Param`-bearing)
/// param + return types. Used at call sites to drive inference:
/// each `Ty::Param(name)` in `params` is matched against the
/// actual argument's type to build a substitution map.
#[derive(Debug, Clone)]
pub struct GenericFnSig {
    pub generic_params: Vec<String>,
    /// Slice 7GEN.5e step 4 + 7GEN.6: bounds for each generic param.
    /// `bounds[i]` lists interface names that `generic_params[i]` must
    /// implement (e.g. `T: Ord + Eq` → `["Ord", "Eq"]`). Empty for
    /// unbounded params.
    pub bounds: Vec<Vec<String>>,
    pub params: Vec<ParamSig>,
    pub return_type: Ty,
}

/// Slice 7GEN.4: a registered `interface Name { fn ... }` declaration.
/// Method signatures are stored with `Self` represented as
/// `Ty::Param("Self")` so impl-validation can substitute the concrete
/// implementing type by walking the type tree once.
#[derive(Debug, Clone)]
pub struct InterfaceDef {
    pub name: String,
    pub methods: HashMap<String, MethodSig>,
    /// File the interface was declared in. Used by the orphan rule
    /// (E0507): an `impl Interface for Type` block must live in the same
    /// file as either the interface or the implementing type. `None` in
    /// single-file mode.
    pub origin_file: Option<String>,
}

#[derive(Debug, Clone)]
struct LocalInfo {
    ty: Ty,
    mutable: bool,
    /// True iff this binding has been consumed by a move. Reads of a moved
    /// binding produce E0335. Move tracking is linear within the body in
    /// Phase 3; flow-sensitive merging across branches is Phase 5 work.
    moved: bool,
    /// True iff this binding has been assigned a value at the current
    /// program point. `let x: T = expr;` starts true; `let x: T;` starts
    /// false. Each subsequent `x = ...` flips it to true. Reads of a
    /// false-assigned binding produce E0345. Flow-sensitive: snapshotted
    /// around `if`/`else`/`match` and merged by intersection — see
    /// `flow_snapshot`/`flow_restore`/`flow_merge`.
    assigned: bool,
}

/// Run sema on a parsed program. Returns all diagnostics produced;
/// the program is well-typed iff none have severity `Error`.
///
/// Single-file entry point. For multi-file projects (Phase 4 slice 4C),
/// see `check_multi`, which threads per-file source through so cross-file
/// diagnostics render with the right file path and line/column.
pub fn check(program: &Program, file: PathBuf, src: &str) -> Vec<Diagnostic> {
    check_with_files(program, file, src, std::collections::BTreeMap::new())
}

/// Slice 7GEN.5a: instantiation info produced by sema and consumed by
/// the `monomorphize` pass. `instantiations` is the deduplicated set
/// of `(generic_fn_name, [concrete_args])` pairs that need synthesized
/// bodies; `call_monos` maps each generic-fn call site (keyed by the
/// `Call` expression's span) to its concrete arg list, so monomorphize
/// can rewrite each callee to the mangled name.
#[derive(Debug, Default, Clone)]
pub struct MonoInfo {
    pub instantiations: std::collections::BTreeSet<(String, Vec<Ty>)>,
    pub call_monos: HashMap<ByteSpan, Vec<Ty>>,
    /// Slice 7GEN.5c: generic-struct instantiations. Maps
    /// `(generic_name, [concrete_args])` to the synthesized
    /// `StructDef` (cloned out of sema's table so monomorphize can
    /// emit AST items + look up mangled names).
    pub struct_instantiations: std::collections::BTreeMap<(String, Vec<Ty>), StructInstantiationInfo>,
    /// Slice 7GEN.5d: generic-enum instantiations.
    pub enum_instantiations: std::collections::BTreeMap<(String, Vec<Ty>), EnumInstantiationInfo>,
    /// Slice 7GEN.5e: generic-method instantiations.
    /// Keyed by `(struct_name, method_name, [concrete_args])`.
    pub method_instantiations: std::collections::BTreeSet<(String, String, Vec<Ty>)>,
    /// Phase 11 polish (2026-05-13): type-alias map. Monomorphize
    /// substitutes `TypeKind::Path(name)` where `name` is an alias to
    /// the alias target before doing anything else — codegen never
    /// sees alias names. Cycle detection happened at sema.
    pub type_aliases: std::collections::BTreeMap<String, crate::ast::Type>,
    /// v0.0.4 Phase 1C: per-call-site marker for the `Type[args]::name(...)`
    /// shape where `name` is a same-module free generic fn (not an impl
    /// method). Sema dispatched the call to the free fn; monomorphize
    /// uses this map to rewrite the `GenericEnumCall` AST node to a
    /// plain `Call { callee: Ident(qualified_fn_name), ... }` instead
    /// of the default enum/struct-variant lowering. Keyed by the
    /// outer call's span (matching the AST node's span).
    pub assoc_free_fn_dispatches: HashMap<ByteSpan, String>,
}

/// Slice 7GEN.5c: per-instantiation info handed to monomorphize.
#[derive(Debug, Clone)]
pub struct StructInstantiationInfo {
    pub mangled_name: String,
    pub fields: Vec<(String, Ty, bool)>,
    pub template_origin_file: Option<String>,
    /// Sema-assigned `StructId.0`. Exposed so consumers can map
    /// `Ty::Struct(id)` back to a mangled name (the `name_of` closure
    /// in `run_monomorphize`). Without this, generic instantiations
    /// would render as `"?"` because they live past the non-generic
    /// portion of `cx.structs`.
    pub id: u32,
}

/// Slice 7GEN.5d: per-enum-instantiation info handed to monomorphize.
#[derive(Debug, Clone)]
pub struct EnumInstantiationInfo {
    pub mangled_name: String,
    pub variants: Vec<EnumVariantDef>,
    pub template_origin_file: Option<String>,
    /// See `StructInstantiationInfo::id` — same role for the enum side.
    pub id: u32,
}

/// Slice 7GEN.5a: like `check_multi`, but also returns the
/// monomorphization data so a follow-up pass can synthesize the
/// concrete fn instantiations + rewrite call sites.
pub fn check_multi_with_mono(
    program: &Program,
    entry_file: PathBuf,
    entry_src: &str,
    files: std::collections::BTreeMap<String, (PathBuf, String)>,
) -> (Vec<Diagnostic>, MonoInfo) {
    check_with_files_inner(program, entry_file, entry_src, files)
}

/// Multi-file entry: as `check`, but with a `files` map providing
/// `(path, source)` per file id. Sema's `err()` consults the current
/// item's `origin_file` and routes to the matching `LineMap` so diagnostic
/// spans render against the file the offending code actually lives in.
/// The `file` + `src` arguments still anchor the "default" line-map used
/// when an item has no `origin_file` (single-file mode artifacts, builtin
/// errors, etc.).
pub fn check_multi(
    program: &Program,
    entry_file: PathBuf,
    entry_src: &str,
    files: std::collections::BTreeMap<String, (PathBuf, String)>,
) -> Vec<Diagnostic> {
    check_with_files(program, entry_file, entry_src, files)
}

fn check_with_files<'a>(
    program: &Program,
    file: PathBuf,
    src: &'a str,
    files_raw: std::collections::BTreeMap<String, (PathBuf, String)>,
) -> Vec<Diagnostic> {
    check_with_files_inner(program, file, src, files_raw).0
}

fn check_with_files_inner<'a>(
    program: &Program,
    file: PathBuf,
    src: &'a str,
    files_raw: std::collections::BTreeMap<String, (PathBuf, String)>,
) -> (Vec<Diagnostic>, MonoInfo) {
    let lm = LineMap::new(src);
    let mut sink = DiagSink::new();
    let files: std::collections::BTreeMap<String, FileCtx> = files_raw
        .into_iter()
        .map(|(fid, (p, s))| {
            let lm = LineMap::new(&s);
            (fid, FileCtx { path: p, src: s, lm })
        })
        .collect();
    let mut cx = SemaCx {
        file,
        src,
        lm,
        sink: &mut sink,
        fns: HashMap::new(),
        enums: Vec::new(),
        enum_by_name: HashMap::new(),
        structs: Vec::new(),
        struct_by_name: HashMap::new(),
        type_aliases: HashMap::new(),
        resolving_aliases: std::collections::HashSet::new(),
        scopes: Vec::new(),
        current_return: Ty::Error,
        current_fn_is_async: false,
        current_file: None,
        files,
        loop_depth: 0,
        unsafe_depth: 0,
        extern_fns: std::collections::HashSet::new(),
        type_params_stack: Vec::new(),
        self_type_stack: Vec::new(),
        interfaces: HashMap::new(),
        interface_impls: std::collections::HashSet::new(),
        fns_generic: HashMap::new(),
        fn_instantiations: std::collections::BTreeSet::new(),
        call_monos: HashMap::new(),
        assoc_free_fn_dispatches: HashMap::new(),
        struct_generic_templates: HashMap::new(),
        struct_instantiations: std::collections::BTreeMap::new(),
        enum_generic_templates: HashMap::new(),
        enum_instantiations: std::collections::BTreeMap::new(),
        method_instantiations: std::collections::BTreeSet::new(),
        generic_impl_methods: HashMap::new(),
    };
    cx.register_builtins();
    // Type collection order:
    //   1. names (struct + enum)
    //   2. struct fields (resolves types referenced in fields)
    //   3. enum variant payloads (resolves types in variant payload lists)
    //   4. methods (also detects `drop` and sets `is_drop`)
    //   5. Copy flags for structs + enums (needs Drop status from step 4)
    //
    // Note: step 4 runs *before* step 5 even though it semantically depends
    // on Copy-ness of param types for `move`-marker checks. That dependency
    // only matters in `check_method_call` (and friends), which run during
    // body-checking — long after all five collection passes have finished.
    cx.collect_type_names(program);
    cx.register_blessed_interfaces();
    cx.collect_interfaces(program);
    cx.collect_struct_fields(program);
    cx.collect_enum_payloads(program);
    cx.collect_methods(program);
    cx.compute_struct_copy_flags();
    cx.compute_enum_copy_flags(program);
    cx.collect_functions(program);
    cx.check_main_signature(program);
    cx.validate_interface_impls(program);
    cx.check_functions(program);
    cx.check_methods(program);
    // Slice 7GEN.5c: hand monomorphize a snapshot of the synthesized
    // struct instantiations (mangled name + concrete fields) so it can
    // emit AST struct items + rewrite Generic types in the program.
    //
    // 7GEN.5c carry-forward (closed 2026-05-13): filter out *placeholder*
    // instantiations whose args still contain `Ty::Param` — those are
    // template-body artifacts (e.g. `Box[T]` mentioned in `fn boxed[T]`'s
    // return type), not real instantiations. Emitting them as AST struct
    // decls would panic codegen on Ty::Param. They live in sema's table
    // only because `resolve_type` dedup'd them; real instantiations get
    // produced by `subst_ty_deep` at concrete call sites.
    let struct_instantiations: std::collections::BTreeMap<(String, Vec<Ty>), StructInstantiationInfo> = cx
        .struct_instantiations
        .iter()
        .filter(|(key, _)| !key.1.iter().any(|t| ty_contains_param(t, &cx.structs, &cx.enums)))
        .map(|(key, &id)| {
            let def = &cx.structs[id.0 as usize];
            let info = StructInstantiationInfo {
                mangled_name: def.name.clone(),
                fields: def.fields.clone(),
                template_origin_file: cx
                    .struct_generic_templates
                    .get(&key.0)
                    .and_then(|t| {
                        // Find the original item's origin_file by walking
                        // the program once. Templates don't carry it
                        // directly; this is best-effort and `None` is
                        // acceptable (single-file mode artifact).
                        program
                            .items
                            .iter()
                            .find(|i| matches!(&i.kind, ItemKind::Struct(s) if s.name.name == t.name.name))
                            .and_then(|i| i.origin_file.clone())
                    }),
                id: id.0,
            };
            (key.clone(), info)
        })
        .collect();
    // Slice 7GEN.5d: enum instantiations. Same placeholder filter as
    // the struct side (see comment above).
    let enum_instantiations: std::collections::BTreeMap<(String, Vec<Ty>), EnumInstantiationInfo> = cx
        .enum_instantiations
        .iter()
        .filter(|(key, _)| !key.1.iter().any(|t| ty_contains_param(t, &cx.structs, &cx.enums)))
        .map(|(key, &id)| {
            let def = &cx.enums[id.0 as usize];
            let info = EnumInstantiationInfo {
                mangled_name: def.name.clone(),
                variants: def.variants.clone(),
                template_origin_file: cx
                    .enum_generic_templates
                    .get(&key.0)
                    .and_then(|t| {
                        program
                            .items
                            .iter()
                            .find(|i| matches!(&i.kind, ItemKind::Enum(e) if e.name.name == t.name.name))
                            .and_then(|i| i.origin_file.clone())
                    }),
                id: id.0,
            };
            (key.clone(), info)
        })
        .collect();
    // Slice 7GEN.5e: convert internal (StructId, ...) keys to
    // (struct_name, ...) for export.
    let method_instantiations: std::collections::BTreeSet<(String, String, Vec<Ty>)> = cx
        .method_instantiations
        .iter()
        .map(|(sid, mname, args)| {
            let sname = cx.structs[sid.0 as usize].name.clone();
            (sname, mname.clone(), args.clone())
        })
        .collect();
    let type_aliases: std::collections::BTreeMap<String, Type> = cx
        .type_aliases
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mono = MonoInfo {
        instantiations: std::mem::take(&mut cx.fn_instantiations),
        call_monos: std::mem::take(&mut cx.call_monos),
        assoc_free_fn_dispatches: std::mem::take(&mut cx.assoc_free_fn_dispatches),
        struct_instantiations,
        enum_instantiations,
        method_instantiations,
        type_aliases,
    };
    (sink.into_vec(), mono)
}

struct FileCtx {
    path: PathBuf,
    src: String,
    lm: LineMap,
}

struct SemaCx<'a> {
    /// Default file path / source / line-map. Used when `current_file`
    /// is None or absent from the `files` map (single-file mode, builtin
    /// errors, etc.). For multi-file projects this is the entry binary.
    file: PathBuf,
    src: &'a str,
    lm: LineMap,
    sink: &'a mut DiagSink,
    fns: HashMap<String, FnSig>,
    enums: Vec<EnumDef>,
    enum_by_name: HashMap<String, EnumId>,
    structs: Vec<StructDef>,
    struct_by_name: HashMap<String, StructId>,
    /// Phase 11 polish (2026-05-13): `type Foo = Bar;` aliases. Maps
    /// the alias name to its target AST `Type`. Resolved on every use
    /// via `resolve_type` — transparent (Foo and Bar are identical at
    /// the Ty level). Detect cycles at resolution time, not collection.
    type_aliases: HashMap<String, Type>,
    /// Cycle-detection set for `resolve_type` recursion through aliases.
    /// `type A = B; type B = A;` fires E0510 when the second resolve
    /// re-enters the same name.
    resolving_aliases: std::collections::HashSet<String>,
    scopes: Vec<HashMap<String, LocalInfo>>,
    current_return: Ty,
    /// v0.0.3 Phase 5 Slice 5E.2: tracks whether the function being
    /// checked is `async fn`. Set by `check_function`/`check_method`
    /// to the function's `is_async` flag for the duration of body
    /// checking; restored on exit. Used by `await EXPR` to enforce
    /// "await only inside async fn" and by `return EXPR` to know that
    /// `current_return` is the post-wrap `Future[T]` whose inner T
    /// is what the user's value must match.
    current_fn_is_async: bool,
    /// Slice 4C: file the currently-checked item originated from (post
    /// resolver merge). `None` in single-file mode or for items the
    /// resolver didn't touch. Used both to gate field-pub access (see
    /// `is_cross_file_access`) and to route `err()` to the right
    /// `LineMap` so cross-file diagnostics render with proper line/col.
    current_file: Option<String>,
    /// Per-file context. Keyed by file id (`src.math`, etc.); built
    /// once at `check_multi` entry from the resolver's source-by-id map.
    files: std::collections::BTreeMap<String, FileCtx>,
    /// Slice 4-end: number of enclosing loops at the current point.
    /// Incremented entering `while` / `for` / `loop` bodies; decremented
    /// on exit. `break` / `continue` require `> 0` (E0353).
    loop_depth: u32,
    /// Slice 10.FFI.3: tracks nesting depth of `unsafe { ... }` blocks.
    /// Zero outside an unsafe block; positive inside. Pointer deref,
    /// extern fn calls, and `str_from_raw_parts` fire **E0801** when
    /// invoked at depth 0.
    unsafe_depth: u32,
    /// Slice 10.FFI.3: names of extern fns declared in this program.
    /// Calls to these fire E0801 outside an unsafe block.
    extern_fns: std::collections::HashSet<String>,
    /// Slice 7GEN.4: stack of type-parameter scopes. Each frame holds the
    /// set of generic-param names visible at the current point. Pushed
    /// before entering a generic item's body (and corresponding
    /// signature-collection pass), popped on exit. `resolve_type`
    /// consults the top-of-stack to recognize `T` as `Ty::Param("T")`
    /// instead of erroring E0303.
    ///
    /// `Self` is treated as a magic type-param name. Inside an
    /// `interface { ... }` body it stays abstract (`Ty::Param("Self")`);
    /// inside an `impl Type { ... }` body it resolves to the impl
    /// target's concrete `Ty` via `self_type_stack` (consulted before
    /// the generic-param scope so `Self` always wins when both are
    /// pushed). Outside both contexts, the name `Self` is rejected
    /// with E0508.
    type_params_stack: Vec<std::collections::HashSet<String>>,
    /// Slice 7GEN.4: stack of "what does `Self` refer to here?" entries.
    /// `Some(ty)` inside an `impl Type { ... }` body (or its method
    /// signatures); `None` everywhere else. When present, `resolve_type`
    /// returns this `Ty` for the magic name `Self`. Used by both impl-body
    /// resolution and the interface-impl signature substitution pass.
    self_type_stack: Vec<Ty>,
    /// Slice 7GEN.4: interface registry. Indexed by name (interfaces live
    /// in their own namespace, distinct from struct/enum). Each entry
    /// holds the interface's declared method signatures with `Self`
    /// represented as `Ty::Param("Self")` — substitution to a concrete
    /// type happens at impl-validation time.
    interfaces: HashMap<String, InterfaceDef>,
    /// Slice 7GEN.4: set of `(interface_name, target_type_name)` pairs
    /// that have a registered `impl Interface for Type` block. Used by
    /// E0506 (duplicate impl) and by future bound-checking call sites
    /// (E0502 — type does not satisfy interface bound, slice 7GEN.5).
    interface_impls: std::collections::HashSet<(String, String)>,
    /// Slice 7GEN.5a: generic-fn signature table. Indexed by name;
    /// disjoint from `fns` (a fn is in exactly one of the two tables
    /// based on whether `generic_params` is non-empty).
    fns_generic: HashMap<String, GenericFnSig>,
    /// Slice 7GEN.5a: unique generic-fn instantiations seen at call
    /// sites. Each entry is `(fn_name, [concrete_arg_types])`. Drives
    /// monomorphization: one synthesized concrete fn per entry.
    fn_instantiations: std::collections::BTreeSet<(String, Vec<Ty>)>,
    /// Slice 7GEN.5a: per-call-site mapping from a generic call's span
    /// to the inferred concrete type-arguments. The monomorphize pass
    /// looks up each `Call` node by span to pick the right mangled
    /// callee name.
    call_monos: HashMap<ByteSpan, Vec<Ty>>,
    /// v0.0.4 Phase 1C: `Type[args]::name(...)` call sites that resolved
    /// to a free generic fn (not an impl method). Maps the
    /// `GenericEnumCall`'s span to the qualified free fn name sema
    /// dispatched to. Monomorphize uses this to lower the AST node
    /// to the right Call shape.
    assoc_free_fn_dispatches: HashMap<ByteSpan, String>,
    /// Slice 7GEN.5c: generic-struct templates. Keyed by source name.
    /// Holds the cloned AST `StructDecl` so on-demand instantiation
    /// (`resolve_generic_instantiation`) can substitute Param types in
    /// field types. Templates are NOT in `struct_by_name` — concrete
    /// instantiations are.
    struct_generic_templates: HashMap<String, StructDecl>,
    /// Slice 7GEN.5c: per-instantiation dedup. Keys are
    /// `(generic_name, [concrete_args])`; values are the StructId of
    /// the synthesized concrete struct. Ensures repeated references to
    /// `Pair[i32, i32]` share one struct.
    struct_instantiations: std::collections::BTreeMap<(String, Vec<Ty>), StructId>,
    /// Slice 7GEN.5d: generic-enum templates. Parallel to
    /// `struct_generic_templates`. Holds the original `EnumDecl`s so
    /// `resolve_generic_instantiation` can substitute Param types in
    /// variant payload types.
    enum_generic_templates: HashMap<String, EnumDecl>,
    /// Slice 7GEN.5d: per-instantiation dedup for enums. Mirrors
    /// `struct_instantiations`.
    enum_instantiations: std::collections::BTreeMap<(String, Vec<Ty>), EnumId>,
    /// Slice 7GEN.5e: generic-method instantiations.
    /// Keyed by `(struct_id, method_name, [concrete_args])`.
    /// At export time `(struct_id, ...)` is converted to
    /// `(struct_name, ...)` for the MonoInfo entry.
    method_instantiations: std::collections::BTreeSet<(StructId, String, Vec<Ty>)>,
    /// Slice 7GEN.5e step 3: methods declared inside generic-typed
    /// impl blocks (`impl Vec[T] { fn push(self, item: T) }`). Keyed
    /// by the generic struct/enum template name (e.g. `"Vec"`). Each
    /// entry carries enough info to populate the methods table of a
    /// synthesized concrete StructDef when the template gets
    /// instantiated. Method bodies remain in the original ItemKind::Impl
    /// AST and are walked by the monomorphize pass.
    generic_impl_methods: HashMap<String, Vec<GenericImplMethodTemplate>>,
}

/// Slice 7GEN.5e step 3: method template stored on a generic-typed
/// impl block, before any per-instantiation substitution.
#[derive(Debug, Clone)]
pub struct GenericImplMethodTemplate {
    pub name: String,
    pub receiver: Option<Receiver>,
    pub params: Vec<ParamSig>,             // may contain Ty::Param
    pub return_type: Ty,                   // may contain Ty::Param
    pub impl_generic_params: Vec<String>,  // T from `impl Vec[T]`
    pub method_generic_params: Vec<String>, // U from `fn map[U]`
    pub is_drop: bool,                     // marker for cached Drop bookkeeping (always false today)
}

impl SemaCx<'_> {
    // ---- diagnostic helpers ----

    fn err(&mut self, code: &'static str, msg: String, span: ByteSpan) {
        // Slice 4C: route through `current_file` so a span belonging to
        // an imported file renders with that file's path + line/col.
        // Falls back to the entry/default context for any item that
        // wasn't tagged by the resolver (single-file mode, builtins).
        let primary = match self.current_file.as_ref().and_then(|f| self.files.get(f)) {
            Some(fc) => fc.lm.span(&fc.path, span, &fc.src),
            None => self.lm.span(&self.file, span, self.src),
        };
        self.sink.emit(Diagnostic {
            severity: Severity::Error,
            code: DiagCode(code),
            message: msg,
            primary,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
    }

    // ---- setup ----

    fn register_builtins(&mut self) {
        // `println(n: i32)` — emitted by codegen as a call to `printf("%d\n", n)`.
        self.fns.insert(
            "println".to_string(),
            FnSig {
                params: vec![ParamSig { ty: Ty::I32, mutable: false, move_: false }],
                return_type: Ty::Unit,
                is_variadic: false,
                link_name: None,
            },
        );
    }

    /// First pass: register every enum and struct *name* (and enum variants),
    /// without resolving struct field types yet. This lets struct fields
    /// reference any user-defined type regardless of declaration order.
    fn collect_type_names(&mut self, p: &Program) {
        for item in &p.items {
            // Slice 4C: set current_file so diagnostics route to the
            // declaring file's line-map. Reset at end of pass.
            self.current_file = item.origin_file.clone();
            match &item.kind {
                ItemKind::Enum(e) => {
                    // Slice 7GEN.5d: generic enum templates go to a
                    // separate table — they're not concrete types
                    // until instantiated.
                    if !e.generic_params.is_empty() {
                        if self.type_name_taken(&e.name.name)
                            || self.enum_generic_templates.contains_key(&e.name.name)
                        {
                            self.err(
                                "E0301",
                                format!("duplicate type definition `{}`", e.name.name),
                                e.name.span,
                            );
                            continue;
                        }
                        self.enum_generic_templates.insert(e.name.name.clone(), e.clone());
                        continue;
                    }
                    let mut seen: HashMap<String, ()> = HashMap::new();
                    let mut variants = Vec::new();
                    for v in &e.variants {
                        if seen.contains_key(&v.name.name) {
                            self.err(
                                "E0318",
                                format!("duplicate variant `{}` in enum `{}`", v.name.name, e.name.name),
                                v.name.span,
                            );
                            continue;
                        }
                        seen.insert(v.name.name.clone(), ());
                        // Payload types are resolved in a separate pass after
                        // all type names exist (so payloads can reference any
                        // struct or enum declared anywhere in the program).
                        variants.push(EnumVariantDef {
                            name: v.name.name.clone(),
                            payload: Vec::new(),
                        });
                    }
                    if self.type_name_taken(&e.name.name) {
                        self.err(
                            "E0301",
                            format!("duplicate type definition `{}`", e.name.name),
                            e.name.span,
                        );
                        continue;
                    }
                    let id = EnumId(self.enums.len() as u32);
                    let is_tagged = e.variants.iter().any(|v| !v.payload.is_empty());
                    self.enums.push(EnumDef {
                        name: e.name.name.clone(),
                        variants,
                        is_copy: false,   // computed later
                        is_tagged,
                        generic_base: None,
                        generic_origin: None,
                    });
                    self.enum_by_name.insert(e.name.name.clone(), id);
                }
                ItemKind::Struct(s) => {
                    if self.type_name_taken(&s.name.name)
                        || self.struct_generic_templates.contains_key(&s.name.name)
                    {
                        self.err(
                            "E0301",
                            format!("duplicate type definition `{}`", s.name.name),
                            s.name.span,
                        );
                        continue;
                    }
                    // Slice 7GEN.5c: generic struct templates go into
                    // their own table. They're not concrete types —
                    // sema synthesizes a concrete StructDef per unique
                    // instantiation lazily via `resolve_generic_instantiation`.
                    if !s.generic_params.is_empty() {
                        self.struct_generic_templates.insert(s.name.name.clone(), s.clone());
                        continue;
                    }
                    let id = StructId(self.structs.len() as u32);
                    // Slice 10.FFI.5: detect `#[repr(C)]` on the
                    // declaration. The attrs pass has already validated
                    // the args are `(C)`; here we just check presence.
                    let is_repr_c = s.attributes.iter().any(|a| a.path.name == "repr");
                    self.structs.push(StructDef {
                        name: s.name.name.clone(),
                        fields: Vec::new(),
                        methods: HashMap::new(),
                        is_copy: false,
                        is_drop: false,
                        is_repr_c,
                        // Slice 4C: an item's origin_file is set by the
                        // resolver. For struct fields' pub gate we want
                        // the file the struct was *declared* in, not
                        // whatever file is currently being checked.
                        origin_file: item.origin_file.clone(),
                        generic_origin: None,
                    });
                    self.struct_by_name.insert(s.name.name.clone(), id);
                }
                // Phase 11 polish: register type aliases by name. We
                // store the *target AST type* (not a resolved Ty) so the
                // resolver runs once at every use site — keeps the alias
                // transparent for paths whose type depends on later
                // items.
                ItemKind::TypeAlias(a) => {
                    if self.type_name_taken(&a.name.name)
                        || self.type_aliases.contains_key(&a.name.name)
                    {
                        self.err(
                            "E0301",
                            format!("duplicate type definition `{}`", a.name.name),
                            a.name.span,
                        );
                        continue;
                    }
                    self.type_aliases.insert(a.name.name.clone(), a.target.clone());
                }
                ItemKind::Function(_) | ItemKind::Impl(_) | ItemKind::Interface(_) => {}
            }
        }
        self.current_file = None;
    }

    fn type_name_taken(&self, name: &str) -> bool {
        self.enum_by_name.contains_key(name)
            || self.struct_by_name.contains_key(name)
            || self.type_aliases.contains_key(name)
    }

    /// Second pass: resolve struct field types and populate `StructDef.fields`.
    /// Detects duplicate field names (E0319).
    fn collect_struct_fields(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Struct(s) = &item.kind else { continue; };
            // Slice 7GEN.5c: skip generic struct templates — they
            // don't have concrete fields until instantiated.
            if !s.generic_params.is_empty() { continue; }
            let Some(&id) = self.struct_by_name.get(&s.name.name) else { continue; };
            // Slice 7GEN.4: generic-param names declared on the struct
            // (`struct Pair[A, B]`) are visible in field type positions.
            self.push_type_params(&s.generic_params);
            let mut seen: HashMap<String, ()> = HashMap::new();
            let mut fields: Vec<(String, Ty, bool)> = Vec::new();
            for f in &s.fields {
                if seen.contains_key(&f.name.name) {
                    self.err(
                        "E0319",
                        format!("duplicate field `{}` in struct `{}`", f.name.name, s.name.name),
                        f.name.span,
                    );
                    continue;
                }
                seen.insert(f.name.name.clone(), ());
                let ty = self.resolve_type(&f.ty);
                fields.push((f.name.name.clone(), ty, f.is_pub));
            }
            self.structs[id.0 as usize].fields = fields;
            self.pop_type_params();
        }
        self.current_file = None;
    }

    /// Compute `is_copy` for every user-defined struct: a struct is `Copy`
    /// iff every field type is `Copy`. The check is iterated to a fixpoint
    /// because struct A's `is_copy` may depend on struct B's, and the
    /// declaration order in source doesn't guarantee a useful evaluation
    /// order. Convergence: at most `N` iterations for `N` structs (each
    /// iteration either flips at least one struct's flag from false to true,
    /// or we stop). Once flipped to true, a flag never flips back — the rule
    /// is monotone.
    ///
    /// See `docs/design/phase3-copy-derivation.md`.
    fn compute_struct_copy_flags(&mut self) {
        loop {
            let mut changed = false;
            for i in 0..self.structs.len() {
                if self.structs[i].is_copy {
                    continue;
                }
                // `Drop` types are non-Copy regardless of fields — allowing
                // Copy on a Drop type would cause double-free. See
                // `docs/design/phase3-drop.md` §4.2.
                if self.structs[i].is_drop {
                    continue;
                }
                let all_fields_copy = self.structs[i]
                    .fields
                    .iter()
                    .all(|(_, ty, _)| self.is_copy(ty));
                if all_fields_copy {
                    self.structs[i].is_copy = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Decide whether a type is `Copy`. Structural auto-derive: every
    /// component must be `Copy`. Structs/enums use precomputed flags;
    /// arrays recurse on the element type. Plain enums (no payloads) are
    /// always Copy (matches the slice-2A bare-`i32` shape).
    pub fn is_copy(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Array(elem, _) => self.is_copy(elem),
            Ty::Struct(id) => self.structs[id.0 as usize].is_copy,
            Ty::Enum(id) => self.enums[id.0 as usize].is_copy,
            _ => ty.is_atomic_copy(),
        }
    }

    /// v0.0.4 Phase 2 Slice 2A: structural Send membership.
    ///
    /// v0.0.4 baseline: every type is Send. The check exists as a
    /// vocabulary anchor — generic signatures can declare `T: Send`
    /// bounds (e.g. `thread::spawn[O: Send]`) and the check accepts
    /// every type today, but the surface is forward-compatible with
    /// the tightening planned for future slices:
    /// - `Rc[T]`, `MutexGuard[T]`: explicit `!Send`.
    /// - Structs with raw-pointer fields: `!Send` unless the user
    ///   opts in via `unsafe impl Send for T {}`.
    ///
    /// Today the check returns true regardless. The bound itself is
    /// the documentation; enforcement tightens incrementally.
    pub fn is_send(&self, _ty: &Ty) -> bool {
        true
    }

    /// v0.0.4 Phase 2 Slice 2A: structural Sync membership. Same
    /// baseline + roadmap as `is_send` — vacuous true for now;
    /// tightening tracks per-type `!Sync` markers + structural
    /// inference for cells/refcells when those land.
    pub fn is_sync(&self, _ty: &Ty) -> bool {
        true
    }

    /// Slice 7GEN.5e step 4: verify each `(param, arg)` pair against
    /// the param's declared bounds at an instantiation site. Emits
    /// **E0502** for each violation, naming the offending bound, type,
    /// and the surrounding instantiation context.
    fn check_generic_bounds(
        &mut self,
        param_names: &[String],
        bounds: &[Vec<String>],
        args: &[Ty],
        span: ByteSpan,
        context_desc: &str,
    ) {
        // 7GEN.5c carry-forward (2026-05-13): when any arg is still
        // `Ty::Param`, we're inside a template-level reference (e.g.
        // the body of `impl Vec[T, A: Allocator]` mentions
        // `Vec[T, A] { ... }`). Bounds will be re-checked at every
        // concrete instantiation; checking them here would require a
        // bound-aware lookup against the surrounding impl scope, which
        // we don't track. Skip in that case — concrete-only enforcement
        // is sound because every Param eventually resolves at a real
        // call site that passes the args through this same check.
        if args.iter().any(|a| ty_contains_param(a, &self.structs, &self.enums)) {
            return;
        }
        for (i, arg_ty) in args.iter().enumerate() {
            let Some(param_bounds) = bounds.get(i) else { continue; };
            for b in param_bounds {
                if !self.satisfies_bound(arg_ty, b) {
                    let pname = param_names.get(i).map(|s| s.as_str()).unwrap_or("?");
                    self.err(
                        "E0502",
                        format!(
                            "type `{}` does not satisfy bound `{}` on type parameter `{}` of {}",
                            ty_display(arg_ty), b, pname, context_desc
                        ),
                        span,
                    );
                }
            }
        }
    }

    /// Slice 7GEN.5e step 4 + 7GEN.6: does `ty` satisfy `bound`?
    /// `Copy` is structurally checked via `is_copy`. Other blessed and
    /// user-declared interfaces use the `interface_impls` registry
    /// keyed by `(interface_name, target_struct_name)`. Built-in
    /// primitives never satisfy non-Copy bounds today (no `impl Ord
    /// for i32` is provided; users would have to wrap them in a
    /// newtype struct).
    pub fn satisfies_bound(&self, ty: &Ty, bound: &str) -> bool {
        if bound == "Copy" {
            return self.is_copy(ty);
        }
        // v0.0.4 Phase 2 Slice 2A: Send / Sync are structural marker
        // interfaces. v0.0.4 baseline is permissive — every type
        // satisfies both. Future slices tighten: Rc[T] / MutexGuard[T]
        // explicitly !Send, raw-pointer-bearing structs !Send unless
        // user opts in via `unsafe impl Send for T`. The bound API
        // is locked in now so generic signatures (`thread::spawn[O: Send]`)
        // compose forward-compatibly with future tightening.
        if bound == "Send" {
            return self.is_send(ty);
        }
        if bound == "Sync" {
            return self.is_sync(ty);
        }
        match ty {
            Ty::Struct(id) => {
                let name = &self.structs[id.0 as usize].name;
                self.interface_impls.contains(&(bound.to_string(), name.clone()))
            }
            Ty::Enum(id) => {
                let name = &self.enums[id.0 as usize].name;
                self.interface_impls.contains(&(bound.to_string(), name.clone()))
            }
            _ => false,
        }
    }

    /// Resolve variant payload types after all type names are known. This
    /// is the enum-side mirror of `collect_struct_fields`: it lets variant
    /// payloads forward-reference any struct or enum declared elsewhere
    /// in the program. See `docs/design/phase3-tagged-unions.md`.
    fn collect_enum_payloads(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Enum(e) = &item.kind else { continue; };
            // Slice 7GEN.5d: generic enum templates have no concrete
            // payloads until instantiation. Skip.
            if !e.generic_params.is_empty() { continue; }
            let Some(&id) = self.enum_by_name.get(&e.name.name) else { continue; };
            // Slice 7GEN.4: generic-param names declared on the enum
            // (`enum Option[T]`) are visible in variant payload types.
            self.push_type_params(&e.generic_params);
            // Walk source variants in declaration order; sema's
            // EnumVariantDef list mirrors the source list (modulo
            // duplicates which were skipped in step 1).
            let mut sema_idx = 0usize;
            for sv in &e.variants {
                if sema_idx >= self.enums[id.0 as usize].variants.len() { break; }
                // Skip variants whose names don't match — those were
                // duplicates rejected in pass 1.
                if self.enums[id.0 as usize].variants[sema_idx].name != sv.name.name {
                    continue;
                }
                let payload: Vec<Ty> = sv.payload.iter().map(|t| self.resolve_type(t)).collect();
                self.enums[id.0 as usize].variants[sema_idx].payload = payload;
                sema_idx += 1;
            }
            self.pop_type_params();
        }
        self.current_file = None;
    }

    /// Compute `is_copy` for every enum. A plain enum (`is_tagged == false`)
    /// is always Copy — same atomic-int rule as Phase 2A. A tagged enum is
    /// Copy iff every variant's payload type is Copy.
    ///
    /// Also enforces §3.3 of the tagged-unions design note: a tagged enum
    /// cannot have a Drop type as a payload in Phase 3 (E0344). Users who
    /// need this write a manual `fn drop(mut self) { match self { ... } }`
    /// on the tagged enum itself — but Phase-3 `impl` is struct-only, so
    /// the manual-drop escape hatch is also unavailable, making the rule
    /// effectively "no Drop payloads, full stop." Acceptable: Phase-3 has
    /// no heap types yet so Drop payloads aren't a real use case.
    fn compute_enum_copy_flags(&mut self, p: &Program) {
        // First pass: enforce the no-Drop-payload rule. Collect diagnostics
        // and the originating file id into a side list so we don't hold
        // an immutable borrow on self.enums while emitting and so we can
        // route each E0344 to the right file's line-map (slice 4C).
        // `self.enums` while we call `self.err` (which needs `&mut self`).
        struct DropDiag {
            enum_name: String,
            variant_name: String,
            span: ByteSpan,
            origin_file: Option<String>,
        }
        let mut diags: Vec<DropDiag> = Vec::new();
        for item in &p.items {
            let ItemKind::Enum(e) = &item.kind else { continue; };
            let Some(&id) = self.enum_by_name.get(&e.name.name) else { continue; };
            let def = &self.enums[id.0 as usize];
            if !def.is_tagged { continue; }
            for (vi, vdef) in def.variants.iter().enumerate() {
                for (pi, pty) in vdef.payload.iter().enumerate() {
                    if self.ty_carries_drop(pty) {
                        let span = e.variants.get(vi)
                            .and_then(|sv| sv.payload.get(pi).map(|t| t.span))
                            .unwrap_or(e.name.span);
                        diags.push(DropDiag {
                            enum_name: e.name.name.clone(),
                            variant_name: vdef.name.clone(),
                            span,
                            origin_file: item.origin_file.clone(),
                        });
                    }
                }
            }
        }
        for d in diags {
            self.current_file = d.origin_file;
            self.err(
                "E0344",
                format!(
                    "tagged-enum variant `{}::{}` has a `Drop`-typed payload, which Phase 3 does not support (no compiler-synthesized drop for tagged unions yet)",
                    d.enum_name, d.variant_name
                ),
                d.span,
            );
        }
        self.current_file = None;
        // Second pass: compute Copy flag. Fixpoint, monotone.
        loop {
            let mut changed = false;
            for i in 0..self.enums.len() {
                if self.enums[i].is_copy { continue; }
                let copy_now = if !self.enums[i].is_tagged {
                    true   // plain enum — always Copy
                } else {
                    let all_payloads_copy = self.enums[i].variants.iter()
                        .all(|v| v.payload.iter().all(|t| self.is_copy(t)));
                    all_payloads_copy
                };
                if copy_now {
                    self.enums[i].is_copy = true;
                    changed = true;
                }
            }
            if !changed { break; }
        }
    }

    /// True iff `ty` itself carries a destructor (its scope-exit drop is
    /// non-trivial). Mirror of `is_copy` for the Drop side. Used by the
    /// tagged-enum payload rule (§3.3 of the design note).
    fn ty_carries_drop(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Struct(id) => self.structs[id.0 as usize].is_drop,
            Ty::Array(elem, _) => self.ty_carries_drop(elem),
            // Plain/tagged enums never carry Drop in Phase 3 — Drop is
            // struct-only via `impl` blocks. (Once enum-impl lands this
            // rule generalizes.)
            _ => false,
        }
    }

    /// Third pass: collect methods from `impl` blocks. Runs after structs
    /// are fully typed so methods can reference any type by name. Reports
    /// E0325 (unknown / non-struct impl target) and E0326 (duplicate method).
    fn collect_methods(&mut self, p: &Program) {
        // v0.0.3 Slice 1P.2 — two-phase: register every generic-impl-method
        // template BEFORE resolving any concrete impl method signature.
        // Otherwise, an `impl Foo { fn bar() -> vec::Vec[u8] { ... } }` in
        // a downstream file triggers `Vec[u8]` instantiation while
        // `vec.cplus`'s `impl Vec[T] { ... }` hasn't been collected yet —
        // the new struct ends up methodless and `buf.push(...)` fires
        // E0324 even though sema sees the generic impl right there.
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Impl(b) = &item.kind else { continue; };
            if !b.target_generic_params.is_empty() {
                self.collect_generic_impl_methods(b);
            }
        }
        self.current_file = None;
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Impl(b) = &item.kind else { continue; };
            // Generic impls were handled in phase 1 above.
            if !b.target_generic_params.is_empty() {
                continue;
            }
            // Slice 7GEN.4: skip impls whose target is an interface — those
            // are handled by `validate_interface_impls`. Inherent impls
            // (`impl Type { ... }`) still flow through this pass.
            let Some(&id) = self.struct_by_name.get(&b.target.name) else {
                if self.enum_by_name.contains_key(&b.target.name) {
                    self.err(
                        "E0325",
                        format!("`impl` on enum type `{}` is not yet supported (Phase 2 supports inherent methods on structs only)", b.target.name),
                        b.target.span,
                    );
                } else {
                    self.err(
                        "E0325",
                        format!("`impl` target `{}` is not a known type", b.target.name),
                        b.target.span,
                    );
                }
                continue;
            };
            // Slice 7GEN.4: `Self` inside an impl body resolves to the
            // target type's concrete `Ty`. Push for the duration of this
            // impl block so method-signature resolution sees it.
            self.self_type_stack.push(Ty::Struct(id));
            for m in &b.methods {
                // Slice 7GEN.5e: push method-level generic params onto the
                // type-param scope so `resolve_type` recognizes them as
                // `Ty::Param(name)` rather than firing E0303.
                let mut mscope = std::collections::HashSet::new();
                for gp in &m.generic_params { mscope.insert(gp.name.name.clone()); }
                self.type_params_stack.push(mscope);
                let params: Vec<ParamSig> = m.params.iter().map(|p| ParamSig {
                    ty: self.resolve_type(&p.ty),
                    mutable: p.mutable,
                    move_: p.move_,
                }).collect();
                let return_type = match &m.return_type {
                    Some(t) => self.resolve_type(t),
                    None => Ty::Unit,
                };
                self.type_params_stack.pop();
                if self.structs[id.0 as usize].methods.contains_key(&m.name.name) {
                    self.err(
                        "E0326",
                        format!("duplicate method `{}` in impl `{}`", m.name.name, b.target.name),
                        m.name.span,
                    );
                    continue;
                }
                // `drop` is the destructor: the signature must be exactly
                // `fn drop(mut self)` (mutable receiver, no extra params, no
                // return type). Defining a method named `drop` marks the
                // struct as `Drop`, which forces non-Copy in
                // `compute_struct_copy_flags`. See
                // `docs/design/phase3-drop.md`.
                if m.name.name == "drop" {
                    let recv_ok = matches!(m.receiver, Some(Receiver::Mut));
                    let no_extra_params = params.is_empty();
                    let no_return = matches!(return_type, Ty::Unit);
                    if !recv_ok || !no_extra_params || !no_return {
                        self.err(
                            "E0338",
                            format!("destructor `{}::drop` must have signature `fn drop(mut self)` — no extra parameters, no return type", b.target.name),
                            m.name.span,
                        );
                    } else {
                        self.structs[id.0 as usize].is_drop = true;
                    }
                }
                let generic_params: Vec<String> = m.generic_params.iter()
                    .map(|gp| gp.name.name.clone())
                    .collect();
                let generic_bounds: Vec<Vec<String>> = m.generic_params.iter()
                    .map(|gp| gp.bounds.iter().map(|b| b.name.clone()).collect())
                    .collect();
                self.structs[id.0 as usize].methods.insert(
                    m.name.name.clone(),
                    MethodSig { receiver: m.receiver, params, return_type, generic_params, generic_bounds },
                );
            }
            self.self_type_stack.pop();
        }
        self.current_file = None;
    }

    /// Slice 7GEN.5e step 3: route methods declared inside a generic-typed
    /// impl block (`impl Vec[T] { ... }`) into `generic_impl_methods`.
    /// They are materialized as concrete `MethodSig`s by
    /// `populate_generic_impl_methods` whenever the template is
    /// instantiated (`Vec[i32]`, `Vec[bool]`, ...).
    fn collect_generic_impl_methods(&mut self, b: &ImplBlock) {
        // Verify the target is a known generic struct or enum template.
        // Today we only allow struct targets; enum-side generic impls
        // are a follow-up.
        if !self.struct_generic_templates.contains_key(&b.target.name) {
            if self.enum_generic_templates.contains_key(&b.target.name) {
                self.err(
                    "E0325",
                    format!("`impl` on generic enum `{}` is not yet supported", b.target.name),
                    b.target.span,
                );
            } else {
                self.err(
                    "E0325",
                    format!("`impl` target `{}` is not a known generic type", b.target.name),
                    b.target.span,
                );
            }
            return;
        }
        // Push impl-level generic params onto the type-param stack so
        // method param/return types can reference `T`.
        let impl_param_names: Vec<String> = b.target_generic_params.iter()
            .map(|g| g.name.name.clone())
            .collect();
        let mut impl_scope = std::collections::HashSet::new();
        for n in &impl_param_names { impl_scope.insert(n.clone()); }
        self.type_params_stack.push(impl_scope);
        // `Self` inside the impl body resolves to the (uninstantiated)
        // generic — represent it as Ty::Param("Self") for now;
        // substitution to a concrete struct happens per instantiation.
        self.self_type_stack.push(Ty::Param("Self".to_string()));
        let mut templates: Vec<GenericImplMethodTemplate> = Vec::with_capacity(b.methods.len());
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for m in &b.methods {
            if !seen.insert(m.name.name.clone()) {
                self.err(
                    "E0326",
                    format!("duplicate method `{}` in impl `{}`", m.name.name, b.target.name),
                    m.name.span,
                );
                continue;
            }
            // Method-level generic params.
            let method_param_names: Vec<String> = m.generic_params.iter()
                .map(|g| g.name.name.clone()).collect();
            let mut method_scope = std::collections::HashSet::new();
            for n in &method_param_names { method_scope.insert(n.clone()); }
            self.type_params_stack.push(method_scope);
            let params: Vec<ParamSig> = m.params.iter().map(|p| ParamSig {
                ty: self.resolve_type(&p.ty),
                mutable: p.mutable,
                move_: p.move_,
            }).collect();
            let return_type = match &m.return_type {
                Some(t) => self.resolve_type(t),
                None => Ty::Unit,
            };
            self.type_params_stack.pop();
            templates.push(GenericImplMethodTemplate {
                name: m.name.name.clone(),
                receiver: m.receiver,
                params,
                return_type,
                impl_generic_params: impl_param_names.clone(),
                method_generic_params: method_param_names,
                is_drop: false,
            });
        }
        self.self_type_stack.pop();
        self.type_params_stack.pop();
        self.generic_impl_methods
            .entry(b.target.name.clone())
            .or_insert_with(Vec::new)
            .extend(templates);
    }

    /// Slice 7GEN.4: collect interface declarations into the
    /// `interfaces` registry. Method signatures are resolved with a
    /// magic `Self` type-param frame pushed so any occurrence of `Self`
    /// in a method signature becomes `Ty::Param("Self")`. Substitution
    /// to a concrete type happens at impl-validation time.
    ///
    /// Reports:
    /// - **E0301** — duplicate interface name (shares the type-namespace
    ///   collision rule with structs/enums).
    /// Slice 7GEN.6: register the compiler-blessed interfaces
    /// (`Copy`, `Eq`, `Ord`, `Hash`, `Clone`) into the interfaces
    /// table. Users implement most of them manually; `Copy` is a
    /// marker that's structurally inferred (manual `impl Copy for X`
    /// is rejected with E0510).
    ///
    /// Method signatures use `Ty::Param("Self")` for the implementing
    /// type, matching the convention used by user-declared interfaces.
    fn register_blessed_interfaces(&mut self) {
        // Copy: marker, no methods.
        self.interfaces.insert(
            "Copy".to_string(),
            InterfaceDef { name: "Copy".to_string(), methods: HashMap::new(), origin_file: None },
        );
        // v0.0.4 Phase 2 Slice 2A: Send and Sync — marker interfaces
        // gating cross-thread transfer (Send) and cross-thread sharing
        // (Sync). Same shape as Copy: no methods, structurally inferred
        // via `is_send` / `is_sync`. v0.0.4 starts permissive (every
        // type satisfies both); future slices tighten by marking
        // specific types (`Rc[T]`, `MutexGuard[T]`) as `!Send` and
        // raw-pointer-containing types as `!Send` unless explicitly
        // opted-in.
        self.interfaces.insert(
            "Send".to_string(),
            InterfaceDef { name: "Send".to_string(), methods: HashMap::new(), origin_file: None },
        );
        self.interfaces.insert(
            "Sync".to_string(),
            InterfaceDef { name: "Sync".to_string(), methods: HashMap::new(), origin_file: None },
        );
        // Single-method interfaces with shared shape.
        // (name, method_name, return_type, takes_other_param)
        let single: &[(&str, &str, Ty, bool)] = &[
            ("Eq",       "eq",        Ty::Bool, true),
            ("Ord",      "cmp",       Ty::I32,  true),
            ("Hash",     "hash",      Ty::U64,  false),
            ("Clone",    "clone",     Ty::Param("Self".to_string()), false),
            // Phase 8 slice 8.STR.6: `ToString` — produces an owned
            // string. Blessed impls cover every primitive + str +
            // string; user types add their own via the usual
            // `impl ToString for Foo { fn to_string(self) -> string }`
            // surface.
            ("ToString", "to_string", Ty::String, false),
        ];
        for (name, mname, ret, has_other) in single {
            let mut methods = HashMap::new();
            let params: Vec<ParamSig> = if *has_other {
                vec![ParamSig { ty: Ty::Param("Self".to_string()), mutable: false, move_: false }]
            } else {
                Vec::new()
            };
            methods.insert((*mname).to_string(), MethodSig {
                receiver: Some(Receiver::Read),
                params,
                return_type: ret.clone(),
                generic_params: Vec::new(),
                generic_bounds: Vec::new(),
            });
            self.interfaces.insert(
                (*name).to_string(),
                InterfaceDef { name: (*name).to_string(), methods, origin_file: None },
            );
        }
    }

    fn collect_interfaces(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Interface(idecl) = &item.kind else { continue; };
            // Name collision with any other type in scope (struct, enum,
            // or another interface).
            if self.type_name_taken(&idecl.name.name)
                || self.interfaces.contains_key(&idecl.name.name)
            {
                self.err(
                    "E0301",
                    format!("duplicate type definition `{}`", idecl.name.name),
                    idecl.name.span,
                );
                continue;
            }
            // Push a one-element type-param frame for `Self` so method
            // signatures resolve uniformly. Method signatures are then
            // stored with `Ty::Param("Self")` standing in for the
            // implementing type.
            let mut self_frame = std::collections::HashSet::new();
            self_frame.insert("Self".to_string());
            self.type_params_stack.push(self_frame);
            let mut methods: HashMap<String, MethodSig> = HashMap::new();
            for m in &idecl.methods {
                let params: Vec<ParamSig> = m.params.iter().map(|p| ParamSig {
                    ty: self.resolve_type(&p.ty),
                    mutable: p.mutable,
                    move_: p.move_,
                }).collect();
                let return_type = match &m.return_type {
                    Some(t) => self.resolve_type(t),
                    None => Ty::Unit,
                };
                if methods.contains_key(&m.name.name) {
                    self.err(
                        "E0326",
                        format!("duplicate method `{}` in interface `{}`", m.name.name, idecl.name.name),
                        m.name.span,
                    );
                    continue;
                }
                methods.insert(
                    m.name.name.clone(),
                    MethodSig { receiver: m.receiver, params, return_type, generic_params: Vec::new(), generic_bounds: Vec::new() },
                );
            }
            self.type_params_stack.pop();
            self.interfaces.insert(
                idecl.name.name.clone(),
                InterfaceDef {
                    name: idecl.name.name.clone(),
                    methods,
                    origin_file: item.origin_file.clone(),
                },
            );
        }
        self.current_file = None;
    }

    /// Slice 7GEN.4: validate every `impl Interface for Type { ... }` block.
    ///
    /// Reports:
    /// - **E0503** — interface impl missing a required method.
    /// - **E0504** — interface impl has a method not declared by the interface.
    /// - **E0505** — interface method signature mismatch (with `Self`
    ///   substituted to the implementing type).
    /// - **E0506** — duplicate `impl Interface for Type` for the same pair.
    /// - **E0507** — orphan rule: impl must live in the same file as
    ///   either the interface or the implementing type.
    ///
    /// Note: this pass runs *after* `collect_methods`, which already
    /// inserted every method from every impl block (inherent or
    /// interface) into the target struct's method table. This pass
    /// validates structural conformance against the interface; the
    /// methods are callable on the type either way.
    fn validate_interface_impls(&mut self, p: &Program) {
        // Defer all diagnostics into a side list keyed by origin_file
        // because we hold immutable borrows on the interface and struct
        // tables while computing them.
        struct Diag {
            code: &'static str,
            msg: String,
            span: ByteSpan,
            origin_file: Option<String>,
        }
        let mut diags: Vec<Diag> = Vec::new();
        for item in &p.items {
            let ItemKind::Impl(b) = &item.kind else { continue; };
            let Some(iface_name) = b.interface_name.as_ref() else { continue; };
            // Slice 7GEN.6: `Copy` is structurally inferred — manual
            // `impl Copy for X` is rejected with E0510. The structural
            // Copy flag (`is_copy` on StructDef/EnumDef) is what
            // satisfies the `T: Copy` bound at use sites.
            if iface_name.name == "Copy" {
                diags.push(Diag {
                    code: "E0510",
                    msg: "`Copy` cannot be manually implemented; it is structurally inferred from field types".to_string(),
                    span: iface_name.span,
                    origin_file: item.origin_file.clone(),
                });
                continue;
            }
            // E0507 / E0506 / E0503 / E0504 / E0505 are interface-impl rules.
            let Some(iface) = self.interfaces.get(&iface_name.name) else {
                diags.push(Diag {
                    code: "E0303",
                    msg: format!("unknown interface `{}`", iface_name.name),
                    span: iface_name.span,
                    origin_file: item.origin_file.clone(),
                });
                continue;
            };
            // Resolve the target type. Interface-for-target must be a
            // known struct (Phase 7 first cut — interface-for-enum or
            // interface-for-builtin lands later).
            let Some(&target_id) = self.struct_by_name.get(&b.target.name) else {
                diags.push(Diag {
                    code: "E0325",
                    msg: format!("`impl {} for {}` — `{}` is not a known struct", iface_name.name, b.target.name, b.target.name),
                    span: b.target.span,
                    origin_file: item.origin_file.clone(),
                });
                continue;
            };
            let target_def = &self.structs[target_id.0 as usize];
            // E0507 — orphan rule: the impl's origin_file must match
            // either the interface's or the target's origin_file. In
            // single-file mode every item has `None` for origin_file,
            // which trivially satisfies the rule (None == None).
            let impl_file = &item.origin_file;
            let iface_file = &iface.origin_file;
            let target_file = &target_def.origin_file;
            if impl_file != iface_file && impl_file != target_file {
                diags.push(Diag {
                    code: "E0507",
                    msg: format!(
                        "orphan `impl {} for {}` — must be declared in the same file as either the interface or the type",
                        iface_name.name, b.target.name
                    ),
                    span: iface_name.span,
                    origin_file: item.origin_file.clone(),
                });
                // Fall through to other checks — diagnostics compose.
            }
            // E0506 — duplicate (interface, type) pair.
            let pair = (iface_name.name.clone(), b.target.name.clone());
            if !self.interface_impls.insert(pair.clone()) {
                diags.push(Diag {
                    code: "E0506",
                    msg: format!(
                        "duplicate `impl {} for {}` — a type may have at most one impl of any given interface",
                        iface_name.name, b.target.name
                    ),
                    span: iface_name.span,
                    origin_file: item.origin_file.clone(),
                });
                continue;
            }
            // Walk every declared interface method; verify the target
            // has a matching impl with the right signature (after
            // substituting Self -> target).
            let target_ty = Ty::Struct(target_id);
            for (mname, iface_sig) in &iface.methods {
                let Some(impl_sig) = target_def.methods.get(mname) else {
                    // E0503 — missing method.
                    let span = b.methods.iter().next().map(|m| m.name.span)
                        .unwrap_or(b.target.span);
                    diags.push(Diag {
                        code: "E0503",
                        msg: format!(
                            "`impl {} for {}` is missing method `{}` required by interface",
                            iface_name.name, b.target.name, mname
                        ),
                        span,
                        origin_file: item.origin_file.clone(),
                    });
                    continue;
                };
                // E0505 — signature equality after Self substitution.
                if !method_sig_matches(iface_sig, impl_sig, &target_ty) {
                    let span = b.methods.iter()
                        .find(|m| &m.name.name == mname)
                        .map(|m| m.name.span)
                        .unwrap_or(b.target.span);
                    diags.push(Diag {
                        code: "E0505",
                        msg: format!(
                            "method `{}` in `impl {} for {}` does not match the interface signature",
                            mname, iface_name.name, b.target.name
                        ),
                        span,
                        origin_file: item.origin_file.clone(),
                    });
                }
            }
            // E0504 — impl block has methods not declared in the interface.
            // Walk the block's methods directly (not the target's full
            // method table, which would also include inherent methods
            // from sibling `impl Type { ... }` blocks).
            for m in &b.methods {
                if !iface.methods.contains_key(&m.name.name) {
                    diags.push(Diag {
                        code: "E0504",
                        msg: format!(
                            "method `{}` in `impl {} for {}` is not declared by the interface; move it to an inherent `impl {} {{ ... }}` block",
                            m.name.name, iface_name.name, b.target.name, b.target.name
                        ),
                        span: m.name.span,
                        origin_file: item.origin_file.clone(),
                    });
                }
            }
        }
        // Emit collected diagnostics.
        for d in diags {
            self.current_file = d.origin_file;
            self.err(d.code, d.msg, d.span);
        }
        self.current_file = None;
    }

    /// Type-check every method body. Runs after function bodies.
    fn check_methods(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Impl(b) = &item.kind else { continue; };
            let Some(&id) = self.struct_by_name.get(&b.target.name) else { continue; };
            // Slice 4C: per-item context. Methods inherit their impl
            // block's origin_file — every impl block lives in the same
            // file as its type (enforced by the resolver).
            self.current_file = item.origin_file.clone();
            for m in &b.methods {
                self.check_method(id, m);
            }
        }
        self.current_file = None;
    }

    fn check_method(&mut self, struct_id: StructId, m: &Method) {
        let Some(sig) = self.structs[struct_id.0 as usize].methods.get(&m.name.name).cloned() else {
            return;
        };
        // Slice 7GEN.4: re-push the impl's `Self` mapping so `Self`
        // references in the method body resolve to the target type.
        self.self_type_stack.push(Ty::Struct(struct_id));
        // Slice 7GEN.5e: re-push method-level generic params for
        // body checking so `T` references in the body resolve.
        self.push_type_params(&m.generic_params);
        self.current_return = sig.return_type.clone();
        self.scopes.push(HashMap::new());

        // Register `self` if there's a receiver. `mut self` makes self
        // a mutable binding (enables `self.x = ...`); other forms don't.
        // `move self` is read-only inside the body — consumption happens at
        // the call site, not from within.
        if let Some(rcv) = sig.receiver {
            let mutable = matches!(rcv, Receiver::Mut);
            self.scopes.last_mut().unwrap().insert(
                "self".to_string(),
                LocalInfo { ty: Ty::Struct(struct_id), mutable, moved: false, assigned: true },
            );
        }
        // Register non-receiver params.
        for (param, psig) in m.params.iter().zip(sig.params.iter()) {
            // E0334: `mut` and `move` are mutually exclusive ownership markers.
            if param.mutable && param.move_ {
                self.err(
                    "E0334",
                    "parameter cannot have both `mut` and `move`; these markers are mutually exclusive".to_string(),
                    param.span,
                );
            }
            self.scopes.last_mut().unwrap().insert(
                param.name.name.clone(),
                LocalInfo { ty: psig.ty.clone(), mutable: param.mutable, moved: false, assigned: true },
            );
        }
        self.check_function_body(&m.body, sig.return_type, m.body.span);
        self.scopes.pop();
        self.pop_type_params();
        self.self_type_stack.pop();
    }

    fn collect_functions(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Function(f) = &item.kind else { continue; };
            // Slice 10.FFI.3: record extern fns so call sites can gate
            // them behind `unsafe { ... }`. The signature still goes
            // into `fns` (so calls type-check normally); the membership
            // set just drives the unsafe gate.
            if f.is_extern {
                self.extern_fns.insert(f.name.name.clone());
            }
            // Phase 11 / ObjC interop: `#[link_name = "..."]` symbol alias.
            // Extract here once and stash on FnSig; gate placement on extern.
            let link_name = extract_link_name(&f.attributes);
            if link_name.is_some() && !f.is_extern {
                // Find the attribute's span for the diagnostic primary.
                let attr_span = f.attributes.iter()
                    .find(|a| a.path.name == "link_name")
                    .map(|a| a.span)
                    .unwrap_or(f.name.span);
                self.err(
                    "E0356",
                    "`#[link_name]` is only valid on `extern fn` declarations".to_string(),
                    attr_span,
                );
            }
            // Slice 7GEN.4: declared generic params (`fn id[T](x: T)`)
            // are visible while resolving the function's parameter and
            // return types.
            self.push_type_params(&f.generic_params);
            let params: Vec<ParamSig> = f.params.iter().map(|p| ParamSig {
                ty: self.resolve_type(&p.ty),
                mutable: p.mutable,
                move_: p.move_,
            }).collect();
            let declared_ret = match &f.return_type {
                Some(t) => self.resolve_type(t),
                None => Ty::Unit,
            };
            // v0.0.3 Phase 5 Slice 5E.2: `async fn foo() -> T` exposes
            // `Future[T]` at the signature level. Callers see the
            // wrapped type; the body still type-checks `return X`
            // against the inner T (handled in `check_function_body`).
            // The `Future` template must be in scope — it lives at
            // `stdlib/future.cplus` and is imported automatically by
            // anything that uses `async fn`.
            let ret = if f.is_async {
                self.wrap_in_future(&declared_ret, f.name.span)
            } else {
                declared_ret.clone()
            };
            self.pop_type_params();
            // Slice 7GEN.5a: generic fns go into a separate table.
            // Call-site inference produces concrete instantiations that
            // monomorphize emits.
            if !f.generic_params.is_empty() {
                if self.fns_generic.contains_key(&f.name.name)
                    || self.fns.contains_key(&f.name.name)
                {
                    self.err(
                        "E0301",
                        format!("duplicate function definition `{}`", f.name.name),
                        f.name.span,
                    );
                    continue;
                }
                self.fns_generic.insert(
                    f.name.name.clone(),
                    GenericFnSig {
                        generic_params: f.generic_params.iter().map(|g| g.name.name.clone()).collect(),
                        bounds: f.generic_params.iter()
                            .map(|g| g.bounds.iter().map(|b| b.name.clone()).collect())
                            .collect(),
                        params,
                        return_type: ret,
                    },
                );
                continue;
            }
            if self.fns.contains_key(&f.name.name) || self.fns_generic.contains_key(&f.name.name) {
                // Phase 1 stdlib note: when both declarations are `extern fn`
                // they target the same external symbol; allow the duplicate
                // silently (e.g. multiple stdlib modules declaring `extern fn
                // write` to reach libc::write). The first declaration wins
                // and stays in `self.fns`. Non-extern duplicates still error.
                if f.is_extern && self.extern_fns.contains(&f.name.name) {
                    continue;
                }
                self.err(
                    "E0301",
                    format!("duplicate function definition `{}`", f.name.name),
                    f.name.span,
                );
                continue;
            }
            self.fns.insert(f.name.name.clone(), FnSig { params, return_type: ret, is_variadic: f.is_variadic, link_name: link_name.clone() });
        }
        self.current_file = None;
    }

    /// v0.0.3 Phase 5 Slice 5E.2: look up the `Future` template from
    /// the user's imported `stdlib/future` module and instantiate it
    /// with one type argument. Returns `Ty::Error` if the template
    /// isn't visible — usually because the project didn't import
    /// stdlib's future module or didn't depend on stdlib at all.
    fn wrap_in_future(&mut self, inner: &Ty, span: ByteSpan) -> Ty {
        // v0.0.3 Phase 5 Slice 5E.3: the resolver qualifies struct
        // names per-file (`<file_id>.Future`), so a bare-name lookup
        // misses imports. Suffix-match `.Future` (or the bare name in
        // single-file builds) like Slice 5B's JoinHandle path.
        let key = self.struct_generic_templates.keys()
            .find(|k| k.as_str() == "Future" || k.ends_with(".Future"))
            .cloned();
        let template_name = match key {
            Some(k) => k,
            None => {
                self.err(
                    "E0300",
                    "`async fn` requires `Future[T]` from `stdlib/future`".to_string(),
                    span,
                );
                return Ty::Error;
            }
        };
        let template = self.struct_generic_templates.get(&template_name).cloned().unwrap();
        self.instantiate_struct_from_arg_tys(&template_name, &template, vec![inner.clone()])
    }

    /// v0.0.3 Phase 5 Slice 5E.2: given a `Ty::Struct(id)` for a
    /// `Future[T]` instantiation, return T. Returns `None` if the
    /// type isn't a Future. Used by `await EXPR` type-checking and
    /// by async-body return-type checks.
    fn unwrap_future(&self, ty: &Ty) -> Option<Ty> {
        // Match on file-qualified template names (see `wrap_in_future`).
        match ty {
            Ty::Struct(id) => {
                let def = &self.structs[id.0 as usize];
                match &def.generic_origin {
                    Some((name, args))
                        if (name == "Future" || name.ends_with(".Future"))
                            && args.len() == 1 => Some(args[0].clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn check_main_signature(&mut self, p: &Program) {
        let Some(sig) = self.fns.get("main").cloned() else { return; };
        let Some((no_params, span, origin)) = p.items.iter().find_map(|it| {
            let ItemKind::Function(f) = &it.kind else { return None; };
            (f.name.name == "main").then(|| (f.params.is_empty(), f.name.span, it.origin_file.clone()))
        }) else { return; };
        self.current_file = origin;
        // If we already errored resolving the return type, don't pile on.
        if sig.return_type == Ty::Error { return; }
        if !no_params || sig.return_type != Ty::I32 {
            self.err(
                "E0309",
                "`main` must have signature `fn main() -> i32` in Phase 1".to_string(),
                span,
            );
        }
        self.current_file = None;
    }

    fn check_functions(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Function(f) = &item.kind else { continue; };
            // Slice 4C: per-item context for field-pub gate.
            self.current_file = item.origin_file.clone();
            self.check_function(f);
        }
        self.current_file = None;
    }

    /// Phase 5 slice 5ATTR.2 — validate sema-level rules for `#[test]` fns:
    /// - **E0358**: test function must have signature `fn() -> i32` or `fn()`.
    ///   No parameters, return type must be unit or `i32` (other types reject).
    /// - **E0359**: test functions cannot be `pub`. Tests are project-internal
    ///   helpers discovered by the runner, never part of the exported API.
    ///
    /// E0360 (`#[test]` inside `impl`) is already caught at the attrs-pass
    /// layer (E0356 on method placement) — fires before sema sees this; the
    /// code is reserved by [docs/design/phase5-attributes.md](../../docs/design/phase5-attributes.md)
    /// in case a future refactor needs a sema-level fallback. Not emitted here.
    fn check_test_attribute_rules(&mut self, f: &Function, sig: &FnSig) {
        // Find the #[test] attribute (if any) — span is used for diagnostic
        // primary location so the error points at the attribute, not the fn.
        let test_attr = f.attributes.iter().find(|a| a.path.name == "test");
        let Some(attr) = test_attr else { return; };
        // E0359 — `pub` rejection.
        if f.is_pub {
            self.err(
                "E0359",
                "test functions cannot be `pub`; tests are project-internal".to_string(),
                attr.span,
            );
        }
        // E0358 — signature.
        let params_ok = f.params.is_empty();
        let return_ok = matches!(sig.return_type, Ty::Unit | Ty::I32);
        if !params_ok || !return_ok {
            self.err(
                "E0358",
                "test function must have signature `fn() -> i32` or `fn()`".to_string(),
                attr.span,
            );
        }
    }

    fn check_function(&mut self, f: &Function) {
        // Phase 5 Slice 5.C: `pub extern fn name(...) { body }` is a
        // C-callable export definition (not an import declaration). Its
        // signature must use only C-ABI-compatible types; the body
        // type-checks normally like any other fn. Plain `extern fn ...;`
        // is an import declaration — no body, no signature check beyond
        // the standard "types resolve" pass already done in collection.
        if f.is_extern {
            // Parser invariant: `is_pub` on an extern fn marks it as the
            // export form (with body); plain `extern fn ...;` always
            // parses with `is_pub = false`. So `is_extern && is_pub` is
            // exactly the export case.
            if f.is_pub {
                self.check_extern_export_signature(f);
                // Fall through to the normal body-checking path below.
            } else {
                // Import declaration: nothing more to check here.
                return;
            }
        }
        let sig = self.fns.get(&f.name.name).cloned();
        let Some(sig) = sig else { return; }; // duplicate def already errored
        // Phase 5 slice 5ATTR.2 — sema rules specific to `#[test]` fns.
        self.check_test_attribute_rules(f, &sig);
        // Slice 7GEN.4: generic params remain in scope across body checking.
        self.push_type_params(&f.generic_params);
        // v0.0.3 Phase 5 Slice 5E.2: async fn body sees the UNWRAPPED
        // return type (the user's declared T), not the `Future[T]`
        // that the signature exposes to callers. The wrap happens at
        // codegen time (Slice 5E.3).
        let body_return = if f.is_async {
            self.unwrap_future(&sig.return_type).unwrap_or_else(|| sig.return_type.clone())
        } else {
            sig.return_type.clone()
        };
        self.current_return = body_return.clone();
        let prev_async = self.current_fn_is_async;
        self.current_fn_is_async = f.is_async;
        self.scopes.push(HashMap::new());
        for (param, psig) in f.params.iter().zip(sig.params.iter()) {
            // E0334: `mut` and `move` are mutually exclusive ownership markers.
            if param.mutable && param.move_ {
                self.err(
                    "E0334",
                    "parameter cannot have both `mut` and `move`; these markers are mutually exclusive".to_string(),
                    param.span,
                );
            }
            // v0.0.4 Phase 1D — E0900 borrow-across-await guard.
            //
            // Async fns suspend at every `await`. Anything live across a
            // suspension must survive in the coroutine frame (which LLVM
            // promotes to the heap). Parameters that are *borrows of
            // caller data* — fat-pointer-shaped (`str`, `T[]`) or
            // mutably-borrowed (`mut x: NonCopyT`, which is pointer-passed
            // per Phase-6 ABI) — risk pointing into a caller's stack
            // frame that doesn't survive the suspension. v0.0.3's
            // executor is compute-only so the hazard is latent; v0.0.4's
            // reactor (Phase 3) lets coroutines actually resume on a
            // different stack frame, making this a live UAF.
            //
            // The check is a parameter-shape gate rather than dataflow
            // because: (a) v0.0.3 has no `&T` references, so the only
            // borrow surface is parameters of these shapes; (b) banning
            // the shape outright forces async fns to be owned-data-only,
            // which is the right default; (c) escape hatches exist
            // (`string`, `Vec[T]`, owned types) for every banned case.
            //
            // Forbidden in async-fn param position:
            //   - `Ty::Str` (fat ptr borrowing into a string)
            //   - `Ty::Slice(_)` (fat ptr borrowing into an array/vec)
            //   - `mut x: NonCopyT` (pointer-passed by Phase-6 ABI)
            if f.is_async {
                let pty = &psig.ty;
                let is_borrow_shape = matches!(pty, Ty::Str | Ty::Slice(_));
                let is_mut_pointer_passed = param.mutable
                    && !param.move_
                    && !self.is_copy(pty);
                if is_borrow_shape {
                    self.err(
                        "E0900",
                        format!(
                            "parameter `{}` has borrow-shaped type `{}` which is not allowed in `async fn` — borrows live across `await` may dangle once the reactor lands (Phase 3). Use an owned type instead (`string` for `str`, `Vec[T]` for `T[]`).",
                            param.name.name, ty_display(pty),
                        ),
                        param.span,
                    );
                }
                if is_mut_pointer_passed {
                    self.err(
                        "E0900",
                        format!(
                            "parameter `{}: {}` is `mut`-bound (pointer-passed) in an `async fn`; that storage may not outlive an `await`. Drop the `mut` and bind locally (`let mut x = x;`) or move ownership in with `move`.",
                            param.name.name, ty_display(pty),
                        ),
                        param.span,
                    );
                }
            }
            self.scopes.last_mut().unwrap().insert(
                param.name.name.clone(),
                LocalInfo { ty: psig.ty.clone(), mutable: param.mutable, moved: false, assigned: true },
            );
        }
        self.check_function_body(&f.body, body_return, f.body.span);
        self.scopes.pop();
        self.current_fn_is_async = prev_async;
        self.pop_type_params();
    }

    /// Function body: must produce a value matching the return type, OR end
    /// with an explicit `return`. Phase-1 heuristic; full divergence analysis
    /// is Phase 3 work.
    /// Phase 5 Slice 5.C: validate that every type in a `pub extern fn`
    /// signature is C-ABI-compatible. Each rejected type emits E0410 at
    /// the parameter's (or return-type's) span, with the unsupported
    /// type named in the message + a suggestion for the conventional
    /// workaround. Body type-checking continues afterward.
    fn check_extern_export_signature(&mut self, f: &Function) {
        // Re-resolve each surface type so we can diagnose against the
        // structural `Ty`. (Sema also cached these in `self.fns` during
        // collection — we use resolve_type here for span fidelity.)
        for p in &f.params {
            let pty = self.resolve_type(&p.ty);
            if let Some(reason) = self.c_exportable_diagnosis(&pty, /*is_return=*/false) {
                self.err(
                    "E0410",
                    format!(
                        "type `{}` in `pub extern fn` parameter is not C-ABI compatible: {reason}",
                        ty_display(&pty),
                    ),
                    p.span,
                );
            }
        }
        if let Some(rt) = &f.return_type {
            let ret_ty = self.resolve_type(rt);
            if let Some(reason) = self.c_exportable_diagnosis(&ret_ty, /*is_return=*/true) {
                self.err(
                    "E0410",
                    format!(
                        "type `{}` in `pub extern fn` return position is not C-ABI compatible: {reason}",
                        ty_display(&ret_ty),
                    ),
                    rt.span,
                );
            }
        }
    }

    /// Returns `Some(reason)` if `ty` is NOT representable across a C
    /// function-call ABI, `None` if it is. The reason string is the
    /// human-readable explanation appended to E0410.
    ///
    /// `is_return = true` allows `Ty::Unit` (a fn returning nothing);
    /// `is_return = false` rejects it (no such thing as a `void` param).
    fn c_exportable_diagnosis(&self, ty: &Ty, is_return: bool) -> Option<String> {
        match ty {
            // Primitives, raw pointers, function pointers — all single-
            // register classes that match the C ABI on every target.
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64
            | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
            | Ty::Isize | Ty::Usize
            | Ty::F32 | Ty::F64
            | Ty::Bool
            | Ty::RawPtr(_) => None,
            Ty::FnPtr { params, return_type } => {
                for p in params {
                    if let Some(r) = self.c_exportable_diagnosis(p, false) {
                        return Some(format!("function-pointer parameter type `{}` is not C-ABI compatible ({r})", ty_display(p)));
                    }
                }
                if let Some(r) = self.c_exportable_diagnosis(return_type, true) {
                    return Some(format!("function-pointer return type `{}` is not C-ABI compatible ({r})", ty_display(return_type)));
                }
                None
            }
            Ty::Unit => {
                if is_return { None } else {
                    Some("the unit type `()` has no C-ABI representation as a parameter".to_string())
                }
            }
            Ty::Str => Some(
                "`str` is a fat pointer with no C-ABI counterpart; pass a `*u8` and a `usize` length instead".to_string()
            ),
            Ty::String => Some(
                "owned `string` has Drop and a 3-word layout that no C ABI describes; pass a `*u8` and a `usize` length (and document the ownership convention)".to_string()
            ),
            Ty::Slice(_) => Some(
                "slice `T[]` is a fat pointer with no C-ABI counterpart; pass a `*T` and a `usize` length instead".to_string()
            ),
            Ty::Enum(id) => {
                let def = &self.enums[id.0 as usize];
                if def.is_tagged {
                    Some(format!(
                        "tagged enum `{}` has no C-ABI counterpart; flatten to a struct with explicit tag + payload union, or expose individual variant constructors",
                        def.name,
                    ))
                } else {
                    // Plain (untagged) enum lowers to `i32` — a fine C ABI shape.
                    None
                }
            }
            Ty::Struct(id) => {
                let def = &self.structs[id.0 as usize];
                if def.is_drop {
                    return Some(format!(
                        "struct `{}` has a `drop` destructor; cross-boundary `Drop` is undefined (no destructor would run on the C side). Expose via opaque pointer + paired `*_free(*T)` instead",
                        def.name,
                    ));
                }
                if !def.is_repr_c {
                    return Some(format!(
                        "struct `{}` is not `#[repr(C)]`; layout is unspecified across the C boundary. Add `#[repr(C)]` to commit to C-compatible field layout",
                        def.name,
                    ));
                }
                // All fields must themselves be C-exportable. Fields
                // carry a third element (pub flag) which we ignore here.
                for (fname, fty, _is_pub) in &def.fields {
                    if let Some(r) = self.c_exportable_diagnosis(fty, false) {
                        return Some(format!(
                            "field `{}` of `{}` has non-C-ABI type `{}` ({r})",
                            fname, def.name, ty_display(fty),
                        ));
                    }
                }
                None
            }
            Ty::Array(elem, _n) => {
                // `[T; N]` is layout-compatible with C `T[N]` when T is.
                if let Some(r) = self.c_exportable_diagnosis(elem, false) {
                    return Some(format!("array element type `{}` is not C-ABI compatible ({r})", ty_display(elem)));
                }
                None
            }
            Ty::Param(name) => Some(format!(
                "generic type parameter `{name}` cannot appear in a `pub extern fn` signature; monomorphize manually by exposing one concrete overload per type",
            )),
            Ty::Error => None,  // Type already errored; don't double-report.
        }
    }

    fn check_function_body(&mut self, body: &Block, expected: Ty, body_span: ByteSpan) {
        // Push the body scope.
        self.scopes.push(HashMap::new());
        for s in &body.stmts {
            self.check_stmt(s);
        }
        // C+ style: function bodies use explicit `return`, never an implicit
        // tail expression. Block expressions remain valid in let initializers,
        // assignments, and return expressions — just not at function-body level.
        if let Some(tail) = &body.tail {
            self.err(
                "E0333",
                "function body cannot end with an implicit tail expression; use `return ...;` instead".to_string(),
                tail.span,
            );
            // Still type-check the tail for cascading diagnostics.
            let _ = self.check_expr(tail, Some(expected.clone()));
        } else if expected != Ty::Unit && expected != Ty::Error && !body_ends_with_return(body) {
            self.err(
                "E0306",
                format!(
                    "function body must end with `return ...;` for type `{}`",
                    expected.name()
                ),
                body_span,
            );
        }
        self.scopes.pop();
    }

    // ---- statements ----

    fn check_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { mutable, name, ty, init } => {
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let (final_ty, assigned) = match init {
                    Some(init_expr) => {
                        let inferred = self.check_expr(init_expr, declared.clone());
                        let final_ty = declared.unwrap_or(inferred);
                        (final_ty, true)
                    }
                    None => {
                        // `let x: T;` — no initializer. Type annotation is
                        // required since there's no expression to infer from.
                        let final_ty = declared.unwrap_or_else(|| {
                            self.err(
                                "E0346",
                                "uninitialized `let` requires a type annotation".to_string(),
                                s.span,
                            );
                            Ty::Error
                        });
                        (final_ty, false)
                    }
                };
                self.scopes.last_mut().unwrap().insert(
                    name.name.clone(),
                    LocalInfo {
                        ty: final_ty,
                        mutable: *mutable,
                        moved: false,
                        assigned,
                    },
                );
            }
            StmtKind::Return(value) => {
                let ret = self.current_return.clone();
                match (value, &ret) {
                    (Some(e), _) => {
                        self.check_expr(e, Some(ret));
                    }
                    (None, &Ty::Unit) | (None, &Ty::Error) => {}
                    (None, _) => {
                        self.err(
                            "E0307",
                            format!("`return` without a value, but function returns `{}`", ret.name()),
                            s.span,
                        );
                    }
                }
            }
            StmtKind::While { cond, body } => {
                let _ = self.check_cond(cond);
                self.scopes.push(HashMap::new());
                self.loop_depth += 1;
                self.check_block_as_stmt(body);
                self.loop_depth -= 1;
                self.scopes.pop();
            }
            StmtKind::For(fl) => {
                self.loop_depth += 1;
                self.check_for(fl);
                self.loop_depth -= 1;
            }
            StmtKind::Expr(e) => {
                let _ = self.check_expr(e, None);
            }
            StmtKind::Defer(e) => {
                // The deferred expression's value is discarded; sema just
                // type-checks it like any expression statement. Codegen
                // re-emits the expression at scope exit (lexical, not
                // runtime-stack — see `docs/design/phase3-drop.md` §4.4).
                let _ = self.check_expr(e, None);
            }
            // The lowering pass (`crate::lower`) replaces every `IfLet` /
            // `GuardLet` / `WhileLet` with an equivalent `match`-using
            // form before sema runs. Hitting one here means the driver
            // skipped lowering; panic instead of silently producing
            // wrong diagnostics.
            StmtKind::IfLet { .. } | StmtKind::GuardLet { .. } | StmtKind::WhileLet { .. } => {
                panic!("sema saw an un-lowered if-let/guard-let/while-let; driver must call crate::lower before sema::check");
            }
            // Slice 4-end: `break;` / `continue;` are valid only inside
            // a loop body. E0353 fires when they appear at function-body
            // level or in a non-loop nested scope.
            StmtKind::Break | StmtKind::Continue => {
                if self.loop_depth == 0 {
                    let kw = if matches!(s.kind, StmtKind::Break) { "break" } else { "continue" };
                    self.err(
                        "E0353",
                        format!("`{kw}` used outside of a loop"),
                        s.span,
                    );
                }
            }
            // Phase 5 slice 5ATTR.3: `assert EXPR;` — expression must be
            // `bool`. Type-mismatch reuses E0302 (the general type-mismatch
            // code) so the diagnostic shape matches every other "wrong type
            // for this position" case. Codegen branches on the value.
            StmtKind::Assert(e) => {
                let actual = self.check_expr(e, Some(Ty::Bool));
                if !matches!(actual, Ty::Bool | Ty::Error) {
                    self.err(
                        "E0302",
                        format!(
                            "`assert` condition must be `bool`, got `{}`",
                            actual.name()
                        ),
                        e.span,
                    );
                }
            }
            // Slice 4-end: `loop { BODY }` — unconditional loop. Body
            // runs in a fresh scope with the loop-depth incremented so
            // any nested break/continue type-checks. Loops always
            // produce unit at the statement level.
            StmtKind::Loop(body) => {
                self.loop_depth += 1;
                self.scopes.push(HashMap::new());
                for stmt in &body.stmts {
                    self.check_stmt(stmt);
                }
                if let Some(tail) = &body.tail {
                    let _ = self.check_expr(tail, None);
                }
                self.scopes.pop();
                self.loop_depth -= 1;
            }
        }
    }

    fn check_for(&mut self, fl: &ForLoop) {
        match fl {
            ForLoop::Range { var, iter, body } => {
                let (start, end) = match &iter.kind {
                    ExprKind::Range { start: Some(s), end: Some(e), .. } => (s.as_ref(), e.as_ref()),
                    _ => {
                        self.err(
                            "E0312",
                            "Phase 1 `for ... in` requires a closed range like `0..n` or `0..=n`".to_string(),
                            iter.span,
                        );
                        return;
                    }
                };
                self.check_expr(start, Some(Ty::I32));
                self.check_expr(end, Some(Ty::I32));
                self.scopes.push(HashMap::new());
                self.scopes.last_mut().unwrap().insert(
                    var.name.clone(),
                    LocalInfo { ty: Ty::I32, mutable: false, moved: false, assigned: true },
                );
                self.check_block_as_stmt(body);
                self.scopes.pop();
            }
            ForLoop::CStyle { init, cond, update, body } => {
                self.scopes.push(HashMap::new());
                if let Some(init) = init { self.check_stmt(init); }
                if let Some(cond) = cond { let _ = self.check_cond(cond); }
                for u in update { let _ = self.check_expr(u, None); }
                self.check_block_as_stmt(body);
                self.scopes.pop();
            }
        }
    }

    /// Type-check a block used in statement position (its value is discarded).
    fn check_block_as_stmt(&mut self, b: &Block) {
        self.scopes.push(HashMap::new());
        for s in &b.stmts { self.check_stmt(s); }
        if let Some(tail) = &b.tail {
            let _ = self.check_expr(tail, None);
        }
        self.scopes.pop();
    }

    /// Condition expressions must be `bool`.
    fn check_cond(&mut self, e: &Expr) -> Ty {
        let t = self.check_expr(e, None);
        if t != Ty::Bool && t != Ty::Error {
            self.err(
                "E0304",
                format!("condition must be `bool`, found `{}`", t.name()),
                e.span,
            );
        }
        Ty::Bool
    }

    // ---- expressions ----

    fn check_expr(&mut self, e: &Expr, expected: Option<Ty>) -> Ty {
        let actual = self.check_expr_kind(e, expected.clone());
        if let Some(exp) = expected {
            if exp != Ty::Error && actual != Ty::Error && exp != actual {
                self.err(
                    "E0302",
                    format!("type mismatch: expected `{}`, found `{}`", exp.name(), actual.name()),
                    e.span,
                );
            }
        }
        actual
    }

    fn check_expr_kind(&mut self, e: &Expr, expected: Option<Ty>) -> Ty {
        match &e.kind {
            ExprKind::IntLit(_, suf) => self.check_int_lit(*suf, expected),
            ExprKind::FloatLit(_, suf) => self.check_float_lit(*suf, expected),
            ExprKind::BoolLit(_) => Ty::Bool,
            ExprKind::StrLit(_) => Ty::Str,
            ExprKind::InterpStr { parts } => self.check_interp_str(parts, e.span),
            ExprKind::Ident(name) => self.resolve_value_ident(name, e.span, expected.clone()),
            ExprKind::Block(b) => self.check_block_as_expr(b),
            ExprKind::Unsafe(b) => {
                self.unsafe_depth += 1;
                let ty = self.check_block_as_expr(b);
                self.unsafe_depth -= 1;
                ty
            }
            // v0.0.3 Phase 5 Slice 5E.2: `await EXPR` evaluates to the
            // inner T of the surrounding `Future[T]` expression. Two
            // gates:
            //   - **E0901**: `await` outside an `async fn` body is
            //     rejected. The keyword is meaningless without a
            //     coroutine frame to suspend.
            //   - **E0902**: the inner expression must evaluate to a
            //     `Future[T]`. (Sema-only check for v0.0.3; users can
            //     only construct futures via `async fn`, but giving a
            //     dedicated error code keeps future expansion clean.)
            ExprKind::Await(inner) => {
                let inner_ty = self.check_expr(inner, None);
                if !self.current_fn_is_async {
                    self.err(
                        "E0901",
                        "`await` is only valid inside an `async fn` body".to_string(),
                        e.span,
                    );
                    return Ty::Error;
                }
                if matches!(inner_ty, Ty::Error) { return Ty::Error; }
                match self.unwrap_future(&inner_ty) {
                    Some(t) => t,
                    None => {
                        self.err(
                            "E0902",
                            format!("`await` requires a `Future[T]` expression, got `{}`", ty_display(&inner_ty)),
                            inner.span,
                        );
                        Ty::Error
                    }
                }
            }
            ExprKind::If { cond, then, else_branch } => {
                self.check_if(cond, then, else_branch.as_deref())
            }
            ExprKind::Call { callee, args, type_args } => self.check_call(callee, args, type_args, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, e.span),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, e.span),
            ExprKind::Assign { op, target, value } => self.check_assign(*op, target, value, e.span),
            ExprKind::Range { .. } => {
                self.err(
                    "E0312",
                    "range expressions are only supported as the iterator in `for ... in`".to_string(),
                    e.span,
                );
                Ty::Error
            }
            ExprKind::Cast { expr, ty } => self.check_cast(expr, ty, e.span),
            ExprKind::Path { segments } => self.check_path(segments, e.span),
            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, e.span),
            ExprKind::GenericStructLit { name, type_args, fields } => {
                self.check_generic_struct_lit(name, type_args, fields, e.span)
            }
            ExprKind::GenericEnumCall { enum_name, type_args, variant, args } => {
                self.check_generic_enum_call(enum_name, type_args, variant, args, e.span)
            }
            ExprKind::Field { receiver, name } => self.check_field(receiver, name),
            ExprKind::ArrayLit { elements } => self.check_array_lit(elements, expected, e.span),
            ExprKind::Index { receiver, index } => self.check_index(receiver, index, e.span),
            ExprKind::Match { scrutinee, arms } => self.check_match(scrutinee, arms, expected, e.span),
        }
    }

    fn check_array_lit(&mut self, elements: &[Expr], expected: Option<Ty>, span: ByteSpan) -> Ty {
        if elements.is_empty() {
            self.err(
                "E0332",
                "empty array literals not supported in Phase 2; provide at least one element".to_string(),
                span,
            );
            return Ty::Error;
        }
        // Use the declared element type if we have an expected array; otherwise infer from first element.
        let expected_elem: Option<Ty> = match &expected {
            Some(Ty::Array(elem, _)) => Some((**elem).clone()),
            _ => None,
        };
        let first_ty = self.check_expr(&elements[0], expected_elem.clone());
        for e in &elements[1..] {
            let got = self.check_expr(e, Some(first_ty.clone()));
            if got != first_ty && got != Ty::Error && first_ty != Ty::Error {
                self.err(
                    "E0329",
                    format!("mixed element types in array literal: expected `{}`, found `{}`", first_ty.name(), got.name()),
                    e.span,
                );
            }
        }
        let len = elements.len() as u32;
        // If we had a declared length expectation, check it matches.
        if let Some(Ty::Array(_, declared_len)) = &expected {
            if *declared_len != len {
                self.err(
                    "E0330",
                    format!("array literal has {} element(s); expected {}", len, declared_len),
                    span,
                );
                return Ty::Error;
            }
        }
        Ty::Array(Box::new(first_ty), len)
    }

    fn check_index(&mut self, receiver: &Expr, index: &Expr, span: ByteSpan) -> Ty {
        let recv_ty = self.check_expr(receiver, None);
        // Index must be `usize`. Numeric literals will coerce via expected-type rule.
        let _ = self.check_expr(index, Some(Ty::Usize));
        match recv_ty {
            Ty::Array(elem, _) => (*elem).clone(),
            // Slice 10.FFI.2: indexing on a raw pointer is unchecked
            // pointer arithmetic. `p[i]` = `*(p + i)`. Pointee is the
            // result type. No bounds check (pointer length unknown).
            // Slice 10.FFI.3: requires `unsafe` block — pointer
            // indexing dereferences arbitrary memory.
            Ty::RawPtr(inner) => {
                if self.unsafe_depth == 0 {
                    self.err(
                        "E0801",
                        "indexing through a raw pointer is unsafe; wrap in `unsafe { ... }`".to_string(),
                        span,
                    );
                }
                (*inner).clone()
            }
            Ty::Error => Ty::Error,
            other => {
                self.err(
                    "E0331",
                    format!("cannot index non-array type `{}`", other.name()),
                    span,
                );
                Ty::Error
            }
        }
    }

    fn check_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], expected: Option<Ty>, span: ByteSpan) -> Ty {
        let scrutinee_ty = self.check_expr(scrutinee, None);
        // Only enums are matchable in Phase 3. Literal patterns (`match n { 0 => ... }`)
        // are E0343 (deferred), and matching arbitrary types isn't supported.
        let enum_id = match scrutinee_ty {
            Ty::Enum(id) => id,
            Ty::Error => {
                // Walk arm bodies for cascading diagnostics, then bail.
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    let _ = self.check_expr(&arm.body, None);
                    self.scopes.pop();
                }
                return Ty::Error;
            }
            other => {
                self.err(
                    "E0341",
                    format!("`match` scrutinee must be an enum type; found `{}`. Literal patterns are deferred (E0343).", other.name()),
                    scrutinee.span,
                );
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    let _ = self.check_expr(&arm.body, None);
                    self.scopes.pop();
                }
                return Ty::Error;
            }
        };

        let enum_name = self.enums[enum_id.0 as usize].name.clone();
        let variant_names: Vec<String> = self.enums[enum_id.0 as usize].variants.iter()
            .map(|v| v.name.clone())
            .collect();

        // Track which variants are covered by name. A wildcard / binding
        // pattern catches everything not yet covered.
        let mut covered: HashMap<String, ()> = HashMap::new();
        let mut has_catchall = false;
        let mut result_ty: Option<Ty> = None;
        // Definite-assignment flow merge across arms: snapshot pre-match
        // state, run each arm from that state, intersect post-arm states.
        let pre_match = self.snapshot_assigned();
        let mut merged_post: Option<Vec<Vec<(String, bool)>>> = None;

        for arm in arms {
            self.scopes.push(HashMap::new());
            // Check the pattern: validate against the scrutinee's enum,
            // bind any payload names. Returns false if the pattern is
            // structurally invalid (errors emitted inline).
            self.check_pattern(&arm.pattern, enum_id, &enum_name, &mut covered, &mut has_catchall);
            // Check the arm body with the expected result type.
            let arm_ty = self.check_expr(&arm.body, expected.clone());
            // First arm sets the result type; later arms must agree —
            // EXCEPT arms whose body syntactically diverges (every path
            // ends in `return` and friends) carry no value to the match
            // expression, so they don't constrain the result type. This
            // is what makes `guard let` work: the else arm always
            // diverges, and the success arm's payload-typed body sets the
            // result.
            let arm_diverges = crate::lower::expr_diverges(&arm.body);
            if !arm_diverges {
                match &result_ty {
                    None => result_ty = Some(arm_ty),
                    Some(rt) if *rt == Ty::Error => {}
                    Some(_) if arm_ty == Ty::Error => {}
                    Some(rt) if *rt != arm_ty => {
                        self.err(
                            "E0302",
                            format!("match arms produce different types: expected `{}`, found `{}`", rt.name(), arm_ty.name()),
                            arm.span,
                        );
                    }
                    _ => {}
                }
            }
            self.scopes.pop();
            // Capture this arm's post-state for flow merging, then reset
            // for the next arm.
            let after_arm = self.snapshot_assigned();
            merged_post = Some(match merged_post {
                None => after_arm,
                Some(prev) => self.intersect_assigned(&prev, &after_arm),
            });
            self.restore_assigned(&pre_match);
        }

        // Exhaustiveness: every variant must be covered, or there must be
        // a catch-all wildcard / binding arm.
        if !has_catchall {
            let mut missing: Vec<String> = variant_names.iter()
                .filter(|n| !covered.contains_key(*n))
                .cloned()
                .collect();
            if !missing.is_empty() {
                missing.sort();   // deterministic for diagnostics
                let list = missing.join(", ");
                self.err(
                    "E0340",
                    format!("non-exhaustive `match` on enum `{}`: missing variant(s) {}", enum_name, list),
                    span,
                );
            }
        }

        // Apply the merged post-match assigned-state. If there were no
        // arms (degenerate), keep pre-match state.
        if let Some(merged) = merged_post {
            self.restore_assigned(&merged);
        }

        result_ty.unwrap_or(Ty::Unit)
    }

    /// Type-check a single match arm's pattern. Variant patterns add
    /// payload bindings into the current scope. Wildcard/binding patterns
    /// act as catch-alls. Diagnostics are emitted inline.
    fn check_pattern(
        &mut self,
        pat: &Pattern,
        enum_id: EnumId,
        enum_name: &str,
        covered: &mut HashMap<String, ()>,
        has_catchall: &mut bool,
    ) {
        match &pat.kind {
            PatternKind::Wildcard => {
                *has_catchall = true;
            }
            PatternKind::Binding(name) => {
                // A bare identifier binds the scrutinee value to that name
                // in the arm scope. The binding's type is the enum itself.
                *has_catchall = true;
                self.scopes.last_mut().unwrap().insert(
                    name.name.clone(),
                    LocalInfo { ty: Ty::Enum(enum_id), mutable: false, moved: false, assigned: true },
                );
            }
            PatternKind::Variant { enum_name: pat_enum, type_args, variant_name, payload } => {
                // Pattern's enum-segment must match the scrutinee's enum.
                //
                // Three acceptance shapes (slice 7GEN.5e — "no mangled
                // names in source"):
                //   (a) `Foo::Bar` against a concrete enum named `Foo`.
                //   (b) `Option[i32]::Some` against an `Option[i32]`
                //       scrutinee — resolve type_args to an EnumId and
                //       compare against the scrutinee's id directly.
                //   (c) `Option::Some` (no type args) against an
                //       `Option[i32]` scrutinee — type-directed: accept
                //       when `pat_enum.name` matches the scrutinee's
                //       `generic_base` source name.
                //
                // The internal mangled name (`Option__i32`) is accepted
                // by shape (a) incidentally because that's what's stored
                // in EnumDef.name today, but it's not a documented
                // source-level surface and may stop matching as the
                // mangling scheme evolves.
                // When the pattern carries explicit type args
                // (`Option[i32]::Some(v)`), resolution is strict: the
                // args must produce the same EnumId as the scrutinee's.
                // Generic-base fallback applies *only* when the
                // pattern has no type args (`Option::Some(v)`).
                let pattern_matches = if !type_args.is_empty() {
                    let resolved = self.resolve_generic_enum_instantiation(
                        &pat_enum.name,
                        type_args,
                        pat.span,
                    );
                    matches!(resolved, Ty::Enum(rid) if rid == enum_id)
                } else {
                    pat_enum.name == enum_name
                        || self.enums[enum_id.0 as usize]
                            .generic_base
                            .as_deref()
                            .map_or(false, |g| g == pat_enum.name)
                };
                if !pattern_matches {
                    let display = if type_args.is_empty() {
                        pat_enum.name.clone()
                    } else {
                        format!("{}[{}]", pat_enum.name, "...")
                    };
                    self.err(
                        "E0341",
                        format!("pattern type `{}` does not match scrutinee enum `{}`", display, enum_name),
                        pat.span,
                    );
                    return;
                }
                // Look up the variant by name; capture its payload types.
                let variant_info = self.enums[enum_id.0 as usize].variants.iter()
                    .find(|v| v.name == variant_name.name)
                    .cloned();
                let Some(vdef) = variant_info else {
                    self.err(
                        "E0317",
                        format!("enum `{}` has no variant `{}`", enum_name, variant_name.name),
                        variant_name.span,
                    );
                    return;
                };
                covered.insert(variant_name.name.clone(), ());
                // Payload arity check.
                if payload.len() != vdef.payload.len() {
                    self.err(
                        "E0342",
                        format!(
                            "variant `{}::{}` takes {} payload value(s); pattern has {}",
                            enum_name, variant_name.name, vdef.payload.len(), payload.len()
                        ),
                        pat.span,
                    );
                    return;
                }
                // Bind payload patterns. Phase 3: only Wildcard / Binding
                // allowed in payload positions (no nested Variant).
                for (pp, pty) in payload.iter().zip(vdef.payload.iter()) {
                    match &pp.kind {
                        PatternKind::Wildcard => {}
                        PatternKind::Binding(name) => {
                            self.scopes.last_mut().unwrap().insert(
                                name.name.clone(),
                                LocalInfo { ty: pty.clone(), mutable: false, moved: false, assigned: true },
                            );
                        }
                        PatternKind::Variant { .. } => {
                            self.err(
                                "E0341",
                                "nested variant patterns are not supported in Phase 3 (payload patterns must be `_` or a binding name)".to_string(),
                                pp.span,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Slice 7GEN.5d: type-check `Option[i32]::Some(7)`. Resolves the
    /// generic enum instantiation, then delegates to `check_assoc_call`
    /// with the mangled enum name + variant — reusing all the variant
    /// arity/payload-type checking already in place.
    fn check_generic_enum_call(
        &mut self,
        enum_name: &Ident,
        type_args: &[Type],
        variant: &Ident,
        args: &[Expr],
        span: ByteSpan,
    ) -> Ty {
        // Slice 7GEN.5c carry-forward (closed 2026-05-13): the parser
        // can't tell at `Ident[args]::name(...)` whether `Ident` is a
        // generic enum (variant constructor) or a generic struct
        // (associated-fn call). Both produce `GenericEnumCall` in the
        // AST; dispatch here based on which template table contains
        // the name. Struct path lowers to a 2-segment `Path` assoc-call
        // on the mangled instantiation.
        if self.struct_generic_templates.contains_key(&enum_name.name) {
            let sty = self.resolve_generic_instantiation(&enum_name.name, type_args, enum_name.span);
            let Ty::Struct(sid) = sty else {
                for a in args { let _ = self.check_expr(a, None); }
                return Ty::Error;
            };
            let mangled = self.structs[sid.0 as usize].name.clone();
            // First try the impl-block method on the instantiated struct.
            // If present, dispatch like any other `Type::method(...)` call.
            if self.structs[sid.0 as usize].methods.contains_key(&variant.name) {
                let segments = vec![
                    Ident { name: mangled, span: enum_name.span },
                    variant.clone(),
                ];
                return self.check_assoc_call(&segments, &[], args, enum_name.span, span);
            }
            // v0.0.4 Phase 1C: free-fn fallback for `Type[args]::name(...)`.
            // Many constructor-shaped fns (`Vec::with_capacity`, `string::new`)
            // are written as module-level free generic fns rather than impl
            // associated fns. Resolve by stripping the struct's last name
            // segment to get the enclosing module prefix and looking up
            // `<module>.<variant>` in the generic-fn table. The Type[args]
            // bracket's type-args become the free fn's type-args.
            let module_prefix = enum_name.name
                .rsplit_once('.')
                .map(|(prefix, _)| prefix.to_string());
            let qualified_fn_name = match &module_prefix {
                Some(prefix) if !prefix.is_empty() => format!("{}.{}", prefix, variant.name),
                _ => variant.name.clone(),
            };
            if let Some(gsig) = self.fns_generic.get(&qualified_fn_name).cloned() {
                self.assoc_free_fn_dispatches.insert(span, qualified_fn_name.clone());
                return self.check_generic_named_call(
                    &qualified_fn_name, &gsig, args, type_args, span,
                );
            }
            self.err(
                "E0324",
                format!(
                    "struct `{}` has no method `{}` and no free fn `{}` in its module",
                    mangled, variant.name, variant.name
                ),
                variant.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let ty = self.resolve_generic_enum_instantiation(&enum_name.name, type_args, enum_name.span);
        let Ty::Enum(id) = ty else {
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        // Slice 7GEN.5d: for payload-less variants accessed without
        // parens (parser produces args=[]), short-circuit to the bare
        // variant value — no E0327 since the user didn't write parens.
        // Multi-payload + parens path delegates to check_assoc_call.
        let def = &self.enums[id.0 as usize];
        if let Some(vdef) = def.variants.iter().find(|v| v.name == variant.name) {
            if vdef.payload.is_empty() && args.is_empty() {
                return Ty::Enum(id);
            }
        } else {
            self.err(
                "E0317",
                format!("enum `{}` has no variant `{}`", def.name, variant.name),
                variant.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let mangled = self.enums[id.0 as usize].name.clone();
        let segments = vec![
            Ident { name: mangled, span: enum_name.span },
            variant.clone(),
        ];
        self.check_assoc_call(&segments, &[], args, enum_name.span, span)
    }

    /// Slice 7GEN.5c: type-check `Pair[i32, bool] { first: 7, second: true }`.
    /// Routes through `resolve_generic_instantiation` to get the
    /// synthesized concrete struct, then delegates to `check_struct_lit`
    /// with the mangled name.
    fn check_generic_struct_lit(
        &mut self,
        name: &Ident,
        type_args: &[Type],
        fields: &[StructLitField],
        span: ByteSpan,
    ) -> Ty {
        let ty = self.resolve_generic_instantiation(&name.name, type_args, name.span);
        let Ty::Struct(id) = ty else {
            for f in fields { let _ = self.check_expr(&f.value, None); }
            return Ty::Error;
        };
        let mangled = self.structs[id.0 as usize].name.clone();
        let mangled_ident = Ident { name: mangled, span: name.span };
        self.check_struct_lit(&mangled_ident, fields, span)
    }

    fn check_struct_lit(&mut self, name: &Ident, fields: &[StructLitField], span: ByteSpan) -> Ty {
        let Some(&id) = self.struct_by_name.get(&name.name) else {
            self.err("E0303", format!("unknown type `{}`", name.name), name.span);
            // Still walk the field exprs so we surface their errors.
            for f in fields { let _ = self.check_expr(&f.value, None); }
            return Ty::Error;
        };
        // Snapshot the declared fields so we can borrow self mutably below.
        let declared: Vec<(String, Ty, bool)> = self.structs[id.0 as usize].fields.clone();
        let struct_name = self.structs[id.0 as usize].name.clone();
        let struct_origin = self.structs[id.0 as usize].origin_file.clone();

        // Detect duplicate-in-literal and unknown-field; type-check each provided value.
        let mut provided: HashMap<String, ()> = HashMap::new();
        for lit_field in fields {
            if provided.contains_key(&lit_field.name.name) {
                self.err(
                    "E0319",
                    format!("duplicate field `{}` in literal of struct `{}`",
                            lit_field.name.name, struct_name),
                    lit_field.name.span,
                );
                let _ = self.check_expr(&lit_field.value, None);
                continue;
            }
            provided.insert(lit_field.name.name.clone(), ());
            let declared_field = declared
                .iter()
                .find(|(n, _, _)| n == &lit_field.name.name);
            match declared_field {
                Some((_, t, is_pub)) => {
                    // Slice 4C: cross-file field-pub gate. Same-file
                    // construction always sees private fields.
                    if !*is_pub && self.is_cross_file_access(&struct_origin) {
                        self.err(
                            "E0403",
                            format!("field `{}` of struct `{}` is private (mark it `pub` in its declaration to expose)", lit_field.name.name, struct_name),
                            lit_field.name.span,
                        );
                    }
                    let _ = self.check_expr(&lit_field.value, Some(t.clone()));
                }
                None => {
                    self.err(
                        "E0322",
                        format!("struct `{struct_name}` has no field `{}`", lit_field.name.name),
                        lit_field.name.span,
                    );
                    let _ = self.check_expr(&lit_field.value, None);
                }
            }
        }
        // Detect missing fields.
        for (declared_name, _, _) in &declared {
            if !provided.contains_key(declared_name) {
                self.err(
                    "E0321",
                    format!("missing field `{declared_name}` in literal of struct `{struct_name}`"),
                    span,
                );
            }
        }
        Ty::Struct(id)
    }

    fn check_field(&mut self, receiver: &Expr, name: &Ident) -> Ty {
        let recv_ty = self.check_expr(receiver, None);
        let Ty::Struct(id) = recv_ty else {
            if recv_ty != Ty::Error {
                self.err(
                    "E0323",
                    format!("field access on non-struct type `{}`", recv_ty.name()),
                    name.span,
                );
            }
            return Ty::Error;
        };
        let def = &self.structs[id.0 as usize];
        let struct_name = def.name.clone();
        let struct_origin = def.origin_file.clone();
        match def.field_with_pub(&name.name) {
            Some((_, ty, is_pub)) => {
                // Slice 4C: cross-file private-field read is E0403/Field.
                if !is_pub && self.is_cross_file_access(&struct_origin) {
                    self.err(
                        "E0403",
                        format!("field `{}` of struct `{}` is private (mark it `pub` in its declaration to expose)", name.name, struct_name),
                        name.span,
                    );
                }
                ty
            }
            None => {
                self.err(
                    "E0320",
                    format!("struct `{}` has no field `{}`", def.name, name.name),
                    name.span,
                );
                Ty::Error
            }
        }
    }

    /// True iff the current site (the function/method being checked) lives
    /// in a different file from `decl_origin`. Same-file access — and
    /// single-file mode where both are `None` — always allowed.
    fn is_cross_file_access(&self, decl_origin: &Option<String>) -> bool {
        match (decl_origin.as_ref(), self.current_file.as_ref()) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }

    fn check_int_lit(&mut self, suffix: NumSuffix, expected: Option<Ty>) -> Ty {
        match suffix {
            NumSuffix::None => match expected {
                Some(t) if t.is_int() => t,
                _ => Ty::I32, // default
            },
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
            // Float suffix on integer literal shouldn't happen — the lexer
            // routes those to FloatLit. Treat defensively.
            NumSuffix::F32 | NumSuffix::F64 => unreachable!("float suffix on int literal"),
        }
    }

    fn check_float_lit(&mut self, suffix: NumSuffix, expected: Option<Ty>) -> Ty {
        match suffix {
            NumSuffix::F32 => Ty::F32,
            NumSuffix::F64 => Ty::F64,
            NumSuffix::None => match expected {
                Some(Ty::F32) => Ty::F32,
                _ => Ty::F64, // default
            },
            _ => unreachable!("integer suffix on float literal"),
        }
    }

    fn check_cast(&mut self, expr: &Expr, target: &Type, span: ByteSpan) -> Ty {
        let from = self.check_expr(expr, None);
        let to = self.resolve_type(target);
        if from == Ty::Error || to == Ty::Error {
            return to;
        }
        if !cast_allowed(&from, &to) {
            self.err(
                "E0315",
                format!("invalid cast: `{}` cannot be cast to `{}`", from.name(), to.name()),
                span,
            );
            return Ty::Error;
        }
        // Phase 11 / P3 from the null-handling design (design.md):
        // integer-to-raw-pointer casts are how C+ expresses FFI null
        // (`0 as *u8`) and how user code constructs typed pointers from
        // raw addresses (interop with C APIs that return integers).
        // Gated by `unsafe` — the cast itself doesn't read memory, but
        // the resulting pointer is meaningful only if the integer was
        // a valid address; trusting the user is the unsafe part.
        if from.is_int() && matches!(to, Ty::RawPtr(_)) && self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "integer-to-pointer cast requires `unsafe { ... }`".to_string(),
                span,
            );
        }
        // Phase 11: raw-pointer → raw-pointer reinterpretation also requires
        // `unsafe`. The cast is mechanically free (both ends lower to LLVM
        // `ptr`), but the caller is asserting the reinterpreted bytes have
        // the new pointee's layout.
        if matches!(from, Ty::RawPtr(_)) && matches!(to, Ty::RawPtr(_)) && from != to && self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "raw-pointer reinterpretation cast requires `unsafe { ... }`".to_string(),
                span,
            );
        }
        to
    }

    fn check_block_as_expr(&mut self, b: &Block) -> Ty {
        // Block-as-expression: if no tail, value is Unit. The surrounding
        // expected-type check will catch genuine mismatches with E0302.
        // E0306 fires only at the function-body level, where "value required"
        // is unambiguous.
        self.scopes.push(HashMap::new());
        for s in &b.stmts { self.check_stmt(s); }
        let ty = match &b.tail {
            Some(t) => self.check_expr(t, None),
            None => Ty::Unit,
        };
        self.scopes.pop();
        ty
    }

    fn check_if(&mut self, cond: &Expr, then: &Block, else_branch: Option<&Expr>) -> Ty {
        let _ = self.check_cond(cond);
        // Definite-assignment flow merge: snapshot the assigned-state of
        // every visible binding before running the then-branch. After the
        // then-branch, capture the resulting state and restore the
        // pre-if state. Run the else-branch from the pre-if state. After
        // both branches, take the intersection — a binding is definitely
        // assigned post-if iff it was assigned in BOTH arms (or was
        // already assigned before the if).
        let pre_if = self.snapshot_assigned();
        let then_ty = self.check_block_as_expr(then);
        let after_then = self.snapshot_assigned();
        self.restore_assigned(&pre_if);
        let else_ty = match else_branch {
            Some(e) => match &e.kind {
                ExprKind::Block(b) => self.check_block_as_expr(b),
                ExprKind::If { .. } => self.check_expr(e, None),
                _ => Ty::Error,
            },
            None => Ty::Unit,
        };
        let after_else = self.snapshot_assigned();
        let merged = self.intersect_assigned(&after_then, &after_else);
        self.restore_assigned(&merged);
        if then_ty == Ty::Error || else_ty == Ty::Error {
            return Ty::Error;
        }
        if then_ty != else_ty {
            self.err(
                "E0302",
                format!(
                    "`if` and `else` branches have incompatible types: `{}` vs `{}`",
                    then_ty.name(), else_ty.name()
                ),
                then.span,
            );
            return Ty::Error;
        }
        then_ty
    }

    /// Snapshot the assigned-state of every binding currently in scope.
    /// Used for definite-assignment flow merging at `if`/`match` boundaries.
    fn snapshot_assigned(&self) -> Vec<Vec<(String, bool)>> {
        self.scopes.iter()
            .map(|scope| scope.iter().map(|(k, v)| (k.clone(), v.assigned)).collect())
            .collect()
    }

    /// Restore each binding's assigned-state from a prior snapshot. The
    /// scope stack shape must match (same names per frame). Used to reset
    /// state before running a parallel control-flow branch.
    fn restore_assigned(&mut self, snap: &[Vec<(String, bool)>]) {
        for (frame, snap_frame) in self.scopes.iter_mut().zip(snap.iter()) {
            for (name, was_assigned) in snap_frame {
                if let Some(info) = frame.get_mut(name) {
                    info.assigned = *was_assigned;
                }
            }
        }
    }

    /// Intersect two assigned-state snapshots: a binding is "assigned" in
    /// the merge iff it was assigned in BOTH inputs. Used post-`if`/`match`
    /// to compute the flow-merged state.
    fn intersect_assigned(&self, a: &[Vec<(String, bool)>], b: &[Vec<(String, bool)>]) -> Vec<Vec<(String, bool)>> {
        a.iter().zip(b.iter()).map(|(fa, fb)| {
            fa.iter().zip(fb.iter()).map(|((name, av), (_, bv))| {
                (name.clone(), *av && *bv)
            }).collect()
        }).collect()
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], type_args: &[Type], call_span: ByteSpan) -> Ty {
        // Slice 11.FN_PTR: when the callee is an Ident bound to a local
        // of FnPtr type, this is an indirect call. Validate args against
        // the pointer's param types, return the pointer's return type.
        // Falls through to the named-call dispatch when the Ident is a
        // fn name (or unknown — that path emits E0300).
        if let ExprKind::Ident(name) = &callee.kind {
            if let Some(info) = self.lookup_local(name) {
                if let Ty::FnPtr { params, return_type } = info.ty.clone() {
                    if !type_args.is_empty() {
                        self.err(
                            "E0501",
                            "indirect calls through a fn-pointer do not accept type arguments".to_string(),
                            callee.span,
                        );
                    }
                    // Note: `info.moved` / `info.assigned` would be useful
                    // diagnostics here too — but they fire when we
                    // resolve the ident below. We don't actually resolve
                    // the ident as a value (we only need the type), so
                    // explicitly read it via resolve_value_ident to keep
                    // the existing E0335/E0345 paths consistent.
                    let _ = self.resolve_value_ident(name, callee.span, None);
                    if args.len() != params.len() {
                        self.err(
                            "E0308",
                            format!(
                                "wrong number of arguments: fn pointer `{name}` expects {}, got {}",
                                params.len(),
                                args.len()
                            ),
                            call_span,
                        );
                        for a in args { let _ = self.check_expr(a, None); }
                        return *return_type;
                    }
                    for (a, p) in args.iter().zip(params.iter()) {
                        let _ = self.check_expr(a, Some(p.clone()));
                    }
                    return *return_type;
                }
            }
        }
        // Slice 11.FN_PTR: when the callee is a Field expression and the
        // field's type is FnPtr, this is an indirect call through a struct
        // field — the struct-of-callbacks pattern. Try field-as-FnPtr
        // first; if the name isn't a field (or isn't FnPtr-typed), fall
        // through to method dispatch.
        if let ExprKind::Field { receiver, name } = &callee.kind {
            let recv_ty = self.check_expr(receiver, None);
            if let Ty::Struct(id) = &recv_ty {
                let sdef = &self.structs[id.0 as usize];
                if let Some((_, ft, _)) = sdef.field_with_pub(&name.name) {
                    if let Ty::FnPtr { params, return_type } = ft {
                        if !type_args.is_empty() {
                            self.err(
                                "E0501",
                                "indirect calls through a fn-pointer field do not accept type arguments".to_string(),
                                callee.span,
                            );
                        }
                        if args.len() != params.len() {
                            self.err(
                                "E0308",
                                format!(
                                    "wrong number of arguments: fn-pointer field `{}` expects {}, got {}",
                                    name.name,
                                    params.len(),
                                    args.len()
                                ),
                                call_span,
                            );
                            for a in args { let _ = self.check_expr(a, None); }
                            return *return_type;
                        }
                        for (a, p) in args.iter().zip(params.iter()) {
                            let _ = self.check_expr(a, Some(p.clone()));
                        }
                        return *return_type;
                    }
                }
            }
        }
        match &callee.kind {
            ExprKind::Ident(_) => self.check_named_call(callee, args, type_args, call_span),
            ExprKind::Field { receiver, name } => {
                // Slice 7GEN.5e: turbofish + inference on method calls.
                // check_method_call now accepts type_args; routes to the
                // generic-method path when sig.generic_params is non-empty.
                self.check_method_call(receiver, name, type_args, args, call_span)
            }
            ExprKind::Path { segments } => {
                // Slice 7GEN.5e: turbofish + inference on assoc calls.
                self.check_assoc_call(segments, type_args, args, callee.span, call_span)
            }
            _ => {
                self.err(
                    "E0312",
                    "callee must be a function name, a method, or a `Type::function` path".to_string(),
                    callee.span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                Ty::Error
            }
        }
    }

    fn check_named_call(&mut self, callee: &Expr, args: &[Expr], type_args: &[Type], call_span: ByteSpan) -> Ty {
        let ExprKind::Ident(name) = &callee.kind else { unreachable!(); };
        // Slice 7GEN.5a: dispatch generic fns through inference.
        // Slice 7GEN.5b: when type_args are explicit, use them directly.
        if let Some(gsig) = self.fns_generic.get(name).cloned() {
            return self.check_generic_named_call(name, &gsig, args, type_args, call_span);
        }
        // Phase 11 slice 11.LAYOUT: `size_of[T]()` and `align_of[T]()`
        // are compiler intrinsics that take exactly one type argument
        // (via turbofish) and no value arguments. Both return `usize`.
        // Safe — no memory access, the LLVM lowering uses GEP on a null
        // pointer to compute the layout query as a constant the optimizer
        // folds at -O1+.
        if name == "size_of" || name == "align_of" {
            if type_args.len() != 1 {
                self.err(
                    "E0501",
                    format!(
                        "`{}` takes exactly 1 type argument, got {}",
                        name,
                        type_args.len()
                    ),
                    callee.span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                return Ty::Usize;
            }
            if !args.is_empty() {
                self.err(
                    "E0302",
                    format!(
                        "`{}` takes no value arguments, got {}",
                        name,
                        args.len()
                    ),
                    call_span,
                );
                for a in args { let _ = self.check_expr(a, None); }
            }
            // Resolve the type argument so unresolved names produce
            // diagnostics here rather than reaching codegen.
            let _ = self.resolve_type(&type_args[0]);
            return Ty::Usize;
        }
        // v0.0.3 Phase 5 Slice 5B: thread spawn/join intrinsics. Placed
        // before the "non-generic fn with turbofish" reject because both
        // intrinsics take one type-argument by design (mirroring size_of's
        // shape) — they're compiler-known and don't appear in `fns_generic`.
        if name == "__cplus_thread_spawn" || name == "__cplus_thread_join" {
            return self.check_thread_intrinsic(name, callee, args, type_args, call_span);
        }
        // v0.0.3 Phase 5 Slice 5C: spawn-with-input intrinsic. Takes
        // two type args (I, O) and two value args (input, f).
        if name == "__cplus_thread_spawn_with" {
            return self.check_thread_spawn_with(callee, args, type_args, call_span);
        }
        // v0.0.3 Phase 5 Slice 5E.5: `__cplus_block_on::[T](future) -> T`.
        // Drives a `Future[T]` to completion via a resume-loop and
        // returns the produced value. Stdlib's `executor::block_on`
        // wraps this so the user-visible API stays in stdlib.
        if name == "__cplus_block_on" {
            return self.check_block_on(callee, args, type_args, call_span);
        }
        // Non-generic fn with turbofish → reject. The user explicitly
        // asked to instantiate something that has no generic params.
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "function `{}` takes no type arguments but {} were provided",
                    name, type_args.len()
                ),
                callee.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        // Phase 8 slice 8.STR.2: `println` is a compiler intrinsic that
        // dispatches by argument type: `println(i32)` and `println(str)`
        // are both accepted. This is not user-visible overloading (§2.8
        // still rejects user-defined overloads); the intrinsic is one of
        // a small set of compiler-known names.
        if name == "println" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], None);
            if !matches!(arg_ty, Ty::I32 | Ty::Str | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`println` accepts `i32` or `str`; got `{}`", ty_display(&arg_ty)),
                    args[0].span,
                );
            }
            return Ty::Unit;
        }
        // Slice 10.FFI.2: `str_ptr(s)` and `str_len(s)` extract the
        // internal fields of a `str` fat-pointer. Bridge for users who
        // want to interop `str` with raw-pointer FFI (e.g. pass a
        // string literal's bytes to a C function expecting `*u8`).
        // The intrinsic shape matches `println` — compiler-known names,
        // not user-overloadable; rejected if arg is not `str`.
        if name == "str_ptr" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], Some(Ty::Str));
            if !matches!(arg_ty, Ty::Str | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`str_ptr` requires a `str` argument, got `{}`", ty_display(&arg_ty)),
                    args[0].span,
                );
            }
            return Ty::RawPtr(Box::new(Ty::U8));
        }
        if name == "str_len" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], Some(Ty::Str));
            if !matches!(arg_ty, Ty::Str | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`str_len` requires a `str` argument, got `{}`", ty_display(&arg_ty)),
                    args[0].span,
                );
            }
            return Ty::Usize;
        }
        // Slice 10.FFI.2: `str_from_raw_parts(p, n)` composes a `str`
        // from its components. The inverse of `str_ptr` + `str_len`.
        // Unsafe — caller is responsible for `p` pointing to `n`
        // valid UTF-8 bytes that live long enough.
        // Slice 10.FFI.3: requires `unsafe` block.
        if name == "str_from_raw_parts" && args.len() == 2 {
            if self.unsafe_depth == 0 {
                self.err(
                    "E0801",
                    "`str_from_raw_parts` is unsafe; wrap in `unsafe { ... }`".to_string(),
                    call_span,
                );
            }
            let p_ty = self.check_expr(&args[0], Some(Ty::RawPtr(Box::new(Ty::U8))));
            let _ = self.check_expr(&args[1], Some(Ty::Usize));
            if !matches!(p_ty, Ty::RawPtr(_) | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`str_from_raw_parts` first arg must be `*u8`, got `{}`", ty_display(&p_ty)),
                    args[0].span,
                );
            }
            return Ty::Str;
        }
        // Phase 11 polish (2026-05-14): slice intrinsics. Same shape as
        // the str intrinsics but generic over the slice's element type.
        // `slice_ptr(s: T[]) -> *T` — extract ptr.
        // `slice_len(s: T[]) -> usize` — extract len.
        // `slice_from_raw_parts(p: *T, n: usize) -> T[]` — unsafe, build
        // a slice from a raw pointer + length. Element type inferred
        // from the pointer's pointee type.
        if name == "slice_ptr" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], None);
            if let Ty::Slice(elem) = &arg_ty {
                return Ty::RawPtr(elem.clone());
            }
            if !matches!(arg_ty, Ty::Error) {
                self.err(
                    "E0302",
                    format!("`slice_ptr` requires a slice argument (e.g. `i32[]`), got `{}`", ty_display(&arg_ty)),
                    args[0].span,
                );
            }
            return Ty::Error;
        }
        // Phase 3A: byte-swap intrinsics. Built-in for network byte order
        // and other endian-flipping needs. `bswapN(x: uN) -> uN` for
        // N ∈ {16, 32, 64}; htons/htonl/ntohs/ntohl are aliases that
        // expand to bswap on little-endian targets (every C+ target today
        // is LE — x86_64, arm64-darwin, arm64-linux).
        if let Some(bswap_ty) = match name.as_str() {
            "bswap16" | "htons" | "ntohs" => Some(Ty::U16),
            "bswap32" | "htonl" | "ntohl" => Some(Ty::U32),
            "bswap64"                     => Some(Ty::U64),
            _ => None,
        } {
            if args.len() != 1 {
                self.err(
                    "E0501",
                    format!("`{name}` takes exactly 1 argument, got {}", args.len()),
                    call_span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                return Ty::Error;
            }
            let _ = self.check_expr(&args[0], Some(bswap_ty.clone()));
            return bswap_ty;
        }
        if name == "slice_len" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], None);
            if !matches!(arg_ty, Ty::Slice(_) | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`slice_len` requires a slice argument (e.g. `i32[]`), got `{}`", ty_display(&arg_ty)),
                    args[0].span,
                );
            }
            return Ty::Usize;
        }
        if name == "slice_from_raw_parts" && args.len() == 2 {
            if self.unsafe_depth == 0 {
                self.err(
                    "E0801",
                    "`slice_from_raw_parts` is unsafe; wrap in `unsafe { ... }`".to_string(),
                    call_span,
                );
            }
            let p_ty = self.check_expr(&args[0], None);
            let _ = self.check_expr(&args[1], Some(Ty::Usize));
            let elem = match &p_ty {
                Ty::RawPtr(inner) => (**inner).clone(),
                Ty::Error => return Ty::Error,
                _ => {
                    self.err(
                        "E0302",
                        format!("`slice_from_raw_parts` first arg must be a raw pointer `*T`, got `{}`", ty_display(&p_ty)),
                        args[0].span,
                    );
                    return Ty::Error;
                }
            };
            return Ty::Slice(Box::new(elem));
        }
        // v0.0.3 Phase 5 Slice 5A: atomic intrinsics. Names match the
        // pattern `__cplus_atomic_<op>_<ty>_<ord>`. See the
        // `cplus_core::atomic` module for the full surface. All
        // atomic ops require `unsafe` — they read/write through a raw
        // pointer whose validity the compiler can't prove.
        if let Some(spec) = crate::atomic::parse_atomic_intrinsic(name) {
            if self.unsafe_depth == 0 {
                self.err(
                    "E0801",
                    format!("`{}` is unsafe; wrap in `unsafe {{ ... }}`", name),
                    call_span,
                );
            }
            let expected_args = 1 + spec.value_arg_count();
            if args.len() != expected_args {
                self.err(
                    "E0308",
                    format!("`{}` takes {} argument(s), got {}", name, expected_args, args.len()),
                    call_span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                return if spec.returns_value() { spec.ty.clone() } else { Ty::Unit };
            }
            let ptr_ty = Ty::RawPtr(Box::new(spec.ty.clone()));
            let p_actual = self.check_expr(&args[0], Some(ptr_ty.clone()));
            if !matches!(p_actual, Ty::RawPtr(_) | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`{}` first argument must be `*{}`, got `{}`", name, spec.ty.name(), ty_display(&p_actual)),
                    args[0].span,
                );
            } else if let Ty::RawPtr(inner) = &p_actual {
                if **inner != spec.ty {
                    self.err(
                        "E0302",
                        format!("`{}` first argument must be `*{}`, got `{}`", name, spec.ty.name(), ty_display(&p_actual)),
                        args[0].span,
                    );
                }
            }
            for a in args.iter().skip(1) {
                let _ = self.check_expr(a, Some(spec.ty.clone()));
            }
            return if spec.returns_value() { spec.ty.clone() } else { Ty::Unit };
        }
        let Some(sig) = self.fns.get(name).cloned() else {
            self.err("E0300", format!("undefined function `{name}`"), callee.span);
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        // Slice 10.FFI.3: extern fn calls require `unsafe { ... }`.
        // The callee's contract is unverified — it may have arbitrary
        // side effects, return uninitialized memory, etc.
        if self.extern_fns.contains(name) && self.unsafe_depth == 0 {
            self.err(
                "E0801",
                format!("calling extern fn `{}` is unsafe; wrap in `unsafe {{ ... }}`", name),
                call_span,
            );
        }
        // Slice 10.FFI.4: variadic extern fns accept any number of
        // extra args beyond their fixed-param list. Fixed-param count
        // is still enforced as a minimum.
        if sig.is_variadic {
            if args.len() < sig.params.len() {
                self.err(
                    "E0308",
                    format!("variadic function `{}` requires at least {} fixed argument(s), got {}", name, sig.params.len(), args.len()),
                    call_span,
                );
            }
        } else if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!("function `{}` takes {} argument(s), got {}", name, sig.params.len(), args.len()),
                call_span,
            );
        }
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            self.check_arg_with_move(a, expected);
        }
        // Slice 10.FFI.4: type-check the variadic tail. C's varargs ABI
        // requires the caller to pass concrete types (no implicit
        // promotion modeling in this first cut); just sema-check that
        // each tail arg has *some* well-typed value.
        if sig.is_variadic {
            for a in args.iter().skip(sig.params.len()) {
                let _ = self.check_expr(a, None);
            }
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        sig.return_type
    }

    /// Slice 7GEN.5a: type-check a call to a generic fn. Infers the
    /// substitution map from argument types (each `Ty::Param(name)`
    /// in the signature unifies with the corresponding arg's type),
    /// records the instantiation in `fn_instantiations` and the
    /// per-call mapping in `call_monos`, and returns the substituted
    /// return type.
    ///
    /// Reports:
    /// - **E0308** — wrong number of arguments.
    /// - **E0500** — a declared generic param was never positioned
    ///   such that inference could pin it (every param appears only
    ///   inside another type with no top-level Param matches the
    ///   generic name). Phase-7 first cut: each generic param must
    ///   appear as a top-level Param somewhere — nested-only or
    ///   return-only params are deferred.
    /// - **E0302** — type mismatch when the same Param infers two
    ///   different concrete types across arguments.
    /// v0.0.3 Phase 5 Slice 5B: type-check `__cplus_thread_spawn::[O](f)`
    /// and `__cplus_thread_join::[O](h)`. Both intrinsics take one
    /// turbofish type argument and one value argument; both require
    /// `unsafe`. Spawn returns `JoinHandle[O]` (from stdlib/thread),
    /// join returns `O`.
    fn check_thread_intrinsic(
        &mut self,
        name: &str,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                format!("`{}` is unsafe; wrap in `unsafe {{ ... }}`", name),
                call_span,
            );
        }
        if type_args.len() != 1 {
            self.err(
                "E0501",
                format!("`{}` takes 1 type argument, got {}", name, type_args.len()),
                callee.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!("`{}` takes 1 value argument, got {}", name, args.len()),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let o_ty = self.resolve_type(&type_args[0]);
        let Some(template) = self.struct_generic_templates.get("JoinHandle").cloned() else {
            self.err(
                "E0300",
                format!("`{}` requires `JoinHandle[O]` from `stdlib/thread`", name),
                call_span,
            );
            return Ty::Error;
        };
        if name == "__cplus_thread_spawn" {
            let expected_f = Ty::FnPtr { params: vec![], return_type: Box::new(o_ty.clone()) };
            let _f_ty = self.check_expr(&args[0], Some(expected_f));
            return self.instantiate_struct_from_arg_tys("JoinHandle", &template, vec![o_ty]);
        }
        // __cplus_thread_join
        let expected_h = self.instantiate_struct_from_arg_tys("JoinHandle", &template, vec![o_ty.clone()]);
        let _h_ty = self.check_expr(&args[0], Some(expected_h));
        o_ty
    }

    /// v0.0.3 Phase 5 Slice 5E.5: type-check
    /// `__cplus_block_on::[T](future)`. Takes one type arg T and one
    /// value arg (a `Future[T]`); returns T. Requires `unsafe`.
    fn check_block_on(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`__cplus_block_on` is unsafe; wrap in `unsafe { ... }`".to_string(),
                call_span,
            );
        }
        if type_args.len() != 1 {
            self.err(
                "E0501",
                format!("`__cplus_block_on` takes 1 type argument, got {}", type_args.len()),
                callee.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!("`__cplus_block_on` takes 1 value argument, got {}", args.len()),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let t_ty = self.resolve_type(&type_args[0]);
        let expected_future = self.wrap_in_future(&t_ty, call_span);
        if matches!(expected_future, Ty::Error) { return Ty::Error; }
        let _ = self.check_expr(&args[0], Some(expected_future));
        t_ty
    }

    /// v0.0.3 Phase 5 Slice 5C: type-check
    /// `__cplus_thread_spawn_with::[I, O](input, f)`. Like
    /// `__cplus_thread_spawn` but with an added `input: I` arg and an
    /// fn signature of `fn(I) -> O`.
    fn check_thread_spawn_with(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`__cplus_thread_spawn_with` is unsafe; wrap in `unsafe { ... }`".to_string(),
                call_span,
            );
        }
        if type_args.len() != 2 {
            self.err(
                "E0501",
                format!("`__cplus_thread_spawn_with` takes 2 type arguments, got {}", type_args.len()),
                callee.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        if args.len() != 2 {
            self.err(
                "E0308",
                format!("`__cplus_thread_spawn_with` takes 2 value arguments, got {}", args.len()),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let i_ty = self.resolve_type(&type_args[0]);
        let o_ty = self.resolve_type(&type_args[1]);
        let _input = self.check_arg_with_move(&args[0], &ParamSig {
            ty: i_ty.clone(), mutable: false, move_: true,
        });
        let expected_f = Ty::FnPtr {
            params: vec![i_ty.clone()],
            return_type: Box::new(o_ty.clone()),
        };
        let _f = self.check_expr(&args[1], Some(expected_f));
        let Some(template) = self.struct_generic_templates.get("JoinHandle").cloned() else {
            self.err(
                "E0300",
                "`__cplus_thread_spawn_with` requires `JoinHandle[O]` from `stdlib/thread`".to_string(),
                call_span,
            );
            return Ty::Error;
        };
        self.instantiate_struct_from_arg_tys("JoinHandle", &template, vec![o_ty])
    }

    fn check_generic_named_call(
        &mut self,
        name: &str,
        gsig: &GenericFnSig,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if args.len() != gsig.params.len() {
            self.err(
                "E0308",
                format!("function `{}` takes {} argument(s), got {}", name, gsig.params.len(), args.len()),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        // Slice 7GEN.5b: explicit `::[T1, T2]` turbofish path. When the
        // user supplies type-args, validate arity and use them directly
        // as the substitution. The arg types are still checked, but
        // against the substituted parameter types — not used to infer.
        let mut concrete_args: Vec<Ty> = Vec::with_capacity(gsig.generic_params.len());
        let mut subst: HashMap<String, Ty> = HashMap::new();
        if !type_args.is_empty() {
            if type_args.len() != gsig.generic_params.len() {
                self.err(
                    "E0501",
                    format!(
                        "function `{}` takes {} type argument(s), got {}",
                        name, gsig.generic_params.len(), type_args.len()
                    ),
                    call_span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                return Ty::Error;
            }
            for (gp_name, ta) in gsig.generic_params.iter().zip(type_args.iter()) {
                let concrete = self.resolve_type(ta);
                subst.insert(gp_name.clone(), concrete.clone());
                concrete_args.push(concrete);
            }
            // Now check each arg against the substituted parameter type.
            // v0.0.3 Phase 5 Slice 5C: thread through the param's move/
            // mutable flags via `check_arg_with_move` so `move`-marked
            // params in a turbofish call correctly mark the source
            // binding as moved. Without this, calls like
            // `thread::spawn_with::[string, i64](s, f)` leave `s`
            // un-marked and post-move use is silently accepted.
            let mut had_err = false;
            for (param, arg) in gsig.params.iter().zip(args.iter()) {
                let expected = self.subst_ty_deep(&param.ty, &subst);
                let actual_before = self.check_expr(arg, Some(expected.clone()));
                if !matches!(actual_before, Ty::Error) && actual_before != expected {
                    self.err(
                        "E0302",
                        format!(
                            "type mismatch in call to `{}`: expected `{}`, got `{}`",
                            name, ty_display(&expected), actual_before.name()
                        ),
                        arg.span,
                    );
                    had_err = true;
                }
                if param.move_ && !self.is_copy(&expected) && !matches!(actual_before, Ty::Error) {
                    // Only flag named-binding moves. A non-Ident arg
                    // (StructLit, enum-variant Path, literal, fresh
                    // Call result) constructs the value in place —
                    // there's no source binding to mark moved. The
                    // strict E0337 in `consume_arg_place` would fire
                    // here for legitimate code like
                    // `io_ok::[File](File { fd: fd })`, so we
                    // sidestep it.
                    if let ExprKind::Ident(n) = &arg.kind {
                        for scope in self.scopes.iter_mut().rev() {
                            if let Some(info) = scope.get_mut(n) {
                                info.moved = true;
                                break;
                            }
                        }
                    }
                }
            }
            if had_err { return Ty::Error; }
            // Slice 7GEN.5e step 4: bound check at the turbofish path.
            self.check_generic_bounds(
                &gsig.generic_params,
                &gsig.bounds,
                &concrete_args,
                call_span,
                &format!("function `{}`", name),
            );
            self.fn_instantiations.insert((name.to_string(), concrete_args.clone()));
            self.call_monos.insert(call_span, concrete_args.clone());
            return self.subst_ty_deep(&gsig.return_type, &subst);
        }
        // Infer concrete types per param position, then unify.
        let mut had_err = false;
        // First pass: check args without an expected type to get their
        // natural type, then unify against the generic param type.
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr(a, None)).collect();
        for (param, arg_ty) in gsig.params.iter().zip(arg_tys.iter()) {
            if matches!(arg_ty, Ty::Error) { had_err = true; continue; }
            if !unify_param_against_concrete(&param.ty, arg_ty, &mut subst) {
                self.err(
                    "E0302",
                    format!(
                        "type mismatch in call to `{}`: parameter is `{}` but argument has type `{}`",
                        name, ty_display(&param.ty), arg_ty.name()
                    ),
                    args.iter().next().map(|a| a.span).unwrap_or(call_span),
                );
                had_err = true;
            }
        }
        if had_err { return Ty::Error; }
        // Ensure every declared generic param got bound.
        for gp in &gsig.generic_params {
            match subst.get(gp) {
                Some(ty) => concrete_args.push(ty.clone()),
                None => {
                    self.err(
                        "E0500",
                        format!(
                            "cannot infer type parameter `{}` for call to `{}`; \
                             supply `::[T]` turbofish or use `{}` in an argument position",
                            gp, name, gp
                        ),
                        call_span,
                    );
                    return Ty::Error;
                }
            }
        }
        // Slice 7GEN.5e step 4: bound check at the inference path.
        self.check_generic_bounds(
            &gsig.generic_params,
            &gsig.bounds,
            &concrete_args,
            call_span,
            &format!("function `{}`", name),
        );
        // Record the instantiation. The mangled name is built by the
        // monomorphize pass; sema only records the concrete args.
        self.fn_instantiations.insert((name.to_string(), concrete_args.clone()));
        self.call_monos.insert(call_span, concrete_args.clone());
        // Substitute the return type and return it as the call's type.
        self.subst_ty_deep(&gsig.return_type, &subst)
    }

    fn check_method_call(&mut self, receiver: &Expr, name: &Ident, type_args: &[Type], args: &[Expr], call_span: ByteSpan) -> Ty {
        let recv_ty = self.check_expr(receiver, None);
        if recv_ty == Ty::Error {
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        // Phase 8 slice 8.STR.3: blessed methods on owned `string`.
        if matches!(recv_ty, Ty::String) {
            if !type_args.is_empty() {
                self.err("E0501",
                    "blessed `string` methods take no type arguments".to_string(),
                    call_span);
            }
            return self.check_string_method_call(name, args, call_span);
        }
        // Phase 8 slice 8.STR.6: blessed `to_string()` on every primitive
        // + `str`. Returns `string` (owned). User-defined structs hit
        // the normal method-lookup below; if they provide
        // `impl ToString for Foo { fn to_string(self) -> string }`, that
        // path handles them.
        if name.name == "to_string" && args.is_empty() && Self::is_blessed_to_string_receiver(&recv_ty) {
            if !type_args.is_empty() {
                self.err("E0501",
                    "`to_string` takes no type arguments".to_string(),
                    call_span);
            }
            return Ty::String;
        }
        let Ty::Struct(id) = recv_ty else {
            self.err(
                "E0324",
                format!("no method `{}` on type `{}`", name.name, recv_ty.name()),
                name.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        let struct_name = self.structs[id.0 as usize].name.clone();
        let Some(sig) = self.structs[id.0 as usize].methods.get(&name.name).cloned() else {
            self.err(
                "E0324",
                format!("no method `{}` on struct `{}`", name.name, struct_name),
                name.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        let Some(rcv) = sig.receiver else {
            self.err(
                "E0327",
                format!("`{}::{}` is an associated function; call it as `{}::{}(...)`", struct_name, name.name, struct_name, name.name),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        if matches!(rcv, Receiver::Mut) && !self.is_writable_place_quiet(receiver) {
            self.err(
                "E0328",
                format!("method `{}::{}` requires a mutable receiver", struct_name, name.name),
                receiver.span,
            );
        }
        // `move self` consumes the receiver place — but only if the struct
        // is non-`Copy`. For a `Copy` struct, `move self` is a redundant
        // marker (the receiver is bitwise-copied); leave the binding usable.
        // Same rule as for `move`-marked parameters.
        if matches!(rcv, Receiver::Move) && !self.structs[id.0 as usize].is_copy {
            self.consume_place(receiver, &struct_name, &name.name);
        }
        if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!("method `{}::{}` takes {} argument(s), got {}", struct_name, name.name, sig.params.len(), args.len()),
                call_span,
            );
        }
        // Slice 7GEN.5e: generic-method dispatch. Non-generic methods
        // (most of them) fall through to plain arg-by-arg type checking;
        // generic methods route through inference + monomorphization
        // bookkeeping.
        if !sig.generic_params.is_empty() {
            return self.check_generic_method_call(
                id, &struct_name, name, &sig, type_args, args, call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "method `{}::{}` takes no type arguments but {} were provided",
                    struct_name, name.name, type_args.len()
                ),
                name.span,
            );
        }
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            self.check_arg_with_move(a, expected);
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        sig.return_type
    }

    /// Slice 7GEN.5e: type-check a generic-method call.
    /// Either uses explicit turbofish `type_args` or infers the
    /// substitution by unifying each `Ty::Param` slot against the
    /// concrete arg type (mirrors the top-level generic-fn flow).
    fn check_generic_method_call(
        &mut self,
        struct_id: StructId,
        struct_name: &str,
        name: &Ident,
        sig: &MethodSig,
        type_args: &[Type],
        args: &[Expr],
        call_span: ByteSpan,
    ) -> Ty {
        let arity = sig.generic_params.len();
        // Build the substitution map: explicit args take priority,
        // otherwise infer from each parameter slot.
        let mut subst: HashMap<String, Ty> = HashMap::new();
        if !type_args.is_empty() {
            if type_args.len() != arity {
                self.err(
                    "E0501",
                    format!(
                        "method `{}::{}` takes {} type argument(s), got {}",
                        struct_name, name.name, arity, type_args.len()
                    ),
                    name.span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                return Ty::Error;
            }
            for (gp, ta) in sig.generic_params.iter().zip(type_args.iter()) {
                let resolved = self.resolve_type(ta);
                if matches!(resolved, Ty::Error) {
                    for a in args { let _ = self.check_expr(a, None); }
                    return Ty::Error;
                }
                subst.insert(gp.clone(), resolved);
            }
        } else {
            // Infer: walk params, unify Ty::Param against arg type.
            for (param_sig, arg) in sig.params.iter().zip(args.iter()) {
                let arg_ty = self.check_expr(arg, None);
                if matches!(arg_ty, Ty::Error) { continue; }
                unify_param_against_concrete(&param_sig.ty, &arg_ty, &mut subst);
            }
            // Every declared generic param must be pinned.
            for gp in &sig.generic_params {
                if !subst.contains_key(gp) {
                    self.err(
                        "E0500",
                        format!(
                            "cannot infer type parameter `{}` for method `{}::{}` \
                             (supply `::[T]` turbofish or use `T` in an argument position)",
                            gp, struct_name, name.name
                        ),
                        call_span,
                    );
                    return Ty::Error;
                }
            }
        }
        // Type-check each arg against the *substituted* parameter type.
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            let expected_ty = self.subst_ty_deep(&expected.ty, &subst);
            let arg_ty = self.check_expr(a, Some(expected_ty.clone()));
            if !matches!(arg_ty, Ty::Error) && arg_ty != expected_ty {
                self.err(
                    "E0302",
                    format!(
                        "type mismatch in method `{}::{}` arg: expected `{}`, got `{}`",
                        struct_name, name.name,
                        ty_display(&expected_ty),
                        ty_display(&arg_ty),
                    ),
                    a.span,
                );
            }
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        // Record the instantiation for monomorphize.
        let arg_tys: Vec<Ty> = sig.generic_params.iter()
            .map(|gp| subst.get(gp).cloned().unwrap_or(Ty::Error))
            .collect();
        // Slice 7GEN.5e step 4: bound check for method-level generics.
        self.check_generic_bounds(
            &sig.generic_params,
            &sig.generic_bounds,
            &arg_tys,
            call_span,
            &format!("method `{}::{}`", struct_name, name.name),
        );
        let key = (struct_id, name.name.clone(), arg_tys.clone());
        self.method_instantiations.insert(key);
        self.call_monos.insert(call_span, arg_tys);
        self.subst_ty_deep(&sig.return_type, &subst)
    }

    /// Phase 8 slice 8.STR.B: type-check an interpolated string literal.
    /// Walk each part; each `Expr` part must have a type that satisfies
    /// `ToString` (blessed primitives + `str`, or a user-declared
    /// `impl ToString for Foo`). Result type is `Ty::String`.
    fn check_interp_str(&mut self, parts: &[crate::ast::InterpStrPart], span: ByteSpan) -> Ty {
        use crate::ast::InterpStrPart;
        for part in parts {
            if let InterpStrPart::Expr(e) = part {
                let ty = self.check_expr(e, None);
                if matches!(ty, Ty::Error) { continue; }
                let ok = Self::is_blessed_to_string_receiver(&ty)
                    || matches!(&ty, Ty::String)
                    || matches!(&ty, Ty::Struct(id)
                        if self.interface_impls.contains(&(
                            "ToString".to_string(),
                            self.structs[id.0 as usize].name.clone(),
                        )));
                if !ok {
                    self.err(
                        "E0612",
                        format!(
                            "type `{}` does not implement `ToString`; \
                             cannot embed in an interpolation `${{...}}` segment",
                            ty_display(&ty)
                        ),
                        e.span,
                    );
                }
            }
        }
        // Future: if the literal has *zero* Expr parts after sub-parsing,
        // could downgrade to Ty::Str. The lexer already returns a plain
        // Str token in that case, so we don't reach here.
        let _ = span;
        Ty::String
    }

    /// Phase 8 slice 8.STR.6: which receiver types get a blessed
    /// `to_string()` method? Every numeric primitive + `bool` + `str`.
    /// `string` is intentionally NOT in this list — its `.clone()` is
    /// the way to duplicate an owned string, and `s.to_string()` would
    /// be a redundant alias. User-declared structs aren't here either
    /// — they go through the normal method-lookup with an
    /// `impl ToString for Foo` provided by the user.
    fn is_blessed_to_string_receiver(ty: &Ty) -> bool {
        matches!(ty,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize
            | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Usize
            | Ty::F32 | Ty::F64
            | Ty::Bool
            | Ty::Str)
    }

    /// Phase 8 slice 8.STR.3: dispatch `s.method(args)` on a `string`
    /// receiver. Methods: `len() -> usize`, `is_empty() -> bool`,
    /// `as_str() -> str`, `clone() -> string`. Anything else fires E0324.
    fn check_string_method_call(&mut self, name: &Ident, args: &[Expr], call_span: ByteSpan) -> Ty {
        let no_args = |this: &mut Self| -> bool {
            if !args.is_empty() {
                this.err("E0308",
                    format!("`string::{}` takes 0 argument(s), got {}", name.name, args.len()),
                    call_span);
                for a in args { let _ = this.check_expr(a, None); }
                false
            } else { true }
        };
        match name.name.as_str() {
            "len" => { let _ = no_args(self); Ty::Usize }
            "is_empty" => { let _ = no_args(self); Ty::Bool }
            "as_str" => { let _ = no_args(self); Ty::Str }
            "clone" => { let _ = no_args(self); Ty::String }
            _ => {
                self.err("E0324",
                    format!("no method `{}` on type `string`", name.name),
                    name.span);
                for a in args { let _ = self.check_expr(a, None); }
                Ty::Error
            }
        }
    }

    /// Phase 8 slice 8.STR.3: dispatch `string::method(args)`. Only two
    /// associated fns ship in v1: `new` (no args, returns empty `string`)
    /// and `with_capacity(n: usize)` (returns a string with `n` bytes
    /// pre-allocated). Anything else fires E0324.
    fn check_string_assoc_call(&mut self, method: &Ident, args: &[Expr], call_span: ByteSpan) -> Ty {
        match method.name.as_str() {
            "new" => {
                if !args.is_empty() {
                    self.err("E0308",
                        format!("`string::new` takes 0 argument(s), got {}", args.len()),
                        call_span);
                }
                Ty::String
            }
            "with_capacity" => {
                if args.len() != 1 {
                    self.err("E0308",
                        format!("`string::with_capacity` takes 1 argument, got {}", args.len()),
                        call_span);
                    for a in args { let _ = self.check_expr(a, None); }
                    return Ty::Error;
                }
                let arg_ty = self.check_expr(&args[0], Some(Ty::Usize));
                if !matches!(arg_ty, Ty::Usize | Ty::Error) {
                    self.err("E0302",
                        format!("`string::with_capacity` expects `usize`, got `{}`", ty_display(&arg_ty)),
                        args[0].span);
                }
                Ty::String
            }
            _ => {
                self.err("E0324",
                    format!("no associated function `{}` on type `string`", method.name),
                    method.span);
                for a in args { let _ = self.check_expr(a, None); }
                Ty::Error
            }
        }
    }

    fn check_assoc_call(&mut self, segments: &[Ident], type_args: &[Type], args: &[Expr], path_span: ByteSpan, call_span: ByteSpan) -> Ty {
        if segments.len() != 2 {
            self.err("E0312", "Phase 2 paths have exactly two segments".to_string(), path_span);
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        let type_seg = &segments[0];
        let method_seg = &segments[1];
        // Phase 8 slice 8.STR.3: blessed assoc fns on the owned `string`
        // type. `string::new()` and `string::with_capacity(n)` are the
        // only ways to construct an owned string in user code (besides
        // interpolation literals — slice 8.STR.B).
        if type_seg.name == "string" {
            if !type_args.is_empty() {
                self.err("E0501",
                    "`string::{new,with_capacity}` take no type arguments".to_string(),
                    call_span);
            }
            return self.check_string_assoc_call(method_seg, args, call_span);
        }
        // Enums: a call shape `Name::Variant(args)` constructs a tagged
        // variant. Look up the variant; verify it has a payload (call form
        // is illegal for payload-less variants — use the bare path); check
        // arg count and types against the payload.
        if let Some(&id) = self.enum_by_name.get(&type_seg.name) {
            let enum_def = self.enums[id.0 as usize].clone();
            let variant = enum_def.variants.iter()
                .find(|v| v.name == method_seg.name);
            let Some(vdef) = variant else {
                self.err(
                    "E0317",
                    format!("enum `{}` has no variant `{}`", type_seg.name, method_seg.name),
                    method_seg.span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                return Ty::Error;
            };
            if vdef.payload.is_empty() {
                // Payload-less variant called with parens — point users at
                // the bare path syntax.
                self.err(
                    "E0327",
                    format!("variant `{}::{}` has no payload; use the bare path `{}::{}`", type_seg.name, method_seg.name, type_seg.name, method_seg.name),
                    call_span,
                );
                for a in args { let _ = self.check_expr(a, None); }
                return Ty::Error;
            }
            if args.len() != vdef.payload.len() {
                self.err(
                    "E0342",
                    format!("variant `{}::{}` takes {} payload value(s); got {}", type_seg.name, method_seg.name, vdef.payload.len(), args.len()),
                    call_span,
                );
            }
            for (a, expected_ty) in args.iter().zip(vdef.payload.iter()) {
                let _ = self.check_expr(a, Some(expected_ty.clone()));
            }
            for a in args.iter().skip(vdef.payload.len()) {
                let _ = self.check_expr(a, None);
            }
            return Ty::Enum(id);
        }
        let Some(&id) = self.struct_by_name.get(&type_seg.name) else {
            self.err("E0303", format!("unknown type `{}`", type_seg.name), type_seg.span);
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        let struct_name = self.structs[id.0 as usize].name.clone();
        let Some(sig) = self.structs[id.0 as usize].methods.get(&method_seg.name).cloned() else {
            self.err(
                "E0324",
                format!("struct `{}` has no method `{}`", struct_name, method_seg.name),
                method_seg.span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        };
        if sig.receiver.is_some() {
            self.err(
                "E0327",
                format!("`{}::{}` is an instance method; call it as `value.{}(...)`", struct_name, method_seg.name, method_seg.name),
                call_span,
            );
            for a in args { let _ = self.check_expr(a, None); }
            return Ty::Error;
        }
        if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!("function `{}::{}` takes {} argument(s), got {}", struct_name, method_seg.name, sig.params.len(), args.len()),
                call_span,
            );
        }
        // Slice 7GEN.5e: generic-method dispatch on assoc-call form
        // (`Type::method(...)` / `Type::method::[T](...)`).
        if !sig.generic_params.is_empty() {
            return self.check_generic_method_call(
                id, &struct_name, method_seg, &sig, type_args, args, call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "associated function `{}::{}` takes no type arguments but {} were provided",
                    struct_name, method_seg.name, type_args.len()
                ),
                method_seg.span,
            );
        }
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            self.check_arg_with_move(a, expected);
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        sig.return_type
    }

    /// Type-check a single call argument and apply move tracking. If the
    /// parameter is `move` and the argument's type is non-Copy, the source
    /// place is consumed:
    ///   - Plain Ident referencing a local: mark the binding as moved.
    ///   - Anything else (Field/Index/temp): reject as E0337 — partial moves
    ///     out of struct fields or array slots are deferred to Phase 5/6.
    /// `Copy`-typed arguments are unaffected — the `move` marker on a Copy
    /// parameter is redundant (a future E0336 lint will suggest removing it).
    fn check_arg_with_move(&mut self, arg: &Expr, expected: &ParamSig) {
        let _ = self.check_expr(arg, Some(expected.ty.clone()));
        if expected.move_ && !self.is_copy(&expected.ty) {
            self.consume_arg_place(arg);
        }
    }

    /// Mark the source binding of an argument as moved. Used by both
    /// `move`-param calls and `move self` receivers. Only plain Ident
    /// references to a local binding are accepted; anything else triggers
    /// E0337 (partial moves deferred).
    fn consume_arg_place(&mut self, arg: &Expr) {
        match &arg.kind {
            ExprKind::Ident(name) => {
                // Find the binding's scope and mark moved. `resolve_value_ident`
                // already ran via `check_expr` and would have produced E0335
                // if the binding was *already* moved; here we just record the
                // new move state.
                for scope in self.scopes.iter_mut().rev() {
                    if let Some(info) = scope.get_mut(name) {
                        info.moved = true;
                        return;
                    }
                }
                // Unknown name — error was already produced by check_expr.
            }
            _ => {
                self.err(
                    "E0337",
                    "cannot move out of this expression; only whole-binding moves are supported in Phase 3 (partial moves of fields or array slots are deferred)".to_string(),
                    arg.span,
                );
            }
        }
    }

    /// Same as `consume_arg_place` but for the receiver in a `move self`
    /// method call. Diagnostic phrasing names the method for clarity.
    fn consume_place(&mut self, receiver: &Expr, type_name: &str, method_name: &str) {
        match &receiver.kind {
            ExprKind::Ident(name) => {
                for scope in self.scopes.iter_mut().rev() {
                    if let Some(info) = scope.get_mut(name) {
                        info.moved = true;
                        return;
                    }
                }
            }
            _ => {
                self.err(
                    "E0337",
                    format!("method `{}::{}` consumes `self`; the receiver must be a whole binding (partial moves are deferred to a later phase)", type_name, method_name),
                    receiver.span,
                );
            }
        }
    }

    fn is_writable_place_quiet(&self, target: &Expr) -> bool {
        match &target.kind {
            ExprKind::Ident(name) => {
                matches!(self.lookup_local(name), Some(info) if info.mutable)
            }
            ExprKind::Field { receiver, .. } => self.is_writable_place_quiet(receiver),
            ExprKind::Index { receiver, .. } => self.is_writable_place_quiet(receiver),
            _ => false,
        }
    }

    fn check_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: ByteSpan) -> Ty {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let lhs_ty = self.check_expr(lhs, None);
                // Slice 10.FFI.2: pointer arithmetic. `p + n` and
                // `p - n` produce another pointer of the same pointee;
                // `n` must be `usize`. Multiplication and division on
                // pointers stay rejected.
                if matches!(lhs_ty, Ty::RawPtr(_)) && matches!(op, BinOp::Add | BinOp::Sub) {
                    let _ = self.check_expr(rhs, Some(Ty::Usize));
                    return lhs_ty;
                }
                if !lhs_ty.is_numeric() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("`{}` requires numeric operands, found `{}`", op_str(op), lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty.clone()));
                lhs_ty
            }
            BinOp::Mod => {
                let lhs_ty = self.check_expr(lhs, None);
                if lhs_ty.is_float() {
                    self.err(
                        "E0316",
                        "modulo (`%`) on float types is not supported".to_string(),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                if !lhs_ty.is_int() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("`%` requires integer operands, found `{}`", lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty.clone()));
                lhs_ty
            }
            BinOp::Eq | BinOp::Ne => {
                let lt = self.check_expr(lhs, None);
                if lt.is_struct() {
                    self.err(
                        "E0302",
                        format!("`==` / `!=` are not implemented for struct types in Phase 2; write your own equality function"),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Bool;
                }
                let _ = self.check_expr(rhs, Some(lt));
                Ty::Bool
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let lhs_ty = self.check_expr(lhs, None);
                if !lhs_ty.is_numeric() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("ordered comparison requires numeric operands, found `{}`", lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Bool;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty));
                Ty::Bool
            }
            BinOp::And | BinOp::Or => {
                self.check_expr(lhs, Some(Ty::Bool));
                self.check_expr(rhs, Some(Ty::Bool));
                Ty::Bool
            }
            BinOp::AddWrap | BinOp::SubWrap | BinOp::MulWrap => {
                let lhs_ty = self.check_expr(lhs, None);
                if !lhs_ty.is_int() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!("`{}` requires integer operands, found `{}`", op_str(op), lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                let _ = self.check_expr(rhs, Some(lhs_ty.clone()));
                lhs_ty
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                // Phase 3A: bitwise ops are defined on every integer type
                // (signed + unsigned, every width). Floats, bool, ptrs are
                // rejected — there's no surface-language meaning for
                // bit-twiddling those. `bool & bool` uses `&&` instead.
                let lhs_ty = self.check_expr(lhs, None);
                if lhs_ty == Ty::Error {
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                if !lhs_ty.is_int() {
                    self.err(
                        "E0302",
                        format!("`{}` requires integer operands, found `{}`", op_str(op), lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                // RHS must be the same integer type as LHS. Coerce
                // unsuffixed integer literals to match.
                let _ = self.check_expr(rhs, Some(lhs_ty.clone()));
                lhs_ty
            }
            BinOp::Shl | BinOp::Shr => {
                // Phase 3A: shift result type is the LHS's integer type.
                // The shift count is an integer; it does NOT have to match
                // the LHS width (C lets `i64 << u8`; same here). Signed
                // LHS uses arithmetic shift right; unsigned uses logical.
                // Codegen picks `ashr` vs `lshr` from the LHS signedness.
                let lhs_ty = self.check_expr(lhs, None);
                if lhs_ty == Ty::Error {
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                if !lhs_ty.is_int() {
                    self.err(
                        "E0302",
                        format!("`{}` requires an integer left operand, found `{}`", op_str(op), lhs_ty.name()),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Error;
                }
                // Shift count: integer, any width. Don't constrain to a
                // specific type — let any int through and let codegen
                // narrow/widen to the LHS width.
                let rhs_ty = self.check_expr(rhs, None);
                if rhs_ty != Ty::Error && !rhs_ty.is_int() {
                    self.err(
                        "E0302",
                        format!("shift count must be an integer, found `{}`", rhs_ty.name()),
                        span,
                    );
                    return Ty::Error;
                }
                lhs_ty
            }
        }
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr, span: ByteSpan) -> Ty {
        match op {
            UnaryOp::Neg => {
                let t = self.check_expr(operand, None);
                if t == Ty::Error { return Ty::Error; }
                if t.is_unsigned_int() {
                    self.err(
                        "E0302",
                        format!("cannot negate unsigned type `{}`; use a signed type instead", t.name()),
                        span,
                    );
                    return Ty::Error;
                }
                if !t.is_signed_int() && !t.is_float() {
                    self.err(
                        "E0302",
                        format!("unary `-` requires a numeric operand, found `{}`", t.name()),
                        span,
                    );
                    return Ty::Error;
                }
                t
            }
            UnaryOp::Not => { self.check_expr(operand, Some(Ty::Bool)); Ty::Bool }
            UnaryOp::BitNot => {
                // Phase 3A: bitwise NOT is defined on every integer type.
                // Codegen lowers via `xor i<N> v, -1` per LLVM idiom.
                let t = self.check_expr(operand, None);
                if t == Ty::Error { return Ty::Error; }
                if !t.is_int() {
                    self.err(
                        "E0302",
                        format!("`~` requires an integer operand, found `{}`", t.name()),
                        span,
                    );
                    return Ty::Error;
                }
                t
            }
            UnaryOp::Ref { .. } => {
                self.err("E0312", "references are not yet supported (Phase 5/6)".to_string(), span);
                let _ = self.check_expr(operand, None);
                Ty::Error
            }
            UnaryOp::Deref => {
                // Slice 10.FFI.2: `*p` reads through a raw pointer.
                // Slice 10.FFI.3: requires `unsafe` block — reads
                // through a raw pointer can violate memory safety.
                let op_ty = self.check_expr(operand, None);
                if matches!(op_ty, Ty::RawPtr(_)) && self.unsafe_depth == 0 {
                    self.err(
                        "E0801",
                        "dereferencing a raw pointer is unsafe; wrap in `unsafe { ... }`".to_string(),
                        span,
                    );
                }
                match op_ty {
                    Ty::RawPtr(inner) => *inner,
                    Ty::Error => Ty::Error,
                    other => {
                        self.err(
                            "E0302",
                            format!("dereference requires a raw pointer (`*T`), got `{}`", ty_display(&other)),
                            span,
                        );
                        Ty::Error
                    }
                }
            }
        }
    }

    fn check_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr, span: ByteSpan) -> Ty {
        // Special case: first write to an unassigned binding via a direct
        // Ident target. This is the initialization site of a `let x: T;`
        // — allowed regardless of `mut` (it's the binding's first value,
        // not a reassignment). After this, the binding is marked assigned
        // and any further writes require the binding to be `mut`.
        // Compound assigns can't init (no prior value to read), so only
        // plain `=` takes this path.
        if matches!(op, AssignOp::Assign) {
            if let ExprKind::Ident(name) = &target.kind {
                let unassigned = self.lookup_local(name)
                    .map(|info| !info.assigned)
                    .unwrap_or(false);
                if unassigned {
                    let target_ty = self.lookup_local(name).map(|i| i.ty.clone()).unwrap_or(Ty::Error);
                    if target_ty != Ty::Error {
                        self.check_expr(value, Some(target_ty));
                    } else {
                        let _ = self.check_expr(value, None);
                    }
                    for scope in self.scopes.iter_mut().rev() {
                        if let Some(info) = scope.get_mut(name) {
                            info.assigned = true;
                            break;
                        }
                    }
                    return Ty::Unit;
                }
            }
        }
        // Regular case: target must be a place rooted at a mutable local.
        if !self.target_is_writable_place(target) {
            let _ = self.check_expr(value, None);
            return Ty::Error;
        }
        let target_ty = self.check_expr(target, None);
        // v0.0.3 Slice 3A: compound assigns (`+=`, `-=`, `*=`, `/=`, `%=`,
        // `&=`, `|=`, `^=`, `<<=`, `>>=`). Each is `a OP= b` ≡ `a = a OP b`;
        // type-check the equivalent binary expr to enforce the same
        // type-rules as the standalone binary op. The +%/-%/*%/etc. wrapping
        // variants are NOT covered — wrapping ops don't have compound forms
        // in C+ (use `a = a +% b` explicitly).
        let value_ty = self.check_expr(value, if target_ty == Ty::Error { None } else { Some(target_ty.clone()) });
        if !matches!(op, AssignOp::Assign) && target_ty != Ty::Error && value_ty != Ty::Error {
            // Check the op is type-compatible with target_ty. For arithmetic
            // ops (+=, -=, *=, /=, %=) — operand must be a numeric type
            // (and float for /% is rejected). For bitwise/shift — operand
            // must be an integer.
            let (op_label, is_arith, is_bitwise) = match op {
                AssignOp::AddAssign => ("`+=`", true, false),
                AssignOp::SubAssign => ("`-=`", true, false),
                AssignOp::MulAssign => ("`*=`", true, false),
                AssignOp::DivAssign => ("`/=`", true, false),
                AssignOp::ModAssign => ("`%=`", true, false),
                AssignOp::BitAndAssign => ("`&=`", false, true),
                AssignOp::BitOrAssign  => ("`|=`", false, true),
                AssignOp::BitXorAssign => ("`^=`", false, true),
                AssignOp::ShlAssign    => ("`<<=`", false, true),
                AssignOp::ShrAssign    => ("`>>=`", false, true),
                AssignOp::Assign => unreachable!(),
            };
            let int_ok = target_ty.is_signed_int() || target_ty.is_unsigned_int();
            let arith_ok = int_ok || matches!(target_ty, Ty::F32 | Ty::F64);
            if is_arith && !arith_ok {
                self.err(
                    "E0302",
                    format!("{op_label} requires a numeric type, got `{}`", ty_display(&target_ty)),
                    span,
                );
            }
            if is_bitwise && !int_ok {
                self.err(
                    "E0302",
                    format!("{op_label} requires an integer type, got `{}`", ty_display(&target_ty)),
                    span,
                );
            }
        }
        Ty::Unit
    }

    /// A place is an Ident referring to a mutable local, or a Field chain
    /// rooted at one. Anything else errors with E0313 / E0305 / E0300.
    fn target_is_writable_place(&mut self, target: &Expr) -> bool {
        match &target.kind {
            ExprKind::Ident(name) => {
                let local = self.lookup_local(name).cloned();
                let Some(info) = local else {
                    self.err("E0300", format!("undefined name `{name}`"), target.span);
                    return false;
                };
                if !info.mutable {
                    self.err(
                        "E0305",
                        format!("cannot assign to immutable binding `{name}`; declare it as `let mut`"),
                        target.span,
                    );
                    return false;
                }
                true
            }
            ExprKind::Field { receiver, .. } => self.target_is_writable_place(receiver),
            ExprKind::Index { receiver, .. } => {
                // Slice 10.FFI.2: `p[i] = v` where p is `*T` doesn't
                // require `p` to be `mut` — the pointer value itself
                // isn't being reassigned; the memory at offset `i` is.
                // This mirrors C semantics.
                let recv_ty = self.check_expr(receiver, None);
                if matches!(recv_ty, Ty::RawPtr(_)) {
                    return true;
                }
                self.target_is_writable_place(receiver)
            }
            // Slice 10.FFI.2: `*p = v` is a write through a raw pointer.
            // The pointer binding `p` doesn't need to be `mut` — what's
            // being mutated is the target memory, not the binding.
            // The deref's writability is gated by `operand` being a
            // pointer type (sema checks during `check_expr`).
            ExprKind::Unary { op: UnaryOp::Deref, operand } => {
                let op_ty = self.check_expr(operand, None);
                if !matches!(op_ty, Ty::RawPtr(_) | Ty::Error) {
                    self.err(
                        "E0302",
                        format!("dereference assignment target must be a raw pointer, got `{}`", ty_display(&op_ty)),
                        target.span,
                    );
                    return false;
                }
                true
            }
            _ => {
                self.err(
                    "E0313",
                    "assignment target is not a place expression".to_string(),
                    target.span,
                );
                false
            }
        }
    }

    // ---- name + type resolution ----

    fn resolve_type(&mut self, t: &Type) -> Ty {
        let name = match &t.kind {
            TypeKind::Path(n) => n,
            TypeKind::Array { elem, len } => {
                let elem_ty = self.resolve_type(elem);
                return Ty::Array(Box::new(elem_ty), *len);
            }
            // Slice 6BC.5: region annotations are transparent at the
            // sema level — `borrow A T` resolves to the same `Ty` as
            // T. The region is borrow-checker metadata.
            TypeKind::Borrowed { inner, .. } => return self.resolve_type(inner),
            // Slice 7GEN.5c: resolve generic-struct instantiation —
            // returns the StructId for the synthesized concrete struct.
            TypeKind::Generic { name, args } => {
                return self.resolve_generic_instantiation(name, args, t.span);
            }
            // Slice 10.FFI.1: raw pointer type.
            TypeKind::RawPtr(inner) => {
                let inner_ty = self.resolve_type(inner);
                return Ty::RawPtr(Box::new(inner_ty));
            }
            // Slice 11.FN_PTR: function pointer type.
            TypeKind::FnPtr { params, return_type } => {
                let resolved_params: Vec<Ty> = params.iter().map(|p| self.resolve_type(p)).collect();
                let resolved_ret = match return_type {
                    Some(rt) => self.resolve_type(rt),
                    None => Ty::Unit,
                };
                return Ty::FnPtr { params: resolved_params, return_type: Box::new(resolved_ret) };
            }
            // Phase 11 polish (2026-05-14): slice type `T[]`.
            TypeKind::Slice(inner) => {
                let inner_ty = self.resolve_type(inner);
                return Ty::Slice(Box::new(inner_ty));
            }
        };
        match name.as_str() {
            "i8" => Ty::I8, "i16" => Ty::I16, "i32" => Ty::I32, "i64" => Ty::I64,
            "u8" => Ty::U8, "u16" => Ty::U16, "u32" => Ty::U32, "u64" => Ty::U64,
            "isize" => Ty::Isize, "usize" => Ty::Usize,
            "f32" => Ty::F32, "f64" => Ty::F64,
            "bool" => Ty::Bool,
            "str" => Ty::Str,
            "string" => Ty::String,
            _ => {
                // Slice 7GEN.4: `Self` inside an `impl Type { ... }` body
                // resolves to the impl target's concrete `Ty`. Inside an
                // `interface { ... }` body it stays abstract as
                // `Ty::Param("Self")` — that case falls through to the
                // generic-param lookup below.
                if name == "Self" {
                    if let Some(ty) = self.self_type_stack.last() {
                        return ty.clone();
                    }
                    // Outside any impl/interface context, `Self` is an error.
                    // It's also recognized as a generic-param name inside an
                    // interface body (pushed in `collect_interfaces`), so the
                    // lookup below catches that case before falling through
                    // to E0508.
                    if !self.type_param_in_scope("Self") {
                        self.err(
                            "E0508",
                            "`Self` is only valid inside an `interface` or `impl` body".to_string(),
                            t.span,
                        );
                        return Ty::Error;
                    }
                }
                // Slice 7GEN.4: a name matching a declared generic
                // parameter resolves to `Ty::Param(name)` rather than
                // erroring. The substitution to a concrete type happens
                // at instantiation/codegen time (slice 7GEN.5).
                if self.type_param_in_scope(name) {
                    return Ty::Param(name.clone());
                }
                if let Some(&id) = self.enum_by_name.get(name) {
                    return Ty::Enum(id);
                }
                if let Some(&id) = self.struct_by_name.get(name) {
                    return Ty::Struct(id);
                }
                // Phase 11 polish: transparent type alias. Recurse into
                // the target. Cycle detection via the resolving set —
                // E0510 if the same alias name appears mid-resolution.
                if let Some(target) = self.type_aliases.get(name).cloned() {
                    if !self.resolving_aliases.insert(name.clone()) {
                        self.err(
                            "E0510",
                            format!("type alias `{}` references itself", name),
                            t.span,
                        );
                        return Ty::Error;
                    }
                    let resolved = self.resolve_type(&target);
                    self.resolving_aliases.remove(name);
                    return resolved;
                }
                self.err("E0303", format!("unknown type `{name}`"), t.span);
                Ty::Error
            }
        }
    }

    /// Slice 7GEN.5c: resolve `Name[arg1, arg2, ...]` to a concrete
    /// `Ty::Struct(id)`. Synthesizes a new `StructDef` per unique
    /// `(name, args)` pair; subsequent references with the same args
    /// share the same `StructId`. Field types are substituted using
    /// the type-arg map.
    ///
    /// Reports:
    /// - **E0303** — unknown generic name.
    /// - **E0501** — wrong number of type arguments.
    fn resolve_generic_instantiation(
        &mut self,
        name: &str,
        args: &[Type],
        span: ByteSpan,
    ) -> Ty {
        // Slice 7GEN.5d: try enum templates first. Struct + enum names
        // live in different tables but the resolution shape is parallel.
        if self.enum_generic_templates.contains_key(name) {
            return self.resolve_generic_enum_instantiation(name, args, span);
        }
        let Some(template) = self.struct_generic_templates.get(name).cloned() else {
            self.err(
                "E0303",
                format!("unknown generic type `{}`", name),
                span,
            );
            return Ty::Error;
        };
        if args.len() != template.generic_params.len() {
            self.err(
                "E0501",
                format!(
                    "type `{}` takes {} type argument(s), got {}",
                    name, template.generic_params.len(), args.len()
                ),
                span,
            );
            return Ty::Error;
        }
        // Resolve each arg to a concrete Ty.
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
        if arg_tys.iter().any(|t| matches!(t, Ty::Error)) {
            return Ty::Error;
        }
        // Slice 7GEN.5e step 4: bound check (only at AST-originating call
        // sites — substitution paths via `subst_ty` skip this because the
        // template's bounds were already checked at the originating site).
        let param_names: Vec<String> = template.generic_params.iter()
            .map(|g| g.name.name.clone()).collect();
        let bounds: Vec<Vec<String>> = template.generic_params.iter()
            .map(|g| g.bounds.iter().map(|b| b.name.clone()).collect())
            .collect();
        self.check_generic_bounds(&param_names, &bounds, &arg_tys, span,
            &format!("generic struct `{}`", name));
        self.instantiate_struct_from_arg_tys(name, &template, arg_tys)
    }

    /// Slice 7GEN.5c (factored 2026-05-13): the post-arg-resolution body
    /// of `resolve_generic_instantiation`. Takes already-resolved `arg_tys`
    /// and a cloned template; synthesizes the concrete `StructDef` (or
    /// returns the existing dedup'd id). Used both from AST-typed entry
    /// (after `resolve_type` on each arg) and from `subst_ty`'s recursion
    /// through nested generic structs (which already has `Ty`s in hand).
    /// Skips bound-checking — callers do it when they have AST spans.
    fn instantiate_struct_from_arg_tys(
        &mut self,
        name: &str,
        template: &crate::ast::StructDecl,
        arg_tys: Vec<Ty>,
    ) -> Ty {
        // Dedup against prior instantiations.
        let key = (name.to_string(), arg_tys.clone());
        if let Some(&existing) = self.struct_instantiations.get(&key) {
            return Ty::Struct(existing);
        }
        // Synthesize a new concrete StructDef. Substitute generic-param
        // names in the template's field types using the arg map; resolve
        // the resulting types to `Ty`s.
        let subst: HashMap<String, Ty> = template
            .generic_params
            .iter()
            .map(|g| g.name.name.clone())
            .zip(arg_tys.iter().cloned())
            .collect();
        let mut fields: Vec<(String, Ty, bool)> = Vec::with_capacity(template.fields.len());
        let mut seen: HashMap<String, ()> = HashMap::new();
        for f in &template.fields {
            if seen.contains_key(&f.name.name) {
                // Template's field-uniqueness is the user's responsibility;
                // they get the same E0319 they would have gotten from the
                // template itself.
                continue;
            }
            seen.insert(f.name.name.clone(), ());
            let resolved = self.resolve_field_type_with_subst(&f.ty, &subst);
            fields.push((f.name.name.clone(), resolved, f.is_pub));
        }
        let mangled = mangle_generic_struct_name(name, &arg_tys, &self.structs, &self.enums);
        let id = StructId(self.structs.len() as u32);
        self.structs.push(StructDef {
            name: mangled.clone(),
            fields,
            methods: HashMap::new(),
            is_copy: false,   // recomputed by compute_struct_copy_flags? not for late-synthesized
            is_drop: false,
            is_repr_c: false,   // generic instantiations don't inherit repr(C); revisit when use case appears
            origin_file: None,
            generic_origin: Some((name.to_string(), arg_tys.clone())),
        });
        self.struct_by_name.insert(mangled.clone(), id);
        self.struct_instantiations.insert(key, id);
        // Slice 7GEN.5e step 3: populate methods on the synthesized
        // StructDef from any `impl Vec[T] { ... }` blocks registered
        // for this template. Substitute impl-level `T` references
        // (Ty::Param) with the concrete arg types; `Self` references
        // resolve to the new Ty::Struct(id).
        if let Some(templates) = self.generic_impl_methods.get(name).cloned() {
            let self_ty = Ty::Struct(id);
            for t in &templates {
                let mut method_subst: HashMap<String, Ty> = HashMap::new();
                for (gp, arg) in t.impl_generic_params.iter().zip(arg_tys.iter()) {
                    method_subst.insert(gp.clone(), arg.clone());
                }
                // 7GEN.5c carry-forward (2026-05-13): use subst_ty_deep so
                // method signatures mentioning *generic structs*
                // (e.g. `fn new(v: T) -> Box[T]` inside `impl Box[T]`)
                // get their inner T substituted at instantiation time.
                let resolved_params: Vec<ParamSig> = {
                    let raw: Vec<(Ty, bool, bool)> = t.params.iter()
                        .map(|p| (p.ty.clone(), p.mutable, p.move_)).collect();
                    raw.into_iter().map(|(ty, mutable, move_)| {
                        let s = self.subst_ty_deep(&ty, &method_subst);
                        ParamSig { ty: subst_self(&s, &self_ty), mutable, move_ }
                    }).collect()
                };
                let resolved_return = {
                    let s = self.subst_ty_deep(&t.return_type, &method_subst);
                    subst_self(&s, &self_ty)
                };
                self.structs[id.0 as usize].methods.insert(
                    t.name.clone(),
                    MethodSig {
                        receiver: t.receiver,
                        params: resolved_params,
                        return_type: resolved_return,
                        // Method-level generic params survive — a method
                        // can still be `fn map[U]` inside `impl Vec[T]`.
                        generic_params: t.method_generic_params.clone(),
                        generic_bounds: Vec::new(),
                    },
                );
            }
        }
        Ty::Struct(id)
    }

    /// Slice 7GEN.5c carry-forward (closed 2026-05-13): substitute generic
    /// params *and* recurse through nested generic struct/enum types. This
    /// is the deep version of `subst_ty` — when the input contains a
    /// `Ty::Struct(id)` that was itself a generic instantiation (recorded
    /// in `generic_origin`), we substitute its args and re-instantiate via
    /// the dedup'd helpers. Closes the long-standing bug where
    /// `fn make[T]() -> Box[T]` left `T` inside `Box[T]` unsubstituted.
    fn subst_ty_deep(&mut self, ty: &Ty, subst: &HashMap<String, Ty>) -> Ty {
        match ty {
            Ty::Param(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Ty::Array(elem, len) => Ty::Array(Box::new(self.subst_ty_deep(elem, subst)), *len),
            // v0.0.3 1P.2 follow-up: slice element types need substitution too.
            // Without this, `impl Vec[T] { fn as_slice(self) -> T[] }`
            // monomorphizes to a function still returning `T[]` (with T as
            // a Param), so consumers calling `.as_slice()` on a `Vec[u8]`
            // got a slice-of-Param instead of `u8[]`.
            Ty::Slice(elem) => Ty::Slice(Box::new(self.subst_ty_deep(elem, subst))),
            Ty::RawPtr(inner) => Ty::RawPtr(Box::new(self.subst_ty_deep(inner, subst))),
            Ty::FnPtr { params, return_type } => {
                let params = params.iter().map(|p| self.subst_ty_deep(p, subst)).collect();
                let return_type = Box::new(self.subst_ty_deep(return_type, subst));
                Ty::FnPtr { params, return_type }
            }
            Ty::Struct(id) => {
                let origin = self.structs[id.0 as usize].generic_origin.clone();
                let Some((name, args)) = origin else { return ty.clone(); };
                let new_args: Vec<Ty> = args.iter().map(|a| self.subst_ty_deep(a, subst)).collect();
                if new_args == args { return ty.clone(); }
                // Re-instantiate. The template lookup must succeed — we
                // wouldn't have a generic_origin recorded otherwise.
                let template = self.struct_generic_templates.get(&name)
                    .cloned()
                    .expect("generic_origin names a template not in struct_generic_templates");
                self.instantiate_struct_from_arg_tys(&name, &template, new_args)
            }
            Ty::Enum(id) => {
                let origin = self.enums[id.0 as usize].generic_origin.clone();
                let Some((name, args)) = origin else { return ty.clone(); };
                let new_args: Vec<Ty> = args.iter().map(|a| self.subst_ty_deep(a, subst)).collect();
                if new_args == args { return ty.clone(); }
                let template = self.enum_generic_templates.get(&name)
                    .cloned()
                    .expect("generic_origin names a template not in enum_generic_templates");
                self.instantiate_enum_from_arg_tys(&name, &template, new_args)
            }
            other => other.clone(),
        }
    }

    /// Slice 7GEN.5c: resolve an AST `Type` to a `Ty`, but substitute
    /// `Ty::Param(name)` to its bound concrete type via `subst` first.
    /// Used during struct-template instantiation.
    /// Slice 7GEN.5d: resolve `EnumName[arg1, arg2, ...]` to a concrete
    /// `Ty::Enum(id)`. Mirrors `resolve_generic_instantiation` for the
    /// enum side: synthesizes a concrete `EnumDef` per unique pair,
    /// substituting Param types in variant payload types.
    fn resolve_generic_enum_instantiation(
        &mut self,
        name: &str,
        args: &[Type],
        span: ByteSpan,
    ) -> Ty {
        let Some(template) = self.enum_generic_templates.get(name).cloned() else {
            self.err(
                "E0303",
                format!("unknown generic enum `{}`", name),
                span,
            );
            return Ty::Error;
        };
        if args.len() != template.generic_params.len() {
            self.err(
                "E0501",
                format!(
                    "enum `{}` takes {} type argument(s), got {}",
                    name, template.generic_params.len(), args.len()
                ),
                span,
            );
            return Ty::Error;
        }
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.resolve_type(a)).collect();
        if arg_tys.iter().any(|t| matches!(t, Ty::Error)) {
            return Ty::Error;
        }
        // Slice 7GEN.5e step 4: bound check (only at AST-originating call
        // sites — see the parallel comment in resolve_generic_instantiation).
        let param_names: Vec<String> = template.generic_params.iter()
            .map(|g| g.name.name.clone()).collect();
        let bounds: Vec<Vec<String>> = template.generic_params.iter()
            .map(|g| g.bounds.iter().map(|b| b.name.clone()).collect())
            .collect();
        self.check_generic_bounds(&param_names, &bounds, &arg_tys, span,
            &format!("generic enum `{}`", name));
        self.instantiate_enum_from_arg_tys(name, &template, arg_tys)
    }

    /// Slice 7GEN.5d (factored 2026-05-13): post-arg-resolution body of
    /// `resolve_generic_enum_instantiation`. See `instantiate_struct_from_arg_tys`
    /// for the rationale (substitution paths re-enter without AST args).
    fn instantiate_enum_from_arg_tys(
        &mut self,
        name: &str,
        template: &crate::ast::EnumDecl,
        arg_tys: Vec<Ty>,
    ) -> Ty {
        let key = (name.to_string(), arg_tys.clone());
        if let Some(&existing) = self.enum_instantiations.get(&key) {
            return Ty::Enum(existing);
        }
        // Build subst map and synthesize variant payloads.
        let subst: HashMap<String, Ty> = template
            .generic_params
            .iter()
            .map(|g| g.name.name.clone())
            .zip(arg_tys.iter().cloned())
            .collect();
        let mut variants: Vec<EnumVariantDef> = Vec::with_capacity(template.variants.len());
        for v in &template.variants {
            let payload: Vec<Ty> = v
                .payload
                .iter()
                .map(|t| self.resolve_field_type_with_subst(t, &subst))
                .collect();
            variants.push(EnumVariantDef {
                name: v.name.name.clone(),
                payload,
            });
        }
        let is_tagged = variants.iter().any(|v| !v.payload.is_empty());
        let mangled = mangle_generic_struct_name(name, &arg_tys, &self.structs, &self.enums);
        let id = EnumId(self.enums.len() as u32);
        self.enums.push(EnumDef {
            name: mangled.clone(),
            variants,
            is_copy: false,
            is_tagged,
            generic_base: Some(name.to_string()),
            generic_origin: Some((name.to_string(), arg_tys.clone())),
        });
        self.enum_by_name.insert(mangled, id);
        self.enum_instantiations.insert(key, id);
        Ty::Enum(id)
    }

    fn resolve_field_type_with_subst(
        &mut self,
        ty: &Type,
        subst: &HashMap<String, Ty>,
    ) -> Ty {
        match &ty.kind {
            TypeKind::Path(name) => {
                if let Some(concrete) = subst.get(name) {
                    return concrete.clone();
                }
                self.resolve_type(ty)
            }
            TypeKind::Array { elem, len } => {
                let elem_ty = self.resolve_field_type_with_subst(elem, subst);
                Ty::Array(Box::new(elem_ty), *len)
            }
            TypeKind::Borrowed { inner, .. } => self.resolve_field_type_with_subst(inner, subst),
            TypeKind::Generic { name, args } => {
                // A field type that itself names a generic — e.g.
                // `struct Pair[A, B] { left: Inner[A], ... }`. Substitute
                // inside the args, then recurse via the on-demand path.
                let substituted_args: Vec<Type> = args
                    .iter()
                    .map(|a| substitute_param_in_type_ast(a, subst))
                    .collect();
                let synthetic = Type {
                    kind: TypeKind::Generic { name: name.clone(), args: substituted_args },
                    span: ty.span,
                };
                self.resolve_type(&synthetic)
            }
            TypeKind::RawPtr(inner) => {
                let inner_ty = self.resolve_field_type_with_subst(inner, subst);
                Ty::RawPtr(Box::new(inner_ty))
            }
            TypeKind::FnPtr { params, return_type } => {
                let resolved_params: Vec<Ty> = params.iter()
                    .map(|p| self.resolve_field_type_with_subst(p, subst))
                    .collect();
                let resolved_ret = match return_type {
                    Some(rt) => self.resolve_field_type_with_subst(rt, subst),
                    None => Ty::Unit,
                };
                Ty::FnPtr { params: resolved_params, return_type: Box::new(resolved_ret) }
            }
            TypeKind::Slice(inner) => {
                let inner_ty = self.resolve_field_type_with_subst(inner, subst);
                Ty::Slice(Box::new(inner_ty))
            }
        }
    }

    /// Slice 7GEN.4: is `name` a generic-parameter name visible at the
    /// current point? Consults the entire stack (inner scopes shadow
    /// outer ones is irrelevant here — we just need to know whether the
    /// name is a type-parameter anywhere up the chain).
    fn type_param_in_scope(&self, name: &str) -> bool {
        self.type_params_stack.iter().any(|frame| frame.contains(name))
    }

    fn push_type_params(&mut self, params: &[GenericParam]) {
        let frame: std::collections::HashSet<String> =
            params.iter().map(|p| p.name.name.clone()).collect();
        self.type_params_stack.push(frame);
    }

    fn pop_type_params(&mut self) {
        self.type_params_stack.pop();
    }

    fn check_path(&mut self, segments: &[Ident], span: ByteSpan) -> Ty {
        // Phase 2A: paths are exactly two segments — `EnumName::Variant`.
        if segments.len() != 2 {
            self.err(
                "E0312",
                "Phase 2 paths must be `EnumName::Variant` (exactly two segments)".to_string(),
                span,
            );
            return Ty::Error;
        }
        let enum_seg = &segments[0];
        let variant_seg = &segments[1];
        let Some(&id) = self.enum_by_name.get(&enum_seg.name) else {
            self.err("E0303", format!("unknown type `{}`", enum_seg.name), enum_seg.span);
            return Ty::Error;
        };
        let def = &self.enums[id.0 as usize];
        if !def.variants.iter().any(|v| v.name == variant_seg.name) {
            self.err(
                "E0317",
                format!("enum `{}` has no variant `{}`", def.name, variant_seg.name),
                variant_seg.span,
            );
            return Ty::Error;
        }
        Ty::Enum(id)
    }

    fn resolve_value_ident(&mut self, name: &str, span: ByteSpan, expected: Option<Ty>) -> Ty {
        if let Some(info) = self.lookup_local(name) {
            let ty = info.ty.clone();
            let moved = info.moved;
            let assigned = info.assigned;
            if moved {
                self.err(
                    "E0335",
                    format!("use of moved value `{name}`"),
                    span,
                );
            } else if !assigned {
                self.err(
                    "E0345",
                    format!("use of possibly-unassigned binding `{name}`; assign it on every control-flow path before reading"),
                    span,
                );
            }
            return ty;
        }
        // Slice 11.FN_PTR: when expected is `Ty::FnPtr { .. }` and `name` is
        // a non-generic fn, coerce the named fn to a fn-pointer value. The
        // surrounding `check_expr` validates signature equality via the
        // generic expected-vs-actual E0302 path. Without an expected-FnPtr
        // hint, fall through to the historical E0312 ("function used as a
        // value") — keeps existing diagnostics for misuse cases.
        let want_fn_ptr = matches!(expected, Some(Ty::FnPtr { .. }));
        if let Some(sig) = self.fns.get(name).cloned() {
            if want_fn_ptr {
                let params: Vec<Ty> = sig.params.iter().map(|p| p.ty.clone()).collect();
                return Ty::FnPtr {
                    params,
                    return_type: Box::new(sig.return_type),
                };
            }
            self.err(
                "E0312",
                format!("function `{name}` used as a value; assign it to a `fn(...)`-typed binding to take its address"),
                span,
            );
            return Ty::Error;
        }
        if self.fns_generic.contains_key(name) {
            // Slice 11.FN_PTR design note §4: generic fns cannot be taken
            // as fn-pointer values without specifying type parameters.
            self.err(
                "E0821",
                format!("cannot take address of generic function `{name}` without specifying type parameters"),
                span,
            );
            return Ty::Error;
        }
        self.err("E0300", format!("undefined name `{name}`"), span);
        Ty::Error
    }

    fn lookup_local(&self, name: &str) -> Option<&LocalInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) { return Some(info); }
        }
        None
    }
}

fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::AddWrap => "+%",
        BinOp::SubWrap => "-%",
        BinOp::MulWrap => "*%",
        _ => "?",
    }
}

/// Slice 7GEN.5a: unify a possibly-Param-bearing signature type
/// `param_ty` against a concrete `arg_ty`, extending `subst` with
/// any newly-bound `Ty::Param("T") -> concrete` mappings. Returns
/// false on mismatch. The unifier handles:
/// - `Param(name)` against any concrete type → bind (or check
///   consistency if already bound).
/// - `Array(elem_a, len_a)` against `Array(elem_b, len_b)` → match
///   lengths and recurse on element types.
/// - Concrete-against-concrete → equality check.
fn unify_param_against_concrete(
    param_ty: &Ty,
    arg_ty: &Ty,
    subst: &mut HashMap<String, Ty>,
) -> bool {
    match (param_ty, arg_ty) {
        (Ty::Param(name), concrete) => {
            if let Some(prior) = subst.get(name) {
                prior == concrete
            } else {
                subst.insert(name.clone(), concrete.clone());
                true
            }
        }
        (Ty::Array(a_elem, a_len), Ty::Array(b_elem, b_len)) => {
            if a_len != b_len { return false; }
            unify_param_against_concrete(a_elem, b_elem, subst)
        }
        (a, b) => a == b,
    }
}

/// 7GEN.5c carry-forward: does this Ty contain an unbound generic param
/// anywhere in its structure — *transitively* through generic struct/enum
/// instantiations? Used at the sema→mono handoff to skip placeholder
/// instantiations (see comment near `struct_instantiations` snapshot in
/// `check`). The recursion through `generic_origin` is what catches nested
/// cases like `Pair[Box[T], i32]`: the outer args list only contains
/// `Ty::Struct(box_T_unbound_id)` (no top-level Param), but Box[T]'s
/// origin args do.
fn ty_contains_param(ty: &Ty, structs: &[StructDef], enums: &[EnumDef]) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::Array(elem, _) => ty_contains_param(elem, structs, enums),
        Ty::RawPtr(inner) => ty_contains_param(inner, structs, enums),
        Ty::FnPtr { params, return_type } => {
            params.iter().any(|p| ty_contains_param(p, structs, enums))
                || ty_contains_param(return_type, structs, enums)
        }
        Ty::Struct(id) => {
            if let Some((_, args)) = &structs[id.0 as usize].generic_origin {
                args.iter().any(|a| ty_contains_param(a, structs, enums))
            } else {
                false
            }
        }
        Ty::Enum(id) => {
            if let Some((_, args)) = &enums[id.0 as usize].generic_origin {
                args.iter().any(|a| ty_contains_param(a, structs, enums))
            } else {
                false
            }
        }
        _ => false,
    }
}

fn ty_display(ty: &Ty) -> String {
    match ty {
        Ty::Param(name) => name.clone(),
        Ty::Array(elem, n) => format!("[{}; {}]", ty_display(elem), n),
        Ty::RawPtr(inner) => format!("*{}", ty_display(inner)),
        Ty::FnPtr { params, return_type } => {
            let params_s = params.iter().map(ty_display).collect::<Vec<_>>().join(", ");
            if matches!(**return_type, Ty::Unit) {
                format!("fn({params_s})")
            } else {
                format!("fn({params_s}) -> {}", ty_display(return_type))
            }
        }
        other => other.name().to_string(),
    }
}

/// Slice 7GEN.5c: walk an AST `Type` replacing any `TypeKind::Path(name)`
/// where `name` is a key in `subst` with a `Path` of the substituted
/// type's source-level name. Used by `resolve_field_type_with_subst` to
/// rebuild the AST for nested generic args before re-resolving. The
/// rendered name uses a minimal cover of primitives + struct/enum names;
/// arrays/borrows in the substitution are not yet supported by this
/// path (no in-tree use case exercises them — extend when needed).
fn substitute_param_in_type_ast(ty: &Type, subst: &HashMap<String, Ty>) -> Type {
    let kind = match &ty.kind {
        TypeKind::Path(name) => {
            if let Some(concrete) = subst.get(name) {
                // Render concrete back to a Path. For now we only emit a
                // path name for primitives; structured types would need
                // a richer rendering. Generic types nesting another
                // generic-param-typed struct lands when motivated.
                TypeKind::Path(ty_to_source_name(concrete))
            } else {
                TypeKind::Path(name.clone())
            }
        }
        TypeKind::Array { elem, len } => TypeKind::Array {
            elem: Box::new(substitute_param_in_type_ast(elem, subst)),
            len: *len,
        },
        TypeKind::Borrowed { region, inner } => TypeKind::Borrowed {
            region: region.clone(),
            inner: Box::new(substitute_param_in_type_ast(inner, subst)),
        },
        TypeKind::Generic { name, args } => TypeKind::Generic {
            name: name.clone(),
            args: args.iter().map(|a| substitute_param_in_type_ast(a, subst)).collect(),
        },
        TypeKind::RawPtr(inner) => TypeKind::RawPtr(Box::new(substitute_param_in_type_ast(inner, subst))),
        TypeKind::FnPtr { params, return_type } => TypeKind::FnPtr {
            params: params.iter().map(|p| substitute_param_in_type_ast(p, subst)).collect(),
            return_type: return_type.as_ref().map(|rt| Box::new(substitute_param_in_type_ast(rt, subst))),
        },
        TypeKind::Slice(inner) => TypeKind::Slice(Box::new(substitute_param_in_type_ast(inner, subst))),
    };
    Type { kind, span: ty.span }
}

/// Slice 7GEN.5c: render a `Ty` to a source-level name string suitable
/// for embedding in an AST `TypeKind::Path`. Conservative — only handles
/// primitive + struct/enum cases that field substitution needs.
fn ty_to_source_name(ty: &Ty) -> String {
    match ty {
        Ty::I8 => "i8".into(), Ty::I16 => "i16".into(), Ty::I32 => "i32".into(), Ty::I64 => "i64".into(),
        Ty::U8 => "u8".into(), Ty::U16 => "u16".into(), Ty::U32 => "u32".into(), Ty::U64 => "u64".into(),
        Ty::Isize => "isize".into(), Ty::Usize => "usize".into(),
        Ty::F32 => "f32".into(), Ty::F64 => "f64".into(),
        Ty::Bool => "bool".into(), Ty::Unit => "()".into(),
        Ty::Str => "str".into(),
        Ty::String => "string".into(),
        Ty::Slice(_) => "<slice>".into(),
        Ty::RawPtr(_) => "<raw-ptr>".into(),
        Ty::FnPtr { .. } => "<fn-ptr>".into(),
        // For struct / enum, return a synthetic placeholder. This path
        // is only exercised during template substitution; the resolved
        // Ty is what sema actually uses, not the rendered name.
        Ty::Struct(_) | Ty::Enum(_) => "<concrete>".into(),
        Ty::Array(_, _) => "<array>".into(),
        Ty::Param(name) => name.clone(),
        Ty::Error => "<error>".into(),
    }
}

/// Slice 7GEN.5c: mangle a generic struct instantiation's name —
/// `Pair[i32, bool]` → `Pair__i32__bool`. Matches the fn-instantiation
/// mangling convention (`name__T1__T2`).
fn mangle_generic_struct_name(
    name: &str,
    args: &[Ty],
    structs: &[StructDef],
    enums: &[EnumDef],
) -> String {
    let mut s = name.to_string();
    for arg in args {
        s.push_str("__");
        s.push_str(&mangle_ty_for_name(arg, structs, enums));
    }
    s
}

fn mangle_ty_for_name(ty: &Ty, structs: &[StructDef], enums: &[EnumDef]) -> String {
    match ty {
        Ty::I8 => "i8".into(), Ty::I16 => "i16".into(), Ty::I32 => "i32".into(), Ty::I64 => "i64".into(),
        Ty::U8 => "u8".into(), Ty::U16 => "u16".into(), Ty::U32 => "u32".into(), Ty::U64 => "u64".into(),
        Ty::Isize => "isize".into(), Ty::Usize => "usize".into(),
        Ty::F32 => "f32".into(), Ty::F64 => "f64".into(),
        Ty::Bool => "bool".into(), Ty::Unit => "unit".into(),
        Ty::Str => "str".into(),
        Ty::String => "string".into(),
        Ty::Slice(inner) => format!("slice_{}", mangle_ty_for_name(inner, structs, enums)),
        Ty::RawPtr(inner) => format!("ptr_{}", mangle_ty_for_name(inner, structs, enums)),
        Ty::FnPtr { params, return_type } => {
            let mut s = String::from("fn");
            for p in params {
                s.push('_');
                s.push_str(&mangle_ty_for_name(p, structs, enums));
            }
            if !matches!(**return_type, Ty::Unit) {
                s.push_str("_ret_");
                s.push_str(&mangle_ty_for_name(return_type, structs, enums));
            }
            s
        }
        Ty::Struct(id) => structs[id.0 as usize].name.clone(),
        Ty::Enum(id) => enums[id.0 as usize].name.clone(),
        Ty::Array(elem, n) => format!("arr{}_{}", n, mangle_ty_for_name(elem, structs, enums)),
        Ty::Param(n) => format!("Param_{n}"),
        Ty::Error => "ERR".into(),
    }
}

/// Slice 7GEN.4: walk a `Ty` replacing every occurrence of
/// `Ty::Param("Self")` with the concrete `target` type. Used during
/// interface-impl signature comparison so the interface's abstract
/// `Self`-typed slots line up against the impl's concrete types.
/// Recurses into `Array`'s element type. Other `Ty::Param` names
/// (proper generics like `T`) are left alone — they're already
/// concrete-relative-to-the-impl since impls don't introduce fresh
/// type params in this slice's surface.
fn subst_self(ty: &Ty, target: &Ty) -> Ty {
    match ty {
        Ty::Param(name) if name == "Self" => target.clone(),
        Ty::Array(elem, len) => Ty::Array(Box::new(subst_self(elem, target)), *len),
        other => other.clone(),
    }
}

/// Slice 7GEN.4: compare an interface method's signature against an
/// impl method's signature after substituting `Self -> target`. The
/// comparison is strict — receiver kind, parameter count, parameter
/// markers (`mut` / `move`), parameter types, and return type all must
/// match.
fn method_sig_matches(iface: &MethodSig, impl_: &MethodSig, target: &Ty) -> bool {
    if iface.receiver != impl_.receiver { return false; }
    if iface.params.len() != impl_.params.len() { return false; }
    for (a, b) in iface.params.iter().zip(impl_.params.iter()) {
        if a.mutable != b.mutable { return false; }
        if a.move_ != b.move_ { return false; }
        if subst_self(&a.ty, target) != b.ty { return false; }
    }
    subst_self(&iface.return_type, target) == impl_.return_type
}

fn cast_allowed(from: &Ty, to: &Ty) -> bool {
    if from == to { return true; }
    // numeric → numeric (any pair)
    if from.is_numeric() && to.is_numeric() { return true; }
    // bool → integer (zext to width)
    if *from == Ty::Bool && to.is_int() { return true; }
    // enum → integer (read the variant index)
    if from.is_enum() && to.is_int() { return true; }
    // Phase 11 / P3 (FFI null + integer-to-pointer): integer → raw pointer.
    // The cast itself just reinterprets bits as an address (LLVM `inttoptr`).
    // The unsafe gate lives in `check_cast` — `cast_allowed` answers only
    // the type-pair shape question.
    if from.is_int() && matches!(to, Ty::RawPtr(_)) { return true; }
    // Phase 11: raw-pointer → raw-pointer reinterpretation (`*u8 as *T`).
    // The standard C / Rust idiom for treating an allocator-returned byte
    // buffer as a typed pointer. Codegen is a no-op at the LLVM level
    // (every raw pointer lowers to `ptr` already). The unsafe gate in
    // `check_cast` covers the soundness side — caller asserts the
    // reinterpretation is valid.
    if matches!(from, Ty::RawPtr(_)) && matches!(to, Ty::RawPtr(_)) { return true; }
    // Forbidden:
    //   - integer/float → bool (use `!= 0`)
    //   - bool → float
    //   - integer → enum (needs runtime range check)
    //   - any other combination
    false
}

/// Phase 11 / ObjC interop: find `#[link_name = "..."]` on an item's
/// attribute list and return the string value. Returns `None` if absent.
/// Attribute-shape validation has already run via attrs::check, so any
/// `link_name` here is guaranteed to have the right arg shape.
fn extract_link_name(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| {
        if a.path.name != "link_name" { return None; }
        match a.args.as_slice() {
            [AttrArg::Str(s, _)] => Some(s.clone()),
            _ => None,
        }
    })
}

fn body_ends_with_return(b: &Block) -> bool {
    b.stmts.last().is_some_and(|s| matches!(s.kind, StmtKind::Return(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use std::path::PathBuf;

    fn check_src(src: &str) -> Vec<Diagnostic> {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        check(&prog, PathBuf::from("test.cplus"), src)
    }

    fn errors(src: &str) -> Vec<&'static str> {
        check_src(src)
            .into_iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| {
                // We need a 'static str for assertions; leak the small string.
                Box::leak(d.code.0.to_string().into_boxed_str()) as &str
            })
            .collect()
    }

    fn assert_clean(src: &str) {
        let diags = check_src(src);
        assert!(
            diags.is_empty(),
            "expected clean type-check, got: {:#?}",
            diags
        );
    }

    fn assert_only_code(src: &str, code: &str) {
        let diags = check_src(src);
        assert_eq!(
            diags.len(),
            1,
            "expected exactly one diagnostic ({code}), got: {:#?}",
            diags
        );
        assert_eq!(diags[0].code.0, code);
    }

    // ---- happy paths: every Phase-1 sample type-checks ----

    #[test]
    fn factorial_clean() {
        assert_clean(include_str!("../../docs/examples/factorial.cplus"));
    }

    #[test]
    fn fibonacci_clean() {
        assert_clean(include_str!("../../docs/examples/fibonacci.cplus"));
    }

    #[test]
    fn sum_range_clean() {
        assert_clean(include_str!("../../docs/examples/sum_range.cplus"));
    }

    #[test]
    fn c_for_clean() {
        assert_clean(include_str!("../../docs/examples/c_for.cplus"));
    }

    #[test]
    fn return_with_value_clean() {
        assert_clean("fn main() -> i32 { return 42; }");
    }

    #[test]
    fn nested_if_expr_clean() {
        assert_clean("fn main() -> i32 { return if true { 1 } else if false { 2 } else { 3 }; }");
    }

    // ---- design-note §7.2 negative cases ----

    #[test]
    fn assign_to_immutable_e0305() {
        assert_only_code("fn main() -> i32 { let x = 1; x = 2; return 0; }", "E0305");
    }

    #[test]
    fn float_literal_in_i32_slot_is_type_mismatch() {
        // Phase 2: floats are supported, so `let x: i32 = 1.5` is a type
        // mismatch (f64 vs i32), not a "feature unsupported" error.
        assert_only_code("fn main() -> i32 { let x: i32 = 1.5; return 0; }", "E0302");
    }

    #[test]
    fn trailing_semi_discards_value_e0306() {
        assert_only_code("fn f() -> i32 { 1; }\nfn main() -> i32 { return f(); }", "E0306");
    }

    #[test]
    fn nonbool_condition_e0304() {
        assert_only_code("fn main() -> i32 { return if 1 { 1 } else { 2 }; }", "E0304");
    }

    #[test]
    fn u64_literal_now_supported() {
        // Phase 2: all integer suffixes supported.
        assert_clean("fn main() -> i32 { let x: u64 = 1u64; let y: u64 = x; let _z = y; return 0; }");
    }

    #[test]
    fn main_must_return_i32_e0309() {
        let codes = errors("fn main() { }");
        assert!(codes.contains(&"E0309"), "expected E0309 in {codes:?}");
    }

    #[test]
    fn return_without_value_e0307() {
        assert_only_code("fn f() -> i32 { return; }\nfn main() -> i32 { return f(); }", "E0307");
    }

    // ---- additional rules ----

    #[test]
    fn undefined_name_e0300() {
        assert_only_code("fn main() -> i32 { return x; }", "E0300");
    }

    #[test]
    fn undefined_function_e0300() {
        assert_only_code("fn main() -> i32 { return foo(1); }", "E0300");
    }

    #[test]
    fn duplicate_fn_e0301() {
        let src = "fn f() -> i32 { 0 }\nfn f() -> i32 { 1 }\nfn main() -> i32 { return f(); }";
        let codes = errors(src);
        assert!(codes.contains(&"E0301"));
    }

    #[test]
    fn type_mismatch_e0302() {
        assert_only_code("fn main() -> i32 { let x: i32 = true; return 0; }", "E0302");
    }

    #[test]
    fn unknown_type_e0303() {
        assert_only_code("fn main() -> Foo { return 0; }", "E0303");
    }

    #[test]
    fn arg_count_mismatch_e0308() {
        // Wrap in a stmt + 0 tail so we don't also trigger E0302 from main's
        // i32 return type vs println's Unit return.
        assert_only_code("fn main() -> i32 { println(1, 2); return 0; }", "E0308");
    }

    #[test]
    fn arg_type_mismatch_e0302() {
        assert_only_code("fn main() -> i32 { println(true); return 0; }", "E0302");
    }

    #[test]
    fn float_literal_now_supported() {
        assert_clean("fn main() -> i32 { let x: f64 = 3.14; let _y: f64 = x; return 0; }");
    }

    #[test]
    fn bitwise_ops_now_supported() {
        // Phase 3A: bitwise + shift on every integer width.
        assert_clean("fn main() -> i32 { return (1 & 2) | (4 ^ 8); }");
        assert_clean("fn main() -> i32 { return 1 << 2 >> 1; }");
        assert_clean("fn main() -> i32 { return ~0; }");
    }

    #[test]
    fn bitwise_on_float_e0302() {
        let codes = errors("fn main() -> i32 { let x: f64 = 1.0; let y: f64 = x & 1.0; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302 on float &, got: {codes:?}");
    }

    #[test]
    fn bitwise_on_bool_e0302() {
        let codes = errors("fn main() -> i32 { let b: bool = true | false; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302 on bool |, got: {codes:?}");
    }

    #[test]
    fn bit_not_on_float_e0302() {
        let codes = errors("fn main() -> i32 { let x: f64 = 1.0; let y: f64 = ~x; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302 on ~f64, got: {codes:?}");
    }

    #[test]
    fn shift_count_can_be_different_int_width() {
        // C lets `i64 << u8`; same here. Shift count is just an integer.
        assert_clean(
            "fn main() -> i32 {\n\
               let x: i64 = 1 as i64;\n\
               let n: u8 = 3 as u8;\n\
               let y: i64 = x << n;\n\
               return 0;\n\
             }"
        );
    }

    #[test]
    fn shift_count_must_be_integer_e0302() {
        let codes = errors(
            "fn main() -> i32 { let x: i64 = 1 as i64; let y: i64 = x << 1.0; return 0; }"
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn wrapping_ops_now_supported() {
        assert_clean("fn main() -> i32 { return (1 +% 2) -% 1 *% 1; }");
    }

    #[test]
    fn wrapping_op_on_float_e0302() {
        let codes = errors(
            "fn main() -> i32 { let x: f64 = 1.0; let y: f64 = x +% 2.0; return 0; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn wrapping_op_on_bool_e0302() {
        let codes = errors(
            "fn main() -> i32 { let _b: bool = true +% false; return 0; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn cast_now_supported() {
        assert_clean("fn main() -> i32 { return 1 as i32; }");
    }

    #[test]
    fn ref_not_supported_e0312() {
        assert_only_code("fn main() -> i32 { let x = 1; let y = &x; return 0; }", "E0312");
    }

    #[test]
    fn compound_assign_supported_clean() {
        // v0.0.3 Slice 3A: `+=` `-=` `*=` `/=` `%=` `&=` `|=` `^=` `<<=` `>>=`
        // all type-check cleanly on appropriate types.
        assert_clean("fn main() -> i32 { let mut x = 1; x += 2; x -= 1; x *= 3; return x; }");
        assert_clean("fn main() -> i32 { let mut b: u32 = 0xff as u32; b &= 0x0f as u32; b |= 0x10 as u32; b ^= 0x01 as u32; b <<= 1 as u32; b >>= 2 as u32; return b as i32; }");
    }

    #[test]
    fn compound_bitwise_assign_on_float_e0302() {
        // Bitwise compound assigns require integer types.
        assert_only_code(
            "fn main() -> i32 { let mut x: f32 = 1.0 as f32; x &= 2.0 as f32; return 0; }",
            "E0302",
        );
    }

    #[test]
    fn assign_to_non_ident_e0313() {
        // Phase 1 has no field/index access yet, so we hit a parse error first
        // for most non-ident targets. Use a literal as a stand-in: parser
        // accepts `1 = 2` as Assign{IntLit, IntLit}.
        let codes = errors("fn main() -> i32 { 1 = 2; return 0; }");
        assert!(codes.contains(&"E0313"));
    }

    #[test]
    fn shadowing_in_inner_scope_clean() {
        assert_clean("fn main() -> i32 { let x = 1; { let x = 2; }; return x; }");
    }

    #[test]
    fn block_value_in_let_clean() {
        assert_clean("fn main() -> i32 { let x = { let y = 5; y + 1 }; return x; }");
    }

    #[test]
    fn while_loop_clean() {
        assert_clean("fn main() -> i32 { let mut i = 0; while i < 10 { i = i + 1; } return i; }");
    }

    #[test]
    fn comparison_returns_bool_clean() {
        assert_clean("fn main() -> i32 { let b: bool = 1 < 2; return if b { 1 } else { 0 }; }");
    }

    #[test]
    fn equality_on_bool_clean() {
        assert_clean("fn main() -> i32 { let b: bool = true == false; return if b { 1 } else { 0 }; }");
    }

    // ---- Phase 2 slice 1: full primitive types + casts ----

    #[test]
    fn all_integer_types_resolve() {
        for t in ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "isize", "usize"] {
            let src = format!("fn main() -> i32 {{ let x: {t} = 0; let _y: {t} = x; return 0; }}");
            assert_clean(&src);
        }
    }

    #[test]
    fn float_types_resolve() {
        assert_clean("fn main() -> i32 { let x: f32 = 1.0; let _y: f32 = x; return 0; }");
        assert_clean("fn main() -> i32 { let x: f64 = 1.0; let _y: f64 = x; return 0; }");
    }

    #[test]
    fn integer_literal_infers_from_expected_type() {
        // Unsuffixed `42` becomes u64 because the let annotation says so.
        assert_clean("fn main() -> i32 { let x: u64 = 42; let _y: u64 = x; return 0; }");
    }

    #[test]
    fn float_literal_infers_from_expected_type() {
        assert_clean("fn main() -> i32 { let x: f32 = 1.5; let _y: f32 = x; return 0; }");
    }

    #[test]
    fn mixed_int_arithmetic_rejected() {
        let codes = errors("fn main() -> i32 { let x: i32 = 1i32 + 1u32; return x; }");
        assert!(codes.contains(&"E0302"), "expected mixed-type error, got: {codes:?}");
    }

    #[test]
    fn float_arithmetic_clean() {
        assert_clean("fn main() -> i32 { let x: f64 = 1.0 + 2.0 * 3.0; let _y: f64 = x; return 0; }");
    }

    #[test]
    fn float_modulo_rejected_e0316() {
        assert_only_code("fn main() -> i32 { let x: f64 = 1.0 % 2.0; let _y: f64 = x; return 0; }", "E0316");
    }

    #[test]
    fn negate_unsigned_rejected() {
        let codes = errors("fn main() -> i32 { let x: u32 = 5; let _y: u32 = -x; return 0; }");
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn negate_float_clean() {
        assert_clean("fn main() -> i32 { let x: f64 = 5.0; let _y: f64 = -x; return 0; }");
    }

    // Casts

    #[test]
    fn cast_int_to_int_widen_clean() {
        assert_clean("fn main() -> i32 { let a: i8 = 5; let _b: i32 = a as i32; return 0; }");
    }

    #[test]
    fn cast_int_to_int_narrow_clean() {
        assert_clean("fn main() -> i32 { let a: i64 = 5; let _b: i8 = a as i8; return 0; }");
    }

    #[test]
    fn cast_int_to_float_clean() {
        assert_clean("fn main() -> i32 { let a: u32 = 5; let _b: f64 = a as f64; return 0; }");
    }

    #[test]
    fn cast_float_to_int_clean() {
        assert_clean("fn main() -> i32 { let a: f64 = 3.7; let _b: i32 = a as i32; return 0; }");
    }

    #[test]
    fn cast_bool_to_int_clean() {
        assert_clean("fn main() -> i32 { let _b: i32 = true as i32; return 0; }");
    }

    #[test]
    fn cast_int_to_bool_rejected_e0315() {
        assert_only_code("fn main() -> i32 { let _b: bool = 1 as bool; return 0; }", "E0315");
    }

    #[test]
    fn cast_float_to_bool_rejected_e0315() {
        assert_only_code("fn main() -> i32 { let _b: bool = 1.0 as bool; return 0; }", "E0315");
    }

    #[test]
    fn cast_bool_to_float_rejected_e0315() {
        assert_only_code("fn main() -> i32 { let _b: f64 = true as f64; return 0; }", "E0315");
    }

    #[test]
    fn comparison_works_on_all_numeric_types() {
        assert_clean("fn main() -> i32 { return if 1u64 < 2u64 { 1 } else { 0 }; }");
        assert_clean("fn main() -> i32 { return if 1.0 < 2.0 { 1 } else { 0 }; }");
        assert_clean("fn main() -> i32 { let a: i8 = 1; let b: i8 = 2; return if a < b { 1 } else { 0 }; }");
    }

    // ---- Phase 2 slice 2A: plain enums + paths ----

    #[test]
    fn enum_decl_clean() {
        assert_clean("enum Color { Red, Green, Blue }\nfn main() -> i32 { return 0; }");
    }

    #[test]
    fn enum_variant_path_clean() {
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let _c: Color = Color::Red; return 0; }"
        );
    }

    #[test]
    fn enum_variant_in_comparison_clean() {
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { let c: Color = Color::Red; return if c == Color::Green { 1 } else { 0 }; }"
        );
    }

    #[test]
    fn enum_argument_and_return_clean() {
        assert_clean(include_str!("../../docs/examples/direction.cplus"));
    }

    #[test]
    fn duplicate_enum_variant_e0318() {
        let codes = errors("enum E { A, A }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0318"));
    }

    #[test]
    fn unknown_enum_variant_e0317() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = Color::Purple; return 0; }"
        );
        assert!(codes.contains(&"E0317"));
    }

    #[test]
    fn unknown_enum_in_path_e0303() {
        // `Foo` not declared anywhere.
        let codes = errors("fn main() -> i32 { let _x: i32 = Foo::Bar as i32; return 0; }");
        assert!(codes.contains(&"E0303"), "expected E0303 in {codes:?}");
    }

    #[test]
    fn ordering_on_enum_rejected_e0302() {
        let codes = errors(
            "enum E { A, B }\nfn main() -> i32 { if E::A < E::B { 1 } else { 0 } }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn enum_to_int_cast_clean() {
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { return Color::Green as i32; }"
        );
    }

    #[test]
    fn int_to_enum_cast_rejected_e0315() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = 0 as Color; return 0; }"
        );
        assert!(codes.contains(&"E0315"));
    }

    #[test]
    fn assigning_int_to_enum_rejected_e0302() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = 0; return 0; }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn assigning_enum_to_int_rejected_e0302() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _x: i32 = Color::Red; return 0; }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn cross_enum_comparison_rejected_e0302() {
        let codes = errors(
            "enum A { X }\nenum B { Y }\n\
             fn main() -> i32 { if A::X == B::Y { 1 } else { 0 } }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn duplicate_enum_name_e0301() {
        let codes = errors("enum E { A }\nenum E { B }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0301"));
    }

    // ---- Phase 2 slice 2B: structs (no methods) ----

    #[test]
    fn struct_decl_clean() {
        assert_clean("struct Point { x: i32, y: i32 }\nfn main() -> i32 { return 0; }");
    }

    #[test]
    fn struct_literal_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let _p: Point = Point { x: 1, y: 2 }; return 0; }"
        );
    }

    #[test]
    fn empty_struct_clean() {
        assert_clean(
            "struct Empty {}\n\
             fn main() -> i32 { let _e: Empty = Empty {}; return 0; }"
        );
    }

    #[test]
    fn struct_field_read_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let p: Point = Point { x: 1, y: 2 }; let _v: i32 = p.x; return 0; }"
        );
    }

    #[test]
    fn struct_field_write_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn main() -> i32 { let mut p: Point = Point { x: 1, y: 2 }; p.x = 10; return 0; }"
        );
    }

    #[test]
    fn struct_passed_by_value_clean() {
        assert_clean(include_str!("../../docs/examples/point.cplus"));
    }

    #[test]
    fn nested_struct_clean() {
        assert_clean(include_str!("../../docs/examples/nested.cplus"));
    }

    #[test]
    fn mutable_struct_loop_clean() {
        assert_clean(include_str!("../../docs/examples/mutable_struct.cplus"));
    }

    #[test]
    fn duplicate_field_e0319() {
        let codes = errors("struct E { x: i32, x: i32 }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0319"));
    }

    #[test]
    fn unknown_field_in_access_e0320() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let a: A = A { x: 1 }; let _v: i32 = a.y; return 0; }"
        );
        assert!(codes.contains(&"E0320"));
    }

    #[test]
    fn missing_field_in_literal_e0321() {
        let codes = errors(
            "struct A { x: i32, y: i32 }\n\
             fn main() -> i32 { let _a: A = A { x: 1 }; return 0; }"
        );
        assert!(codes.contains(&"E0321"));
    }

    #[test]
    fn extra_field_in_literal_e0322() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let _a: A = A { x: 1, y: 2 }; return 0; }"
        );
        assert!(codes.contains(&"E0322"));
    }

    #[test]
    fn field_access_on_non_struct_e0323() {
        let codes = errors(
            "fn main() -> i32 { let x: i32 = 5; let _v: i32 = x.foo; return 0; }"
        );
        assert!(codes.contains(&"E0323"));
    }

    #[test]
    fn field_assign_on_immutable_e0305() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let a: A = A { x: 1 }; a.x = 2; return 0; }"
        );
        assert!(codes.contains(&"E0305"));
    }

    #[test]
    fn assign_to_temporary_struct_e0313() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { A { x: 1 }.x = 2; return 0; }"
        );
        assert!(codes.contains(&"E0313"));
    }

    #[test]
    fn duplicate_struct_name_e0301() {
        let codes = errors("struct P { x: i32 }\nstruct P { y: i32 }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0301"));
    }

    #[test]
    fn enum_struct_name_collision_e0301() {
        let codes = errors("enum X { A }\nstruct X { x: i32 }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0301"));
    }

    #[test]
    fn struct_eq_rejected_e0302() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let a: A = A { x: 1 }; let b: A = A { x: 1 }; if a == b { 1 } else { 0 } }"
        );
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn nested_field_write_on_mutable_root_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             struct Line  { from: Point, to: Point }\n\
             fn main() -> i32 { let mut l: Line = Line { from: Point { x: 0, y: 0 }, to: Point { x: 0, y: 0 } }; l.to.x = 5; return 0; }"
        );
    }

    #[test]
    fn forward_ref_struct_field_clean() {
        // Struct B references A which is declared later.
        assert_clean(
            "struct B { a: A }\nstruct A { x: i32 }\nfn main() -> i32 { return 0; }"
        );
    }

    // ---- Phase 2 slice 2C: methods + impl blocks ----

    #[test]
    fn empty_impl_block_clean() {
        assert_clean("struct P {}\nimpl P {}\nfn main() -> i32 { return 0; }");
    }

    #[test]
    fn associated_function_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn new(x: i32) -> P { return P { x: x }; } }\n\
             fn main() -> i32 { let _p: P = P::new(5); return 0; }"
        );
    }

    #[test]
    fn ref_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.get(); }"
        );
    }

    #[test]
    fn ref_mut_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn set(mut self, v: i32) { self.x = v; } }\n\
             fn main() -> i32 { let mut p: P = P { x: 0 }; p.set(5); return p.x; }"
        );
    }

    #[test]
    fn value_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn into_x(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.into_x(); }"
        );
    }

    #[test]
    fn methods_sample_clean() {
        assert_clean(include_str!("../../docs/examples/methods.cplus"));
    }

    #[test]
    fn impl_on_unknown_type_e0325() {
        let codes = errors("impl Foo { fn f(self) {} }\nfn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0325"));
    }

    #[test]
    fn impl_on_enum_e0325() {
        let codes = errors(
            "enum E { A }\nimpl E { fn f(self) {} }\nfn main() -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0325"));
    }

    #[test]
    fn duplicate_method_e0326() {
        let codes = errors(
            "struct P {}\nimpl P { fn f(self) {} fn f(self) {} }\nfn main() -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0326"));
    }

    #[test]
    fn no_such_method_e0324() {
        let codes = errors(
            "struct P {}\nimpl P {}\nfn main() -> i32 { let p: P = P {}; return p.missing(); }"
        );
        assert!(codes.contains(&"E0324"));
    }

    #[test]
    fn calling_assoc_fn_as_method_e0327() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn make() -> P { return P { x: 0 }; } }\n\
             fn main() -> i32 { let p: P = P { x: 0 }; let _q: P = p.make(); return 0; }"
        );
        assert!(codes.contains(&"E0327"));
    }

    #[test]
    fn calling_method_via_type_e0327() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { return P::get(); }"
        );
        assert!(codes.contains(&"E0327"));
    }

    #[test]
    fn calling_mut_method_on_immutable_e0328() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn bump(mut self) { self.x = self.x + 1; } }\n\
             fn main() -> i32 { let p: P = P { x: 0 }; p.bump(); return 0; }"
        );
        assert!(codes.contains(&"E0328"));
    }

    #[test]
    fn self_in_function_body_e0300() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn bad() -> i32 { return self.x; } }\n\
             fn main() -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0300"));
    }

    #[test]
    fn method_via_field_chain_clean() {
        assert_clean(
            "struct Inner { v: i32 }\n\
             struct Outer { inner: Inner }\n\
             impl Inner { fn get(self) -> i32 { return self.v; } }\n\
             fn main() -> i32 { let o: Outer = Outer { inner: Inner { v: 42 } }; return o.inner.get(); }"
        );
    }

    #[test]
    fn enum_variant_not_callable_e0327() {
        let codes = errors(
            "enum E { A }\n\
             fn main() -> i32 { return E::A(); }"
        );
        assert!(codes.contains(&"E0327"));
    }

    // ---- Phase 2 slice 2D: fixed-size arrays ----

    #[test]
    fn array_decl_and_literal_clean() {
        assert_clean(
            "fn main() -> i32 { let _xs: [i32; 3] = [1, 2, 3]; return 0; }"
        );
    }

    #[test]
    fn array_indexing_clean() {
        assert_clean(
            "fn main() -> i32 { let xs: [i32; 3] = [10, 20, 30]; return xs[0 as usize]; }"
        );
    }

    #[test]
    fn array_indexed_assign_clean() {
        assert_clean(
            "fn main() -> i32 { let mut xs: [i32; 3] = [0, 0, 0]; xs[1 as usize] = 5; return xs[1 as usize]; }"
        );
    }

    #[test]
    fn array_as_struct_field_clean() {
        assert_clean(include_str!("../../docs/examples/array_struct.cplus"));
    }

    #[test]
    fn array_sum_sample_clean() {
        assert_clean(include_str!("../../docs/examples/array_sum.cplus"));
    }

    #[test]
    fn array_literal_length_mismatch_e0330() {
        let codes = errors("fn main() -> i32 { let _xs: [i32; 3] = [1, 2]; return 0; }");
        assert!(codes.contains(&"E0330"), "expected E0330, got: {codes:?}");
    }

    #[test]
    fn array_literal_mixed_types_e0329() {
        let codes = errors("fn main() -> i32 { let _xs: [i32; 2] = [1, true]; return 0; }");
        assert!(codes.contains(&"E0329"));
    }

    #[test]
    fn indexing_non_array_e0331() {
        let codes = errors("fn main() -> i32 { let x: i32 = 5; return x[0 as usize]; }");
        assert!(codes.contains(&"E0331"));
    }

    #[test]
    fn empty_array_literal_e0332() {
        let codes = errors("fn main() -> i32 { let _xs: [i32; 0] = []; return 0; }");
        assert!(codes.contains(&"E0332"));
    }

    #[test]
    fn array_field_indexed_write_on_immutable_e0305() {
        let codes = errors(
            "struct C { xs: [i32; 2] }\n\
             fn main() -> i32 { let c: C = C { xs: [0, 0] }; c.xs[0 as usize] = 5; return 0; }"
        );
        assert!(codes.contains(&"E0305"));
    }

    #[test]
    fn array_in_function_signature_clean() {
        assert_clean(
            "fn first(xs: [i32; 3]) -> i32 { return xs[0 as usize]; }\n\
             fn main() -> i32 { return first([10, 20, 30]); }"
        );
    }

    #[test]
    fn diagnostic_includes_correct_span() {
        let diags = check_src("fn main() -> i32 { return foo(); }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.0, "E0300");
        // span must point at `foo`, which starts at byte offset 26
        assert_eq!(diags[0].primary.start.byte, 26);
    }

    // ----- Phase 3 slice 3A: ownership markers on params -----

    #[test]
    fn mut_and_move_on_param_e0334() {
        let codes = errors(
            "fn f(mut move x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return f(1); }",
        );
        assert!(codes.contains(&"E0334"), "expected E0334, got: {codes:?}");
    }

    #[test]
    fn move_and_mut_on_param_e0334() {
        let codes = errors(
            "fn f(move mut x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return f(1); }",
        );
        assert!(codes.contains(&"E0334"), "expected E0334, got: {codes:?}");
    }

    #[test]
    fn mut_param_makes_binding_mutable() {
        // `mut x: T` should allow writing `x = ...` inside the body without
        // E0305 (assignment to immutable binding).
        assert_clean(
            "fn inc(mut x: i32) -> i32 { x = x + 1; return x; }\n\
             fn main() -> i32 { return inc(1); }",
        );
    }

    #[test]
    fn plain_param_remains_immutable_e0305() {
        let codes = errors(
            "fn bad(x: i32) -> i32 { x = x + 1; return x; }\n\
             fn main() -> i32 { return bad(1); }",
        );
        assert!(codes.contains(&"E0305"), "expected E0305, got: {codes:?}");
    }

    #[test]
    fn move_param_parses_clean() {
        // `move x: T` is accepted; full move tracking is deferred to a later
        // slice of Phase 3, so this should currently behave like a plain param.
        assert_clean(
            "fn consume(move x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return consume(7); }",
        );
    }

    #[test]
    fn move_self_method_parses_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn take(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 4 }; return p.take(); }",
        );
    }

    #[test]
    fn mut_and_move_on_method_param_e0334() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn f(self, mut move y: i32) -> i32 { return self.x + y; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; return p.f(2); }",
        );
        assert!(codes.contains(&"E0334"), "expected E0334, got: {codes:?}");
    }

    // ----- Phase 3 slice 3A: move tracking + E0335 -----
    //
    // Revived in slice 3F: each test's `struct P { x: i32 }` now also has
    // an empty `impl P { fn drop(mut self) {} }` block. The presence of a
    // destructor makes P non-Copy (Drop overrides Copy auto-derive), which
    // makes the `move` consumption real and re-fires E0335 / E0337.

    #[test]
    fn move_param_consumes_non_copy_binding_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn drop(mut self) {} }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = take(p); return p.x; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    fn move_param_double_call_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn drop(mut self) {} }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = take(p); let r: i32 = take(p); return 0; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    fn move_self_consumes_receiver_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn drop(mut self) {} fn into_x(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = p.into_x(); return p.x; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    fn move_self_double_call_e0335() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn drop(mut self) {} fn into_x(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = p.into_x(); let r: i32 = p.into_x(); return 0; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    fn move_on_copy_param_does_not_consume() {
        // `move x: i32` is redundant — `i32` is Copy, so the source remains
        // usable. (A future E0336 lint will suggest removing the keyword.)
        assert_clean(
            "fn take(move x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { let x: i32 = 5; let r: i32 = take(x); return x; }",
        );
    }

    #[test]
    fn shared_borrow_does_not_consume() {
        // `p: P` (no `move`) is a shared borrow at the design level; in
        // Phase 3 it doesn't track borrows yet, but the source must remain
        // usable across calls.
        assert_clean(
            "struct P { x: i32 }\n\
             fn read(p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; let a: i32 = read(p); let b: i32 = read(p); return a + b; }",
        );
    }

    #[test]
    fn move_from_field_e0337() {
        // Partial moves out of struct fields are deferred. With Inner marked
        // Drop (and therefore non-Copy), passing `o.i` through a `move`
        // parameter must be rejected.
        let codes = errors(
            "struct Inner { x: i32 }\n\
             impl Inner { fn drop(mut self) {} }\n\
             struct Outer { i: Inner }\n\
             fn take(move i: Inner) -> i32 { return i.x; }\n\
             fn main() -> i32 { let o: Outer = Outer { i: Inner { x: 1 } }; return take(o.i); }",
        );
        assert!(codes.contains(&"E0337"), "expected E0337, got: {codes:?}");
    }

    #[test]
    fn move_chain_through_function_is_clean() {
        // Building owned values, threading them through one consuming call,
        // and producing an owned result: nothing should remain usable, but
        // also nothing should error.
        assert_clean(
            "struct P { x: i32 }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 42 }; return take(p); }",
        );
    }

    #[test]
    fn move_then_assign_recovers_binding() {
        // Sanity check the boundary: once moved, the binding stays moved.
        // (A re-`let` would shadow it, but the same `p` cannot be revived.)
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn drop(mut self) {} }\n\
             fn take(move p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: P = P { x: 1 }; let r: i32 = take(p); let q: i32 = p.x; return q; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    // ----- Phase 3 slice 3C: Copy auto-derive -----

    #[test]
    fn copy_struct_remains_usable_after_pass() {
        // `Point { x: i32, y: i32 }` is Copy under auto-derive. Passing by
        // value (default shared) does not consume; the source stays usable.
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn read(p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: Point = Point { x: 3, y: 4 }; let a: i32 = read(p); let b: i32 = p.y; return a + b; }",
        );
    }

    #[test]
    fn copy_struct_with_array_field_is_copy() {
        // Array of Copy → Copy. Struct containing array of Copy → Copy.
        assert_clean(
            "struct C { xs: [i32; 3] }\n\
             fn first(c: C) -> i32 { return c.xs[0 as usize]; }\n\
             fn main() -> i32 { let c: C = C { xs: [1, 2, 3] }; let a: i32 = first(c); return a + c.xs[1 as usize]; }",
        );
    }

    #[test]
    fn nested_copy_struct_is_copy() {
        assert_clean(
            "struct Inner { x: i32 }\n\
             struct Outer { i: Inner, k: i32 }\n\
             fn read(o: Outer) -> i32 { return o.i.x + o.k; }\n\
             fn main() -> i32 { let o: Outer = Outer { i: Inner { x: 1 }, k: 2 }; let _a: i32 = read(o); return o.i.x; }",
        );
    }

    #[test]
    fn copy_struct_move_marker_is_silent_noop() {
        // `move p: Point` on a Copy struct: redundant marker, source still
        // usable. Same shape as the existing `move_on_copy_param_does_not_consume`
        // test for `i32` — now extended to aggregates under auto-derive.
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             fn take(move p: Point) -> i32 { return p.x; }\n\
             fn main() -> i32 { let p: Point = Point { x: 1, y: 2 }; let a: i32 = take(p); return a + p.y; }",
        );
    }

    #[test]
    fn copy_struct_move_self_is_silent_noop() {
        // `move self` on a Copy receiver: ditto.
        assert_clean(
            "struct Point { x: i32, y: i32 }\n\
             impl Point { fn into_x(move self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: Point = Point { x: 1, y: 2 }; let a: i32 = p.into_x(); return a + p.y; }",
        );
    }

    // ----- Phase 3 slice 3F: Drop -----

    #[test]
    fn drop_method_makes_struct_non_copy() {
        // A struct with only Copy fields would normally auto-derive Copy,
        // but the destructor flips it to non-Copy. With this, sema must
        // reject moves from fields (E0337) and the "shared borrow" form
        // does not consume — the design note's mechanism for expressing
        // a non-Copy aggregate.
        let codes = errors(
            "struct B { x: i32 }\n\
             impl B { fn drop(mut self) {} }\n\
             struct C { b: B }\n\
             fn take(move b: B) -> i32 { return b.x; }\n\
             fn main() -> i32 { let c: C = C { b: B { x: 1 } }; return take(c.b); }",
        );
        assert!(codes.contains(&"E0337"), "expected E0337, got: {codes:?}");
    }

    #[test]
    fn drop_wrong_receiver_e0338() {
        let codes = errors(
            "struct B { x: i32 }\n\
             impl B { fn drop(self) {} }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0338"), "expected E0338, got: {codes:?}");
    }

    #[test]
    fn drop_extra_param_e0338() {
        let codes = errors(
            "struct B { x: i32 }\n\
             impl B { fn drop(mut self, k: i32) {} }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0338"), "expected E0338, got: {codes:?}");
    }

    #[test]
    fn drop_with_return_e0338() {
        let codes = errors(
            "struct B { x: i32 }\n\
             impl B { fn drop(mut self) -> i32 { return 0; } }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0338"), "expected E0338, got: {codes:?}");
    }

    #[test]
    fn drop_empty_body_is_clean() {
        // The user-accessible "make this aggregate non-Copy" idiom: an
        // empty destructor. Must compile cleanly.
        assert_clean(
            "struct B { x: i32 }\n\
             impl B { fn drop(mut self) {} }\n\
             fn main() -> i32 { let b: B = B { x: 1 }; return b.x; }",
        );
    }

    // ----- Phase 3 slice 3G: defer -----

    #[test]
    fn defer_stmt_is_clean() {
        assert_clean("fn main() -> i32 { defer println(1); return 0; }");
    }

    #[test]
    fn defer_with_type_error_e0302() {
        // The deferred expression is type-checked; passing the wrong type
        // to println surfaces the regular type-error.
        let codes = errors(
            "fn main() -> i32 { defer println(true); return 0; }",
        );
        // println takes i32; bool argument is a mismatch.
        assert!(
            codes.contains(&"E0302") || codes.contains(&"E0308"),
            "expected type-error on defer body, got: {codes:?}"
        );
    }

    #[test]
    fn defer_in_inner_block_clean() {
        assert_clean(
            "fn main() -> i32 { if 1 == 1 { defer println(42); } return 0; }",
        );
    }

    // ----- Phase 3 slice 3I: tagged unions + match -----

    #[test]
    fn tagged_enum_construction_clean() {
        assert_clean(
            "enum M { A(i32), B }\n\
             fn main() -> i32 { let m: M = M::A(7); return match m { M::A(v) => v, M::B => 0 }; }",
        );
    }

    #[test]
    fn match_non_exhaustive_e0340() {
        let codes = errors(
            "enum M { A, B, C }\n\
             fn main() -> i32 { let m: M = M::A; return match m { M::A => 0 }; }",
        );
        assert!(codes.contains(&"E0340"), "expected E0340, got: {codes:?}");
    }

    #[test]
    fn match_wildcard_makes_exhaustive() {
        assert_clean(
            "enum M { A, B, C }\n\
             fn main() -> i32 { let m: M = M::A; return match m { M::A => 1, _ => 0 }; }",
        );
    }

    #[test]
    fn match_binding_makes_exhaustive() {
        assert_clean(
            "enum M { A, B }\n\
             fn main() -> i32 { let m: M = M::A; return match m { _x => 0 }; }",
        );
    }

    #[test]
    fn match_wrong_payload_arity_e0342() {
        let codes = errors(
            "enum M { A(i32, i32) }\n\
             fn main() -> i32 { let m: M = M::A(1, 2); return match m { M::A(v) => v }; }",
        );
        assert!(codes.contains(&"E0342"), "expected E0342, got: {codes:?}");
    }

    #[test]
    fn variant_call_arity_e0342() {
        let codes = errors(
            "enum M { A(i32, i32) }\n\
             fn main() -> i32 { let m: M = M::A(1); return 0; }",
        );
        assert!(codes.contains(&"E0342"), "expected E0342, got: {codes:?}");
    }

    #[test]
    fn match_on_non_enum_e0341() {
        let codes = errors(
            "fn main() -> i32 { let x: i32 = 5; return match x { _ => 0 }; }",
        );
        assert!(codes.contains(&"E0341"), "expected E0341, got: {codes:?}");
    }

    #[test]
    fn match_arm_type_mismatch_e0302() {
        let codes = errors(
            "enum M { A, B }\n\
             fn main() -> i32 { let m: M = M::A; let r: i32 = match m { M::A => 0, M::B => true }; return r; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn tagged_enum_with_drop_payload_e0344() {
        let codes = errors(
            "struct R { x: i32 }\n\
             impl R { fn drop(mut self) {} }\n\
             enum E { Hold(R), Empty }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0344"), "expected E0344, got: {codes:?}");
    }

    #[test]
    fn variant_path_no_paren_for_payloadless_clean() {
        // Payload-less variant constructed via bare path.
        assert_clean(
            "enum M { A(i32), B }\n\
             fn main() -> i32 { let m: M = M::B; return match m { M::A(v) => v, M::B => 0 }; }",
        );
    }

    #[test]
    fn variant_call_payloadless_e0327() {
        // Payload-less variant called with parens — point user at bare path.
        let codes = errors(
            "enum M { A }\n\
             fn main() -> i32 { let m: M = M::A(); return 0; }",
        );
        assert!(codes.contains(&"E0327"), "expected E0327, got: {codes:?}");
    }

    // ----- Phase 3 slice 3J: definite assignment -----

    #[test]
    fn uninit_let_then_assign_then_read_clean() {
        assert_clean(
            "fn main() -> i32 { let x: i32; x = 5; return x; }",
        );
    }

    #[test]
    fn uninit_let_read_before_assign_e0345() {
        let codes = errors(
            "fn main() -> i32 { let x: i32; return x; }",
        );
        assert!(codes.contains(&"E0345"), "expected E0345, got: {codes:?}");
    }

    #[test]
    fn uninit_let_no_type_e0346() {
        let codes = errors(
            "fn main() -> i32 { let x; x = 5; return x; }",
        );
        assert!(codes.contains(&"E0346"), "expected E0346, got: {codes:?}");
    }

    #[test]
    fn both_branches_assign_clean() {
        // Flow merge: both arms of the if assign x, so x is definitely
        // assigned after the if.
        assert_clean(
            "fn main() -> i32 { let x: i32; if 1 == 1 { x = 1; } else { x = 2; } return x; }",
        );
    }

    #[test]
    fn one_branch_assigns_e0345() {
        // Only the then-branch assigns. Flow merge says "maybe assigned"
        // — reading x after the if must error.
        let codes = errors(
            "fn main() -> i32 { let x: i32; if 1 == 1 { x = 1; } return x; }",
        );
        assert!(codes.contains(&"E0345"), "expected E0345, got: {codes:?}");
    }

    #[test]
    fn match_all_arms_assign_clean() {
        assert_clean(
            "enum M { A, B }\n\
             fn main() -> i32 {\n\
               let m: M = M::A;\n\
               let x: i32;\n\
               match m { M::A => { x = 1; }, M::B => { x = 2; } }\n\
               return x;\n\
             }",
        );
    }

    #[test]
    fn first_write_to_immutable_uninit_clean() {
        // The first write to an unassigned binding doesn't need `mut`.
        // A second write would. (This test only verifies the first write.)
        assert_clean(
            "fn main() -> i32 { let x: i32; x = 5; return x; }",
        );
    }

    #[test]
    fn second_write_to_unmut_after_init_e0305() {
        // After the first write initializes the immutable binding, further
        // writes need `mut`. This test confirms the second write is
        // rejected with the same E0305 rule that governs assignment to
        // already-initialized immutable bindings.
        let codes = errors(
            "fn main() -> i32 { let x: i32; x = 5; x = 6; return x; }",
        );
        assert!(codes.contains(&"E0305"), "expected E0305, got: {codes:?}");
    }

    // ---- Phase 5 slice 5ATTR.2: sema-level `#[test]` validation ----

    #[test]
    fn test_fn_with_no_return_clean() {
        // `fn()` is one of the two accepted shapes.
        assert_clean(
            "#[test] fn t() { return; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn test_fn_with_i32_return_clean() {
        // `fn() -> i32` is the other accepted shape.
        assert_clean(
            "#[test] fn t() -> i32 { return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn test_fn_with_param_rejected_e0358() {
        let codes = errors(
            "#[test] fn t(n: i32) { return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0358"), "expected E0358, got: {codes:?}");
    }

    #[test]
    fn test_fn_with_wrong_return_type_rejected_e0358() {
        let codes = errors(
            "#[test] fn t() -> bool { return true; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0358"), "expected E0358, got: {codes:?}");
    }

    #[test]
    fn test_fn_pub_rejected_e0359() {
        let codes = errors(
            "#[test] pub fn t() { return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0359"), "expected E0359, got: {codes:?}");
    }

    #[test]
    fn test_fn_pub_and_wrong_signature_emits_both() {
        // Independent rules — both fire on the same fn.
        let codes = errors(
            "#[test] pub fn t(n: i32) -> bool { return true; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0358"), "expected E0358, got: {codes:?}");
        assert!(codes.contains(&"E0359"), "expected E0359, got: {codes:?}");
    }

    #[test]
    fn non_test_fn_with_pub_is_clean() {
        // Sanity guard: `pub` rejection is gated on the `#[test]` attribute.
        // A regular `pub fn` is fine.
        assert_clean(
            "pub fn helper() { return; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn non_test_fn_with_param_is_clean() {
        // Same gate: signature constraints only apply to `#[test]` fns.
        assert_clean(
            "fn helper(n: i32) -> i32 { return n; }\n\
             fn main() -> i32 { return helper(5); }",
        );
    }

    // ---- Phase 5 slice 5ATTR.3: `assert EXPR;` sema ----

    #[test]
    fn assert_with_bool_condition_clean() {
        assert_clean("fn main() -> i32 { assert 1 == 1; return 0; }");
    }

    #[test]
    fn assert_with_non_bool_condition_e0302() {
        let codes = errors("fn main() -> i32 { assert 42; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn assert_with_float_condition_e0302() {
        let codes = errors("fn main() -> i32 { assert 1.5; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    // ---- Slice 7GEN.4: generics + interface validation ----

    #[test]
    fn generic_fn_param_in_scope_compiles_clean() {
        assert_clean("fn identity[T](x: T) -> T { return x; } fn main() -> i32 { return 0; }");
    }

    #[test]
    fn generic_fn_multi_param_clean() {
        assert_clean(
            "fn pick[A, B](a: A, b: B) -> A { return a; } fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn generic_struct_field_uses_param_clean() {
        assert_clean(
            "struct Pair[A, B] { first: A, second: B } fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn generic_enum_payload_uses_param_clean() {
        assert_clean(
            "enum Maybe[T] { Some(T), None } fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn unknown_type_param_still_e0303() {
        let codes = errors("fn id[T](x: T) -> U { return x; } fn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0303"), "expected E0303 for unknown U, got: {codes:?}");
    }

    #[test]
    fn type_param_does_not_leak_outside_generic_fn() {
        // T is declared on identity but not on consumer; consumer sees T as unknown.
        let codes = errors(
            "fn identity[T](x: T) -> T { return x; } \
             fn consumer(x: T) -> T { return x; } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0303"), "expected E0303, got: {codes:?}");
    }

    #[test]
    fn self_outside_impl_or_interface_e0508() {
        let codes = errors("fn loose(x: Self) -> i32 { return 0; } fn main() -> i32 { return 0; }");
        assert!(codes.contains(&"E0508"), "expected E0508, got: {codes:?}");
    }

    #[test]
    fn self_inside_inherent_impl_resolves_to_target() {
        // The method's `other: Self` param resolves to Point; calling with a
        // Point argument compiles. Without Self resolution this would fire E0303.
        assert_clean(
            "struct Point { x: i32, y: i32 } \
             impl Point { fn sum(self, other: Self) -> i32 { return self.x + other.x; } } \
             fn main() -> i32 { let a: Point = Point { x: 1, y: 2 }; \
                                let b: Point = Point { x: 3, y: 4 }; \
                                return a.sum(b); }",
        );
    }

    #[test]
    fn interface_decl_method_signature_clean() {
        // Slice 7GEN.6 renamed: `Ord` is now compiler-blessed, so use
        // a distinct name in this generic-interface-decl test.
        assert_clean(
            "interface Compare { fn compare(self, other: i32) -> i32; } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn impl_interface_for_struct_matching_signature_clean() {
        assert_clean(
            "interface Compare { fn compare(self, other: i32) -> i32; } \
             struct Point { x: i32, y: i32 } \
             impl Compare for Point { fn compare(self, other: i32) -> i32 { return 0; } } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn impl_interface_missing_method_e0503() {
        let codes = errors(
            "interface Two { fn first(self) -> i32; fn second(self) -> i32; } \
             struct P { x: i32 } \
             impl Two for P { fn first(self) -> i32 { return 0; } } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0503"), "expected E0503, got: {codes:?}");
    }

    #[test]
    fn impl_interface_extra_method_e0504() {
        let codes = errors(
            "interface One { fn a(self) -> i32; } \
             struct P { x: i32 } \
             impl One for P { fn a(self) -> i32 { return 0; } \
                              fn extra(self) -> i32 { return 1; } } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0504"), "expected E0504, got: {codes:?}");
    }

    #[test]
    fn impl_interface_signature_mismatch_e0505() {
        // Interface declares `fn a(self) -> i32`, impl provides `fn a(self) -> bool`.
        let codes = errors(
            "interface One { fn a(self) -> i32; } \
             struct P { x: i32 } \
             impl One for P { fn a(self) -> bool { return true; } } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0505"), "expected E0505, got: {codes:?}");
    }

    #[test]
    fn impl_interface_duplicate_e0506() {
        let codes = errors(
            "interface One { fn a(self) -> i32; } \
             struct P { x: i32 } \
             impl One for P { fn a(self) -> i32 { return 0; } } \
             impl One for P { fn a(self) -> i32 { return 1; } } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0506"), "expected E0506, got: {codes:?}");
    }

    #[test]
    fn interface_method_with_self_substitutes_in_impl() {
        // Interface says `fn dup(self) -> Self`; impl must return Point.
        assert_clean(
            "interface Dup { fn dup(self) -> Self; } \
             struct Point { x: i32, y: i32 } \
             impl Dup for Point { fn dup(self) -> Point { return Point { x: self.x, y: self.y }; } } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn interface_method_with_self_impl_returns_wrong_type_e0505() {
        // Interface says `fn dup(self) -> Self`; impl returns i32 instead of Point.
        let codes = errors(
            "interface Dup { fn dup(self) -> Self; } \
             struct Point { x: i32, y: i32 } \
             impl Dup for Point { fn dup(self) -> i32 { return 0; } } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0505"), "expected E0505, got: {codes:?}");
    }

    #[test]
    fn duplicate_interface_name_e0301() {
        let codes = errors(
            "interface A { fn a(self) -> i32; } \
             interface A { fn b(self) -> i32; } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0301"), "expected E0301, got: {codes:?}");
    }

    // ---- Slice 7GEN.5b: turbofish syntax ----

    #[test]
    fn turbofish_at_call_site_compiles_clean() {
        assert_clean(
            "fn identity[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = identity::[i32](7); return a; }",
        );
    }

    #[test]
    fn turbofish_wrong_arity_e0501() {
        let codes = errors(
            "fn id[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = id::[i32, bool](7); return a; }",
        );
        assert!(codes.contains(&"E0501"), "expected E0501 for arity mismatch, got: {codes:?}");
    }

    #[test]
    fn turbofish_on_non_generic_fn_e0501() {
        let codes = errors(
            "fn plain(x: i32) -> i32 { return x; } \
             fn main() -> i32 { return plain::[i32](7); }",
        );
        assert!(codes.contains(&"E0501"), "expected E0501 on non-generic fn turbofish, got: {codes:?}");
    }

    #[test]
    fn turbofish_arg_type_validated_against_substituted_param() {
        // identity[i32] expects i32; passing bool fires E0302.
        let codes = errors(
            "fn identity[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = identity::[i32](true); return a; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302 for arg/type-arg mismatch, got: {codes:?}");
    }

    // ---- Slice 7GEN.5c: generic-struct instantiation ----

    #[test]
    fn generic_struct_used_at_type_position_clean() {
        assert_clean(
            "struct Pair[A, B] { first: A, second: B } \
             fn use_pair(p: Pair[i32, bool]) -> i32 { return p.first; } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn generic_struct_literal_clean() {
        assert_clean(
            "struct Pair[A, B] { first: A, second: B } \
             fn main() -> i32 { \
                 let p: Pair[i32, i32] = Pair[i32, i32] { first: 7, second: 35 }; \
                 return p.first + p.second; \
             }",
        );
    }

    #[test]
    fn generic_struct_distinct_instantiations_share_template() {
        assert_clean(
            "struct Pair[A, B] { first: A, second: B } \
             fn f(p: Pair[i32, i32]) -> i32 { return p.first; } \
             fn g(p: Pair[bool, i32]) -> i32 { return p.second; } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn generic_struct_wrong_arity_e0501() {
        let codes = errors(
            "struct Pair[A, B] { first: A, second: B } \
             fn main() -> i32 { \
                 let p: Pair[i32] = Pair[i32] { first: 7 }; \
                 return 0; \
             }",
        );
        assert!(codes.contains(&"E0501"), "expected E0501, got: {codes:?}");
    }

    #[test]
    fn generic_struct_unknown_template_e0303() {
        let codes = errors(
            "fn main() -> i32 { \
                 let p: Bogus[i32, i32] = Bogus[i32, i32] { first: 7, second: 35 }; \
                 return 0; \
             }",
        );
        assert!(codes.contains(&"E0303"), "expected E0303, got: {codes:?}");
    }

    // ---- Slice 7GEN.5d: generic-enum instantiation ----

    #[test]
    fn generic_enum_used_at_type_position_clean() {
        assert_clean(
            "enum Option[T] { Some(T), None } \
             fn use_o(o: Option[i32]) -> i32 { return 0; } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn generic_enum_constructor_clean() {
        assert_clean(
            "enum Option[T] { Some(T), None } \
             fn main() -> i32 { \
                 let a: Option[i32] = Option[i32]::Some(7); \
                 let b: Option[i32] = Option[i32]::None; \
                 return 0; \
             }",
        );
    }

    #[test]
    fn generic_enum_wrong_arity_e0501() {
        let codes = errors(
            "enum Option[T] { Some(T), None } \
             fn main() -> i32 { \
                 let a: Option[i32, bool] = Option[i32, bool]::None; \
                 return 0; \
             }",
        );
        assert!(codes.contains(&"E0501"), "expected E0501, got: {codes:?}");
    }

    #[test]
    fn generic_enum_unknown_template_e0303() {
        let codes = errors(
            "fn main() -> i32 { let a: Bogus[i32] = Bogus[i32]::Some(7); return 0; }",
        );
        assert!(codes.contains(&"E0303"), "expected E0303, got: {codes:?}");
    }

    #[test]
    fn impl_unknown_interface_e0303() {
        let codes = errors(
            "struct P { x: i32 } \
             impl Bogus for P { fn a(self) -> i32 { return 0; } } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0303"), "expected E0303, got: {codes:?}");
    }

    // ---- Phase 8 slice 8.STR.1–.3: strings ----

    // ---- Phase 10 slice 10.FFI.1: extern fn + raw pointers ----

    #[test]
    fn extern_fn_declaration_clean() {
        // Slice 10.FFI.3: extern call must be in `unsafe { ... }`.
        assert_clean(
            "extern fn abs(x: i32) -> i32; \
             fn main() -> i32 { return unsafe { abs(0 -% 42) }; }",
        );
    }

    #[test]
    fn extern_fn_call_with_wrong_arg_type_e0302() {
        let codes = errors(
            "extern fn abs(x: i32) -> i32; \
             fn main() -> i32 { return unsafe { abs(true) }; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn extern_fn_with_raw_pointer_param_clean() {
        // Parser + sema accept `*u8` in extern fn signatures.
        assert_clean(
            "extern fn strlen(s: *u8) -> usize; \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn raw_pointer_type_in_let_clean() {
        // Type position works outside extern fn too. No way to
        // construct a *u8 from nothing yet (10.FFI.2 wires that),
        // so this test only checks the type parses + resolves.
        // Slice 10.FFI.3: extern calls require `unsafe`.
        assert_clean(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { let p: *u8 = unsafe { malloc(8 as usize) }; return 0; }",
        );
    }

    #[test]
    fn raw_pointer_nested_clean() {
        // `**i32` parses and resolves.
        assert_clean(
            "extern fn dummy(p: **i32) -> i32; \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn pointer_deref_returns_pointee_clean() {
        // Slice 10.FFI.2a: `*p` where p: *u8 has type u8.
        // Slice 10.FFI.3: deref + extern call wrapped in `unsafe`.
        assert_clean(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                 return unsafe { \
                     let p: *u8 = malloc(1 as usize); \
                     let b: u8 = *p; \
                     b as i32 \
                 }; \
             }",
        );
    }

    #[test]
    fn pointer_deref_non_pointer_e0302() {
        let codes = errors(
            "fn main() -> i32 { let x: i32 = 7; return *x; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn pointer_deref_outside_unsafe_e0801() {
        // Slice 10.FFI.3: deref outside `unsafe` fires E0801.
        let codes = errors(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                 let p: *u8 = unsafe { malloc(1 as usize) }; \
                 let b: u8 = *p; \
                 return b as i32; \
             }",
        );
        assert!(codes.contains(&"E0801"), "expected E0801, got: {codes:?}");
    }

    #[test]
    fn variadic_extern_fn_accepts_extra_args_clean() {
        // Slice 10.FFI.4: variadic fn admits arbitrary tail args.
        assert_clean(
            "extern fn printf(fmt: *u8, ...) -> i32; \
             fn main() -> i32 { \
                 return unsafe { printf(str_ptr(\"hi %d\\n\"), 7) }; \
             }",
        );
    }

    #[test]
    fn variadic_extern_fn_too_few_fixed_args_e0308() {
        let codes = errors(
            "extern fn printf(fmt: *u8, ...) -> i32; \
             fn main() -> i32 { return unsafe { printf() }; }",
        );
        assert!(codes.contains(&"E0308"), "expected E0308, got: {codes:?}");
    }

    #[test]
    fn extern_call_outside_unsafe_e0801() {
        let codes = errors(
            "extern fn abs(x: i32) -> i32; \
             fn main() -> i32 { return abs(0 -% 7); }",
        );
        assert!(codes.contains(&"E0801"), "expected E0801, got: {codes:?}");
    }

    #[test]
    fn pointer_store_through_deref_clean() {
        // Slice 10.FFI.2b + 10.FFI.3: `*p = v` inside unsafe.
        assert_clean(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                 unsafe { \
                     let p: *u8 = malloc(1 as usize); \
                     *p = 42 as u8; \
                 } \
                 return 0; \
             }",
        );
    }

    #[test]
    fn pointer_indexing_returns_pointee_clean() {
        // Slice 10.FFI.2c + 10.FFI.3: `p[i]` inside unsafe.
        assert_clean(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                 return unsafe { \
                     let p: *u8 = malloc(4 as usize); \
                     p[0] = 1 as u8; \
                     let b: u8 = p[0]; \
                     b as i32 \
                 }; \
             }",
        );
    }

    #[test]
    fn pointer_arithmetic_add_returns_pointer_clean() {
        // Slice 10.FFI.2d + 10.FFI.3: pointer arithmetic + deref inside unsafe.
        assert_clean(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                 return unsafe { \
                     let p: *u8 = malloc(4 as usize); \
                     let q: *u8 = p + 1 as usize; \
                     *q = 7 as u8; \
                     let b: u8 = *q; \
                     b as i32 \
                 }; \
             }",
        );
    }

    #[test]
    fn raw_pointer_is_copy_clean() {
        // Pointers are Copy — passing through a fn doesn't move them.
        // Slice 10.FFI.3: extern calls live in an unsafe block.
        assert_clean(
            "extern fn use_ptr(p: *u8) -> i32; \
             extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                 return unsafe { \
                     let p: *u8 = malloc(8 as usize); \
                     let a: i32 = use_ptr(p); \
                     let b: i32 = use_ptr(p); \
                     a + b \
                 }; \
             }",
        );
    }

    #[test]
    fn str_literal_has_str_type_clean() {
        assert_clean(
            "fn main() -> i32 { let s: str = \"hello\"; return 0; }",
        );
    }

    #[test]
    fn str_literal_typed_inferred_clean() {
        // No type annotation; literal's natural type is `str`.
        assert_clean(
            "fn main() -> i32 { let s = \"hello\"; return 0; }",
        );
    }

    #[test]
    fn println_accepts_str_clean() {
        // 8.STR.2: println overload accepts str.
        assert_clean(
            "fn main() -> i32 { println(\"hi\"); return 0; }",
        );
    }

    #[test]
    fn println_rejects_non_int_non_str_arg_e0302() {
        // Phase 8 narrowed println: bool, structs, etc. all rejected.
        let codes = errors(
            "fn main() -> i32 { println(true); return 0; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn str_equality_returns_bool_clean() {
        // 8.STR.3: `==` and `!=` on str values type-check to bool.
        assert_clean(
            "fn main() -> i32 { \
                if \"a\" == \"a\" { return 0; } \
                if \"a\" != \"b\" { return 0; } \
                return 1; \
            }",
        );
    }

    #[test]
    fn str_type_annotation_in_fn_params_clean() {
        assert_clean(
            "fn take(s: str) -> i32 { return 0; } \
             fn main() -> i32 { return take(\"hi\"); }",
        );
    }

    // ---- Phase 7 slice 7GEN.5e: source-level generic-enum patterns ----

    #[test]
    fn generic_enum_pattern_with_explicit_type_args_clean() {
        // The headline of 7GEN.5e: `Option[i32]::Some(v)` in pattern
        // position. No mangled name in source.
        assert_clean(
            "enum Option[T] { Some(T), None } \
             fn unwrap_or(o: Option[i32], default: i32) -> i32 { \
                 return match o { \
                     Option[i32]::Some(v) => v, \
                     Option[i32]::None => default, \
                 }; \
             } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn generic_enum_pattern_unqualified_clean() {
        // Type-directed: `Option::Some(v)` with no type args resolves
        // against an `Option[i32]` scrutinee via the EnumDef's
        // `generic_base` field.
        assert_clean(
            "enum Option[T] { Some(T), None } \
             fn unwrap_or(o: Option[i32], default: i32) -> i32 { \
                 return match o { \
                     Option::Some(v) => v, \
                     Option::None => default, \
                 }; \
             } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn generic_enum_pattern_wrong_type_arg_e0341() {
        // `Option[bool]::Some` against an `Option[i32]` scrutinee
        // resolves to a different EnumId and is rejected.
        let codes = errors(
            "enum Option[T] { Some(T), None } \
             fn pick(o: Option[i32]) -> i32 { \
                 return match o { \
                     Option[bool]::Some(_) => 1, \
                     Option[bool]::None => 0, \
                 }; \
             } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0341"), "expected E0341, got: {codes:?}");
    }

    // ---- Phase 7 slice 7GEN.5e step 4 + 7GEN.6: bounds + blessed interfaces ----

    #[test]
    fn blessed_ord_user_can_impl_clean() {
        // The blessed Ord interface is in scope; users can impl it.
        assert_clean(
            "struct Point { x: i32 } \
             impl Ord for Point { fn cmp(self, other: Point) -> i32 { return 0; } } \
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn blessed_ord_redeclaration_e0301() {
        // User cannot redeclare `Ord` — it's a blessed interface.
        let codes = errors(
            "interface Ord { fn cmp(self, other: i32) -> i32; } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0301"), "expected E0301, got: {codes:?}");
    }

    #[test]
    fn manual_copy_impl_rejected_e0510() {
        let codes = errors(
            "struct Point { x: i32 } \
             impl Copy for Point {} \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0510"), "expected E0510, got: {codes:?}");
    }

    #[test]
    fn bound_violation_at_generic_fn_call_e0502() {
        // `fn max[T: Ord]` called with a type that doesn't impl Ord.
        let codes = errors(
            "fn max[T: Ord](a: T, b: T) -> T { return a; } \
             struct Point { x: i32 } \
             fn main() -> i32 { \
                 let p: Point = Point { x: 0 }; \
                 let r: Point = max(p, p); \
                 return 0; \
             }",
        );
        assert!(codes.contains(&"E0502"), "expected E0502, got: {codes:?}");
    }

    #[test]
    fn bound_satisfied_at_generic_fn_call_clean() {
        // Same fn but Point has `impl Ord`.
        assert_clean(
            "fn max[T: Ord](a: T, b: T) -> T { return a; } \
             struct Point { x: i32 } \
             impl Ord for Point { fn cmp(self, other: Point) -> i32 { return 0; } } \
             fn main() -> i32 { \
                 let p: Point = Point { x: 0 }; \
                 let r: Point = max(p, p); \
                 return 0; \
             }",
        );
    }

    #[test]
    fn copy_bound_satisfied_structurally_clean() {
        // `T: Copy` is satisfied for i32 (a primitive Copy type) without
        // any user-written impl.
        assert_clean(
            "fn pick[T: Copy](a: T, b: T) -> T { return a; } \
             fn main() -> i32 { return pick(7, 35); }",
        );
    }

    #[test]
    fn copy_bound_violated_by_drop_struct_e0502() {
        // A struct with a `drop` method is non-Copy and fails T: Copy.
        let codes = errors(
            "struct Resource { x: i32 } \
             impl Resource { fn drop(mut self) {} } \
             fn pick[T: Copy](a: T, b: T) -> T { return a; } \
             fn main() -> i32 { \
                 let r: Resource = Resource { x: 0 }; \
                 let r2: Resource = pick(r, r); \
                 return 0; \
             }",
        );
        assert!(codes.contains(&"E0502"), "expected E0502, got: {codes:?}");
    }

    #[test]
    fn bound_violation_at_generic_struct_e0502() {
        let codes = errors(
            "struct Wrapper[T: Ord] { v: T } \
             struct Point { x: i32 } \
             fn main() -> i32 { \
                 let w: Wrapper[Point] = Wrapper[Point] { v: Point { x: 0 } }; \
                 return 0; \
             }",
        );
        assert!(codes.contains(&"E0502"), "expected E0502, got: {codes:?}");
    }

    #[test]
    fn generic_typed_impl_get_clean() {
        // Slice 7GEN.5e step 3: `impl Box[T] { fn get(self) -> T }`.
        assert_clean(
            "struct Box[T] { value: T } \
             impl Box[T] { fn get(self) -> T { return self.value; } } \
             fn main() -> i32 { \
                 let b: Box[i32] = Box[i32] { value: 42 }; \
                 return b.get(); \
             }",
        );
    }

    #[test]
    fn generic_typed_impl_two_params_clean() {
        // `impl Pair[A, B]` — multiple impl-level params.
        assert_clean(
            "struct Pair[A, B] { first: A, second: B } \
             impl Pair[A, B] { \
                 fn first(self) -> A { return self.first; } \
                 fn second(self) -> B { return self.second; } \
             } \
             fn main() -> i32 { \
                 let p: Pair[i32, bool] = Pair[i32, bool] { first: 42, second: true }; \
                 return p.first(); \
             }",
        );
    }

    #[test]
    fn generic_typed_impl_method_uses_param_clean() {
        // Method takes T as a param.
        assert_clean(
            "struct Box[T] { value: T } \
             impl Box[T] { fn replace(mut self, new_value: T) { self.value = new_value; } } \
             fn main() -> i32 { \
                 let mut b: Box[i32] = Box[i32] { value: 0 }; \
                 b.replace(42); \
                 return b.value; \
             }",
        );
    }

    #[test]
    fn generic_method_with_turbofish_clean() {
        // Slice 7GEN.5e: `p.cast::[i32](42)` on a generic method.
        assert_clean(
            "struct P { x: i32 } \
             impl P { fn cast[T](self, value: T) -> T { return value; } } \
             fn main() -> i32 { \
                 let p: P = P { x: 0 }; \
                 return p.cast::[i32](42); \
             }",
        );
    }

    #[test]
    fn generic_method_inferred_clean() {
        // Inference picks T from the arg type.
        assert_clean(
            "struct P { x: i32 } \
             impl P { fn cast[T](self, value: T) -> T { return value; } } \
             fn main() -> i32 { \
                 let p: P = P { x: 0 }; \
                 return p.cast(42); \
             }",
        );
    }

    #[test]
    fn generic_assoc_call_with_turbofish_clean() {
        // `Type::method::[T](...)` form.
        assert_clean(
            "struct P { x: i32 } \
             impl P { fn ident[T](value: T) -> T { return value; } } \
             fn main() -> i32 { return P::ident::[i32](42); }",
        );
    }

    #[test]
    fn generic_method_turbofish_arity_mismatch_e0501() {
        let codes = errors(
            "struct P { x: i32 } \
             impl P { fn cast[T](self, value: T) -> T { return value; } } \
             fn main() -> i32 { \
                 let p: P = P { x: 0 }; \
                 return p.cast::[i32, bool](42); \
             }",
        );
        assert!(codes.contains(&"E0501"), "expected E0501, got: {codes:?}");
    }

    #[test]
    fn turbofish_on_non_generic_method_e0501() {
        let codes = errors(
            "struct P { x: i32 } \
             impl P { fn id(self) -> i32 { return self.x; } } \
             fn main() -> i32 { \
                 let p: P = P { x: 0 }; \
                 return p.id::[i32](); \
             }",
        );
        assert!(codes.contains(&"E0501"), "expected E0501, got: {codes:?}");
    }

    #[test]
    fn generic_enum_pattern_payload_binds_concrete_type_clean() {
        // The bound payload (`v`) must have the concrete instantiation
        // type (`i32`), not `Ty::Param("T")`. Use it as an `i32`.
        assert_clean(
            "enum Option[T] { Some(T), None } \
             fn add_one_or(o: Option[i32], default: i32) -> i32 { \
                 return match o { \
                     Option[i32]::Some(v) => v + 1, \
                     Option[i32]::None => default, \
                 }; \
             } \
             fn main() -> i32 { return 0; }",
        );
    }

    // Phase 11 slice 11.LAYOUT: size_of[T]() / align_of[T]() intrinsics.

    #[test]
    fn size_of_primitive_clean() {
        assert_clean(
            "fn main() -> i32 { let n: usize = size_of::[i32](); return 0; }",
        );
    }

    #[test]
    fn align_of_primitive_clean() {
        assert_clean(
            "fn main() -> i32 { let a: usize = align_of::[i32](); return 0; }",
        );
    }

    #[test]
    fn size_of_struct_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 } \
             fn main() -> i32 { let n: usize = size_of::[Point](); return 0; }",
        );
    }

    #[test]
    fn size_of_returns_usize() {
        // Result must be usable in usize arithmetic without a cast.
        assert_clean(
            "fn main() -> i32 { let n: usize = size_of::[i32]() *% 10 as usize; return 0; }",
        );
    }

    #[test]
    fn size_of_no_type_arg_rejected_e0501() {
        let codes = errors("fn main() -> i32 { let n: usize = size_of(); return 0; }");
        assert!(codes.contains(&"E0501"), "expected E0501 for missing type arg, got: {codes:?}");
    }

    #[test]
    fn size_of_two_type_args_rejected_e0501() {
        let codes = errors(
            "fn main() -> i32 { let n: usize = size_of::[i32, bool](); return 0; }",
        );
        assert!(codes.contains(&"E0501"), "expected E0501 for two type args, got: {codes:?}");
    }

    #[test]
    fn size_of_with_value_arg_rejected_e0302() {
        let codes = errors(
            "fn main() -> i32 { let n: usize = size_of::[i32](7); return 0; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302 for value arg, got: {codes:?}");
    }

    #[test]
    fn size_of_unknown_type_rejected_e0303() {
        let codes = errors(
            "fn main() -> i32 { let n: usize = size_of::[Bogus](); return 0; }",
        );
        assert!(codes.contains(&"E0303"), "expected E0303 for unknown type, got: {codes:?}");
    }

    #[test]
    fn align_of_no_type_arg_rejected_e0501() {
        let codes = errors("fn main() -> i32 { let n: usize = align_of(); return 0; }");
        assert!(codes.contains(&"E0501"), "expected E0501 for missing type arg, got: {codes:?}");
    }

    #[test]
    fn size_of_raw_pointer_type_clean() {
        // Verifies size_of works for raw-pointer types — needed for
        // allocator implementations that hand out typed pointers.
        assert_clean(
            "fn main() -> i32 { let n: usize = size_of::[*u8](); return 0; }",
        );
    }

    // Phase 11 / P3 from null design (design.md): integer-to-raw-pointer
    // casts. `0 as *T` is how C+ expresses FFI null and how integer addresses
    // become typed pointers. Gated by `unsafe` — the cast itself just
    // reinterprets bits; the unsafety is trusting the integer is a valid address.

    #[test]
    fn int_to_raw_pointer_cast_in_unsafe_clean() {
        assert_clean(
            "fn main() -> i32 { let p: *u8 = unsafe { 0 as *u8 }; return 0; }",
        );
    }

    #[test]
    fn int_to_raw_pointer_cast_outside_unsafe_rejected_e0801() {
        let codes = errors(
            "fn main() -> i32 { let p: *u8 = 0 as *u8; return 0; }",
        );
        assert!(codes.contains(&"E0801"), "expected E0801 outside unsafe, got: {codes:?}");
    }

    #[test]
    fn usize_to_raw_pointer_cast_in_unsafe_clean() {
        // Real-world FFI: take an integer-flavored address from a C API
        // (e.g. mmap's return) and treat it as a typed pointer.
        assert_clean(
            "fn main() -> i32 { let addr: usize = 0xDEAD as usize; let p: *u8 = unsafe { addr as *u8 }; return 0; }",
        );
    }

    #[test]
    fn cast_to_double_indirection_pointer_clean() {
        assert_clean(
            "fn main() -> i32 { let p: **i32 = unsafe { 0 as **i32 }; return 0; }",
        );
    }

    // Phase 11 / ObjC interop: `#[link_name = "..."]` attribute.
    // Aliases an extern fn's linker symbol so multiple typed signatures
    // can resolve to the same C symbol. The load-bearing trick for ObjC
    // `objc_msgSend`-style FFI where one symbol takes many call shapes.

    #[test]
    fn link_name_on_extern_fn_clean() {
        assert_clean(
            "#[link_name = \"abs\"] extern fn my_abs(x: i32) -> i32; \
             fn main() -> i32 { return unsafe { my_abs(0 -% 42) }; }",
        );
    }

    #[test]
    fn link_name_on_non_extern_fn_rejected_e0356() {
        let codes = errors(
            "#[link_name = \"foo\"] fn local(x: i32) -> i32 { return x; } \
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0356"), "expected E0356 on non-extern link_name, got: {codes:?}");
    }

    #[test]
    fn link_name_aliases_two_decls_to_same_symbol_clean() {
        // The headline ObjC use case shape: two typed signatures, one symbol.
        // Sema accepts; codegen dedups the `declare` (verified by e2e).
        assert_clean(
            "#[link_name = \"objc_msgSend\"] extern fn msg_void(recv: *u8, sel: *u8); \
             #[link_name = \"objc_msgSend\"] extern fn msg_id(recv: *u8, sel: *u8) -> *u8; \
             fn main() -> i32 { return 0; }",
        );
    }

    // Phase 11 slice 11.FN_PTR: function pointer types + values.

    #[test]
    fn fn_pointer_type_parses_in_let_annotation() {
        assert_clean(
            "fn double(x: i32) -> i32 { return x +% x; } \
             fn main() -> i32 { let f: fn(i32) -> i32 = double; return f(5); }",
        );
    }

    #[test]
    fn fn_pointer_type_no_return_parses() {
        assert_clean(
            "fn handler(x: i32) { println(x); } \
             fn main() -> i32 { let f: fn(i32) = handler; f(7); return 0; }",
        );
    }

    #[test]
    fn fn_pointer_as_struct_field_parses() {
        assert_clean(
            "struct Actions { on_click: fn(i32) -> i32 } \
             fn click(x: i32) -> i32 { return x +% 1; } \
             fn main() -> i32 { let a: Actions = Actions { on_click: click }; return a.on_click(5); }",
        );
    }

    #[test]
    fn fn_pointer_as_fn_parameter_parses() {
        assert_clean(
            "fn apply(f: fn(i32) -> i32, x: i32) -> i32 { return f(x); } \
             fn double(x: i32) -> i32 { return x +% x; } \
             fn main() -> i32 { return apply(double, 7); }",
        );
    }

    #[test]
    fn fn_pointer_signature_mismatch_rejected_e0302() {
        let codes = errors(
            "fn double(x: i32) -> i32 { return x +% x; } \
             fn main() -> i32 { let f: fn(i32, i32) -> i32 = double; return 0; }",
        );
        assert!(codes.contains(&"E0302"), "expected E0302 for signature mismatch, got: {codes:?}");
    }

    #[test]
    fn fn_used_as_value_without_expected_fnptr_e0312() {
        // Defensive: without an expected FnPtr type, a fn name in value
        // position still fires E0312 (existing behavior). Coercion is
        // type-directed only.
        let codes = errors(
            "fn double(x: i32) -> i32 { return x +% x; } \
             fn main() -> i32 { let f = double; return 0; }",
        );
        assert!(codes.contains(&"E0312"), "expected E0312 for bare fn-as-value without expected type, got: {codes:?}");
    }

    #[test]
    fn generic_fn_as_pointer_rejected_e0821() {
        let codes = errors(
            "fn identity[T](x: T) -> T { return x; } \
             fn main() -> i32 { let f: fn(i32) -> i32 = identity; return 0; }",
        );
        assert!(codes.contains(&"E0821"), "expected E0821 for generic fn as pointer, got: {codes:?}");
    }

    #[test]
    fn indirect_call_wrong_arity_e0308() {
        let codes = errors(
            "fn double(x: i32) -> i32 { return x +% x; } \
             fn main() -> i32 { let f: fn(i32) -> i32 = double; return f(1, 2); }",
        );
        assert!(codes.contains(&"E0308"), "expected E0308 for wrong arity, got: {codes:?}");
    }

    #[test]
    fn fn_pointer_in_extern_fn_signature_clean() {
        // The headline ObjC interop use case: extern fn takes a callback.
        assert_clean(
            "extern fn atexit(cb: fn()) -> i32; \
             fn cleanup() { } \
             fn main() -> i32 { return unsafe { atexit(cleanup) }; }",
        );
    }

    #[test]
    fn size_of_inside_generic_fn_clean() {
        // size_of::[T]() inside a generic fn body — the type arg `T` is
        // a Ty::Param at sema-time; resolve_type allows it; monomorphize
        // substitutes T to the concrete type via subst_type_ast.
        assert_clean(
            "fn typed_alloc[T](n: usize) -> usize { return n *% size_of::[T](); } \
             fn main() -> i32 { let bytes: usize = typed_alloc::[i32](10 as usize); return 0; }",
        );
    }

    // ---- Phase 5 Slice 5.C: `pub extern fn body` export signature gates ----

    #[test]
    fn pub_extern_fn_with_scalar_args_clean() {
        assert_clean(
            "pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }"
        );
    }

    #[test]
    fn pub_extern_fn_with_raw_pointer_clean() {
        assert_clean(
            "pub extern fn load(p: *i32) -> i32 { return unsafe { *p }; }"
        );
    }

    #[test]
    fn pub_extern_fn_with_repr_c_struct_clean() {
        assert_clean(
            "#[repr(C)]\n\
             struct Point { x: i32, y: i32 }\n\
             pub extern fn sum(p: Point) -> i32 { return p.x + p.y; }"
        );
    }

    #[test]
    fn pub_extern_fn_with_str_rejected_e0410() {
        let codes = errors("pub extern fn echo(s: str) -> i32 { return 0; }");
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_string_return_rejected_e0410() {
        let codes = errors("pub extern fn make() -> string { return string::new(); }");
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_slice_rejected_e0410() {
        let codes = errors("pub extern fn len(s: i32[]) -> i32 { return 0; }");
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_tagged_enum_rejected_e0410() {
        let codes = errors(
            "enum Opt { Some(i32), None }\n\
             pub extern fn take(o: Opt) -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_plain_enum_clean() {
        // Plain (untagged) enum lowers to i32 — fine across the C ABI.
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             pub extern fn pick(c: Color) -> i32 { return 0; }"
        );
    }

    #[test]
    fn pub_extern_fn_with_non_repr_c_struct_rejected_e0410() {
        let codes = errors(
            "struct Point { x: i32, y: i32 }\n\
             pub extern fn sum(p: Point) -> i32 { return p.x + p.y; }"
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_drop_struct_rejected_e0410() {
        let codes = errors(
            "#[repr(C)]\n\
             struct R { fd: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             pub extern fn use_it(r: R) -> i32 { return r.fd; }"
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_unit_return_clean() {
        // Unit return is fine — maps to C `void`.
        assert_clean(
            "pub extern fn noop() { return; }"
        );
    }

    #[test]
    fn pub_extern_fn_with_array_clean() {
        // Fixed-size array of C-compatible element is layout-compatible
        // with C `T[N]`.
        assert_clean(
            "pub extern fn first(xs: [i32; 4]) -> i32 { return xs[0 as usize]; }"
        );
    }

    #[test]
    fn pub_extern_fn_with_fn_ptr_clean() {
        // Function-pointer params/returns work when their own signatures
        // are C-exportable.
        assert_clean(
            "pub extern fn invoke(f: fn(i32) -> i32, x: i32) -> i32 { return f(x); }"
        );
    }

    #[test]
    fn pub_extern_fn_with_fn_ptr_of_slice_rejected_e0410() {
        // A fn-ptr whose param uses a non-C type propagates the rejection.
        let codes = errors(
            "pub extern fn bad(f: fn(i32[]) -> i32) -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_non_repr_c_field_in_struct_rejected_e0410() {
        // A `#[repr(C)]` struct still must have C-exportable fields.
        let codes = errors(
            "#[repr(C)]\n\
             struct Outer { inner: str }\n\
             pub extern fn use_it(o: Outer) -> i32 { return 0; }"
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    // ---- v0.0.3 Phase 5 Slice 5E.2: async fn + await sema ----

    const FUTURE_PRELUDE: &str = "pub struct Future[T] { pub handle: *u8 } ";

    #[test]
    fn async_fn_body_returns_inner_type() {
        // `async fn foo() -> i32` body uses `return X` for X: i32,
        // NOT for X: Future[i32]. The `Future[i32]` wrap is sema's
        // view at call sites only.
        assert_clean(&format!("{FUTURE_PRELUDE}async fn fetch() -> i32 {{ return 42 as i32; }}"));
    }

    #[test]
    fn async_fn_signature_resolves_to_future_at_call_site() {
        // Calling an async fn from a sync context yields `Future[i32]`,
        // which the sync fn can hold but can't await — so we just bind
        // it to a local typed `Future[i32]`.
        assert_clean(&format!(
            "{FUTURE_PRELUDE}async fn fetch() -> i32 {{ return 1 as i32; }} \
             fn main() -> i32 {{ let f: Future[i32] = fetch(); return 0; }}"
        ));
    }

    #[test]
    fn await_outside_async_fn_e0901() {
        let codes = errors(&format!(
            "{FUTURE_PRELUDE}async fn fetch() -> i32 {{ return 1 as i32; }} \
             fn main() -> i32 {{ let x: i32 = await fetch(); return x; }}"
        ));
        assert!(codes.contains(&"E0901"), "expected E0901, got: {codes:?}");
    }

    #[test]
    fn await_of_non_future_e0902() {
        let codes = errors(&format!(
            "{FUTURE_PRELUDE}async fn bad() -> i32 {{ let x: i32 = await (7 as i32); return x; }}"
        ));
        assert!(codes.contains(&"E0902"), "expected E0902 (await of non-Future), got: {codes:?}");
    }

    #[test]
    fn await_inside_async_fn_yields_inner_type() {
        // Chained async: `outer` awaits `inner`'s Future[i32] and binds
        // the i32 result. Sema-only check; codegen still traps on
        // await but the typecheck must pass.
        assert_clean(&format!(
            "{FUTURE_PRELUDE}async fn inner() -> i32 {{ return 7 as i32; }} \
             async fn outer() -> i32 {{ let x: i32 = await inner(); return x; }}"
        ));
    }

    #[test]
    fn async_fn_without_future_in_scope_e0300() {
        // No Future template imported → wrap_in_future fails.
        let codes = errors("async fn fetch() -> i32 { return 0 as i32; }");
        assert!(codes.contains(&"E0300"), "expected E0300 (Future not in scope), got: {codes:?}");
    }

    // ---- v0.0.4 Phase 1D: E0900 borrow-across-await parameter guard ----

    #[test]
    fn async_fn_with_str_param_emits_e0900() {
        let codes = errors(&format!(
            "{FUTURE_PRELUDE}async fn fetch(url: str) -> i32 {{ return 0 as i32; }}"
        ));
        assert!(codes.contains(&"E0900"), "expected E0900 (str borrow in async param), got: {codes:?}");
    }

    #[test]
    fn async_fn_with_slice_param_emits_e0900() {
        let codes = errors(&format!(
            "{FUTURE_PRELUDE}async fn proc(buf: i32[]) -> i32 {{ return 0 as i32; }}"
        ));
        assert!(codes.contains(&"E0900"), "expected E0900 (slice borrow in async param), got: {codes:?}");
    }

    #[test]
    fn async_fn_with_owned_string_param_clean() {
        // `string` is owned → safe to hold across await.
        assert_clean(&format!(
            "{FUTURE_PRELUDE}async fn fetch(url: string) -> i32 {{ return 0 as i32; }}"
        ));
    }

    #[test]
    fn async_fn_with_copy_param_clean() {
        // i32 is Copy → safe.
        assert_clean(&format!(
            "{FUTURE_PRELUDE}async fn id(x: i32) -> i32 {{ return x; }}"
        ));
    }

    #[test]
    fn async_fn_with_mut_noncopy_param_emits_e0900() {
        // `mut buf: string` is pointer-passed in Phase-6 ABI; storage
        // doesn't necessarily live in the coroutine frame.
        let codes = errors(&format!(
            "{FUTURE_PRELUDE}async fn proc(mut buf: string) -> i32 {{ return 0 as i32; }}"
        ));
        assert!(codes.contains(&"E0900"), "expected E0900 (mut non-Copy param), got: {codes:?}");
    }

    // ---- v0.0.4 Phase 2 Slice 2A: Send / Sync marker interfaces ----

    #[test]
    fn send_bound_accepts_primitive() {
        // `fn worker[T: Send](x: T) -> T { return x; }` instantiated with
        // i32 must pass. v0.0.4 baseline is permissive — every type is
        // Send — so this is the canonical "vocabulary works" check.
        assert_clean(
            "fn worker[T: Send](x: T) -> T { return x; }\n\
             fn main() -> i32 { return worker::[i32](42); }"
        );
    }

    #[test]
    fn send_bound_accepts_user_struct() {
        // User-defined struct: also Send under the v0.0.4 baseline.
        assert_clean(
            "struct Pt { x: i32, y: i32 }\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let p: Pt = Pt { x: 1, y: 2 };\n\
                 let q: Pt = ship::[Pt](p);\n\
                 return q.x;\n\
             }"
        );
    }

    #[test]
    fn sync_bound_accepts_primitive() {
        // Same shape, Sync bound.
        assert_clean(
            "fn share[T: Sync](x: T) -> T { return x; }\n\
             fn main() -> i32 { return share::[i32](42); }"
        );
    }

    #[test]
    fn send_and_sync_compose_with_other_bounds() {
        // Multiple bounds on one type param — verifies the bound-list
        // parsing/resolution sees Send / Sync as first-class.
        assert_clean(
            "fn need_both[T: Send + Sync](x: T) -> T { return x; }\n\
             fn main() -> i32 { return need_both::[i32](7); }"
        );
    }
}
