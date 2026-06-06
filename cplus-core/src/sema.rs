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
//! - E0344: (retired v0.0.14) tagged-enum Drop payloads are now supported via synthesized enum-variant drop
//! - E0345: use of possibly-unassigned binding (definite-assignment failure)
//! - E0346: uninitialized `let` requires a type annotation

use crate::ast::*;
use crate::diagnostics::{DiagCode, DiagSink, Diagnostic, LineMap, Severity};
use crate::lexer::{NumSuffix, Span as ByteSpan};
use std::collections::{BTreeSet, HashMap};
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
    I8,
    I16,
    I32,
    I64,
    // Unsigned integers
    U8,
    U16,
    U32,
    U64,
    // Pointer-sized
    Isize,
    Usize,
    // Floats
    F16,
    F32,
    F64,
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
    FnPtr {
        params: Vec<Ty>,
        return_type: Box<Ty>,
    },
    Enum(EnumId),
    Struct(StructId),
    /// Fixed-size array: element type + length.
    Array(Box<Ty>, u32),
    /// v0.0.6 Slice 1B: fixed-width SIMD vector. `elem` is the per-lane
    /// scalar type (one of the numeric primitives); `lanes` is the lane
    /// count. Lowered to LLVM `<lanes x elem>`. Distinct from `Array`
    /// because arithmetic + intrinsic methods dispatch through a SIMD
    /// method table and codegen uses vector ops (`fadd <4 x float>` vs
    /// scalar `fadd float` element-by-element). Copy.
    Simd {
        elem: Box<Ty>,
        lanes: u32,
    },
    /// v0.0.9 follow-up: SIMD comparison-result mask, distinct from
    /// `Ty::Simd`. The element type matches the width-equivalent signed
    /// integer (mask32x4's `elem` is `Ty::I32`, etc.); lanes counts the
    /// vector width. LLVM lowering is identical to the matching `Ty::Simd`
    /// (`<lanes x iN>`) — the distinction is type-system-only, kept so
    /// `select` / `any` / `all` can reject non-mask arguments, comparison
    /// results carry forward as mask values, and arithmetic on masks fires
    /// a real diagnostic instead of silently working. Crossing to a real
    /// integer SIMD requires `mask.to_bits()`; crossing back requires
    /// `simd.to_mask()`. Both are zero-cost relabels at the IR level.
    Mask {
        elem: Box<Ty>,
        lanes: u32,
    },
    /// Slice 7GEN.4: a generic type parameter, identified by name. Appears
    /// inside the body of a generic fn / method / struct / enum or inside an
    /// `interface` / `impl Interface for ...` block (where `Self` is
    /// represented as `Param("Self")`). Two `Ty::Param` values are equal
    /// iff their names match — the surrounding signature gives them meaning.
    /// Substitution at instantiation time (slice 7GEN.5) replaces each
    /// `Param` with a concrete type.
    Param(String),
    Error, // sentinel for recovery; matches anything
}

impl Ty {
    /// Human-readable type name. For enums and structs we render a generic
    /// kind name; SemaCx has the actual table if higher-fidelity names are
    /// needed in a diagnostic message.
    pub fn name(&self) -> &'static str {
        match self {
            Ty::I8 => "i8",
            Ty::I16 => "i16",
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::U8 => "u8",
            Ty::U16 => "u16",
            Ty::U32 => "u32",
            Ty::U64 => "u64",
            Ty::Isize => "isize",
            Ty::Usize => "usize",
            Ty::F16 => "f16",
            Ty::F32 => "f32",
            Ty::F64 => "f64",
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
            Ty::Simd { .. } => "simd",
            Ty::Mask { .. } => "mask",
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
    pub fn is_int(&self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }
    pub fn is_float(&self) -> bool {
        matches!(self, Ty::F16 | Ty::F32 | Ty::F64)
    }
    pub fn is_numeric(&self) -> bool {
        self.is_int() || self.is_float()
    }
    pub fn is_enum(&self) -> bool {
        matches!(self, Ty::Enum(_))
    }
    pub fn is_struct(&self) -> bool {
        matches!(self, Ty::Struct(_))
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Ty::Array(_, _))
    }

    /// Phase 3 conservative `Copy` rule: primitives, `bool`, `()`, and plain
    /// Atomic `Copy` rule: types whose `Copy`-ness is fixed by the type itself,
    /// not by its components. Primitives, `bool`, `()`, and the `Error`
    /// sentinel (treated as Copy to avoid cascading move diagnostics on
    /// already-broken code). For composite types (`Array`, `Struct`,
    /// `Enum`) call `SemaCx::is_copy(&ty)` instead — the answer depends on
    /// the type table (a tagged enum is Copy iff every payload is Copy).
    pub fn is_atomic_copy(&self) -> bool {
        match self {
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
            | Ty::Str
            | Ty::Slice(_)
            | Ty::RawPtr(_)
            | Ty::FnPtr { .. }
            | Ty::Simd { .. }
            | Ty::Mask { .. }
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
    /// v0.0.5 Phase 2C: inherent methods declared via `impl EnumName { fn
    /// ... }`. Keyed by method name; same shape as `StructDef::methods`.
    /// Empty for enums without an explicit impl block.
    pub methods: HashMap<String, MethodSig>,
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
    /// v0.0.9 follow-up: `borrow x: T` — explicit shared by-value
    /// parameter. Phase 5 mechanism plumbing — propagates `Param.borrow_`
    /// from the AST so call-site logic can opt out of the future
    /// "move-by-default for non-Copy" behaviour. Today this flag is
    /// purely informational; the default at call sites remains
    /// "shared, no consume". When the Phase 5 flip lands, an unmarked
    /// non-Copy param will consume the caller's binding *unless*
    /// `borrow_` is set.
    pub borrow_: bool,
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
        self.fields
            .iter()
            .enumerate()
            .find_map(|(i, (n, t, _))| (n == name).then(|| (i as u32, t.clone())))
    }

    /// Like `field` but also returns the field's `pub` flag.
    pub fn field_with_pub(&self, name: &str) -> Option<(u32, Ty, bool)> {
        self.fields
            .iter()
            .enumerate()
            .find_map(|(i, (n, t, p))| (n == name).then(|| (i as u32, t.clone(), *p)))
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
    /// Possible roots for a borrow-shaped local (`str` / `T[]`). Empty
    /// means literal/static/unknown provenance. Non-empty roots are used
    /// to reject returning views into locals that will be dropped.
    borrow_roots: BTreeSet<String>,
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
    pub struct_instantiations:
        std::collections::BTreeMap<(String, Vec<Ty>), StructInstantiationInfo>,
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
    /// v0.0.6 Slice 1A / v0.0.7 Slice 3.1: `include_bytes!("path")` and
    /// `include_str!("path")` resolved entries. Keyed by the call
    /// expression's span. Each entry carries the resolved absolute path
    /// (for dedup) and the file bytes read at sema time. Codegen consumes
    /// this to emit one private constant `[N x i8]` global per unique
    /// absolute path; the AST node variant (`IncludeBytes` vs
    /// `IncludeStr`) determines whether the lowered expression is a raw
    /// pointer or a `str` fat-pointer aggregate.
    pub compile_time_blobs: HashMap<ByteSpan, CompileTimeBlobEntry>,
    /// v0.0.8 Phase 4: per-call-site `env!("NAME")` lookup. Resolved at
    /// sema time (read from the compiler's process environment via
    /// `std::env::var`). Value is the env var's value as a UTF-8 string.
    /// Codegen consumes this to emit one private `[N x i8]` global per
    /// unique env value + the matching `{ ptr, i64 }` fat-pointer
    /// construction at the use site. Same dedup behavior as
    /// `compile_time_blobs` but keyed by the env var name (sema dedup
    /// happens in the codegen-side emission pass).
    pub env_vars: HashMap<ByteSpan, EnvVarEntry>,
    /// v0.0.9 Phase 4: module-scope `static mut? NAME: Ty = LIT;` items.
    /// Keyed by qualified name. Codegen iterates this to emit one
    /// LLVM global per static, then routes use-site Ident references
    /// through load/store ops against the emitted symbol.
    pub statics: std::collections::BTreeMap<String, StaticInfo>,
    /// v0.0.10 Phase 4A: unique ObjC selector names used by `#selector`
    /// and `#msg_send` intrinsics. Codegen emits one cached-pointer
    /// global pair per name: `@__cplus.sel.<n>.{data, cached}`.
    pub selectors: std::collections::BTreeSet<String>,
    /// v0.0.10 Phase 4C: `#compile_shader`-produced byte blobs.
    /// Keyed by the call expression's span. Codegen emits one private
    /// constant `[N x i8]` global per entry.
    pub shader_blobs: HashMap<ByteSpan, Vec<u8>>,
    /// v0.0.14 graph value-depth: `(origin_file, expr_span, rendered_type)` for
    /// every type-checked expression. Empty unless the graph/LSP entry point
    /// requested it; backs inferred `type-at`.
    pub value_types: Vec<(Option<String>, ByteSpan, String)>,
}

/// v0.0.6 Slice 1A / v0.0.7 Slice 3.1: one resolved `include_bytes!` or
/// `include_str!` call.
#[derive(Debug, Clone)]
pub struct CompileTimeBlobEntry {
    pub abs_path: std::path::PathBuf,
    pub bytes: Vec<u8>,
}

/// v0.0.8 Phase 4: one resolved `env!("NAME")` call.
#[derive(Debug, Clone)]
pub struct EnvVarEntry {
    pub name: String,
    pub value: String,
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
    check_with_files_inner(program, entry_file, entry_src, files, false)
}

/// v0.0.14 graph value-depth entry: like `check_multi_with_mono`, but also
/// records every expression's resolved type into `MonoInfo.value_types` for
/// the code-knowledge-graph `type-at` index. Used by `cpc graph`/LSP, not the
/// compile path (which never pays the recording cost).
pub fn check_multi_with_value_types(
    program: &Program,
    entry_file: PathBuf,
    entry_src: &str,
    files: std::collections::BTreeMap<String, (PathBuf, String)>,
) -> (Vec<Diagnostic>, MonoInfo) {
    check_with_files_inner(program, entry_file, entry_src, files, true)
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
    check_with_files_inner(program, file, src, files_raw, false).0
}

fn check_with_files_inner<'a>(
    program: &Program,
    file: PathBuf,
    src: &'a str,
    files_raw: std::collections::BTreeMap<String, (PathBuf, String)>,
    record_types: bool,
) -> (Vec<Diagnostic>, MonoInfo) {
    let lm = LineMap::new(src);
    let mut sink = DiagSink::new();
    let files: std::collections::BTreeMap<String, FileCtx> = files_raw
        .into_iter()
        .map(|(fid, (p, s))| {
            let lm = LineMap::new(&s);
            (
                fid,
                FileCtx {
                    path: p,
                    src: s,
                    lm,
                },
            )
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
        current_fn_is_gen: false,
        current_gen_yield_ty: None,
        current_fn_param_names: std::collections::HashSet::new(),
        current_fn_param_regions: HashMap::new(),
        current_fn_return_region: None,
        current_fn_no_alloc: false,
        current_fn_no_block: false,
        method_contracts: HashMap::new(),
        current_file: None,
        files,
        loop_depth: 0,
        unsafe_depth: 0,
        extern_fns: std::collections::HashSet::new(),
        type_params_stack: Vec::new(),
        param_bounds_stack: Vec::new(),
        self_type_stack: Vec::new(),
        interfaces: HashMap::new(),
        interface_impls: std::collections::HashSet::new(),
        marker_overrides: HashMap::new(),
        no_alloc_drop_types: std::collections::HashSet::new(),
        record_value_types: false,
        value_types: Vec::new(),
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
        compile_time_blobs_table: HashMap::new(),
        env_vars_table: HashMap::new(),
        statics_table: HashMap::new(),
        selectors_table: std::collections::BTreeSet::new(),
        shader_blobs_table: HashMap::new(),
        msg_send_shapes: std::collections::BTreeSet::new(),
    };
    cx.record_value_types = record_types;
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
    cx.collect_method_contracts(program);
    cx.reconcile_drop_from_methods();
    cx.compute_struct_copy_flags();
    cx.compute_enum_copy_flags(program);
    cx.collect_functions(program);
    // v0.0.9 Phase 4: register module-scope const/static items. Runs
    // after function collection so cross-item name collisions are
    // detected; runs before body-checking so use-site lookups see the
    // table populated. The collection pass also type-checks each
    // initializer against the declared type.
    cx.collect_consts_and_statics(program);
    cx.check_main_signature(program);
    cx.validate_interface_impls(program);
    // v0.0.14 `#[no_alloc]` drop-glue: record which types have a
    // `#[no_alloc]`-marked `drop` before body-checking, so the scope-exit
    // drop-glue check in `check_stmt` can consult it.
    cx.collect_no_alloc_drop_types(program);
    cx.lint_generic_fn_bodies(program);
    cx.check_functions(program);
    cx.check_methods(program);
    // v0.0.10 Phase 1: `#[no_alloc]` real-time contract.
    // Walks every `#[no_alloc]`-marked function's body, resolves direct
    // callee names, and rejects calls into the allocator blocklist or
    // into user functions that aren't themselves marked `#[no_alloc]`.
    // Must run after `check_functions` / `check_methods` so the body's
    // call sites have already been type-checked (we trust the AST shape).
    // v0.0.10 Phase 3: `#[bounded_recursion]` rides the same walker.
    cx.check_no_alloc(program);
    cx.check_naked(program);
    cx.check_no_block(program);
    cx.check_bounded_recursion(program);
    cx.check_max_stack(program);
    // v0.0.13 (plan.opaque.md): raw-pointer accountability. Every raw-pointer
    // struct field must be accounted for — released by the struct's `drop`, or
    // marked `opaque` (managed elsewhere) — otherwise E0510. Structural check
    // over the `drop` body (no dataflow); runs after method collection so the
    // drop bodies are available.
    cx.check_raw_pointer_accountability(program);
    // v0.0.5 Phase 3 Slice 3C: enum-pattern-discovery propagation.
    //
    // Sema's check_method type-checks each impl method body *once* using
    // `Ty::Param` placeholders for the impl's generic params, so any
    // `Option[T]::Some(v)` pattern inside (e.g.) `Iterator[T]::filter`
    // registers as a placeholder `Option[Param("T")]` — filtered out
    // when MonoInfo is built. Mono later synthesizes `filter` with
    // `T = i32`, but by then sema is gone and there's no machinery to
    // register `Option[i32]` as a real instantiation. Codegen then
    // panics on `Ty::Enum(EnumId(0))` (lookup_option_ty's fallback).
    //
    // Fix: iterate concrete struct_instantiations × generic-impl-method
    // bodies, walk each body's `match`/`if let`/`while let`/`guard let`
    // patterns, and for every `PatternKind::Variant { enum_name,
    // type_args, .. }` with non-empty `type_args`, substitute through
    // the struct's subst and call `instantiate_enum_from_arg_tys`. Run
    // to a fixed point so an instantiation that itself drags in another
    // enum (e.g. a `Vec[Option[i32]]` field type) gets registered too.
    cx.propagate_pattern_instantiations(program);
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
        compile_time_blobs: std::mem::take(&mut cx.compile_time_blobs_table),
        env_vars: std::mem::take(&mut cx.env_vars_table),
        statics: cx
            .statics_table
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        selectors: std::mem::take(&mut cx.selectors_table),
        shader_blobs: std::mem::take(&mut cx.shader_blobs_table),
        value_types: std::mem::take(&mut cx.value_types),
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
    /// v0.0.4 Phase 4 Slice 4A: tracks whether the surrounding fn is a
    /// `gen fn`. `yield` is only valid when true.
    current_fn_is_gen: bool,
    /// v0.0.4 Phase 4 Slice 4A: the inner yield element type for the
    /// surrounding `gen fn`. `None` outside a gen fn or when the
    /// return type is malformed.
    current_gen_yield_ty: Option<Ty>,
    /// v0.0.12 (returned-borrow checking): names bound as parameters (plus
    /// `self` for methods) of the function currently being checked. Lets the
    /// `return` site tell a parameter-rooted borrow (caller-tied, sound) from a
    /// local-rooted one (dropped at function exit → dangling). Set fresh at the
    /// top of `check_function` / `check_method`.
    current_fn_param_names: std::collections::HashSet<String>,
    /// v0.0.12 (#2 region enforcement): map from parameter name to the explicit
    /// `borrow REGION T` region it carries, for parameters that have one. Used
    /// to validate that a region-annotated return borrows a same-region param.
    current_fn_param_regions: HashMap<String, String>,
    /// v0.0.12 (#2 region enforcement): the explicit region on the current
    /// function's return type (`-> borrow REGION T`), if any.
    current_fn_return_region: Option<String>,
    /// v0.0.12 realtime Phase 1 (method-dispatch hole): whether the function
    /// whose body is currently being checked carries `#[no_alloc]` (directly
    /// or via `#[realtime]` / a `[profile.realtime]` injection). Set fresh at
    /// the top of each body-check entry. Drives the method-call contract check
    /// in `check_method_call` — the free-call / interpolation cases stay in the
    /// post-pass `check_no_alloc`, but method dispatch (`recv.method()`) can
    /// only be resolved precisely here, where the receiver type is known.
    current_fn_no_alloc: bool,
    /// Companion to `current_fn_no_alloc` for the `#[no_block]` contract.
    current_fn_no_block: bool,
    /// v0.0.12 realtime Phase 1: `(type_name, method_name)` → `(no_alloc,
    /// no_block)` for every method in any `impl` block, keyed by the
    /// *source-level* target type name (so a generic `impl Vec[T]` is keyed
    /// `("Vec", "push")` and matches an instantiation via its generic origin).
    /// Built once by `collect_method_contracts` from the actual method
    /// attributes, so the verdict is correct for dependency methods regardless
    /// of any local `[profile.realtime]` injection.
    method_contracts: HashMap<(String, String), (bool, bool)>,
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
    /// v0.0.5: parallel to `type_params_stack` — maps each in-scope
    /// generic param to its declared interface bounds. Lets method-call
    /// dispatch on `Ty::Param(name)` find the bound interface's method
    /// signature instead of erroring out with E0324. Pushed/popped
    /// together with `type_params_stack` via `push_type_params` /
    /// `pop_type_params`.
    param_bounds_stack: Vec<HashMap<String, Vec<String>>>,
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
    /// v0.0.14: registered `unsafe impl Send/Sync for T {}` overrides, keyed
    /// by `(marker, type_name)` where `type_name` is the source-written name
    /// (the template leaf for a generic target, e.g. `"Arc"`). The value is
    /// the per-type-param bound list from a conditional impl
    /// (`unsafe impl Send for Arc[T: Send + Sync]` → `[["Send","Sync"]]`);
    /// an empty Vec means an unconditional override (`unsafe impl Send for
    /// Handle {}`). Consulted by `is_send`/`is_sync` to re-enable a type the
    /// structural raw-pointer rule would otherwise reject.
    marker_overrides: HashMap<(String, String), Vec<Vec<String>>>,
    /// v0.0.14 `#[no_alloc]` drop-glue: leaf names of types whose user `drop`
    /// method carries `#[no_alloc]`/`#[realtime]` (so its body was verified
    /// non-allocating by `check_no_alloc`). A drop-carrying local in a
    /// `#[no_alloc]` fn is rejected unless every destructor its scope-exit
    /// teardown runs is in this set (or is a pure auto field-drop with no
    /// heap-freeing leaf). `string`/`Vec`/`Box` are never in it — their drop
    /// frees.
    no_alloc_drop_types: std::collections::HashSet<String>,
    /// v0.0.14 graph value-depth: when set, `check_expr` records each
    /// expression's resolved type into `value_types` (off for normal compiles,
    /// so they pay nothing). Populated only by the graph/LSP entry point.
    record_value_types: bool,
    /// v0.0.14 graph value-depth: `(origin_file, expr_span, rendered_type)` for
    /// every type-checked expression, in source order. Backs inferred `type-at`
    /// (call results, arithmetic, field/index, match/if values) — the cases the
    /// AST-only graph couldn't see.
    value_types: Vec<(Option<String>, ByteSpan, String)>,
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
    /// v0.0.6 Slice 1A / v0.0.7 Slice 3.1: `include_bytes!` and
    /// `include_str!` resolutions. Sema reads the file at type-check
    /// time to compute the result type's `N` (and to UTF-8-validate
    /// for `include_str!`); the bytes are stashed here for codegen to
    /// materialize. See [`CompileTimeBlobEntry`] /
    /// [`MonoInfo::compile_time_blobs`].
    compile_time_blobs_table: HashMap<ByteSpan, CompileTimeBlobEntry>,
    /// v0.0.8 Phase 4: resolved `env!("NAME")` lookups, keyed by the
    /// macro call's source span. Sema populates; codegen reads from
    /// `MonoInfo::env_vars` (built by `mem::take` at hand-off).
    env_vars_table: HashMap<ByteSpan, EnvVarEntry>,
    /// v0.0.9 Phase 4: module-scope `static mut? NAME: Ty = LIT;` items.
    /// Keyed by qualified name (the resolver-rewritten form). Used by
    /// `resolve_value_ident` to surface the static's type at use sites
    /// and by `check_assign` to gate `static mut` writes behind
    /// `unsafe { ... }`. Codegen reads the snapshot from `MonoInfo`.
    statics_table: HashMap<String, StaticInfo>,
    /// v0.0.10 Phase 4A: ObjC selectors used by `#selector(...)` /
    /// `#msg_send(... "sel" ...)`. Codegen emits one cached-pointer
    /// global per unique selector name.
    selectors_table: std::collections::BTreeSet<String>,
    /// v0.0.10 Phase 4B: per-call objc_msgSend shapes, keyed by call
    /// span. Each entry records the (return_type, arg_types) tuple so
    /// codegen synthesizes the right per-call extern declaration.
    msg_send_shapes: std::collections::BTreeSet<ByteSpan>,
    /// v0.0.10 Phase 4C: `#compile_shader(...)`-produced byte blobs.
    /// Keyed by call span. Codegen emits one private constant global
    /// per entry.
    shader_blobs_table: HashMap<ByteSpan, Vec<u8>>,
}

/// v0.0.9 Phase 4: sema-resolved info for a module-scope `static`.
/// Initializer kept as the original AST expression so codegen can
/// render it as an LLVM constant operand without re-parsing.
#[derive(Debug, Clone)]
pub struct StaticInfo {
    /// Resolved type of the static (what the user wrote after `:`).
    pub ty: Ty,
    /// `true` for `static mut NAME: ...`. Reads and writes require
    /// `unsafe { ... }` only when this flag is set; immutable statics
    /// read safely from any context.
    pub is_mut: bool,
    /// Initializer expression, post-lower-validation. Always one of
    /// the literal shapes accepted by `lower::is_const_initializer`.
    pub init: Expr,
    /// Decl span for diagnostics that need to point at the original
    /// declaration (e.g. "first declared here").
    pub decl_span: ByteSpan,
}

/// Slice 7GEN.5e step 3: method template stored on a generic-typed
/// impl block, before any per-instantiation substitution.
#[derive(Debug, Clone)]
pub struct GenericImplMethodTemplate {
    pub name: String,
    pub receiver: Option<Receiver>,
    pub params: Vec<ParamSig>,              // may contain Ty::Param
    pub return_type: Ty,                    // may contain Ty::Param
    pub impl_generic_params: Vec<String>,   // T from `impl Vec[T]`
    pub method_generic_params: Vec<String>, // U from `fn map[U]`
    pub is_drop: bool, // marker for cached Drop bookkeeping (always false today)
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

    /// Emit a non-fatal warning. Same span-routing as `err`, but
    /// `Severity::Warning` so the build continues (`has_error` ignores it).
    /// Used for lints that flag a likely mistake without rejecting code that
    /// is occasionally legitimate.
    fn warn(&mut self, code: &'static str, msg: String, span: ByteSpan) {
        let primary = match self.current_file.as_ref().and_then(|f| self.files.get(f)) {
            Some(fc) => fc.lm.span(&fc.path, span, &fc.src),
            None => self.lm.span(&self.file, span, self.src),
        };
        self.sink.emit(Diagnostic {
            severity: Severity::Warning,
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
        // `#println(n: i32)` — emitted by codegen as a call to `printf("%d\n", n)`.
        self.fns.insert(
            "println".to_string(),
            FnSig {
                params: vec![ParamSig {
                    ty: Ty::I32,
                    mutable: false,
                    move_: false,
                    borrow_: false,
                }],
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
                        self.enum_generic_templates
                            .insert(e.name.name.clone(), e.clone());
                        continue;
                    }
                    let mut seen: HashMap<String, ()> = HashMap::new();
                    let mut variants = Vec::new();
                    for v in &e.variants {
                        if seen.contains_key(&v.name.name) {
                            self.err(
                                "E0318",
                                format!(
                                    "duplicate variant `{}` in enum `{}`",
                                    v.name.name, e.name.name
                                ),
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
                        is_copy: false, // computed later
                        is_tagged,
                        generic_base: None,
                        generic_origin: None,
                        methods: HashMap::new(),
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
                        self.struct_generic_templates
                            .insert(s.name.name.clone(), s.clone());
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
                    self.type_aliases
                        .insert(a.name.name.clone(), a.target.clone());
                }
                ItemKind::Function(_) | ItemKind::Impl(_) | ItemKind::Interface(_) => {}
                // v0.0.9 Phase 4: const/static items don't register a
                // *type* — they register a *value name* in a separate
                // pass (`collect_consts_and_statics`) after type names
                // are known. Nothing to do here.
                // v0.0.15: module-scope `#asm("...")` is inert in sema —
                // raw assembly, no name, no type to register or check.
                ItemKind::Const(_) | ItemKind::Static(_) | ItemKind::ModuleAsm(_) => {}
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
            let ItemKind::Struct(s) = &item.kind else {
                continue;
            };
            // Slice 7GEN.5c: skip generic struct templates — they
            // don't have concrete fields until instantiated.
            if !s.generic_params.is_empty() {
                continue;
            }
            let Some(&id) = self.struct_by_name.get(&s.name.name) else {
                continue;
            };
            // Slice 7GEN.4: generic-param names declared on the struct
            // (`struct Pair[A, B]`) are visible in field type positions.
            self.push_type_params(&s.generic_params);
            let mut seen: HashMap<String, ()> = HashMap::new();
            let mut fields: Vec<(String, Ty, bool)> = Vec::new();
            for f in &s.fields {
                if seen.contains_key(&f.name.name) {
                    self.err(
                        "E0319",
                        format!(
                            "duplicate field `{}` in struct `{}`",
                            f.name.name, s.name.name
                        ),
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
    /// v0.0.15: reconcile each struct's `is_drop` flag with its method table
    /// before the Copy/Drop fixpoints run. A `struct`/`impl` with a `drop`
    /// method sets `is_drop` at collection time, but a *generic instantiation*
    /// (e.g. `Vec[i32]`) can be created while resolving an enum payload / struct
    /// field *before* its template's `impl … { fn drop }` is collected, so it
    /// carries the `drop` method in its table yet never had `is_drop` set. Left
    /// unreconciled, the Copy fixpoint sees no Drop and all-Copy fields
    /// (ptr/len/cap) and flips the instantiation to Copy — which then makes any
    /// enum/struct using it as a payload/field Copy too, silently dropping the
    /// use-after-move diagnostic (the `enum W { A(Vec[i32]) }` / recursive
    /// `Node { Branch(Vec[Node]) }` / json `Value::Array(Vec[Value])` gap).
    /// Running after `collect_methods` (method tables fully populated) and before
    /// the fixpoints fixes the classification for every instantiation path.
    fn reconcile_drop_from_methods(&mut self) {
        for s in self.structs.iter_mut() {
            if !s.is_drop && s.methods.contains_key("drop") {
                s.is_drop = true;
            }
        }
    }

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

    /// Structural Send membership.
    ///
    /// v0.0.12 realtime Phase 6 (core): the single-threaded shared-ownership
    /// type `Rc[T]` and a `MutexGuard[T]` are `!Send` — moving either across
    /// threads is a soundness bug (non-atomic refcount race; a guard is bound
    /// to the acquiring thread). All other types remain `Send`. Detected by
    /// the generic template name behind the instantiated struct.
    ///
    /// Deferred (needs an `unsafe impl Send for T {}` opt-in that doesn't
    /// exist yet): the broad "structs with raw-pointer fields are `!Send`"
    /// rule. Enabling it without an escape hatch would reject most FFI code
    /// (ObjC bindings, channels, mutexes). Structural propagation through a
    /// struct that *holds* an `Rc` is likewise future work.
    pub fn is_send(&self, ty: &Ty) -> bool {
        self.type_is_marker(ty, "Send")
    }

    /// Structural Sync membership. A nominal type that (transitively) hides a
    /// raw pointer is `!Sync` unless the author opts in via `unsafe impl Sync
    /// for T {}`; `Rc[T]` is always `!Sync` (shared `&Rc` across threads would
    /// race the non-atomic refcount).
    pub fn is_sync(&self, ty: &Ty) -> bool {
        self.type_is_marker(ty, "Sync")
    }

    /// v0.0.14 broad rule: a type satisfies the `Send`/`Sync` marker unless it
    /// is a nominal type that (transitively) hides a raw pointer with no
    /// override. A *bare* raw pointer (`*u8` used directly, e.g. as a spawn
    /// result) stays Send/Sync — it is already visibly unsafe at every use;
    /// the rule targets struct/enum types that wrap a pointer behind a
    /// safe-looking API, where the unsafety would otherwise be invisible.
    fn type_is_marker(&self, ty: &Ty, marker: &str) -> bool {
        match ty {
            Ty::RawPtr(_) => true,
            _ => !self.marker_blocked(ty, marker, &mut Vec::new()),
        }
    }

    /// Does `ty` carry something that blocks `marker`? A raw-pointer *field*
    /// blocks; an `Rc`/`MutexGuard` blocks (by template-leaf name, even with
    /// no literal pointer field); a sub-aggregate blocks if it does. An
    /// `unsafe impl` override on a nominal type short-circuits — making it
    /// (and any container holding it) unblocked, subject to the conditional
    /// bounds. `visited` breaks struct/enum reference cycles.
    fn marker_blocked(&self, ty: &Ty, marker: &str, visited: &mut Vec<u32>) -> bool {
        match ty {
            Ty::RawPtr(_) => true,
            Ty::Array(elem, _) => self.marker_blocked(elem, marker, visited),
            Ty::Struct(id) => {
                let leaf = self.nominal_template_leaf(ty).map(|s| s.to_string()).unwrap_or_else(
                    || name_leaf(&self.structs[id.0 as usize].name).to_string(),
                );
                if let Some(bounds) =
                    self.marker_overrides.get(&(marker.to_string(), leaf.clone()))
                {
                    return !self.override_satisfied(ty, bounds);
                }
                if Self::is_builtin_marker_blocked(&leaf, marker) {
                    return true;
                }
                if visited.contains(&id.0) {
                    return false;
                }
                visited.push(id.0);
                let fields = self.structs[id.0 as usize].fields.clone();
                let blocked = fields
                    .iter()
                    .any(|(_, fty, _)| self.marker_blocked(fty, marker, visited));
                visited.pop();
                blocked
            }
            Ty::Enum(id) => {
                let leaf = self.nominal_template_leaf(ty).map(|s| s.to_string()).unwrap_or_else(
                    || name_leaf(&self.enums[id.0 as usize].name).to_string(),
                );
                if let Some(bounds) =
                    self.marker_overrides.get(&(marker.to_string(), leaf.clone()))
                {
                    return !self.override_satisfied(ty, bounds);
                }
                if Self::is_builtin_marker_blocked(&leaf, marker) {
                    return true;
                }
                if visited.contains(&id.0) {
                    return false;
                }
                visited.push(id.0);
                let payloads: Vec<Ty> = self.enums[id.0 as usize]
                    .variants
                    .iter()
                    .flat_map(|v| v.payload.clone())
                    .collect();
                let blocked = payloads
                    .iter()
                    .any(|pty| self.marker_blocked(pty, marker, visited));
                visited.pop();
                blocked
            }
            // Primitives, `str`/`string`/slices (safe abstractions over their
            // storage), fn pointers, SIMD, unit — never block.
            _ => false,
        }
    }

    /// The stdlib types that block a marker structurally, recognized by
    /// template-leaf name so a renamed instantiation or a pointer-free
    /// stand-in still trips the rule. `Rc` is `!Send` and `!Sync`;
    /// `MutexGuard` is `!Send` (bound to its acquiring thread).
    fn is_builtin_marker_blocked(leaf: &str, marker: &str) -> bool {
        match marker {
            "Send" => matches!(leaf, "Rc" | "MutexGuard"),
            "Sync" => leaf == "Rc",
            _ => false,
        }
    }

    /// For a conditional override (`unsafe impl Send for Arc[T: Send + Sync]`),
    /// check the instantiation's type args against the declared per-param
    /// bounds. Empty bounds = an unconditional override (`unsafe impl Send for
    /// Handle {}`). A conditional impl on a type with no recoverable args is
    /// treated as unsatisfied (the impl is malformed).
    fn override_satisfied(&self, ty: &Ty, bounds: &[Vec<String>]) -> bool {
        if bounds.is_empty() {
            return true;
        }
        let args: Vec<Ty> = match ty {
            Ty::Struct(id) => self.structs[id.0 as usize]
                .generic_origin
                .as_ref()
                .map(|(_, a)| a.clone())
                .unwrap_or_default(),
            Ty::Enum(id) => self.enums[id.0 as usize]
                .generic_origin
                .as_ref()
                .map(|(_, a)| a.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for (i, param_bounds) in bounds.iter().enumerate() {
            let Some(arg) = args.get(i) else {
                return false;
            };
            for b in param_bounds {
                if !self.satisfies_bound(arg, b) {
                    return false;
                }
            }
        }
        true
    }

    /// For an instantiated generic struct or enum, the trailing segment of its
    /// template name (`stdlib.rc.Rc` → `"Rc"`). `None` for non-nominal or
    /// non-generic types. Used to recognize stdlib marker types and match
    /// `unsafe impl` overrides structurally rather than by mangled name.
    fn nominal_template_leaf(&self, ty: &Ty) -> Option<&str> {
        match ty {
            Ty::Struct(id) => self.structs[id.0 as usize]
                .generic_origin
                .as_ref()
                .map(|(tmpl, _)| tmpl.rsplit('.').next().unwrap_or(tmpl)),
            Ty::Enum(id) => self.enums[id.0 as usize]
                .generic_origin
                .as_ref()
                .map(|(tmpl, _)| tmpl.rsplit('.').next().unwrap_or(tmpl)),
            _ => None,
        }
    }

    /// v0.0.14 graph value-depth: render a resolved `Ty` to a display string
    /// with concrete nominal names (`Vec[i32]`, `Point`, `Option[string]`,
    /// `*u8`) — unlike `ty_display`, which prints `struct`/`enum` for nominals
    /// because it has no table access.
    fn render_ty(&self, ty: &Ty) -> String {
        match ty {
            Ty::Struct(id) => {
                let d = &self.structs[id.0 as usize];
                self.render_nominal(&d.name, &d.generic_origin)
            }
            Ty::Enum(id) => {
                let d = &self.enums[id.0 as usize];
                self.render_nominal(&d.name, &d.generic_origin)
            }
            Ty::Array(elem, n) => format!("[{}; {}]", self.render_ty(elem), n),
            Ty::RawPtr(inner) => format!("*{}", self.render_ty(inner)),
            Ty::Slice(inner) => format!("{}[]", self.render_ty(inner)),
            Ty::FnPtr {
                params,
                return_type,
            } => {
                let ps = params
                    .iter()
                    .map(|p| self.render_ty(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                if matches!(**return_type, Ty::Unit) {
                    format!("fn({ps})")
                } else {
                    format!("fn({ps}) -> {}", self.render_ty(return_type))
                }
            }
            other => ty_display(other),
        }
    }

    fn render_nominal(&self, name: &str, generic_origin: &Option<(String, Vec<Ty>)>) -> String {
        match generic_origin {
            Some((tmpl, args)) if !args.is_empty() => {
                let args_s = args
                    .iter()
                    .map(|a| self.render_ty(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}[{}]", name_leaf(tmpl), args_s)
            }
            _ => name_leaf(name).to_string(),
        }
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
        if args
            .iter()
            .any(|a| ty_contains_param(a, &self.structs, &self.enums))
        {
            return;
        }
        for (i, arg_ty) in args.iter().enumerate() {
            let Some(param_bounds) = bounds.get(i) else {
                continue;
            };
            for b in param_bounds {
                if !self.satisfies_bound(arg_ty, b) {
                    let pname = param_names.get(i).map(|s| s.as_str()).unwrap_or("?");
                    self.err(
                        "E0502",
                        format!(
                            "type `{}` does not satisfy bound `{}` on type parameter `{}` of {}",
                            ty_display(arg_ty),
                            b,
                            pname,
                            context_desc
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
                self.interface_impls
                    .contains(&(bound.to_string(), name.clone()))
            }
            Ty::Enum(id) => {
                let name = &self.enums[id.0 as usize].name;
                self.interface_impls
                    .contains(&(bound.to_string(), name.clone()))
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
            let ItemKind::Enum(e) = &item.kind else {
                continue;
            };
            // Slice 7GEN.5d: generic enum templates have no concrete
            // payloads until instantiation. Skip.
            if !e.generic_params.is_empty() {
                continue;
            }
            let Some(&id) = self.enum_by_name.get(&e.name.name) else {
                continue;
            };
            // Slice 7GEN.4: generic-param names declared on the enum
            // (`enum Option[T]`) are visible in variant payload types.
            self.push_type_params(&e.generic_params);
            // Walk source variants in declaration order; sema's
            // EnumVariantDef list mirrors the source list (modulo
            // duplicates which were skipped in step 1).
            let mut sema_idx = 0usize;
            for sv in &e.variants {
                if sema_idx >= self.enums[id.0 as usize].variants.len() {
                    break;
                }
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
    /// Copy iff every variant's payload type is Copy. (v0.0.14: owning payloads
    /// are allowed — they make the enum non-Copy and drop-carrying, dropped via
    /// synthesized enum-variant drop; the old E0344 ban is gone.)
    fn compute_enum_copy_flags(&mut self, _p: &Program) {
        // v0.0.14: the former "no Drop payload in a tagged enum" rule (E0344)
        // is removed — tagged enums with owning payloads are now supported via
        // synthesized enum-variant drop (codegen switches on the tag and tears
        // down the active payload). The Copy fixpoint below still makes such an
        // enum non-Copy, since a non-Copy payload makes the enum non-Copy.
        //
        // Compute Copy flag. Fixpoint, monotone.
        loop {
            let mut changed = false;
            for i in 0..self.enums.len() {
                if self.enums[i].is_copy {
                    continue;
                }
                let copy_now = if !self.enums[i].is_tagged {
                    true // plain enum — always Copy
                } else {
                    let all_payloads_copy = self.enums[i]
                        .variants
                        .iter()
                        .all(|v| v.payload.iter().all(|t| self.is_copy(t)));
                    all_payloads_copy
                };
                if copy_now {
                    self.enums[i].is_copy = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// True iff `ty` itself carries a destructor (its scope-exit drop is
    /// non-trivial). Mirror of `is_copy` for the Drop side. Used by the
    /// tagged-enum payload rule (§3.3 of the design note).
    fn ty_carries_drop(&self, ty: &Ty) -> bool {
        match ty {
            // v0.0.14 auto field-drop: `string` owns a heap buffer.
            Ty::String => true,
            // A struct carries drop if it has an explicit `drop` OR any field
            // is itself drop-carrying (transitive auto field-drop). Cycle-safe:
            // by-value struct containment is acyclic (infinite-size structs are
            // rejected), and Vec/Box break recursion via raw-pointer fields.
            Ty::Struct(id) => {
                let def = &self.structs[id.0 as usize];
                def.is_drop || def.fields.iter().any(|f| self.ty_carries_drop(&f.1))
            }
            // v0.0.14 enum-variant drop: a tagged enum carries drop if any
            // variant payload does. Cycle-safe for the same reason as structs.
            Ty::Enum(id) => {
                let def = &self.enums[id.0 as usize];
                def.is_tagged
                    && def
                        .variants
                        .iter()
                        .any(|v| v.payload.iter().any(|t| self.ty_carries_drop(t)))
            }
            Ty::Array(elem, _) => self.ty_carries_drop(elem),
            _ => false,
        }
    }

    /// v0.0.14 `#[no_alloc]` drop-glue: record the leaf names of types whose
    /// `drop` method carries `#[no_alloc]`/`#[realtime]`. Such a `drop` body
    /// has already been verified non-allocating by `check_no_alloc`, so
    /// running it implicitly at scope exit is allowed inside a `#[no_alloc]`
    /// function.
    fn collect_no_alloc_drop_types(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            // Inherent impls only carry `drop`; an interface impl never does.
            if b.interface_name.is_some() {
                continue;
            }
            for m in &b.methods {
                if m.name.name == "drop" && marks_no_alloc(&m.attributes) {
                    self.no_alloc_drop_types
                        .insert(name_leaf(&b.target.name).to_string());
                }
            }
        }
    }

    /// v0.0.14 `#[no_alloc]` drop-glue: is the scope-exit teardown of `ty`
    /// non-allocating? `string` (and any `Vec`/`Box`-style type) frees its
    /// buffer, so it is never safe. A struct with a user `drop` is safe only
    /// if that `drop` is `#[no_alloc]` (its body verified by `check_no_alloc`);
    /// then its auto-dropped fields must each be safe too. Enums recurse into
    /// payloads; arrays into elements. Non-drop types are trivially safe.
    fn no_alloc_safe_drop(&self, ty: &Ty) -> bool {
        match ty {
            Ty::String => false,
            Ty::Struct(id) => {
                let def = &self.structs[id.0 as usize];
                if def.is_drop && !self.no_alloc_drop_types.contains(name_leaf(&def.name)) {
                    return false;
                }
                let fields = def.fields.clone();
                fields.iter().all(|(_, fty, _)| self.no_alloc_safe_drop(fty))
            }
            Ty::Enum(id) => {
                let variants = self.enums[id.0 as usize].variants.clone();
                variants
                    .iter()
                    .all(|v| v.payload.iter().all(|t| self.no_alloc_safe_drop(t)))
            }
            Ty::Array(elem, _) => self.no_alloc_safe_drop(elem),
            _ => true,
        }
    }

    /// v0.0.15 `#[no_alloc]` drop-glue, parameter arm: does an owned parameter
    /// of this `(marker, ty)` run a destructor in *this* function at scope exit?
    /// Mirrors codegen's callee-side drop rule (`effective_move` + the
    /// Struct/Enum `register_drop` path in `gen_function`):
    ///   - **Owned** means `move x: T`, or a bare `x: T` whose non-Copy struct
    ///     type is move-by-default (the v0.0.10 rule). `borrow x` / `mut x` are
    ///     caller-owned — no callee teardown.
    ///   - Of the owned params, codegen only emits a callee drop for a
    ///     `Struct`/`Enum` aggregate. A `string`/`Vec[T]` *value* param is
    ///     caller-dropped via the auto-clone-on-return safety net
    ///     (`borrowed_params`), so the callee frees nothing — `Ty::String`
    ///     therefore returns `false` here even under `move`.
    /// Returning `true` means this parameter's implicit teardown is the
    /// callee's, so a `#[no_alloc]` function must reject it when that teardown
    /// allocates (`!no_alloc_safe_drop`).
    fn no_alloc_param_drops_here(&self, param: &Param, ty: &Ty) -> bool {
        let owned = param.move_
            || (!param.mutable
                && !param.borrow_
                && matches!(ty, Ty::Struct(_))
                && !self.is_copy(ty));
        owned && matches!(ty, Ty::Struct(_) | Ty::Enum(_))
    }

    /// v0.0.15 `#[no_alloc]` drop-glue, temporary arm: does this expression
    /// denote *existing* storage (a place) rather than producing a fresh owned
    /// temporary? A place — a binding, a field/index projection of a place, or
    /// a pointer deref — is owned by something else and isn't torn down when its
    /// value is discarded. Anything else (a call, constructor, struct literal,
    /// arithmetic, …) materializes a new value, so discarding it as an
    /// expression statement drops that temporary at statement end. The receiver
    /// chain is followed so `make_struct().field` (a field of a *call* result)
    /// is correctly classified as a temporary, while `obj.field` is a place.
    fn expr_is_place(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Ident(_) => true,
            ExprKind::Field { receiver, .. } => self.expr_is_place(receiver),
            ExprKind::Index { receiver, .. } => self.expr_is_place(receiver),
            ExprKind::Unary {
                op: UnaryOp::Deref,
                ..
            } => true,
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
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            if !b.target_generic_params.is_empty() {
                self.collect_generic_impl_methods(b);
            }
        }
        self.current_file = None;
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            // Generic impls were handled in phase 1 above.
            if !b.target_generic_params.is_empty() {
                continue;
            }
            // Slice 7GEN.4: skip impls whose target is an interface — those
            // are handled by `validate_interface_impls`. Inherent impls
            // (`impl Type { ... }`) still flow through this pass.
            let Some(&id) = self.struct_by_name.get(&b.target.name) else {
                // v0.0.5 Phase 2C: enum impls now collect into
                // `EnumDef::methods`. Generic enum impls still pending
                // (block-level generic_params already routed above).
                if let Some(&enum_id) = self.enum_by_name.get(&b.target.name) {
                    self.collect_enum_impl_methods(enum_id, b);
                    continue;
                }
                self.err(
                    "E0325",
                    format!("`impl` target `{}` is not a known type", b.target.name),
                    b.target.span,
                );
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
                for gp in &m.generic_params {
                    mscope.insert(gp.name.name.clone());
                }
                self.type_params_stack.push(mscope);
                let params: Vec<ParamSig> = m
                    .params
                    .iter()
                    .map(|p| ParamSig {
                        ty: self.resolve_type(&p.ty),
                        mutable: p.mutable,
                        move_: p.move_,
                        borrow_: p.borrow_ || matches!(p.ty.kind, TypeKind::Borrowed { .. }),
                    })
                    .collect();
                let declared_ret = match &m.return_type {
                    Some(t) => self.resolve_type(t),
                    None => Ty::Unit,
                };
                // v0.0.5 Phase 2B: `gen fn` methods expose `Iterator[T]`
                // at the call site (mirror of `gen fn` free fns).
                // v0.0.5 Phase 4 Slice 4B: same wrap for `async fn`
                // methods — callers see `Future[T]`, body sees T.
                let return_type = if m.is_gen {
                    self.wrap_in_iterator(&declared_ret, m.name.span)
                } else if m.is_async {
                    self.wrap_in_future(&declared_ret, m.name.span)
                } else {
                    declared_ret
                };
                self.type_params_stack.pop();
                if self.structs[id.0 as usize]
                    .methods
                    .contains_key(&m.name.name)
                {
                    self.err(
                        "E0326",
                        format!(
                            "duplicate method `{}` in impl `{}`",
                            m.name.name, b.target.name
                        ),
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
                let generic_params: Vec<String> = m
                    .generic_params
                    .iter()
                    .map(|gp| gp.name.name.clone())
                    .collect();
                let generic_bounds: Vec<Vec<String>> = m
                    .generic_params
                    .iter()
                    .map(|gp| gp.bounds.iter().map(|b| b.name.clone()).collect())
                    .collect();
                self.structs[id.0 as usize].methods.insert(
                    m.name.name.clone(),
                    MethodSig {
                        receiver: m.receiver,
                        params,
                        return_type,
                        generic_params,
                        generic_bounds,
                    },
                );
            }
            self.self_type_stack.pop();
        }
        self.current_file = None;
        self.backfill_generic_struct_methods();
    }

    /// G-022 fix: backfill methods on generic struct instantiations that
    /// were synthesized by `instantiate_struct_from_arg_tys` *before*
    /// their `impl Vec[T] { ... }` block templates were registered in
    /// `generic_impl_methods`. This window opens any time a struct field
    /// type names a cross-package generic instantiation — `collect_struct_fields`
    /// runs before `collect_methods`, so the early instantiation's
    /// `methods` table is left empty and later method calls on that
    /// concrete type fire E0324 even though sema has the impl block.
    ///
    /// Repro that this closes:
    /// ```cplus
    /// // vendor/inner/src/inner.cplus
    /// import "stdlib/hash_map" as map;
    /// struct Holder { m: map::HashMap[i32, i32] }   // early instantiation
    /// pub fn touch() -> bool {
    ///     let mut h: map::HashMap[i32, i32] = map::new::[i32, i32]();
    ///     h.insert(1 as i32, 2 as i32);              // <- E0324 before fix
    ///     return h.contains_key(1 as i32);
    /// }
    /// ```
    fn backfill_generic_struct_methods(&mut self) {
        let to_backfill: Vec<(StructId, String, Vec<Ty>)> = self
            .structs
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| {
                if !s.methods.is_empty() {
                    return None;
                }
                let (name, args) = s.generic_origin.as_ref()?;
                if !self.generic_impl_methods.contains_key(name) {
                    return None;
                }
                Some((StructId(idx as u32), name.clone(), args.clone()))
            })
            .collect();
        for (id, name, arg_tys) in to_backfill {
            let templates = match self.generic_impl_methods.get(&name).cloned() {
                Some(t) => t,
                None => continue,
            };
            let self_ty = Ty::Struct(id);
            for t in &templates {
                let mut method_subst: HashMap<String, Ty> = HashMap::new();
                for (gp, arg) in t.impl_generic_params.iter().zip(arg_tys.iter()) {
                    method_subst.insert(gp.clone(), arg.clone());
                }
                let resolved_params: Vec<ParamSig> = {
                    let raw: Vec<(Ty, bool, bool, bool)> = t
                        .params
                        .iter()
                        .map(|p| (p.ty.clone(), p.mutable, p.move_, p.borrow_))
                        .collect();
                    raw.into_iter()
                        .map(|(ty, mutable, move_, borrow_)| {
                            let s = self.subst_ty_deep(&ty, &method_subst);
                            ParamSig {
                                ty: subst_self(&s, &self_ty),
                                mutable,
                                move_,
                                borrow_,
                            }
                        })
                        .collect()
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
                        generic_params: t.method_generic_params.clone(),
                        generic_bounds: Vec::new(),
                    },
                );
            }
        }
    }

    /// v0.0.5 Phase 2C: collect methods from `impl EnumName { fn ... }`.
    /// Mirror of the struct path in `collect_methods` — sigs go into
    /// `EnumDef::methods`, `Self` resolves to `Ty::Enum(enum_id)`, no
    /// destructor detection (enums don't carry Drop yet; relaxes when
    /// the no-Drop-payload rule does).
    fn collect_enum_impl_methods(&mut self, enum_id: EnumId, b: &ImplBlock) {
        self.self_type_stack.push(Ty::Enum(enum_id));
        for m in &b.methods {
            let mut mscope = std::collections::HashSet::new();
            for gp in &m.generic_params {
                mscope.insert(gp.name.name.clone());
            }
            self.type_params_stack.push(mscope);
            let params: Vec<ParamSig> = m
                .params
                .iter()
                .map(|p| ParamSig {
                    ty: self.resolve_type(&p.ty),
                    mutable: p.mutable,
                    move_: p.move_,
                    borrow_: p.borrow_ || matches!(p.ty.kind, TypeKind::Borrowed { .. }),
                })
                .collect();
            let declared_ret = match &m.return_type {
                Some(t) => self.resolve_type(t),
                None => Ty::Unit,
            };
            let return_type = if m.is_gen {
                self.wrap_in_iterator(&declared_ret, m.name.span)
            } else if m.is_async {
                self.wrap_in_future(&declared_ret, m.name.span)
            } else {
                declared_ret
            };
            self.type_params_stack.pop();
            if self.enums[enum_id.0 as usize]
                .methods
                .contains_key(&m.name.name)
            {
                self.err(
                    "E0326",
                    format!(
                        "duplicate method `{}` in impl `{}`",
                        m.name.name, b.target.name
                    ),
                    m.name.span,
                );
                continue;
            }
            // Reject a *user-written* `drop` on enums (E0338): v0.0.14 tears
            // down owning enum payloads via compiler-synthesized enum-variant
            // drop, so a hand-written enum destructor is unnecessary and
            // unsupported.
            if m.name.name == "drop" {
                self.err(
                    "E0338",
                    format!(
                        "destructor methods on enums are not yet supported (`impl {}::drop`)",
                        b.target.name
                    ),
                    m.name.span,
                );
                continue;
            }
            let generic_params: Vec<String> = m
                .generic_params
                .iter()
                .map(|gp| gp.name.name.clone())
                .collect();
            let generic_bounds: Vec<Vec<String>> = m
                .generic_params
                .iter()
                .map(|gp| gp.bounds.iter().map(|b| b.name.clone()).collect())
                .collect();
            self.enums[enum_id.0 as usize].methods.insert(
                m.name.name.clone(),
                MethodSig {
                    receiver: m.receiver,
                    params,
                    return_type,
                    generic_params,
                    generic_bounds,
                },
            );
        }
        self.self_type_stack.pop();
    }

    /// Slice 7GEN.5e step 3: route methods declared inside a generic-typed
    /// impl block (`impl Vec[T] { ... }`) into `generic_impl_methods`.
    /// They are materialized as concrete `MethodSig`s by
    /// `populate_generic_impl_methods` whenever the template is
    /// instantiated (`Vec[i32]`, `Vec[bool]`, ...).
    fn collect_generic_impl_methods(&mut self, b: &ImplBlock) {
        // v0.0.5: accept impl targets that are either generic structs
        // OR generic enums (the latter previously rejected with E0325).
        // Both kinds funnel into the same `generic_impl_methods` table
        // (keyed by template name — names are unique across the type
        // namespace, so no collision risk). The per-instantiation
        // method population is routed differently inside
        // `instantiate_struct_from_arg_tys` vs `instantiate_enum_from_arg_tys`.
        let is_struct = self.struct_generic_templates.contains_key(&b.target.name);
        let is_enum = self.enum_generic_templates.contains_key(&b.target.name);
        if !is_struct && !is_enum {
            self.err(
                "E0325",
                format!(
                    "`impl` target `{}` is not a known generic type",
                    b.target.name
                ),
                b.target.span,
            );
            return;
        }
        // Push impl-level generic params onto the type-param stack so
        // method param/return types can reference `T`.
        let impl_param_names: Vec<String> = b
            .target_generic_params
            .iter()
            .map(|g| g.name.name.clone())
            .collect();
        let mut impl_scope = std::collections::HashSet::new();
        for n in &impl_param_names {
            impl_scope.insert(n.clone());
        }
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
                    format!(
                        "duplicate method `{}` in impl `{}`",
                        m.name.name, b.target.name
                    ),
                    m.name.span,
                );
                continue;
            }
            // Method-level generic params.
            let method_param_names: Vec<String> = m
                .generic_params
                .iter()
                .map(|g| g.name.name.clone())
                .collect();
            let mut method_scope = std::collections::HashSet::new();
            for n in &method_param_names {
                method_scope.insert(n.clone());
            }
            self.type_params_stack.push(method_scope);
            let params: Vec<ParamSig> = m
                .params
                .iter()
                .map(|p| ParamSig {
                    ty: self.resolve_type(&p.ty),
                    mutable: p.mutable,
                    move_: p.move_,
                    borrow_: p.borrow_ || matches!(p.ty.kind, TypeKind::Borrowed { .. }),
                })
                .collect();
            let declared_ret = match &m.return_type {
                Some(t) => self.resolve_type(t),
                None => Ty::Unit,
            };
            // v0.0.5 Phase 2B: gen-method template — wrap T → Iterator[T]
            // at the signature level. Methods on generic structs preserve
            // the wrap through monomorphization (synthesize_generic_typed_impls
            // re-renders the return type via subst_type_ast).
            // v0.0.5 Phase 4 Slice 4B: same wrap for async methods.
            let return_type = if m.is_gen {
                self.wrap_in_iterator(&declared_ret, m.name.span)
            } else if m.is_async {
                self.wrap_in_future(&declared_ret, m.name.span)
            } else {
                declared_ret
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
            InterfaceDef {
                name: "Copy".to_string(),
                methods: HashMap::new(),
                origin_file: None,
            },
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
            InterfaceDef {
                name: "Send".to_string(),
                methods: HashMap::new(),
                origin_file: None,
            },
        );
        self.interfaces.insert(
            "Sync".to_string(),
            InterfaceDef {
                name: "Sync".to_string(),
                methods: HashMap::new(),
                origin_file: None,
            },
        );
        // Single-method interfaces with shared shape.
        // (name, method_name, return_type, takes_other_param)
        let single: &[(&str, &str, Ty, bool)] = &[
            ("Eq", "eq", Ty::Bool, true),
            ("Ord", "cmp", Ty::I32, true),
            ("Hash", "hash", Ty::U64, false),
            ("Clone", "clone", Ty::Param("Self".to_string()), false),
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
                vec![ParamSig {
                    ty: Ty::Param("Self".to_string()),
                    mutable: false,
                    move_: false,
                    borrow_: false,
                }]
            } else {
                Vec::new()
            };
            methods.insert(
                (*mname).to_string(),
                MethodSig {
                    receiver: Some(Receiver::Read),
                    params,
                    return_type: ret.clone(),
                    generic_params: Vec::new(),
                    generic_bounds: Vec::new(),
                },
            );
            self.interfaces.insert(
                (*name).to_string(),
                InterfaceDef {
                    name: (*name).to_string(),
                    methods,
                    origin_file: None,
                },
            );
        }
    }

    fn collect_interfaces(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Interface(idecl) = &item.kind else {
                continue;
            };
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
                let params: Vec<ParamSig> = m
                    .params
                    .iter()
                    .map(|p| ParamSig {
                        ty: self.resolve_type(&p.ty),
                        mutable: p.mutable,
                        move_: p.move_,
                        borrow_: p.borrow_ || matches!(p.ty.kind, TypeKind::Borrowed { .. }),
                    })
                    .collect();
                let return_type = match &m.return_type {
                    Some(t) => self.resolve_type(t),
                    None => Ty::Unit,
                };
                if methods.contains_key(&m.name.name) {
                    self.err(
                        "E0326",
                        format!(
                            "duplicate method `{}` in interface `{}`",
                            m.name.name, idecl.name.name
                        ),
                        m.name.span,
                    );
                    continue;
                }
                methods.insert(
                    m.name.name.clone(),
                    MethodSig {
                        receiver: m.receiver,
                        params,
                        return_type,
                        generic_params: Vec::new(),
                        generic_bounds: Vec::new(),
                    },
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
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            let Some(iface_name) = b.interface_name.as_ref() else {
                continue;
            };
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
            // v0.0.14: `Send` / `Sync` are unsafe marker overrides. They take
            // no methods (the body must be empty `{}`) and assert thread
            // safety the compiler can't prove, so the impl must carry
            // `unsafe`. A bare `impl Send for T {}` is E0860. The override is
            // recorded for `is_send`/`is_sync`; a conditional impl
            // (`unsafe impl Send for Arc[T: Send + Sync]`) records its
            // per-param bounds so the marker only holds when they're met.
            if iface_name.name == "Send" || iface_name.name == "Sync" {
                if !b.is_unsafe {
                    diags.push(Diag {
                        code: "E0860",
                        msg: format!(
                            "`{}` is an unsafe assertion — write `unsafe impl {} for {} {{}}` to vouch for thread safety the compiler can't verify",
                            iface_name.name, iface_name.name, b.target.name
                        ),
                        span: iface_name.span,
                        origin_file: item.origin_file.clone(),
                    });
                    continue;
                }
                if !b.methods.is_empty() {
                    diags.push(Diag {
                        code: "E0860",
                        msg: format!(
                            "`unsafe impl {} for {}` must have an empty body — `Send`/`Sync` are marker interfaces with no methods",
                            iface_name.name, b.target.name
                        ),
                        span: iface_name.span,
                        origin_file: item.origin_file.clone(),
                    });
                    continue;
                }
                let bounds: Vec<Vec<String>> = b
                    .target_generic_params
                    .iter()
                    .map(|gp| gp.bounds.iter().map(|bn| bn.name.clone()).collect())
                    .collect();
                // Key on the leaf so a qualified multi-file target name
                // (`vendor.stdlib.src.arc.Arc`) matches the instantiation's
                // template leaf (`Arc`) at the use site.
                self.marker_overrides.insert(
                    (iface_name.name.clone(), name_leaf(&b.target.name).to_string()),
                    bounds,
                );
                continue;
            }
            // `unsafe` is meaningful only on the `Send`/`Sync` markers.
            if b.is_unsafe {
                diags.push(Diag {
                    code: "E0861",
                    msg: format!(
                        "`unsafe impl` applies only to the `Send` / `Sync` markers, not `{}`",
                        iface_name.name
                    ),
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
                    msg: format!(
                        "`impl {} for {}` — `{}` is not a known struct",
                        iface_name.name, b.target.name, b.target.name
                    ),
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
                    let span = b
                        .methods
                        .iter()
                        .next()
                        .map(|m| m.name.span)
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
                    let span = b
                        .methods
                        .iter()
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
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            self.current_file = item.origin_file.clone();
            // Slice 4C: per-item context. Methods inherit their impl
            // block's origin_file — every impl block lives in the same
            // file as its type (enforced by the resolver).
            if let Some(&id) = self.struct_by_name.get(&b.target.name) {
                for m in &b.methods {
                    self.check_method(id, m);
                }
                continue;
            }
            // v0.0.5 Phase 2C: enum impl bodies.
            if let Some(&enum_id) = self.enum_by_name.get(&b.target.name) {
                for m in &b.methods {
                    self.check_enum_method(enum_id, m);
                }
                continue;
            }
        }
        self.current_file = None;
    }

    /// v0.0.5 Phase 2C: type-check the body of a method declared inside
    /// `impl EnumName { fn ... }`. Mirror of `check_method` for structs;
    /// `Self` resolves to `Ty::Enum(enum_id)` and receiver bindings
    /// take the enum's type.
    fn check_enum_method(&mut self, enum_id: EnumId, m: &Method) {
        let Some(sig) = self.enums[enum_id.0 as usize]
            .methods
            .get(&m.name.name)
            .cloned()
        else {
            return;
        };
        self.self_type_stack.push(Ty::Enum(enum_id));
        self.push_type_params(&m.generic_params);
        let body_return = if m.is_gen {
            Ty::Unit
        } else if m.is_async {
            self.unwrap_future(&sig.return_type)
                .unwrap_or_else(|| sig.return_type.clone())
        } else {
            sig.return_type.clone()
        };
        self.current_return = body_return;
        let prev_gen = self.current_fn_is_gen;
        let prev_gen_ty = self.current_gen_yield_ty.clone();
        self.current_fn_is_gen = m.is_gen;
        self.current_gen_yield_ty = if m.is_gen {
            self.unwrap_iterator(&sig.return_type)
        } else {
            None
        };
        let prev_async = self.current_fn_is_async;
        self.current_fn_is_async = m.is_async;
        self.scopes.push(HashMap::new());
        if let Some(rcv) = sig.receiver {
            let mutable = matches!(rcv, Receiver::Mut);
            self.scopes.last_mut().unwrap().insert(
                "self".to_string(),
                LocalInfo {
                    ty: Ty::Enum(enum_id),
                    mutable,
                    moved: false,
                    assigned: true,
                    borrow_roots: BTreeSet::new(),
                },
            );
        }
        for (param, psig) in m.params.iter().zip(sig.params.iter()) {
            if param.mutable && param.move_ {
                self.err("E0334",
                    "parameter cannot have both `mut` and `move`; these markers are mutually exclusive".to_string(),
                    param.span);
            }
            // v0.0.9 follow-up: `borrow` is mutually exclusive with
            // both `move` (ownership-transfer vs shared) and `mut`
            // (exclusive borrow vs shared). Reuses E0334 — same shape
            // of category error.
            if param.borrow_ && param.move_ {
                self.err("E0334",
                    "parameter cannot have both `borrow` and `move`; `borrow` is shared by-value, `move` transfers ownership".to_string(),
                    param.span);
            }
            if param.borrow_ && param.mutable {
                self.err("E0334",
                    "parameter cannot have both `borrow` and `mut`; `borrow` is shared by-value, `mut` is an exclusive borrow".to_string(),
                    param.span);
            }
            // v0.0.8 post-bench-gap: `restrict` is only valid on raw
            // pointer (`*T`) parameters. The borrow checker doesn't
            // reason about raw pointers; `restrict` is an opt-in
            // `noalias` assertion the programmer makes about the
            // pointer's relationship to other reachable pointers.
            // Putting it on a struct / primitive / aggregate param is a
            // category error.
            if param.restrict && !matches!(psig.ty, Ty::RawPtr(_)) {
                self.err(
                    "E0411",
                    "`restrict` is only valid on raw pointer (`*T`) parameters".to_string(),
                    param.span,
                );
            }
            self.scopes.last_mut().unwrap().insert(
                param.name.name.clone(),
                LocalInfo {
                    ty: psig.ty.clone(),
                    mutable: param.mutable,
                    moved: false,
                    assigned: true,
                    borrow_roots: BTreeSet::new(),
                },
            );
        }
        self.setup_returned_borrow_ctx(&m.params, &m.return_type, m.receiver.is_some());
        self.check_function_body(
            &m.body,
            self.current_return.clone(),
            m.body.span,
            marks_no_alloc(&m.attributes),
            marks_no_block(&m.attributes),
            has_attr_named(&m.attributes, "naked"),
        );
        self.scopes.pop();
        self.current_fn_is_gen = prev_gen;
        self.current_gen_yield_ty = prev_gen_ty;
        self.current_fn_is_async = prev_async;
        self.pop_type_params();
        self.self_type_stack.pop();
    }

    fn check_method(&mut self, struct_id: StructId, m: &Method) {
        let Some(sig) = self.structs[struct_id.0 as usize]
            .methods
            .get(&m.name.name)
            .cloned()
        else {
            return;
        };
        // Slice 7GEN.4: re-push the impl's `Self` mapping so `Self`
        // references in the method body resolve to the target type.
        self.self_type_stack.push(Ty::Struct(struct_id));
        // Slice 7GEN.5e: re-push method-level generic params for
        // body checking so `T` references in the body resolve.
        self.push_type_params(&m.generic_params);
        // v0.0.5 Phase 2B: gen methods (e.g. `pub gen fn iter(self) -> T`)
        // expose `Iterator[T]` at the signature level; the body sees the
        // unwrapped element type T and uses `yield V;` to produce values.
        // Mirror `check_function`'s state-threading for `current_fn_is_gen`
        // and `current_gen_yield_ty` so `yield` checks fire correctly
        // inside method bodies.
        // v0.0.5 Phase 4 Slice 4B: same shape for async methods —
        // body sees the inner T, callers see Future[T]; `current_fn_is_async`
        // gates the body's `await` checks.
        let body_return = if m.is_gen {
            Ty::Unit
        } else if m.is_async {
            self.unwrap_future(&sig.return_type)
                .unwrap_or_else(|| sig.return_type.clone())
        } else {
            sig.return_type.clone()
        };
        self.current_return = body_return;
        let prev_gen = self.current_fn_is_gen;
        let prev_gen_ty = self.current_gen_yield_ty.clone();
        self.current_fn_is_gen = m.is_gen;
        self.current_gen_yield_ty = if m.is_gen {
            self.unwrap_iterator(&sig.return_type)
        } else {
            None
        };
        let prev_async = self.current_fn_is_async;
        self.current_fn_is_async = m.is_async;
        let fn_no_alloc = marks_no_alloc(&m.attributes);
        self.scopes.push(HashMap::new());

        // v0.0.15 `#[no_alloc]` drop-glue, receiver arm: a `move self` receiver
        // is consumed by the method, so codegen registers a scope-exit drop for
        // it (mirrors `gen_function`'s self path) — unless this *is* the
        // destructor, where self is being torn down already. An allocating
        // teardown of an owned `self` violates `#[no_alloc]`.
        if fn_no_alloc {
            if let Some(Receiver::Move) = sig.receiver {
                let self_ty = Ty::Struct(struct_id);
                if m.name.name != "drop" && !self.no_alloc_safe_drop(&self_ty) {
                    self.err(
                        "E0901",
                        format!(
                            "`#[no_alloc]` method: `move self` receiver of type `{}` runs an allocating destructor at scope exit (its `drop` frees heap or is not marked `#[no_alloc]`)",
                            ty_display(&self_ty),
                        ),
                        m.name.span,
                    );
                }
            }
        }

        // Register `self` if there's a receiver. `mut self` makes self
        // a mutable binding (enables `self.x = ...`); other forms don't.
        // `move self` is read-only inside the body — consumption happens at
        // the call site, not from within.
        if let Some(rcv) = sig.receiver {
            let mutable = matches!(rcv, Receiver::Mut);
            self.scopes.last_mut().unwrap().insert(
                "self".to_string(),
                LocalInfo {
                    ty: Ty::Struct(struct_id),
                    mutable,
                    moved: false,
                    assigned: true,
                    borrow_roots: BTreeSet::new(),
                },
            );
        }
        // Register non-receiver params.
        for (param, psig) in m.params.iter().zip(sig.params.iter()) {
            // v0.0.15 `#[no_alloc]` drop-glue, parameter arm (see
            // `check_function` for the rationale and `no_alloc_param_drops_here`
            // for the ownership rule).
            if fn_no_alloc
                && self.no_alloc_param_drops_here(param, &psig.ty)
                && !self.no_alloc_safe_drop(&psig.ty)
            {
                self.err(
                    "E0901",
                    format!(
                        "`#[no_alloc]` method: owned parameter `{}` of type `{}` runs an allocating destructor at scope exit (its `drop` frees heap or is not marked `#[no_alloc]`)",
                        param.name.name,
                        ty_display(&psig.ty),
                    ),
                    param.span,
                );
            }
            // E0334: `mut` and `move` are mutually exclusive ownership markers.
            if param.mutable && param.move_ {
                self.err(
                    "E0334",
                    "parameter cannot have both `mut` and `move`; these markers are mutually exclusive".to_string(),
                    param.span,
                );
            }
            // E0411: `restrict` only applies to raw pointer params.
            if param.restrict && !matches!(psig.ty, Ty::RawPtr(_)) {
                self.err(
                    "E0411",
                    "`restrict` is only valid on raw pointer (`*T`) parameters".to_string(),
                    param.span,
                );
            }
            self.scopes.last_mut().unwrap().insert(
                param.name.name.clone(),
                LocalInfo {
                    ty: psig.ty.clone(),
                    mutable: param.mutable,
                    moved: false,
                    assigned: true,
                    borrow_roots: BTreeSet::new(),
                },
            );
        }
        self.setup_returned_borrow_ctx(&m.params, &m.return_type, m.receiver.is_some());
        self.check_function_body(
            &m.body,
            self.current_return.clone(),
            m.body.span,
            marks_no_alloc(&m.attributes),
            marks_no_block(&m.attributes),
            has_attr_named(&m.attributes, "naked"),
        );
        self.scopes.pop();
        self.current_fn_is_gen = prev_gen;
        self.current_gen_yield_ty = prev_gen_ty;
        self.current_fn_is_async = prev_async;
        self.pop_type_params();
        self.self_type_stack.pop();
    }

    fn collect_functions(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Function(f) = &item.kind else {
                continue;
            };
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
                let attr_span = f
                    .attributes
                    .iter()
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
            // v0.0.5 Phase 1A: auto-promote non-Copy value params to `move`
            // semantics. A param `x: T` where T is non-Copy and there's no
            // explicit `mut` / `move` annotation behaves as if `move x: T`
            // was written — caller's binding marked moved at the call site,
            // callee owns. Closes the value-pass-without-`move` double-free
            // (`fn echo(x: string) -> string { return x; }` no longer
            // double-frees). Read-only callees on non-Copy types now also
            // consume the source — callers needing to retain ownership
            // clone explicitly. Explicit `mut` keeps its pointer-pass
            // semantics; explicit `move` is preserved verbatim.
            let params: Vec<ParamSig> = f
                .params
                .iter()
                .map(|p| ParamSig {
                    ty: self.resolve_type(&p.ty),
                    mutable: p.mutable,
                    move_: p.move_,
                    borrow_: p.borrow_ || matches!(p.ty.kind, TypeKind::Borrowed { .. }),
                })
                .collect();
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
            } else if f.is_gen {
                // v0.0.4 Phase 4 Slice 4A: `gen fn name() -> T` exposes
                // `Iterator[T]` at the signature level; the body still
                // type-checks `yield X` against the inner T.
                self.wrap_in_iterator(&declared_ret, f.name.span)
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
                        generic_params: f
                            .generic_params
                            .iter()
                            .map(|g| g.name.name.clone())
                            .collect(),
                        bounds: f
                            .generic_params
                            .iter()
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
            self.fns.insert(
                f.name.name.clone(),
                FnSig {
                    params,
                    return_type: ret,
                    is_variadic: f.is_variadic,
                    link_name: link_name.clone(),
                },
            );
        }
        self.current_file = None;
    }

    /// v0.0.3 Phase 5 Slice 5E.2: look up the `Future` template from
    /// the user's imported `stdlib/future` module and instantiate it
    /// with one type argument. Returns `Ty::Error` if the template
    /// isn't visible — usually because the project didn't import
    /// stdlib's future module or didn't depend on stdlib at all.
    /// v0.0.4 Phase 4 Slice 4A: mirror of `wrap_in_future` for `gen fn`.
    /// The `Iterator` template must be in scope — it lives at
    /// `stdlib/iterator.cplus`.
    fn wrap_in_iterator(&mut self, inner: &Ty, span: ByteSpan) -> Ty {
        let key = self
            .struct_generic_templates
            .keys()
            .find(|k| k.as_str() == "Iterator" || k.ends_with(".Iterator"))
            .cloned();
        let template_name = match key {
            Some(k) => k,
            None => {
                self.err(
                    "E1000",
                    "`gen fn` requires `Iterator[T]` from `stdlib/iterator`".to_string(),
                    span,
                );
                return Ty::Error;
            }
        };
        let template = self
            .struct_generic_templates
            .get(&template_name)
            .cloned()
            .unwrap();
        self.instantiate_struct_from_arg_tys(&template_name, &template, vec![inner.clone()])
    }

    /// v0.0.4 Phase 4 Slice 4B: instantiate the `Option[T]` enum from
    /// `stdlib/option`. Used by `Iterator::next()`'s blessed-method
    /// return type and by `for x in ...` desugaring.
    fn instantiate_option(&mut self, inner: &Ty, span: ByteSpan) -> Ty {
        let key = self
            .enum_generic_templates
            .keys()
            .find(|k| k.as_str() == "Option" || k.ends_with(".Option"))
            .cloned();
        let template_name = match key {
            Some(k) => k,
            None => {
                self.err(
                    "E1000",
                    "`Iterator::next` requires `Option[T]` from `stdlib/option`".to_string(),
                    span,
                );
                return Ty::Error;
            }
        };
        let template = self
            .enum_generic_templates
            .get(&template_name)
            .cloned()
            .unwrap();
        self.instantiate_enum_from_arg_tys(&template_name, &template, vec![inner.clone()])
    }

    /// v0.0.4 Phase 4 Slice 4A: given `Iterator[T]`, return T.
    fn unwrap_iterator(&self, ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Struct(id) => {
                let def = &self.structs[id.0 as usize];
                match &def.generic_origin {
                    Some((name, args))
                        if (name == "Iterator" || name.ends_with(".Iterator"))
                            && args.len() == 1 =>
                    {
                        Some(args[0].clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn wrap_in_future(&mut self, inner: &Ty, span: ByteSpan) -> Ty {
        // v0.0.3 Phase 5 Slice 5E.3: the resolver qualifies struct
        // names per-file (`<file_id>.Future`), so a bare-name lookup
        // misses imports. Suffix-match `.Future` (or the bare name in
        // single-file builds) like Slice 5B's JoinHandle path.
        let key = self
            .struct_generic_templates
            .keys()
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
        let template = self
            .struct_generic_templates
            .get(&template_name)
            .cloned()
            .unwrap();
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
                        if (name == "Future" || name.ends_with(".Future")) && args.len() == 1 =>
                    {
                        Some(args[0].clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// v0.0.9 Phase 4: collect module-scope `const` and `static` items
    /// into sema's tables and type-check each initializer against the
    /// declared type.
    ///
    /// `const` items are kept here only to gate name collisions against
    /// functions (E0301) and to type-check their initializers — lower
    /// already substituted their use sites away, and sema never looks
    /// them up via `resolve_value_ident`.
    ///
    /// `static` items land in `statics_table` so `resolve_value_ident`
    /// can surface them as values at use sites. Codegen reads the
    /// snapshot from `MonoInfo::statics` (built at `check_multi_with_mono`
    /// exit) and emits one LLVM global per entry.
    fn collect_consts_and_statics(&mut self, p: &Program) {
        // Track every value-level name in this pass so a `const FOO`
        // followed by a `static FOO` (or vice versa) trips E0301.
        // Consts go here too even though we don't keep their bodies
        // in sema's tables — duplicate detection is the only thing
        // we need them for at this stage (their use sites were
        // already substituted in `lower::substitute_consts`).
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            match &item.kind {
                ItemKind::Const(c) => {
                    if self.fns.contains_key(&c.name.name) || seen.contains(&c.name.name) {
                        self.err(
                            "E0301",
                            format!("duplicate item definition `{}`", c.name.name),
                            c.name.span,
                        );
                        continue;
                    }
                    seen.insert(c.name.name.clone());
                    let declared = self.resolve_type(&c.ty);
                    // Type-check the initializer against the declared type.
                    // `check_expr` requires a current scope frame — push
                    // an empty one for the duration of the check, then
                    // pop. The initializer is a literal post-lower, so
                    // no local lookups occur in practice.
                    self.scopes.push(HashMap::new());
                    let _ = self.check_expr(&c.value, Some(declared));
                    self.scopes.pop();
                }
                ItemKind::Static(s) => {
                    if self.fns.contains_key(&s.name.name) || seen.contains(&s.name.name) {
                        self.err(
                            "E0301",
                            format!("duplicate item definition `{}`", s.name.name),
                            s.name.span,
                        );
                        continue;
                    }
                    seen.insert(s.name.name.clone());
                    let declared = self.resolve_type(&s.ty);
                    self.scopes.push(HashMap::new());
                    let _ = self.check_expr(&s.value, Some(declared.clone()));
                    self.scopes.pop();
                    self.statics_table.insert(
                        s.name.name.clone(),
                        StaticInfo {
                            ty: declared,
                            is_mut: s.is_mut,
                            init: s.value.clone(),
                            decl_span: s.name.span,
                        },
                    );
                }
                _ => {}
            }
        }
        self.current_file = None;
    }

    fn check_main_signature(&mut self, p: &Program) {
        let Some(sig) = self.fns.get("main").cloned() else {
            return;
        };
        let Some((no_params, span, origin)) = p.items.iter().find_map(|it| {
            let ItemKind::Function(f) = &it.kind else {
                return None;
            };
            (f.name.name == "main")
                .then(|| (f.params.is_empty(), f.name.span, it.origin_file.clone()))
        }) else {
            return;
        };
        self.current_file = origin;
        // If we already errored resolving the return type, don't pile on.
        if sig.return_type == Ty::Error {
            return;
        }
        if !no_params || sig.return_type != Ty::I32 {
            self.err(
                "E0309",
                "`main` must have signature `fn main() -> i32` in Phase 1".to_string(),
                span,
            );
        }
        self.current_file = None;
    }

    /// v0.0.5: targeted lint over generic-fn bodies catching ordered
    /// comparisons (`<` / `<=` / `>` / `>=`) on bare-param-typed idents.
    /// Generic bodies are NOT fully sema-checked at definition time (the
    /// codebase is built around lazy "check after monomorphization" — a
    /// real change there cascades through intrinsic dispatch + struct
    /// instantiation lookups, see the v0.0.5 false-start). This narrow
    /// AST walk closes the specific footgun documented at SKILL.md §2.6:
    /// `fn max[T: Ord](a: T, b: T) -> T { if a < b { ... } }` would
    /// otherwise silently fall through sema and produce an invalid
    /// `icmp slt %StructTy` at codegen, surfacing as a downstream LLVM
    /// rejection ("icmp requires integer operands") only when the user
    /// happened to instantiate with a non-numeric concrete type.
    ///
    /// Limitation: only catches bare-Ident-of-param operand shapes. A
    /// `let x = a; if x < b { ... }` body still skips this check. The
    /// canonical idiom — `a.cmp(b)` returning i32 — is what users should
    /// write; the diagnostic points there.
    fn lint_generic_fn_bodies(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Function(f) = &item.kind else {
                continue;
            };
            if f.generic_params.is_empty() {
                continue;
            }
            // Build the set of bare-param-typed parameter NAMES — these
            // are the idents that, when compared with `<`/etc., produce
            // the codegen failure.
            let mut param_typed: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for p in &f.params {
                if let TypeKind::Path(name) = &p.ty.kind {
                    if f.generic_params.iter().any(|g| g.name.name == *name) {
                        param_typed.insert(p.name.name.clone());
                    }
                }
            }
            if param_typed.is_empty() {
                continue;
            }
            self.walk_block_for_param_compare(&f.body, &param_typed);
        }
    }

    /// AST walker for `lint_generic_fn_bodies`. Visits every `Binary`
    /// expression in the block and emits E0302 when an ordered-comparison
    /// operand is a bare Ident that names a generic-param-typed
    /// parameter.
    fn walk_block_for_param_compare(
        &mut self,
        block: &Block,
        param_idents: &std::collections::HashSet<String>,
    ) {
        for stmt in &block.stmts {
            self.walk_stmt_for_param_compare(stmt, param_idents);
        }
        if let Some(tail) = &block.tail {
            self.walk_expr_for_param_compare(tail, param_idents);
        }
    }

    fn walk_stmt_for_param_compare(
        &mut self,
        stmt: &Stmt,
        param_idents: &std::collections::HashSet<String>,
    ) {
        match &stmt.kind {
            StmtKind::Let { init: Some(e), .. } => {
                self.walk_expr_for_param_compare(e, param_idents)
            }
            StmtKind::Let { init: None, .. } => {}
            StmtKind::Expr(e)
            | StmtKind::Return(Some(e))
            | StmtKind::Defer(e)
            | StmtKind::Assert(e) => self.walk_expr_for_param_compare(e, param_idents),
            StmtKind::While { cond, body, .. } => {
                self.walk_expr_for_param_compare(cond, param_idents);
                self.walk_block_for_param_compare(body, param_idents);
            }
            StmtKind::Loop(b, _) => self.walk_block_for_param_compare(b, param_idents),
            StmtKind::IfLet {
                scrutinee,
                body,
                else_body,
                ..
            } => {
                self.walk_expr_for_param_compare(scrutinee, param_idents);
                self.walk_block_for_param_compare(body, param_idents);
                if let Some(eb) = else_body {
                    self.walk_block_for_param_compare(eb, param_idents);
                }
            }
            StmtKind::WhileLet {
                scrutinee, body, ..
            } => {
                self.walk_expr_for_param_compare(scrutinee, param_idents);
                self.walk_block_for_param_compare(body, param_idents);
            }
            StmtKind::GuardLet {
                scrutinee,
                else_body,
                ..
            } => {
                self.walk_expr_for_param_compare(scrutinee, param_idents);
                self.walk_block_for_param_compare(else_body, param_idents);
            }
            StmtKind::For(_, _) | StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {
            }
        }
    }

    fn walk_expr_for_param_compare(
        &mut self,
        e: &Expr,
        param_idents: &std::collections::HashSet<String>,
    ) {
        match &e.kind {
            ExprKind::Binary { op, lhs, rhs } => {
                if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
                    let bad = |operand: &Expr| -> Option<String> {
                        if let ExprKind::Ident(n) = &operand.kind {
                            if param_idents.contains(n) {
                                return Some(n.clone());
                            }
                        }
                        None
                    };
                    if let Some(n) = bad(lhs).or_else(|| bad(rhs)) {
                        self.err(
                            "E0302",
                            format!(
                                "ordered comparison on generic-parameter binding `{}` is not supported; \
                                 use `{}.cmp(other)` (returns i32) and compare its result \
                                 — C+ has no operator overloading (§2.6)",
                                n, n
                            ),
                            e.span,
                        );
                    }
                }
                self.walk_expr_for_param_compare(lhs, param_idents);
                self.walk_expr_for_param_compare(rhs, param_idents);
            }
            ExprKind::Unary { operand, .. } => {
                self.walk_expr_for_param_compare(operand, param_idents)
            }
            ExprKind::Call { callee, args, .. } => {
                self.walk_expr_for_param_compare(callee, param_idents);
                for a in args {
                    self.walk_expr_for_param_compare(a, param_idents);
                }
            }
            ExprKind::Field { receiver, .. } => {
                self.walk_expr_for_param_compare(receiver, param_idents)
            }
            ExprKind::Index { receiver, index } => {
                self.walk_expr_for_param_compare(receiver, param_idents);
                self.walk_expr_for_param_compare(index, param_idents);
            }
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.walk_expr_for_param_compare(cond, param_idents);
                self.walk_block_for_param_compare(then, param_idents);
                if let Some(eb) = else_branch {
                    self.walk_expr_for_param_compare(eb, param_idents);
                }
            }
            ExprKind::Block(b) => self.walk_block_for_param_compare(b, param_idents),
            ExprKind::Unsafe(b) => self.walk_block_for_param_compare(b, param_idents),
            ExprKind::Match { scrutinee, arms } => {
                self.walk_expr_for_param_compare(scrutinee, param_idents);
                for arm in arms {
                    self.walk_expr_for_param_compare(&arm.body, param_idents);
                }
            }
            ExprKind::Assign { target, value, .. } => {
                self.walk_expr_for_param_compare(target, param_idents);
                self.walk_expr_for_param_compare(value, param_idents);
            }
            ExprKind::Cast { expr, .. } => self.walk_expr_for_param_compare(expr, param_idents),
            ExprKind::Await(inner) | ExprKind::Yield(inner) => {
                self.walk_expr_for_param_compare(inner, param_idents)
            }
            _ => {}
        }
    }

    fn check_functions(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Function(f) = &item.kind else {
                continue;
            };
            // Slice 4C: per-item context for field-pub gate.
            self.current_file = item.origin_file.clone();
            self.check_function(f);
        }
        self.current_file = None;
    }

    // ========================================================================
    // v0.0.10 Phase 1 + Phase 3: `#[no_alloc]` / `#[bounded_recursion]` passes
    // ========================================================================
    //
    // Both attributes share the same machinery: build a name → AST-function
    // lookup, then walk every marked function's body and apply per-attribute
    // rules. `#[no_alloc]` rejects calls into the libc allocator blocklist
    // (or into user fns not themselves marked); `#[bounded_recursion]`
    // rejects any path that leads back to the marked fn itself.
    //
    // The walker resolves callees from the AST — `Ident(name)` and
    // `Path { segments }` cases only. Field-method calls (`recv.method(...)`)
    // are skipped: resolving them needs full type-dispatch info that we
    // don't conservatively expose here. In practice this is fine because
    // the only way to construct an allocating receiver (Vec / String / Box /
    // HashMap) is through a free or assoc fn that the walker *does* see —
    // so the call to e.g. `vec::new()` already fires E0901.

    /// v0.0.12 realtime Phase 1 (method-dispatch hole): record every
    /// `impl` method's `#[no_alloc]` / `#[no_block]` status, keyed by
    /// `(source-level target type name, method name)`. Built from the raw
    /// AST so the recorded verdict reflects each method's actual attributes
    /// — independent of any `[profile.realtime]` injection, which only
    /// touches local functions. Generic impls (`impl Vec[T]`) key under the
    /// template name (`"Vec"`); a call on an instantiation maps back via the
    /// struct's `generic_origin` in `source_type_name`.
    fn collect_method_contracts(&mut self, p: &Program) {
        for item in &p.items {
            let ItemKind::Impl(b) = &item.kind else {
                continue;
            };
            for m in &b.methods {
                self.method_contracts.insert(
                    (b.target.name.clone(), m.name.name.clone()),
                    (marks_no_alloc(&m.attributes), marks_no_block(&m.attributes)),
                );
            }
        }
    }

    /// Source-level type name for a resolved `Ty`, used to look up method
    /// contracts. Generic instantiations (whose `StructDef`/`EnumDef` name is
    /// mangled, e.g. `Vec__i32`) map back to their template name (`Vec`) via
    /// `generic_origin`, since the `impl Vec[T]` block — and therefore the
    /// contract entry — is keyed on the template.
    fn source_type_name(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Struct(id) => {
                let sd = &self.structs[id.0 as usize];
                Some(match &sd.generic_origin {
                    Some((tmpl, _)) => tmpl.clone(),
                    None => sd.name.clone(),
                })
            }
            Ty::Enum(id) => {
                let ed = &self.enums[id.0 as usize];
                Some(match &ed.generic_origin {
                    Some((tmpl, _)) => tmpl.clone(),
                    None => ed.name.clone(),
                })
            }
            _ => None,
        }
    }

    /// v0.0.12 realtime Phase 1 (method-dispatch hole): enforce the
    /// `#[no_alloc]` / `#[no_block]` contract on a `recv.method()` call site
    /// when the enclosing function carries the contract. Closes the hole where
    /// method dispatch slipped past the post-pass walker (which only sees
    /// free-fn calls). The receiver type is resolved precisely here, so the
    /// verdict is the *dispatched* method's, not a name-collision guess.
    /// Unknown `(type, method)` pairs — blessed/builtin methods handled in the
    /// early branches of `check_method_call` — are not in the map and are
    /// checked at their own sites (e.g. `to_string`).
    fn check_method_contract(&mut self, recv_ty: &Ty, method: &str, span: ByteSpan) {
        if !self.current_fn_no_alloc && !self.current_fn_no_block {
            return;
        }
        let Some(tname) = self.source_type_name(recv_ty) else {
            return;
        };
        let Some(&(m_no_alloc, m_no_block)) =
            self.method_contracts.get(&(tname.clone(), method.to_string()))
        else {
            return;
        };
        if self.current_fn_no_alloc && !m_no_alloc {
            self.err(
                "E0901",
                format!(
                    "function is marked `#[no_alloc]` but calls method `{tname}::{method}` which is not marked `#[no_alloc]`",
                ),
                span,
            );
        }
        if self.current_fn_no_block && !m_no_block {
            self.err(
                "E0907",
                format!(
                    "function is marked `#[no_block]` but calls method `{tname}::{method}` which is not marked `#[no_block]`",
                ),
                span,
            );
        }
    }

    /// v0.0.14 inline asm Tier 3: a `#[naked]` function emits no
    /// prologue/epilogue, so its body must be inline asm only — anything else
    /// would read/write a stack frame that doesn't exist. Each statement must
    /// be `#asm(...)` (optionally wrapped in `unsafe { ... }`); a non-asm
    /// statement or a value tail is **E0906**.
    fn check_naked(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let ItemKind::Function(f) = &item.kind else {
                continue;
            };
            if !has_attr_named(&f.attributes, "naked") {
                continue;
            }
            if f.is_extern {
                continue;
            }
            for s in &f.body.stmts {
                if !stmt_is_asm_only(s) {
                    self.err(
                        "E0909",
                        format!(
                            "`#[naked]` function `{}` may contain only inline `#asm(...)` statements (no prologue/epilogue is emitted); move any other code into a normal function the asm calls",
                            f.name.name
                        ),
                        s.span,
                    );
                }
            }
            if let Some(tail) = &f.body.tail {
                if !expr_is_asm_only(tail) {
                    self.err(
                        "E0909",
                        format!(
                            "`#[naked]` function `{}` cannot end with a value expression — the asm must perform the return itself",
                            f.name.name
                        ),
                        tail.span,
                    );
                }
            }
        }
        self.current_file = None;
    }

    fn check_no_alloc(&mut self, p: &Program) {
        let fn_table = build_no_alloc_fn_table(p);
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            match &item.kind {
                ItemKind::Function(f) => {
                    if !marks_no_alloc(&f.attributes) {
                        continue;
                    }
                    // `#[no_alloc] extern fn ...` is the user's promise that
                    // the C symbol doesn't allocate; no body to walk.
                    if f.is_extern {
                        continue;
                    }
                    self.walk_no_alloc_body(&f.body, &f.name.name, &fn_table);
                }
                ItemKind::Impl(b) => {
                    for m in &b.methods {
                        if !marks_no_alloc(&m.attributes) {
                            continue;
                        }
                        self.walk_no_alloc_body(&m.body, &m.name.name, &fn_table);
                    }
                }
                _ => {}
            }
        }
        self.current_file = None;
    }

    fn walk_no_alloc_body(&mut self, body: &Block, caller: &str, fn_table: &NoAllocFnTable) {
        let mut effects = BodyEffects::default();
        collect_effects_block(body, &mut effects);
        for (callee_raw, span) in &effects.calls {
            self.check_no_alloc_call(callee_raw, *span, caller, fn_table);
        }
        // Allocating language constructs (string interpolation → malloc).
        for span in &effects.interps {
            self.err(
                "E0901",
                format!(
                    "function `{}` is marked `#[no_alloc]` but uses string interpolation, which heap-allocates",
                    leaf_name(caller),
                ),
                *span,
            );
        }
    }

    fn check_no_alloc_call(
        &mut self,
        callee_raw: &str,
        span: ByteSpan,
        caller: &str,
        fn_table: &NoAllocFnTable,
    ) {
        // Trailing-segment name — used for blocklist/whitelist matching when
        // the resolver has qualified the callee (e.g. `vec.malloc`).
        let leaf = callee_raw
            .rsplit_once('.')
            .map(|(_, n)| n)
            .unwrap_or(callee_raw);

        let info = fn_table
            .fns
            .get(callee_raw)
            .or_else(|| fn_table.fns.get(leaf));

        if let Some(fi) = info {
            // Effective C symbol: prefer #[link_name], else the leaf name.
            let symbol = fi.link_name.as_deref().unwrap_or(leaf);
            if ALLOC_BLOCKLIST.contains(&symbol) {
                self.err(
                    "E0901",
                    format!(
                        "function `{}` is marked `#[no_alloc]` but calls allocating function `{}`",
                        leaf_name(caller),
                        symbol,
                    ),
                    span,
                );
                return;
            }
            if fi.is_extern {
                if LEAF_WHITELIST.contains(&symbol) || fi.has_no_alloc {
                    return;
                }
                self.err(
                    "E0901",
                    format!(
                        "function `{}` is marked `#[no_alloc]` but calls extern `{}` which is not in the known-non-allocating leaf set; add `#[no_alloc]` to the extern declaration if it is known not to allocate",
                        leaf_name(caller),
                        symbol,
                    ),
                    span,
                );
                return;
            }
            // User-defined fn: must itself be `#[no_alloc]`.
            if !fi.has_no_alloc {
                self.err(
                    "E0901",
                    format!(
                        "function `{}` is marked `#[no_alloc]` but calls `{}` which is not marked `#[no_alloc]`",
                        leaf_name(caller),
                        leaf,
                    ),
                    span,
                );
            }
            return;
        }
        // Unresolved callee — likely a method-dispatch shape we don't
        // statically resolve here, or a name the resolver knows under a form
        // we didn't index. Skip silently; the rule blocks the common cases
        // (direct allocator externs, constructor-style free/assoc fns).
    }

    fn check_bounded_recursion(&mut self, p: &Program) {
        let fn_table = build_no_alloc_fn_table(p);
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            let (caller, body, span) = match &item.kind {
                ItemKind::Function(f) if marks_bounded_recursion(&f.attributes) => {
                    if f.is_extern {
                        continue;
                    }
                    (f.name.name.clone(), &f.body, f.name.span)
                }
                _ => continue,
            };
            // Walk reachable set; if `caller` reachable from itself, error.
            let mut reachable: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut worklist: Vec<&Block> = vec![body];
            let mut found_cycle = false;
            while let Some(blk) = worklist.pop() {
                let mut effects = BodyEffects::default();
                collect_effects_block(blk, &mut effects);
                for (callee_raw, _span) in effects.calls {
                    // Try full + leaf lookup, same as no_alloc.
                    let key_full = callee_raw.clone();
                    let leaf = callee_raw
                        .rsplit_once('.')
                        .map(|(_, n)| n.to_string())
                        .unwrap_or_else(|| callee_raw.clone());
                    let canonical = if fn_table.fns.contains_key(&key_full) {
                        key_full
                    } else if fn_table.fns.contains_key(&leaf) {
                        leaf
                    } else {
                        continue;
                    };
                    if canonical == caller {
                        found_cycle = true;
                        break;
                    }
                    if reachable.insert(canonical.clone()) {
                        if let Some(fi) = fn_table.fns.get(&canonical) {
                            if let Some(b) = fi.body {
                                worklist.push(b);
                            }
                        }
                    }
                }
                if found_cycle {
                    break;
                }
            }
            if found_cycle {
                self.err(
                    "E0906",
                    format!(
                        "function `{}` is marked `#[bounded_recursion]` but its call graph leads back to itself",
                        leaf_name(&caller),
                    ),
                    span,
                );
            }
        }
        self.current_file = None;
    }
    // v0.0.12 realtime Phase 3: `#[no_block]` pass. Same walker shape as
    // `check_no_alloc`, different verdict: a `#[no_block]` (or `#[realtime]`)
    // function must not call a blocking primitive directly or transitively.
    fn check_no_block(&mut self, p: &Program) {
        let fn_table = build_no_alloc_fn_table(p);
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            match &item.kind {
                ItemKind::Function(f) => {
                    if !marks_no_block(&f.attributes) {
                        continue;
                    }
                    // `#[no_block] extern fn ...` is the user's promise that
                    // the C symbol doesn't block; no body to walk.
                    if f.is_extern {
                        continue;
                    }
                    self.walk_no_block_body(&f.body, &f.name.name, &fn_table);
                }
                ItemKind::Impl(b) => {
                    for m in &b.methods {
                        if !marks_no_block(&m.attributes) {
                            continue;
                        }
                        self.walk_no_block_body(&m.body, &m.name.name, &fn_table);
                    }
                }
                _ => {}
            }
        }
        self.current_file = None;
    }

    fn walk_no_block_body(&mut self, body: &Block, caller: &str, fn_table: &NoAllocFnTable) {
        let mut effects = BodyEffects::default();
        collect_effects_block(body, &mut effects);
        for (callee_raw, span) in &effects.calls {
            self.check_no_block_call(callee_raw, *span, caller, fn_table);
        }
        // String interpolation allocates but does not block — not a no_block
        // concern; `effects.interps` is intentionally ignored here.
    }

    fn check_no_block_call(
        &mut self,
        callee_raw: &str,
        span: ByteSpan,
        caller: &str,
        fn_table: &NoAllocFnTable,
    ) {
        let leaf = callee_raw
            .rsplit_once('.')
            .map(|(_, n)| n)
            .unwrap_or(callee_raw);

        let info = fn_table
            .fns
            .get(callee_raw)
            .or_else(|| fn_table.fns.get(leaf));

        if let Some(fi) = info {
            // Effective C symbol: prefer #[link_name], else the leaf name.
            let symbol = fi.link_name.as_deref().unwrap_or(leaf);
            if BLOCK_BLOCKLIST.contains(&symbol) {
                self.err(
                    "E0907",
                    format!(
                        "function `{}` is marked `#[no_block]` but calls blocking function `{}`",
                        leaf_name(caller),
                        symbol,
                    ),
                    span,
                );
                return;
            }
            if fi.is_extern {
                if BLOCK_SAFE_LEAF.contains(&symbol) || fi.has_no_block {
                    return;
                }
                self.err(
                    "E0907",
                    format!(
                        "function `{}` is marked `#[no_block]` but calls extern `{}` which is not in the known-nonblocking leaf set; add `#[no_block]` to the extern declaration if it is known not to block",
                        leaf_name(caller),
                        symbol,
                    ),
                    span,
                );
                return;
            }
            // User-defined fn: must itself be `#[no_block]` (or `#[realtime]`).
            if !fi.has_no_block {
                self.err(
                    "E0907",
                    format!(
                        "function `{}` is marked `#[no_block]` but calls `{}` which is not marked `#[no_block]`",
                        leaf_name(caller),
                        leaf,
                    ),
                    span,
                );
            }
            return;
        }
        // Unresolved callee — method-dispatch shape we don't statically
        // resolve here. Skip silently; the rule blocks the common cases
        // (direct blocking externs, free/assoc fns).
    }
    // ========================================================================
    // End #[no_alloc] / #[no_block] / #[bounded_recursion]
    // ========================================================================

    // v0.0.12 realtime Phase 4: `#[max_stack(N)]` — bound a function's stack
    // frame. The estimate is the sum of (a) parameter sizes and (b) the sizes
    // of every `let` binding with a known type, walked through all nested
    // blocks. This is deliberately a conservative over-estimate (all locals
    // counted as live simultaneously, regardless of scope overlap) so a pass
    // is a real guarantee. It is also a *lower bound on coverage*: locals
    // whose type can only be inferred (untyped `let x = ...`) and
    // compiler-inserted temporaries are not yet counted; the headline cases
    // (large fixed arrays, by-value aggregates) carry explicit types in
    // practice. Call-chain worst-case and coroutine frames are future work.
    /// v0.0.13 (plan.opaque.md §2/§6): raw-pointer accountability. Every
    /// raw-pointer (`*T`) struct field must be *accounted for*:
    ///   - marked `opaque` (another owner releases it), or
    ///   - released by the struct's `drop`, in one of two structural shapes —
    ///     an unconditional direct release (`unsafe { free(self.f); }`) or a
    ///     release guarded by a null-test on the same field
    ///     (`if self.f.is_not_null() { ... }` / `if !self.f.is_null() { ... }`).
    /// Anything else (no `drop`, an omitted field, or a release hidden behind an
    /// arbitrary condition / loop / helper call) is **E0510**. The check is a
    /// structural pattern match over the `drop` body — no dataflow, no
    /// interprocedural walk — per the design's "local direct release" rule.
    fn check_raw_pointer_accountability(&mut self, p: &Program) {
        use std::collections::HashMap;

        fn is_raw_ptr(t: &Type) -> bool {
            matches!(t.kind, TypeKind::RawPtr(_))
        }
        /// `self.field` used as a call argument — seeing through an `as` cast,
        /// since the common release idiom is `free(self.ptr as *u8)` (the
        /// releaser takes `*u8` but the field is `*T`). The cast is trivial and
        /// keeps the field visibly the call's argument, so it still satisfies
        /// the "direct, local release" rule.
        fn arg_is_self_field(e: &Expr, field: &str) -> bool {
            match &e.kind {
                ExprKind::Field { receiver, name } => {
                    name.name == field
                        && matches!(&receiver.kind, ExprKind::Ident(n) if n == "self")
                }
                ExprKind::Cast { expr, .. } => arg_is_self_field(expr, field),
                _ => false,
            }
        }
        /// `self.field.is_not_null()` or `!self.field.is_null()`.
        fn is_null_guard(cond: &Expr, field: &str) -> bool {
            match &cond.kind {
                ExprKind::Call { callee, args, .. } if args.is_empty() => {
                    matches!(&callee.kind, ExprKind::Field { receiver, name }
                        if name.name == "is_not_null" && arg_is_self_field(receiver, field))
                }
                ExprKind::Unary { op: UnaryOp::Not, operand } => matches!(
                    &operand.kind, ExprKind::Call { callee, args, .. } if args.is_empty()
                    && matches!(&callee.kind, ExprKind::Field { receiver, name }
                        if name.name == "is_null" && arg_is_self_field(receiver, field))),
                _ => false,
            }
        }
        fn expr_releases(e: &Expr, field: &str) -> bool {
            match &e.kind {
                // A direct release: `field` passed to a direct call.
                ExprKind::Call { args, .. } => args.iter().any(|a| arg_is_self_field(a, field)),
                // Transparent wrappers — `unsafe { ... }` and bare blocks.
                ExprKind::Unsafe(b) | ExprKind::Block(b) => block_releases(b, field),
                // The one allowed conditional: a null-guard on the same field,
                // no `else`. Arbitrary conditions are intentionally NOT descended.
                ExprKind::If { cond, then, else_branch } => {
                    else_branch.is_none() && is_null_guard(cond, field) && block_releases(then, field)
                }
                _ => false,
            }
        }
        /// PROVABLY released: an unconditional direct release, or one guarded
        /// only by a null-test on the same field (both leak-free by inspection).
        fn block_releases(b: &Block, field: &str) -> bool {
            b.stmts.iter().any(|s| match &s.kind {
                StmtKind::Expr(e) | StmtKind::Defer(e) => expr_releases(e, field),
                _ => false,
            }) || b.tail.as_ref().map_or(false, |t| expr_releases(t, field))
        }

        /// A direct release of `field` APPEARS somewhere in the drop body —
        /// descending through *all* control flow (any `if`/`else`, loop, match).
        /// Used to tell "freed conditionally" (→ warning) from "never freed at
        /// all / delegated to a helper" (→ error). A method/helper call that
        /// doesn't pass the field as an argument is not a release, so delegation
        /// (`self.cleanup()`) correctly does not count.
        fn appears_released(body: &Block, field: &str) -> bool {
            fn e(x: &Expr, f: &str) -> bool {
                match &x.kind {
                    ExprKind::Call { callee, args, .. } => {
                        args.iter().any(|a| arg_is_self_field(a, f))
                            || e(callee, f)
                            || args.iter().any(|a| e(a, f))
                    }
                    ExprKind::Unsafe(bl) | ExprKind::Block(bl) => b(bl, f),
                    ExprKind::If { cond, then, else_branch } => {
                        e(cond, f)
                            || b(then, f)
                            || else_branch.as_ref().is_some_and(|eb| e(eb, f))
                    }
                    ExprKind::Match { scrutinee, arms } => {
                        e(scrutinee, f) || arms.iter().any(|a| e(&a.body, f))
                    }
                    ExprKind::Binary { lhs, rhs, .. } => e(lhs, f) || e(rhs, f),
                    ExprKind::Unary { operand, .. } => e(operand, f),
                    ExprKind::Cast { expr, .. } => e(expr, f),
                    ExprKind::Field { receiver, .. } => e(receiver, f),
                    ExprKind::Index { receiver, index } => e(receiver, f) || e(index, f),
                    ExprKind::Assign { target, value, .. } => e(target, f) || e(value, f),
                    ExprKind::Await(x2) | ExprKind::Yield(x2) => e(x2, f),
                    ExprKind::Range { start, end, .. } => {
                        start.as_ref().is_some_and(|s| e(s, f))
                            || end.as_ref().is_some_and(|s| e(s, f))
                    }
                    _ => false,
                }
            }
            fn s(st: &Stmt, f: &str) -> bool {
                match &st.kind {
                    StmtKind::Expr(x) | StmtKind::Defer(x) | StmtKind::Assert(x) => e(x, f),
                    StmtKind::Return(Some(x)) => e(x, f),
                    StmtKind::Let { init: Some(x), .. } => e(x, f),
                    StmtKind::While { cond, body, .. } => e(cond, f) || b(body, f),
                    StmtKind::Loop(bl, _) => b(bl, f),
                    StmtKind::For(fl, _) => match fl {
                        ForLoop::Range { iter, body, .. } => e(iter, f) || b(body, f),
                        ForLoop::CStyle { init, cond, update, body } => {
                            init.as_ref().is_some_and(|i| s(i, f))
                                || cond.as_ref().is_some_and(|c| e(c, f))
                                || update.iter().any(|u| e(u, f))
                                || b(body, f)
                        }
                    },
                    StmtKind::IfLet { scrutinee, body, else_body, .. } => {
                        e(scrutinee, f)
                            || b(body, f)
                            || else_body.as_ref().is_some_and(|eb| b(eb, f))
                    }
                    StmtKind::GuardLet { scrutinee, else_body, .. } => e(scrutinee, f) || b(else_body, f),
                    StmtKind::WhileLet { scrutinee, body, .. } => e(scrutinee, f) || b(body, f),
                    _ => false,
                }
            }
            fn b(bl: &Block, f: &str) -> bool {
                bl.stmts.iter().any(|st| s(st, f)) || bl.tail.as_ref().is_some_and(|t| e(t, f))
            }
            b(body, field)
        }

        // Each struct's `drop` body, keyed by the impl target name.
        let mut drop_bodies: HashMap<&str, &Block> = HashMap::new();
        for item in &p.items {
            if let ItemKind::Impl(b) = &item.kind {
                for m in &b.methods {
                    if m.name.name == "drop" {
                        drop_bodies.insert(b.target.name.as_str(), &m.body);
                    }
                }
            }
        }

        for item in &p.items {
            let ItemKind::Struct(s) = &item.kind else { continue; };
            self.current_file = item.origin_file.clone();
            let body = drop_bodies.get(s.name.name.as_str());
            for f in &s.fields {
                if f.is_opaque || !is_raw_ptr(&f.ty) {
                    continue;
                }
                let fname = &f.name.name;
                // Severity tracks what the compiler can prove (plan.opaque.md §6):
                //   provably freed (unconditional / null-guarded)  -> clean
                //   freed, but only under some other condition       -> W0002 warning
                //   no direct free of the field appears at all       -> E0510 error
                if body.is_some_and(|b| block_releases(b, fname)) {
                    continue;
                }
                if body.is_some_and(|b| appears_released(b, fname)) {
                    self.warn(
                        "W0002",
                        format!(
                            "raw-pointer field `{fname}` is freed only conditionally in `drop`; \
                             the compiler can't prove the release always runs. Intended for \
                             refcounted/optional ownership — confirm it frees on every owning path"
                        ),
                        f.span,
                    );
                    continue;
                }
                let detail = if body.is_none() {
                    "the struct has no `drop` to release it"
                } else {
                    "this struct's `drop` never frees it (a release delegated to a helper \
                     doesn't count — it must be a direct call here)"
                };
                self.err(
                    "E0510",
                    format!(
                        "raw-pointer field `{fname}` is unaccounted: {detail}. \
                         Mark it `opaque {fname}: ...` if another owner frees it, \
                         or release it in `fn drop(mut self)` — e.g. \
                         `unsafe {{ free(self.{fname}); }}`"
                    ),
                    f.span,
                );
            }
        }
        self.current_file = None;
    }

    fn check_max_stack(&mut self, p: &Program) {
        for item in &p.items {
            self.current_file = item.origin_file.clone();
            match &item.kind {
                ItemKind::Function(f) => {
                    if f.is_extern {
                        continue;
                    }
                    if let Some((budget, span)) = max_stack_budget(&f.attributes) {
                        self.check_one_max_stack(&f.params, &f.body, &f.name.name, budget, span);
                    }
                }
                ItemKind::Impl(b) => {
                    for m in &b.methods {
                        if let Some((budget, span)) = max_stack_budget(&m.attributes) {
                            self.check_one_max_stack(
                                &m.params,
                                &m.body,
                                &m.name.name,
                                budget,
                                span,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        self.current_file = None;
    }

    fn check_one_max_stack(
        &mut self,
        params: &[crate::ast::Param],
        body: &Block,
        name: &str,
        budget: u64,
        span: ByteSpan,
    ) {
        let mut total: u64 = 0;
        for pm in params {
            let ty = self.resolve_type(&pm.ty);
            total = total.saturating_add(self.stack_size_of(&ty));
        }
        let mut effects = BodyEffects::default();
        collect_effects_block(body, &mut effects);
        for t in &effects.let_tys {
            let ty = self.resolve_type(t);
            total = total.saturating_add(self.stack_size_of(&ty));
        }
        if total > budget {
            self.err(
                "E0908",
                format!(
                    "function `{}` is marked `#[max_stack({})]` but its estimated stack frame is {} bytes (parameters + locals with known types)",
                    leaf_name(name),
                    budget,
                    total,
                ),
                span,
            );
        }
    }

    /// Static byte size of a type for the `#[max_stack]` estimate. Mirrors
    /// codegen's `static_layout` ABI rules (the two must agree); kept in sema
    /// so the bounded-stack pass needs no codegen type table. Unsizable types
    /// (`Param`, `Error`) contribute 0 — a generic frame can't be sized
    /// before monomorphization.
    fn stack_size_of(&self, ty: &Ty) -> u64 {
        self.layout_of(ty).0
    }

    fn layout_of(&self, ty: &Ty) -> (u64, u64) {
        fn align_up(off: u64, al: u64) -> u64 {
            if al == 0 {
                off
            } else {
                (off + al - 1) & !(al - 1)
            }
        }
        match ty {
            Ty::I8 | Ty::U8 | Ty::Bool => (1, 1),
            Ty::I16 | Ty::U16 | Ty::F16 => (2, 2),
            Ty::I32 | Ty::U32 | Ty::F32 => (4, 4),
            Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::F64 => (8, 8),
            Ty::RawPtr(_) | Ty::FnPtr { .. } => (8, 8),
            Ty::Str | Ty::Slice(_) => (16, 8),
            Ty::String => (24, 8),
            Ty::Unit => (0, 1),
            Ty::Array(elem, n) => {
                let (esz, ea) = self.layout_of(elem);
                (esz.saturating_mul(*n as u64), ea)
            }
            Ty::Struct(id) => {
                let info = &self.structs[id.0 as usize];
                let mut off: u64 = 0;
                let mut max_al: u64 = 1;
                for (_, fty, _) in &info.fields {
                    let (sz, al) = self.layout_of(fty);
                    if al > max_al {
                        max_al = al;
                    }
                    off = align_up(off, al);
                    off = off.saturating_add(sz);
                }
                (align_up(off, max_al), max_al.max(1))
            }
            Ty::Enum(id) => {
                let info = &self.enums[id.0 as usize];
                if !info.is_tagged {
                    (4, 4)
                } else {
                    let mut max_slots: u64 = 0;
                    for v in &info.variants {
                        let mut bytes: u64 = 0;
                        for pty in &v.payload {
                            let (sz, _) = self.layout_of(pty);
                            bytes = bytes.saturating_add((sz + 7) & !7);
                        }
                        let slots = (bytes + 7) / 8;
                        if slots > max_slots {
                            max_slots = slots;
                        }
                    }
                    (8u64.saturating_add(max_slots.saturating_mul(8)), 8)
                }
            }
            Ty::Simd { elem, lanes } | Ty::Mask { elem, lanes } => {
                let (esz, ea) = self.layout_of(elem);
                let size = esz.saturating_mul(*lanes as u64);
                let align = if size.is_power_of_two() { size } else { ea };
                (size, align)
            }
            Ty::Error | Ty::Param(_) => (0, 0),
        }
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
        let Some(attr) = test_attr else {
            return;
        };
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
        let Some(sig) = sig else {
            return;
        }; // duplicate def already errored
           // Phase 5 slice 5ATTR.2 — sema rules specific to `#[test]` fns.
        self.check_test_attribute_rules(f, &sig);
        // Slice 7GEN.4: generic params remain in scope across body checking.
        self.push_type_params(&f.generic_params);
        // v0.0.3 Phase 5 Slice 5E.2: async fn body sees the UNWRAPPED
        // return type (the user's declared T), not the `Future[T]`
        // that the signature exposes to callers. The wrap happens at
        // codegen time (Slice 5E.3).
        let body_return = if f.is_async {
            self.unwrap_future(&sig.return_type)
                .unwrap_or_else(|| sig.return_type.clone())
        } else if f.is_gen {
            // v0.0.4 Phase 4 Slice 4A: gen fn body sees Unit return.
            // The body's job is to `yield` values; falling off the end
            // (or `return;`) completes the iteration with None.
            Ty::Unit
        } else {
            sig.return_type.clone()
        };
        self.current_return = body_return.clone();
        let prev_async = self.current_fn_is_async;
        self.current_fn_is_async = f.is_async;
        let prev_gen = self.current_fn_is_gen;
        let prev_gen_ty = self.current_gen_yield_ty.clone();
        self.current_fn_is_gen = f.is_gen;
        self.current_gen_yield_ty = if f.is_gen {
            self.unwrap_iterator(&sig.return_type)
        } else {
            None
        };
        let fn_no_alloc = marks_no_alloc(&f.attributes);
        self.scopes.push(HashMap::new());
        for (param, psig) in f.params.iter().zip(sig.params.iter()) {
            // v0.0.15 `#[no_alloc]` drop-glue, parameter arm: an owned
            // drop-carrying parameter whose teardown the callee runs (see
            // `no_alloc_param_drops_here`) frees heap at scope exit — invisible
            // in the body but real. Reject it, mirroring the `let`-local rule.
            if fn_no_alloc
                && self.no_alloc_param_drops_here(param, &psig.ty)
                && !self.no_alloc_safe_drop(&psig.ty)
            {
                self.err(
                    "E0901",
                    format!(
                        "`#[no_alloc]` function: owned parameter `{}` of type `{}` runs an allocating destructor at scope exit (its `drop` frees heap or is not marked `#[no_alloc]`)",
                        param.name.name,
                        ty_display(&psig.ty),
                    ),
                    param.span,
                );
            }
            // E0334: `mut` and `move` are mutually exclusive ownership markers.
            if param.mutable && param.move_ {
                self.err(
                    "E0334",
                    "parameter cannot have both `mut` and `move`; these markers are mutually exclusive".to_string(),
                    param.span,
                );
            }
            // v0.0.9 follow-up: `borrow` is mutually exclusive with
            // both `move` and `mut`. See the matching check in
            // `check_methods` for the rationale.
            if param.borrow_ && param.move_ {
                self.err("E0334",
                    "parameter cannot have both `borrow` and `move`; `borrow` is shared by-value, `move` transfers ownership".to_string(),
                    param.span);
            }
            if param.borrow_ && param.mutable {
                self.err("E0334",
                    "parameter cannot have both `borrow` and `mut`; `borrow` is shared by-value, `mut` is an exclusive borrow".to_string(),
                    param.span);
            }
            // v0.0.8 post-bench-gap: E0411 — `restrict` requires a raw
            // pointer (`*T`) param. It's an opt-in `noalias` assertion
            // and has no meaning on other shapes.
            if param.restrict && !matches!(psig.ty, Ty::RawPtr(_)) {
                self.err(
                    "E0411",
                    "`restrict` is only valid on raw pointer (`*T`) parameters".to_string(),
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
                let is_mut_pointer_passed = param.mutable && !param.move_ && !self.is_copy(pty);
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
                LocalInfo {
                    ty: psig.ty.clone(),
                    mutable: param.mutable,
                    moved: false,
                    assigned: true,
                    borrow_roots: BTreeSet::new(),
                },
            );
        }
        self.setup_returned_borrow_ctx(&f.params, &f.return_type, false);
        self.check_function_body(
            &f.body,
            body_return,
            f.body.span,
            marks_no_alloc(&f.attributes),
            marks_no_block(&f.attributes),
            has_attr_named(&f.attributes, "naked"),
        );
        self.scopes.pop();
        self.current_fn_is_async = prev_async;
        self.current_fn_is_gen = prev_gen;
        self.current_gen_yield_ty = prev_gen_ty;
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
            if let Some(reason) = self.c_exportable_diagnosis(&pty, /*is_return=*/ false) {
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
            if let Some(reason) = self.c_exportable_diagnosis(&ret_ty, /*is_return=*/ true) {
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
            | Ty::F16 | Ty::F32 | Ty::F64
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
            // v0.0.6 Slice 1B: SIMD types are not C-ABI compatible by
            // default (no portable C representation; cf. NEON's `float32x4_t`
            // vs SSE's `__m128`). Bitcast to `[T; N]` via `to_array` at the
            // boundary. Mask types share the SIMD lowering; same reject.
            Ty::Simd { lanes, elem } => Some(format!(
                "SIMD type `{}` has no portable C-ABI representation; cast to `[{}; {}]` via `.to_array()` at the boundary",
                ty_display(ty), ty_display(elem), lanes,
            )),
            Ty::Mask { lanes, elem } => Some(format!(
                "SIMD mask type `{}` has no portable C-ABI representation; convert to a `Simd` via `.to_bits()` then `.to_array()` at the boundary (lanes={lanes}, elem={})",
                ty_display(ty), ty_display(elem),
            )),
            Ty::Error => None,  // Type already errored; don't double-report.
        }
    }

    fn check_function_body(
        &mut self,
        body: &Block,
        expected: Ty,
        body_span: ByteSpan,
        no_alloc: bool,
        no_block: bool,
        is_naked: bool,
    ) {
        // v0.0.12 realtime Phase 1: record the enclosing function's contract
        // so `check_method_call` can enforce it on method-dispatch sites. Reset
        // to the default (false) on exit — nothing nests, and every completed
        // body leaves the flags clear so stray `check_expr` calls in later
        // passes never see a stale contract.
        self.current_fn_no_alloc = no_alloc;
        self.current_fn_no_block = no_block;
        // Push the body scope.
        self.scopes.push(HashMap::new());
        for s in &body.stmts {
            self.check_stmt(s);
        }
        // C+ style: function bodies use explicit `return`, never an implicit
        // tail expression. Block expressions remain valid in let initializers,
        // assignments, and return expressions — just not at function-body level.
        if let Some(tail) = &body.tail {
            // Type-check the tail first so the E0333 message can suggest the
            // right fix. v0.0.12 G-022: when the function returns `()` and
            // the tail expression is itself unit-typed (the very common
            // `fn f() { unsafe { ... } }` / `fn f() { if c { ... } }`
            // shape), the right fix is `;` after the closing brace, not
            // `return ...;`. The previous one-size message led writers to
            // append `return;` which compiles but reads worse than `};`.
            // A `#[naked]` body is inline asm that returns on its own; an asm
            // tail is its normal shape, so don't type it against the declared
            // return (it is `()`), and skip the implicit-tail rules.
            let tail_ty = if is_naked {
                self.check_expr(tail, None)
            } else {
                self.check_expr(tail, Some(expected.clone()))
            };
            if !is_naked {
                let msg = if expected == Ty::Unit && tail_ty == Ty::Unit {
                    "function body cannot end with an implicit tail expression; \
                     add `;` after the closing `}` (or `return;` if you prefer the explicit form)"
                        .to_string()
                } else {
                    "function body cannot end with an implicit tail expression; \
                     use `return ...;` instead"
                        .to_string()
                };
                self.err("E0333", msg, tail.span);
            }
        } else if !is_naked
            && expected != Ty::Unit
            && expected != Ty::Error
            && !body_ends_with_return(body)
        {
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
        self.current_fn_no_alloc = false;
        self.current_fn_no_block = false;
    }

    // ---- statements ----

    fn check_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            } => {
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let (final_ty, assigned) = match init {
                    Some(init_expr) => {
                        let inferred = self.check_expr(init_expr, declared.clone());
                        let final_ty = declared.clone().unwrap_or(inferred);
                        // E0509: `let q = drop_typed.field;` moves a field out
                        // from under a live destructor — double-free. Reject.
                        self.reject_partial_move_of_drop(init_expr, &final_ty);
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
                // v0.0.14 `#[no_alloc]` drop-glue: a local whose scope-exit
                // teardown frees heap (a `string`/`Vec`/`Box`) or runs a
                // `drop` not marked `#[no_alloc]` would allocate/deallocate at
                // the closing brace — invisible in the body but real. Reject it
                // in a `#[no_alloc]` function.
                if self.current_fn_no_alloc && !self.no_alloc_safe_drop(&final_ty) {
                    self.err(
                        "E0901",
                        format!(
                            "`#[no_alloc]` function: local `{}` of type `{}` runs an allocating destructor at scope exit (its `drop` frees heap or is not marked `#[no_alloc]`)",
                            name.name,
                            ty_display(&final_ty),
                        ),
                        s.span,
                    );
                }
                let borrow_roots = if matches!(final_ty, Ty::Str | Ty::Slice(_)) {
                    init.as_ref()
                        .map(|e| self.returned_borrow_roots(e))
                        .unwrap_or_default()
                } else {
                    BTreeSet::new()
                };
                self.scopes.last_mut().unwrap().insert(
                    name.name.clone(),
                    LocalInfo {
                        ty: final_ty,
                        mutable: *mutable,
                        moved: false,
                        assigned,
                        borrow_roots,
                    },
                );
            }
            StmtKind::Return(value) => {
                let ret = self.current_return.clone();
                match (value, &ret) {
                    (Some(e), _) => {
                        let t = self.check_expr(e, Some(ret.clone()));
                        // E0509: `return drop_typed.field;` moves a field out
                        // from under a live destructor — double-free. Reject.
                        let moved_ty = if matches!(ret, Ty::Error) {
                            t
                        } else {
                            ret.clone()
                        };
                        self.reject_partial_move_of_drop(e, &moved_ty);
                        // E0512/E0513: returned borrow must outlive the call —
                        // region-matched (#2) and not rooted at a dropped local (#3).
                        self.check_returned_borrow(e);
                    }
                    (None, &Ty::Unit) | (None, &Ty::Error) => {}
                    (None, _) => {
                        self.err(
                            "E0307",
                            format!(
                                "`return` without a value, but function returns `{}`",
                                ret.name()
                            ),
                            s.span,
                        );
                    }
                }
            }
            StmtKind::While {
                cond,
                body,
                attributes,
            } => {
                self.check_loop_attrs(attributes);
                let _ = self.check_cond(cond);
                self.scopes.push(HashMap::new());
                self.loop_depth += 1;
                self.check_block_as_stmt(body);
                self.loop_depth -= 1;
                self.scopes.pop();
            }
            StmtKind::For(fl, attributes) => {
                self.check_loop_attrs(attributes);
                self.loop_depth += 1;
                self.check_for(fl);
                self.loop_depth -= 1;
            }
            StmtKind::Expr(e) => {
                let ty = self.check_expr(e, None);
                // v0.0.15 `#[no_alloc]` drop-glue, temporary arm: a discarded
                // expression statement that materializes a fresh owned value
                // (a call, constructor, …) drops that unnamed temporary at
                // statement end. If the teardown allocates, reject it — the same
                // implicit-drop rule the `let`-local and parameter arms enforce.
                // Place expressions (`x;`, `obj.field;`) name existing storage
                // dropped at its own scope exit, so they are exempt.
                if self.current_fn_no_alloc
                    && !self.expr_is_place(e)
                    && !self.no_alloc_safe_drop(&ty)
                {
                    self.err(
                        "E0901",
                        format!(
                            "`#[no_alloc]` function: discarded temporary of type `{}` runs an allocating destructor at statement end (its `drop` frees heap or is not marked `#[no_alloc]`)",
                            ty_display(&ty),
                        ),
                        e.span,
                    );
                }
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
                    let kw = if matches!(s.kind, StmtKind::Break) {
                        "break"
                    } else {
                        "continue"
                    };
                    self.err("E0353", format!("`{kw}` used outside of a loop"), s.span);
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
                        format!("`assert` condition must be `bool`, got `{}`", actual.name()),
                        e.span,
                    );
                }
            }
            // Slice 4-end: `loop { BODY }` — unconditional loop. Body
            // runs in a fresh scope with the loop-depth incremented so
            // any nested break/continue type-checks. Loops always
            // produce unit at the statement level.
            StmtKind::Loop(body, attributes) => {
                self.check_loop_attrs(attributes);
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

    /// v0.0.7 Slice 1.3: validate `#[unroll(N)]` / `#[vectorize_width(N)]`
    /// on loop statements. Range validation: N is a literal in
    /// `[1, 256]` — emit **E0510** on out-of-range. Unknown attribute
    /// names + bad arg shapes are caught at the boundary in
    /// `attrs.rs::check`; this pass only enforces the range rule
    /// which is per-attribute and needs the loop-statement context.
    fn check_loop_attrs(&mut self, attributes: &[crate::ast::Attribute]) {
        for a in attributes {
            if a.path.name != "unroll" && a.path.name != "vectorize_width" {
                continue;
            }
            let n = match a.args.as_slice() {
                [crate::ast::AttrArg::Int(v, _)] => *v,
                _ => continue, // shape error fires from attrs.rs
            };
            if !(1..=256).contains(&n) {
                self.err(
                    "E0510",
                    format!(
                        "`#[{}]` requires an integer in [1, 256], got {}",
                        a.path.name, n
                    ),
                    a.span,
                );
            }
        }
    }

    fn check_for(&mut self, fl: &ForLoop) {
        match fl {
            ForLoop::Range { var, iter, body } => {
                // First try the literal-range form `for x in 0..n`.
                if let ExprKind::Range {
                    start: Some(s),
                    end: Some(e),
                    ..
                } = &iter.kind
                {
                    self.check_expr(s, Some(Ty::I32));
                    self.check_expr(e, Some(Ty::I32));
                    self.scopes.push(HashMap::new());
                    self.scopes.last_mut().unwrap().insert(
                        var.name.clone(),
                        LocalInfo {
                            ty: Ty::I32,
                            mutable: false,
                            moved: false,
                            assigned: true,
                            borrow_roots: BTreeSet::new(),
                        },
                    );
                    self.check_block_as_stmt(body);
                    self.scopes.pop();
                    return;
                }
                // v0.0.4 Phase 4 Slice 4C: iterator-form `for x in expr`
                // where expr is an `Iterator[T]`. Bind `var: T` inside
                // the body. Lowering happens in codegen.
                let it_ty = self.check_expr(iter, None);
                if matches!(it_ty, Ty::Error) {
                    return;
                }
                let elem_ty = match self.unwrap_iterator(&it_ty) {
                    Some(t) => t,
                    None => {
                        self.err(
                            "E0312",
                            format!(
                                "`for ... in` requires either a closed range (`0..n`) or an `Iterator[T]`, got `{}`",
                                ty_display(&it_ty),
                            ),
                            iter.span,
                        );
                        return;
                    }
                };
                self.scopes.push(HashMap::new());
                self.scopes.last_mut().unwrap().insert(
                    var.name.clone(),
                    LocalInfo {
                        ty: elem_ty,
                        mutable: false,
                        moved: false,
                        assigned: true,
                        borrow_roots: BTreeSet::new(),
                    },
                );
                self.check_block_as_stmt(body);
                self.scopes.pop();
            }
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                self.scopes.push(HashMap::new());
                if let Some(init) = init {
                    self.check_stmt(init);
                }
                if let Some(cond) = cond {
                    let _ = self.check_cond(cond);
                }
                for u in update {
                    let _ = self.check_expr(u, None);
                }
                self.check_block_as_stmt(body);
                self.scopes.pop();
            }
        }
    }

    /// Type-check a block used in statement position (its value is discarded).
    fn check_block_as_stmt(&mut self, b: &Block) {
        self.scopes.push(HashMap::new());
        for s in &b.stmts {
            self.check_stmt(s);
        }
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
        // v0.0.14 graph value-depth: retain each expression's resolved type
        // (rendered with concrete names) for `type-at`. Skip error/unit noise.
        if self.record_value_types && !matches!(actual, Ty::Error | Ty::Unit) {
            let rendered = self.render_ty(&actual);
            self.value_types
                .push((self.current_file.clone(), e.span, rendered));
        }
        if let Some(exp) = expected {
            if exp != Ty::Error && actual != Ty::Error && exp != actual {
                self.err(
                    "E0302",
                    format!(
                        "type mismatch: expected `{}`, found `{}`",
                        exp.name(),
                        actual.name()
                    ),
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
            ExprKind::CStrLit(_) => Ty::RawPtr(Box::new(Ty::U8)),
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
                if matches!(inner_ty, Ty::Error) {
                    return Ty::Error;
                }
                match self.unwrap_future(&inner_ty) {
                    Some(t) => t,
                    None => {
                        self.err(
                            "E0902",
                            format!(
                                "`await` requires a `Future[T]` expression, got `{}`",
                                ty_display(&inner_ty)
                            ),
                            inner.span,
                        );
                        Ty::Error
                    }
                }
            }
            // v0.0.4 Phase 4 Slice 4A: `yield EXPR` produces one value
            // from a generator. Allowed only inside a `gen fn` body;
            // the value type must match the iterator's T. yield is a
            // statement-shaped expression with value type Unit (no
            // result back from the consumer).
            //
            //   - **E1001**: `yield` outside a `gen fn` body.
            //   - **E1002**: yielded value type doesn't match the
            //     iterator's element type.
            ExprKind::Yield(inner) => {
                if !self.current_fn_is_gen {
                    let _ = self.check_expr(inner, None);
                    self.err(
                        "E1001",
                        "`yield` is only valid inside a `gen fn` body".to_string(),
                        e.span,
                    );
                    return Ty::Error;
                }
                let expected = self.current_gen_yield_ty.clone();
                let _inner_ty = self.check_expr(inner, expected.clone());
                Ty::Unit
            }
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => self.check_if(cond, then, else_branch.as_deref()),
            ExprKind::Call {
                callee,
                args,
                type_args,
            } => self.check_call(callee, args, type_args, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, e.span),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, expected, e.span),
            ExprKind::Assign { op, target, value } => self.check_assign(*op, target, value, e.span),
            ExprKind::Range { .. } => {
                self.err(
                    "E0312",
                    "range expressions are only supported as the iterator in `for ... in`"
                        .to_string(),
                    e.span,
                );
                Ty::Error
            }
            ExprKind::Cast { expr, ty } => self.check_cast(expr, ty, e.span),
            ExprKind::Path { segments } => self.check_path(segments, e.span),
            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, e.span),
            ExprKind::GenericStructLit {
                name,
                type_args,
                fields,
            } => self.check_generic_struct_lit(name, type_args, fields, e.span),
            ExprKind::GenericEnumCall {
                enum_name,
                type_args,
                variant,
                args,
            } => self.check_generic_enum_call(enum_name, type_args, variant, args, e.span),
            ExprKind::Field { receiver, name } => self.check_field(receiver, name),
            ExprKind::ArrayLit { elements } => self.check_array_lit(elements, expected, e.span),
            ExprKind::ArrayFill { fill, count, .. } => {
                self.check_array_fill(fill, *count, expected, e.span)
            }
            ExprKind::TupleLit { elements } => self.check_tuple_lit(elements, expected, e.span),
            ExprKind::Index { receiver, index } => self.check_index(receiver, index, e.span),
            ExprKind::Match { scrutinee, arms } => {
                self.check_match(scrutinee, arms, expected, e.span)
            }
            ExprKind::IncludeBytes { path } => self.check_include_bytes(path, e.span),
            ExprKind::IncludeStr { path } => self.check_include_str(path, e.span),
            ExprKind::EnvVar { name } => self.check_env_var(name, e.span),
            ExprKind::Intrinsic {
                name,
                type_args,
                args,
                ret_ty,
            } => self.check_intrinsic(name, type_args, args, ret_ty.as_ref(), e.span),
            ExprKind::Asm {
                template,
                operands,
                clobbers,
            } => self.check_asm(template, operands, clobbers, e.span),
        }
    }

    /// v0.0.16: type-check the `#`-sigil FFI/raw + byte-swap builtin intrinsics
    /// — `#str_ptr`, `#str_len`, `#str_from_raw_parts`, `#slice_ptr`,
    /// `#slice_len`, `#slice_from_raw_parts`, and `#bswap{16,32,64}` /
    /// `#htons` / `#htonl` / `#ntohs` / `#ntohl`. Returns `Some(result_ty)` when
    /// `name` is one of them (else `None`). `*_from_raw_parts` require an
    /// enclosing `unsafe` block. The single source for both the `#name`
    /// dispatch (`check_intrinsic`) and the bare-call path during migration.
    fn ffi_builtin_ty(&mut self, name: &str, args: &[Expr], span: ByteSpan) -> Option<Ty> {
        // `#println(x)` — type-dispatched primitive print (`i32` or `str`). Not
        // user-visible overloading; one of the compiler-known builtins. The
        // library wrapper `io::println` is separate.
        if name == "println" {
            if args.len() != 1 {
                self.err(
                    "E0308",
                    format!("`println` takes exactly 1 argument, got {}", args.len()),
                    span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Some(Ty::Error);
            }
            let arg_ty = self.check_expr(&args[0], None);
            if !matches!(arg_ty, Ty::I32 | Ty::Str | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`println` accepts `i32` or `str`; got `{}`", ty_display(&arg_ty)),
                    args[0].span,
                );
            }
            return Some(Ty::Unit);
        }
        if name == "str_ptr" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], Some(Ty::Str));
            if !matches!(arg_ty, Ty::Str | Ty::Error) {
                self.err(
                    "E0302",
                    format!("`str_ptr` requires a `str` argument, got `{}`", ty_display(&arg_ty)),
                    args[0].span,
                );
            }
            return Some(Ty::RawPtr(Box::new(Ty::U8)));
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
            return Some(Ty::Usize);
        }
        if name == "str_from_raw_parts" && args.len() == 2 {
            if self.unsafe_depth == 0 {
                self.err(
                    "E0801",
                    "`str_from_raw_parts` is unsafe; wrap in `unsafe { ... }`".to_string(),
                    span,
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
            return Some(Ty::Str);
        }
        if name == "slice_ptr" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], None);
            if let Ty::Slice(elem) = &arg_ty {
                return Some(Ty::RawPtr(elem.clone()));
            }
            if !matches!(arg_ty, Ty::Error) {
                self.err(
                    "E0302",
                    format!(
                        "`slice_ptr` requires a slice argument (e.g. `i32[]`), got `{}`",
                        ty_display(&arg_ty)
                    ),
                    args[0].span,
                );
            }
            return Some(Ty::Error);
        }
        if name == "slice_len" && args.len() == 1 {
            let arg_ty = self.check_expr(&args[0], None);
            if !matches!(arg_ty, Ty::Slice(_) | Ty::Error) {
                self.err(
                    "E0302",
                    format!(
                        "`slice_len` requires a slice argument (e.g. `i32[]`), got `{}`",
                        ty_display(&arg_ty)
                    ),
                    args[0].span,
                );
            }
            return Some(Ty::Usize);
        }
        if name == "slice_from_raw_parts" && args.len() == 2 {
            if self.unsafe_depth == 0 {
                self.err(
                    "E0801",
                    "`slice_from_raw_parts` is unsafe; wrap in `unsafe { ... }`".to_string(),
                    span,
                );
            }
            let p_ty = self.check_expr(&args[0], None);
            let _ = self.check_expr(&args[1], Some(Ty::Usize));
            let elem = match &p_ty {
                Ty::RawPtr(inner) => (**inner).clone(),
                Ty::Error => return Some(Ty::Error),
                _ => {
                    self.err(
                        "E0302",
                        format!(
                            "`slice_from_raw_parts` first arg must be a raw pointer `*T`, got `{}`",
                            ty_display(&p_ty)
                        ),
                        args[0].span,
                    );
                    return Some(Ty::Error);
                }
            };
            return Some(Ty::Slice(Box::new(elem)));
        }
        if let Some(bswap_ty) = match name {
            "bswap16" | "htons" | "ntohs" => Some(Ty::U16),
            "bswap32" | "htonl" | "ntohl" => Some(Ty::U32),
            "bswap64" => Some(Ty::U64),
            _ => None,
        } {
            if args.len() != 1 {
                self.err(
                    "E0501",
                    format!("`{name}` takes exactly 1 argument, got {}", args.len()),
                    span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Some(Ty::Error);
            }
            let _ = self.check_expr(&args[0], Some(bswap_ty.clone()));
            return Some(bswap_ty);
        }
        None
    }

    /// v0.0.10 Phase 4: dispatch `#name(...)` intrinsics. Names are looked
    /// up in a hardcoded table; unknown names fire E0905. Each intrinsic
    /// implements its own arg-shape validation, then returns the result
    /// type. Codegen consults a parallel table.
    fn check_intrinsic(
        &mut self,
        name: &str,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        match name {
            // Phase 4A: `#selector("setBuffer:offset:atIndex:")` → `*u8`.
            // String-literal-only arg, no type args, no ret ascription.
            "selector" => self.check_intrinsic_selector(type_args, args, ret_ty, span),
            // Phase 4B: `#msg_send(recv, "selector", a, b, ...) -> RetTy`.
            // First arg is the receiver; second arg is a string-literal
            // selector; remaining args are forwarded. `RetTy` is required.
            "msg_send" => self.check_intrinsic_msg_send(type_args, args, ret_ty, span),
            // Phase 4C: `#compile_shader("path", "msl")` → `*[u8; N]`.
            // First arg is a string-literal path, second is the target.
            "compile_shader" => self.check_intrinsic_compile_shader(type_args, args, ret_ty, span),
            // v0.0.11 Phase 4: intrinsic-spelling migration. The five names
            // below were historically called as bare `#addr_of(x)` or
            // `!`-suffix macros (`include_bytes!`, `include_str!`, `env!`)
            // or as generic-fn-shaped calls (`#size_of::[T]()`,
            // `#align_of::[T]()`). Routed through the unified `#name` dispatch
            // so every compiler-known builtin shares one sigil.
            "addr_of" => self.check_intrinsic_addr_of(type_args, args, ret_ty, span),
            "include_bytes" => self.check_intrinsic_include_bytes(type_args, args, ret_ty, span),
            "include_str" => self.check_intrinsic_include_str(type_args, args, ret_ty, span),
            "env" => self.check_intrinsic_env(type_args, args, ret_ty, span),
            "size_of" => self.check_intrinsic_layout("size_of", type_args, args, ret_ty, span),
            "align_of" => self.check_intrinsic_layout("align_of", type_args, args, ret_ty, span),
            // v0.0.12 G-028 (llama.cplus G-026): `#zero::[T]()` returns a
            // value of type `T` with all bytes zeroed. Composes with
            // normal field-set syntax (`let mut x = #zero::[T](); x.a = 1;`)
            // so users can express C99-style partial init without leaking
            // garbage from `malloc`. Safe — no memory access, just an
            // alloca + memset at codegen.
            "zero" => self.check_intrinsic_zero(type_args, args, ret_ty, span),
            // v0.0.12 G-031 (llama.cplus G-030): `#cpu_relax()` — spin-loop
            // hint. Per-arch lowering at codegen (aarch64 `yield`, x86_64
            // `pause`, no-op elsewhere). No args, no type args, returns
            // unit. Correctness-irrelevant — without it spin-waits still
            // terminate; with it CPUs throttle pipeline + reduce power.
            // Safe — pure hint, no memory access.
            "cpu_relax" => self.check_intrinsic_cpu_relax(type_args, args, ret_ty, span),
            // `#asm(...)` never reaches here: the parser routes it to
            // `ExprKind::Asm` (Tier 1 + Tier 2), checked by `check_asm`.
            _ => {
                // FFI/raw + byte-swap builtins (`#str_ptr`, `#slice_ptr`,
                // `#bswap32`, …) share one handler with the (transitional)
                // bare-call path.
                if let Some(ty) = self.ffi_builtin_ty(name, args, span) {
                    return ty;
                }
                self.err(
                    "E0905",
                    format!("unknown compiler intrinsic `#{}`", name),
                    span,
                );
                // Still walk args / ret_ty so downstream diagnostics fire.
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                if let Some(rt) = ret_ty {
                    let _ = self.resolve_type(rt);
                }
                Ty::Error
            }
        }
    }

    // ---- Phase 4A: `#selector("name")` ----
    fn check_intrinsic_selector(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0903",
                format!(
                    "`#selector` takes no type arguments, got {}",
                    type_args.len()
                ),
                span,
            );
        }
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#selector` does not accept a `-> T` return-type ascription".to_string(),
                span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0903",
                format!(
                    "`#selector` takes exactly 1 string-literal argument, got {}",
                    args.len()
                ),
                span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::RawPtr(Box::new(Ty::U8));
        }
        // Sema-time validation: arg must be a string literal.
        let name = match &args[0].kind {
            ExprKind::StrLit(s) => s.clone(),
            _ => {
                self.err(
                    "E0903",
                    "`#selector` argument must be a string literal".to_string(),
                    args[0].span,
                );
                let _ = self.check_expr(&args[0], None);
                return Ty::RawPtr(Box::new(Ty::U8));
            }
        };
        // Record the selector name so codegen can emit the cached-pointer
        // global. (Codegen reads `selectors_table`; populated in 4A codegen.)
        self.selectors_table.insert(name);
        // Selector pointer is `*u8` (an opaque ObjC SEL pointer).
        Ty::RawPtr(Box::new(Ty::U8))
    }

    // ---- Phase 4B: `#msg_send(recv, "sel", args...) -> T` ----
    fn check_intrinsic_msg_send(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0903",
                "`#msg_send` takes no type arguments".to_string(),
                span,
            );
        }
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`#msg_send` is an unsafe FFI primitive; wrap the call in `unsafe { ... }`"
                    .to_string(),
                span,
            );
        }
        if args.len() < 2 {
            self.err("E0903",
                format!("`#msg_send` takes a receiver, a selector string literal, and zero or more arguments (got {})", args.len()),
                span);
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        // Arg 0: receiver expression (any type, ObjC objects are *u8 in C+).
        let _recv_ty = self.check_expr(&args[0], None);
        // Arg 1: string-literal selector.
        let sel = match &args[1].kind {
            ExprKind::StrLit(s) => s.clone(),
            _ => {
                self.err(
                    "E0903",
                    "second argument to `#msg_send` must be a string-literal selector name"
                        .to_string(),
                    args[1].span,
                );
                String::new()
            }
        };
        // Record the selector so codegen emits the global.
        if !sel.is_empty() {
            self.selectors_table.insert(sel);
        }
        // Remaining args: forwarded to objc_msgSend.
        for a in &args[2..] {
            let _ = self.check_expr(a, None);
        }
        // Record the call-site shape so codegen synthesizes the right
        // per-call objc_msgSend declaration.
        self.msg_send_shapes.insert(span);
        // Return type comes from the `-> T` ascription; default Unit.
        match ret_ty {
            Some(rt) => self.resolve_type(rt),
            None => Ty::Unit,
        }
    }

    /// v0.0.10 Phase 4C: invoke the shader toolchain at sema time and
    /// stash the resulting bytes in `shader_blobs_table`. The return type
    /// is `*[u8; N]` where N is the blob's byte count — parallels
    /// `include_bytes!`.
    fn compile_shader_at_sema(&mut self, path: &str, target: &str, span: ByteSpan) -> Ty {
        if target != "msl" {
            self.err("E0904",
                format!("`#compile_shader` target `{}` is not supported in this slice (only `\"msl\"` is implemented)", target),
                span);
            return Ty::Error;
        }
        // Resolve `path` relative to the current source file (mirrors
        // include_bytes' behaviour). For minimal viable scope, only the
        // entry file is consulted.
        let base = self
            .file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let shader_path = base.join(path);
        // Invoke xcrun + metallib via `Command::new`. Two-step:
        //   xcrun -sdk macosx metal -c <src> -o <tmp.air>
        //   xcrun -sdk macosx metallib <tmp.air> -o <tmp.metallib>
        // Errors from either step surface as E0904 with stderr text.
        let tmp_dir = std::env::temp_dir();
        let stem: String = shader_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("shader")
            .to_string();
        let air_path = tmp_dir.join(format!("{}.{:x}.air", stem, span.start));
        let metallib_path = tmp_dir.join(format!("{}.{:x}.metallib", stem, span.start));
        let step1 = std::process::Command::new("xcrun")
            .args(["-sdk", "macosx", "metal", "-c"])
            .arg(&shader_path)
            .arg("-o")
            .arg(&air_path)
            .output();
        let step1 = match step1 {
            Ok(o) => o,
            Err(e) => {
                self.err(
                    "E0904",
                    format!("failed to invoke `xcrun metal` for `{}`: {}", path, e),
                    span,
                );
                return Ty::Error;
            }
        };
        if !step1.status.success() {
            let stderr = String::from_utf8_lossy(&step1.stderr).into_owned();
            self.err(
                "E0904",
                format!(
                    "shader compilation failed for `{}`:\n{}",
                    path,
                    stderr.trim()
                ),
                span,
            );
            return Ty::Error;
        }
        let step2 = std::process::Command::new("xcrun")
            .args(["-sdk", "macosx", "metallib"])
            .arg(&air_path)
            .arg("-o")
            .arg(&metallib_path)
            .output();
        let step2 = match step2 {
            Ok(o) => o,
            Err(e) => {
                self.err(
                    "E0904",
                    format!("failed to invoke `xcrun metallib` for `{}`: {}", path, e),
                    span,
                );
                return Ty::Error;
            }
        };
        if !step2.status.success() {
            let stderr = String::from_utf8_lossy(&step2.stderr).into_owned();
            self.err(
                "E0904",
                format!("metallib failed for `{}`:\n{}", path, stderr.trim()),
                span,
            );
            return Ty::Error;
        }
        let bytes = match std::fs::read(&metallib_path) {
            Ok(b) => b,
            Err(e) => {
                self.err(
                    "E0904",
                    format!("could not read metallib output for `{}`: {}", path, e),
                    span,
                );
                return Ty::Error;
            }
        };
        let len = bytes.len() as u32;
        self.shader_blobs_table.insert(span, bytes);
        // Result type matches `include_bytes!`: `*[u8; N]`.
        Ty::RawPtr(Box::new(Ty::Array(Box::new(Ty::U8), len)))
    }

    // ---- Phase 4C: `#compile_shader("path", "msl")` ----
    fn check_intrinsic_compile_shader(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0903",
                "`#compile_shader` takes no type arguments".to_string(),
                span,
            );
        }
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#compile_shader` does not accept a `-> T` ascription".to_string(),
                span,
            );
        }
        if args.len() < 1 || args.len() > 2 {
            self.err("E0903",
                format!("`#compile_shader` takes 1 or 2 string-literal arguments (path, [target]); got {}", args.len()),
                span);
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let path = match &args[0].kind {
            ExprKind::StrLit(s) => s.clone(),
            _ => {
                self.err(
                    "E0903",
                    "first argument to `#compile_shader` must be a string-literal path".to_string(),
                    args[0].span,
                );
                return Ty::Error;
            }
        };
        let target = if args.len() == 2 {
            match &args[1].kind {
                ExprKind::StrLit(s) => s.clone(),
                _ => {
                    self.err("E0903",
                        "second argument to `#compile_shader` must be a string-literal target (\"msl\")".to_string(),
                        args[1].span);
                    return Ty::Error;
                }
            }
        } else {
            "msl".to_string()
        };
        // Sema-time invocation of the shader toolchain. Errors propagate
        // as E0904.
        self.compile_shader_at_sema(&path, &target, span)
    }

    // ---- v0.0.11 Phase 4: `#addr_of(x)` ----
    fn check_intrinsic_addr_of(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "`#addr_of` takes no type arguments, got {}",
                    type_args.len()
                ),
                span,
            );
        }
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#addr_of` does not accept a `-> T` return-type ascription".to_string(),
                span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!("`#addr_of` takes exactly 1 argument, got {}", args.len()),
                span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`#addr_of` is unsafe; wrap in `unsafe { ... }`".to_string(),
                span,
            );
        }
        let arg = &args[0];
        // v0.0.12 G-025: accept any place expression — `Ident`, `Field`,
        // `Index`, `Deref`, and chains thereof — so `#addr_of((*o).field)`
        // and `#addr_of(arr[i])` work without a bind-to-temporary dance.
        // Codegen reuses `gen_place`, which already lowers each of these
        // to the right GEP. Rejects call results, arithmetic, etc. —
        // those aren't places so taking their address is meaningless.
        if !is_addr_of_place(arg) {
            self.err(
                "E0302",
                "`#addr_of` argument must be a place expression — a bare \
                 identifier, a field access (`s.f`, `(*p).f`), an index \
                 (`a[i]`), or a deref (`*p`). Call results, arithmetic, \
                 and other temporaries are not addressable."
                    .to_string(),
                arg.span,
            );
            let _ = self.check_expr(arg, None);
            return Ty::Error;
        }
        let ty = self.check_expr(arg, None);
        if matches!(ty, Ty::Error) {
            return Ty::Error;
        }
        Ty::RawPtr(Box::new(ty))
    }

    // ---- v0.0.11 Phase 4: `#include_bytes("path")` ----
    fn check_intrinsic_include_bytes(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0903",
                format!(
                    "`#include_bytes` takes no type arguments, got {}",
                    type_args.len()
                ),
                span,
            );
        }
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#include_bytes` does not accept a `-> T` return-type ascription".to_string(),
                span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0903",
                format!(
                    "`#include_bytes` takes exactly 1 string-literal path, got {}",
                    args.len()
                ),
                span,
            );
            return Ty::Error;
        }
        let path = match &args[0].kind {
            ExprKind::StrLit(s) => s.clone(),
            _ => {
                self.err(
                    "E0871",
                    "`#include_bytes` argument must be a string literal".to_string(),
                    args[0].span,
                );
                return Ty::Error;
            }
        };
        self.check_include_bytes(&path, span)
    }

    // ---- v0.0.11 Phase 4: `#include_str("path")` ----
    fn check_intrinsic_include_str(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0903",
                format!(
                    "`#include_str` takes no type arguments, got {}",
                    type_args.len()
                ),
                span,
            );
        }
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#include_str` does not accept a `-> T` return-type ascription".to_string(),
                span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0903",
                format!(
                    "`#include_str` takes exactly 1 string-literal path, got {}",
                    args.len()
                ),
                span,
            );
            return Ty::Error;
        }
        let path = match &args[0].kind {
            ExprKind::StrLit(s) => s.clone(),
            _ => {
                self.err(
                    "E0871",
                    "`#include_str` argument must be a string literal".to_string(),
                    args[0].span,
                );
                return Ty::Error;
            }
        };
        self.check_include_str(&path, span)
    }

    // ---- v0.0.11 Phase 4: `#env("NAME")` ----
    fn check_intrinsic_env(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0903",
                format!("`#env` takes no type arguments, got {}", type_args.len()),
                span,
            );
        }
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#env` does not accept a `-> T` return-type ascription".to_string(),
                span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0903",
                format!(
                    "`#env` takes exactly 1 string-literal env-var name, got {}",
                    args.len()
                ),
                span,
            );
            return Ty::Error;
        }
        let name = match &args[0].kind {
            ExprKind::StrLit(s) => s.clone(),
            _ => {
                self.err(
                    "E0903",
                    "`#env` argument must be a string literal".to_string(),
                    args[0].span,
                );
                return Ty::Error;
            }
        };
        self.check_env_var(&name, span)
    }

    // ---- v0.0.11 Phase 4: `#size_of::[T]()` / `#align_of::[T]()` ----
    fn check_intrinsic_layout(
        &mut self,
        kind: &str,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if ret_ty.is_some() {
            self.err(
                "E0903",
                format!(
                    "`#{}` does not accept a `-> T` return-type ascription",
                    kind
                ),
                span,
            );
        }
        if type_args.len() != 1 {
            self.err(
                "E0501",
                format!(
                    "`#{}` takes exactly 1 type argument, got {}",
                    kind,
                    type_args.len()
                ),
                span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Usize;
        }
        if !args.is_empty() {
            self.err(
                "E0302",
                format!("`#{}` takes no value arguments, got {}", kind, args.len()),
                span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
        }
        let _ = self.resolve_type(&type_args[0]);
        Ty::Usize
    }

    // ---- v0.0.12 G-028: `#zero::[T]() -> T` ----
    //
    // A value of type `T` with every byte set to zero. Safe — no memory
    // access at the call site beyond writing to a fresh stack slot.
    // Composes with the regular field-set / index-write syntax to
    // express C99 `(T){.a = 1}` partial init without leaking garbage
    // from `malloc`.
    fn check_intrinsic_zero(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#zero` does not accept a `-> T` return-type ascription".to_string(),
                span,
            );
        }
        if type_args.len() != 1 {
            self.err(
                "E0501",
                format!(
                    "`#zero` takes exactly 1 type argument, got {}",
                    type_args.len()
                ),
                span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        if !args.is_empty() {
            self.err(
                "E0302",
                format!("`#zero` takes no value arguments, got {}", args.len()),
                span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
        }
        self.resolve_type(&type_args[0])
    }

    // ---- v0.0.12 G-031: `#cpu_relax() -> ()` ----
    //
    // Spin-loop hint. Lowers to `llvm.aarch64.hint(i32 1)` on aarch64
    // (the YIELD hint) or `llvm.x86.sse2.pause()` on x86_64. On other
    // targets it lowers to nothing (no instruction emitted). Safe;
    // correctness-irrelevant by design — a missing hint just wastes
    // power in tight spin loops, but never changes program output.
    fn check_intrinsic_cpu_relax(
        &mut self,
        type_args: &[Type],
        args: &[Expr],
        ret_ty: Option<&Type>,
        span: ByteSpan,
    ) -> Ty {
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "`#cpu_relax` takes no type arguments, got {}",
                    type_args.len()
                ),
                span,
            );
        }
        if ret_ty.is_some() {
            self.err(
                "E0903",
                "`#cpu_relax` does not accept a `-> T` return-type ascription".to_string(),
                span,
            );
        }
        if !args.is_empty() {
            self.err(
                "E0308",
                format!("`#cpu_relax` takes 0 arguments, got {}", args.len()),
                span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
        }
        Ty::Unit
    }

    // ---- v0.0.14 inline-asm Tier 1: `#asm("template") -> ()` ----
    //
    // A bare template-string inline-asm with no operands and no clobbers.
    // Lowers to `call void asm sideeffect "<template>", ""()`. Always
    // `sideeffect`, so it is never DCE'd (the whole point: fences, barriers,
    // serializing hints). Cannot read or write C+ values — that is Tier 2
    // (operands + clobbers), which needs an explicit operand-syntax design.
    //
    // Requires `unsafe` (E0801): inline asm can violate every invariant the
    // compiler relies on. Errors:
    //   - **E0501**: type arguments supplied.
    //   - **E0903**: a `-> T` return ascription (Tier 1 has no outputs).
    //   - **E0308**: not exactly one argument.
    //   - **E0871**: the argument is not a string literal.
    //   - **E0801**: used outside an `unsafe` block.
    /// v0.0.14 inline asm Tier 2. `#asm("tmpl {a},{b}", a = in(reg) x,
    /// b = out(reg) y, clobber("cc"))`. Tier 1 (`#asm("dmb ish")`) is the
    /// no-operand case. `#asm` is unsafe (E0801). Each operand: must have a
    /// matching `{name}` placeholder (E0893), no duplicate names (E0890), a
    /// register-sized scalar type (E0892); an `out`/`inout` operand must be a
    /// writable variable (E0895 otherwise; E0305 if not `mut`). The expression
    /// itself is `()` — outputs flow through the bound places, not the value.
    fn check_asm(
        &mut self,
        template: &str,
        operands: &[AsmOperand],
        _clobbers: &[String],
        span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`#asm` is unsafe; wrap it in `unsafe { ... }`".to_string(),
                span,
            );
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for op in operands {
            if !seen.insert(op.name.as_str()) {
                self.err(
                    "E0890",
                    format!("duplicate `#asm` operand name `{}`", op.name),
                    op.span,
                );
            }
            // A `reg` (compiler-chosen) operand must be referenced by `{name}`
            // so the template can name the register the compiler picked. An
            // explicit-register operand (`out("x0")`) may instead use the
            // register name directly in the template, so its placeholder is
            // optional.
            if matches!(op.reg, AsmReg::Any) {
                let placeholder = format!("{{{}}}", op.name);
                if !template.contains(&placeholder) {
                    self.err(
                        "E0893",
                        format!(
                            "`#asm` operand `{}` uses `reg` but has no `{{{}}}` placeholder in the template",
                            op.name, op.name
                        ),
                        op.span,
                    );
                }
            }
            let vty = match op.dir {
                AsmDir::In => self.check_expr(&op.value, None),
                AsmDir::Out | AsmDir::InOut => {
                    if let ExprKind::Ident(name) = &op.value.kind {
                        if let Some(info) = self.lookup_local(name).cloned() {
                            // Writing a previously-assigned binding requires
                            // `mut`; the first write to a `let x: T;` initializes
                            // it (allowed, like assignment). `inout` always reads
                            // first, so it needs an assigned, mutable binding.
                            if info.assigned && !info.mutable {
                                self.err(
                                    "E0305",
                                    format!(
                                        "`#asm` writes operand `{}` but `{}` is not declared `mut`",
                                        op.name, name
                                    ),
                                    op.span,
                                );
                            }
                            for scope in self.scopes.iter_mut().rev() {
                                if let Some(i) = scope.get_mut(name) {
                                    i.assigned = true;
                                    break;
                                }
                            }
                            info.ty
                        } else {
                            // Unknown name — let the normal resolver surface it.
                            self.check_expr(&op.value, None)
                        }
                    } else {
                        self.err(
                            "E0895",
                            format!(
                                "`#asm` `out`/`inout` operand `{}` must be a variable; general places (field/index) are not yet supported",
                                op.name
                            ),
                            op.span,
                        );
                        Ty::Error
                    }
                }
            };
            if !matches!(vty, Ty::Error) && !is_asm_scalar(&vty) {
                self.err(
                    "E0892",
                    format!(
                        "`#asm` operand `{}` has type `{}`; only integer, pointer, and `bool` operands fit a register",
                        op.name,
                        ty_display(&vty)
                    ),
                    op.span,
                );
            }
        }
        Ty::Unit
    }

    /// v0.0.8 Phase 4: `env!("NAME")` resolution. Reads the env var via
    /// `std::env::var` at sema time (cpc's own process environment).
    /// Errors:
    ///   - **E0876**: environment variable not set when cpc was invoked.
    /// On success, stashes the resolved value in `env_vars_table` keyed
    /// by call span; codegen reads from there to emit the global. Result
    /// type is `str` (the fat-pointer view over the value's bytes).
    fn check_env_var(&mut self, name: &str, span: ByteSpan) -> Ty {
        match std::env::var(name) {
            Ok(value) => {
                self.env_vars_table.insert(
                    span,
                    EnvVarEntry {
                        name: name.to_string(),
                        value,
                    },
                );
                Ty::Str
            }
            Err(_) => {
                self.err(
                    "E0876",
                    format!("environment variable `{name}` is not set at compile time"),
                    span,
                );
                Ty::Error
            }
        }
    }

    /// v0.0.6 Slice 1A: `include_bytes!("path")` resolution.
    ///
    /// Read the file at type-check time so we know its byte length N
    /// (the result type is `*const [u8; N]`). Errors:
    ///   - **E0870**: file not found at the resolved absolute path.
    ///   - **E0872**: file exceeds 64 MiB (sanity cap).
    ///
    /// E0871 (non-string-literal argument) fires at parse time — the
    /// parser only constructs `ExprKind::IncludeBytes` when the source
    /// matches `include_bytes!(StringLit)` exactly; any other form is a
    /// parse error before sema sees it.
    fn check_include_bytes(&mut self, path: &str, span: ByteSpan) -> Ty {
        let Some((abs_path, bytes)) = self.resolve_compile_time_blob(path, span, "include_bytes")
        else {
            return Ty::Error;
        };
        let len = bytes.len() as u32;
        self.compile_time_blobs_table
            .insert(span, CompileTimeBlobEntry { abs_path, bytes });
        Ty::RawPtr(Box::new(Ty::Array(Box::new(Ty::U8), len)))
    }

    /// v0.0.7 Slice 3.1: `include_str!("path")` resolution.
    ///
    /// Companion to `include_bytes!`. Same path resolution + same dedup
    /// table, but the bytes are UTF-8-validated at sema time and the
    /// returned type is `str` (the fat-pointer view). Errors:
    ///   - **E0870**: file not found (shared with `include_bytes!`).
    ///   - **E0872**: file exceeds 64 MiB (shared sanity cap).
    ///   - **E0875**: file contains invalid UTF-8; the message includes
    ///     the byte offset of the first bad byte.
    fn check_include_str(&mut self, path: &str, span: ByteSpan) -> Ty {
        let Some((abs_path, bytes)) = self.resolve_compile_time_blob(path, span, "include_str")
        else {
            return Ty::Error;
        };
        if let Err(e) = std::str::from_utf8(&bytes) {
            self.err(
                "E0875",
                format!(
                    "`#include_str` file `{}` is not valid UTF-8 (first invalid byte at offset {})",
                    abs_path.display(),
                    e.valid_up_to(),
                ),
                span,
            );
            return Ty::Error;
        }
        self.compile_time_blobs_table
            .insert(span, CompileTimeBlobEntry { abs_path, bytes });
        Ty::Str
    }

    /// Shared path resolution + read used by `include_bytes!` and
    /// `include_str!`. Returns `(canonicalized_path, bytes)` on success;
    /// emits E0870 (read error) or E0872 (size cap) and returns `None`
    /// on failure. The caller picks the result type and (for `str`)
    /// runs UTF-8 validation.
    fn resolve_compile_time_blob(
        &mut self,
        path: &str,
        span: ByteSpan,
        macro_name: &'static str,
    ) -> Option<(std::path::PathBuf, Vec<u8>)> {
        let base_dir: std::path::PathBuf =
            match self.current_file.as_ref().and_then(|f| self.files.get(f)) {
                Some(fc) => fc
                    .path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default(),
                None => self
                    .file
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default(),
            };
        let raw = std::path::PathBuf::from(path);
        let resolved = if raw.is_absolute() {
            raw
        } else {
            base_dir.join(&raw)
        };
        let abs_path = match resolved.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                self.err(
                    "E0870",
                    format!(
                        "`#{macro_name}` cannot find file `{}` (resolved to `{}`)",
                        path,
                        resolved.display(),
                    ),
                    span,
                );
                return None;
            }
        };
        let bytes = match std::fs::read(&abs_path) {
            Ok(b) => b,
            Err(e) => {
                self.err(
                    "E0870",
                    format!(
                        "`#{macro_name}` failed to read `{}`: {}",
                        abs_path.display(),
                        e,
                    ),
                    span,
                );
                return None;
            }
        };
        const MAX_INCLUDE_BYTES: usize = 64 * 1024 * 1024;
        if bytes.len() > MAX_INCLUDE_BYTES {
            self.err(
                "E0872",
                format!(
                    "`#{macro_name}` file `{}` is {} bytes; exceeds 64 MiB sanity limit",
                    abs_path.display(),
                    bytes.len(),
                ),
                span,
            );
            return None;
        }
        Some((abs_path, bytes))
    }

    fn check_array_lit(&mut self, elements: &[Expr], expected: Option<Ty>, span: ByteSpan) -> Ty {
        if elements.is_empty() {
            self.err(
                "E0332",
                "empty array literals not supported in Phase 2; provide at least one element"
                    .to_string(),
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
                    format!(
                        "mixed element types in array literal: expected `{}`, found `{}`",
                        first_ty.name(),
                        got.name()
                    ),
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
                    format!(
                        "array literal has {} element(s); expected {}",
                        len, declared_len
                    ),
                    span,
                );
                return Ty::Error;
            }
        }
        Ty::Array(Box::new(first_ty), len)
    }

    /// v0.0.11 Phase 3: type-check a fill-array literal `[EXPR; N]`.
    /// Element type comes from `EXPR` (or the expected array's element
    /// type if available). Length is N; if the expected array declared
    /// a different length, E0330. Returns `Ty::Array(elem, N)`.
    fn check_array_fill(
        &mut self,
        fill: &Expr,
        count: u32,
        expected: Option<Ty>,
        span: ByteSpan,
    ) -> Ty {
        let expected_elem: Option<Ty> = match &expected {
            Some(Ty::Array(elem, _)) => Some((**elem).clone()),
            _ => None,
        };
        let fill_ty = self.check_expr(fill, expected_elem);
        if let Some(Ty::Array(_, declared_len)) = &expected {
            if *declared_len != count {
                self.err(
                    "E0330",
                    format!(
                        "array literal has {} element(s); expected {}",
                        count, declared_len
                    ),
                    span,
                );
                return Ty::Error;
            }
        }
        Ty::Array(Box::new(fill_ty), count)
    }

    /// v0.0.5 Phase 3 Slice 3B: type-check a tuple literal `(a, b, ...)`.
    /// Resolves each element type (using the expected tuple element
    /// when known), synthesizes/looks up the matching tuple struct,
    /// and returns `Ty::Struct(id)`. Codegen sees TupleLit directly
    /// and lowers via `gen_tuple_lit` — no AST rewrite needed.
    fn check_tuple_lit(&mut self, elements: &[Expr], expected: Option<Ty>, span: ByteSpan) -> Ty {
        if elements.len() < 2 {
            self.err(
                "E0700",
                "tuple literal must have at least 2 elements (`()` is the unit value, `(x)` is grouping)".to_string(),
                span,
            );
            return Ty::Error;
        }
        // If `expected` is a tuple struct (its generic_origin marks it
        // as a synthesized tuple), feed per-element expectations.
        let expected_elem_tys: Vec<Option<Ty>> = match &expected {
            Some(Ty::Struct(id)) => {
                let def = &self.structs[id.0 as usize];
                match &def.generic_origin {
                    Some((name, args)) if name == "__Tuple" && args.len() == elements.len() => {
                        args.iter().map(|t| Some(t.clone())).collect()
                    }
                    _ => vec![None; elements.len()],
                }
            }
            _ => vec![None; elements.len()],
        };
        let elem_tys: Vec<Ty> = elements
            .iter()
            .zip(expected_elem_tys.iter())
            .map(|(e, exp)| self.check_expr(e, exp.clone()))
            .collect();
        if elem_tys.iter().any(|t| matches!(t, Ty::Error)) {
            return Ty::Error;
        }
        self.synthesize_tuple_struct(&elem_tys, span)
    }

    /// v0.0.5 Phase 3 Slice 3B: synthesize (or look up) a concrete
    /// tuple struct for the given element types. Fields are named
    /// `_0`, `_1`, ... in element order. Deduplicates against prior
    /// instantiations — `(i32, i32)` always resolves to the same
    /// struct id. Registered under `struct_instantiations` with the
    /// pseudo-template name `"__Tuple"` so monomorphize emits an AST
    /// struct item per unique tuple at codegen-handoff time.
    fn synthesize_tuple_struct(&mut self, elem_tys: &[Ty], _span: ByteSpan) -> Ty {
        let key = ("__Tuple".to_string(), elem_tys.to_vec());
        if let Some(&existing) = self.struct_instantiations.get(&key) {
            return Ty::Struct(existing);
        }
        let mangled = format!(
            "__tuple_{}",
            elem_tys
                .iter()
                .map(|t| mangle_ty_for_name(t, &self.structs, &self.enums))
                .collect::<Vec<_>>()
                .join("_")
        );
        let fields: Vec<(String, Ty, bool)> = elem_tys
            .iter()
            .enumerate()
            .map(|(i, t)| (format!("_{}", i), t.clone(), true))
            .collect();
        let id = StructId(self.structs.len() as u32);
        self.structs.push(StructDef {
            name: mangled.clone(),
            fields,
            methods: HashMap::new(),
            is_copy: false,
            is_drop: false,
            is_repr_c: false,
            origin_file: None,
            generic_origin: Some(("__Tuple".to_string(), elem_tys.to_vec())),
        });
        self.struct_by_name.insert(mangled.clone(), id);
        self.struct_instantiations.insert(key, id);
        Ty::Struct(id)
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
                        "indexing through a raw pointer is unsafe; wrap in `unsafe { ... }`"
                            .to_string(),
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

    fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        expected: Option<Ty>,
        span: ByteSpan,
    ) -> Ty {
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
        let variant_names: Vec<String> = self.enums[enum_id.0 as usize]
            .variants
            .iter()
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
        let mut merged_post: Option<Vec<Vec<(String, bool, BTreeSet<String>)>>> = None;
        // v0.0.15 flow-sensitive moves: arms are mutually exclusive, so a move
        // in one arm must not poison a binding for a sibling arm; and a move in
        // a *diverging* arm (the `guard let` else, an early `return`) must not
        // reach the code after the `match`. Snapshot the pre-match moved-state,
        // run each arm from it, and fold in only the fall-through arms' moves.
        let moved_pre = self.snapshot_moved();
        let mut nondiverging_arm_moves: Vec<Vec<Vec<(String, bool)>>> = Vec::new();

        for arm in arms {
            self.restore_moved(&moved_pre);
            self.scopes.push(HashMap::new());
            // Check the pattern: validate against the scrutinee's enum,
            // bind any payload names. Returns false if the pattern is
            // structurally invalid (errors emitted inline).
            self.check_pattern(
                &arm.pattern,
                enum_id,
                &enum_name,
                &mut covered,
                &mut has_catchall,
            );
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
                            format!(
                                "match arms produce different types: expected `{}`, found `{}`",
                                rt.name(),
                                arm_ty.name()
                            ),
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
            // v0.0.15: remember a fall-through arm's moves to fold in after the
            // match; a diverging arm's moves are dropped (they never reach past
            // the match). `arm_diverges` was computed above.
            if !arm_diverges {
                nondiverging_arm_moves.push(self.snapshot_moved());
            }
            self.restore_assigned(&pre_match);
        }

        // Exhaustiveness: every variant must be covered, or there must be
        // a catch-all wildcard / binding arm.
        if !has_catchall {
            let mut missing: Vec<String> = variant_names
                .iter()
                .filter(|n| !covered.contains_key(*n))
                .cloned()
                .collect();
            if !missing.is_empty() {
                missing.sort(); // deterministic for diagnostics
                let list = missing.join(", ");
                self.err(
                    "E0340",
                    format!(
                        "non-exhaustive `match` on enum `{}`: missing variant(s) {}",
                        enum_name, list
                    ),
                    span,
                );
            }
        }

        // Apply the merged post-match assigned-state. If there were no
        // arms (degenerate), keep pre-match state.
        if let Some(merged) = merged_post {
            self.restore_assigned(&merged);
        }
        // v0.0.15: apply the merged post-match moved-state — pre-match moves
        // plus every fall-through arm's moves (diverging arms excluded).
        self.restore_moved(&moved_pre);
        for snap in &nondiverging_arm_moves {
            self.union_moved(snap);
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
                    LocalInfo {
                        ty: Ty::Enum(enum_id),
                        mutable: false,
                        moved: false,
                        assigned: true,
                        borrow_roots: BTreeSet::new(),
                    },
                );
            }
            PatternKind::Variant {
                enum_name: pat_enum,
                type_args,
                variant_name,
                payload,
            } => {
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
                        format!(
                            "pattern type `{}` does not match scrutinee enum `{}`",
                            display, enum_name
                        ),
                        pat.span,
                    );
                    return;
                }
                // Look up the variant by name; capture its payload types.
                let variant_info = self.enums[enum_id.0 as usize]
                    .variants
                    .iter()
                    .find(|v| v.name == variant_name.name)
                    .cloned();
                let Some(vdef) = variant_info else {
                    self.err(
                        "E0317",
                        format!(
                            "enum `{}` has no variant `{}`",
                            enum_name, variant_name.name
                        ),
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
                            enum_name,
                            variant_name.name,
                            vdef.payload.len(),
                            payload.len()
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
                                LocalInfo {
                                    ty: pty.clone(),
                                    mutable: false,
                                    moved: false,
                                    assigned: true,
                                    borrow_roots: BTreeSet::new(),
                                },
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
            let sty =
                self.resolve_generic_instantiation(&enum_name.name, type_args, enum_name.span);
            let Ty::Struct(sid) = sty else {
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Error;
            };
            let mangled = self.structs[sid.0 as usize].name.clone();
            // First try the impl-block method on the instantiated struct.
            // If present, dispatch like any other `Type::method(...)` call.
            if self.structs[sid.0 as usize]
                .methods
                .contains_key(&variant.name)
            {
                let segments = vec![
                    Ident {
                        name: mangled,
                        span: enum_name.span,
                    },
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
            let module_prefix = enum_name
                .name
                .rsplit_once('.')
                .map(|(prefix, _)| prefix.to_string());
            let qualified_fn_name = match &module_prefix {
                Some(prefix) if !prefix.is_empty() => format!("{}.{}", prefix, variant.name),
                _ => variant.name.clone(),
            };
            if let Some(gsig) = self.fns_generic.get(&qualified_fn_name).cloned() {
                self.assoc_free_fn_dispatches
                    .insert(span, qualified_fn_name.clone());
                return self.check_generic_named_call(
                    &qualified_fn_name,
                    &gsig,
                    args,
                    type_args,
                    span,
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
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let ty =
            self.resolve_generic_enum_instantiation(&enum_name.name, type_args, enum_name.span);
        let Ty::Enum(id) = ty else {
            for a in args {
                let _ = self.check_expr(a, None);
            }
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
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let mangled = self.enums[id.0 as usize].name.clone();
        let segments = vec![
            Ident {
                name: mangled,
                span: enum_name.span,
            },
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
            for f in fields {
                let _ = self.check_expr(&f.value, None);
            }
            return Ty::Error;
        };
        let mangled = self.structs[id.0 as usize].name.clone();
        let mangled_ident = Ident {
            name: mangled,
            span: name.span,
        };
        self.check_struct_lit(&mangled_ident, fields, span)
    }

    fn check_struct_lit(&mut self, name: &Ident, fields: &[StructLitField], span: ByteSpan) -> Ty {
        let mut struct_id = self.struct_by_name.get(&name.name).copied();
        if struct_id.is_none() {
            let temp_ast_ty = crate::ast::Type {
                kind: crate::ast::TypeKind::Path(name.name.clone()),
                span: name.span,
            };
            let ty = self.resolve_type(&temp_ast_ty);
            match ty {
                Ty::Struct(id) => {
                    struct_id = Some(id);
                }
                Ty::Error => {
                    for f in fields {
                        let _ = self.check_expr(&f.value, None);
                    }
                    return Ty::Error;
                }
                _ => {
                    self.err("E0303", format!("unknown type `{}`", name.name), name.span);
                    for f in fields {
                        let _ = self.check_expr(&f.value, None);
                    }
                    return Ty::Error;
                }
            }
        }
        let id = struct_id.unwrap();
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
                    format!(
                        "duplicate field `{}` in literal of struct `{}`",
                        lit_field.name.name, struct_name
                    ),
                    lit_field.name.span,
                );
                let _ = self.check_expr(&lit_field.value, None);
                continue;
            }
            provided.insert(lit_field.name.name.clone(), ());
            let declared_field = declared.iter().find(|(n, _, _)| n == &lit_field.name.name);
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
                    let vty = self.check_expr(&lit_field.value, Some(t.clone()));
                    // v0.0.14 soundness: the value is moved into the new struct's
                    // field; moving a non-Copy field/index out of a Drop
                    // aggregate would double-free (E0509), same as let/return.
                    if vty != Ty::Error {
                        self.reject_partial_move_of_drop(&lit_field.value, &vty);
                    }
                }
                None => {
                    self.err(
                        "E0322",
                        format!(
                            "struct `{struct_name}` has no field `{}`",
                            lit_field.name.name
                        ),
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
            NumSuffix::F16 | NumSuffix::F32 | NumSuffix::F64 => {
                unreachable!("float suffix on int literal")
            }
        }
    }

    fn check_float_lit(&mut self, suffix: NumSuffix, expected: Option<Ty>) -> Ty {
        match suffix {
            NumSuffix::F16 => Ty::F16,
            NumSuffix::F32 => Ty::F32,
            NumSuffix::F64 => Ty::F64,
            NumSuffix::None => match expected {
                Some(Ty::F16) => Ty::F16,
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
                format!(
                    "invalid cast: `{}` cannot be cast to `{}`",
                    from.name(),
                    to.name()
                ),
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
        // v0.0.9 Phase 6 (cpc-gaps G-016): raw-pointer → integer cast.
        // The `cast_allowed` check above only admits 64-bit targets
        // (usize / u64 / isize / i64); a narrower target already fell
        // through to E0315. The remaining check is the unsafe gate:
        // pointer-as-integer crosses the type system and the borrow
        // checker has no visibility into what the integer is used for.
        if matches!(from, Ty::RawPtr(_))
            && matches!(to, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64)
            && self.unsafe_depth == 0
        {
            self.err(
                "E0801",
                "pointer-to-integer cast requires `unsafe { ... }`".to_string(),
                span,
            );
        }
        // Phase 11: raw-pointer → raw-pointer reinterpretation also requires
        // `unsafe`. The cast is mechanically free (both ends lower to LLVM
        // `ptr`), but the caller is asserting the reinterpreted bytes have
        // the new pointee's layout.
        if matches!(from, Ty::RawPtr(_))
            && matches!(to, Ty::RawPtr(_))
            && from != to
            && self.unsafe_depth == 0
        {
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
        for s in &b.stmts {
            self.check_stmt(s);
        }
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
        let moved_pre = self.snapshot_moved();
        let then_ty = self.check_block_as_expr(then);
        let after_then = self.snapshot_assigned();
        let moved_after_then = self.snapshot_moved();
        let then_diverges = block_diverges(then);
        self.restore_assigned(&pre_if);
        self.restore_moved(&moved_pre);
        let else_ty = match else_branch {
            Some(e) => match &e.kind {
                ExprKind::Block(b) => self.check_block_as_expr(b),
                ExprKind::If { .. } => self.check_expr(e, None),
                _ => Ty::Error,
            },
            None => Ty::Unit,
        };
        let after_else = self.snapshot_assigned();
        let moved_after_else = self.snapshot_moved();
        let else_diverges = else_branch.is_some_and(expr_diverges);
        let merged = self.intersect_assigned(&after_then, &after_else);
        self.restore_assigned(&merged);
        // v0.0.15 flow-sensitive moves: a binding is moved after the `if` iff it
        // was moved before, or moved by a branch that *falls through*. Moves in a
        // diverging branch (it `return`s / `break`s / `continue`s, so the code
        // after the `if` only runs when that branch was NOT taken) are dropped —
        // this is what stops the linear E0335 check firing on
        // `if done { return consume(x); } use(x);`.
        self.restore_moved(&moved_pre);
        if !then_diverges {
            self.union_moved(&moved_after_then);
        }
        if !else_diverges {
            self.union_moved(&moved_after_else);
        }
        if then_ty == Ty::Error || else_ty == Ty::Error {
            return Ty::Error;
        }
        if then_ty != else_ty {
            self.err(
                "E0302",
                format!(
                    "`if` and `else` branches have incompatible types: `{}` vs `{}`",
                    then_ty.name(),
                    else_ty.name()
                ),
                then.span,
            );
            return Ty::Error;
        }
        then_ty
    }

    /// Snapshot the assigned-state and borrow provenance of every binding
    /// currently in scope. Used for flow merging at `if`/`match` boundaries.
    fn snapshot_assigned(&self) -> Vec<Vec<(String, bool, BTreeSet<String>)>> {
        self.scopes
            .iter()
            .map(|scope| {
                scope
                    .iter()
                    .map(|(k, v)| (k.clone(), v.assigned, v.borrow_roots.clone()))
                    .collect()
            })
            .collect()
    }

    /// Restore each binding's flow state from a prior snapshot. The
    /// scope stack shape must match (same names per frame). Used to reset
    /// state before running a parallel control-flow branch.
    fn restore_assigned(&mut self, snap: &[Vec<(String, bool, BTreeSet<String>)>]) {
        for (frame, snap_frame) in self.scopes.iter_mut().zip(snap.iter()) {
            for (name, was_assigned, borrow_roots) in snap_frame {
                if let Some(info) = frame.get_mut(name) {
                    info.assigned = *was_assigned;
                    info.borrow_roots = borrow_roots.clone();
                }
            }
        }
    }

    /// Merge two flow-state snapshots: a binding is "assigned" iff it was
    /// assigned in BOTH inputs; borrow roots are unioned so any dangling path
    /// remains visible after the merge.
    fn intersect_assigned(
        &self,
        a: &[Vec<(String, bool, BTreeSet<String>)>],
        b: &[Vec<(String, bool, BTreeSet<String>)>],
    ) -> Vec<Vec<(String, bool, BTreeSet<String>)>> {
        a.iter()
            .zip(b.iter())
            .map(|(fa, fb)| {
                fa.iter()
                    .zip(fb.iter())
                    .map(|((name, av, ar), (_, bv, br))| {
                        let mut roots = ar.clone();
                        roots.extend(br.iter().cloned());
                        (name.clone(), *av && *bv, roots)
                    })
                    .collect()
            })
            .collect()
    }

    /// v0.0.15 flow-sensitive moves: snapshot each binding's `moved` flag,
    /// parallel to `snapshot_assigned`. Used to discard moves made in a
    /// diverging branch (one that ends in `return`/`break`/`continue`), so the
    /// linear E0335 check stops false-positiving on the common
    /// `if done { return consume(x); } use(x);` shape (e.g. the stdlib
    /// read loops that build a `Vec` and return it on EOF).
    fn snapshot_moved(&self) -> Vec<Vec<(String, bool)>> {
        self.scopes
            .iter()
            .map(|scope| scope.iter().map(|(k, v)| (k.clone(), v.moved)).collect())
            .collect()
    }

    /// Restore each binding's `moved` flag from a snapshot (scope shape must
    /// match — same names per frame, as for `restore_assigned`).
    fn restore_moved(&mut self, snap: &[Vec<(String, bool)>]) {
        for (frame, snap_frame) in self.scopes.iter_mut().zip(snap.iter()) {
            for (name, was_moved) in snap_frame {
                if let Some(info) = frame.get_mut(name) {
                    info.moved = *was_moved;
                }
            }
        }
    }

    /// OR the moves recorded in `snap` into the current state (set a binding
    /// `moved` if it is moved in `snap`). Used to fold a fall-through branch's
    /// moves into the post-`if`/post-`match` state.
    fn union_moved(&mut self, snap: &[Vec<(String, bool)>]) {
        for (frame, snap_frame) in self.scopes.iter_mut().zip(snap.iter()) {
            for (name, was_moved) in snap_frame {
                if *was_moved {
                    if let Some(info) = frame.get_mut(name) {
                        info.moved = true;
                    }
                }
            }
        }
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        // Slice 11.FN_PTR: when the callee is an Ident bound to a local
        // of FnPtr type, this is an indirect call. Validate args against
        // the pointer's param types, return the pointer's return type.
        // Falls through to the named-call dispatch when the Ident is a
        // fn name (or unknown — that path emits E0300).
        if let ExprKind::Ident(name) = &callee.kind {
            if let Some(info) = self.lookup_local(name) {
                if let Ty::FnPtr {
                    params,
                    return_type,
                } = info.ty.clone()
                {
                    if !type_args.is_empty() {
                        self.err(
                            "E0501",
                            "indirect calls through a fn-pointer do not accept type arguments"
                                .to_string(),
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
                        for a in args {
                            let _ = self.check_expr(a, None);
                        }
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
                    if let Ty::FnPtr {
                        params,
                        return_type,
                    } = ft
                    {
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
                            for a in args {
                                let _ = self.check_expr(a, None);
                            }
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
                    "callee must be a function name, a method, or a `Type::function` path"
                        .to_string(),
                    callee.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                Ty::Error
            }
        }
    }

    fn check_named_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        let ExprKind::Ident(name) = &callee.kind else {
            unreachable!();
        };
        // Slice 7GEN.5a: dispatch generic fns through inference.
        // Slice 7GEN.5b: when type_args are explicit, use them directly.
        if let Some(gsig) = self.fns_generic.get(name).cloned() {
            return self.check_generic_named_call(name, &gsig, args, type_args, call_span);
        }
        // v0.0.3 Phase 5 Slice 5B: thread spawn/join intrinsics. Placed
        // before the "non-generic fn with turbofish" reject because both
        // intrinsics take one type-argument by design (mirroring size_of's
        // shape) — they're compiler-known and don't appear in `fns_generic`.
        if name == "__cplus_thread_spawn" || name == "__cplus_thread_join" {
            return self.check_thread_intrinsic(name, callee, args, type_args, call_span);
        }
        // v0.0.5 Phase 1C: `__cplus_drop_in_place::[T](p: *T)` — drop the
        // value at *p in place. Compiler lowers to a call to `T::drop(p)`
        // for the monomorphized T, or to a no-op when T has no Drop. Used
        // by stdlib containers to invoke inner-T Drop before freeing
        // their storage. Unsafe (raw-pointer write semantics).
        if name == "__cplus_drop_in_place" {
            if type_args.len() != 1 {
                self.err(
                    "E0501",
                    format!(
                        "`__cplus_drop_in_place` takes exactly 1 type argument, got {}",
                        type_args.len()
                    ),
                    callee.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Unit;
            }
            if args.len() != 1 {
                self.err(
                    "E0308",
                    format!(
                        "`__cplus_drop_in_place` takes 1 value argument, got {}",
                        args.len()
                    ),
                    call_span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Unit;
            }
            if self.unsafe_depth == 0 {
                self.err(
                    "E0801",
                    "`__cplus_drop_in_place` is unsafe; wrap in `unsafe { ... }`".to_string(),
                    call_span,
                );
            }
            let target_ty = self.resolve_type(&type_args[0]);
            let _ = self.check_expr(&args[0], Some(Ty::RawPtr(Box::new(target_ty))));
            return Ty::Unit;
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
        // v0.0.4 Phase 3 Slice 3A.1: reactor-suspend intrinsics. Each
        // requires `unsafe { ... }` and must appear inside an async fn
        // body — the suspend needs an enclosing coroutine to suspend.
        if name == "__cplus_reactor_wait_read" {
            return self.check_reactor_wait_read(callee, args, type_args, call_span);
        }
        if name == "__cplus_reactor_wait_write" {
            return self.check_reactor_wait_write(callee, args, type_args, call_span);
        }
        if name == "__cplus_reactor_wait_timer" {
            return self.check_reactor_wait_timer(callee, args, type_args, call_span);
        }
        if name == "__cplus_reactor_spawn_local" {
            return self.check_reactor_spawn_local(callee, args, type_args, call_span);
        }
        if name == "__cplus_reactor_yield_now" {
            return self.check_reactor_yield_now(callee, args, type_args, call_span);
        }
        // Non-generic fn with turbofish → reject. The user explicitly
        // asked to instantiate something that has no generic params.
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "function `{}` takes no type arguments but {} were provided",
                    name,
                    type_args.len()
                ),
                callee.span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        // v0.0.16: `#println`, the FFI/raw + byte-swap builtins are `#name(...)` intrinsics
        // now (one sigil for every compiler-known builtin). A bare call is a
        // migration error with a fix-it; `#name(...)` is type-checked by
        // `check_intrinsic` -> `ffi_builtin_ty`.
        if is_ffi_builtin_name(name) {
            self.err(
                "E0905",
                format!("`{name}` is a compiler intrinsic — spell it `#{name}(...)`"),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
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
                    format!(
                        "`{}` takes {} argument(s), got {}",
                        name,
                        expected_args,
                        args.len()
                    ),
                    call_span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return if spec.returns_value() {
                    spec.ty.clone()
                } else {
                    Ty::Unit
                };
            }
            let ptr_ty = Ty::RawPtr(Box::new(spec.ty.clone()));
            let p_actual = self.check_expr(&args[0], Some(ptr_ty.clone()));
            if !matches!(p_actual, Ty::RawPtr(_) | Ty::Error) {
                self.err(
                    "E0302",
                    format!(
                        "`{}` first argument must be `*{}`, got `{}`",
                        name,
                        spec.ty.name(),
                        ty_display(&p_actual)
                    ),
                    args[0].span,
                );
            } else if let Ty::RawPtr(inner) = &p_actual {
                if **inner != spec.ty {
                    self.err(
                        "E0302",
                        format!(
                            "`{}` first argument must be `*{}`, got `{}`",
                            name,
                            spec.ty.name(),
                            ty_display(&p_actual)
                        ),
                        args[0].span,
                    );
                }
            }
            for a in args.iter().skip(1) {
                let _ = self.check_expr(a, Some(spec.ty.clone()));
            }
            return if spec.returns_value() {
                spec.ty.clone()
            } else {
                Ty::Unit
            };
        }
        // v0.0.12 G-030 (llama.cplus G-029): standalone memory fence
        // `__cplus_atomic_fence_<ord>()`. No type, no operand — just a
        // barrier on the sequenced-before/happens-before edges of the
        // surrounding atomic ops. Same unsafe requirement as the typed
        // atomic ops (it influences other unsafe-gated atomic accesses).
        if crate::atomic::parse_atomic_fence(name).is_some() {
            if self.unsafe_depth == 0 {
                self.err(
                    "E0801",
                    format!("`{}` is unsafe; wrap in `unsafe {{ ... }}`", name),
                    call_span,
                );
            }
            if !args.is_empty() {
                self.err(
                    "E0308",
                    format!("`{}` takes 0 arguments, got {}", name, args.len()),
                    call_span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
            }
            return Ty::Unit;
        }
        let Some(sig) = self.fns.get(name).cloned() else {
            self.err("E0300", format!("undefined function `{name}`"), callee.span);
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        };
        // Slice 10.FFI.3: extern fn calls require `unsafe { ... }`.
        // The callee's contract is unverified — it may have arbitrary
        // side effects, return uninitialized memory, etc.
        if self.extern_fns.contains(name) && self.unsafe_depth == 0 {
            self.err(
                "E0801",
                format!(
                    "calling extern fn `{}` is unsafe; wrap in `unsafe {{ ... }}`",
                    name
                ),
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
                    format!(
                        "variadic function `{}` requires at least {} fixed argument(s), got {}",
                        name,
                        sig.params.len(),
                        args.len()
                    ),
                    call_span,
                );
            }
        } else if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!(
                    "function `{}` takes {} argument(s), got {}",
                    name,
                    sig.params.len(),
                    args.len()
                ),
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
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!("`{}` takes 1 value argument, got {}", name, args.len()),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
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
            let expected_f = Ty::FnPtr {
                params: vec![],
                return_type: Box::new(o_ty.clone()),
            };
            let _f_ty = self.check_expr(&args[0], Some(expected_f));
            return self.instantiate_struct_from_arg_tys("JoinHandle", &template, vec![o_ty]);
        }
        // __cplus_thread_join
        let expected_h =
            self.instantiate_struct_from_arg_tys("JoinHandle", &template, vec![o_ty.clone()]);
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
                format!(
                    "`__cplus_block_on` takes 1 type argument, got {}",
                    type_args.len()
                ),
                callee.span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!(
                    "`__cplus_block_on` takes 1 value argument, got {}",
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let t_ty = self.resolve_type(&type_args[0]);
        let expected_future = self.wrap_in_future(&t_ty, call_span);
        if matches!(expected_future, Ty::Error) {
            return Ty::Error;
        }
        let _ = self.check_expr(&args[0], Some(expected_future));
        t_ty
    }

    /// v0.0.4 Phase 3 Slice 3A.1: type-check
    /// `__cplus_reactor_wait_read(fd: i32)`. Single i32 arg, returns
    /// Unit. Must appear inside an `async fn` body (we need a coroutine
    /// to suspend) and inside `unsafe { ... }` (it's an FFI-shaped
    /// intrinsic that the compiler can't safety-check beyond shape).
    fn check_reactor_wait_read(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`__cplus_reactor_wait_read` is unsafe; wrap in `unsafe { ... }`".to_string(),
                call_span,
            );
        }
        if !self.current_fn_is_async {
            self.err(
                "E0901",
                "`__cplus_reactor_wait_read` is only valid inside an `async fn` body".to_string(),
                call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                "`__cplus_reactor_wait_read` takes 0 type arguments".to_string(),
                callee.span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!(
                    "`__cplus_reactor_wait_read` takes 1 value argument, got {}",
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let _ = self.check_expr(&args[0], Some(Ty::I32));
        Ty::Unit
    }

    /// v0.0.4 Phase 3 Slice 3A.3: type-check
    /// `__cplus_reactor_wait_write(fd: i32)`. Same shape as wait_read.
    fn check_reactor_wait_write(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`__cplus_reactor_wait_write` is unsafe; wrap in `unsafe { ... }`".to_string(),
                call_span,
            );
        }
        if !self.current_fn_is_async {
            self.err(
                "E0901",
                "`__cplus_reactor_wait_write` is only valid inside an `async fn` body".to_string(),
                call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                "`__cplus_reactor_wait_write` takes 0 type arguments".to_string(),
                callee.span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!(
                    "`__cplus_reactor_wait_write` takes 1 value argument, got {}",
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let _ = self.check_expr(&args[0], Some(Ty::I32));
        Ty::Unit
    }

    /// v0.0.5 Phase 4 Slice 4A: `__cplus_reactor_wait_timer(ms: u64)`.
    /// Registers a kqueue EVFILT_TIMER for `ms` milliseconds, then
    /// suspends self. Reactor wakes us when the timer fires. Same
    /// unsafe + async-only gates as wait_read / wait_write.
    fn check_reactor_wait_timer(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`__cplus_reactor_wait_timer` is unsafe; wrap in `unsafe { ... }`".to_string(),
                call_span,
            );
        }
        if !self.current_fn_is_async {
            self.err(
                "E0901",
                "`__cplus_reactor_wait_timer` is only valid inside an `async fn` body".to_string(),
                call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                "`__cplus_reactor_wait_timer` takes 0 type arguments".to_string(),
                callee.span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!(
                    "`__cplus_reactor_wait_timer` takes 1 value argument, got {}",
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let _ = self.check_expr(&args[0], Some(Ty::U64));
        Ty::Unit
    }

    /// v0.0.4 Phase 3 Slice 3A.2: type-check
    /// `__cplus_reactor_spawn_local(future: Future[T])`. Pushes the
    /// future onto the reactor's task queue. Returns Unit.
    fn check_reactor_spawn_local(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`__cplus_reactor_spawn_local` is unsafe; wrap in `unsafe { ... }`".to_string(),
                call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                "`__cplus_reactor_spawn_local` takes 0 type arguments".to_string(),
                callee.span,
            );
        }
        if args.len() != 1 {
            self.err(
                "E0308",
                format!(
                    "`__cplus_reactor_spawn_local` takes 1 value argument, got {}",
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        // Accept any Future[T] type. Sema doesn't enforce the specific
        // T; the runtime treats handles uniformly.
        let _ = self.check_expr(&args[0], None);
        Ty::Unit
    }

    /// v0.0.4 Phase 3 Slice 3A.2: type-check `__cplus_reactor_yield_now()`.
    /// Zero args, zero type args, returns Unit. Must be inside async fn.
    fn check_reactor_yield_now(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        type_args: &[Type],
        call_span: ByteSpan,
    ) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(
                "E0801",
                "`__cplus_reactor_yield_now` is unsafe; wrap in `unsafe { ... }`".to_string(),
                call_span,
            );
        }
        if !self.current_fn_is_async {
            self.err(
                "E0901",
                "`__cplus_reactor_yield_now` is only valid inside an `async fn` body".to_string(),
                call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                "`__cplus_reactor_yield_now` takes 0 type arguments".to_string(),
                callee.span,
            );
        }
        if !args.is_empty() {
            self.err(
                "E0308",
                format!(
                    "`__cplus_reactor_yield_now` takes 0 value arguments, got {}",
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
        }
        Ty::Unit
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
                format!(
                    "`__cplus_thread_spawn_with` takes 2 type arguments, got {}",
                    type_args.len()
                ),
                callee.span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        if args.len() != 2 {
            self.err(
                "E0308",
                format!(
                    "`__cplus_thread_spawn_with` takes 2 value arguments, got {}",
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        let i_ty = self.resolve_type(&type_args[0]);
        let o_ty = self.resolve_type(&type_args[1]);
        let _input = self.check_arg_with_move(
            &args[0],
            &ParamSig {
                ty: i_ty.clone(),
                mutable: false,
                move_: true,
                borrow_: false,
            },
        );
        let expected_f = Ty::FnPtr {
            params: vec![i_ty.clone()],
            return_type: Box::new(o_ty.clone()),
        };
        let _f = self.check_expr(&args[1], Some(expected_f));
        let Some(template) = self.struct_generic_templates.get("JoinHandle").cloned() else {
            self.err(
                "E0300",
                "`__cplus_thread_spawn_with` requires `JoinHandle[O]` from `stdlib/thread`"
                    .to_string(),
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
                format!(
                    "function `{}` takes {} argument(s), got {}",
                    name,
                    gsig.params.len(),
                    args.len()
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
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
                        name,
                        gsig.generic_params.len(),
                        type_args.len()
                    ),
                    call_span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
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
                            name,
                            ty_display(&expected),
                            actual_before.name()
                        ),
                        arg.span,
                    );
                    had_err = true;
                }
                // v0.0.14 soundness: consume non-Copy args, matching the
                // non-generic path. A `move` param OR an implicit-move (bare)
                // param consumes a *place* arg — a whole-binding Ident moves;
                // a Field/Index/Deref projection is a partial move and is
                // rejected (E0337 via consume_arg_place). Rvalues (struct/enum
                // literals, fresh call results — e.g.
                // `io_ok::[File](File { fd: fd })`) aren't places, so they own
                // their value outright and pass through untouched.
                let implicit_move = !param.mutable && !param.borrow_;
                if (param.move_ || implicit_move)
                    && !self.is_copy(&expected)
                    && !matches!(actual_before, Ty::Error)
                    && is_addr_of_place(arg)
                {
                    self.consume_arg_place(arg);
                }
            }
            if had_err {
                return Ty::Error;
            }
            // Slice 7GEN.5e step 4: bound check at the turbofish path.
            self.check_generic_bounds(
                &gsig.generic_params,
                &gsig.bounds,
                &concrete_args,
                call_span,
                &format!("function `{}`", name),
            );
            self.fn_instantiations
                .insert((name.to_string(), concrete_args.clone()));
            self.call_monos.insert(call_span, concrete_args.clone());
            return self.subst_ty_deep(&gsig.return_type, &subst);
        }
        // Infer concrete types per param position, then unify.
        let mut had_err = false;
        // First pass: check args without an expected type to get their
        // natural type, then unify against the generic param type.
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr(a, None)).collect();
        for (param, arg_ty) in gsig.params.iter().zip(arg_tys.iter()) {
            if matches!(arg_ty, Ty::Error) {
                had_err = true;
                continue;
            }
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
        if had_err {
            return Ty::Error;
        }
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
        self.fn_instantiations
            .insert((name.to_string(), concrete_args.clone()));
        self.call_monos.insert(call_span, concrete_args.clone());
        // Substitute the return type and return it as the call's type.
        self.subst_ty_deep(&gsig.return_type, &subst)
    }

    fn check_method_call(
        &mut self,
        receiver: &Expr,
        name: &Ident,
        type_args: &[Type],
        args: &[Expr],
        call_span: ByteSpan,
    ) -> Ty {
        let recv_ty = self.check_expr(receiver, None);
        if recv_ty == Ty::Error {
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        // v0.0.12 realtime Phase 1 (method-dispatch hole): if the enclosing
        // function is `#[no_alloc]` / `#[no_block]`, the dispatched method must
        // carry the same contract. The receiver type is resolved now, so this
        // picks the *actual* method (not a name-collision guess). User struct /
        // generic / enum methods live in `method_contracts`; blessed/builtin
        // receivers (string / SIMD / raw-ptr / iterator) are not in the map and
        // are handled at their own sites below (e.g. `to_string`).
        self.check_method_contract(&recv_ty, &name.name, call_span);
        // Phase 8 slice 8.STR.3: blessed methods on owned `string`.
        if matches!(recv_ty, Ty::String) {
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    "blessed `string` methods take no type arguments".to_string(),
                    call_span,
                );
            }
            return self.check_string_method_call(name, args, call_span);
        }
        // v0.0.6 Slice 1B: blessed methods on SIMD vector receivers.
        // v0.0.9 follow-up: also catch `Ty::Mask` receivers — the
        // method body checks `is_mask` to route Simd-only ops
        // (arithmetic, splat) and Mask-only ops (select, any, all)
        // appropriately.
        if matches!(&recv_ty, Ty::Simd { .. } | Ty::Mask { .. }) {
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    "SIMD method calls take no type arguments".to_string(),
                    call_span,
                );
            }
            return self.check_simd_method_call(&recv_ty, name, args, call_span);
        }
        // Phase 8 slice 8.STR.6: blessed `to_string()` on every primitive
        // + `str`. Returns `string` (owned). User-defined structs hit
        // the normal method-lookup below; if they provide
        // `impl ToString for Foo { fn to_string(self) -> string }`, that
        // path handles them.
        if name.name == "to_string"
            && args.is_empty()
            && Self::is_blessed_to_string_receiver(&recv_ty)
        {
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    "`to_string` takes no type arguments".to_string(),
                    call_span,
                );
            }
            // v0.0.12 realtime Phase 1: blessed `to_string()` allocates an
            // owned `string`, so it's banned in a `#[no_alloc]` body.
            if self.current_fn_no_alloc {
                self.err(
                    "E0901",
                    "function is marked `#[no_alloc]` but calls `to_string()`, which heap-allocates".to_string(),
                    call_span,
                );
            }
            return Ty::String;
        }
        // v0.0.12 G-045 (llama.cplus): blessed `to_bits()` on a float scalar —
        // bit-preserving reinterpret to the same-width unsigned int (LLVM
        // `bitcast`). Pairs with `fN::from_bits(uN)`. Safe; no allocation.
        if name.name == "to_bits" && args.is_empty() && recv_ty.is_float() {
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    "`to_bits` takes no type arguments".to_string(),
                    call_span,
                );
            }
            return match recv_ty {
                Ty::F16 => Ty::U16,
                Ty::F32 => Ty::U32,
                Ty::F64 => Ty::U64,
                _ => Ty::Error,
            };
        }
        // v0.0.4 Phase 4 Slice 4B: blessed `next()` on `Iterator[T]`
        // receiver — returns `Option[T]`. The method has no source-level
        // body (codegen lowers inline via coro.done + coro.resume +
        // coro.promise). Sema enforces the shape: 0 args, 0 type args,
        // receiver is some Iterator[T].
        if name.name == "next" && args.is_empty() {
            if let Some(elem) = self.unwrap_iterator(&recv_ty) {
                if !type_args.is_empty() {
                    self.err(
                        "E0501",
                        "`Iterator::next` takes no type arguments".to_string(),
                        call_span,
                    );
                }
                return self.instantiate_option(&elem, call_span);
            }
        }
        // v0.0.4 Phase 3 Slice 3B.5: blessed `hash()` on every primitive
        // hashable receiver. Returns u64. Generic HashMap[K, V] calls
        // `k.hash()` in its body; after K is monomorphized to a primitive
        // type the blessed path produces a real hash. User structs hit
        // the normal method-lookup below (they must provide `impl Hash`).
        if name.name == "hash" && args.is_empty() && Self::is_blessed_hash_receiver(&recv_ty) {
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    "`hash` takes no type arguments".to_string(),
                    call_span,
                );
            }
            return Ty::U64;
        }
        // v0.0.4 Phase 3 Slice 3B.5: blessed `eq(other)` on primitive
        // receivers — lowers to the same memcmp/icmp shape as `==`.
        // Generic HashMap[K, V]'s probe loop uses `k.eq(stored)` so
        // monomorphized code over user K can use their impl Eq while
        // monomorphized code over primitive K uses the blessed lowering.
        if name.name == "eq" && args.len() == 1 && Self::is_blessed_eq_receiver(&recv_ty) {
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    "`eq` takes no type arguments".to_string(),
                    call_span,
                );
            }
            let _ = self.check_expr(&args[0], Some(recv_ty.clone()));
            return Ty::Bool;
        }
        // v0.0.12 G-024: blessed `is_null()` / `is_not_null()` on raw-pointer
        // receivers. Lowers to a single `icmp eq ptr %p, null` (and its
        // inverse). No memory access — just inspecting the bit pattern —
        // so neither method requires `unsafe`. Closes the C `if (p == NULL)`
        // ergonomic gap without needing a `#null` intrinsic or special-case
        // sugar.
        if matches!(recv_ty, Ty::RawPtr(_))
            && args.is_empty()
            && (name.name == "is_null" || name.name == "is_not_null")
        {
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    format!("`{}` takes no type arguments", name.name),
                    call_span,
                );
            }
            return Ty::Bool;
        }
        // v0.0.12 G-028 (llama.cplus G-026): blessed `write_zeroed()` on a
        // raw-pointer receiver — zero the *T-many bytes* the pointer
        // refers to. Companion to `#zero::[T]()` for the through-pointer
        // case (e.g. just after `malloc(#size_of::[T]())`). Unsafe
        // because we're writing through a raw pointer the borrow checker
        // can't reason about; sema enforces the unsafe block (E0801).
        if let Ty::RawPtr(_inner) = &recv_ty {
            if args.is_empty() && name.name == "write_zeroed" {
                if !type_args.is_empty() {
                    self.err(
                        "E0501",
                        "`write_zeroed` takes no type arguments".to_string(),
                        call_span,
                    );
                }
                if self.unsafe_depth == 0 {
                    self.err(
                        "E0801",
                        "`write_zeroed` is unsafe (writes through a raw pointer); wrap in `unsafe { ... }`".to_string(),
                        call_span,
                    );
                }
                return Ty::Unit;
            }
        }
        // v0.0.5 Phase 2C: dispatch on enum receivers via the new
        // `EnumDef::methods` table. Same shape as the struct path —
        // method lookup + receiver-mutability + arg checks.
        if let Ty::Enum(eid) = recv_ty {
            let enum_name = self.enums[eid.0 as usize].name.clone();
            let Some(sig) = self.enums[eid.0 as usize].methods.get(&name.name).cloned() else {
                self.err(
                    "E0324",
                    format!("no method `{}` on enum `{}`", name.name, enum_name),
                    name.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Error;
            };
            return self
                .check_enum_method_call(eid, &enum_name, name, &sig, args, call_span, receiver);
        }
        // v0.0.5: method call on a generic-parameter receiver — `t.cmp(o)`
        // where `t: T` and the enclosing fn declared `T: Ord`. Resolve via
        // the bound interface's method table. Each arg is type-checked
        // against the interface signature (with `Self` substituted to the
        // receiver's `Ty::Param(name)`). Codegen then handles the dispatch
        // through monomorphization — each instantiation calls the concrete
        // impl's method directly.
        if let Ty::Param(ref pname) = recv_ty {
            if let Some(msig) = self.lookup_bound_method(pname, &name.name) {
                // Substitute `Self` → `Ty::Param(pname)` in the interface
                // method signature so arg-type checks match what the user
                // wrote (`other: T` not `other: Self`).
                let mut subst = HashMap::new();
                subst.insert("Self".to_string(), recv_ty.clone());
                let mut arg_idx = 0;
                for psig in &msig.params {
                    if arg_idx >= args.len() {
                        break;
                    }
                    let want = self.subst_ty_deep(&psig.ty, &subst);
                    let _ = self.check_expr(&args[arg_idx], Some(want));
                    arg_idx += 1;
                }
                // Drain any extra args (sema-error fallback) so type
                // checking still walks them.
                for a in &args[arg_idx..] {
                    let _ = self.check_expr(a, None);
                }
                return self.subst_ty_deep(&msig.return_type, &subst);
            }
        }
        let Ty::Struct(id) = recv_ty else {
            self.err(
                "E0324",
                format!("no method `{}` on type `{}`", name.name, recv_ty.name()),
                name.span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        };
        let struct_name = self.structs[id.0 as usize].name.clone();
        let Some(sig) = self.structs[id.0 as usize].methods.get(&name.name).cloned() else {
            self.err(
                "E0324",
                format!("no method `{}` on struct `{}`", name.name, struct_name),
                name.span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        };
        let Some(rcv) = sig.receiver else {
            self.err(
                "E0327",
                format!(
                    "`{}::{}` is an associated function; call it as `{}::{}(...)`",
                    struct_name, name.name, struct_name, name.name
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        };
        if matches!(rcv, Receiver::Mut) && !self.is_writable_place_quiet(receiver) {
            self.err(
                "E0328",
                format!(
                    "method `{}::{}` requires a mutable receiver",
                    struct_name, name.name
                ),
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
                format!(
                    "method `{}::{}` takes {} argument(s), got {}",
                    struct_name,
                    name.name,
                    sig.params.len(),
                    args.len()
                ),
                call_span,
            );
        }
        // Slice 7GEN.5e: generic-method dispatch. Non-generic methods
        // (most of them) fall through to plain arg-by-arg type checking;
        // generic methods route through inference + monomorphization
        // bookkeeping.
        if !sig.generic_params.is_empty() {
            return self.check_generic_method_call(
                id,
                &struct_name,
                name,
                &sig,
                type_args,
                args,
                call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "method `{}::{}` takes no type arguments but {} were provided",
                    struct_name,
                    name.name,
                    type_args.len()
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

    /// v0.0.5 Phase 2C: type-check a method call on an enum receiver.
    /// Mirrors the struct path (`check_method_call`'s tail) but uses
    /// `EnumDef::methods` for sig lookup. Generic-method dispatch on
    /// enums isn't supported yet (no `impl Option[T] { fn map[U](...) }`);
    /// that requires the generic-enum impl synthesis still pending.
    fn check_enum_method_call(
        &mut self,
        enum_id: EnumId,
        enum_name: &str,
        name: &Ident,
        sig: &MethodSig,
        args: &[Expr],
        call_span: ByteSpan,
        receiver: &Expr,
    ) -> Ty {
        let Some(rcv) = sig.receiver else {
            self.err(
                "E0327",
                format!(
                    "`{}::{}` is an associated function; call it as `{}::{}(...)`",
                    enum_name, name.name, enum_name, name.name
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        };
        if matches!(rcv, Receiver::Mut) && !self.is_writable_place_quiet(receiver) {
            self.err(
                "E0328",
                format!(
                    "method `{}::{}` requires a mutable receiver",
                    enum_name, name.name
                ),
                receiver.span,
            );
        }
        if matches!(rcv, Receiver::Move) && !self.enums[enum_id.0 as usize].is_copy {
            self.consume_place(receiver, enum_name, &name.name);
        }
        if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!(
                    "method `{}::{}` takes {} argument(s), got {}",
                    enum_name,
                    name.name,
                    sig.params.len(),
                    args.len()
                ),
                call_span,
            );
        }
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            self.check_arg_with_move(a, expected);
        }
        for a in args.iter().skip(sig.params.len()) {
            let _ = self.check_expr(a, None);
        }
        sig.return_type.clone()
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
                        struct_name,
                        name.name,
                        arity,
                        type_args.len()
                    ),
                    name.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Error;
            }
            for (gp, ta) in sig.generic_params.iter().zip(type_args.iter()) {
                let resolved = self.resolve_type(ta);
                if matches!(resolved, Ty::Error) {
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                subst.insert(gp.clone(), resolved);
            }
        } else {
            // Infer: walk params, unify Ty::Param against arg type.
            for (param_sig, arg) in sig.params.iter().zip(args.iter()) {
                let arg_ty = self.check_expr(arg, None);
                if matches!(arg_ty, Ty::Error) {
                    continue;
                }
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
                        struct_name,
                        name.name,
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
        let arg_tys: Vec<Ty> = sig
            .generic_params
            .iter()
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
                if matches!(ty, Ty::Error) {
                    continue;
                }
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

    /// v0.0.4 Phase 3 Slice 3B.5: blessed `hash()` for primitive
    /// receivers. Integer types + str. (Bool and floats aren't typical
    /// hashmap keys; add if motivated.)
    fn is_blessed_hash_receiver(ty: &Ty) -> bool {
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

    /// v0.0.4 Phase 3 Slice 3B.5: blessed `eq(other)` for primitive
    /// receivers. Same set as Hash plus Bool — anywhere `==` works
    /// today.
    fn is_blessed_eq_receiver(ty: &Ty) -> bool {
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

    /// Phase 8 slice 8.STR.3: dispatch `s.method(args)` on a `string`
    /// receiver. Methods: `len() -> usize`, `is_empty() -> bool`,
    /// `as_str() -> str`, `clone() -> string`. Anything else fires E0324.
    /// v0.0.6 Slice 1B: dispatch a SIMD instance method on a `Ty::Simd`
    /// receiver. Methods are blessed (no user-defined SIMD impls); the
    /// table here is the source of truth for which methods exist and
    /// their per-method signatures.
    ///
    /// First cut: only `f32x4` works end-to-end. Methods accepted:
    ///   - `add(b: Self)`, `sub(b: Self)`, `mul(b: Self)`, `div(b: Self)` → Self
    ///   - `fma(b: Self, c: Self) -> Self` (uses `llvm.fma.<vN>`)
    ///   - `sqrt() -> Self` (uses `llvm.sqrt.<vN>`)
    ///   - `lane(i: u32) -> elem` (i must be a literal `u32` in `0..lanes`)
    ///   - `with_lane(i: u32, x: elem) -> Self`
    ///   - `to_array() -> [elem; lanes]`
    ///
    /// E0873: non-literal lane index.
    /// E0874: lane index out of range.
    /// E0324: unknown method on Ty::Simd.
    fn check_simd_method_call(
        &mut self,
        recv: &Ty,
        name: &Ident,
        args: &[Expr],
        call_span: ByteSpan,
    ) -> Ty {
        // v0.0.9 follow-up: routes both `Ty::Simd` and `Ty::Mask`. The
        // `is_mask` flag selects per-method behavior — e.g. comparisons
        // require a Simd receiver and produce a Mask, while `select` /
        // `any` / `all` require a Mask receiver; arithmetic on a Mask
        // is rejected; bitwise ops work on either.
        let (elem_ty, lanes_u, is_mask) = match recv {
            Ty::Simd { elem, lanes } => ((**elem).clone(), *lanes, false),
            Ty::Mask { elem, lanes } => ((**elem).clone(), *lanes, true),
            _ => return Ty::Error,
        };
        let arity_err = |this: &mut Self, expected: usize| -> bool {
            if args.len() != expected {
                this.err(
                    "E0308",
                    format!(
                        "`{}::{}` takes {} argument(s), got {}",
                        ty_display(recv),
                        name.name,
                        expected,
                        args.len()
                    ),
                    call_span,
                );
                for a in args {
                    let _ = this.check_expr(a, None);
                }
                false
            } else {
                true
            }
        };
        // v0.0.9 follow-up: helper that rejects mask receivers for
        // ops that only make sense on numeric SIMD (arithmetic, sqrt,
        // abs, etc.). Callers that hit this path should be using
        // `mask.to_bits()` to get an integer SIMD they can do
        // arithmetic on.
        let reject_on_mask = |this: &mut Self, op: &str, args: &[Expr]| -> bool {
            if is_mask {
                this.err(
                    "E0324",
                    format!(
                        "`{op}` is not available on mask types (`{}`); convert via `.to_bits()` first",
                        ty_display(recv)
                    ),
                    name.span,
                );
                for a in args {
                    let _ = this.check_expr(a, None);
                }
                true
            } else {
                false
            }
        };
        match name.name.as_str() {
            "add" | "sub" | "mul" | "div" => {
                if reject_on_mask(self, name.name.as_str(), args) {
                    return Ty::Error;
                }
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(recv.clone()));
                recv.clone()
            }
            "fma" => {
                if reject_on_mask(self, "fma", args) {
                    return Ty::Error;
                }
                if !elem_ty.is_float() {
                    self.err(
                        "E0324",
                        format!(
                            "`fma` is only available on floating-point SIMD types, not `{}`",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if !arity_err(self, 2) {
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(recv.clone()));
                let _ = self.check_expr(&args[1], Some(recv.clone()));
                recv.clone()
            }
            "sqrt" => {
                if reject_on_mask(self, "sqrt", args) {
                    return Ty::Error;
                }
                if !elem_ty.is_float() {
                    self.err(
                        "E0324",
                        format!(
                            "`sqrt` is only available on floating-point SIMD types, not `{}`",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                recv.clone()
            }
            // G-042: round-to-nearest-even per lane (float SIMD only).
            // Lowers to `llvm.roundeven` — same semantics as AArch64
            // `vcvtnq_s32_f32`/FCVTNS. Compose with `INTxN::from_float`
            // for a rounded float→int convert (the quantizer pattern).
            "round" => {
                if reject_on_mask(self, "round", args) {
                    return Ty::Error;
                }
                if !elem_ty.is_float() {
                    self.err(
                        "E0324",
                        format!(
                            "`round` is only available on floating-point SIMD types, not `{}`",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                recv.clone()
            }
            "abs" => {
                if reject_on_mask(self, "abs", args) {
                    return Ty::Error;
                }
                // `abs` available on signed-integer + float SIMD widths.
                // Unsigned-integer `abs` would be a no-op; reject to keep
                // the matrix clear.
                if !(elem_ty.is_float() || elem_ty.is_signed_int()) {
                    self.err(
                        "E0324",
                        format!("`abs` is only available on float and signed-integer SIMD types, not `{}`",
                            ty_display(recv)),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                recv.clone()
            }
            "min" | "max" => {
                if reject_on_mask(self, name.name.as_str(), args) {
                    return Ty::Error;
                }
                // Available on all numeric SIMD widths. Float widths use
                // `llvm.minnum`/`maxnum` (treats NaN as missing); integer
                // widths use the signed (`smin`/`smax`) or unsigned
                // (`umin`/`umax`) intrinsic per the lane type.
                if !elem_ty.is_numeric() {
                    self.err(
                        "E0324",
                        format!(
                            "`{}` requires a numeric SIMD type, not `{}`",
                            name.name,
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(recv.clone()));
                recv.clone()
            }
            "and" | "or" | "xor" => {
                // Bitwise ops: integer SIMD only. Float bitwise is
                // useful (sign masking, etc.) but requires bitcast
                // boilerplate at the source level; defer until a real
                // use case surfaces.
                if !elem_ty.is_int() {
                    self.err(
                        "E0324",
                        format!(
                            "bitwise `{}` requires an integer SIMD type, not `{}`",
                            name.name,
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(recv.clone()));
                recv.clone()
            }
            "not" => {
                if !elem_ty.is_int() {
                    self.err(
                        "E0324",
                        format!(
                            "bitwise `not` requires an integer SIMD type, not `{}`",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                recv.clone()
            }
            "shl" | "shr" => {
                if reject_on_mask(self, name.name.as_str(), args) {
                    return Ty::Error;
                }
                // Element-wise shift by a scalar count (every lane shifted
                // by the same amount). The amount must be `u32` and a
                // literal in `0..lane_bits` — sema enforces both. `shr`
                // is arithmetic for signed lanes (`ashr`), logical for
                // unsigned (`lshr`).
                if !elem_ty.is_int() {
                    self.err(
                        "E0324",
                        format!(
                            "shift `{}` requires an integer SIMD type, not `{}`",
                            name.name,
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let count_lit = match &args[0].kind {
                    ExprKind::IntLit(v, _) => Some(*v),
                    ExprKind::Cast { expr, .. } => {
                        if let ExprKind::IntLit(v, _) = &expr.kind {
                            Some(*v)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let _ = self.check_expr(&args[0], Some(Ty::U32));
                let lane_bits: u64 = match &elem_ty {
                    Ty::I8 | Ty::U8 => 8,
                    Ty::I16 | Ty::U16 => 16,
                    Ty::I32 | Ty::U32 => 32,
                    Ty::I64 | Ty::U64 => 64,
                    _ => 0,
                };
                match count_lit {
                    None => {
                        self.err(
                            "E0873",
                            format!(
                                "shift count for `{}.{}(...)` must be a literal `u32`",
                                ty_display(recv),
                                name.name
                            ),
                            args[0].span,
                        );
                        return Ty::Error;
                    }
                    Some(c) if c >= lane_bits => {
                        self.err(
                            "E0874",
                            format!(
                                "shift count {c} out of range for `{}` ({} bits per lane)",
                                ty_display(recv),
                                lane_bits
                            ),
                            args[0].span,
                        );
                        return Ty::Error;
                    }
                    _ => {}
                }
                recv.clone()
            }
            "lane" => {
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                // Lane index must be a literal so codegen can emit
                // `extractelement <N x T> v, i32 <const>`.
                let idx_lit = match &args[0].kind {
                    ExprKind::IntLit(v, _) => Some(*v),
                    ExprKind::Cast { expr, .. } => {
                        if let ExprKind::IntLit(v, _) = &expr.kind {
                            Some(*v)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let _ = self.check_expr(&args[0], Some(Ty::U32));
                match idx_lit {
                    None => {
                        self.err(
                            "E0873",
                            format!(
                                "lane index for `{}.lane(...)` must be a literal `u32`",
                                ty_display(recv)
                            ),
                            args[0].span,
                        );
                        return Ty::Error;
                    }
                    Some(i) if (i as u32) >= lanes_u => {
                        self.err(
                            "E0874",
                            format!(
                                "lane index {i} out of range for `{}` ({} lanes)",
                                ty_display(recv),
                                lanes_u
                            ),
                            args[0].span,
                        );
                        return Ty::Error;
                    }
                    Some(_) => {}
                }
                elem_ty
            }
            "with_lane" => {
                if !arity_err(self, 2) {
                    return Ty::Error;
                }
                let idx_lit = match &args[0].kind {
                    ExprKind::IntLit(v, _) => Some(*v),
                    ExprKind::Cast { expr, .. } => {
                        if let ExprKind::IntLit(v, _) = &expr.kind {
                            Some(*v)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let _ = self.check_expr(&args[0], Some(Ty::U32));
                let _ = self.check_expr(&args[1], Some(elem_ty.clone()));
                match idx_lit {
                    None => {
                        self.err(
                            "E0873",
                            format!(
                                "lane index for `{}.with_lane(...)` must be a literal `u32`",
                                ty_display(recv)
                            ),
                            args[0].span,
                        );
                        return Ty::Error;
                    }
                    Some(i) if (i as u32) >= lanes_u => {
                        self.err(
                            "E0874",
                            format!(
                                "lane index {i} out of range for `{}` ({} lanes)",
                                ty_display(recv),
                                lanes_u
                            ),
                            args[0].span,
                        );
                        return Ty::Error;
                    }
                    Some(_) => {}
                }
                recv.clone()
            }
            "to_array" => {
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                Ty::Array(Box::new(elem_ty), lanes_u)
            }
            "store" => {
                // Unsafe — writes through a raw pointer. Caller owns
                // alignment (lane-sized minimum; misaligned addresses
                // are UB).
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                if self.unsafe_depth == 0 {
                    self.err(
                        "E0801",
                        format!(
                            "`{}.store` is unsafe; wrap in `unsafe {{ ... }}`",
                            ty_display(recv)
                        ),
                        call_span,
                    );
                }
                let want = Ty::RawPtr(Box::new(elem_ty));
                let _ = self.check_expr(&args[0], Some(want));
                Ty::Unit
            }
            // v0.0.7 Slice 2.1: lane-wise comparisons. Result is the
            // width-matched `Ty::Mask` (distinct from `Ty::Simd` since
            // v0.0.9). Float widths use `fcmp`, int widths `icmp`
            // (signed for signed-int lanes, unsigned for unsigned-int
            // lanes); the result is sext'd to the mask shape.
            "lt" | "le" | "gt" | "ge" | "eq" | "ne" => {
                if reject_on_mask(self, name.name.as_str(), args) {
                    return Ty::Error;
                }
                if !elem_ty.is_numeric() {
                    self.err(
                        "E0324",
                        format!(
                            "comparison `{}` requires a numeric SIMD type, not `{}`",
                            name.name,
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(recv.clone()));
                // Result type: mask of matching lane width.
                Ty::Mask {
                    elem: Box::new(matching_signed_int_lane(&elem_ty)),
                    lanes: lanes_u,
                }
            }
            // v0.0.7 Slice 2.1: blend per lane. v0.0.9 follow-up:
            // receiver must be a `Ty::Mask`; the two value args must
            // match each other and must be the same lane count as
            // the mask. Returns the value-arg type.
            "select" => {
                if !is_mask {
                    self.err(
                        "E0324",
                        format!(
                            "`select` requires a mask receiver, got `{}` — use a comparison (`.lt`/`.gt`/etc.) or `.to_mask()` to produce one",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if !arity_err(self, 2) {
                    return Ty::Error;
                }
                let t_ty = self.check_expr(&args[0], None);
                let _ = self.check_expr(&args[1], Some(t_ty.clone()));
                // Constrain: t_ty must be a Simd (not Mask) with matching lane count.
                match &t_ty {
                    Ty::Simd { lanes: tl, .. } if *tl == lanes_u => t_ty,
                    _ => {
                        self.err(
                            "E0324",
                            format!(
                                "`select` arms must be a SIMD value of the same lane count as the mask `{}`",
                                ty_display(recv)
                            ),
                            name.span,
                        );
                        Ty::Error
                    }
                }
            }
            // v0.0.7 Slice 2.1: mask reductions. v0.0.9 follow-up:
            // receiver must be a `Ty::Mask`. `any` is true iff any
            // lane is set; `all` is true iff every lane is.
            "any" | "all" => {
                if !is_mask {
                    self.err(
                        "E0324",
                        format!(
                            "`{}` requires a mask receiver, got `{}` — use a comparison or `.to_mask()` to produce one",
                            name.name, ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                Ty::Bool
            }
            // v0.0.7 Slice 2.1: horizontal reductions. `sum` and
            // `product` available on every numeric width. Result type
            // is the lane scalar.
            "sum" | "product" => {
                if reject_on_mask(self, name.name.as_str(), args) {
                    return Ty::Error;
                }
                if !elem_ty.is_numeric() {
                    self.err(
                        "E0324",
                        format!(
                            "`{}` requires a numeric SIMD type, not `{}`",
                            name.name,
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                // Lint (W0001): a horizontal `sum`/`product` over narrow
                // integer lanes (< 32 bits) returns that same narrow lane
                // type, which cannot hold the reduction of more than a couple
                // of near-max lanes — the `i8x16.mul().sum()` quant footgun
                // that silently wraps. Same-width arithmetic and `sum` stay
                // legal (they're useful for small-valued lanes), so this is a
                // non-fatal warning, not E0324. The fix: `.widen()` the lanes
                // first, or use `simd/integer::dot_i32` for a widening dot.
                if elem_ty.is_int() && simd_lane_bits(&elem_ty) < 32 {
                    self.warn(
                        "W0001",
                        format!(
                            "`{}` over narrow integer lanes (`{}`) returns `{}` and silently wraps when the reduction exceeds that type; `.widen()` the lanes first, or use `simd/integer::dot_i32` for a widening dot product",
                            name.name,
                            ty_display(recv),
                            elem_ty.name(),
                        ),
                        name.span,
                    );
                }
                elem_ty.clone()
            }
            // v0.0.7 Slice 2.1: horizontal min/max. Float widths use
            // `llvm.vector.reduce.fmin/fmax`; int widths use the
            // signed (`smin`/`smax`) or unsigned (`umin`/`umax`) variant.
            "min_across" | "max_across" => {
                if reject_on_mask(self, name.name.as_str(), args) {
                    return Ty::Error;
                }
                if !elem_ty.is_numeric() {
                    self.err(
                        "E0324",
                        format!(
                            "`{}` requires a numeric SIMD type, not `{}`",
                            name.name,
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                elem_ty.clone()
            }
            // v0.0.7 Slice 2.1: per-lane permutations. `reverse` is
            // zero-arg; `swizzle` takes a `[u32; N]` array literal
            // whose values index into the source vector (validated
            // at codegen via `simd_lane_literal`-style helper).
            "reverse" => {
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                recv.clone()
            }
            // G-039b: split a SIMD vector into its low / high half (NEON
            // `vget_low`/`vget_high`). Result has the same lane type, half the
            // lanes (e.g. `i8x16` → `i8x8`). Requires an even lane count.
            "low" | "high" => {
                if reject_on_mask(self, name.name.as_str(), args) {
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                if lanes_u < 2 || lanes_u % 2 != 0 {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::{}` requires an even-lane SIMD (got {} lanes)",
                            ty_display(recv),
                            name.name,
                            lanes_u
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                Ty::Simd { elem: Box::new(elem_ty), lanes: lanes_u / 2 }
            }
            // G-039b: join two equal half-width vectors into a full-width one
            // (NEON `vcombine`). `lo.combine(hi)` → twice the lanes, same lane
            // type (e.g. two `i8x8` → `i8x16`); `lo` fills the low lanes.
            "combine" => {
                if reject_on_mask(self, "combine", args) {
                    return Ty::Error;
                }
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(recv.clone()));
                Ty::Simd { elem: Box::new(elem_ty), lanes: lanes_u * 2 }
            }
            // G-038b: widen each integer lane to the next size up, preserving
            // lane count (NEON `vmovl`: `i8x8` → `i16x8`). Signed lanes
            // sign-extend, unsigned zero-extend (decided in codegen).
            "widen" => {
                if reject_on_mask(self, "widen", args) {
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                match simd_widen_elem(&elem_ty) {
                    Some(w) => Ty::Simd { elem: Box::new(w), lanes: lanes_u },
                    None => {
                        self.err(
                            "E0324",
                            format!(
                                "`{}::widen` requires an integer SIMD with lanes ≤ 32 bits",
                                ty_display(recv)
                            ),
                            name.span,
                        );
                        Ty::Error
                    }
                }
            }
            // G-038b: narrow each integer lane to the next size down by
            // truncation, preserving lane count (NEON `vmovn`: `i16x8` →
            // `i8x8`). Drops the high bits of each lane.
            "narrow" => {
                if reject_on_mask(self, "narrow", args) {
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                match simd_narrow_elem(&elem_ty) {
                    Some(n) => Ty::Simd { elem: Box::new(n), lanes: lanes_u },
                    None => {
                        self.err(
                            "E0324",
                            format!(
                                "`{}::narrow` requires an integer SIMD with lanes ≥ 16 bits",
                                ty_display(recv)
                            ),
                            name.span,
                        );
                        Ty::Error
                    }
                }
            }
            // G-040: data-dependent byte table lookup (NEON `vqtbl1q`).
            // `tbl.table(idx)` — the receiver is a 16-byte lookup table
            // (`i8x16`/`u8x16`), `idx` is a `u8x16` of per-lane indices;
            // result[i] = tbl[idx[i]], with out-of-range indices yielding 0.
            // The one runtime-index shuffle (`swizzle` needs literal indices),
            // used to expand 4-bit nibbles through a dequant LUT.
            "table" => {
                if reject_on_mask(self, "table", args) {
                    return Ty::Error;
                }
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let is_byte16 =
                    matches!(&elem_ty, Ty::I8 | Ty::U8) && lanes_u == 16;
                if !is_byte16 {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::table` requires a 16-byte SIMD table (`i8x16` or `u8x16`)",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let want = Ty::Simd { elem: Box::new(Ty::U8), lanes: 16 };
                let at = self.check_expr(&args[0], Some(want));
                if !matches!(&at, Ty::Simd { elem, lanes } if matches!(**elem, Ty::U8) && *lanes == 16)
                    && at != Ty::Error
                {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::table` index argument must be `u8x16`, found `{}`",
                            ty_display(recv),
                            ty_display(&at)
                        ),
                        call_span,
                    );
                    return Ty::Error;
                }
                recv.clone()
            }
            "swizzle" => {
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let want = Ty::Array(Box::new(Ty::U32), lanes_u);
                let _ = self.check_expr(&args[0], Some(want));
                // Codegen lowers `swizzle` to a constant `shufflevector`
                // mask, so the index array must be an array literal of
                // compile-time constants. Enforce that here (mirroring the
                // `lane`/`shl` literal requirement) rather than letting
                // codegen's `.expect(...)` panic on a runtime array. (G-035)
                match &args[0].kind {
                    ExprKind::ArrayLit { elements } => {
                        for el in elements {
                            let v = match &el.kind {
                                ExprKind::IntLit(v, _) => Some(*v),
                                ExprKind::Cast { expr, .. } => match &expr.kind {
                                    ExprKind::IntLit(v, _) => Some(*v),
                                    _ => None,
                                },
                                _ => None,
                            };
                            match v {
                                None => {
                                    self.err(
                                        "E0873",
                                        format!(
                                            "`{}.swizzle(...)` indices must be compile-time literals",
                                            ty_display(recv)
                                        ),
                                        el.span,
                                    );
                                    return Ty::Error;
                                }
                                Some(i) if (i as u32) >= lanes_u => {
                                    self.err(
                                        "E0874",
                                        format!(
                                            "swizzle index {i} out of range for `{}` ({} lanes)",
                                            ty_display(recv),
                                            lanes_u
                                        ),
                                        el.span,
                                    );
                                    return Ty::Error;
                                }
                                Some(_) => {}
                            }
                        }
                    }
                    _ => {
                        self.err(
                            "E0873",
                            format!(
                                "`{}.swizzle(...)` requires a `[u32; {}]` array literal of compile-time lane indices",
                                ty_display(recv),
                                lanes_u
                            ),
                            args[0].span,
                        );
                        return Ty::Error;
                    }
                }
                recv.clone()
            }
            // v0.0.7 Slice 2.1: even/odd interleaves with another
            // same-shape vector. Returns the same shape.
            "interleave_lo" | "interleave_hi" => {
                if !arity_err(self, 1) {
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(recv.clone()));
                recv.clone()
            }
            // v0.0.9 follow-up: explicit Mask <-> Simd conversions.
            // Both are zero-cost at the IR level (same `<N x iN>`
            // lowering); they exist to make the type-system crossing
            // intentional in source.
            "to_bits" => {
                if !is_mask {
                    self.err(
                        "E0324",
                        format!(
                            "`to_bits` is only available on mask types; got `{}` (already an integer SIMD)",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                Ty::Simd {
                    elem: Box::new(elem_ty.clone()),
                    lanes: lanes_u,
                }
            }
            "to_mask" => {
                if is_mask {
                    self.err(
                        "E0324",
                        format!("`to_mask` is only available on integer SIMD types; receiver `{}` is already a mask", ty_display(recv)),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !elem_ty.is_signed_int() {
                    self.err(
                        "E0324",
                        format!(
                            "`to_mask` requires a signed-integer SIMD receiver, not `{}` — masks are width-tagged by their lane sign convention",
                            ty_display(recv)
                        ),
                        name.span,
                    );
                    return Ty::Error;
                }
                if !arity_err(self, 0) {
                    return Ty::Error;
                }
                Ty::Mask {
                    elem: Box::new(elem_ty.clone()),
                    lanes: lanes_u,
                }
            }
            _ => {
                self.err(
                    "E0324",
                    format!(
                        "no method `{}` on SIMD type `{}`",
                        name.name,
                        ty_display(recv)
                    ),
                    name.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                Ty::Error
            }
        }
    }

    /// v0.0.6 Slice 1B: dispatch a SIMD associated function — `f32x4::splat(s)`,
    /// `f32x4::new(a, b, c, d)`, `f32x4::from_array(a)`. The path is parsed
    /// as `Path { segments: ["f32x4", "splat"] }` and routes here when sema
    /// recognizes the first segment as a SIMD type name.
    fn check_simd_assoc_call(
        &mut self,
        recv: &Ty,
        method: &Ident,
        args: &[Expr],
        call_span: ByteSpan,
    ) -> Ty {
        let Ty::Simd { elem, lanes } = recv else {
            return Ty::Error;
        };
        let elem_ty = (**elem).clone();
        let lanes_u = *lanes;
        match method.name.as_str() {
            "splat" => {
                if args.len() != 1 {
                    self.err(
                        "E0308",
                        format!(
                            "`{}::splat` takes 1 argument, got {}",
                            ty_display(recv),
                            args.len()
                        ),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let _ = self.check_expr(&args[0], Some(elem_ty));
                recv.clone()
            }
            "new" => {
                if args.len() != lanes_u as usize {
                    self.err(
                        "E0308",
                        format!(
                            "`{}::new` takes {} argument(s), got {}",
                            ty_display(recv),
                            lanes_u,
                            args.len()
                        ),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                for a in args {
                    let _ = self.check_expr(a, Some(elem_ty.clone()));
                }
                recv.clone()
            }
            "from_array" => {
                if args.len() != 1 {
                    self.err(
                        "E0308",
                        format!(
                            "`{}::from_array` takes 1 argument, got {}",
                            ty_display(recv),
                            args.len()
                        ),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let want = Ty::Array(Box::new(elem_ty), lanes_u);
                let _ = self.check_expr(&args[0], Some(want));
                recv.clone()
            }
            "load" => {
                // Unsafe — reads through a raw pointer. Caller owns
                // alignment (lane-sized minimum; misaligned addresses
                // are UB).
                if args.len() != 1 {
                    self.err(
                        "E0308",
                        format!(
                            "`{}::load` takes 1 argument, got {}",
                            ty_display(recv),
                            args.len()
                        ),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if self.unsafe_depth == 0 {
                    self.err(
                        "E0801",
                        format!(
                            "`{}::load` is unsafe; wrap in `unsafe {{ ... }}`",
                            ty_display(recv)
                        ),
                        call_span,
                    );
                }
                let want = Ty::RawPtr(Box::new(elem_ty));
                let _ = self.check_expr(&args[0], Some(want));
                recv.clone()
            }
            // G-037: `TARGET::reinterpret(v)` — bitcast a SIMD value to the
            // target lane shape, preserving the bits (NEON `vreinterpretq_*`).
            // Source and target must have the same total width; lane type/count
            // may differ (e.g. `i16x8::reinterpret(v: i8x16)`). Safe — no memory
            // access, no value change.
            "reinterpret" => {
                if args.len() != 1 {
                    self.err(
                        "E0308",
                        format!(
                            "`{}::reinterpret` takes 1 argument, got {}",
                            ty_display(recv),
                            args.len()
                        ),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let at = self.check_expr(&args[0], None);
                if let Ty::Simd { elem: se, lanes: sl } = &at {
                    let src_bits = simd_lane_bits(se) * *sl;
                    let dst_bits = simd_lane_bits(&elem_ty) * lanes_u;
                    if src_bits == dst_bits && src_bits != 0 {
                        return recv.clone();
                    }
                    self.err(
                        "E0324",
                        format!(
                            "`{}::reinterpret` requires a SIMD value of the same total width ({} bits); `{}` is {} bits",
                            ty_display(recv),
                            dst_bits,
                            ty_display(&at),
                            src_bits
                        ),
                        call_span,
                    );
                } else if at != Ty::Error {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::reinterpret` expects a SIMD argument, found `{}`",
                            ty_display(recv),
                            ty_display(&at)
                        ),
                        call_span,
                    );
                }
                Ty::Error
            }
            // G-038a: `FLOATxN::from_int(v)` — lane-wise integer→float convert
            // (NEON `vcvtq_f32_s32` / `_u32`). Target lanes are float; the
            // argument is an int SIMD of the same lane count and lane width
            // (e.g. `f32x4::from_int(i32x4)`, `f64x2::from_int(u64x2)`).
            // Signedness of the source picks `sitofp` vs `uitofp` in codegen.
            "from_int" => {
                if !elem_ty.is_float() {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::from_int` is only valid on a float SIMD target",
                            ty_display(recv)
                        ),
                        method.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if args.len() != 1 {
                    self.err(
                        "E0308",
                        format!("`{}::from_int` takes 1 argument, got {}", ty_display(recv), args.len()),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let at = self.check_expr(&args[0], None);
                if let Ty::Simd { elem: se, lanes: sl } = &at {
                    if se.is_int() && *sl == lanes_u && simd_lane_bits(se) == simd_lane_bits(&elem_ty) {
                        return recv.clone();
                    }
                }
                if at != Ty::Error {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::from_int` expects an integer SIMD of the same lane count and width (e.g. `i32x4` → `f32x4`), found `{}`",
                            ty_display(recv),
                            ty_display(&at)
                        ),
                        call_span,
                    );
                }
                Ty::Error
            }
            // G-038a: `INTxN::from_float(v)` — lane-wise float→integer convert
            // (NEON `vcvtq_s32_f32` / `_u32`). Target lanes are int; the
            // argument is a float SIMD of the same lane count and lane width
            // (e.g. `i32x4::from_float(f32x4)`). Target signedness picks
            // `fptosi` vs `fptoui`. Truncates toward zero, like a scalar `as`.
            "from_float" => {
                if !elem_ty.is_int() {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::from_float` is only valid on an integer SIMD target",
                            ty_display(recv)
                        ),
                        method.span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                if args.len() != 1 {
                    self.err(
                        "E0308",
                        format!("`{}::from_float` takes 1 argument, got {}", ty_display(recv), args.len()),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let at = self.check_expr(&args[0], None);
                if let Ty::Simd { elem: se, lanes: sl } = &at {
                    if se.is_float() && *sl == lanes_u && simd_lane_bits(se) == simd_lane_bits(&elem_ty) {
                        return recv.clone();
                    }
                }
                if at != Ty::Error {
                    self.err(
                        "E0324",
                        format!(
                            "`{}::from_float` expects a float SIMD of the same lane count and width (e.g. `f32x4` → `i32x4`), found `{}`",
                            ty_display(recv),
                            ty_display(&at)
                        ),
                        call_span,
                    );
                }
                Ty::Error
            }
            _ => {
                self.err(
                    "E0324",
                    format!(
                        "no associated function `{}` on SIMD type `{}`",
                        method.name,
                        ty_display(recv)
                    ),
                    method.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                Ty::Error
            }
        }
    }

    fn check_string_method_call(&mut self, name: &Ident, args: &[Expr], call_span: ByteSpan) -> Ty {
        let no_args = |this: &mut Self| -> bool {
            if !args.is_empty() {
                this.err(
                    "E0308",
                    format!(
                        "`string::{}` takes 0 argument(s), got {}",
                        name.name,
                        args.len()
                    ),
                    call_span,
                );
                for a in args {
                    let _ = this.check_expr(a, None);
                }
                false
            } else {
                true
            }
        };
        match name.name.as_str() {
            "len" => {
                let _ = no_args(self);
                Ty::Usize
            }
            "is_empty" => {
                let _ = no_args(self);
                Ty::Bool
            }
            "as_str" => {
                let _ = no_args(self);
                Ty::Str
            }
            "clone" => {
                let _ = no_args(self);
                Ty::String
            }
            _ => {
                self.err(
                    "E0324",
                    format!("no method `{}` on type `string`", name.name),
                    name.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                Ty::Error
            }
        }
    }

    /// Phase 8 slice 8.STR.3: dispatch `string::method(args)`. Only two
    /// associated fns ship in v1: `new` (no args, returns empty `string`)
    /// and `with_capacity(n: usize)` (returns a string with `n` bytes
    /// pre-allocated). Anything else fires E0324.
    fn check_string_assoc_call(
        &mut self,
        method: &Ident,
        args: &[Expr],
        call_span: ByteSpan,
    ) -> Ty {
        match method.name.as_str() {
            "new" => {
                if !args.is_empty() {
                    self.err(
                        "E0308",
                        format!("`string::new` takes 0 argument(s), got {}", args.len()),
                        call_span,
                    );
                }
                Ty::String
            }
            "with_capacity" => {
                if args.len() != 1 {
                    self.err(
                        "E0308",
                        format!(
                            "`string::with_capacity` takes 1 argument, got {}",
                            args.len()
                        ),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let arg_ty = self.check_expr(&args[0], Some(Ty::Usize));
                if !matches!(arg_ty, Ty::Usize | Ty::Error) {
                    self.err(
                        "E0302",
                        format!(
                            "`string::with_capacity` expects `usize`, got `{}`",
                            ty_display(&arg_ty)
                        ),
                        args[0].span,
                    );
                }
                Ty::String
            }
            _ => {
                self.err(
                    "E0324",
                    format!("no associated function `{}` on type `string`", method.name),
                    method.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                Ty::Error
            }
        }
    }

    fn check_assoc_call(
        &mut self,
        segments: &[Ident],
        type_args: &[Type],
        args: &[Expr],
        path_span: ByteSpan,
        call_span: ByteSpan,
    ) -> Ty {
        if segments.len() != 2 {
            self.err(
                "E0312",
                "Phase 2 paths have exactly two segments".to_string(),
                path_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
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
                self.err(
                    "E0501",
                    "`string::{new,with_capacity}` take no type arguments".to_string(),
                    call_span,
                );
            }
            return self.check_string_assoc_call(method_seg, args, call_span);
        }
        // v0.0.12 G-045 (llama.cplus): blessed `fN::from_bits(uN)` — a
        // bit-preserving reinterpret from the same-width unsigned int to the
        // float (LLVM `bitcast`). Associated constructor on the float type;
        // pairs with the `.to_bits()` instance method. Only `from_bits` is
        // intercepted — any other `fN::x()` falls through to the normal
        // (error) path.
        if method_seg.name == "from_bits" {
            let float_ty = match type_seg.name.as_str() {
                "f16" => Some(Ty::F16),
                "f32" => Some(Ty::F32),
                "f64" => Some(Ty::F64),
                _ => None,
            };
            if let Some(fty) = float_ty {
                if !type_args.is_empty() {
                    self.err(
                        "E0501",
                        "`from_bits` takes no type arguments".to_string(),
                        call_span,
                    );
                }
                let want = match fty {
                    Ty::F16 => Ty::U16,
                    Ty::F32 => Ty::U32,
                    _ => Ty::U64,
                };
                if args.len() != 1 {
                    self.err(
                        "E0327",
                        format!("`{}::from_bits` takes exactly one argument", type_seg.name),
                        call_span,
                    );
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                let got = self.check_expr(&args[0], Some(want.clone()));
                if got != want && got != Ty::Error {
                    self.err(
                        "E0302",
                        format!(
                            "`{}::from_bits` expects `{}`, got `{}`",
                            type_seg.name,
                            want.name(),
                            got.name()
                        ),
                        args[0].span,
                    );
                }
                return fty;
            }
        }
        // v0.0.6 Slice 1B: SIMD type associated functions —
        // `f32x4::splat(s)`, `f32x4::new(a, b, c, d)`, `f32x4::from_array(a)`.
        // v0.0.9 follow-up: `mask{N}x{M}::splat / new / from_array` are
        // rejected — masks are produced by comparisons (`.lt`, etc.) or
        // by `simd.to_mask()`, never constructed lane-by-lane.
        if let Some(recv) = simd_ty_from_name(&type_seg.name) {
            if matches!(recv, Ty::Mask { .. }) {
                self.err(
                    "E0324",
                    format!(
                        "`{}` is a mask type; construct via a SIMD comparison (`.lt`/`.gt`/etc.) or `.to_mask()`, not `{}::{}`",
                        type_seg.name, type_seg.name, method_seg.name
                    ),
                    call_span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Error;
            }
            if !type_args.is_empty() {
                self.err(
                    "E0501",
                    format!(
                        "`{}::{{splat,new,from_array}}` take no type arguments",
                        type_seg.name
                    ),
                    call_span,
                );
            }
            return self.check_simd_assoc_call(&recv, method_seg, args, call_span);
        }
        // Enums: a call shape `Name::Variant(args)` constructs a tagged
        // variant. Look up the variant; verify it has a payload (call form
        // is illegal for payload-less variants — use the bare path); check
        // arg count and types against the payload.
        let mut enum_id = self.enum_by_name.get(&type_seg.name).copied();
        let mut struct_id = self.struct_by_name.get(&type_seg.name).copied();
        if enum_id.is_none() && struct_id.is_none() {
            let temp_ast_ty = crate::ast::Type {
                kind: crate::ast::TypeKind::Path(type_seg.name.clone()),
                span: type_seg.span,
            };
            let ty = self.resolve_type(&temp_ast_ty);
            match ty {
                Ty::Enum(id) => enum_id = Some(id),
                Ty::Struct(id) => struct_id = Some(id),
                Ty::Error => {
                    for a in args {
                        let _ = self.check_expr(a, None);
                    }
                    return Ty::Error;
                }
                _ => {}
            }
        }

        if let Some(id) = enum_id {
            let enum_def = self.enums[id.0 as usize].clone();
            let variant = enum_def.variants.iter().find(|v| v.name == method_seg.name);
            let Some(vdef) = variant else {
                self.err(
                    "E0317",
                    format!(
                        "enum `{}` has no variant `{}`",
                        type_seg.name, method_seg.name
                    ),
                    method_seg.span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Error;
            };
            if vdef.payload.is_empty() {
                // Payload-less variant called with parens — point users at
                // the bare path syntax.
                self.err(
                    "E0327",
                    format!(
                        "variant `{}::{}` has no payload; use the bare path `{}::{}`",
                        type_seg.name, method_seg.name, type_seg.name, method_seg.name
                    ),
                    call_span,
                );
                for a in args {
                    let _ = self.check_expr(a, None);
                }
                return Ty::Error;
            }
            if args.len() != vdef.payload.len() {
                self.err(
                    "E0342",
                    format!(
                        "variant `{}::{}` takes {} payload value(s); got {}",
                        type_seg.name,
                        method_seg.name,
                        vdef.payload.len(),
                        args.len()
                    ),
                    call_span,
                );
            }
            for (a, expected_ty) in args.iter().zip(vdef.payload.iter()) {
                let vty = self.check_expr(a, Some(expected_ty.clone()));
                // v0.0.14 soundness: the payload value is moved into the new
                // variant; reject a partial move of a non-Copy field/index out
                // of a Drop aggregate (E0509), as at every other value site.
                if vty != Ty::Error {
                    self.reject_partial_move_of_drop(a, &vty);
                }
            }
            for a in args.iter().skip(vdef.payload.len()) {
                let _ = self.check_expr(a, None);
            }
            return Ty::Enum(id);
        }
        let Some(id) = struct_id else {
            self.err(
                "E0303",
                format!("unknown type `{}`", type_seg.name),
                type_seg.span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        };
        let struct_name = self.structs[id.0 as usize].name.clone();
        let Some(sig) = self.structs[id.0 as usize]
            .methods
            .get(&method_seg.name)
            .cloned()
        else {
            self.err(
                "E0324",
                format!(
                    "struct `{}` has no method `{}`",
                    struct_name, method_seg.name
                ),
                method_seg.span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        };
        if sig.receiver.is_some() {
            self.err(
                "E0327",
                format!(
                    "`{}::{}` is an instance method; call it as `value.{}(...)`",
                    struct_name, method_seg.name, method_seg.name
                ),
                call_span,
            );
            for a in args {
                let _ = self.check_expr(a, None);
            }
            return Ty::Error;
        }
        if args.len() != sig.params.len() {
            self.err(
                "E0308",
                format!(
                    "function `{}::{}` takes {} argument(s), got {}",
                    struct_name,
                    method_seg.name,
                    sig.params.len(),
                    args.len()
                ),
                call_span,
            );
        }
        // Slice 7GEN.5e: generic-method dispatch on assoc-call form
        // (`Type::method(...)` / `Type::method::[T](...)`).
        if !sig.generic_params.is_empty() {
            return self.check_generic_method_call(
                id,
                &struct_name,
                method_seg,
                &sig,
                type_args,
                args,
                call_span,
            );
        }
        if !type_args.is_empty() {
            self.err(
                "E0501",
                format!(
                    "associated function `{}::{}` takes no type arguments but {} were provided",
                    struct_name,
                    method_seg.name,
                    type_args.len()
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
        // v0.0.10 Phase 5: non-Copy params consume the caller's binding by
        // default. `move x: T` is the explicit spelling (kept for
        // back-compat); `mut x: T` is the exclusive-borrow form (no
        // consume); `borrow x: T` is the new shared-borrow opt-out for
        // non-Copy types. Copy types are never consumed (no Drop, the
        // marker is just informational).
        //
        // Rvalues (struct literals, generic struct literals, enum
        // constructors, calls returning by value, etc.) own their value
        // outright — there's no caller binding to mark moved, so the
        // "implicit move" default is a no-op for them. Only explicit
        // `move x: T` (which would have errored before, and still does
        // for Field/Index partial moves) and named-binding arguments
        // exercise consume_arg_place.
        if self.is_copy(&expected.ty) {
            return;
        }
        let implicit_move = !expected.mutable && !expected.borrow_;
        let explicit_move = expected.move_;
        if explicit_move {
            self.consume_arg_place(arg);
        } else if implicit_move && is_addr_of_place(arg) {
            // v0.0.14 soundness fix: a non-Copy *place* passed by value to a
            // value param is a move — a whole-binding Ident is consumed, a
            // Field/Index/Deref projection is a partial move and rejected with
            // E0337 (`consume_arg_place`). Previously only whole-binding Idents
            // were consumed, so a non-Copy field/element arg was silently
            // bit-copied (aliased) — sound only while nothing dropped; under
            // auto field-drop it double-freed. Rvalues (struct/enum literals,
            // call results) aren't places, so they fall through untouched —
            // they own their value outright.
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

    /// Resolve the type of a *place* expression by lookup only — no error
    /// emission, no move-state mutation. Returns `None` for non-place
    /// expressions or places this lightweight resolver can't follow
    /// (method results, calls, etc.). Used by `reject_partial_move_of_drop`
    /// to walk a projection chain.
    fn place_ty_quiet(&self, e: &Expr) -> Option<Ty> {
        match &e.kind {
            ExprKind::Ident(name) => self.lookup_local(name).map(|i| i.ty.clone()),
            ExprKind::Field { receiver, name } => {
                let Ty::Struct(id) = self.place_ty_quiet(receiver)? else {
                    return None;
                };
                self.structs
                    .get(id.0 as usize)?
                    .field(&name.name)
                    .map(|(_, ty)| ty)
            }
            ExprKind::Index { receiver, .. } => match self.place_ty_quiet(receiver)? {
                Ty::Array(elem, _) => Some(*elem),
                Ty::Slice(elem) => Some(*elem),
                _ => None,
            },
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => match self.place_ty_quiet(operand)? {
                Ty::RawPtr(pointee) => Some(*pointee),
                _ => None,
            },
            _ => None,
        }
    }

    /// E0509: reject moving a non-Copy value out of a field/index of a place
    /// whose type — at any level of the projection chain — implements `drop`.
    ///
    /// C+'s drop model (docs/design/phase3-drop.md §5) makes a destructor
    /// responsible for freeing its own fields by hand; the compiler does not
    /// synthesize per-field drops for Drop types. So stealing a field out from
    /// under a live destructor is a guaranteed double-free / use-after-free:
    /// the moved-to binding drops the field, and the owner's destructor frees
    /// it again. Mirrors Rust's "cannot move out of type which implements
    /// `Drop`". `move_ty` is the already-checked type of `e`; a `Copy` move is
    /// a harmless read and is exempt.
    fn reject_partial_move_of_drop(&mut self, e: &Expr, moved_ty: &Ty) {
        if self.is_copy(moved_ty) {
            return;
        }
        let mut cur = e;
        loop {
            match &cur.kind {
                ExprKind::Field { receiver, .. } | ExprKind::Index { receiver, .. } => {
                    if let Some(base_ty) = self.place_ty_quiet(receiver) {
                        if self.ty_carries_drop(&base_ty) {
                            let base_name = match &base_ty {
                                Ty::Struct(id) => self.structs[id.0 as usize].name.clone(),
                                Ty::Array(elem, _) => format!("[{}; N]", elem.name()),
                                _ => base_ty.name().to_string(),
                            };
                            self.err(
                                "E0509",
                                format!(
                                    "cannot move a field out of `{base_name}` because its type implements `drop` — the destructor would free the moved field a second time. Clone the field instead, or restructure so the value isn't owned by a Drop type."
                                ),
                                e.span,
                            );
                            return;
                        }
                    }
                    cur = receiver;
                }
                ExprKind::Unary {
                    op: UnaryOp::Deref,
                    operand,
                } => {
                    cur = operand;
                }
                _ => return,
            }
        }
    }

    /// v0.0.12 (#2/#3 returned-borrow checking): record the param names,
    /// per-param borrow regions, and return region of the function/method
    /// about to be body-checked, and validate the signature-level region rule
    /// (E0511). Call once before checking the body; the `return`-site checks
    /// (`check_returned_borrow`) read this state.
    fn setup_returned_borrow_ctx(
        &mut self,
        params: &[Param],
        ret_ty: &Option<Type>,
        has_self_receiver: bool,
    ) {
        let mut names = std::collections::HashSet::new();
        let mut regions: HashMap<String, String> = HashMap::new();
        if has_self_receiver {
            names.insert("self".to_string());
        }
        for p in params {
            names.insert(p.name.name.clone());
            if let TypeKind::Borrowed { region, .. } = &p.ty.kind {
                regions.insert(p.name.name.clone(), region.clone());
            }
        }
        let ret_region = match ret_ty {
            Some(t) => match &t.kind {
                TypeKind::Borrowed { region, .. } => Some(region.clone()),
                _ => None,
            },
            None => None,
        };
        // E0511: a return region must be declared on some parameter — otherwise
        // the borrow it names has no provenance and the annotation is inert.
        if let Some(r) = &ret_region {
            if !regions.values().any(|pr| pr == r) {
                if let Some(t) = ret_ty {
                    self.err(
                        "E0511",
                        format!(
                            "return type names borrow region `{r}`, but no parameter declares region `{r}` — a returned borrow must originate from a same-region parameter"
                        ),
                        t.span,
                    );
                }
            }
        }
        self.current_fn_param_names = names;
        self.current_fn_param_regions = regions;
        self.current_fn_return_region = ret_region;
    }

    /// v0.0.12 (#2/#3): validate a `return EXPR` whose value is borrow-shaped.
    ///
    /// - **#2 (E0512)** — if the signature declares a return region, a returned
    ///   borrow rooted at a parameter must root at a *same-region* parameter.
    ///   `fn f(a: borrow A str, b: borrow B str) -> borrow A str { return b; }`
    ///   is rejected (b lives in region B, not A).
    /// - **#3 (E0513)** — a `str` / `T[]` view rooted at a *local* non-Copy
    ///   owned value (a `string`, `Vec[T]`, or any Drop type — directly, or via
    ///   `as_str` / `as_slice`) dangles: that local is freed when the function
    ///   returns. Borrows rooted at parameters / `self` are caller-tied and
    ///   left alone; literals and untraceable call results are conservatively
    ///   allowed.
    fn check_returned_borrow(&mut self, e: &Expr) {
        // v0.0.13 (Tier 1): a view rooted at a local escaping *inside a returned
        // aggregate literal* — `return Holder { view: s.as_str() };`. The bare-
        // view return below (#3) only fires when the return *type* is a view;
        // this catches the same dangle hidden in a struct/array/tuple the
        // function returns. Runs regardless of return type.
        self.flag_escaping_local_views(e);

        let ret_borrow_shaped = matches!(self.current_return, Ty::Str | Ty::Slice(_));
        let ret_region = self.current_fn_return_region.clone();
        if !ret_borrow_shaped && ret_region.is_none() {
            return;
        }
        let roots = self.returned_borrow_roots(e);
        if roots.is_empty() {
            return; // literal ('static) or untraceable — not provably dangling
        }
        // #2 — region provenance on a region-annotated return.
        for root in &roots {
            if let Some(r) = &ret_region {
                if let Some(pr) = self.current_fn_param_regions.get(root) {
                    if pr != r {
                        self.err(
                            "E0512",
                            format!(
                                "returning a borrow from region `{pr}` where the signature declares region `{r}` — the returned borrow must come from a `{r}`-region parameter"
                            ),
                            e.span,
                        );
                        return;
                    }
                }
            }
        }
        // #3 — dangling view into a local owned value.
        if ret_borrow_shaped {
            for root in &roots {
                if !self.current_fn_param_names.contains(root) {
                    let local_ty = self.lookup_local(root).map(|i| i.ty.clone());
                    if let Some(ty) = local_ty {
                        if !self.is_copy(&ty) {
                            self.err(
                                "E0513",
                                format!(
                                    "cannot return a borrow of local `{root}`: it owns heap that is freed when the function returns, so the returned view would dangle. Return an owned value (`string` / `Vec[T]`) instead, or borrow from a parameter"
                                ),
                                e.span,
                            );
                            return;
                        }
                    }
                }
            }
        }
    }

    /// v0.0.13 (Tier 1): walk a returned **aggregate literal** and flag any
    /// view leaf (`local.as_str()` / `local.as_slice()`) rooted at a non-Copy
    /// local — that local drops at return, so the stored view would dangle.
    /// Only engages for aggregate literals; a bare-view return is left to the
    /// `ret_borrow_shaped` path (#3) so the two don't double-report. Only the
    /// unambiguous view-producing forms are flagged, so moving an owned
    /// `string`/`Vec` field (`Holder { s: local }`) is never a false positive.
    fn flag_escaping_local_views(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::StructLit { .. }
            | ExprKind::GenericStructLit { .. }
            | ExprKind::ArrayLit { .. }
            | ExprKind::TupleLit { .. }
            | ExprKind::ArrayFill { .. } => self.flag_view_leaves(e),
            _ => {}
        }
    }

    fn flag_view_leaves(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::StructLit { fields, .. } | ExprKind::GenericStructLit { fields, .. } => {
                for f in fields {
                    self.flag_view_leaves(&f.value);
                }
            }
            ExprKind::ArrayLit { elements } | ExprKind::TupleLit { elements } => {
                for el in elements {
                    self.flag_view_leaves(el);
                }
            }
            ExprKind::ArrayFill { fill, .. } => self.flag_view_leaves(fill),
            // Leaf: a `local.as_str()` / `local.as_slice()` borrowing a non-Copy local.
            _ => {
                if let Some(root) = view_producing_root(e) {
                    if !self.current_fn_param_names.contains(root) {
                        if let Some(ty) = self.lookup_local(root).map(|i| i.ty.clone()) {
                            if !self.is_copy(&ty) {
                                self.err(
                                    "E0513",
                                    format!(
                                        "view of local `{root}` escapes inside the returned value: `{root}` is freed when the function returns, so the stored view would dangle. Store an owned `string` / `Vec[T]`, or borrow the view from a parameter"
                                    ),
                                    e.span,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn returned_borrow_roots(&self, e: &Expr) -> BTreeSet<String> {
        let mut roots = BTreeSet::new();
        let Some(root) = returned_borrow_root(e) else {
            return roots;
        };
        if let Some(info) = self.lookup_local(root) {
            if !info.borrow_roots.is_empty() {
                roots.extend(info.borrow_roots.iter().cloned());
                return roots;
            }
        }
        roots.insert(root.to_string());
        roots
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
                        format!(
                            "`{}` requires numeric operands, found `{}`",
                            op_str(op),
                            lhs_ty.name()
                        ),
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
                // v0.0.5: generic-parameter operands hit a separate path —
                // C+ doesn't have operator overloading (SKILL.md §2.6), so
                // `<` on a `T: Ord` cannot desugar to `T::cmp` the way it
                // would in Rust/C++. Without this catch, sema let the
                // comparison through, monomorphization substituted `T` with
                // a concrete struct like `Point`, and codegen emitted
                // `icmp slt %Point %a, %b` — LLVM rejected the IR ("icmp
                // requires integer operands"). Diagnose at the source level
                // and point users to the `cmp()` method call shape.
                if let Ty::Param(pname) = &lhs_ty {
                    self.err(
                        "E0302",
                        format!(
                            "ordered comparison on generic type parameter `{}` is not supported; \
                             use `a.cmp(b)` (returns i32) and compare its result \
                             — C+ has no operator overloading (§2.6)",
                            pname
                        ),
                        span,
                    );
                    let _ = self.check_expr(rhs, None);
                    return Ty::Bool;
                }
                if !lhs_ty.is_numeric() && lhs_ty != Ty::Error {
                    self.err(
                        "E0302",
                        format!(
                            "ordered comparison requires numeric operands, found `{}`",
                            lhs_ty.name()
                        ),
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
                        format!(
                            "`{}` requires integer operands, found `{}`",
                            op_str(op),
                            lhs_ty.name()
                        ),
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
                        format!(
                            "`{}` requires integer operands, found `{}`",
                            op_str(op),
                            lhs_ty.name()
                        ),
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
                        format!(
                            "`{}` requires an integer left operand, found `{}`",
                            op_str(op),
                            lhs_ty.name()
                        ),
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

    fn check_unary(
        &mut self,
        op: UnaryOp,
        operand: &Expr,
        expected: Option<Ty>,
        span: ByteSpan,
    ) -> Ty {
        match op {
            UnaryOp::Neg => {
                // v0.0.12 G-023: propagate the expected type into the
                // operand so `let x: i64 = -100;` works the same way
                // `let x: i64 = 100;` already does. The literal/float-lit
                // checkers (`check_int_lit` / `check_float_lit`) already
                // honor an expected type that's numeric; for non-numeric
                // expected types this is harmless (the existing operand
                // checks below still gate). Unsigned expected types fall
                // through to the `is_unsigned_int` error below with the
                // same message as before.
                let op_expected = expected.filter(|t| t.is_signed_int() || t.is_float());
                let t = self.check_expr(operand, op_expected);
                if t == Ty::Error {
                    return Ty::Error;
                }
                if t.is_unsigned_int() {
                    self.err(
                        "E0302",
                        format!(
                            "cannot negate unsigned type `{}`; use a signed type instead",
                            t.name()
                        ),
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
            UnaryOp::Not => {
                self.check_expr(operand, Some(Ty::Bool));
                Ty::Bool
            }
            UnaryOp::BitNot => {
                // Phase 3A: bitwise NOT is defined on every integer type.
                // Codegen lowers via `xor i<N> v, -1` per LLVM idiom.
                let t = self.check_expr(operand, None);
                if t == Ty::Error {
                    return Ty::Error;
                }
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
                self.err(
                    "E0312",
                    "references are not yet supported (Phase 5/6)".to_string(),
                    span,
                );
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
                        "dereferencing a raw pointer is unsafe; wrap in `unsafe { ... }`"
                            .to_string(),
                        span,
                    );
                }
                match op_ty {
                    Ty::RawPtr(inner) => *inner,
                    Ty::Error => Ty::Error,
                    other => {
                        self.err(
                            "E0302",
                            format!(
                                "dereference requires a raw pointer (`*T`), got `{}`",
                                ty_display(&other)
                            ),
                            span,
                        );
                        Ty::Error
                    }
                }
            }
        }
    }

    fn check_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr, span: ByteSpan) -> Ty {
        // v0.0.9 Phase 4: assignment to a module-scope `static`.
        // Routed before the local-binding path because static names
        // don't appear in `self.scopes`. Immutable `static` writes
        // fail with the same diagnostic shape as an immutable local
        // (E0305); `static mut` writes require `unsafe { ... }` (E0X34).
        if let ExprKind::Ident(name) = &target.kind {
            if self.lookup_local(name).is_none() {
                if let Some(info) = self.statics_table.get(name).cloned() {
                    if !info.is_mut {
                        self.err(
                            "E0305",
                            format!("cannot assign to immutable `static {name}`; declare it as `static mut` to permit writes"),
                            target.span,
                        );
                        let _ = self.check_expr(value, Some(info.ty));
                        return Ty::Error;
                    }
                    if self.unsafe_depth == 0 {
                        self.err(
                            "E0X34",
                            format!("write to `static mut {name}` requires an enclosing `unsafe {{ ... }}` block"),
                            span,
                        );
                    }
                    let _ = self.check_expr(value, Some(info.ty));
                    return Ty::Unit;
                }
            }
        }
        // Special case: first write to an unassigned binding via a direct
        // Ident target. This is the initialization site of a `let x: T;`
        // — allowed regardless of `mut` (it's the binding's first value,
        // not a reassignment). After this, the binding is marked assigned
        // and any further writes require the binding to be `mut`.
        // Compound assigns can't init (no prior value to read), so only
        // plain `=` takes this path.
        if matches!(op, AssignOp::Assign) {
            if let ExprKind::Ident(name) = &target.kind {
                let unassigned = self
                    .lookup_local(name)
                    .map(|info| !info.assigned)
                    .unwrap_or(false);
                if unassigned {
                    let target_ty = self
                        .lookup_local(name)
                        .map(|i| i.ty.clone())
                        .unwrap_or(Ty::Error);
                    if target_ty != Ty::Error {
                        self.check_expr(value, Some(target_ty.clone()));
                    } else {
                        let _ = self.check_expr(value, None);
                    }
                    let borrow_roots = if matches!(target_ty, Ty::Str | Ty::Slice(_)) {
                        self.returned_borrow_roots(value)
                    } else {
                        BTreeSet::new()
                    };
                    for scope in self.scopes.iter_mut().rev() {
                        if let Some(info) = scope.get_mut(name) {
                            info.assigned = true;
                            info.borrow_roots = borrow_roots.clone();
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
        let value_ty = self.check_expr(
            value,
            if target_ty == Ty::Error {
                None
            } else {
                Some(target_ty.clone())
            },
        );
        // v0.0.14 soundness: a plain `=` moves the RHS into the target. Moving a
        // non-Copy field/index out of a Drop aggregate is E0509 here too (same
        // as `let` / `return`) — otherwise the source's destructor double-frees
        // the moved field. (Compound ops read a Copy numeric, so they're exempt.)
        if matches!(op, AssignOp::Assign) && value_ty != Ty::Error {
            self.reject_partial_move_of_drop(value, &value_ty);
        }
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
                AssignOp::BitOrAssign => ("`|=`", false, true),
                AssignOp::BitXorAssign => ("`^=`", false, true),
                AssignOp::ShlAssign => ("`<<=`", false, true),
                AssignOp::ShrAssign => ("`>>=`", false, true),
                AssignOp::Assign => unreachable!(),
            };
            let int_ok = target_ty.is_signed_int() || target_ty.is_unsigned_int();
            let arith_ok = int_ok || matches!(target_ty, Ty::F32 | Ty::F64);
            if is_arith && !arith_ok {
                self.err(
                    "E0302",
                    format!(
                        "{op_label} requires a numeric type, got `{}`",
                        ty_display(&target_ty)
                    ),
                    span,
                );
            }
            if is_bitwise && !int_ok {
                self.err(
                    "E0302",
                    format!(
                        "{op_label} requires an integer type, got `{}`",
                        ty_display(&target_ty)
                    ),
                    span,
                );
            }
        }
        if matches!(op, AssignOp::Assign) {
            if let ExprKind::Ident(name) = &target.kind {
                let borrow_roots = if matches!(target_ty, Ty::Str | Ty::Slice(_)) {
                    self.returned_borrow_roots(value)
                } else {
                    BTreeSet::new()
                };
                for scope in self.scopes.iter_mut().rev() {
                    if let Some(info) = scope.get_mut(name) {
                        info.borrow_roots = borrow_roots.clone();
                        break;
                    }
                }
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
                    // v0.0.12 G-034 (llama.cplus): not a local — a module-scope
                    // `static mut` is a writable place root. `TABLE[i] = v` and
                    // `STATIC.field = v` reach here through the Index / Field
                    // arms' recursion on their receiver; the scalar `STATIC = v`
                    // case is handled earlier in `check_assign`. The `unsafe`
                    // requirement is enforced when the receiver is read (the
                    // Index/Field arm calls `check_expr`, which routes a
                    // `static mut` read through E0X33). An immutable `static`
                    // is E0305; anything else is genuinely undefined (E0300).
                    if let Some(s) = self.statics_table.get(name).cloned() {
                        if !s.is_mut {
                            self.err(
                                "E0305",
                                format!("cannot assign to immutable `static {name}`; declare it as `static mut` to permit writes"),
                                target.span,
                            );
                            return false;
                        }
                        return true;
                    }
                    self.err("E0300", format!("undefined name `{name}`"), target.span);
                    return false;
                };
                if !info.mutable {
                    self.err(
                        "E0305",
                        format!(
                            "cannot assign to immutable binding `{name}`; declare it as `let mut`"
                        ),
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
            ExprKind::Unary {
                op: UnaryOp::Deref,
                operand,
            } => {
                let op_ty = self.check_expr(operand, None);
                if !matches!(op_ty, Ty::RawPtr(_) | Ty::Error) {
                    self.err(
                        "E0302",
                        format!(
                            "dereference assignment target must be a raw pointer, got `{}`",
                            ty_display(&op_ty)
                        ),
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
            TypeKind::Array { elem, len, .. } => {
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
            TypeKind::FnPtr {
                params,
                return_type,
            } => {
                let resolved_params: Vec<Ty> =
                    params.iter().map(|p| self.resolve_type(p)).collect();
                let resolved_ret = match return_type {
                    Some(rt) => self.resolve_type(rt),
                    None => Ty::Unit,
                };
                return Ty::FnPtr {
                    params: resolved_params,
                    return_type: Box::new(resolved_ret),
                };
            }
            // Phase 11 polish (2026-05-14): slice type `T[]`.
            TypeKind::Slice(inner) => {
                let inner_ty = self.resolve_type(inner);
                return Ty::Slice(Box::new(inner_ty));
            }
            // v0.0.5 Phase 3 Slice 3B: tuple type `(T1, T2, ...)`.
            // Resolve each element type then synthesize (or look up) a
            // concrete tuple struct with fields `_0`, `_1`, ...
            TypeKind::Tuple(elems) => {
                let elem_tys: Vec<Ty> = elems.iter().map(|e| self.resolve_type(e)).collect();
                return self.synthesize_tuple_struct(&elem_tys, t.span);
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
            // v0.0.12 G-026: `()` as a type — the unit type. Parser
            // produces `Path("()")` for source-level `()` (in turbofish
            // type args, fn-pointer return types, etc.) so it resolves
            // through this name path. `Ty::Unit` is the same unit type
            // implicit `fn f() { ... }` returns.
            "()" => Ty::Unit,
            // v0.0.6 Slice 1B: SIMD type names. Each entry here must also
            // appear in `simd_ty_from_name` (free fn below) for path
            // dispatch, and in codegen's `simd_ty_from_name` mirror.
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
            // v0.0.12 SIMD Tier-1 (G-039a): 64-bit (sub-128) widths — the
            // NEON D-register family. Mostly produced by `i8x16::low`/`high`
            // and consumed by `widen` / `combine`; also constructible
            // directly via `splat`/`new`. Same elem leaves, half the lanes.
            "i8x8" => Ty::Simd { elem: Box::new(Ty::I8), lanes: 8 },
            "u8x8" => Ty::Simd { elem: Box::new(Ty::U8), lanes: 8 },
            "i16x4" => Ty::Simd { elem: Box::new(Ty::I16), lanes: 4 },
            "u16x4" => Ty::Simd { elem: Box::new(Ty::U16), lanes: 4 },
            "i32x2" => Ty::Simd { elem: Box::new(Ty::I32), lanes: 2 },
            "u32x2" => Ty::Simd { elem: Box::new(Ty::U32), lanes: 2 },
            "f32x2" => Ty::Simd { elem: Box::new(Ty::F32), lanes: 2 },
            // v0.0.7 Slice 2.2: 256-bit widths. AArch64 splits these
            // into two 128-bit ops at codegen; AVX2 / SVE2 hosts use
            // native 256-bit vectors. Same elem-type leaves as the
            // 128-bit family — the type-name and lane-count are the
            // only thing that changes.
            "f32x8" => Ty::Simd {
                elem: Box::new(Ty::F32),
                lanes: 8,
            },
            "f64x4" => Ty::Simd {
                elem: Box::new(Ty::F64),
                lanes: 4,
            },
            "i8x32" => Ty::Simd {
                elem: Box::new(Ty::I8),
                lanes: 32,
            },
            "u8x32" => Ty::Simd {
                elem: Box::new(Ty::U8),
                lanes: 32,
            },
            "i16x16" => Ty::Simd {
                elem: Box::new(Ty::I16),
                lanes: 16,
            },
            "u16x16" => Ty::Simd {
                elem: Box::new(Ty::U16),
                lanes: 16,
            },
            "i32x8" => Ty::Simd {
                elem: Box::new(Ty::I32),
                lanes: 8,
            },
            "u32x8" => Ty::Simd {
                elem: Box::new(Ty::U32),
                lanes: 8,
            },
            "i64x4" => Ty::Simd {
                elem: Box::new(Ty::I64),
                lanes: 4,
            },
            "u64x4" => Ty::Simd {
                elem: Box::new(Ty::U64),
                lanes: 4,
            },
            // v0.0.9 follow-up: mask types are a distinct `Ty::Mask`
            // variant. Codegen lowers them identically to the matching
            // signed-int SIMD, but sema rejects Mask <-> Simd implicit
            // assignment, requires `Ty::Mask` for `select` / `any` /
            // `all`, and rejects arithmetic. Use `.to_bits()` /
            // `.to_mask()` for explicit conversions.
            "mask8x16" => Ty::Mask {
                elem: Box::new(Ty::I8),
                lanes: 16,
            },
            "mask16x8" => Ty::Mask {
                elem: Box::new(Ty::I16),
                lanes: 8,
            },
            "mask32x4" => Ty::Mask {
                elem: Box::new(Ty::I32),
                lanes: 4,
            },
            "mask64x2" => Ty::Mask {
                elem: Box::new(Ty::I64),
                lanes: 2,
            },
            "mask8x32" => Ty::Mask {
                elem: Box::new(Ty::I8),
                lanes: 32,
            },
            "mask16x16" => Ty::Mask {
                elem: Box::new(Ty::I16),
                lanes: 16,
            },
            "mask32x8" => Ty::Mask {
                elem: Box::new(Ty::I32),
                lanes: 8,
            },
            "mask64x4" => Ty::Mask {
                elem: Box::new(Ty::I64),
                lanes: 4,
            },
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
    fn resolve_generic_instantiation(&mut self, name: &str, args: &[Type], span: ByteSpan) -> Ty {
        // Slice 7GEN.5d: try enum templates first. Struct + enum names
        // live in different tables but the resolution shape is parallel.
        if self.enum_generic_templates.contains_key(name) {
            return self.resolve_generic_enum_instantiation(name, args, span);
        }
        let Some(template) = self.struct_generic_templates.get(name).cloned() else {
            self.err("E0303", format!("unknown generic type `{}`", name), span);
            return Ty::Error;
        };
        if args.len() != template.generic_params.len() {
            self.err(
                "E0501",
                format!(
                    "type `{}` takes {} type argument(s), got {}",
                    name,
                    template.generic_params.len(),
                    args.len()
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
        let param_names: Vec<String> = template
            .generic_params
            .iter()
            .map(|g| g.name.name.clone())
            .collect();
        let bounds: Vec<Vec<String>> = template
            .generic_params
            .iter()
            .map(|g| g.bounds.iter().map(|b| b.name.clone()).collect())
            .collect();
        self.check_generic_bounds(
            &param_names,
            &bounds,
            &arg_tys,
            span,
            &format!("generic struct `{}`", name),
        );
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
            is_copy: false, // recomputed by compute_struct_copy_flags? not for late-synthesized
            is_drop: false,
            is_repr_c: false, // generic instantiations don't inherit repr(C); revisit when use case appears
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
                    let raw: Vec<(Ty, bool, bool, bool)> = t
                        .params
                        .iter()
                        .map(|p| (p.ty.clone(), p.mutable, p.move_, p.borrow_))
                        .collect();
                    raw.into_iter()
                        .map(|(ty, mutable, move_, borrow_)| {
                            let s = self.subst_ty_deep(&ty, &method_subst);
                            ParamSig {
                                ty: subst_self(&s, &self_ty),
                                mutable,
                                move_,
                                borrow_,
                            }
                        })
                        .collect()
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
            Ty::FnPtr {
                params,
                return_type,
            } => {
                let params = params
                    .iter()
                    .map(|p| self.subst_ty_deep(p, subst))
                    .collect();
                let return_type = Box::new(self.subst_ty_deep(return_type, subst));
                Ty::FnPtr {
                    params,
                    return_type,
                }
            }
            Ty::Struct(id) => {
                let origin = self.structs[id.0 as usize].generic_origin.clone();
                let Some((name, args)) = origin else {
                    return ty.clone();
                };
                let new_args: Vec<Ty> = args.iter().map(|a| self.subst_ty_deep(a, subst)).collect();
                if new_args == args {
                    return ty.clone();
                }
                // Re-instantiate. The template lookup must succeed — we
                // wouldn't have a generic_origin recorded otherwise.
                let template = self
                    .struct_generic_templates
                    .get(&name)
                    .cloned()
                    .expect("generic_origin names a template not in struct_generic_templates");
                self.instantiate_struct_from_arg_tys(&name, &template, new_args)
            }
            Ty::Enum(id) => {
                let origin = self.enums[id.0 as usize].generic_origin.clone();
                let Some((name, args)) = origin else {
                    return ty.clone();
                };
                let new_args: Vec<Ty> = args.iter().map(|a| self.subst_ty_deep(a, subst)).collect();
                if new_args == args {
                    return ty.clone();
                }
                let template = self
                    .enum_generic_templates
                    .get(&name)
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
            self.err("E0303", format!("unknown generic enum `{}`", name), span);
            return Ty::Error;
        };
        if args.len() != template.generic_params.len() {
            self.err(
                "E0501",
                format!(
                    "enum `{}` takes {} type argument(s), got {}",
                    name,
                    template.generic_params.len(),
                    args.len()
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
        let param_names: Vec<String> = template
            .generic_params
            .iter()
            .map(|g| g.name.name.clone())
            .collect();
        let bounds: Vec<Vec<String>> = template
            .generic_params
            .iter()
            .map(|g| g.bounds.iter().map(|b| b.name.clone()).collect())
            .collect();
        self.check_generic_bounds(
            &param_names,
            &bounds,
            &arg_tys,
            span,
            &format!("generic enum `{}`", name),
        );
        self.instantiate_enum_from_arg_tys(name, &template, arg_tys)
    }

    /// v0.0.5 Phase 3 Slice 3C: walk generic-impl-method bodies for
    /// `match`/`if let`/`while let`/`guard let` patterns whose
    /// `PatternKind::Variant` carries explicit type-args. For each
    /// concrete struct instantiation, substitute the impl's generic
    /// params and instantiate the discovered enums. See the explanatory
    /// comment at the call site in `check_with_files_inner`.
    fn propagate_pattern_instantiations(&mut self, program: &Program) {
        // Worklist seed: every concrete struct_instantiation. Each entry
        // walks its matching `impl <name>[<params>]` body once; new
        // enum-instantiations discovered along the way don't themselves
        // produce more struct-instantiations (struct fields are AST
        // `Type` nodes which can't carry `PatternKind::Variant`), so
        // a single pass over the seed set suffices — no fixed-point
        // loop needed.
        //
        // Snapshot the seed so the mutable `instantiate_enum_from_arg_tys`
        // calls inside the loop don't fight the borrow checker.
        let seed: Vec<(String, Vec<Ty>)> = self
            .struct_instantiations
            .iter()
            .filter(|(key, _)| {
                !key.1
                    .iter()
                    .any(|t| ty_contains_param(t, &self.structs, &self.enums))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for (sname, sargs) in seed {
            // Find the generic impl block matching this struct's template.
            for item in &program.items {
                let ItemKind::Impl(b) = &item.kind else {
                    continue;
                };
                if b.target_generic_params.is_empty() {
                    continue;
                }
                if b.target.name != sname {
                    continue;
                }
                if b.target_generic_params.len() != sargs.len() {
                    continue;
                }
                // Build a name → concrete-Ty subst from the impl's
                // target generic params.
                let subst: HashMap<String, Ty> = b
                    .target_generic_params
                    .iter()
                    .zip(sargs.iter())
                    .map(|(gp, t)| (gp.name.name.clone(), t.clone()))
                    .collect();
                // Collect all variant patterns in the impl's method
                // bodies into a flat list first; instantiation needs
                // `&mut self`, but the walker only needs an immutable
                // borrow of `program`.
                let mut discoveries: Vec<(String, Vec<Type>)> = Vec::new();
                for m in &b.methods {
                    walk_variant_patterns_in_block(&m.body, &mut |enum_name, type_args| {
                        if type_args.is_empty() {
                            return;
                        }
                        discoveries.push((enum_name.to_string(), type_args.to_vec()));
                    });
                }
                for (enum_name, type_args) in discoveries {
                    self.try_instantiate_enum_from_pattern_args(&enum_name, &type_args, &subst);
                }
            }
        }
        // Same pass for generic free fns: every concrete `fn_instantiation`
        // walks its template body. Sema doesn't type-check generic fn
        // bodies (see `check_function`'s early-return on `fns.get`
        // miss), so variant patterns inside `fn map[T, U](...)` would
        // otherwise be invisible.
        let fn_seed: Vec<(String, Vec<Ty>)> = self
            .fn_instantiations
            .iter()
            .filter(|(_, args)| {
                !args
                    .iter()
                    .any(|t| ty_contains_param(t, &self.structs, &self.enums))
            })
            .cloned()
            .collect();
        for (fname, fargs) in fn_seed {
            for item in &program.items {
                let ItemKind::Function(f) = &item.kind else {
                    continue;
                };
                if f.name.name != fname {
                    continue;
                }
                if f.generic_params.len() != fargs.len() {
                    continue;
                }
                let subst: HashMap<String, Ty> = f
                    .generic_params
                    .iter()
                    .zip(fargs.iter())
                    .map(|(gp, t)| (gp.name.name.clone(), t.clone()))
                    .collect();
                let mut discoveries: Vec<(String, Vec<Type>)> = Vec::new();
                walk_variant_patterns_in_block(&f.body, &mut |enum_name, type_args| {
                    if type_args.is_empty() {
                        return;
                    }
                    discoveries.push((enum_name.to_string(), type_args.to_vec()));
                });
                for (enum_name, type_args) in discoveries {
                    self.try_instantiate_enum_from_pattern_args(&enum_name, &type_args, &subst);
                }
            }
        }
    }

    /// v0.0.5 Phase 3 Slice 3C helper: substitute the given subst through
    /// each pattern type-arg, resolve to `Ty`, and feed to
    /// `instantiate_enum_from_arg_tys`. No-op when the enum isn't
    /// generic, the arity mismatches, or any arg still references a
    /// type-param after substitution (the latter happens when a generic
    /// impl-method body references an enum with a generic param the
    /// outer subst doesn't bind — pattern discovery from that inner
    /// substitution is the caller's responsibility).
    fn try_instantiate_enum_from_pattern_args(
        &mut self,
        enum_name: &str,
        type_args: &[Type],
        subst: &HashMap<String, Ty>,
    ) {
        let Some(template) = self.enum_generic_templates.get(enum_name).cloned() else {
            return;
        };
        if template.generic_params.len() != type_args.len() {
            return;
        }
        let resolved_args: Vec<Ty> = type_args
            .iter()
            .map(|t| {
                let substituted =
                    substitute_param_in_type_ast_with_tables(t, subst, &self.structs, &self.enums);
                self.resolve_field_type_with_subst(&substituted, subst)
            })
            .collect();
        if resolved_args
            .iter()
            .any(|t| ty_contains_param(t, &self.structs, &self.enums) || matches!(t, Ty::Error))
        {
            return;
        }
        self.instantiate_enum_from_arg_tys(enum_name, &template, resolved_args);
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
            methods: HashMap::new(),
        });
        self.enum_by_name.insert(mangled, id);
        self.enum_instantiations.insert(key, id);
        // v0.0.5 generic-enum impl carry-over: populate methods on the
        // synthesized concrete enum from any `impl Option[T] { ... }`
        // generic impl block registered for this template. Mirror of
        // the struct-side population path; substitutes impl-level `T`
        // refs (Ty::Param) with the concrete arg types, `Self` refs
        // resolve to the new Ty::Enum(id).
        if let Some(templates) = self.generic_impl_methods.get(name).cloned() {
            let self_ty = Ty::Enum(id);
            for t in &templates {
                let mut method_subst: HashMap<String, Ty> = HashMap::new();
                for (gp, arg) in t.impl_generic_params.iter().zip(arg_tys.iter()) {
                    method_subst.insert(gp.clone(), arg.clone());
                }
                let resolved_params: Vec<ParamSig> = {
                    let raw: Vec<(Ty, bool, bool, bool)> = t
                        .params
                        .iter()
                        .map(|p| (p.ty.clone(), p.mutable, p.move_, p.borrow_))
                        .collect();
                    raw.into_iter()
                        .map(|(ty, mutable, move_, borrow_)| {
                            let s = self.subst_ty_deep(&ty, &method_subst);
                            ParamSig {
                                ty: subst_self(&s, &self_ty),
                                mutable,
                                move_,
                                borrow_,
                            }
                        })
                        .collect()
                };
                let resolved_return = {
                    let s = self.subst_ty_deep(&t.return_type, &method_subst);
                    subst_self(&s, &self_ty)
                };
                self.enums[id.0 as usize].methods.insert(
                    t.name.clone(),
                    MethodSig {
                        receiver: t.receiver,
                        params: resolved_params,
                        return_type: resolved_return,
                        generic_params: t.method_generic_params.clone(),
                        generic_bounds: Vec::new(),
                    },
                );
            }
        }
        Ty::Enum(id)
    }

    fn resolve_field_type_with_subst(&mut self, ty: &Type, subst: &HashMap<String, Ty>) -> Ty {
        match &ty.kind {
            TypeKind::Path(name) => {
                if let Some(concrete) = subst.get(name) {
                    return concrete.clone();
                }
                self.resolve_type(ty)
            }
            TypeKind::Array { elem, len, .. } => {
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
                    .map(|a| {
                        substitute_param_in_type_ast_with_tables(
                            a,
                            subst,
                            &self.structs,
                            &self.enums,
                        )
                    })
                    .collect();
                let synthetic = Type {
                    kind: TypeKind::Generic {
                        name: name.clone(),
                        args: substituted_args,
                    },
                    span: ty.span,
                };
                self.resolve_type(&synthetic)
            }
            TypeKind::RawPtr(inner) => {
                let inner_ty = self.resolve_field_type_with_subst(inner, subst);
                Ty::RawPtr(Box::new(inner_ty))
            }
            TypeKind::FnPtr {
                params,
                return_type,
            } => {
                let resolved_params: Vec<Ty> = params
                    .iter()
                    .map(|p| self.resolve_field_type_with_subst(p, subst))
                    .collect();
                let resolved_ret = match return_type {
                    Some(rt) => self.resolve_field_type_with_subst(rt, subst),
                    None => Ty::Unit,
                };
                Ty::FnPtr {
                    params: resolved_params,
                    return_type: Box::new(resolved_ret),
                }
            }
            TypeKind::Slice(inner) => {
                let inner_ty = self.resolve_field_type_with_subst(inner, subst);
                Ty::Slice(Box::new(inner_ty))
            }
            TypeKind::Tuple(elems) => {
                let elem_tys: Vec<Ty> = elems
                    .iter()
                    .map(|e| self.resolve_field_type_with_subst(e, subst))
                    .collect();
                self.synthesize_tuple_struct(&elem_tys, ty.span)
            }
        }
    }

    /// Slice 7GEN.4: is `name` a generic-parameter name visible at the
    /// current point? Consults the entire stack (inner scopes shadow
    /// outer ones is irrelevant here — we just need to know whether the
    /// name is a type-parameter anywhere up the chain).
    fn type_param_in_scope(&self, name: &str) -> bool {
        self.type_params_stack
            .iter()
            .any(|frame| frame.contains(name))
    }

    fn push_type_params(&mut self, params: &[GenericParam]) {
        let frame: std::collections::HashSet<String> =
            params.iter().map(|p| p.name.name.clone()).collect();
        self.type_params_stack.push(frame);
        // v0.0.5: parallel frame tracking the interface bounds per param.
        // Used by method-call dispatch on `Ty::Param(name)`: when the body
        // calls `t.cmp(other)` with `t: T` and the active generic context
        // declares `T: Ord`, we resolve `cmp` against `Ord`'s method table
        // rather than emitting E0324.
        let bound_frame: HashMap<String, Vec<String>> = params
            .iter()
            .map(|p| {
                (
                    p.name.name.clone(),
                    p.bounds.iter().map(|b| b.name.clone()).collect(),
                )
            })
            .collect();
        self.param_bounds_stack.push(bound_frame);
    }

    fn pop_type_params(&mut self) {
        self.type_params_stack.pop();
        self.param_bounds_stack.pop();
    }

    /// v0.0.5: when `Ty::Param(name)` appears as a method-call receiver,
    /// walk the active bound stack to find the first frame that declares
    /// a bound for `name`, then return the union of method signatures
    /// across every named interface bound. Used by `check_method_call`
    /// to make `T: Ord` → `t.cmp(other)` resolve.
    fn lookup_bound_method(&self, param_name: &str, method: &str) -> Option<MethodSig> {
        for frame in self.param_bounds_stack.iter().rev() {
            if let Some(bounds) = frame.get(param_name) {
                for iface_name in bounds {
                    if let Some(iface) = self.interfaces.get(iface_name) {
                        if let Some(sig) = iface.methods.get(method) {
                            return Some(sig.clone());
                        }
                    }
                }
                // Param name found in this frame but no bound provides
                // the method — shadowing rules say inner frames mask
                // outer ones, so stop here.
                return None;
            }
        }
        None
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
            self.err(
                "E0303",
                format!("unknown type `{}`", enum_seg.name),
                enum_seg.span,
            );
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
                self.err("E0335", format!("use of moved value `{name}`"), span);
            } else if !assigned {
                self.err(
                    "E0345",
                    format!("use of possibly-unassigned binding `{name}`; assign it on every control-flow path before reading"),
                    span,
                );
            }
            return ty;
        }
        // v0.0.9 Phase 4: module-scope `static` lookup. Immutable
        // `static` is readable from any context. `static mut` reads
        // require an enclosing `unsafe { ... }` block — the borrow
        // checker can't prove absence of data races on module-scope
        // mutable state.
        if let Some(info) = self.statics_table.get(name).cloned() {
            if info.is_mut && self.unsafe_depth == 0 {
                self.err(
                    "E0X33",
                    format!("read of `static mut {name}` requires an enclosing `unsafe {{ ... }}` block"),
                    span,
                );
            }
            return info.ty;
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
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
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
            if a_len != b_len {
                return false;
            }
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
/// The trailing dotted segment of a (possibly module-qualified) type name —
/// `vendor.stdlib.src.arc.Arc` → `Arc`, `Handle` → `Handle`. Used to match
/// `unsafe impl Send/Sync` overrides by leaf so registration (which sees the
/// qualified target name in multi-file builds) and lookup (which sees the
/// instantiation's template leaf) agree.
fn name_leaf(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// v0.0.14 inline asm Tier 2: a type that fits in a single general register —
/// the only operand types supported today. Floats (which need an `f`-class
/// constraint) and aggregates are rejected (E0892).
fn is_asm_scalar(ty: &Ty) -> bool {
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
            | Ty::RawPtr(_)
    )
}

/// v0.0.14 inline asm Tier 3: is this expression nothing but inline asm? `#asm`,
/// or an `unsafe { ... }` / block wrapping asm-only statements. Used to verify a
/// `#[naked]` body.
fn expr_is_asm_only(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Asm { .. } => true,
        ExprKind::Unsafe(b) | ExprKind::Block(b) => {
            b.stmts.iter().all(stmt_is_asm_only)
                && b.tail.as_deref().map(expr_is_asm_only).unwrap_or(true)
        }
        _ => false,
    }
}

fn stmt_is_asm_only(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Expr(e) => expr_is_asm_only(e),
        _ => false,
    }
}

fn ty_contains_param(ty: &Ty, structs: &[StructDef], enums: &[EnumDef]) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::Array(elem, _) => ty_contains_param(elem, structs, enums),
        Ty::RawPtr(inner) => ty_contains_param(inner, structs, enums),
        Ty::FnPtr {
            params,
            return_type,
        } => {
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

/// v0.0.5 Phase 3 Slice 3C: walk a `Block` and invoke `f` with
/// `(pat_enum_name, pat_type_args)` for every `PatternKind::Variant`
/// reachable through `match`/`if let`/`while let`/`guard let` arm and
/// payload sub-patterns. Used by `propagate_pattern_instantiations` to
/// discover enums mentioned only at pattern positions inside generic
/// method bodies — `ExprKind::Call` propagation alone misses them.
fn walk_variant_patterns_in_block(block: &Block, f: &mut impl FnMut(&str, &[Type])) {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Let { init: Some(e), .. } => walk_variant_patterns_in_expr(e, f),
            StmtKind::Let { init: None, .. } => {}
            StmtKind::Expr(e) => walk_variant_patterns_in_expr(e, f),
            StmtKind::Return(Some(e)) => walk_variant_patterns_in_expr(e, f),
            StmtKind::Return(None) => {}
            StmtKind::While { cond, body, .. } => {
                walk_variant_patterns_in_expr(cond, f);
                walk_variant_patterns_in_block(body, f);
            }
            StmtKind::For(forloop, _) => match forloop {
                ForLoop::Range { iter, body, .. } => {
                    walk_variant_patterns_in_expr(iter, f);
                    walk_variant_patterns_in_block(body, f);
                }
                ForLoop::CStyle {
                    init,
                    cond,
                    update,
                    body,
                } => {
                    if let Some(s) = init.as_deref() {
                        let wrap = Block {
                            stmts: vec![s.clone()],
                            tail: None,
                            span: stmt.span,
                        };
                        walk_variant_patterns_in_block(&wrap, f);
                    }
                    if let Some(c) = cond {
                        walk_variant_patterns_in_expr(c, f);
                    }
                    for u in update {
                        walk_variant_patterns_in_expr(u, f);
                    }
                    walk_variant_patterns_in_block(body, f);
                }
            },
            StmtKind::Defer(e) | StmtKind::Assert(e) => walk_variant_patterns_in_expr(e, f),
            StmtKind::Loop(body, _) => walk_variant_patterns_in_block(body, f),
            StmtKind::IfLet {
                pattern,
                scrutinee,
                body,
                else_body,
            } => {
                walk_variant_patterns_in_pat(pattern, f);
                walk_variant_patterns_in_expr(scrutinee, f);
                walk_variant_patterns_in_block(body, f);
                if let Some(b) = else_body {
                    walk_variant_patterns_in_block(b, f);
                }
            }
            StmtKind::WhileLet {
                pattern,
                scrutinee,
                body,
            } => {
                walk_variant_patterns_in_pat(pattern, f);
                walk_variant_patterns_in_expr(scrutinee, f);
                walk_variant_patterns_in_block(body, f);
            }
            StmtKind::GuardLet {
                pattern,
                scrutinee,
                complement,
                else_body,
            } => {
                walk_variant_patterns_in_pat(pattern, f);
                walk_variant_patterns_in_expr(scrutinee, f);
                if let Some(c) = complement {
                    walk_variant_patterns_in_pat(c, f);
                }
                walk_variant_patterns_in_block(else_body, f);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
    if let Some(t) = &block.tail {
        walk_variant_patterns_in_expr(t, f);
    }
}

fn walk_variant_patterns_in_expr(expr: &Expr, f: &mut impl FnMut(&str, &[Type])) {
    match &expr.kind {
        ExprKind::Match { scrutinee, arms } => {
            walk_variant_patterns_in_expr(scrutinee, f);
            for arm in arms {
                walk_variant_patterns_in_pat(&arm.pattern, f);
                walk_variant_patterns_in_expr(&arm.body, f);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            walk_variant_patterns_in_expr(callee, f);
            for a in args {
                walk_variant_patterns_in_expr(a, f);
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => walk_variant_patterns_in_block(b, f),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            walk_variant_patterns_in_expr(cond, f);
            walk_variant_patterns_in_block(then, f);
            if let Some(e) = else_branch {
                walk_variant_patterns_in_expr(e, f);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_variant_patterns_in_expr(lhs, f);
            walk_variant_patterns_in_expr(rhs, f);
        }
        ExprKind::Unary { operand, .. } => walk_variant_patterns_in_expr(operand, f),
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                walk_variant_patterns_in_expr(s, f);
            }
            if let Some(e) = end {
                walk_variant_patterns_in_expr(e, f);
            }
        }
        ExprKind::Assign { target, value, .. } => {
            walk_variant_patterns_in_expr(target, f);
            walk_variant_patterns_in_expr(value, f);
        }
        ExprKind::Field { receiver, .. } => walk_variant_patterns_in_expr(receiver, f),
        ExprKind::Index { receiver, index } => {
            walk_variant_patterns_in_expr(receiver, f);
            walk_variant_patterns_in_expr(index, f);
        }
        ExprKind::Cast { expr, .. } => walk_variant_patterns_in_expr(expr, f),
        ExprKind::StructLit { fields, .. } | ExprKind::GenericStructLit { fields, .. } => {
            for sf in fields {
                walk_variant_patterns_in_expr(&sf.value, f);
            }
        }
        ExprKind::ArrayLit { elements } => {
            for e in elements {
                walk_variant_patterns_in_expr(e, f);
            }
        }
        ExprKind::GenericEnumCall { args, .. } => {
            for a in args {
                walk_variant_patterns_in_expr(a, f);
            }
        }
        ExprKind::Await(inner) | ExprKind::Yield(inner) => walk_variant_patterns_in_expr(inner, f),
        ExprKind::InterpStr { parts } => {
            for p in parts {
                if let InterpStrPart::Expr(e) = p {
                    walk_variant_patterns_in_expr(e, f);
                }
            }
        }
        _ => {}
    }
}

fn walk_variant_patterns_in_pat(pat: &Pattern, f: &mut impl FnMut(&str, &[Type])) {
    if let PatternKind::Variant {
        enum_name,
        type_args,
        payload,
        ..
    } = &pat.kind
    {
        f(&enum_name.name, type_args);
        for p in payload {
            walk_variant_patterns_in_pat(p, f);
        }
    }
}

/// v0.0.7 Slice 2.1: pick the signed-integer type whose bit width
/// matches `elem`. Used by comparison ops to produce the "mask"
/// shape — a signed-int SIMD with the same lane count as the source.
fn matching_signed_int_lane(elem: &Ty) -> Ty {
    match elem {
        Ty::I8 | Ty::U8 | Ty::Bool => Ty::I8,
        Ty::I16 | Ty::U16 => Ty::I16,
        Ty::I32 | Ty::U32 | Ty::F32 => Ty::I32,
        Ty::I64 | Ty::U64 | Ty::F64 | Ty::Isize | Ty::Usize => Ty::I64,
        _ => Ty::I32, // shouldn't reach here on numeric SIMD
    }
}

/// v0.0.6 Slice 1B: parse a SIMD type name (`f32x4`, etc.) back to its
/// `Ty::Simd` representation. Used by `check_assoc_call` to recognize
/// `f32x4::splat(...)`-style paths before falling through to enum/struct
/// dispatch. First cut: only `f32x4`; other widths land in follow-on
/// slices.
fn simd_ty_from_name(name: &str) -> Option<Ty> {
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
        "i8x8" => Some(Ty::Simd { elem: Box::new(Ty::I8), lanes: 8 }),
        "u8x8" => Some(Ty::Simd { elem: Box::new(Ty::U8), lanes: 8 }),
        "i16x4" => Some(Ty::Simd { elem: Box::new(Ty::I16), lanes: 4 }),
        "u16x4" => Some(Ty::Simd { elem: Box::new(Ty::U16), lanes: 4 }),
        "i32x2" => Some(Ty::Simd { elem: Box::new(Ty::I32), lanes: 2 }),
        "u32x2" => Some(Ty::Simd { elem: Box::new(Ty::U32), lanes: 2 }),
        "f32x2" => Some(Ty::Simd { elem: Box::new(Ty::F32), lanes: 2 }),
        // v0.0.7 Slice 2.2: 256-bit widths.
        "f32x8" => Some(Ty::Simd {
            elem: Box::new(Ty::F32),
            lanes: 8,
        }),
        "f64x4" => Some(Ty::Simd {
            elem: Box::new(Ty::F64),
            lanes: 4,
        }),
        "i8x32" => Some(Ty::Simd {
            elem: Box::new(Ty::I8),
            lanes: 32,
        }),
        "u8x32" => Some(Ty::Simd {
            elem: Box::new(Ty::U8),
            lanes: 32,
        }),
        "i16x16" => Some(Ty::Simd {
            elem: Box::new(Ty::I16),
            lanes: 16,
        }),
        "u16x16" => Some(Ty::Simd {
            elem: Box::new(Ty::U16),
            lanes: 16,
        }),
        "i32x8" => Some(Ty::Simd {
            elem: Box::new(Ty::I32),
            lanes: 8,
        }),
        "u32x8" => Some(Ty::Simd {
            elem: Box::new(Ty::U32),
            lanes: 8,
        }),
        "i64x4" => Some(Ty::Simd {
            elem: Box::new(Ty::I64),
            lanes: 4,
        }),
        "u64x4" => Some(Ty::Simd {
            elem: Box::new(Ty::U64),
            lanes: 4,
        }),
        // v0.0.9 follow-up: mask types resolve to `Ty::Mask`, a sema-
        // level type distinct from Ty::Simd. Codegen lowers both to the
        // same `<N x iN>` LLVM type, so layout/ABI is unchanged.
        "mask8x16" => Some(Ty::Mask {
            elem: Box::new(Ty::I8),
            lanes: 16,
        }),
        "mask16x8" => Some(Ty::Mask {
            elem: Box::new(Ty::I16),
            lanes: 8,
        }),
        "mask32x4" => Some(Ty::Mask {
            elem: Box::new(Ty::I32),
            lanes: 4,
        }),
        "mask64x2" => Some(Ty::Mask {
            elem: Box::new(Ty::I64),
            lanes: 2,
        }),
        "mask8x32" => Some(Ty::Mask {
            elem: Box::new(Ty::I8),
            lanes: 32,
        }),
        "mask16x16" => Some(Ty::Mask {
            elem: Box::new(Ty::I16),
            lanes: 16,
        }),
        "mask32x8" => Some(Ty::Mask {
            elem: Box::new(Ty::I32),
            lanes: 8,
        }),
        "mask64x4" => Some(Ty::Mask {
            elem: Box::new(Ty::I64),
            lanes: 4,
        }),
        _ => None,
    }
}

/// Bit width of a SIMD lane scalar type. `isize`/`usize` are 64-bit on the
/// targets cpc supports today. Returns 0 for non-scalar types (callers only
/// pass SIMD element types, which are always scalar). Used by the SIMD
/// reinterpret / int↔float-convert assoc calls to check total-width and
/// lane-width compatibility.
fn simd_lane_bits(ty: &Ty) -> u32 {
    match ty {
        Ty::I8 | Ty::U8 => 8,
        Ty::I16 | Ty::U16 => 16,
        Ty::I32 | Ty::U32 | Ty::F32 => 32,
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::F64 => 64,
        _ => 0,
    }
}

/// G-038b widen: the integer lane type one step wider (i8→i16, …, i32→i64).
/// `None` for float lanes or 64-bit lanes (nothing wider). Signedness preserved.
fn simd_widen_elem(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::I8 => Some(Ty::I16),
        Ty::I16 => Some(Ty::I32),
        Ty::I32 => Some(Ty::I64),
        Ty::U8 => Some(Ty::U16),
        Ty::U16 => Some(Ty::U32),
        Ty::U32 => Some(Ty::U64),
        _ => None,
    }
}

/// G-038b narrow: the integer lane type one step narrower (i16→i8, …,
/// i64→i32). `None` for float lanes or 8-bit lanes. Signedness preserved.
fn simd_narrow_elem(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::I16 => Some(Ty::I8),
        Ty::I32 => Some(Ty::I16),
        Ty::I64 => Some(Ty::I32),
        Ty::U16 => Some(Ty::U8),
        Ty::U32 => Some(Ty::U16),
        Ty::U64 => Some(Ty::U32),
        _ => None,
    }
}

fn ty_display(ty: &Ty) -> String {
    match ty {
        Ty::Param(name) => name.clone(),
        Ty::Array(elem, n) => format!("[{}; {}]", ty_display(elem), n),
        Ty::RawPtr(inner) => format!("*{}", ty_display(inner)),
        Ty::FnPtr {
            params,
            return_type,
        } => {
            let params_s = params.iter().map(ty_display).collect::<Vec<_>>().join(", ");
            if matches!(**return_type, Ty::Unit) {
                format!("fn({params_s})")
            } else {
                format!("fn({params_s}) -> {}", ty_display(return_type))
            }
        }
        // v0.0.6 Slice 1B: SIMD vectors render as `<elem>x<lanes>`,
        // matching the source-level type names (`f32x4`, etc.).
        Ty::Simd { elem, lanes } => format!("{}x{}", ty_display(elem), lanes),
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
    substitute_param_in_type_ast_with_tables(ty, subst, &[], &[])
}

/// G-026 fix: name-aware variant of `substitute_param_in_type_ast`. When
/// substituting a `Param("T")` with a concrete `Ty::Struct(id)` or
/// `Ty::Enum(id)`, render the real source name from the struct/enum
/// tables instead of the `<concrete>` placeholder. Without this, a
/// recursive enum payload like `Value::Array(Vec[Value])` round-trips
/// `Value` through `<concrete>` and fires E0303 at re-resolution.
fn substitute_param_in_type_ast_with_tables(
    ty: &Type,
    subst: &HashMap<String, Ty>,
    structs: &[StructDef],
    enums: &[EnumDef],
) -> Type {
    let kind = match &ty.kind {
        TypeKind::Path(name) => {
            if let Some(concrete) = subst.get(name) {
                TypeKind::Path(ty_to_source_name_with_tables(concrete, structs, enums))
            } else {
                TypeKind::Path(name.clone())
            }
        }
        TypeKind::Array { elem, len, .. } => TypeKind::Array {
            elem: Box::new(substitute_param_in_type_ast_with_tables(
                elem, subst, structs, enums,
            )),
            len: *len,
            len_name: None,
        },
        TypeKind::Borrowed { region, inner } => TypeKind::Borrowed {
            region: region.clone(),
            inner: Box::new(substitute_param_in_type_ast_with_tables(
                inner, subst, structs, enums,
            )),
        },
        TypeKind::Generic { name, args } => TypeKind::Generic {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| substitute_param_in_type_ast_with_tables(a, subst, structs, enums))
                .collect(),
        },
        TypeKind::RawPtr(inner) => TypeKind::RawPtr(Box::new(
            substitute_param_in_type_ast_with_tables(inner, subst, structs, enums),
        )),
        TypeKind::FnPtr {
            params,
            return_type,
        } => TypeKind::FnPtr {
            params: params
                .iter()
                .map(|p| substitute_param_in_type_ast_with_tables(p, subst, structs, enums))
                .collect(),
            return_type: return_type.as_ref().map(|rt| {
                Box::new(substitute_param_in_type_ast_with_tables(
                    rt, subst, structs, enums,
                ))
            }),
        },
        TypeKind::Slice(inner) => TypeKind::Slice(Box::new(
            substitute_param_in_type_ast_with_tables(inner, subst, structs, enums),
        )),
        TypeKind::Tuple(elems) => TypeKind::Tuple(
            elems
                .iter()
                .map(|e| substitute_param_in_type_ast_with_tables(e, subst, structs, enums))
                .collect(),
        ),
    };
    Type {
        kind,
        span: ty.span,
    }
}

fn ty_to_source_name_with_tables(ty: &Ty, structs: &[StructDef], enums: &[EnumDef]) -> String {
    match ty {
        Ty::Struct(id) => structs[id.0 as usize].name.clone(),
        Ty::Enum(id) => enums[id.0 as usize].name.clone(),
        _ => ty_to_source_name(ty),
    }
}

/// Slice 7GEN.5c: render a `Ty` to a source-level name string suitable
/// for embedding in an AST `TypeKind::Path`. Conservative — only handles
/// primitive + struct/enum cases that field substitution needs.
fn ty_to_source_name(ty: &Ty) -> String {
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
        Ty::Unit => "()".into(),
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
        Ty::Simd { elem, lanes } => format!("{}x{}", ty_to_source_name(elem), lanes),
        Ty::Mask { elem, lanes } => {
            let width: u32 = match elem.as_ref() {
                Ty::I8 => 8,
                Ty::I16 => 16,
                Ty::I32 => 32,
                Ty::I64 => 64,
                _ => 0,
            };
            format!("mask{width}x{lanes}")
        }
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
        Ty::Slice(inner) => format!("slice_{}", mangle_ty_for_name(inner, structs, enums)),
        Ty::RawPtr(inner) => format!("ptr_{}", mangle_ty_for_name(inner, structs, enums)),
        Ty::FnPtr {
            params,
            return_type,
        } => {
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
        Ty::Simd { elem, lanes } => {
            format!("{}x{}", mangle_ty_for_name(elem, structs, enums), lanes)
        }
        Ty::Mask { elem, lanes } => {
            format!("mask{}x{}", mangle_ty_for_name(elem, structs, enums), lanes)
        }
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
    if iface.receiver != impl_.receiver {
        return false;
    }
    if iface.params.len() != impl_.params.len() {
        return false;
    }
    for (a, b) in iface.params.iter().zip(impl_.params.iter()) {
        if a.mutable != b.mutable {
            return false;
        }
        if a.move_ != b.move_ {
            return false;
        }
        if subst_self(&a.ty, target) != b.ty {
            return false;
        }
    }
    subst_self(&iface.return_type, target) == impl_.return_type
}

fn cast_allowed(from: &Ty, to: &Ty) -> bool {
    if from == to {
        return true;
    }
    // numeric → numeric (any pair)
    if from.is_numeric() && to.is_numeric() {
        return true;
    }
    // bool → integer (zext to width)
    if *from == Ty::Bool && to.is_int() {
        return true;
    }
    // enum → integer (read the variant index)
    if from.is_enum() && to.is_int() {
        return true;
    }
    // Phase 11 / P3 (FFI null + integer-to-pointer): integer → raw pointer.
    // The cast itself just reinterprets bits as an address (LLVM `inttoptr`).
    // The unsafe gate lives in `check_cast` — `cast_allowed` answers only
    // the type-pair shape question.
    if from.is_int() && matches!(to, Ty::RawPtr(_)) {
        return true;
    }
    // Phase 11: raw-pointer → raw-pointer reinterpretation (`*u8 as *T`).
    // The standard C / Rust idiom for treating an allocator-returned byte
    // buffer as a typed pointer. Codegen is a no-op at the LLVM level
    // (every raw pointer lowers to `ptr` already). The unsafe gate in
    // `check_cast` covers the soundness side — caller asserts the
    // reinterpretation is valid.
    if matches!(from, Ty::RawPtr(_)) && matches!(to, Ty::RawPtr(_)) {
        return true;
    }
    // v0.0.9 Phase 6 (cpc-gaps G-016): raw-pointer → 64-bit integer.
    // Only `usize`, `u64`, `isize`, and `i64` are accepted — narrower
    // targets would silently truncate a 64-bit address and are almost
    // always a bug. Codegen lowers to LLVM `ptrtoint`. The unsafe gate
    // in `check_cast` covers the safety side — pointer-as-integer
    // crosses the type system; the borrow checker has no way to reason
    // about whether the resulting integer round-trips back to a
    // pointer that gets dereferenced.
    if matches!(from, Ty::RawPtr(_)) && matches!(to, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) {
        return true;
    }
    // Forbidden:
    //   - integer/float → bool (use `!= 0`)
    //   - bool → float
    //   - integer → enum (needs runtime range check)
    //   - raw-pointer → narrow integer (`*T as u32`, `*T as i16`, ...)
    //     — cast to `usize` first, then narrow if you really mean to.
    //   - any other combination
    false
}

/// Phase 11 / ObjC interop: find `#[link_name = "..."]` on an item's
/// attribute list and return the string value. Returns `None` if absent.
/// Attribute-shape validation has already run via attrs::check, so any
/// `link_name` here is guaranteed to have the right arg shape.
/// v0.0.10 Phase 5: is the argument expression a named binding (a plain
/// `Ident` referring to a local / parameter) that the move-by-default
/// rule should consume? Rvalues (struct literals, calls returning by
/// value, etc.) own their result outright and don't have a caller
/// binding to mark moved — applying the implicit move to them is a
/// no-op and would otherwise spuriously fire E0337 on temporaries.
/// v0.0.12 (returned-borrow checking): the binding a borrow-shaped value
/// (`str` / `T[]`) is rooted at, tracing through place projections
/// (field / index / deref) and the canonical view accessors `as_str` /
/// `as_slice` (which borrow their receiver). Returns `None` for values
/// whose provenance can't be traced syntactically — literals (`'static`),
/// arbitrary call results, etc. — which the return check then treats
/// conservatively as "not provably dangling".
/// v0.0.13 (Tier 1): the root local of an *unambiguous view-producing*
/// expression — only `recv.as_str()` / `recv.as_slice()`. Unlike
/// `returned_borrow_root`, a bare identifier/field is NOT view-producing here
/// (it might be an owned value being moved), so this never flags a legitimate
/// move of an owned field into a returned aggregate.
fn view_producing_root(e: &Expr) -> Option<&str> {
    if let ExprKind::Call { callee, .. } = &e.kind {
        if let ExprKind::Field { receiver, name } = &callee.kind {
            if matches!(name.name.as_str(), "as_str" | "as_slice") {
                return returned_borrow_root(receiver);
            }
        }
    }
    None
}

fn returned_borrow_root(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.as_str()),
        ExprKind::Field { receiver, .. } => returned_borrow_root(receiver),
        ExprKind::Index { receiver, .. } => returned_borrow_root(receiver),
        ExprKind::Unary {
            op: UnaryOp::Deref,
            operand,
        } => returned_borrow_root(operand),
        // `recv.as_str()` / `recv.as_slice()` return a view borrowing `recv`.
        ExprKind::Call { callee, .. } => {
            if let ExprKind::Field { receiver, name } = &callee.kind {
                if matches!(name.name.as_str(), "as_str" | "as_slice") {
                    return returned_borrow_root(receiver);
                }
            }
            None
        }
        _ => None,
    }
}

fn extract_link_name(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| {
        if a.path.name != "link_name" {
            return None;
        }
        match a.args.as_slice() {
            [AttrArg::Str(s, _)] => Some(s.clone()),
            _ => None,
        }
    })
}

// ============================================================================
// v0.0.10 Phase 1 + Phase 3: shared `#[no_alloc]` / `#[bounded_recursion]`
// support — fn-table + AST callee-name walker.
// ============================================================================

/// Hardcoded blocklist of libc allocator symbol names. A `#[no_alloc]`
/// function must not call any of these directly or transitively.
/// Matching is done against the callee's `#[link_name]` symbol if set,
/// else its trailing-segment source name.
const ALLOC_BLOCKLIST: &[&str] = &[
    "malloc",
    "calloc",
    "realloc",
    "reallocf",
    "aligned_alloc",
    "valloc",
    "memalign",
    "posix_memalign",
    "free",
];

/// Whitelist of libc / runtime externs that are known not to allocate.
/// A `#[no_alloc]` fn may call any of these. Adding to this list is the
/// way to admit a new "known-leaf" symbol; alternatively the user can
/// annotate the extern declaration with `#[no_alloc]`.
const LEAF_WHITELIST: &[&str] = &[
    // memory helpers
    "memcpy",
    "memmove",
    "memset",
    "memcmp",
    "memchr",
    "bzero",
    "bcopy",
    // string scanning (no allocation)
    "strlen",
    "strnlen",
    "strcmp",
    "strncmp",
    "strchr",
    "strrchr",
    "strstr",
    "strspn",
    "strcspn",
    "strpbrk",
    // bounded copies (caller-supplied buffers)
    "strcpy",
    "strncpy",
    "strcat",
    "strncat",
    "snprintf",
    "vsnprintf",
    // I/O (syscalls — no heap)
    "write",
    "read",
    "fwrite",
    "fread",
    "fputs",
    "fputc",
    "puts",
    "putchar",
    "putc",
    "printf",
    "fprintf",
    "vprintf",
    "vfprintf",
    "fflush",
    "fclose",
    // process control (terminate — no heap)
    "exit",
    "_exit",
    "abort",
    "_Exit",
    // libc double math
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "sinh",
    "cosh",
    "tanh",
    "asinh",
    "acosh",
    "atanh",
    "exp",
    "exp2",
    "expm1",
    "log",
    "log2",
    "log10",
    "log1p",
    "pow",
    "sqrt",
    "cbrt",
    "hypot",
    "ceil",
    "floor",
    "round",
    "trunc",
    "fmod",
    "fabs",
    "ldexp",
    "frexp",
    "modf",
    "remainder",
    "copysign",
    // libc float math
    "sinf",
    "cosf",
    "tanf",
    "asinf",
    "acosf",
    "atanf",
    "atan2f",
    "expf",
    "exp2f",
    "logf",
    "log2f",
    "log10f",
    "powf",
    "sqrtf",
    "cbrtf",
    "hypotf",
    "fabsf",
    "floorf",
    "ceilf",
    "roundf",
    "truncf",
    "fmodf",
    // ObjC runtime helpers (v0.0.10 Phase 4 / vendor/metal)
    "objc_msgSend",
    "objc_release",
    "objc_retain",
    "objc_autorelease",
    "sel_registerName",
    "sel_getName",
    "objc_getClass",
    "objc_lookUpClass",
    "object_getClass",
    "class_getName",
    "class_respondsToSelector",
    // misc — pure or no-alloc
    "errno",
    "strerror_r",
];

/// Blocklist of blocking primitives. A `#[no_block]` function must not call
/// any of these directly or transitively. Matching is done against the
/// callee's `#[link_name]` symbol if set, else its trailing-segment name.
/// Covers the hazards enumerated in `realtime.md` Phase 3: lock acquisition,
/// condvar/barrier waits, thread join, sleep/timer waits, blocking file and
/// socket I/O, and blocking I/O multiplexing.
const BLOCK_BLOCKLIST: &[&str] = &[
    // pthread lock acquisition + waits
    "pthread_mutex_lock",
    "pthread_mutex_timedlock",
    "pthread_rwlock_rdlock",
    "pthread_rwlock_wrlock",
    "pthread_rwlock_timedrdlock",
    "pthread_rwlock_timedwrlock",
    "pthread_cond_wait",
    "pthread_cond_timedwait",
    "pthread_barrier_wait",
    "pthread_join",
    "pthread_spin_lock",
    // sleep / timer waits
    "sleep",
    "usleep",
    "nanosleep",
    "clock_nanosleep",
    // process waits
    "wait",
    "waitpid",
    "wait3",
    "wait4",
    "waitid",
    // blocking I/O multiplexing
    "poll",
    "ppoll",
    "select",
    "pselect",
    "epoll_wait",
    "kevent",
    // blocking file I/O (raw syscalls + buffered reads + flush/sync)
    "read",
    "pread",
    "readv",
    "write",
    "pwrite",
    "writev",
    "fread",
    "fwrite",
    "fgets",
    "fgetc",
    "getc",
    "getchar",
    "getline",
    "fputs",
    "fputc",
    "putc",
    "putchar",
    "puts",
    "fflush",
    "scanf",
    "fscanf",
    "vscanf",
    "vfscanf",
    "fsync",
    "fdatasync",
    "msync",
    // blocking socket I/O
    "recv",
    "recvfrom",
    "recvmsg",
    "send",
    "sendto",
    "sendmsg",
    "accept",
    "connect",
    // buffered stdio writers that can block on a slow sink
    "printf",
    "fprintf",
    "vprintf",
    "vfprintf",
];

/// Whitelist of externs known not to block. A `#[no_block]` fn may call any
/// of these. Pure computation (math), caller-buffer memory/string ops, and
/// process termination — none of which wait on another thread, a lock, a
/// timer, or I/O. Deliberately excludes the I/O helpers on `LEAF_WHITELIST`
/// (those are no-alloc but *can* block on a pipe/socket).
const BLOCK_SAFE_LEAF: &[&str] = &[
    // memory helpers (caller-supplied buffers — no I/O, no waiting)
    "memcpy",
    "memmove",
    "memset",
    "memcmp",
    "memchr",
    "bzero",
    "bcopy",
    // string scanning (pure)
    "strlen",
    "strnlen",
    "strcmp",
    "strncmp",
    "strchr",
    "strrchr",
    "strstr",
    "strspn",
    "strcspn",
    "strpbrk",
    // bounded copies / formatting into caller buffers (no I/O)
    "strcpy",
    "strncpy",
    "strcat",
    "strncat",
    "snprintf",
    "vsnprintf",
    // process control (terminate — does not return, never blocks a hot path)
    "exit",
    "_exit",
    "abort",
    "_Exit",
    // libc double math
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "sinh",
    "cosh",
    "tanh",
    "asinh",
    "acosh",
    "atanh",
    "exp",
    "exp2",
    "expm1",
    "log",
    "log2",
    "log10",
    "log1p",
    "pow",
    "sqrt",
    "cbrt",
    "hypot",
    "ceil",
    "floor",
    "round",
    "trunc",
    "fmod",
    "fabs",
    "ldexp",
    "frexp",
    "modf",
    "remainder",
    "copysign",
    // libc float math
    "sinf",
    "cosf",
    "tanf",
    "asinf",
    "acosf",
    "atanf",
    "atan2f",
    "expf",
    "exp2f",
    "logf",
    "log2f",
    "log10f",
    "powf",
    "sqrtf",
    "cbrtf",
    "hypotf",
    "fabsf",
    "floorf",
    "ceilf",
    "roundf",
    "truncf",
    "fmodf",
    // nonblocking synchronization primitives + hints
    "pthread_mutex_trylock",
    "pthread_rwlock_tryrdlock",
    "pthread_rwlock_trywrlock",
    "pthread_spin_trylock",
    "sched_yield",
    // misc — pure
    "errno",
    "strerror_r",
];

/// Info captured about every fn/method in the program, used by the
/// `check_no_alloc`, `check_no_block`, and `check_bounded_recursion` passes.
struct NoAllocFnInfo<'a> {
    is_extern: bool,
    has_no_alloc: bool,
    has_no_block: bool,
    link_name: Option<String>,
    body: Option<&'a Block>,
}

/// A function satisfies the `#[no_alloc]` contract at a call site if it is
/// marked `#[no_alloc]` directly or `#[realtime]` (which bundles it).
fn marks_no_alloc(attrs: &[Attribute]) -> bool {
    has_attr_named(attrs, "no_alloc") || has_attr_named(attrs, "realtime")
}

/// A function satisfies the `#[no_block]` contract at a call site if it is
/// marked `#[no_block]` directly or `#[realtime]` (which bundles it).
fn marks_no_block(attrs: &[Attribute]) -> bool {
    has_attr_named(attrs, "no_block") || has_attr_named(attrs, "realtime")
}

/// A function is subject to the `#[bounded_recursion]` pass if it is marked
/// `#[bounded_recursion]` directly or `#[realtime]` (which bundles it).
fn marks_bounded_recursion(attrs: &[Attribute]) -> bool {
    has_attr_named(attrs, "bounded_recursion") || has_attr_named(attrs, "realtime")
}

struct NoAllocFnTable<'a> {
    fns: std::collections::HashMap<String, NoAllocFnInfo<'a>>,
}

fn build_no_alloc_fn_table(p: &Program) -> NoAllocFnTable<'_> {
    let mut fns = std::collections::HashMap::new();
    for item in &p.items {
        match &item.kind {
            ItemKind::Function(f) => {
                let info = NoAllocFnInfo {
                    is_extern: f.is_extern,
                    has_no_alloc: marks_no_alloc(&f.attributes),
                    has_no_block: marks_no_block(&f.attributes),
                    link_name: extract_link_name(&f.attributes),
                    body: if f.is_extern { None } else { Some(&f.body) },
                };
                fns.insert(f.name.name.clone(), info);
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    let info = NoAllocFnInfo {
                        is_extern: false,
                        has_no_alloc: marks_no_alloc(&m.attributes),
                        has_no_block: marks_no_block(&m.attributes),
                        link_name: None,
                        body: Some(&m.body),
                    };
                    fns.insert(m.name.name.clone(), info);
                }
            }
            _ => {}
        }
    }
    NoAllocFnTable { fns }
}

fn has_attr_named(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path.name == name)
}

/// Extract the byte budget + attribute span from a `#[max_stack(N)]` attribute,
/// if present. Returns `None` when absent or malformed (attrs validation has
/// already rejected the malformed shape with E0355).
fn max_stack_budget(attrs: &[Attribute]) -> Option<(u64, ByteSpan)> {
    for a in attrs {
        if a.path.name == "max_stack" {
            if let [AttrArg::Int(v, _)] = a.args.as_slice() {
                return Some((*v as u64, a.span));
            }
        }
    }
    None
}

/// Trailing-segment of a possibly-qualified fn name (e.g. `vec.malloc` →
/// `malloc`). Used to produce user-friendly diagnostic text.
fn leaf_name(qualified: &str) -> &str {
    qualified
        .rsplit_once('.')
        .map(|(_, n)| n)
        .unwrap_or(qualified)
}

/// Hot-path-relevant effects gathered from a function body by the shared
/// real-time walker. Extensible: future allocating constructs (e.g. owned
/// container growth) add a field here rather than a parallel traversal.
#[derive(Default)]
struct BodyEffects {
    /// `(callee_name, call_span)` for every resolvable `ExprKind::Call`.
    calls: Vec<(String, ByteSpan)>,
    /// Spans of allocating language constructs that are not function calls.
    /// Today: string interpolation (lowers to a `__string_concat` malloc).
    interps: Vec<ByteSpan>,
    /// Declared types of `let` bindings with a type annotation. Used by the
    /// `#[max_stack]` frame estimate; ignored by the no_alloc/no_block passes.
    let_tys: Vec<Type>,
}

/// Walk a Block, collecting hot-path effects into `out`. Calls are recorded
/// for every `ExprKind::Call` whose callee resolves to a textual name:
///   - `Ident(name)` → `name`
///   - `Path { segments }` → `seg1.seg2....segN` (matches the resolver's
///     qualified-name form)
///   - `Field` method calls → skipped (cannot resolve without dispatch info)
/// Allocating non-call constructs (string interpolation) are recorded too.
fn collect_effects_block(block: &Block, out: &mut BodyEffects) {
    for s in &block.stmts {
        collect_effects_stmt(s, out);
    }
    if let Some(tail) = &block.tail {
        collect_effects_expr(tail, out);
    }
}

fn collect_effects_stmt(stmt: &Stmt, out: &mut BodyEffects) {
    match &stmt.kind {
        StmtKind::Let { init, ty, .. } => {
            // Capture the annotated type for the `#[max_stack]` frame estimate.
            if let Some(t) = ty {
                out.let_tys.push(t.clone());
            }
            if let Some(e) = init {
                collect_effects_expr(e, out);
            }
        }
        StmtKind::Return(Some(e)) => collect_effects_expr(e, out),
        StmtKind::Return(None) => {}
        StmtKind::While { cond, body, .. } => {
            collect_effects_expr(cond, out);
            collect_effects_block(body, out);
        }
        StmtKind::For(fl, _) => match fl {
            ForLoop::Range { iter, body, .. } => {
                collect_effects_expr(iter, out);
                collect_effects_block(body, out);
            }
            ForLoop::CStyle {
                init,
                cond,
                update,
                body,
            } => {
                if let Some(s) = init {
                    collect_effects_stmt(s, out);
                }
                if let Some(c) = cond {
                    collect_effects_expr(c, out);
                }
                for u in update {
                    collect_effects_expr(u, out);
                }
                collect_effects_block(body, out);
            }
        },
        StmtKind::Expr(e) | StmtKind::Defer(e) | StmtKind::Assert(e) => {
            collect_effects_expr(e, out);
        }
        StmtKind::Loop(b, _) => collect_effects_block(b, out),
        // After the lowering pass these are gone. Walk defensively in case
        // sema runs without lower (or for AST-level test harnesses).
        StmtKind::IfLet {
            scrutinee,
            body,
            else_body,
            ..
        } => {
            collect_effects_expr(scrutinee, out);
            collect_effects_block(body, out);
            if let Some(eb) = else_body {
                collect_effects_block(eb, out);
            }
        }
        StmtKind::WhileLet {
            scrutinee, body, ..
        } => {
            collect_effects_expr(scrutinee, out);
            collect_effects_block(body, out);
        }
        StmtKind::GuardLet {
            scrutinee,
            else_body,
            ..
        } => {
            collect_effects_expr(scrutinee, out);
            collect_effects_block(else_body, out);
        }
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_effects_expr(e: &Expr, out: &mut BodyEffects) {
    match &e.kind {
        ExprKind::Call { callee, args, .. } => {
            if let Some(name) = extract_call_name(callee) {
                out.calls.push((name, e.span));
            }
            // Walk arguments — nested calls inside args still count.
            collect_effects_expr(callee, out);
            for a in args {
                collect_effects_expr(a, out);
            }
        }
        ExprKind::GenericEnumCall { args, .. } => {
            for a in args {
                collect_effects_expr(a, out);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_effects_expr(lhs, out);
            collect_effects_expr(rhs, out);
        }
        ExprKind::Unary { operand, .. } => collect_effects_expr(operand, out),
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_effects_expr(s, out);
            }
            if let Some(e) = end {
                collect_effects_expr(e, out);
            }
        }
        ExprKind::Assign { target, value, .. } => {
            collect_effects_expr(target, out);
            collect_effects_expr(value, out);
        }
        ExprKind::Cast { expr, .. } => collect_effects_expr(expr, out),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            collect_effects_expr(cond, out);
            collect_effects_block(then, out);
            if let Some(eb) = else_branch {
                collect_effects_expr(eb, out);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_effects_expr(scrutinee, out);
            for arm in arms {
                collect_effects_expr(&arm.body, out);
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => collect_effects_block(b, out),
        ExprKind::Field { receiver, .. } => collect_effects_expr(receiver, out),
        ExprKind::Index { receiver, index } => {
            collect_effects_expr(receiver, out);
            collect_effects_expr(index, out);
        }
        ExprKind::StructLit { fields, .. } => {
            for f in fields {
                collect_effects_expr(&f.value, out);
            }
        }
        ExprKind::GenericStructLit { fields, .. } => {
            for f in fields {
                collect_effects_expr(&f.value, out);
            }
        }
        ExprKind::ArrayLit { elements } | ExprKind::TupleLit { elements } => {
            for el in elements {
                collect_effects_expr(el, out);
            }
        }
        ExprKind::ArrayFill { fill, .. } => collect_effects_expr(fill, out),
        ExprKind::InterpStr { parts } => {
            // String interpolation lowers to `__string_concat` — a malloc per
            // evaluation (see codegen `gen_interp_str`). Any interpolation with
            // an embedded expression allocates; record the site so the
            // `#[no_alloc]` pass can reject it. (Pure-literal strings never
            // reach this node — the lexer emits a plain `Str` token.)
            if parts.iter().any(|p| matches!(p, InterpStrPart::Expr(_))) {
                out.interps.push(e.span);
            }
            for p in parts {
                if let InterpStrPart::Expr(e) = p {
                    collect_effects_expr(e, out);
                }
            }
        }
        ExprKind::Await(inner) | ExprKind::Yield(inner) => {
            collect_effects_expr(inner, out);
        }
        ExprKind::Intrinsic { args, .. } => {
            // Intrinsics are dispatched by the compiler directly — there is
            // no user-fn callee — but nested calls inside args still count.
            for a in args {
                collect_effects_expr(a, out);
            }
        }
        ExprKind::Asm { operands, .. } => {
            // `#asm` itself is not a call; operand value-exprs still might
            // contain one.
            for op in operands {
                collect_effects_expr(&op.value, out);
            }
        }
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
    }
}

/// Resolve a Call expression's callee to a textual name. Returns None for
/// shapes the walker cannot statically resolve (method dispatch through
/// `Field`, calls through computed receivers, etc.).
fn extract_call_name(callee: &Expr) -> Option<String> {
    match &callee.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Path { segments } => {
            // `foo::bar::baz` → "foo.bar.baz" (matches resolver's qualified
            // form). Strip turbofish-only edge cases — if any segment is
            // empty, bail out.
            if segments.iter().any(|s| s.name.is_empty()) {
                return None;
            }
            Some(
                segments
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
            )
        }
        ExprKind::Field { .. } => None,
        _ => None,
    }
}
// ============================================================================
// End shared #[no_alloc] / #[bounded_recursion] support
// ============================================================================

/// v0.0.16: the FFI/raw + byte-swap builtins that are now spelled `#name(...)`.
/// Used to give a bare call a migration fix-it instead of a generic
/// "unknown function".
fn is_ffi_builtin_name(name: &str) -> bool {
    matches!(
        name,
        "println"
            | "str_ptr"
            | "str_len"
            | "str_from_raw_parts"
            | "slice_ptr"
            | "slice_len"
            | "slice_from_raw_parts"
            | "bswap16"
            | "bswap32"
            | "bswap64"
            | "htons"
            | "htonl"
            | "ntohs"
            | "ntohl"
    )
}

fn body_ends_with_return(b: &Block) -> bool {
    b.stmts
        .last()
        .is_some_and(|s| matches!(s.kind, StmtKind::Return(_)))
}

/// v0.0.15 flow-sensitive moves: does this block *diverge* — i.e. never fall
/// through to the code after it? True when its last statement is a
/// `return`/`break`/`continue` (or its tail is a diverging `if`/`match`). Used
/// so moves performed only on a diverging branch don't poison the binding for
/// the code that runs when the branch is *not* taken.
fn block_diverges(b: &Block) -> bool {
    if let Some(t) = &b.tail {
        return expr_diverges(t);
    }
    b.stmts.last().is_some_and(|s| {
        matches!(
            s.kind,
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
        )
    })
}

/// Companion to `block_diverges` for an expression in branch position. A bare
/// `if`/`else` diverges only if *both* arms do; a block defers to
/// `block_diverges`. Anything else is treated as falling through (conservative
/// for moves: its moves are kept).
fn expr_diverges(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Block(b) | ExprKind::Unsafe(b) => block_diverges(b),
        ExprKind::If {
            then, else_branch, ..
        } => block_diverges(then) && else_branch.as_deref().is_some_and(expr_diverges),
        _ => false,
    }
}

/// v0.0.12 G-025: a "place" for `#addr_of` is anything codegen's
/// `gen_place` can produce a pointer for — bare bindings, field
/// accesses, indexed loads, and dereferences (and chains thereof).
/// Call results / arithmetic / etc. are rvalues with no stable address.
fn is_addr_of_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_) => true,
        ExprKind::Unary {
            op: UnaryOp::Deref, ..
        } => true,
        ExprKind::Field { receiver, .. } => is_addr_of_place(receiver),
        ExprKind::Index { receiver, .. } => is_addr_of_place(receiver),
        _ => false,
    }
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

    // ---- `f16` literal suffix ----

    #[test]
    fn f16_literal_types_as_f16() {
        // `1.5f16` is an f16, and an unsuffixed literal coerces to an f16
        // annotation.
        assert!(errors("fn f() -> i32 { let h: f16 = 1.5f16; return 0; }").is_empty());
        assert!(errors("fn f() -> i32 { let h: f16 = 1.5; return 0; }").is_empty());
    }

    // ---- `c"..."` C-string literals (type `*u8`) ----

    #[test]
    fn cstring_literal_types_as_raw_ptr() {
        // c"..." is `*u8` and safe to *form* (a pointer to static data); no
        // `unsafe` is needed to bind it.
        assert!(errors("fn f() -> i32 { let p: *u8 = c\"hi\"; return 0; }").is_empty());
        // Binding it to a non-pointer is a type mismatch.
        assert!(errors("fn f() -> i32 { let n: i32 = c\"hi\"; return 0; }").contains(&"E0302"));
    }

    // ---- v0.0.8 Phase 4: `env!("NAME")` compile-time env-var read ----

    #[test]
    fn env_macro_resolves_set_var_to_str() {
        // Positive case: var is set in the compiler's environment, so
        // `env!("NAME")` resolves to a `str` value at sema time.
        std::env::set_var("CPC_TEST_ENV_VAR", "from-test");
        assert_clean(
            "fn main() -> i32 {\n\
                 let v: str = #env(\"CPC_TEST_ENV_VAR\");\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn env_macro_missing_var_e0876() {
        // Negative case: env var not set → E0876 at sema time.
        std::env::remove_var("CPC_TEST_DEFINITELY_MISSING_99");
        assert_only_code(
            "fn main() -> i32 {\n\
                 let _v: str = #env(\"CPC_TEST_DEFINITELY_MISSING_99\");\n\
                 return 0;\n\
             }",
            "E0876",
        );
    }

    // ---- v0.0.8 post-bench-gap: `restrict` param marker (E0411) ----

    #[test]
    fn restrict_on_raw_pointer_param_clean() {
        // `restrict p: *T` is well-formed.
        assert_clean(
            "fn axpy(n: usize, restrict x: *f32, restrict y: *f32) { return; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn restrict_on_integer_param_e0411() {
        // E0411: `restrict x: i32` — restrict requires `*T`.
        assert_only_code(
            "fn bad(restrict x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return bad(0); }",
            "E0411",
        );
    }

    #[test]
    fn restrict_on_struct_param_e0411() {
        // E0411: `restrict s: Point` — restrict requires `*T`.
        assert_only_code(
            "struct Point { x: i32, y: i32 }\n\
             fn bad(restrict s: Point) -> i32 { return s.x; }\n\
             fn main() -> i32 { let p: Point = Point { x: 1, y: 2 }; return bad(p); }",
            "E0411",
        );
    }

    // ---- #addr_of(x) intrinsic ----

    // ---- v0.0.9 follow-up: `borrow x: T` parameter marker ----

    #[test]
    fn borrow_param_marker_clean() {
        // `borrow x: T` is an explicit form of the current shared
        // by-value default. Semantically a no-op for v0.0.9; reserved
        // so a future Phase 1 default-move flip has a clean opt-out.
        assert_clean(
            "fn add(borrow a: i32, borrow b: i32) -> i32 { return a + b; }\n\
             fn main() -> i32 { return add(2, 3); }",
        );
    }

    #[test]
    fn borrow_plus_move_e0334() {
        assert_only_code(
            "fn bad(borrow move x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return bad(0); }",
            "E0334",
        );
    }

    #[test]
    fn borrow_plus_mut_e0334() {
        assert_only_code(
            "fn bad(borrow mut x: i32) -> i32 { return x; }\n\
             fn main() -> i32 { return bad(0); }",
            "E0334",
        );
    }

    #[test]
    fn borrow_does_not_collide_with_borrow_region_type() {
        // `borrow x: T` (param marker) and `borrow REGION T` (type
        // position) coexist. The region-annotated type only appears
        // after the colon — parse_type's `borrow REGION T` branch
        // handles it. The param-marker loop only fires before the
        // colon. This test pins both: a marker + a region-annotated
        // type in the same parameter is valid syntax.
        // (The semantic check for `move x: borrow A T` lives in the
        // parser as a hard error — exercised in parser tests.)
        assert_clean(
            "fn f(borrow x: borrow A i32) -> i32 { return x; }\n\
             fn main() -> i32 { return f(0); }",
        );
    }

    #[test]
    fn addr_of_local_in_unsafe_clean() {
        // The happy path: `unsafe { #addr_of(x) }` returns `*T` for a
        // local binding. No-unsafe / non-Ident / wrong-arity cases are
        // checked separately below.
        assert_clean(
            "extern fn use_ptr(p: *i64);\n\
             fn main() -> i32 {\n\
                 let t: i64 = 0;\n\
                 unsafe { use_ptr(#addr_of(t)); }\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn addr_of_outside_unsafe_e0801() {
        assert_only_code(
            "fn main() -> i32 {\n\
                 let t: i64 = 0;\n\
                 let p: *i64 = #addr_of(t);\n\
                 return 0;\n\
             }",
            "E0801",
        );
    }

    // v0.0.12 G-025: `#addr_of` accepts any place expression — fields,
    // indices, derefs, and chains. Call results / arithmetic / etc.
    // remain rejected because they have no stable address.

    #[test]
    fn addr_of_field_access_clean_g025() {
        assert_clean(
            "struct P { x: i32, y: i32 }\n\
             fn main() -> i32 {\n\
                 let p: P = P { x: 5, y: 6 };\n\
                 let q: *i32 = unsafe { #addr_of(p.x) };\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn addr_of_deref_field_clean_g025() {
        assert_clean(
            "struct P { x: i32 }\n\
             fn main() -> i32 {\n\
                 let p: P = P { x: 5 };\n\
                 let pp: *P = unsafe { #addr_of(p) };\n\
                 let q: *i32 = unsafe { #addr_of((*pp).x) };\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn addr_of_array_index_clean_g025() {
        assert_clean(
            "fn main() -> i32 {\n\
                 let a: [i32; 4] = [10, 20, 30, 40];\n\
                 let q: *i32 = unsafe { #addr_of(a[2]) };\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn addr_of_deref_clean_g025() {
        assert_clean(
            "fn main() -> i32 {\n\
                 let n: i64 = 5;\n\
                 let p: *i64 = unsafe { #addr_of(n) };\n\
                 let q: *i64 = unsafe { #addr_of(*p) };\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn addr_of_call_result_rejected_e0302_g025() {
        // Call result is an rvalue — no stable address.
        assert_only_code(
            "fn foo() -> i32 { return 42; }\n\
             fn main() -> i32 {\n\
                 let q: *i32 = unsafe { #addr_of(foo()) };\n\
                 return 0;\n\
             }",
            "E0302",
        );
    }

    #[test]
    fn addr_of_arithmetic_rejected_e0302_g025() {
        assert_only_code(
            "fn main() -> i32 {\n\
                 let x: i32 = 1;\n\
                 let q: *i32 = unsafe { #addr_of(x +% 1) };\n\
                 return 0;\n\
             }",
            "E0302",
        );
    }

    // v0.0.12 G-024: `is_null()` / `is_not_null()` on raw pointers.
    // Safe to call (just compares the bit pattern); returns `bool`.

    #[test]
    fn is_null_on_raw_ptr_clean_g024() {
        assert_clean(
            "fn main() -> i32 {\n\
                 let p: *u8 = unsafe { 0 as *u8 };\n\
                 if p.is_null() { return 1; }\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn is_not_null_on_raw_ptr_clean_g024() {
        assert_clean(
            "fn main() -> i32 {\n\
                 let p: *u8 = unsafe { 0 as *u8 };\n\
                 if p.is_not_null() { return 1; }\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn is_null_does_not_require_unsafe_g024() {
        // is_null inspects the bit pattern; no memory access, no unsafe.
        assert_clean(
            "fn need_ptr() -> *u8 { return unsafe { 0 as *u8 }; }\n\
             fn main() -> i32 {\n\
                 let p: *u8 = need_ptr();\n\
                 if p.is_null() { return 0; }\n\
                 return 1;\n\
             }",
        );
    }

    // v0.0.12 G-026: `()` resolves to `Ty::Unit` and round-trips
    // through generic instantiation (e.g. `spawn::[()]`).

    // v0.0.12 G-028 (llama.cplus G-026): `#zero::[T]()` and `*T.write_zeroed()`
    // for explicit zero-fill — closes the C99 partial-init / silent-garbage gap.

    #[test]
    fn zero_intrinsic_returns_type_clean_g028() {
        assert_clean(
            "struct P { x: i32, y: i32 }\n\
             fn main() -> i32 { let _p: P = #zero::[P](); return 0; }",
        );
    }

    #[test]
    fn zero_intrinsic_primitive_clean_g028() {
        assert_clean("fn main() -> i32 { let _x: i64 = #zero::[i64](); return 0; }");
    }

    #[test]
    fn zero_intrinsic_no_type_arg_rejected_e0501_g028() {
        let codes = errors("fn main() -> i32 { let _x: i32 = #zero(); return 0; }");
        assert!(codes.contains(&"E0501"));
    }

    #[test]
    fn write_zeroed_on_raw_ptr_clean_g028() {
        assert_clean(
            "extern fn malloc(n: usize) -> *u8;\n\
             struct P { x: i32 }\n\
             fn main() -> i32 {\n\
                 let p: *P = unsafe { malloc(#size_of::[P]()) as *P };\n\
                 unsafe { p.write_zeroed(); }\n\
                 return 0;\n\
             }",
        );
    }

    // v0.0.12 G-030 (llama.cplus G-029): `__cplus_atomic_fence_<ord>`
    // memory fence intrinsic. Requires unsafe; 0 args.

    #[test]
    fn atomic_fence_seqcst_in_unsafe_clean_g030() {
        assert_clean(
            "fn main() -> i32 {\n\
                 unsafe { __cplus_atomic_fence_seqcst(); }\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn atomic_fence_relaxed_clean_g030() {
        // Relaxed is accepted by sema (it's a no-op at codegen).
        assert_clean(
            "fn main() -> i32 {\n\
                 unsafe { __cplus_atomic_fence_relaxed(); }\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn atomic_fence_outside_unsafe_e0801_g030() {
        let codes = errors("fn main() -> i32 { __cplus_atomic_fence_seqcst(); return 0; }");
        assert!(codes.contains(&"E0801"));
    }

    #[test]
    fn atomic_fence_with_args_e0308_g030() {
        let codes = errors(
            "fn main() -> i32 {\n\
                 unsafe { __cplus_atomic_fence_seqcst(1 as i32); }\n\
                 return 0;\n\
             }",
        );
        assert!(codes.contains(&"E0308"));
    }

    // v0.0.12 G-031 (llama.cplus G-030): `#cpu_relax()` spin-loop hint.
    // Safe; 0 args; 0 type args; returns unit.

    #[test]
    fn cpu_relax_clean_g031() {
        assert_clean(
            "fn main() -> i32 {\n\
                 let mut i: i32 = 0;\n\
                 while i < 3 { #cpu_relax(); i = i +% 1; }\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn cpu_relax_with_args_e0308_g031() {
        let codes = errors("fn main() -> i32 { #cpu_relax(1 as i32); return 0; }");
        assert!(codes.contains(&"E0308"));
    }

    // v0.0.12 G-033 (llama.cplus G-032): `#zero::[T]()` accepted in
    // const/static initializer position. Closes the BSS-zero global case.

    #[test]
    fn zero_in_static_init_clean_g033() {
        assert_clean(
            "#[repr(C)] struct S { a: i32, b: i64 }\n\
             pub static T: S = #zero::[S]();\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn zero_in_static_mut_init_clean_g033() {
        assert_clean(
            "pub static mut TABLE: [i32; 16] = #zero::[[i32; 16]]();\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn zero_in_const_init_clean_g033() {
        // const FOO also accepts #zero (papercut they hit in
        // threadpool.cplus). Same shape as static.
        assert_clean(
            "pub const ZEROS: [u8; 64] = #zero::[[u8; 64]]();\n\
             fn main() -> i32 { return 0; }",
        );
    }

    // Note: E0X30 lives in the lower pass, not sema, so the negative
    // tests for it live near the other lower-pass tests further down
    // (using `lowered_errors`). Cross-ref: array_literal_still_rejected
    // / fill_array_in_static_still_rejected, both labeled `_g033`.

    #[test]
    fn cpu_relax_with_type_args_e0501_g031() {
        let codes = errors("fn main() -> i32 { #cpu_relax::[i32](); return 0; }");
        assert!(codes.contains(&"E0501"));
    }

    #[test]
    fn write_zeroed_outside_unsafe_e0801_g028() {
        let codes = errors(
            "extern fn malloc(n: usize) -> *u8;\n\
             struct P { x: i32 }\n\
             fn main() -> i32 {\n\
                 let p: *P = unsafe { malloc(#size_of::[P]()) as *P };\n\
                 p.write_zeroed();\n\
                 return 0;\n\
             }",
        );
        assert!(codes.contains(&"E0801"));
    }

    #[test]
    fn unit_type_as_turbofish_arg_clean_g026() {
        assert_clean(
            "fn id[T]() -> () { return; }\n\
             fn main() -> i32 { id::[()](); return 0; }",
        );
    }

    #[test]
    fn unit_type_as_explicit_return_clean_g026() {
        assert_clean("fn f() -> () { return; }\nfn main() -> i32 { f(); return 0; }");
    }

    #[test]
    fn is_null_on_non_pointer_rejected_g024() {
        // Receiver isn't a raw pointer → falls through to normal method
        // lookup → no `is_null` on i32 → E0324.
        let codes =
            errors("fn main() -> i32 { let x: i32 = 5; if x.is_null() { return 1; } return 0; }");
        assert!(codes.iter().any(|c| c.starts_with("E0")));
    }

    #[test]
    fn addr_of_wrong_arity_e0308() {
        assert_only_code(
            "fn main() -> i32 {\n\
                 let x: i64 = 0;\n\
                 let y: i64 = 0;\n\
                 let p: *i64 = unsafe { #addr_of(x, y) };\n\
                 return 0;\n\
             }",
            "E0308",
        );
    }

    #[test]
    fn addr_of_rejects_turbofish_e0501() {
        // `addr_of` infers the type from the binding — explicit type
        // args are meaningless and rejected.
        assert_only_code(
            "fn main() -> i32 {\n\
                 let t: i64 = 0;\n\
                 let p: *i64 = unsafe { addr_of::[i64](t) };\n\
                 return 0;\n\
             }",
            "E0501",
        );
    }

    #[test]
    fn g022_generic_field_does_not_starve_methods() {
        // G-022 (fixed 2026-05-23): when a struct field type names a
        // cross-package generic instantiation, the early instantiation
        // performed by `collect_struct_fields` ran *before*
        // `collect_methods` had populated `generic_impl_methods`. The
        // synthesized `StructDef` was cached with an empty methods table.
        // Later consumer calls hit dedup and got E0324 "no method".
        //
        // Repro in one file: `Pair[T]` with an `impl Pair[T] { fn put(...) }`;
        // a `Holder` struct whose field is `Pair[i32]`; and a function that
        // calls `.put` on a local `Pair[i32]`. Pre-fix this would error.
        // Post-fix the backfill pass populates methods on `Pair[i32]` even
        // though the field-resolution instantiated it first.
        assert_clean(
            "struct Pair[T] { a: T, b: T }\n\
             impl Pair[T] {\n\
                 fn put(mut self, v: T) { self.a = v; return; }\n\
             }\n\
             struct Holder { p: Pair[i32] }\n\
             fn touch() -> i32 {\n\
                 let mut x: Pair[i32] = Pair[i32] { a: 0 as i32, b: 0 as i32 };\n\
                 x.put(7 as i32);\n\
                 return x.a;\n\
             }\n\
             fn main() -> i32 { return touch(); }",
        );
    }

    #[test]
    fn restrict_on_method_param_clean() {
        // Method params accept `restrict *T` the same as free fns.
        assert_clean(
            "struct State { v: i32 }\n\
             impl State { fn fill(self, restrict p: *u8, n: usize) { return; } }\n\
             fn main() -> i32 { return 0; }",
        );
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
        assert_only_code(
            "fn f() -> i32 { 1; }\nfn main() -> i32 { return f(); }",
            "E0306",
        );
    }

    #[test]
    fn nonbool_condition_e0304() {
        assert_only_code(
            "fn main() -> i32 { return if 1 { 1 } else { 2 }; }",
            "E0304",
        );
    }

    #[test]
    fn u64_literal_now_supported() {
        // Phase 2: all integer suffixes supported.
        assert_clean(
            "fn main() -> i32 { let x: u64 = 1u64; let y: u64 = x; let _z = y; return 0; }",
        );
    }

    #[test]
    fn main_must_return_i32_e0309() {
        let codes = errors("fn main() { }");
        assert!(codes.contains(&"E0309"), "expected E0309 in {codes:?}");
    }

    #[test]
    fn return_without_value_e0307() {
        assert_only_code(
            "fn f() -> i32 { return; }\nfn main() -> i32 { return f(); }",
            "E0307",
        );
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
        assert_only_code("fn main() -> i32 { #println(1, 2); return 0; }", "E0308");
    }

    #[test]
    fn arg_type_mismatch_e0302() {
        assert_only_code("fn main() -> i32 { #println(true); return 0; }", "E0302");
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
        let codes =
            errors("fn main() -> i32 { let x: f64 = 1.0; let y: f64 = x & 1.0; return 0; }");
        assert!(
            codes.contains(&"E0302"),
            "expected E0302 on float &, got: {codes:?}"
        );
    }

    #[test]
    fn bitwise_on_bool_e0302() {
        let codes = errors("fn main() -> i32 { let b: bool = true | false; return 0; }");
        assert!(
            codes.contains(&"E0302"),
            "expected E0302 on bool |, got: {codes:?}"
        );
    }

    #[test]
    fn bit_not_on_float_e0302() {
        let codes = errors("fn main() -> i32 { let x: f64 = 1.0; let y: f64 = ~x; return 0; }");
        assert!(
            codes.contains(&"E0302"),
            "expected E0302 on ~f64, got: {codes:?}"
        );
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
             }",
        );
    }

    #[test]
    fn shift_count_must_be_integer_e0302() {
        let codes =
            errors("fn main() -> i32 { let x: i64 = 1 as i64; let y: i64 = x << 1.0; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn wrapping_ops_now_supported() {
        assert_clean("fn main() -> i32 { return (1 +% 2) -% 1 *% 1; }");
    }

    #[test]
    fn wrapping_op_on_float_e0302() {
        let codes =
            errors("fn main() -> i32 { let x: f64 = 1.0; let y: f64 = x +% 2.0; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn wrapping_op_on_bool_e0302() {
        let codes = errors("fn main() -> i32 { let _b: bool = true +% false; return 0; }");
        assert!(codes.contains(&"E0302"), "expected E0302, got: {codes:?}");
    }

    #[test]
    fn cast_now_supported() {
        assert_clean("fn main() -> i32 { return 1 as i32; }");
    }

    #[test]
    fn ref_not_supported_e0312() {
        assert_only_code(
            "fn main() -> i32 { let x = 1; let y = &x; return 0; }",
            "E0312",
        );
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
        assert_clean(
            "fn main() -> i32 { let b: bool = true == false; return if b { 1 } else { 0 }; }",
        );
    }

    // ---- Phase 2 slice 1: full primitive types + casts ----

    #[test]
    fn all_integer_types_resolve() {
        for t in [
            "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "isize", "usize",
        ] {
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
        assert!(
            codes.contains(&"E0302"),
            "expected mixed-type error, got: {codes:?}"
        );
    }

    #[test]
    fn float_arithmetic_clean() {
        assert_clean(
            "fn main() -> i32 { let x: f64 = 1.0 + 2.0 * 3.0; let _y: f64 = x; return 0; }",
        );
    }

    #[test]
    fn float_modulo_rejected_e0316() {
        assert_only_code(
            "fn main() -> i32 { let x: f64 = 1.0 % 2.0; let _y: f64 = x; return 0; }",
            "E0316",
        );
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

    // v0.0.12 G-023: LHS type annotation propagates into unary-minus
    // operand so `let x: i64 = -100;` works the same way the positive
    // literal `let x: i64 = 100;` already did.

    #[test]
    fn neg_lit_i64_from_lhs_clean() {
        assert_clean("fn main() -> i32 { let _x: i64 = -100; return 0; }");
    }

    #[test]
    fn neg_lit_i16_from_lhs_clean() {
        assert_clean("fn main() -> i32 { let _x: i16 = -32768; return 0; }");
    }

    #[test]
    fn neg_lit_i8_from_lhs_clean() {
        assert_clean("fn main() -> i32 { let _x: i8 = -1; return 0; }");
    }

    #[test]
    fn neg_lit_i64_past_i32_min_clean() {
        assert_clean("fn main() -> i32 { let _x: i64 = -2_147_483_649; return 0; }");
    }

    #[test]
    fn neg_lit_f32_from_lhs_clean() {
        assert_clean("fn main() -> i32 { let _x: f32 = -1.5f32; return 0; }");
    }

    #[test]
    fn neg_lit_unsigned_target_still_e0302() {
        let codes = errors("fn main() -> i32 { let _x: u32 = -1; return 0; }");
        assert!(codes.contains(&"E0302"));
    }

    // v0.0.12 G-022: E0333 diagnostic suggests `;` for unit-typed tail
    // blocks in unit-returning functions, and `return ...;` only when
    // there's an actual value being abandoned.

    fn first_e0333_message(src: &str) -> String {
        let d = check_src(src)
            .into_iter()
            .find(|d| d.code.0 == "E0333")
            .expect("expected E0333");
        d.message
    }

    #[test]
    fn e0333_unit_tail_suggests_semicolon_g022() {
        let msg = first_e0333_message(
            "fn f() { unsafe { let mut x: i32 = 1; x = x +% 1; } }\n\
             fn main() -> i32 { f(); return 0; }",
        );
        assert!(
            msg.contains("add `;`"),
            "expected `;`-fix in unit-tail message, got: {msg}"
        );
    }

    #[test]
    fn e0333_value_tail_still_suggests_return_g022() {
        let msg = first_e0333_message(
            "fn f() -> i32 { 42 }\n\
             fn main() -> i32 { return f(); }",
        );
        assert!(
            msg.contains("`return ...;`"),
            "expected `return ...;` suggestion for value tail, got: {msg}"
        );
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
        assert_only_code(
            "fn main() -> i32 { let _b: bool = 1 as bool; return 0; }",
            "E0315",
        );
    }

    #[test]
    fn cast_float_to_bool_rejected_e0315() {
        assert_only_code(
            "fn main() -> i32 { let _b: bool = 1.0 as bool; return 0; }",
            "E0315",
        );
    }

    #[test]
    fn cast_bool_to_float_rejected_e0315() {
        assert_only_code(
            "fn main() -> i32 { let _b: f64 = true as f64; return 0; }",
            "E0315",
        );
    }

    #[test]
    fn comparison_works_on_all_numeric_types() {
        assert_clean("fn main() -> i32 { return if 1u64 < 2u64 { 1 } else { 0 }; }");
        assert_clean("fn main() -> i32 { return if 1.0 < 2.0 { 1 } else { 0 }; }");
        assert_clean(
            "fn main() -> i32 { let a: i8 = 1; let b: i8 = 2; return if a < b { 1 } else { 0 }; }",
        );
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
             fn main() -> i32 { let _c: Color = Color::Red; return 0; }",
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
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = Color::Purple; return 0; }",
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
        let codes = errors("enum E { A, B }\nfn main() -> i32 { if E::A < E::B { 1 } else { 0 } }");
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn enum_to_int_cast_clean() {
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             fn main() -> i32 { return Color::Green as i32; }",
        );
    }

    #[test]
    fn int_to_enum_cast_rejected_e0315() {
        let codes = errors(
            "enum Color { Red }\nfn main() -> i32 { let _c: Color = 0 as Color; return 0; }",
        );
        assert!(codes.contains(&"E0315"));
    }

    #[test]
    fn assigning_int_to_enum_rejected_e0302() {
        let codes = errors("enum Color { Red }\nfn main() -> i32 { let _c: Color = 0; return 0; }");
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn assigning_enum_to_int_rejected_e0302() {
        let codes =
            errors("enum Color { Red }\nfn main() -> i32 { let _x: i32 = Color::Red; return 0; }");
        assert!(codes.contains(&"E0302"));
    }

    #[test]
    fn cross_enum_comparison_rejected_e0302() {
        let codes = errors(
            "enum A { X }\nenum B { Y }\n\
             fn main() -> i32 { if A::X == B::Y { 1 } else { 0 } }",
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
             fn main() -> i32 { let _p: Point = Point { x: 1, y: 2 }; return 0; }",
        );
    }

    #[test]
    fn empty_struct_clean() {
        assert_clean(
            "struct Empty {}\n\
             fn main() -> i32 { let _e: Empty = Empty {}; return 0; }",
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
             fn main() -> i32 { let mut p: Point = Point { x: 1, y: 2 }; p.x = 10; return 0; }",
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
             fn main() -> i32 { let a: A = A { x: 1 }; let _v: i32 = a.y; return 0; }",
        );
        assert!(codes.contains(&"E0320"));
    }

    #[test]
    fn missing_field_in_literal_e0321() {
        let codes = errors(
            "struct A { x: i32, y: i32 }\n\
             fn main() -> i32 { let _a: A = A { x: 1 }; return 0; }",
        );
        assert!(codes.contains(&"E0321"));
    }

    #[test]
    fn extra_field_in_literal_e0322() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let _a: A = A { x: 1, y: 2 }; return 0; }",
        );
        assert!(codes.contains(&"E0322"));
    }

    #[test]
    fn field_access_on_non_struct_e0323() {
        let codes = errors("fn main() -> i32 { let x: i32 = 5; let _v: i32 = x.foo; return 0; }");
        assert!(codes.contains(&"E0323"));
    }

    #[test]
    fn field_assign_on_immutable_e0305() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { let a: A = A { x: 1 }; a.x = 2; return 0; }",
        );
        assert!(codes.contains(&"E0305"));
    }

    #[test]
    fn assign_to_temporary_struct_e0313() {
        let codes = errors(
            "struct A { x: i32 }\n\
             fn main() -> i32 { A { x: 1 }.x = 2; return 0; }",
        );
        assert!(codes.contains(&"E0313"));
    }

    #[test]
    fn duplicate_struct_name_e0301() {
        let codes =
            errors("struct P { x: i32 }\nstruct P { y: i32 }\nfn main() -> i32 { return 0; }");
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
        assert_clean("struct B { a: A }\nstruct A { x: i32 }\nfn main() -> i32 { return 0; }");
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
             fn main() -> i32 { let _p: P = P::new(5); return 0; }",
        );
    }

    #[test]
    fn ref_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.get(); }",
        );
    }

    #[test]
    fn ref_mut_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn set(mut self, v: i32) { self.x = v; } }\n\
             fn main() -> i32 { let mut p: P = P { x: 0 }; p.set(5); return p.x; }",
        );
    }

    #[test]
    fn value_self_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn into_x(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { let p: P = P { x: 7 }; return p.into_x(); }",
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
    fn impl_on_concrete_enum_accepted_phase2c() {
        // v0.0.5 Phase 2C: `impl EnumName { ... }` on a non-generic
        // enum is now accepted; the old E0325 rejection lifted. Generic
        // enum impls (e.g. `impl Option[T]`) still error pending the
        // monomorphize-side synthesis.
        let codes =
            errors("enum E { A }\nimpl E { fn f(self) {} }\nfn main() -> i32 { return 0; }");
        assert!(
            !codes.contains(&"E0325"),
            "expected non-generic enum impl to be accepted; got: {codes:?}"
        );
    }

    #[test]
    fn duplicate_method_e0326() {
        let codes = errors(
            "struct P {}\nimpl P { fn f(self) {} fn f(self) {} }\nfn main() -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0326"));
    }

    #[test]
    fn no_such_method_e0324() {
        let codes = errors(
            "struct P {}\nimpl P {}\nfn main() -> i32 { let p: P = P {}; return p.missing(); }",
        );
        assert!(codes.contains(&"E0324"));
    }

    #[test]
    fn calling_assoc_fn_as_method_e0327() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn make() -> P { return P { x: 0 }; } }\n\
             fn main() -> i32 { let p: P = P { x: 0 }; let _q: P = p.make(); return 0; }",
        );
        assert!(codes.contains(&"E0327"));
    }

    #[test]
    fn calling_method_via_type_e0327() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn get(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { return P::get(); }",
        );
        assert!(codes.contains(&"E0327"));
    }

    #[test]
    fn calling_mut_method_on_immutable_e0328() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn bump(mut self) { self.x = self.x + 1; } }\n\
             fn main() -> i32 { let p: P = P { x: 0 }; p.bump(); return 0; }",
        );
        assert!(codes.contains(&"E0328"));
    }

    #[test]
    fn self_in_function_body_e0300() {
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn bad() -> i32 { return self.x; } }\n\
             fn main() -> i32 { return 0; }",
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
             fn main() -> i32 { return E::A(); }",
        );
        assert!(codes.contains(&"E0327"));
    }

    // ---- Phase 2 slice 2D: fixed-size arrays ----

    #[test]
    fn array_decl_and_literal_clean() {
        assert_clean("fn main() -> i32 { let _xs: [i32; 3] = [1, 2, 3]; return 0; }");
    }

    #[test]
    fn array_indexing_clean() {
        assert_clean(
            "fn main() -> i32 { let xs: [i32; 3] = [10, 20, 30]; return xs[0 as usize]; }",
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
             fn main() -> i32 { let c: C = C { xs: [0, 0] }; c.xs[0 as usize] = 5; return 0; }",
        );
        assert!(codes.contains(&"E0305"));
    }

    #[test]
    fn array_in_function_signature_clean() {
        assert_clean(
            "fn first(xs: [i32; 3]) -> i32 { return xs[0 as usize]; }\n\
             fn main() -> i32 { return first([10, 20, 30]); }",
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

    // ---- v0.0.10 Phase 5: default-move flip + `borrow` opt-out ----

    #[test]
    fn phase5_implicit_non_copy_param_consumes_e0335() {
        // No explicit `move` — but the param is non-Copy, so under
        // v0.0.10 Phase 5 semantics the caller's `p` is consumed.
        // Reading `p.x` after the call fires E0335.
        let codes = errors(
            "struct P { x: i32 }\n\
             impl P { fn drop(mut self) {} }\n\
             fn echo(p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 {\n\
                 let p: P = P { x: 1 };\n\
                 let r: i32 = echo(p);\n\
                 return p.x;\n\
             }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got {codes:?}");
    }

    #[test]
    fn phase5_borrow_param_does_not_consume_clean() {
        // `borrow x: T` opts out of the move-by-default. Caller can read
        // the binding after the call.
        assert_clean(
            "struct P { x: i32 }\n\
             impl P { fn drop(mut self) {} }\n\
             fn peek(borrow p: P) -> i32 { return p.x; }\n\
             fn main() -> i32 {\n\
                 let p: P = P { x: 1 };\n\
                 let r: i32 = peek(p);\n\
                 return p.x;\n\
             }",
        );
    }

    #[test]
    fn phase5_copy_param_unchanged_clean() {
        // Copy types are never consumed at call sites, regardless of
        // the move marker on the param. (Sanity: Phase 5 doesn't break
        // existing primitive-type call patterns.)
        assert_clean(
            "fn echo(n: i32) -> i32 { return n; }\n\
             fn main() -> i32 {\n\
                 let n: i32 = 7;\n\
                 let r: i32 = echo(n);\n\
                 return n;\n\
             }",
        );
    }

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

    // ---- v0.0.15: generic-instantiation Copy classification + flow-sensitive
    // moves. A generic struct instantiation (`Held[i32]`) inherits its
    // template's Drop, so an enum/struct using it as a payload/field is non-Copy
    // and a use-after-move is caught — but only the *linear* (non-diverging)
    // moves count, so the common `if done { return consume(x); } use(x);` shape
    // is not a false positive.

    #[test]
    fn generic_payload_enum_use_after_move_e0335() {
        // `Held[i32]` has a Drop (from `impl Held[T]`), so `W` is non-Copy;
        // using `w` after it's moved into a call must be caught. Pre-fix, the
        // instantiation was misclassified Copy → no move tracking → undetected.
        let codes = errors(
            "struct Held[T] { v: T }\n\
             impl Held[T] { fn drop(mut self) { return; } }\n\
             enum W { A(Held[i32]), B }\n\
             fn sink(w: W) -> i32 { return 0; }\n\
             fn main() -> i32 { let w: W = W::B; let a: i32 = sink(w); let b: i32 = sink(w); return a +% b; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    fn recursive_generic_payload_enum_use_after_move_e0335() {
        // The recursive `Node { Branch(Vec[Node]) }` shape, modeled with a
        // self-contained generic Drop struct.
        let codes = errors(
            "struct Held[T] { v: T }\n\
             impl Held[T] { fn drop(mut self) { return; } }\n\
             enum Node { Leaf(i32), Branch(Held[Node]) }\n\
             fn sink(n: Node) -> i32 { return 0; }\n\
             fn main() -> i32 { let n: Node = Node::Leaf(1); let a: i32 = sink(n); let b: i32 = sink(n); return a +% b; }",
        );
        assert!(codes.contains(&"E0335"), "expected E0335, got: {codes:?}");
    }

    #[test]
    fn move_in_returning_branch_not_flagged_clean() {
        // `h` is moved only on branches that `return`, so the code reached when
        // those branches are NOT taken still sees `h` live. Flow-sensitive move
        // tracking must not report a use-after-move here. (Pre-fix this was a
        // false positive once `Held[i32]` became correctly non-Copy — it broke
        // the stdlib read loops.)
        assert_clean(
            "struct Held[T] { v: T }\n\
             impl Held[T] { fn drop(mut self) { return; } }\n\
             fn consume(h: Held[i32]) -> i32 { return 0; }\n\
             fn pick(flag: bool) -> i32 {\n\
                 let h: Held[i32] = Held[i32] { v: 1 };\n\
                 if flag { return consume(h); }\n\
                 return consume(h);\n\
             }\n\
             fn main() -> i32 { return pick(false); }",
        );
    }

    #[test]
    fn move_across_exclusive_match_arms_not_flagged_clean() {
        // Both arms move `x`, but arms are mutually exclusive: a move in one arm
        // must not poison `x` for the other. Flow-sensitive per-arm reset keeps
        // this clean.
        assert_clean(
            "struct Held[T] { v: T }\n\
             impl Held[T] { fn drop(mut self) { return; } }\n\
             enum Opt { Some(i32), None }\n\
             fn consume(h: Held[i32]) -> i32 { return 0; }\n\
             fn f(o: Opt) -> i32 {\n\
                 let x: Held[i32] = Held[i32] { v: 1 };\n\
                 let r: i32 = match o { Opt::Some(n) => consume(x), Opt::None => consume(x) };\n\
                 return r;\n\
             }\n\
             fn main() -> i32 { return f(Opt::None); }",
        );
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
        assert_clean("fn main() -> i32 { defer #println(1); return 0; }");
    }

    #[test]
    fn defer_with_type_error_e0302() {
        // The deferred expression is type-checked; passing the wrong type
        // to println surfaces the regular type-error.
        let codes = errors("fn main() -> i32 { defer #println(true); return 0; }");
        // println takes i32; bool argument is a mismatch.
        assert!(
            codes.contains(&"E0302") || codes.contains(&"E0308"),
            "expected type-error on defer body, got: {codes:?}"
        );
    }

    #[test]
    fn defer_in_inner_block_clean() {
        assert_clean("fn main() -> i32 { if 1 == 1 { defer #println(42); } return 0; }");
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
        let codes = errors("fn main() -> i32 { let x: i32 = 5; return match x { _ => 0 }; }");
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
    fn tagged_enum_with_drop_payload_now_allowed() {
        // v0.0.14: a tagged enum with an owning (Drop) payload used to be
        // E0344; it is now allowed — codegen synthesizes enum-variant drop.
        assert_clean(
            "struct R { x: i32 }\n\
             impl R { fn drop(mut self) {} }\n\
             enum E { Hold(R), Empty }\n\
             fn main() -> i32 { return 0; }",
        );
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
        assert_clean("fn main() -> i32 { let x: i32; x = 5; return x; }");
    }

    #[test]
    fn uninit_let_read_before_assign_e0345() {
        let codes = errors("fn main() -> i32 { let x: i32; return x; }");
        assert!(codes.contains(&"E0345"), "expected E0345, got: {codes:?}");
    }

    #[test]
    fn uninit_let_no_type_e0346() {
        let codes = errors("fn main() -> i32 { let x; x = 5; return x; }");
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
        let codes = errors("fn main() -> i32 { let x: i32; if 1 == 1 { x = 1; } return x; }");
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
        assert_clean("fn main() -> i32 { let x: i32; x = 5; return x; }");
    }

    #[test]
    fn second_write_to_unmut_after_init_e0305() {
        // After the first write initializes the immutable binding, further
        // writes need `mut`. This test confirms the second write is
        // rejected with the same E0305 rule that governs assignment to
        // already-initialized immutable bindings.
        let codes = errors("fn main() -> i32 { let x: i32; x = 5; x = 6; return x; }");
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
        assert_clean("fn pick[A, B](a: A, b: B) -> A { return a; } fn main() -> i32 { return 0; }");
    }

    #[test]
    fn generic_struct_field_uses_param_clean() {
        assert_clean("struct Pair[A, B] { first: A, second: B } fn main() -> i32 { return 0; }");
    }

    #[test]
    fn generic_enum_payload_uses_param_clean() {
        assert_clean("enum Maybe[T] { Some(T), None } fn main() -> i32 { return 0; }");
    }

    #[test]
    fn unknown_type_param_still_e0303() {
        let codes = errors("fn id[T](x: T) -> U { return x; } fn main() -> i32 { return 0; }");
        assert!(
            codes.contains(&"E0303"),
            "expected E0303 for unknown U, got: {codes:?}"
        );
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
        assert!(
            codes.contains(&"E0501"),
            "expected E0501 for arity mismatch, got: {codes:?}"
        );
    }

    #[test]
    fn turbofish_on_non_generic_fn_e0501() {
        let codes = errors(
            "fn plain(x: i32) -> i32 { return x; } \
             fn main() -> i32 { return plain::[i32](7); }",
        );
        assert!(
            codes.contains(&"E0501"),
            "expected E0501 on non-generic fn turbofish, got: {codes:?}"
        );
    }

    #[test]
    fn turbofish_arg_type_validated_against_substituted_param() {
        // identity[i32] expects i32; passing bool fires E0302.
        let codes = errors(
            "fn identity[T](x: T) -> T { return x; } \
             fn main() -> i32 { let a: i32 = identity::[i32](true); return a; }",
        );
        assert!(
            codes.contains(&"E0302"),
            "expected E0302 for arg/type-arg mismatch, got: {codes:?}"
        );
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
        let codes =
            errors("fn main() -> i32 { let a: Bogus[i32] = Bogus[i32]::Some(7); return 0; }");
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
        let codes = errors("fn main() -> i32 { let x: i32 = 7; return *x; }");
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
                 return unsafe { printf(#str_ptr(\"hi %d\\n\"), 7) }; \
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
        assert_clean("fn main() -> i32 { let s: str = \"hello\"; return 0; }");
    }

    #[test]
    fn str_literal_typed_inferred_clean() {
        // No type annotation; literal's natural type is `str`.
        assert_clean("fn main() -> i32 { let s = \"hello\"; return 0; }");
    }

    #[test]
    fn println_accepts_str_clean() {
        // 8.STR.2: println overload accepts str.
        assert_clean("fn main() -> i32 { #println(\"hi\"); return 0; }");
    }

    #[test]
    fn println_rejects_non_int_non_str_arg_e0302() {
        // Phase 8 narrowed println: bool, structs, etc. all rejected.
        let codes = errors("fn main() -> i32 { #println(true); return 0; }");
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
        assert_clean("fn main() -> i32 { let n: usize = #size_of::[i32](); return 0; }");
    }

    #[test]
    fn align_of_primitive_clean() {
        assert_clean("fn main() -> i32 { let a: usize = #align_of::[i32](); return 0; }");
    }

    #[test]
    fn size_of_struct_clean() {
        assert_clean(
            "struct Point { x: i32, y: i32 } \
             fn main() -> i32 { let n: usize = #size_of::[Point](); return 0; }",
        );
    }

    #[test]
    fn size_of_returns_usize() {
        // Result must be usable in usize arithmetic without a cast.
        assert_clean(
            "fn main() -> i32 { let n: usize = #size_of::[i32]() *% 10 as usize; return 0; }",
        );
    }

    #[test]
    fn size_of_no_type_arg_rejected_e0501() {
        let codes = errors("fn main() -> i32 { let n: usize = #size_of(); return 0; }");
        assert!(
            codes.contains(&"E0501"),
            "expected E0501 for missing type arg, got: {codes:?}"
        );
    }

    #[test]
    fn size_of_two_type_args_rejected_e0501() {
        let codes =
            errors("fn main() -> i32 { let n: usize = #size_of::[i32, bool](); return 0; }");
        assert!(
            codes.contains(&"E0501"),
            "expected E0501 for two type args, got: {codes:?}"
        );
    }

    #[test]
    fn size_of_with_value_arg_rejected_e0302() {
        let codes = errors("fn main() -> i32 { let n: usize = #size_of::[i32](7); return 0; }");
        assert!(
            codes.contains(&"E0302"),
            "expected E0302 for value arg, got: {codes:?}"
        );
    }

    #[test]
    fn size_of_unknown_type_rejected_e0303() {
        let codes = errors("fn main() -> i32 { let n: usize = #size_of::[Bogus](); return 0; }");
        assert!(
            codes.contains(&"E0303"),
            "expected E0303 for unknown type, got: {codes:?}"
        );
    }

    #[test]
    fn align_of_no_type_arg_rejected_e0501() {
        let codes = errors("fn main() -> i32 { let n: usize = #align_of(); return 0; }");
        assert!(
            codes.contains(&"E0501"),
            "expected E0501 for missing type arg, got: {codes:?}"
        );
    }

    #[test]
    fn size_of_raw_pointer_type_clean() {
        // Verifies size_of works for raw-pointer types — needed for
        // allocator implementations that hand out typed pointers.
        assert_clean("fn main() -> i32 { let n: usize = #size_of::[*u8](); return 0; }");
    }

    // Phase 11 / P3 from null design (design.md): integer-to-raw-pointer
    // casts. `0 as *T` is how C+ expresses FFI null and how integer addresses
    // become typed pointers. Gated by `unsafe` — the cast itself just
    // reinterprets bits; the unsafety is trusting the integer is a valid address.

    #[test]
    fn int_to_raw_pointer_cast_in_unsafe_clean() {
        assert_clean("fn main() -> i32 { let p: *u8 = unsafe { 0 as *u8 }; return 0; }");
    }

    #[test]
    fn int_to_raw_pointer_cast_outside_unsafe_rejected_e0801() {
        let codes = errors("fn main() -> i32 { let p: *u8 = 0 as *u8; return 0; }");
        assert!(
            codes.contains(&"E0801"),
            "expected E0801 outside unsafe, got: {codes:?}"
        );
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
        assert_clean("fn main() -> i32 { let p: **i32 = unsafe { 0 as **i32 }; return 0; }");
    }

    // ---- v0.0.9 Phase 6 (cpc-gaps G-016): raw-pointer → integer cast ----

    #[test]
    fn pointer_to_usize_cast_in_unsafe_clean() {
        // The canonical alignment-check shape from C ports:
        //   `(p as usize) % alignment`
        assert_clean(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: usize = unsafe { p as usize }; \
                return 0; \
             }",
        );
    }

    #[test]
    fn pointer_to_u64_cast_in_unsafe_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: u64 = unsafe { p as u64 }; \
                return 0; \
             }",
        );
    }

    #[test]
    fn pointer_to_isize_cast_in_unsafe_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: isize = unsafe { p as isize }; \
                return 0; \
             }",
        );
    }

    #[test]
    fn pointer_to_i64_cast_in_unsafe_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: i64 = unsafe { p as i64 }; \
                return 0; \
             }",
        );
    }

    #[test]
    fn pointer_to_typed_pointer_then_usize_clean() {
        // Realistic chain: opaque byte buffer → typed pointer → address.
        // Exercises both raw-ptr-to-raw-ptr (Phase 11) and the new
        // raw-ptr-to-integer cast in a single unsafe block.
        assert_clean(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let fp: *f32 = unsafe { p as *f32 }; \
                let addr: usize = unsafe { fp as usize }; \
                return 0; \
             }",
        );
    }

    #[test]
    fn pointer_to_usize_outside_unsafe_e0801() {
        // Cast itself is admitted by `cast_allowed`, but the unsafe gate
        // in `check_cast` fires when not inside an unsafe block.
        let codes = errors(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: usize = p as usize; \
                return 0; \
             }",
        );
        assert!(
            codes.contains(&"E0801"),
            "expected E0801 outside unsafe, got {codes:?}"
        );
    }

    #[test]
    fn pointer_to_u32_rejected_e0315() {
        // u32 isn't 64 bits — narrowing a pointer is almost always a bug,
        // so `cast_allowed` doesn't admit it even inside unsafe.
        let codes = errors(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: u32 = unsafe { p as u32 }; \
                return 0; \
             }",
        );
        assert!(
            codes.contains(&"E0315"),
            "expected E0315 for narrowing cast, got {codes:?}"
        );
    }

    #[test]
    fn pointer_to_i32_rejected_e0315() {
        let codes = errors(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: i32 = unsafe { p as i32 }; \
                return 0; \
             }",
        );
        assert!(
            codes.contains(&"E0315"),
            "expected E0315 for narrowing cast, got {codes:?}"
        );
    }

    #[test]
    fn pointer_to_bool_rejected_e0315() {
        let codes = errors(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let alive: bool = unsafe { p as bool }; \
                return 0; \
             }",
        );
        assert!(
            codes.contains(&"E0315"),
            "expected E0315 for ptr→bool, got {codes:?}"
        );
    }

    #[test]
    fn pointer_to_usize_roundtrip_back_to_pointer_clean() {
        // Roundtrip pattern from the llama port's aligned_offset helper:
        //   addr = p as usize; if addr % align == 0 { return addr as *T; }
        assert_clean(
            "fn main() -> i32 { \
                let p: *u8 = unsafe { 0 as *u8 }; \
                let addr: usize = unsafe { p as usize }; \
                let aligned: usize = addr; \
                let back: *u8 = unsafe { aligned as *u8 }; \
                return 0; \
             }",
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
        assert!(
            codes.contains(&"E0356"),
            "expected E0356 on non-extern link_name, got: {codes:?}"
        );
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
            "fn handler(x: i32) { #println(x); } \
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
        assert!(
            codes.contains(&"E0302"),
            "expected E0302 for signature mismatch, got: {codes:?}"
        );
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
        assert!(
            codes.contains(&"E0312"),
            "expected E0312 for bare fn-as-value without expected type, got: {codes:?}"
        );
    }

    #[test]
    fn generic_fn_as_pointer_rejected_e0821() {
        let codes = errors(
            "fn identity[T](x: T) -> T { return x; } \
             fn main() -> i32 { let f: fn(i32) -> i32 = identity; return 0; }",
        );
        assert!(
            codes.contains(&"E0821"),
            "expected E0821 for generic fn as pointer, got: {codes:?}"
        );
    }

    #[test]
    fn indirect_call_wrong_arity_e0308() {
        let codes = errors(
            "fn double(x: i32) -> i32 { return x +% x; } \
             fn main() -> i32 { let f: fn(i32) -> i32 = double; return f(1, 2); }",
        );
        assert!(
            codes.contains(&"E0308"),
            "expected E0308 for wrong arity, got: {codes:?}"
        );
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
        // #size_of::[T]() inside a generic fn body — the type arg `T` is
        // a Ty::Param at sema-time; resolve_type allows it; monomorphize
        // substitutes T to the concrete type via subst_type_ast.
        assert_clean(
            "fn typed_alloc[T](n: usize) -> usize { return n *% #size_of::[T](); } \
             fn main() -> i32 { let bytes: usize = typed_alloc::[i32](10 as usize); return 0; }",
        );
    }

    // ---- Phase 5 Slice 5.C: `pub extern fn body` export signature gates ----

    #[test]
    fn pub_extern_fn_with_scalar_args_clean() {
        assert_clean("pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }");
    }

    #[test]
    fn pub_extern_fn_with_raw_pointer_clean() {
        assert_clean("pub extern fn load(p: *i32) -> i32 { return unsafe { *p }; }");
    }

    #[test]
    fn pub_extern_fn_with_repr_c_struct_clean() {
        assert_clean(
            "#[repr(C)]\n\
             struct Point { x: i32, y: i32 }\n\
             pub extern fn sum(p: Point) -> i32 { return p.x + p.y; }",
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
             pub extern fn take(o: Opt) -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_plain_enum_clean() {
        // Plain (untagged) enum lowers to i32 — fine across the C ABI.
        assert_clean(
            "enum Color { Red, Green, Blue }\n\
             pub extern fn pick(c: Color) -> i32 { return 0; }",
        );
    }

    #[test]
    fn pub_extern_fn_with_non_repr_c_struct_rejected_e0410() {
        let codes = errors(
            "struct Point { x: i32, y: i32 }\n\
             pub extern fn sum(p: Point) -> i32 { return p.x + p.y; }",
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_drop_struct_rejected_e0410() {
        let codes = errors(
            "#[repr(C)]\n\
             struct R { fd: i32 }\n\
             impl R { fn drop(mut self) { return; } }\n\
             pub extern fn use_it(r: R) -> i32 { return r.fd; }",
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_unit_return_clean() {
        // Unit return is fine — maps to C `void`.
        assert_clean("pub extern fn noop() { return; }");
    }

    #[test]
    fn pub_extern_fn_with_array_clean() {
        // Fixed-size array of C-compatible element is layout-compatible
        // with C `T[N]`.
        assert_clean("pub extern fn first(xs: [i32; 4]) -> i32 { return xs[0 as usize]; }");
    }

    #[test]
    fn pub_extern_fn_with_fn_ptr_clean() {
        // Function-pointer params/returns work when their own signatures
        // are C-exportable.
        assert_clean("pub extern fn invoke(f: fn(i32) -> i32, x: i32) -> i32 { return f(x); }");
    }

    #[test]
    fn pub_extern_fn_with_fn_ptr_of_slice_rejected_e0410() {
        // A fn-ptr whose param uses a non-C type propagates the rejection.
        let codes = errors("pub extern fn bad(f: fn(i32[]) -> i32) -> i32 { return 0; }");
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    #[test]
    fn pub_extern_fn_with_non_repr_c_field_in_struct_rejected_e0410() {
        // A `#[repr(C)]` struct still must have C-exportable fields.
        let codes = errors(
            "#[repr(C)]\n\
             struct Outer { inner: str }\n\
             pub extern fn use_it(o: Outer) -> i32 { return 0; }",
        );
        assert!(codes.contains(&"E0410"), "expected E0410, got: {codes:?}");
    }

    // ---- v0.0.3 Phase 5 Slice 5E.2: async fn + await sema ----

    const FUTURE_PRELUDE: &str = "pub struct Future[T] { pub opaque handle: *u8 } ";

    #[test]
    fn async_fn_body_returns_inner_type() {
        // `async fn foo() -> i32` body uses `return X` for X: i32,
        // NOT for X: Future[i32]. The `Future[i32]` wrap is sema's
        // view at call sites only.
        assert_clean(&format!(
            "{FUTURE_PRELUDE}async fn fetch() -> i32 {{ return 42 as i32; }}"
        ));
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
        assert!(
            codes.contains(&"E0902"),
            "expected E0902 (await of non-Future), got: {codes:?}"
        );
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
        assert!(
            codes.contains(&"E0300"),
            "expected E0300 (Future not in scope), got: {codes:?}"
        );
    }

    // ---- v0.0.4 Phase 1D: E0900 borrow-across-await parameter guard ----

    #[test]
    fn async_fn_with_str_param_emits_e0900() {
        let codes = errors(&format!(
            "{FUTURE_PRELUDE}async fn fetch(url: str) -> i32 {{ return 0 as i32; }}"
        ));
        assert!(
            codes.contains(&"E0900"),
            "expected E0900 (str borrow in async param), got: {codes:?}"
        );
    }

    #[test]
    fn async_fn_with_slice_param_emits_e0900() {
        let codes = errors(&format!(
            "{FUTURE_PRELUDE}async fn proc(buf: i32[]) -> i32 {{ return 0 as i32; }}"
        ));
        assert!(
            codes.contains(&"E0900"),
            "expected E0900 (slice borrow in async param), got: {codes:?}"
        );
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
        assert!(
            codes.contains(&"E0900"),
            "expected E0900 (mut non-Copy param), got: {codes:?}"
        );
    }

    // ---- v0.0.4 Phase 2 Slice 2A: Send / Sync marker interfaces ----

    #[test]
    fn send_bound_accepts_primitive() {
        // `fn worker[T: Send](x: T) -> T { return x; }` instantiated with
        // i32 must pass. v0.0.4 baseline is permissive — every type is
        // Send — so this is the canonical "vocabulary works" check.
        assert_clean(
            "fn worker[T: Send](x: T) -> T { return x; }\n\
             fn main() -> i32 { return worker::[i32](42); }",
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
             }",
        );
    }

    #[test]
    fn sync_bound_accepts_primitive() {
        // Same shape, Sync bound.
        assert_clean(
            "fn share[T: Sync](x: T) -> T { return x; }\n\
             fn main() -> i32 { return share::[i32](42); }",
        );
    }

    #[test]
    fn send_and_sync_compose_with_other_bounds() {
        // Multiple bounds on one type param — verifies the bound-list
        // parsing/resolution sees Send / Sync as first-class.
        assert_clean(
            "fn need_both[T: Send + Sync](x: T) -> T { return x; }\n\
             fn main() -> i32 { return need_both::[i32](7); }",
        );
    }

    // ---- v0.0.12 realtime Phase 6 (core): Rc / MutexGuard are !Send ----

    #[test]
    fn send_bound_rejects_rc_e0502() {
        // `Rc[T]` is `!Send` — passing one to a `Send`-bounded generic fails.
        // (Matched by template-name leaf, so a local `Rc` exercises the rule.)
        let codes = errors(
            "struct Rc[T] { v: T }\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let r: Rc[i32] = Rc[i32] { v: 5 };\n\
                 let _q: Rc[i32] = ship::[Rc[i32]](r);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0502"), "got {:?}", codes);
    }

    #[test]
    fn send_bound_rejects_mutex_guard_e0502() {
        let codes = errors(
            "struct MutexGuard[T] { v: T }\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let g: MutexGuard[i32] = MutexGuard[i32] { v: 5 };\n\
                 let _q: MutexGuard[i32] = ship::[MutexGuard[i32]](g);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0502"), "got {:?}", codes);
    }

    #[test]
    fn sync_bound_rejects_rc_e0502() {
        let codes = errors(
            "struct Rc[T] { v: T }\n\
             fn share[T: Sync](x: T) -> T { return x; }\n\
             fn main() -> i32 {\n\
                 let r: Rc[i32] = Rc[i32] { v: 5 };\n\
                 let _q: Rc[i32] = share::[Rc[i32]](r);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0502"), "got {:?}", codes);
    }

    #[test]
    fn send_bound_accepts_other_generic_struct() {
        // A generic struct that is *not* Rc/MutexGuard stays Send.
        assert_clean(
            "struct Holder[T] { v: T }\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let h: Holder[i32] = Holder[i32] { v: 5 };\n\
                 let q: Holder[i32] = ship::[Holder[i32]](h);\n\
                 return q.v;\n\
             }",
        );
    }

    #[test]
    fn mutex_guard_is_still_sync() {
        // Only Rc is !Sync; a MutexGuard satisfies a Sync bound.
        assert_clean(
            "struct MutexGuard[T] { v: T }\n\
             fn share[T: Sync](x: T) -> T { return x; }\n\
             fn main() -> i32 {\n\
                 let g: MutexGuard[i32] = MutexGuard[i32] { v: 5 };\n\
                 let q: MutexGuard[i32] = share::[MutexGuard[i32]](g);\n\
                 return q.v;\n\
             }",
        );
    }

    // ---- v0.0.14: broad raw-pointer !Send rule + `unsafe impl Send/Sync` ----

    #[test]
    fn send_rejects_raw_ptr_struct_e0502() {
        // A struct that hides a raw pointer is !Send by the structural rule.
        let codes = errors(
            "struct Handle { opaque p: *u8 }\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let h: Handle = Handle { p: unsafe { 0 as *u8 } };\n\
                 let _q: Handle = ship::[Handle](h);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0502"), "got {:?}", codes);
    }

    #[test]
    fn sync_rejects_raw_ptr_struct_e0502() {
        let codes = errors(
            "struct Handle { opaque p: *u8 }\n\
             fn share[T: Sync](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let h: Handle = Handle { p: unsafe { 0 as *u8 } };\n\
                 let _q: Handle = share::[Handle](h);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0502"), "got {:?}", codes);
    }

    #[test]
    fn unsafe_impl_send_overrides_raw_ptr_struct() {
        // `unsafe impl Send for Handle {}` re-enables the marker.
        assert_clean(
            "struct Handle { opaque p: *u8 }\n\
             unsafe impl Send for Handle {}\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let h: Handle = Handle { p: unsafe { 0 as *u8 } };\n\
                 let _q: Handle = ship::[Handle](h);\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn bare_raw_pointer_stays_send() {
        // A *bare* raw pointer (not wrapped in a nominal type) is still Send —
        // it is visibly unsafe at every use; the rule targets pointer-hiding
        // structs. Preserves `thread::spawn::[*u8]`.
        assert_clean(
            "fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let p: *u8 = unsafe { 0 as *u8 };\n\
                 let _q: *u8 = ship::[*u8](p);\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn unsafe_impl_send_conditional_generic_met() {
        // `unsafe impl Send for Arc[T: Send + Sync]` — Arc[i32] is Send
        // because i32 is Send + Sync.
        assert_clean(
            "struct Arc[T] { opaque ctrl: *u8 }\n\
             unsafe impl Send for Arc[T: Send + Sync] {}\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let a: Arc[i32] = Arc[i32] { ctrl: unsafe { 0 as *u8 } };\n\
                 let _q: Arc[i32] = ship::[Arc[i32]](a);\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn unsafe_impl_send_conditional_generic_unmet_e0502() {
        // Arc[Handle] is !Send: the conditional bound `T: Send` is unmet
        // because Handle (raw-ptr struct, no override) is itself !Send.
        let codes = errors(
            "struct Handle { opaque p: *u8 }\n\
             struct Arc[T] { opaque ctrl: *u8 }\n\
             unsafe impl Send for Arc[T: Send + Sync] {}\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let a: Arc[Handle] = Arc[Handle] { ctrl: unsafe { 0 as *u8 } };\n\
                 let _q: Arc[Handle] = ship::[Arc[Handle]](a);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0502"), "got {:?}", codes);
    }

    #[test]
    fn struct_holding_overridden_send_field_is_send() {
        // A struct whose only pointer is reached *through* a Send-overridden
        // sub-type is itself Send (the recursion stops at the override).
        assert_clean(
            "struct Arc[T] { opaque ctrl: *u8 }\n\
             unsafe impl Send for Arc[T: Send + Sync] {}\n\
             struct Wrap { inner: Arc[i32], tag: i32 }\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let w: Wrap = Wrap { inner: Arc[i32] { ctrl: unsafe { 0 as *u8 } }, tag: 1 };\n\
                 let _q: Wrap = ship::[Wrap](w);\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn safe_impl_send_rejected_e0860() {
        // `Send` is an unsafe assertion — a bare `impl Send` is rejected.
        let codes = errors(
            "struct Handle { opaque p: *u8 }\n\
             impl Send for Handle {}\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0860"), "got {:?}", codes);
    }

    #[test]
    fn unsafe_impl_on_regular_interface_rejected_e0861() {
        // `unsafe` applies only to the Send/Sync markers.
        let codes = errors(
            "interface Greet { fn hi(self) -> i32; }\n\
             struct S { x: i32 }\n\
             unsafe impl Greet for S { fn hi(self) -> i32 { return self.x; } }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0861"), "got {:?}", codes);
    }

    #[test]
    fn enum_with_raw_ptr_payload_is_not_send_e0502() {
        // The rule reaches through enum payloads too.
        let codes = errors(
            "enum Maybe { None, Ptr(*u8) }\n\
             fn ship[T: Send](v: T) -> T { return v; }\n\
             fn main() -> i32 {\n\
                 let m: Maybe = Maybe::None;\n\
                 let _q: Maybe = ship::[Maybe](m);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0502"), "got {:?}", codes);
    }

    // ---- v0.0.6 Slice 1A: include_bytes! ----

    /// Helper: write `src` to `<dir>/src.cplus`, write `bytes` to
    /// `<dir>/<asset_name>`, and run sema against the source file using
    /// the temp source's path so relative include_bytes! paths resolve.
    fn check_with_asset(src: &str, asset_name: &str, bytes: &[u8]) -> Vec<Diagnostic> {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_path = dir.path().join("src.cplus");
        let asset_path = dir.path().join(asset_name);
        std::fs::write(&src_path, src).expect("write src");
        std::fs::write(&asset_path, bytes).expect("write asset");
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, src_path, src);
        drop(dir);
        diags
    }

    #[test]
    fn include_bytes_clean_when_file_exists() {
        let diags = check_with_asset(
            "fn main() -> i32 { let p = #include_bytes(\"hello.bin\"); return 0; }",
            "hello.bin",
            b"hello",
        );
        assert!(diags.is_empty(), "expected clean, got: {:#?}", diags);
    }

    #[test]
    fn include_bytes_missing_file_e0870() {
        // Reference a sibling file that does not exist.
        let dir = tempfile::tempdir().expect("tempdir");
        let src_path = dir.path().join("src.cplus");
        let src = "fn main() -> i32 { let p = #include_bytes(\"missing.bin\"); return 0; }";
        std::fs::write(&src_path, src).expect("write");
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, src_path, src);
        let codes: Vec<_> = diags
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| d.code.0)
            .collect();
        assert!(codes.contains(&"E0870"), "expected E0870, got {:?}", codes);
    }

    #[test]
    fn include_bytes_non_literal_arg_parse_error() {
        // The parser only constructs ExprKind::IncludeBytes for the
        // strict `include_bytes!(StringLit)` form. A bare identifier
        // fails parse before sema sees it.
        let src = "fn main() -> i32 { let p = include_bytes!(some_var); return 0; }";
        let toks = tokenize(src).expect("lex");
        assert!(
            parse(toks).is_err(),
            "expected parse error on non-literal include_bytes! arg"
        );
    }

    #[test]
    fn include_bytes_type_is_rawptr_to_byte_array() {
        // Round-trip: assigning the result to a `*[u8; N]` typed local
        // succeeds; assigning to a wrong N is a type mismatch.
        let dir = tempfile::tempdir().expect("tempdir");
        let src_path = dir.path().join("src.cplus");
        let asset_path = dir.path().join("bytes.bin");
        std::fs::write(&src_path, "").expect("write");
        std::fs::write(&asset_path, b"abc").expect("write"); // len 3
        let src = "fn main() -> i32 { let p: *[u8; 3] = #include_bytes(\"bytes.bin\"); return 0; }";
        std::fs::write(&src_path, src).expect("write");
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, src_path, src);
        assert!(
            !diags.iter().any(|d| matches!(d.severity, Severity::Error)),
            "expected clean assignment to *[u8; 3], got: {:#?}",
            diags
        );
    }

    #[test]
    fn include_bytes_wrong_length_type_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_path = dir.path().join("src.cplus");
        let asset_path = dir.path().join("bytes.bin");
        std::fs::write(&asset_path, b"abc").expect("write"); // len 3
        let src = "fn main() -> i32 { let p: *[u8; 5] = #include_bytes(\"bytes.bin\"); return 0; }";
        std::fs::write(&src_path, src).expect("write");
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, src_path, src);
        assert!(
            diags.iter().any(|d| matches!(d.severity, Severity::Error)),
            "expected a type-mismatch diagnostic for [u8; 5] vs [u8; 3]"
        );
    }

    // ---- v0.0.7 Slice 2.1: SIMD shuffles + reductions + masked ops ----

    #[test]
    fn simd_compare_returns_mask_clean() {
        // `f32x4.lt(b)` produces a mask32x4 (signed-int SIMD of
        // matching width). Sema accepts assigning the result to a
        // mask32x4 binding.
        assert_clean(
            "fn main() -> i32 { \
                let a: f32x4 = f32x4::splat(1.0f32); \
                let b: f32x4 = f32x4::splat(2.0f32); \
                let m: mask32x4 = a.lt(b); \
                let _r: i32 = m.lane(0 as u32); \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_select_on_mask_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: f32x4 = f32x4::splat(1.0f32); \
                let b: f32x4 = f32x4::splat(2.0f32); \
                let m: mask32x4 = a.lt(b); \
                let blended: f32x4 = m.select(a, b); \
                let _r: f32 = blended.lane(0 as u32); \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_any_all_on_mask_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: i32x4 = i32x4::splat(5); \
                let m: mask32x4 = a.gt(i32x4::splat(0)); \
                let h: bool = m.any(); \
                let q: bool = m.all(); \
                if h { if q { return 0; } } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_reductions_return_lane_scalar() {
        assert_clean(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let s: f32 = v.sum(); \
                let p: f32 = v.product(); \
                let lo: f32 = v.min_across(); \
                let hi: f32 = v.max_across(); \
                if s != 0.0f32 { return 1; } \
                if p != 0.0f32 { return 2; } \
                if lo != 0.0f32 { return 3; } \
                if hi != 0.0f32 { return 4; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_reverse_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let r: f32x4 = v.reverse(); \
                let _x: f32 = r.lane(0 as u32); \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_swizzle_clean() {
        // Swizzle accepts a `[u32; N]` array literal.
        assert_clean(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32); \
                let s: f32x4 = v.swizzle([3 as u32, 2 as u32, 1 as u32, 0 as u32]); \
                let _y: f32 = s.lane(0 as u32); \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_interleave_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: i32x4 = i32x4::splat(1); \
                let b: i32x4 = i32x4::splat(2); \
                let lo: i32x4 = a.interleave_lo(b); \
                let hi: i32x4 = a.interleave_hi(b); \
                let _l: i32 = lo.lane(0 as u32); \
                let _h: i32 = hi.lane(0 as u32); \
                return 0; \
            }",
        );
    }

    // ---- v0.0.9 follow-up: Ty::Mask distinct from Ty::Simd ----

    #[test]
    fn simd_mask_no_implicit_coerce_from_simd_e0302() {
        // Pre-v0.0.9, mask32x4 was a type alias for i32x4 — assigning a
        // splat'd i32x4 to a mask binding worked silently. With Ty::Mask
        // distinct, the assignment is a real type mismatch.
        assert_only_code(
            "fn main() -> i32 { \
                let v: i32x4 = i32x4::splat(0); \
                let m: mask32x4 = v; \
                return 0; \
            }",
            "E0302",
        );
    }

    #[test]
    fn simd_mask_no_implicit_coerce_to_simd_e0302() {
        // The reverse direction: a mask value can't silently become an
        // integer SIMD. Caller must use `.to_bits()` explicitly.
        assert_only_code(
            "fn main() -> i32 { \
                let a: f32x4 = f32x4::splat(1.0f32); \
                let m: mask32x4 = a.lt(a); \
                let v: i32x4 = m; \
                return 0; \
            }",
            "E0302",
        );
    }

    #[test]
    fn simd_mask_arithmetic_rejected_e0324() {
        // Masks have no arithmetic — they're 0/all-ones bitmasks; `+`
        // would break the invariant. Caller must convert via `.to_bits()`
        // if they really want lane-wise arithmetic on the underlying ints.
        assert_only_code(
            "fn main() -> i32 { \
                let a: f32x4 = f32x4::splat(1.0f32); \
                let m1: mask32x4 = a.lt(a); \
                let m2: mask32x4 = a.gt(a); \
                let _r: mask32x4 = m1.add(m2); \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_mask_to_bits_clean() {
        // `.to_bits()` is the explicit Mask → Simd conversion (zero-cost
        // at the IR level; just relabels the type). Returns the signed-
        // int SIMD of matching lane width.
        assert_clean(
            "fn main() -> i32 { \
                let a: f32x4 = f32x4::splat(1.0f32); \
                let m: mask32x4 = a.lt(a); \
                let bits: i32x4 = m.to_bits(); \
                let _r: i32 = bits.lane(0 as u32); \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_simd_to_mask_clean() {
        // `.to_mask()` is the reverse — Simd → Mask. Only valid on
        // signed-int SIMD (the lane sign convention disambiguates).
        assert_clean(
            "fn main() -> i32 { \
                let v: i32x4 = i32x4::splat(0); \
                let m: mask32x4 = v.to_mask(); \
                let _b: bool = m.any(); \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_unsigned_to_mask_rejected_e0324() {
        // `to_mask` rejects unsigned-int SIMD — there's no `umaskNxM`
        // type and the lane-sign convention picks the signed form.
        assert_only_code(
            "fn main() -> i32 { \
                let v: u32x4 = u32x4::splat(0); \
                let m: mask32x4 = v.to_mask(); \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_mask_bitwise_combine_clean() {
        // `.and` / `.or` / `.xor` / `.not` work on Mask receivers
        // (mask combining is a primary use case) and return Mask.
        assert_clean(
            "fn main() -> i32 { \
                let a: f32x4 = f32x4::splat(1.0f32); \
                let m1: mask32x4 = a.lt(a); \
                let m2: mask32x4 = a.gt(a); \
                let both: mask32x4 = m1.and(m2); \
                let either: mask32x4 = m1.or(m2); \
                let neither: mask32x4 = both.not(); \
                if neither.any() { return 0; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_to_bits_on_simd_rejected_e0324() {
        // `to_bits` is mask-only; calling on a Simd is a misuse (the
        // value IS already an integer SIMD).
        assert_only_code(
            "fn main() -> i32 { \
                let v: i32x4 = i32x4::splat(0); \
                let _r: i32x4 = v.to_bits(); \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_select_on_non_mask_e0324() {
        // `.select` on a float SIMD (not a mask) must reject.
        let codes = errors(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let r: f32x4 = v.select(v, v); \
                if r.lane(0 as u32) != 0.0f32 { return 1; } \
                return 0; \
            }",
        );
        assert!(codes.contains(&"E0324"), "expected E0324, got {:?}", codes);
    }

    // ---- v0.0.7 Slice 2.2: 256-bit SIMD widths ----

    #[test]
    fn simd_f32x8_resolves_and_methods_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: f32x8 = f32x8::splat(1.0f32); \
                let b: f32x8 = f32x8::splat(2.0f32); \
                let c: f32x8 = a.add(b).mul(b).fma(a, b); \
                let s: f32x8 = c.sqrt(); \
                let lane: f32 = s.lane(7 as u32); \
                if lane != 0.0f32 { return 1; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_f64x4_resolves_and_methods_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: f64x4 = f64x4::new(1.0, 2.0, 3.0, 4.0); \
                let b: f64x4 = a.add(f64x4::splat(0.5)).min(f64x4::splat(10.0)); \
                if b.lane(0 as u32) != 0.0 { return 1; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_i32x8_int_methods_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: i32x8 = i32x8::splat(5); \
                let b: i32x8 = a.add(i32x8::splat(3)).abs(); \
                let c: i32x8 = b.and(i32x8::splat(0x0F)).shl(1 as u32); \
                if c.lane(0 as u32) != 0 { return 1; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_u64x4_unsigned_methods_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: u64x4 = u64x4::splat(10 as u64); \
                let b: u64x4 = a.min(u64x4::splat(5 as u64)).max(u64x4::splat(2 as u64)); \
                if b.lane(0 as u32) != (0 as u64) { return 1; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_256bit_widths_lane_out_of_range_e0874() {
        // f32x8 has 8 lanes; lane(8) is out of range.
        let codes = errors(
            "fn main() -> i32 { \
                let v: f32x8 = f32x8::splat(1.0f32); \
                let x: f32 = v.lane(8 as u32); \
                if x != 0.0f32 { return 1; } \
                return 0; \
            }",
        );
        assert!(codes.contains(&"E0874"), "expected E0874, got {:?}", codes);
    }

    // ---- v0.0.7 Slice 1.3: loop-hint attributes ----

    #[test]
    fn loop_unroll_in_range_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let mut i: i32 = 0; \
                #[unroll(4)] while i < 10 { i = i + 1; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn loop_vectorize_width_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let mut s: i32 = 0; \
                #[vectorize_width(8)] for i in 0..16 { s = s + i; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn loop_unroll_zero_e0510() {
        // N == 0 is out of range — must fire E0510.
        let codes = errors(
            "fn main() -> i32 { \
                let mut i: i32 = 0; \
                #[unroll(0)] while i < 10 { i = i + 1; } \
                return 0; \
            }",
        );
        assert!(codes.contains(&"E0510"), "expected E0510, got {:?}", codes);
    }

    #[test]
    fn loop_unroll_above_cap_e0510() {
        // N == 257 is out of range.
        let codes = errors(
            "fn main() -> i32 { \
                let mut i: i32 = 0; \
                #[unroll(257)] while i < 10 { i = i + 1; } \
                return 0; \
            }",
        );
        assert!(codes.contains(&"E0510"), "expected E0510, got {:?}", codes);
    }

    #[test]
    fn loop_attr_on_non_loop_e0356() {
        // The attrs walker rejects `#[unroll]` placement on a `let`
        // (well — actually a Pound followed by anything that isn't
        // while/loop/for is a parser error). Verify that the error
        // fires at the parsing boundary by attempting the construct.
        let toks =
            tokenize("fn main() -> i32 { #[unroll(4)] let x: i32 = 7; return x; }").expect("lex");
        assert!(
            parse(toks).is_err(),
            "expected parse error: loop-attr on a non-loop statement"
        );
    }

    // ---- v0.0.7 Slice 3.1: include_str! ----

    #[test]
    fn include_str_clean_when_file_is_utf8() {
        let diags = check_with_asset(
            "fn main() -> i32 { let s: str = #include_str(\"hello.txt\"); return 0; }",
            "hello.txt",
            "hello, world\n".as_bytes(),
        );
        assert!(diags.is_empty(), "expected clean, got: {:#?}", diags);
    }

    #[test]
    fn include_str_accepts_non_ascii_utf8() {
        // Multibyte UTF-8 (emoji + accented chars) must validate cleanly.
        let diags = check_with_asset(
            "fn main() -> i32 { let s: str = #include_str(\"utf8.txt\"); return 0; }",
            "utf8.txt",
            "café — résumé 🎉\n".as_bytes(),
        );
        assert!(diags.is_empty(), "expected clean, got: {:#?}", diags);
    }

    #[test]
    fn include_str_rejects_invalid_utf8_with_e0875() {
        // 0xFF is never valid as a UTF-8 leading byte; sema must reject.
        let diags = check_with_asset(
            "fn main() -> i32 { let s: str = #include_str(\"bad.bin\"); return 0; }",
            "bad.bin",
            &[b'o', b'k', 0xFF, b'!'],
        );
        let codes: Vec<_> = diags
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| d.code.0)
            .collect();
        assert!(codes.contains(&"E0875"), "expected E0875, got {:?}", codes);
    }

    #[test]
    fn include_str_missing_file_e0870() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_path = dir.path().join("src.cplus");
        let src = "fn main() -> i32 { let s: str = #include_str(\"missing.txt\"); return 0; }";
        std::fs::write(&src_path, src).expect("write");
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let diags = check(&prog, src_path, src);
        let codes: Vec<_> = diags
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| d.code.0)
            .collect();
        assert!(codes.contains(&"E0870"), "expected E0870, got {:?}", codes);
    }

    #[test]
    fn include_str_non_literal_arg_parse_error() {
        // Strict form: `include_str!(StringLit)` only.
        let src = "fn main() -> i32 { let s: str = include_str!(some_var); return 0; }";
        let toks = tokenize(src).expect("lex");
        assert!(
            parse(toks).is_err(),
            "expected parse error on non-literal include_str! arg"
        );
    }

    #[test]
    fn include_str_wrong_target_type_mismatch() {
        // Assigning the `str` result to a `*[u8; N]` typed local is a
        // type mismatch — `include_str!` and `include_bytes!` produce
        // different shapes.
        let diags = check_with_asset(
            "fn main() -> i32 { let p: *[u8; 5] = #include_str(\"hi.txt\"); return 0; }",
            "hi.txt",
            b"hello",
        );
        assert!(
            diags.iter().any(|d| matches!(d.severity, Severity::Error)),
            "expected type mismatch (str vs *[u8; 5])"
        );
    }

    // ---- v0.0.6 Slice 1B: SIMD types ----

    #[test]
    fn simd_f32x4_type_resolves_and_methods_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32); \
                let b: f32x4 = f32x4::splat(0.5f32); \
                let c: f32x4 = a.add(b); \
                let d: f32x4 = c.mul(a).fma(b, c); \
                let e: f32x4 = d.sqrt(); \
                let lane: f32 = e.lane(0 as u32); \
                let with: f32x4 = e.with_lane(1 as u32, 9.0f32); \
                let arr: [f32; 4] = with.to_array(); \
                let from: f32x4 = f32x4::from_array(arr); \
                if lane != 0.0f32 { return 1; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_lane_out_of_range_e0874() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let x: f32 = v.lane(7 as u32); \
                if x != 0.0f32 { return 1; } \
                return 0; \
            }",
            "E0874",
        );
    }

    #[test]
    fn simd_lane_non_literal_e0873() {
        let codes = errors(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let mut i: u32 = 0 as u32; \
                let x: f32 = v.lane(i); \
                if x != 0.0f32 { return 1; } \
                return 0; \
            }",
        );
        assert!(codes.contains(&"E0873"), "expected E0873, got {:?}", codes);
    }

    #[test]
    fn simd_unknown_method_e0324() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let r: f32x4 = v.frobnicate(v); \
                if r.lane(0 as u32) != 0.0f32 { return 1; } \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_wrong_arity_for_new_e0308() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::new(1.0f32, 2.0f32); \
                if v.lane(0 as u32) != 0.0f32 { return 1; } \
                return 0; \
            }",
            "E0308",
        );
    }

    #[test]
    fn simd_in_pub_extern_fn_rejected_e0410() {
        let codes = errors("pub extern fn f(v: f32x4) -> f32x4 { return v; }");
        assert!(codes.contains(&"E0410"), "expected E0410, got {:?}", codes);
    }

    #[test]
    fn simd_f64x2_type_resolves_and_methods_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: f64x2 = f64x2::new(1.0, 2.0); \
                let b: f64x2 = f64x2::splat(0.5); \
                let c: f64x2 = a.add(b).mul(b).fma(a, b); \
                let s: f64x2 = c.sqrt(); \
                let lane: f64 = s.lane(0 as u32); \
                let with: f64x2 = s.with_lane(1 as u32, 9.0); \
                let arr: [f64; 2] = with.to_array(); \
                let from: f64x2 = f64x2::from_array(arr); \
                if lane != 0.0 { return 1; } \
                if from.lane(1 as u32) != 0.0 { return 2; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_f64x2_lane_out_of_range_e0874() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: f64x2 = f64x2::splat(1.0); \
                let x: f64 = v.lane(5 as u32); \
                if x != 0.0 { return 1; } \
                return 0; \
            }",
            "E0874",
        );
    }

    #[test]
    fn simd_f64x2_wrong_arity_for_new_e0308() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: f64x2 = f64x2::new(1.0, 2.0, 3.0); \
                if v.lane(0 as u32) != 0.0 { return 1; } \
                return 0; \
            }",
            "E0308",
        );
    }

    #[test]
    fn simd_i32x4_type_and_int_methods_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: i32x4 = i32x4::new(1, 2, 3, 4); \
                let b: i32x4 = i32x4::splat(10); \
                let c: i32x4 = a.add(b).sub(b).mul(b).div(i32x4::splat(2)); \
                let d: i32x4 = c.abs(); \
                let lane: i32 = d.lane(0 as u32); \
                let with: i32x4 = d.with_lane(1 as u32, 99); \
                let arr: [i32; 4] = with.to_array(); \
                let from: i32x4 = i32x4::from_array(arr); \
                if lane != 0 { return 1; } \
                if from.lane(1 as u32) != 0 { return 2; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_i32x4_sqrt_rejected_e0324() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: i32x4 = i32x4::splat(1); \
                let r: i32x4 = v.sqrt(); \
                if r.lane(0 as u32) != 0 { return 1; } \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_i32x4_fma_rejected_e0324() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: i32x4 = i32x4::splat(1); \
                let r: i32x4 = v.fma(v, v); \
                if r.lane(0 as u32) != 0 { return 1; } \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_min_max_clean_across_widths() {
        assert_clean(
            "fn main() -> i32 { \
                let a4: f32x4 = f32x4::splat(1.0f32); \
                let b4: f32x4 = f32x4::splat(2.0f32); \
                let _f32: f32x4 = a4.min(b4).max(a4); \
                let a2: f64x2 = f64x2::splat(1.0); \
                let b2: f64x2 = f64x2::splat(2.0); \
                let _f64: f64x2 = a2.min(b2).max(a2); \
                let i4: i32x4 = i32x4::splat(1); \
                let j4: i32x4 = i32x4::splat(2); \
                let _i32: i32x4 = i4.min(j4).max(i4); \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_min_wrong_arity_e0308() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let r: f32x4 = v.min(); \
                if r.lane(0 as u32) != 0.0f32 { return 1; } \
                return 0; \
            }",
            "E0308",
        );
    }

    #[test]
    fn simd_load_store_clean_under_unsafe() {
        assert_clean(
            "extern fn malloc(n: usize) -> *u8; \
             extern fn free(p: *u8); \
             fn main() -> i32 { \
                let buf: *u8 = unsafe { malloc(16 as usize) }; \
                let fp: *f32 = unsafe { buf as *f32 }; \
                let v: f32x4 = f32x4::splat(1.0f32); \
                unsafe { v.store(fp); } \
                let r: f32x4 = unsafe { f32x4::load(fp) }; \
                unsafe { free(buf); } \
                if r.lane(0 as u32) != 0.0f32 { return 1; } \
                return 0; \
             }",
        );
    }

    #[test]
    fn simd_load_outside_unsafe_e0801() {
        let codes = errors(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                let buf: *u8 = unsafe { malloc(16 as usize) }; \
                let fp: *f32 = unsafe { buf as *f32 }; \
                let r: f32x4 = f32x4::load(fp); \
                if r.lane(0 as u32) != 0.0f32 { return 1; } \
                return 0; \
             }",
        );
        assert!(codes.contains(&"E0801"), "expected E0801, got {:?}", codes);
    }

    #[test]
    fn simd_i64x2_and_u32x4_resolve_and_arithmetic_clean() {
        assert_clean(
            "fn main() -> i32 { \
                let a: i64x2 = i64x2::new(1 as i64, 2 as i64); \
                let b: i64x2 = a.add(i64x2::splat(10 as i64)).abs(); \
                let c: u32x4 = u32x4::new(1 as u32, 2 as u32, 3 as u32, 4 as u32); \
                let d: u32x4 = c.mul(u32x4::splat(2 as u32)).min(c); \
                if b.lane(0 as u32) != (0 as i64) { return 1; } \
                if d.lane(0 as u32) != (0 as u32) { return 2; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_u64x2_resolves_and_unsigned_methods_clean() {
        // v0.0.7 Slice 2.2 audit fix: `u64x2` was the missing 128-bit
        // 8-byte-lane width (only `i64x2` shipped in 1B). Exercises
        // the full method matrix the umin/umax intrinsic declarations
        // back, plus the methods that lower to native LLVM ops.
        assert_clean(
            "fn main() -> i32 { \
                let a: u64x2 = u64x2::new(1 as u64, 2 as u64); \
                let b: u64x2 = u64x2::splat(10 as u64); \
                let c: u64x2 = a.add(b).sub(a).mul(b).div(u64x2::splat(2 as u64)); \
                let d: u64x2 = c.min(b).max(a); \
                let e: u64x2 = d.and(u64x2::splat(0xFF as u64)).or(a).xor(b).not(); \
                let f: u64x2 = e.shl(1 as u32).shr(2 as u32); \
                let lane: u64 = f.lane(0 as u32); \
                let with: u64x2 = f.with_lane(1 as u32, 99 as u64); \
                let arr: [u64; 2] = with.to_array(); \
                let from: u64x2 = u64x2::from_array(arr); \
                if lane != (0 as u64) { return 1; } \
                if from.lane(1 as u32) != (0 as u64) { return 2; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_u64x2_abs_rejected_e0324() {
        // `abs` is gated to float + signed-int SIMD; unsigned widths
        // (u64x2 included) reject it with E0324.
        assert_only_code(
            "fn main() -> i32 { \
                let v: u64x2 = u64x2::splat(1 as u64); \
                let r: u64x2 = v.abs(); \
                if r.lane(0 as u32) != (0 as u64) { return 1; } \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_bitwise_and_shifts_clean_on_int() {
        assert_clean(
            "fn main() -> i32 { \
                let a: i32x4 = i32x4::splat(0xFF); \
                let b: i32x4 = a.and(i32x4::splat(0x0F)); \
                let c: i32x4 = a.or(b).xor(a).not(); \
                let d: i32x4 = c.shl(2 as u32).shr(1 as u32); \
                if d.lane(0 as u32) != 0 { return 1; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_bitwise_rejected_on_float_e0324() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: f32x4 = f32x4::splat(1.0f32); \
                let r: f32x4 = v.and(v); \
                if r.lane(0 as u32) != 0.0f32 { return 1; } \
                return 0; \
            }",
            "E0324",
        );
    }

    #[test]
    fn simd_shift_non_literal_e0873() {
        let codes = errors(
            "fn main() -> i32 { \
                let v: i32x4 = i32x4::splat(1); \
                let mut n: u32 = 2 as u32; \
                let r: i32x4 = v.shl(n); \
                if r.lane(0 as u32) != 0 { return 1; } \
                return 0; \
            }",
        );
        assert!(codes.contains(&"E0873"), "expected E0873, got {:?}", codes);
    }

    #[test]
    fn simd_shift_out_of_range_e0874() {
        assert_only_code(
            "fn main() -> i32 { \
                let v: i32x4 = i32x4::splat(1); \
                let r: i32x4 = v.shl(64 as u32); \
                if r.lane(0 as u32) != 0 { return 1; } \
                return 0; \
            }",
            "E0874",
        );
    }

    #[test]
    fn simd_byte_short_widths_clean() {
        // i8x16 / i16x8 / u8x16 / u16x8 all resolve and admit the full
        // int-SIMD method matrix (arithmetic, bitwise, shifts, min/max,
        // abs on signed widths).
        assert_clean(
            "fn main() -> i32 { \
                let a: u8x16 = u8x16::splat(10 as u8); \
                let b: u8x16 = a.add(u8x16::splat(5 as u8)).and(u8x16::splat(0x0F as u8)); \
                let c: i8x16 = i8x16::splat(-3 as i8).abs().min(i8x16::splat(100 as i8)); \
                let d: u16x8 = u16x8::splat(0x00FF as u16).shl(4 as u32); \
                let e: i16x8 = i16x8::splat(-5 as i16).abs().max(i16x8::splat(0 as i16)); \
                if b.lane(0 as u32) != (0 as u8) { return 1; } \
                if c.lane(0 as u32) != (0 as i8) { return 2; } \
                if d.lane(0 as u32) != (0 as u16) { return 3; } \
                if e.lane(0 as u32) != (0 as i16) { return 4; } \
                return 0; \
            }",
        );
    }

    #[test]
    fn simd_store_outside_unsafe_e0801() {
        let codes = errors(
            "extern fn malloc(n: usize) -> *u8; \
             fn main() -> i32 { \
                let buf: *u8 = unsafe { malloc(16 as usize) }; \
                let fp: *f32 = unsafe { buf as *f32 }; \
                let v: f32x4 = f32x4::splat(1.0f32); \
                v.store(fp); \
                return 0; \
             }",
        );
        assert!(codes.contains(&"E0801"), "expected E0801, got {:?}", codes);
    }

    // ---- v0.0.9 Phase 4: module-scope `const` and `static` ----

    /// Run the lower pass before sema. Required for const-substitution
    /// tests (the literal-only check + const-ref → literal rewrite both
    /// live in `crate::lower`).
    fn check_src_lowered(src: &str) -> Vec<Diagnostic> {
        let toks = tokenize(src).expect("lex");
        let mut prog = parse(toks).expect("parse");
        let path = PathBuf::from("test.cplus");
        let mut diags = crate::lower::lower(&mut prog, &path, src);
        diags.extend(check(&prog, path, src));
        diags
    }

    fn lowered_errors(src: &str) -> Vec<String> {
        check_src_lowered(src)
            .into_iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| d.code.0.to_string())
            .collect()
    }

    #[test]
    fn const_int_initializer_typecheck_clean() {
        let diags = check_src_lowered(
            "const HEADER_BYTES: usize = 176; \
             fn main() -> i32 { let n: usize = HEADER_BYTES; return 0; }",
        );
        assert!(
            diags.is_empty(),
            "expected clean type-check, got {:#?}",
            diags
        );
    }

    #[test]
    fn const_substituted_into_arithmetic() {
        // After lowering, HEADER_BYTES becomes the literal 176 at the
        // use site, which is u64-compatible and adds fine with another
        // usize value.
        let diags = check_src_lowered(
            "const HEADER_BYTES: usize = 176; \
             fn main() -> i32 { let n: usize = HEADER_BYTES + (8 as usize); return 0; }",
        );
        assert!(diags.is_empty(), "got {:#?}", diags);
    }

    #[test]
    fn const_initializer_type_mismatch_e0302() {
        // Initializer is an int literal but declared type is bool —
        // sema's check_expr surfaces E0302 (or the literal/type
        // mismatch code) from the standard literal-check pipeline.
        let codes = lowered_errors("const FOO: bool = 5;");
        assert!(
            codes.iter().any(|c| c == "E0302" || c == "E0303"),
            "expected E0302/E0303 type mismatch, got {:?}",
            codes
        );
    }

    #[test]
    fn const_with_non_literal_initializer_e0x30() {
        // Arithmetic in the initializer is rejected by lower with E0X30.
        let codes = lowered_errors("const FOO: i32 = 1 + 2;");
        assert!(codes.iter().any(|c| c == "E0X30"), "got {:?}", codes);
    }

    #[test]
    fn const_with_ident_initializer_e0x30() {
        // Referring to another binding/const from an initializer is
        // out of scope for v0.0.9.
        let codes = lowered_errors(
            "const A: i32 = 5; \
             const B: i32 = A;",
        );
        assert!(codes.iter().any(|c| c == "E0X30"), "got {:?}", codes);
    }

    // v0.0.12 G-033: array literals + fill literals still rejected in
    // static-init position. `#zero::[T]()` is the only non-scalar shape
    // accepted (option a from llama.cplus G-032 ranking).
    // v0.0.12 G-043 (llama.cplus): array literal / fill IS now accepted as a
    // `static` initializer (supersedes the G-033-era rejection). Codegen emits
    // an LLVM constant aggregate; see `render_static_literal`.
    #[test]
    fn array_literal_in_static_accepted_g043() {
        let codes = lowered_errors("pub static T: [i32; 4] = [1, 2, 3, 4];");
        assert!(!codes.iter().any(|c| c == "E0X30"), "expected no E0X30, got {:?}", codes);
    }

    #[test]
    fn fill_array_in_static_accepted_g043() {
        let codes = lowered_errors("pub static T: [u8; 256] = [0u8; 256];");
        assert!(!codes.iter().any(|c| c == "E0X30"), "expected no E0X30, got {:?}", codes);
    }

    // G-043 keeps `const` literal-only: an array literal on a `const` is still
    // E0X30 (consts inline at use sites; arrays belong in `static`).
    #[test]
    fn array_literal_in_const_still_rejected_e0x30_g043() {
        let codes = lowered_errors("pub const C: [i32; 4] = [1, 2, 3, 4];");
        assert!(codes.iter().any(|c| c == "E0X30"), "expected E0X30, got {:?}", codes);
    }

    // v0.0.13 G-043 (second half): a non-generic struct literal whose fields are
    // themselves static initializers is accepted in `static` position.
    #[test]
    fn struct_literal_in_static_accepted_g043b() {
        let codes = lowered_errors(
            "struct P { x: i32, y: f32, ok: bool } \
             pub static S: P = P { x: 1, y: 2.0f32, ok: true };",
        );
        assert!(!codes.iter().any(|c| c == "E0X30"), "expected no E0X30, got {:?}", codes);
    }

    // Struct-of-struct and array-of-struct compose recursively.
    #[test]
    fn nested_struct_literal_in_static_accepted_g043b() {
        let codes = lowered_errors(
            "struct Inner { a: i32 } \
             struct Outer { i: Inner, n: i32 } \
             pub static O: Outer = Outer { i: Inner { a: 5 }, n: 6 }; \
             pub static A: [Outer; 2] = [ \
                 Outer { i: Inner { a: 1 }, n: 2 }, \
                 Outer { i: Inner { a: 3 }, n: 4 } \
             ];",
        );
        assert!(!codes.iter().any(|c| c == "E0X30"), "expected no E0X30, got {:?}", codes);
    }

    // A struct literal with a non-literal field value is still E0X30.
    #[test]
    fn struct_literal_with_call_field_rejected_e0x30_g043b() {
        let codes = lowered_errors(
            "struct P { x: i32 } \
             fn f() -> i32 { return 3; } \
             pub static S: P = P { x: f() };",
        );
        assert!(codes.iter().any(|c| c == "E0X30"), "expected E0X30, got {:?}", codes);
    }

    // The generic struct-literal form is excluded (it reaches codegen
    // un-monomorphized in static position); it stays E0X30.
    #[test]
    fn generic_struct_literal_in_static_rejected_e0x30_g043b() {
        let codes = lowered_errors(
            "struct Pair[A, B] { first: A, second: B } \
             pub static G: Pair[i32, bool] = Pair[i32, bool] { first: 1, second: true };",
        );
        assert!(codes.iter().any(|c| c == "E0X30"), "expected E0X30, got {:?}", codes);
    }

    // `const` stays literal-only: a struct literal on a `const` is E0X30.
    #[test]
    fn struct_literal_in_const_still_rejected_e0x30_g043b() {
        let codes = lowered_errors(
            "struct P { x: i32 } pub const C: P = P { x: 1 };",
        );
        assert!(codes.iter().any(|c| c == "E0X30"), "expected E0X30, got {:?}", codes);
    }

    #[test]
    fn zero_intrinsic_in_static_accepted_g033() {
        // The complementary positive — lower accepts #zero::[T]() as
        // a const-init shape; sema then type-checks the RHS against
        // the declared type.
        let codes = lowered_errors("pub static T: [u8; 256] = #zero::[[u8; 256]]();");
        assert!(
            !codes.iter().any(|c| c == "E0X30"),
            "expected no E0X30, got {:?}",
            codes
        );
    }

    #[test]
    fn const_negative_int_initializer_clean() {
        let diags = check_src_lowered(
            "const NEG_ONE: i32 = -1; \
             fn main() -> i32 { return NEG_ONE; }",
        );
        assert!(diags.is_empty(), "got {:#?}", diags);
    }

    #[test]
    fn static_int_decl_clean() {
        let diags = check_src_lowered(
            "static RNG_SEED: u32 = 305419896; \
             fn main() -> i32 { let s: u32 = RNG_SEED; return 0; }",
        );
        assert!(diags.is_empty(), "got {:#?}", diags);
    }

    #[test]
    fn static_mut_read_outside_unsafe_e0x33() {
        let codes = lowered_errors(
            "static mut COUNTER: i32 = 0; \
             fn main() -> i32 { return COUNTER; }",
        );
        assert!(codes.iter().any(|c| c == "E0X33"), "got {:?}", codes);
    }

    #[test]
    fn static_mut_read_inside_unsafe_clean() {
        let diags = check_src_lowered(
            "static mut COUNTER: i32 = 0; \
             fn main() -> i32 { let n: i32 = unsafe { COUNTER }; return n; }",
        );
        assert!(diags.is_empty(), "got {:#?}", diags);
    }

    #[test]
    fn static_mut_write_outside_unsafe_e0x34() {
        let codes = lowered_errors(
            "static mut COUNTER: i32 = 0; \
             fn main() -> i32 { COUNTER = 5; return 0; }",
        );
        assert!(codes.iter().any(|c| c == "E0X34"), "got {:?}", codes);
    }

    #[test]
    fn static_mut_write_inside_unsafe_clean() {
        let diags = check_src_lowered(
            "static mut COUNTER: i32 = 0; \
             fn main() -> i32 { unsafe { COUNTER = 5; } return 0; }",
        );
        assert!(diags.is_empty(), "got {:#?}", diags);
    }

    #[test]
    fn write_to_immutable_static_e0305() {
        let codes = lowered_errors(
            "static FROZEN: i32 = 0; \
             fn main() -> i32 { FROZEN = 5; return 0; }",
        );
        assert!(codes.iter().any(|c| c == "E0305"), "got {:?}", codes);
    }

    #[test]
    fn const_name_collides_with_fn_e0301() {
        let codes = lowered_errors(
            "fn FOO() -> i32 { return 1; } \
             const FOO: i32 = 1;",
        );
        assert!(codes.iter().any(|c| c == "E0301"), "got {:?}", codes);
    }

    #[test]
    fn static_name_collides_with_const_e0301() {
        let codes = lowered_errors(
            "const FOO: i32 = 1; \
             static FOO: i32 = 1;",
        );
        // Note: const FOO collides with static FOO because both go
        // through the same name-uniqueness check at sema's collect
        // pass — but const items aren't in the statics_table, so the
        // collision is detected when the second item is processed.
        // Lower also picks up duplicate-const in `consts` HashMap
        // silently overwriting, which is fine — the sema check fires
        // first.
        assert!(codes.iter().any(|c| c == "E0301"), "got {:?}", codes);
    }

    #[test]
    fn static_initializer_type_mismatch() {
        let codes = lowered_errors("static FOO: bool = 5;");
        assert!(
            codes.iter().any(|c| c == "E0302" || c == "E0303"),
            "expected type mismatch, got {:?}",
            codes
        );
    }

    // ========================================================================
    // v0.0.10 Phase 1: `#[no_alloc]` attribute
    // ========================================================================

    #[test]
    fn no_alloc_pure_arith_clean() {
        assert_clean(
            "#[no_alloc] fn pure_arith(x: i32) -> i32 { return x + 1; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_direct_malloc_call_e0901() {
        let codes = errors(
            "extern fn malloc(n: usize) -> *u8;\n\
             #[no_alloc] fn f() { unsafe { malloc(8 as usize); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_direct_free_call_e0901() {
        let codes = errors(
            "extern fn free(p: *u8);\n\
             #[no_alloc] fn f(p: *u8) { unsafe { free(p); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_realloc_via_link_name_e0901() {
        // `#[link_name = "realloc"]` triggers the blocklist even when the
        // source-level name is something else.
        let codes = errors(
            "#[link_name = \"realloc\"] extern fn grow(p: *u8, n: usize) -> *u8;\n\
             #[no_alloc] fn f(p: *u8) { unsafe { grow(p, 16 as usize); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_calls_other_no_alloc_clean() {
        assert_clean(
            "#[no_alloc] fn a(x: i32) -> i32 { return b(x); }\n\
             #[no_alloc] fn b(x: i32) -> i32 { return x +% 1; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_calls_unmarked_user_fn_e0901() {
        let codes = errors(
            "fn helper(x: i32) -> i32 { return x +% 1; }\n\
             #[no_alloc] fn caller(x: i32) -> i32 { return helper(x); }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_calls_leaf_whitelist_clean() {
        // `memcpy` is on the leaf whitelist → fine to call from #[no_alloc].
        assert_clean(
            "extern fn memcpy(dest: *u8, src: *u8, n: usize) -> *u8;\n\
             #[no_alloc] fn copy(dest: *u8, src: *u8, n: usize) {\n\
                 unsafe { memcpy(dest, src, n); }\n\
                 return;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_unknown_extern_e0901() {
        // Conservative: extern not in the whitelist and not marked
        // `#[no_alloc]` is rejected. User can opt in via `#[no_alloc]`
        // on the extern decl.
        let codes = errors(
            "extern fn mystery(x: i32) -> i32;\n\
             #[no_alloc] fn caller(x: i32) -> i32 { return unsafe { mystery(x) }; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_extern_self_marked_clean() {
        // The user vouches for the extern by marking it `#[no_alloc]`.
        assert_clean(
            "#[no_alloc] extern fn vouch(x: i32) -> i32;\n\
             #[no_alloc] fn caller(x: i32) -> i32 { return unsafe { vouch(x) }; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_method_marked_clean() {
        // A `#[no_alloc]` method body that only calls `#[no_alloc]` helpers.
        assert_clean(
            "struct P { x: i32 }\n\
             impl P {\n\
                 #[no_alloc] fn doubled(self) -> i32 { return self.x +% self.x; }\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_extern_decl_clean() {
        // Carrying `#[no_alloc]` on an extern declaration is the user's
        // promise — no body to walk, no diagnostic.
        assert_clean(
            "#[no_alloc] extern fn pure(x: i32) -> i32;\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_string_interpolation_e0901() {
        // `"...${x}..."` lowers to a `__string_concat` malloc — rejected.
        let codes = errors(
            "#[no_alloc] fn f(x: i32) -> i32 { let s = \"v=${x}\"; return x; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_plain_string_literal_clean() {
        // A non-interpolated string literal is a static constant — no alloc.
        assert_clean(
            "#[no_alloc] fn f() -> str { return \"static\"; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_block_string_interpolation_clean() {
        // Interpolation allocates but does not block — fine under #[no_block].
        assert_clean(
            "#[no_block] fn f(x: i32) -> i32 { let s = \"v=${x}\"; return x; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    // ---- v0.0.14: #[no_alloc] drop-glue (implicit scope-exit destructors) ----

    #[test]
    fn no_alloc_string_local_drops_at_scope_exit_e0901() {
        // A `string` local frees its buffer at scope exit — invisible in the
        // body, but the drop glue deallocates. (`malloc`/`to_string` is not
        // even called here; the local is built from a static literal slice.)
        let codes = errors(
            "#[no_alloc] fn f(s: str) -> i32 {\n\
                 let owned: string = s.to_string();\n\
                 return 0;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_local_with_freeing_drop_e0901() {
        // A struct whose `drop` frees (and is not itself #[no_alloc]): the
        // scope-exit teardown deallocates. Constructed from a passed-in raw
        // pointer, so no allocating *call* appears in the body — the drop glue
        // is the sole violation.
        let codes = errors(
            "extern fn free(p: *u8);\n\
             struct Handle { opaque p: *u8 }\n\
             impl Handle { fn drop(mut self) { unsafe { free(self.p); }; } }\n\
             #[no_alloc] fn f(raw: *u8) -> i32 {\n\
                 let h: Handle = Handle { p: raw };\n\
                 return 0;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_local_with_no_alloc_drop_clean() {
        // A `#[no_alloc]` drop is verified non-allocating, so running it at
        // scope exit is allowed.
        assert_clean(
            "struct Tracker { opaque p: *u8 }\n\
             impl Tracker { #[no_alloc] fn drop(mut self) { return; } }\n\
             #[no_alloc] fn f(raw: *u8) -> i32 {\n\
                 let t: Tracker = Tracker { p: raw };\n\
                 return 0;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_field_carries_freeing_drop_e0901() {
        // The rule reaches through fields: a struct holding a `string` field
        // auto-drops that field (freeing) at scope exit.
        let codes = errors(
            "struct Wrap { name: string, tag: i32 }\n\
             #[no_alloc] fn f(s: str) -> i32 {\n\
                 let w: Wrap = Wrap { name: s.to_string(), tag: 1 };\n\
                 return 0;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_non_drop_local_clean() {
        // A plain non-drop local (Copy / no owning fields) has no scope-exit
        // teardown — allowed.
        assert_clean(
            "struct Pt { x: i32, y: i32 }\n\
             #[no_alloc] fn f() -> i32 {\n\
                 let p: Pt = Pt { x: 1, y: 2 };\n\
                 return p.x;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_opaque_ptr_local_without_drop_clean() {
        // A raw-pointer-holding struct with NO `drop` (the pointer is `opaque`,
        // freed elsewhere) has no scope-exit teardown — allowed.
        assert_clean(
            "struct View { opaque p: *u8 }\n\
             #[no_alloc] fn f(raw: *u8) -> i32 {\n\
                 let v: View = View { p: raw };\n\
                 return 0;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    // ---- v0.0.15: #[no_alloc] drop-glue — owned parameters + temporaries ----

    #[test]
    fn no_alloc_move_param_freeing_drop_e0901() {
        // A `move` parameter is the callee's to tear down. A freeing `drop`
        // therefore deallocates at scope exit — invisible in the body.
        let codes = errors(
            "extern fn free(p: *u8);\n\
             struct Handle { opaque p: *u8 }\n\
             impl Handle { fn drop(mut self) { unsafe { free(self.p); }; } }\n\
             #[no_alloc] fn f(move h: Handle) -> i32 { return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_bare_nonpcopy_param_carries_freeing_drop_e0901() {
        // A bare `w: Wrap` non-Copy struct param is move-by-default (v0.0.10),
        // so the callee drops it — and the auto-dropped `string` field frees.
        let codes = errors(
            "struct Wrap { name: string, tag: i32 }\n\
             #[no_alloc] fn f(w: Wrap) -> i32 { return w.tag; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_borrow_param_drop_type_clean() {
        // `borrow` is a shared by-value parameter: the caller keeps ownership,
        // so there's no callee-side teardown to allocate.
        assert_clean(
            "struct Wrap { name: string, tag: i32 }\n\
             #[no_alloc] fn f(borrow w: Wrap) -> i32 { return w.tag; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_mut_param_drop_type_clean() {
        // `mut` is an exclusive borrow (pointer-passed): caller-owned, no
        // callee drop.
        assert_clean(
            "struct Wrap { name: string, tag: i32 }\n\
             #[no_alloc] fn f(mut w: Wrap) -> i32 { return w.tag; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_move_param_no_alloc_drop_clean() {
        // A `move` param whose `drop` is itself `#[no_alloc]` is verified
        // non-allocating, so running it at scope exit is allowed.
        assert_clean(
            "struct Tracker { opaque p: *u8 }\n\
             impl Tracker { #[no_alloc] fn drop(mut self) { return; } }\n\
             #[no_alloc] fn f(move t: Tracker) -> i32 { return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_alloc_move_self_freeing_drop_e0901() {
        // A `move self` receiver is consumed by the method, so codegen drops it
        // at scope exit. A freeing `drop` deallocates there.
        let codes = errors(
            "extern fn free(p: *u8);\n\
             struct Handle { opaque p: *u8 }\n\
             impl Handle {\n\
                 fn drop(mut self) { unsafe { free(self.p); }; }\n\
                 #[no_alloc] fn consume(move self) -> i32 { return 0; }\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_discarded_drop_temporary_e0901() {
        // A discarded struct-literal temporary carrying a freeing `drop` is torn
        // down at statement end. The literal itself allocates nothing — the drop
        // glue is the sole violation.
        let codes = errors(
            "extern fn free(p: *u8);\n\
             struct Handle { opaque p: *u8 }\n\
             impl Handle { fn drop(mut self) { unsafe { free(self.p); }; } }\n\
             #[no_alloc] fn f(raw: *u8) -> i32 { Handle { p: raw }; return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn no_alloc_discarded_non_drop_temporary_clean() {
        // A discarded temporary with no destructor has no teardown — allowed.
        // (Exercises the `no_alloc_safe_drop` gate on the temporary arm.)
        assert_clean(
            "struct Pt { x: i32, y: i32 }\n\
             #[no_alloc] fn f() -> i32 { Pt { x: 1, y: 2 }; return 0; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn realtime_string_interpolation_e0901() {
        // The bundle includes no_alloc, so interpolation is rejected.
        let codes = errors(
            "#[realtime] fn f(x: i32) -> i32 { let s = \"v=${x}\"; return x; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    // Wrong-target rejection for `#[no_alloc]` lives in attrs.rs's test
    // module — sema's `check()` does not invoke the attrs pass.

    // ========================================================================
    // v0.0.10 Phase 3: `#[bounded_recursion]` attribute
    // ========================================================================

    #[test]
    fn bounded_recursion_non_recursive_clean() {
        assert_clean(
            "#[bounded_recursion] fn a(x: i32) -> i32 { return x +% 1; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn bounded_recursion_self_recursive_e0906() {
        let codes = errors(
            "#[bounded_recursion] fn r(x: i32) -> i32 {\n\
                 if x == 0 { return 0; }\n\
                 return r(x -% 1);\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0906"), "got {:?}", codes);
    }

    // ========================================================================
    // v0.0.12 realtime Phase 3: `#[no_block]` attribute
    // ========================================================================

    #[test]
    fn no_block_pure_arith_clean() {
        assert_clean(
            "#[no_block] fn pure_arith(x: i32) -> i32 { return x +% 1; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_block_direct_mutex_lock_e0907() {
        let codes = errors(
            "extern fn pthread_mutex_lock(m: *u8) -> i32;\n\
             #[no_block] fn f(m: *u8) { unsafe { pthread_mutex_lock(m); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0907"), "got {:?}", codes);
    }

    #[test]
    fn no_block_direct_sleep_e0907() {
        let codes = errors(
            "extern fn sleep(secs: u32) -> u32;\n\
             #[no_block] fn f() { unsafe { sleep(1); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0907"), "got {:?}", codes);
    }

    #[test]
    fn no_block_blocking_read_e0907() {
        // `read` is a blocking syscall — rejected even though it never heap-allocs.
        let codes = errors(
            "extern fn read(fd: i32, buf: *u8, n: usize) -> isize;\n\
             #[no_block] fn f(fd: i32, buf: *u8) { unsafe { read(fd, buf, 8 as usize); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0907"), "got {:?}", codes);
    }

    #[test]
    fn no_block_cond_wait_via_link_name_e0907() {
        // `#[link_name]` resolves to a blocklisted symbol even when the
        // source name is something else.
        let codes = errors(
            "#[link_name = \"pthread_cond_wait\"] extern fn park(c: *u8, m: *u8) -> i32;\n\
             #[no_block] fn f(c: *u8, m: *u8) { unsafe { park(c, m); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0907"), "got {:?}", codes);
    }

    #[test]
    fn no_block_safe_leaf_math_clean() {
        // `sqrtf` is a pure leaf — fine to call from #[no_block].
        assert_clean(
            "extern fn sqrtf(x: f32) -> f32;\n\
             #[no_block] fn root(x: f32) -> f32 { return unsafe { sqrtf(x) }; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_block_trylock_clean() {
        // The nonblocking `pthread_mutex_trylock` is on the safe-leaf set.
        assert_clean(
            "extern fn pthread_mutex_trylock(m: *u8) -> i32;\n\
             #[no_block] fn f(m: *u8) -> i32 { return unsafe { pthread_mutex_trylock(m) }; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_block_calls_other_no_block_clean() {
        assert_clean(
            "#[no_block] fn a(x: i32) -> i32 { return b(x); }\n\
             #[no_block] fn b(x: i32) -> i32 { return x +% 1; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_block_calls_unmarked_user_fn_e0907() {
        let codes = errors(
            "fn helper(x: i32) -> i32 { return x +% 1; }\n\
             #[no_block] fn caller(x: i32) -> i32 { return helper(x); }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0907"), "got {:?}", codes);
    }

    #[test]
    fn no_block_unknown_extern_e0907() {
        let codes = errors(
            "extern fn mystery(x: i32) -> i32;\n\
             #[no_block] fn caller(x: i32) -> i32 { return unsafe { mystery(x) }; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0907"), "got {:?}", codes);
    }

    #[test]
    fn no_block_extern_self_marked_clean() {
        // The user vouches for the extern by marking it `#[no_block]`.
        assert_clean(
            "#[no_block] extern fn vouch(x: i32) -> i32;\n\
             #[no_block] fn caller(x: i32) -> i32 { return unsafe { vouch(x) }; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_block_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P {\n\
                 #[no_block] fn doubled(self) -> i32 { return self.x +% self.x; }\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn no_block_cpu_relax_intrinsic_clean() {
        // `#cpu_relax()` is an intrinsic, not a call — the allowed spin hint.
        assert_clean(
            "#[no_block] fn spin() { #cpu_relax(); return; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    // ========================================================================
    // v0.0.12 realtime Phase 4: `#[realtime]` bundle attribute
    // ========================================================================

    #[test]
    fn realtime_pure_arith_clean() {
        assert_clean(
            "#[realtime] fn process(x: i32) -> i32 { return x +% 1; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn realtime_alloc_violation_e0901() {
        let codes = errors(
            "extern fn malloc(n: usize) -> *u8;\n\
             #[realtime] fn f() { unsafe { malloc(8 as usize); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0901"), "got {:?}", codes);
    }

    #[test]
    fn realtime_block_violation_e0907() {
        let codes = errors(
            "extern fn pthread_mutex_lock(m: *u8) -> i32;\n\
             #[realtime] fn f(m: *u8) { unsafe { pthread_mutex_lock(m); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0907"), "got {:?}", codes);
    }

    #[test]
    fn realtime_recursion_violation_e0906() {
        let codes = errors(
            "#[realtime] fn r(x: i32) -> i32 {\n\
                 if x == 0 { return 0; }\n\
                 return r(x -% 1);\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0906"), "got {:?}", codes);
    }

    #[test]
    fn realtime_satisfies_no_alloc_callee_clean() {
        // A `#[realtime]` callee satisfies a `#[no_alloc]` caller's requirement.
        assert_clean(
            "#[realtime] fn leaf(x: i32) -> i32 { return x +% 1; }\n\
             #[no_alloc] fn caller(x: i32) -> i32 { return leaf(x); }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn realtime_satisfies_no_block_callee_clean() {
        assert_clean(
            "#[realtime] fn leaf(x: i32) -> i32 { return x +% 1; }\n\
             #[no_block] fn caller(x: i32) -> i32 { return leaf(x); }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    // ========================================================================
    // v0.0.12 realtime Phase 4: `#[max_stack(N)]` bounded-stack estimate
    // ========================================================================

    #[test]
    fn max_stack_small_frame_clean() {
        // 100-byte array is under the 256-byte budget.
        assert_clean(
            "#[max_stack(256)] fn f() { let buf: [u8; 100] = [0u8; 100]; return; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn max_stack_large_array_over_budget_e0908() {
        let codes = errors(
            "#[max_stack(64)] fn f() { let buf: [u8; 100] = [0u8; 100]; return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0908"), "got {:?}", codes);
    }

    #[test]
    fn max_stack_params_counted_clean_at_boundary() {
        // Two i64 params = 16 bytes; budget 16; `>` not `>=`, so clean.
        assert_clean(
            "#[max_stack(16)] fn f(a: i64, b: i64) -> i64 { return a +% b; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn max_stack_params_over_budget_e0908() {
        let codes = errors(
            "#[max_stack(8)] fn f(a: i64, b: i64) -> i64 { return a +% b; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0908"), "got {:?}", codes);
    }

    #[test]
    fn max_stack_by_value_struct_over_budget_e0908() {
        // A 1000-byte by-value aggregate local blows a 128-byte budget.
        let codes = errors(
            "struct Big { data: [u8; 1000] }\n\
             #[max_stack(128)] fn f() { let b: Big = Big { data: [0u8; 1000] }; return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0908"), "got {:?}", codes);
    }

    #[test]
    fn max_stack_nested_block_locals_counted_e0908() {
        // Locals inside nested blocks are summed (conservative: all live).
        let codes = errors(
            "#[max_stack(64)] fn f(flag: bool) {\n\
                 if flag { let buf: [u8; 100] = [0u8; 100]; }\n\
                 return;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0908"), "got {:?}", codes);
    }

    #[test]
    fn max_stack_method_clean() {
        assert_clean(
            "struct P { x: i32 }\n\
             impl P {\n\
                 #[max_stack(64)] fn small(self) -> i32 { let t: i32 = self.x; return t; }\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    // ========================================================================
    // v0.0.10 Phase 4: `#name(...)` compiler-intrinsic syntax
    // ========================================================================

    #[test]
    fn intrinsic_selector_string_literal_clean() {
        assert_clean(
            "fn main() -> i32 {\n\
                 let s: *u8 = #selector(\"length\");\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn intrinsic_unknown_name_e0905() {
        let codes = errors(
            "fn main() -> i32 {\n\
                 let p: *u8 = #frobnicate(\"x\");\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0905"), "got {:?}", codes);
    }

    // v0.0.16: FFI/raw builtins are `#name(...)` intrinsics. A bare call is a
    // migration error; the `#` form type-checks.
    #[test]
    fn ffi_builtin_bare_call_is_migration_error_e0905() {
        let codes = errors("fn main() -> i32 { let p: *u8 = str_ptr(\"x\"); return 0; }");
        assert!(codes.iter().any(|c| *c == "E0905"), "got {:?}", codes);
    }

    #[test]
    fn ffi_builtin_hash_form_clean() {
        assert_clean("fn main() -> i32 { let p: *u8 = #str_ptr(\"x\"); return 0; }");
    }

    #[test]
    fn bare_println_is_migration_error_e0905() {
        let codes = errors("fn main() -> i32 { println(\"x\"); return 0; }");
        assert!(codes.iter().any(|c| *c == "E0905"), "got {:?}", codes);
    }

    #[test]
    fn hash_println_clean() {
        assert_clean("fn main() -> i32 { #println(\"x\"); return 0; }");
    }

    #[test]
    fn intrinsic_selector_non_string_e0903() {
        let codes = errors(
            "fn main() -> i32 {\n\
                 let n: i32 = 42;\n\
                 let p: *u8 = #selector(n);\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0903"), "got {:?}", codes);
    }

    #[test]
    fn intrinsic_msg_send_outside_unsafe_e0801() {
        let codes = errors(
            "fn main() -> i32 {\n\
                 let obj: *u8 = unsafe { 0 as *u8 };\n\
                 #msg_send(obj, \"hello\");\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0801"), "got {:?}", codes);
    }

    #[test]
    fn intrinsic_msg_send_inside_unsafe_clean() {
        assert_clean(
            "fn main() -> i32 {\n\
                 unsafe {\n\
                     let obj: *u8 = 0 as *u8;\n\
                     #msg_send(obj, \"hello\");\n\
                 }\n\
                 return 0;\n\
             }",
        );
    }

    #[test]
    fn intrinsic_msg_send_with_return_type_clean() {
        assert_clean(
            "fn main() -> i32 {\n\
                 let obj: *u8 = unsafe { 0 as *u8 };\n\
                 let n: u64 = unsafe { #msg_send(obj, \"length\") -> u64 };\n\
                 return n as i32;\n\
             }",
        );
    }

    #[test]
    fn intrinsic_asm_outside_unsafe_e0801() {
        let codes = errors(
            "fn main() -> i32 {\n\
                 #asm(\"nop\");\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0801"), "got {:?}", codes);
    }

    #[test]
    fn intrinsic_asm_inside_unsafe_clean() {
        assert_clean(
            "fn main() -> i32 {\n\
                 unsafe { #asm(\"dmb ish\"); }\n\
                 return 0;\n\
             }",
        );
    }

    // v0.0.14 inline asm Tier 2 changed `#asm`'s grammar: a non-string
    // template, a turbofish, a `-> T` ascription, and a non-operand second
    // argument are now rejected by the dedicated `parse_asm_intrinsic` grammar
    // (parse errors) rather than by sema. They are still rejected — these
    // tests pin that.
    fn parse_fails_src(src: &str) -> bool {
        match tokenize(src) {
            Ok(toks) => parse(toks).is_err(),
            Err(_) => true,
        }
    }

    #[test]
    fn asm_non_string_template_rejected() {
        assert!(parse_fails_src(
            "fn main() -> i32 {\n\
                 let x: i32 = 1;\n\
                 unsafe { #asm(x); }\n\
                 return 0;\n\
             }",
        ));
    }

    #[test]
    fn asm_type_args_rejected() {
        assert!(parse_fails_src(
            "fn main() -> i32 {\n\
                 unsafe { #asm::[i32](\"nop\"); }\n\
                 return 0;\n\
             }",
        ));
    }

    #[test]
    fn asm_ret_ascription_rejected() {
        assert!(parse_fails_src(
            "fn main() -> i32 {\n\
                 unsafe { let _x: i32 = #asm(\"nop\") -> i32; }\n\
                 return 0;\n\
             }",
        ));
    }

    #[test]
    fn asm_second_arg_must_be_operand_rejected() {
        assert!(parse_fails_src(
            "fn main() -> i32 {\n\
                 unsafe { #asm(\"a\", \"b\"); }\n\
                 return 0;\n\
             }",
        ));
    }

    // ---- v0.0.14 inline asm Tier 2: operands + clobbers ----

    #[test]
    fn asm_tier1_no_operands_still_clean() {
        assert_clean(
            "fn f() { unsafe { #asm(\"dmb ish\"); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn asm_tier2_in_out_clean() {
        assert_clean(
            "fn add(a: i64, b: i64) -> i64 {\n\
                 let mut s: i64 = 0;\n\
                 unsafe { #asm(\"add {s}, {a}, {b}\", s = out(reg) s, a = in(reg) a, b = in(reg) b); }\n\
                 return s;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn asm_tier2_inout_clean() {
        assert_clean(
            "fn inc(x: i64) -> i64 {\n\
                 let mut v: i64 = x;\n\
                 unsafe { #asm(\"add {v}, {v}, #1\", v = inout(reg) v); }\n\
                 return v;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn asm_tier2_explicit_reg_and_clobber_clean() {
        // Explicit-register operand needs no `{name}` placeholder.
        assert_clean(
            "fn getpid() -> i64 {\n\
                 let mut p: i64 = 0;\n\
                 unsafe { #asm(\"mov x16, #20\", p = out(\"x0\") p, clobber(\"x16\")); }\n\
                 return p;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn asm_tier2_outside_unsafe_e0801() {
        let codes = errors(
            "fn f(a: i64) -> i64 {\n\
                 let mut s: i64 = 0;\n\
                 #asm(\"mov {s}, {a}\", s = out(reg) s, a = in(reg) a);\n\
                 return s;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0801"), "got {:?}", codes);
    }

    #[test]
    fn asm_tier2_out_needs_mut_e0305() {
        let codes = errors(
            "fn f(a: i64) -> i64 {\n\
                 let s: i64 = 0;\n\
                 unsafe { #asm(\"mov {s}, {a}\", s = out(reg) s, a = in(reg) a); }\n\
                 return s;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0305"), "got {:?}", codes);
    }

    #[test]
    fn asm_tier2_non_scalar_operand_e0892() {
        let codes = errors(
            "fn f(a: string) { unsafe { #asm(\"nop {a}\", a = in(reg) a); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0892"), "got {:?}", codes);
    }

    #[test]
    fn asm_tier2_reg_missing_placeholder_e0893() {
        let codes = errors(
            "fn f(a: i64) { unsafe { #asm(\"nop\", a = in(reg) a); } return; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0893"), "got {:?}", codes);
    }

    #[test]
    fn asm_tier2_out_must_be_variable_e0895() {
        let codes = errors(
            "struct P { x: i64 }\n\
             fn f(mut p: P, a: i64) {\n\
                 unsafe { #asm(\"mov {o}, {a}\", o = out(reg) p.x, a = in(reg) a); }\n\
                 return;\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0895"), "got {:?}", codes);
    }

    // ---- v0.0.14 inline asm Tier 3: #[naked] ----

    #[test]
    fn naked_asm_only_body_clean() {
        assert_clean(
            "#[naked]\n\
             fn raw_add(a: i64, b: i64) -> i64 {\n\
                 unsafe { #asm(\"add x0, x0, x1\\nret\"); }\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
    }

    #[test]
    fn naked_non_asm_statement_e0909() {
        let codes = errors(
            "#[naked]\n\
             fn bad() -> i64 { let x: i64 = 1; return x; }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0909"), "got {:?}", codes);
    }

    #[test]
    fn naked_value_tail_e0909() {
        let codes = errors(
            "#[naked]\n\
             fn bad(a: i64) -> i64 { a }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0909"), "got {:?}", codes);
    }

    // ---- v0.0.14 graph value-depth: sema span->Ty retention ----

    fn value_types_of(src: &str) -> Vec<String> {
        let toks = tokenize(src).expect("lex");
        let prog = parse(toks).expect("parse");
        let (diags, mono) = check_multi_with_value_types(
            &prog,
            PathBuf::from("test.cplus"),
            src,
            std::collections::BTreeMap::new(),
        );
        assert!(diags.is_empty(), "sema errors: {diags:#?}");
        mono.value_types.into_iter().map(|(_, _, t)| t).collect()
    }

    #[test]
    fn value_types_records_inferred_nominal_and_primitive() {
        // `mk()` result is `Point`, `p.x` is `i32` — both inferred (not
        // annotated at the use site), so only value-depth retention sees them.
        let tys = value_types_of(
            "struct Point { x: i32, y: i32 }\n\
             fn mk() -> Point { return Point { x: 1, y: 2 }; }\n\
             fn main() -> i32 {\n\
                 let p: Point = mk();\n\
                 return p.x;\n\
             }",
        );
        assert!(tys.iter().any(|t| t == "Point"), "no Point spot: {tys:?}");
        assert!(tys.iter().any(|t| t == "i32"), "no i32 spot: {tys:?}");
    }

    #[test]
    fn value_types_renders_generic_instantiation() {
        // A generic struct literal renders with concrete args: `Box[i32]`.
        let tys = value_types_of(
            "struct Box[T] { v: T }\n\
             fn main() -> i32 {\n\
                 let b: Box[i32] = Box[i32] { v: 5 };\n\
                 return b.v;\n\
             }",
        );
        assert!(tys.iter().any(|t| t == "Box[i32]"), "no Box[i32] spot: {tys:?}");
    }

    #[test]
    fn value_types_empty_without_recording() {
        // The normal compile path does not record (zero overhead).
        let toks = tokenize("fn main() -> i32 { let x: i32 = 1; return x; }").expect("lex");
        let prog = parse(toks).expect("parse");
        let (_diags, mono) = check_multi_with_mono(
            &prog,
            PathBuf::from("test.cplus"),
            "fn main() -> i32 { let x: i32 = 1; return x; }",
            std::collections::BTreeMap::new(),
        );
        assert!(mono.value_types.is_empty(), "compile path should not record types");
    }

    #[test]
    fn intrinsic_compile_shader_bad_target_e0904() {
        let codes = errors(
            "fn main() -> i32 {\n\
                 let p: *u8 = #compile_shader(\"k.spv\", \"spirv\") as *u8;\n\
                 return 0;\n\
             }",
        );
        assert!(codes.iter().any(|c| *c == "E0904"), "got {:?}", codes);
    }

    #[test]
    fn bounded_recursion_mutual_recursive_e0906() {
        let codes = errors(
            "#[bounded_recursion] fn a(x: i32) -> i32 {\n\
                 if x == 0 { return 0; }\n\
                 return b(x -% 1);\n\
             }\n\
             fn b(x: i32) -> i32 {\n\
                 return a(x);\n\
             }\n\
             fn main() -> i32 { return 0; }",
        );
        assert!(codes.iter().any(|c| *c == "E0906"), "got {:?}", codes);
    }
}
