use crate::lexer::{NumSuffix, Span};

impl Span {
    pub fn merge(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// File-top `import "path" as name;` declarations. Always parsed before
    /// any items; an `import` appearing later in the file is a parse error.
    /// Resolution is the driver's job (Phase 4 slice 4A).
    pub imports: Vec<ImportDecl>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    /// Raw path string from the source, e.g. `"util/strings.cplus"`. The
    /// driver resolves it relative to the importing file's directory.
    pub path: String,
    /// The mandatory `as NAME` prefix. Every import declares one; without
    /// an alias the import doesn't parse (no unprefixed form).
    pub as_name: Ident,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub span: Span,
    /// Slice 4C: the file id this item originated from after resolver
    /// merge (e.g. `"src.math"`). `None` in single-file mode, before the
    /// resolver runs, or for parser-only consumers. Sema uses this to
    /// determine same-vs-cross-file context when enforcing field-level
    /// `pub`. The entry binary's items carry `Some(entry_file_id)` — the
    /// special-casing of `fn main()`'s mangled name doesn't leak here.
    pub origin_file: Option<String>,
}

/// Phase 5 slice 5ATTR.1 — `#[NAME]` or `#[NAME(args)]` attribute attached
/// to an item. Pure declarative metadata read by compiler stages (sema, codegen)
/// or external tools (`cpc test`). Never an AST transformation source —
/// see plan.md §2.8d and [docs/design/phase5-attributes.md](../../docs/design/phase5-attributes.md).
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    /// Attribute name — `"test"`, `"inline"`, `"repr"`. Single-segment in
    /// Phase 5; multi-segment names (`#[derive(...)]`-style) are out of scope.
    pub path: Ident,
    /// Empty for bare-form `#[name]`; non-empty for `#[name(arg, ...)]`.
    pub args: Vec<AttrArg>,
    /// Whole-attribute span including the surrounding `#[...]`.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttrArg {
    /// A bare identifier argument: `#[repr(C)]`, `#[ignore(slow)]`.
    Ident(Ident),
    /// A string literal argument: `#[deprecated("use parse_v2 instead")]`.
    Str(String, Span),
    /// v0.0.7 Slice 1.3: an integer literal argument — `#[unroll(4)]`,
    /// `#[vectorize_width(8)]`. Parser stores the raw value; attrs
    /// validation + sema check the per-attribute range.
    Int(i64, Span),
    /// `name = VALUE` form: `#[link(name = "z", kind = "static")]`. Not used
    /// by any Phase 5 attribute; parser admits the shape for forward-compat.
    KeyValue(Ident, AttrValue),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Ident(Ident),
    Str(String, Span),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Function(Function),
    Enum(EnumDecl),
    Struct(StructDecl),
    Impl(ImplBlock),
    /// Slice 7GEN.3: `interface Name { fn ... }` declaration. Lists the
    /// method signatures (no bodies) that implementing types must
    /// provide. `Self` inside method signatures refers to the
    /// implementing type at `impl`-resolution time.
    Interface(InterfaceDecl),
    /// Phase 11 polish (2026-05-13): `type Foo = Bar;` — transparent
    /// type alias. The aliased name resolves to the same `Ty` as the
    /// target everywhere it's used. No new type, no nominal distinction.
    /// Cross-file `pub` visible per the usual rules.
    TypeAlias(TypeAlias),
    /// v0.0.9 Phase 4: `pub? const NAME: Ty = LIT;` module-scope named
    /// literal. Lowered by `crate::lower` — every use-site path
    /// expression that resolves to a const is rewritten to a clone of
    /// the initializer expression before sema runs its expression-level
    /// checks. No LLVM global emitted. Initializer must be a literal
    /// (sema enforces, E0X30); type annotation required (parser
    /// enforces, E0X31).
    Const(ConstDecl),
    /// v0.0.9 Phase 4: `pub? static mut? NAME: Ty = LIT;` module-scope
    /// global with a real address. Immutable form lowers to LLVM
    /// `@NAME = constant <ty> <lit>` in `.rodata`; the `mut` form
    /// lowers to `@NAME = global <ty> <lit>` in `.data`. Reads and
    /// writes of `static mut` must occur inside `unsafe { ... }`
    /// (E0X33 / E0X34) — the borrow checker can't prove absence of
    /// data races for module-scope mutable state.
    Static(StaticDecl),
}

/// v0.0.9 Phase 4: module-scope `const NAME: Ty = LIT;` declaration.
/// Lowered away by `crate::lower` before sema's body-check pass —
/// every reference to a const name is replaced with a clone of the
/// initializer expression. By the time codegen runs there are no
/// `ItemKind::Const` items left in the program.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: Type,
    /// Initializer expression. Sema (`check_const_static_inits`) enforces
    /// the literal-only rule with E0X30. The accepted shapes are
    /// `IntLit` / `FloatLit` / `BoolLit` / `StrLit` plus optional
    /// `Unary { op: Neg, operand: <numeric lit> }` for negative
    /// numeric constants. Anything else is a hard error before the
    /// substitution pass runs.
    pub value: Expr,
    pub is_pub: bool,
    pub attributes: Vec<Attribute>,
}

/// v0.0.9 Phase 4: module-scope `static mut? NAME: Ty = LIT;`
/// declaration. Survives through lowering and reaches codegen, which
/// emits one LLVM global per declaration (read via load, written via
/// store for the `mut` variant).
#[derive(Debug, Clone, PartialEq)]
pub struct StaticDecl {
    pub name: Ident,
    pub ty: Type,
    /// Initializer expression. Same literal-only rule as `ConstDecl`
    /// for v0.0.9 (struct-literal / array-literal extensions wait for
    /// a real consumer beyond the immediate raytracer use case).
    pub value: Expr,
    pub is_mut: bool,
    pub is_pub: bool,
    pub attributes: Vec<Attribute>,
}

/// Phase 11 type alias: `type Name = TargetType;`. The resolver
/// transparently substitutes references at every use site.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlias {
    pub name: Ident,
    pub target: Type,
    pub is_pub: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<EnumVariant>,
    /// Slice 4B: `pub enum E { ... }` exports the enum's name AND all its
    /// variants to importers. There is no per-variant `pub` (variants
    /// inherit the enum's visibility).
    pub is_pub: bool,
    /// Slice 5ATTR.1: `#[NAME] enum E { ... }` attributes collected by the
    /// parser. Empty when no attributes precede the declaration.
    pub attributes: Vec<Attribute>,
    /// Slice 7GEN.2: generic type parameters — `enum Option[T] { ... }`.
    /// Empty for non-generic enums. Each instantiation monomorphizes
    /// into a distinct LLVM enum type at codegen time (slice 7GEN.5).
    pub generic_params: Vec<GenericParam>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: Ident,
    /// Positional payload types. Empty for payload-less (plain) variants.
    /// Named-field payloads (`Variant { f: T }`) are deferred — see
    /// `docs/design/phase3-tagged-unions.md`.
    pub payload: Vec<Type>,
    pub span: Span,
    /// Slice 5ATTR.1: attributes attached to this variant.
    pub attributes: Vec<Attribute>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    pub name: Ident,
    pub fields: Vec<StructField>,
    /// Slice 4B: `pub struct S { ... }` exports the type-name. Fields stay
    /// private unless individually marked `pub field: T` — "expose the
    /// type, hide the layout" is the default.
    pub is_pub: bool,
    /// Slice 5ATTR.1: attributes attached to this struct.
    pub attributes: Vec<Attribute>,
    /// Slice 7GEN.2: generic type parameters — `struct Pair[A, B] { ... }`.
    /// Empty for non-generic structs. Each instantiation monomorphizes
    /// into a distinct LLVM struct type (`%Pair__i32__string`) at
    /// codegen time (slice 7GEN.5).
    pub generic_params: Vec<GenericParam>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: Ident,
    pub ty: Type,
    pub span: Span,
    /// Slice 4B: `pub field: T`. Individually-marked fields are visible
    /// to cross-file struct-literal construction and field access; without
    /// `pub` the field is only reachable from within the same file even
    /// when the struct type itself is `pub`.
    pub is_pub: bool,
    /// Slice 5ATTR.1: attributes attached to this field.
    pub attributes: Vec<Attribute>,
    /// v0.0.13 (plan.opaque.md): `opaque field: *T` declares that a raw-pointer
    /// field is *not this struct's responsibility* to release (managed
    /// elsewhere). It suppresses the raw-pointer-accountability error (E0510)
    /// that an unmarked, un-`drop`-released raw-pointer field otherwise triggers.
    /// Only meaningful on `*T` fields; a no-op marker on any other type.
    pub is_opaque: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    pub target: Ident,
    /// Slice 7GEN.5e: impl-level generic parameters declared on the
    /// target — `impl Vec[T] { ... }` records `T` here so methods
    /// inside the block can reference `T` in their signatures.
    /// Empty for plain inherent impls `impl Point { ... }`. When
    /// non-empty, every method inside the block is implicitly
    /// parameterized by these params during monomorphization.
    pub target_generic_params: Vec<GenericParam>,
    pub methods: Vec<Method>,
    /// Slice 7GEN.3: when present, this `impl Interface for Type`
    /// block claims that `target` implements `interface_name`'s method
    /// set. Sema validates method-coverage / signature-match (E0503 /
    /// E0504 / E0505) and coherence (E0507). `None` for plain inherent
    /// `impl Type { ... }` blocks.
    pub interface_name: Option<Ident>,
    /// v0.0.14: `unsafe impl Send for T {}` / `unsafe impl Sync for T {}`.
    /// `Send`/`Sync` are unsafe assertions (the author vouches for thread
    /// safety the compiler can't prove), so their impls must carry `unsafe`.
    /// `false` for every ordinary `impl` / `impl Interface for Type`.
    pub is_unsafe: bool,
}

/// Slice 7GEN.3: an interface declaration. The body holds method
/// signatures with bodies elided (`fn name(self, ...) -> T;` — note
/// the trailing `;` instead of a body block). `Self` appearing
/// anywhere in a method signature is a placeholder for the
/// implementing type.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceDecl {
    pub name: Ident,
    pub methods: Vec<InterfaceMethod>,
    /// Slice 4B: interfaces are `pub`-flagged like other items.
    pub is_pub: bool,
    pub attributes: Vec<Attribute>,
}

/// Slice 7GEN.3: a single method signature inside an interface body.
/// Mirrors `Method` but without `body` (interfaces declare contracts;
/// implementations supply bodies).
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceMethod {
    pub name: Ident,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    pub name: Ident,
    /// Slice 7GEN.5e: method-level generic parameters. `fn cast[T](self) -> T`
    /// records `T` here. Distinct from the enclosing impl block's
    /// `target_generic_params` (which apply to all methods in the
    /// block); these apply only to this method.
    pub generic_params: Vec<GenericParam>,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Block,
    pub span: Span,
    /// Slice 4B: methods are individually `pub`-flagged even on `pub` types.
    /// A non-`pub` method on a `pub struct` is only callable from inside
    /// the declaring file — same logic as private fields.
    pub is_pub: bool,
    /// Slice 5ATTR.1: attributes attached to this method. Per the design
    /// note `#[test]` is rejected inside `impl` (E0360); validation lives
    /// in the post-parse attribute_check pass, not here.
    pub attributes: Vec<Attribute>,
    /// v0.0.4 Phase 4 Slice 4E: `async fn` / `gen fn` method modifiers.
    /// Currently only `is_gen = true` exercises a real lowering path
    /// (async methods land alongside non-Copy `self` async).
    pub is_async: bool,
    pub is_gen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// `self` — read-only access; lowered to a pointer parameter.
    Read,
    /// `mut self` — mutable access; lowered to a pointer parameter; the
    /// caller's place must be writable.
    Mut,
    /// `move self` — ownership transfer; lowered to a pointer parameter;
    /// the caller's place becomes uninitialized after the call.
    Move,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    /// For `extern fn` declarations (slice 10.FFI.1), this is a
    /// synthesized empty `Block` — there's no body in source. Codegen
    /// branches on `is_extern` to emit `declare` instead of `define`;
    /// sema skips body-checking when `is_extern` is set. Keeping the
    /// field as a real `Block` (instead of `Option<Block>`) avoids
    /// touching every site that walks the AST.
    pub body: Block,
    /// Slice 10.FFI.1: `extern fn` declarations. When `true`, the
    /// body field is a synthesized empty block; codegen emits
    /// `declare TYPE @name(...)` with the `ccc` calling convention.
    pub is_extern: bool,
    /// Slice 10.FFI.4: variadic-arg extern fn (e.g.
    /// `extern fn printf(fmt: *u8, ...) -> i32;`). Valid only when
    /// `is_extern` is true. Codegen emits `(<fixed params>, ...)` in
    /// the LLVM `declare` and routes call sites through varargs ABI.
    pub is_variadic: bool,
    /// Slice 4B: `pub fn foo(...)` exports the function to importers.
    pub is_pub: bool,
    /// Slice 5ATTR.1: attributes attached to this function. `#[test]`
    /// discovery walks the merged Program looking for fns whose attributes
    /// include `test`; sema validates the test-fn signature when present.
    pub attributes: Vec<Attribute>,
    /// Slice 7GEN.1: generic type parameters. Empty for non-generic
    /// functions (the common case). Each `GenericParam` carries its
    /// declared name and zero or more interface bounds (e.g.
    /// `T: Ord + Eq` becomes `bounds: ["Ord", "Eq"]`). Monomorphization
    /// (slice 7GEN.5) generates one concrete LLVM function per unique
    /// `(name, [concrete_types])` pair.
    pub generic_params: Vec<GenericParam>,
    /// v0.0.3 Phase 5 Slice 5E.1: `async fn foo() -> T` declarations.
    /// Sema rewrites the user's declared return type from `T` to
    /// `Future[T]` and admits `await EXPR` inside the body. Codegen
    /// (5E.3) lowers the body to an LLVM coroutine via `llvm.coro.*`
    /// intrinsics. False for synchronous functions (the common case).
    pub is_async: bool,
    /// v0.0.4 Phase 4 Slice 4A: `gen fn foo() -> T` declarations.
    /// Sema rewrites the declared return type from `T` to `Iterator[T]`
    /// and admits `yield EXPR;` inside the body. Codegen lowers the
    /// body to an LLVM coroutine that suspends at each yield with the
    /// yielded value stashed in the coroutine promise; `Iterator::next`
    /// resumes + reads + returns `Option::Some(v)` (or `Option::None`
    /// when the coroutine completes).
    pub is_gen: bool,
}

/// Slice 7GEN.1: a single type parameter declaration in a generic
/// item's `[T: Bound1 + Bound2, ...]` list. Used by `Function`,
/// `StructDecl`, `EnumDecl`, and (slice 7GEN.3) interfaces.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    pub name: Ident,
    /// Zero or more interface names bounding this parameter. The
    /// parser keeps them as flat identifiers; sema resolves each to
    /// an interface declaration at substitution time (slice 7GEN.4).
    pub bounds: Vec<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: Type,
    /// `mut x: T` — exclusive borrow for non-Copy types; mutable local
    /// binding for Copy types. Mutually exclusive with `move_`.
    pub mutable: bool,
    /// `move x: T` — ownership transfer. Mutually exclusive with `mutable`.
    pub move_: bool,
    /// v0.0.8 (post-bench-gap): `restrict x: *T` — opt-in `noalias` for
    /// raw-pointer params. The borrow checker doesn't reason about
    /// `*T`, so cpc would otherwise emit just `noundef` on these. With
    /// `restrict`, the programmer asserts the pointer doesn't alias any
    /// other pointer reachable in the function body — violations are
    /// UB. C ABI compatible (LLVM `noalias` is an attribute hint, not
    /// part of the calling convention). Sema (E0411) restricts this to
    /// `*T` param types; on other shapes it's a hard error.
    pub restrict: bool,
    /// v0.0.9 follow-up: `borrow x: T` — explicit shared by-value
    /// parameter. For v0.0.9 this is semantically identical to the
    /// unmarked form (`x: T`) on non-Copy types — both mean "callee
    /// takes a shared copy of the binding, no ownership transfer".
    /// The flag is reserved for a future Phase 1 slice that flips
    /// the default for non-Copy `T` to `move` semantics; `borrow`
    /// will then be the opt-out escape hatch. Sema rejects
    /// `borrow` + `move` and `borrow` + `mut` (mutually exclusive
    /// ownership semantics, like `mut` + `move`).
    pub borrow_: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    Path(String),
    /// Fixed-size array type: `[T; N]`. Length stored as a u32. `N` is an
    /// integer literal, or (v0.0.13) a non-negative integer `const` name —
    /// recorded in `len_name` with `len` a placeholder `0` and folded into
    /// `len` by the lower pass `resolve_const_array_lengths` before sema, so
    /// every later pass still sees a plain `u32` length.
    Array {
        elem: Box<Type>,
        len: u32,
        len_name: Option<String>,
    },
    /// Slice 6BC.5: region-annotated borrow type — `borrow REGION T`.
    /// The region is a region-name identifier local to the enclosing
    /// signature (or struct definition); the inner type is the
    /// underlying place's type. Sema and codegen treat this as a
    /// transparent wrapper for the inner type — region info is
    /// metadata that only the borrow checker reads. Composes with
    /// parameter markers: `xs: borrow A T` is a shared borrow,
    /// `mut xs: borrow A T` an exclusive borrow; `move x: borrow A T`
    /// is a parse error (ownership transfer doesn't borrow).
    Borrowed {
        region: String,
        inner: Box<Type>,
    },
    /// Slice 7GEN.5c: generic type instantiation — `Pair[i32, bool]`.
    /// `name` is the generic type's declared name; `args` is the list
    /// of concrete type arguments. Sema's `resolve_type` synthesizes
    /// a concrete StructDef per unique instantiation and returns the
    /// matching `Ty::Struct(id)`. Monomorphize rewrites every
    /// `TypeKind::Generic` reference to `TypeKind::Path(mangled_name)`
    /// before codegen so codegen only sees concrete struct paths.
    Generic {
        name: String,
        args: Vec<Type>,
    },
    /// Slice 10.FFI.1: raw pointer `*T`. Maps to LLVM `ptr` (opaque,
    /// 8 bytes on 64-bit). Copy semantics (it's just an address). No
    /// borrow checking — caller is responsible for lifetime and
    /// aliasing. Phase-10 first cut: the pointer type parses and
    /// flows through the type system; deref / index / arithmetic
    /// operations land in a follow-up slice (10.FFI.2).
    RawPtr(Box<Type>),
    /// Slice 11.FN_PTR: function pointer type — `fn(T1, T2) -> R` (or
    /// `fn(T1, T2)` with implicit unit return). Maps to LLVM `ptr`,
    /// same lowering as raw data pointers. Always carries the C
    /// calling convention (ccc) at the LLVM level. `Copy` (a pointer
    /// is 8 bytes; identity-equal pointers compare equal). Coercion
    /// from a named C+ fn to a fn-pointer value is type-directed —
    /// the bare identifier in an expected-FnPtr context resolves to
    /// the symbol's address. No closures, no environment capture.
    FnPtr {
        params: Vec<Type>,
        return_type: Option<Box<Type>>,
    },
    /// Phase 11 polish (2026-05-14): slice type `T[]` — fat-pointer
    /// view `{ptr, len}` over a contiguous run of `T`. Copy semantics
    /// (a view, not an owner). Constructed via `slice_from_raw_parts`
    /// (unsafe) or — pending follow-up — via an array→slice conversion.
    /// Indexing `s[i]` is bounds-checked at runtime; element access
    /// via `slice_ptr(s)` / `slice_len(s)` intrinsics is safe.
    Slice(Box<Type>),
    /// v0.0.5 Phase 3 Slice 3B: tuple type `(T1, T2, ...)`. Arity must
    /// be ≥ 2; a parenthesised single type is grouping, and `()` is the
    /// unit type which has its own `Path("()")` representation. Sema
    /// synthesizes a concrete struct per unique `(T1, T2, ...)` combo
    /// (named `__tuple_N_<t1>_<t2>_...`) with fields `_0`, `_1`, ...
    /// Codegen then sees it as any other struct — field access via
    /// `.0` / `.1` desugars to `._0` / `._1` field projection.
    Tuple(Vec<Type>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Let {
        mutable: bool,
        name: Ident,
        ty: Option<Type>,
        /// Optional initializer. `let x: T;` is allowed and produces an
        /// unassigned binding; sema (definite-assignment analysis) verifies
        /// every read is preceded by an assignment. A `let` without init
        /// must declare a type — sema cannot infer the binding's type
        /// without an initializer.
        init: Option<Expr>,
    },
    Return(Option<Expr>),
    While {
        cond: Expr,
        body: Block,
        /// v0.0.7 Slice 1.3: statement-level loop-hint attributes
        /// (`#[unroll(N)]`, `#[vectorize_width(N)]`). Codegen emits
        /// `!llvm.loop` metadata on the back-edge branch.
        attributes: Vec<Attribute>,
    },
    For(ForLoop, Vec<Attribute>),
    Expr(Expr),
    /// `defer EXPR;` — registers the expression to run at the enclosing
    /// scope's exit, in LIFO order with any `Drop` calls. See
    /// `docs/design/phase3-drop.md` §4.4. The deferred expression is
    /// re-emitted at scope exit (lexical, not Go's runtime-stack model):
    /// whatever the expression evaluates to at scope-exit time is what
    /// executes — so `defer println(x)` reads x's final value, not its
    /// value at the `defer` statement.
    Defer(Expr),
    /// `if let PATTERN = SCRUTINEE { BODY }` and the two-arm form with
    /// `else`. Pure sugar over `match` (slice 4A.5). The lowering pass
    /// (`crate::lower`) verifies the pattern is refutable (E0347) and then
    /// rewrites this node to an equivalent match. After the lowering pass
    /// runs, no `IfLet` nodes survive into sema. See
    /// `docs/design/phase4-pattern-let.md`.
    IfLet {
        pattern: Pattern,
        scrutinee: Expr,
        body: Block,
        else_body: Option<Block>,
    },
    /// `break;` — exits the innermost enclosing loop. Sema rejects
    /// `break` outside a loop context with E0353. Phase 4 carries no
    /// labelled-break form (`break 'outer;`).
    Break,
    /// `continue;` — jumps to the next iteration of the innermost
    /// enclosing loop. Same context rule (E0353).
    Continue,
    /// `assert EXPR;` — Phase 5 slice 5ATTR.3. The expression must be
    /// `bool`; codegen branches on it and traps via `llvm.trap` on the
    /// false path. In test builds (synthesized by `cpc test`, slice
    /// 5ATTR.4) the trap is replaced by a per-test failure-flag write
    /// so the runner can report which test failed without aborting the
    /// whole process. Source-line attribution (which assert fired) is
    /// future work per design note [docs/design/phase5-attributes.md](../../docs/design/phase5-attributes.md) §6.3.
    Assert(Expr),
    /// `loop { BODY }` — unconditional loop. Exits only via `break`,
    /// `return`, or a no-return call. Codegen emits a simple back-edge
    /// from end-of-body to start-of-body. v0.0.7 Slice 1.3: optional
    /// loop-hint attributes (`#[unroll(N)]`, `#[vectorize_width(N)]`).
    Loop(Block, Vec<Attribute>),
    /// `while let PATTERN = SCRUTINEE { BODY }` — loop body runs each
    /// iteration the pattern matches; loop exits as soon as it doesn't.
    /// Lowered (in `crate::lower`) to `loop { match SCRUTINEE { PAT =>
    /// BODY, _ => break, } }`. Refutable pattern required (same
    /// reasoning as `if let` — E0347).
    WhileLet {
        pattern: Pattern,
        scrutinee: Expr,
        body: Block,
    },
    /// `guard let PATTERN = SCRUTINEE else { ELSE };` —
    /// the binding(s) from PATTERN live in the *enclosing* scope after the
    /// statement, on the proven assumption that the else block diverges
    /// (return / break / continue). With `else |COMPLEMENT|`, the
    /// complement pattern receives the non-matching value and the two
    /// patterns must cover the scrutinee exhaustively. Slice 4A.5.
    /// Lowering: verifies else divergence (E0348) + complement coverage
    /// (E0349, E0350), then rewrites to a `let` + `match` pair.
    GuardLet {
        pattern: Pattern,
        scrutinee: Expr,
        /// `else |Pat|` — present iff the user wrote the complement form.
        /// When absent the lowering pass synthesizes a `_` arm.
        complement: Option<Pattern>,
        else_body: Block,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForLoop {
    CStyle {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Vec<Expr>,
        body: Block,
    },
    Range {
        var: Ident,
        iter: Expr,
        body: Block,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    IntLit(u64, NumSuffix),
    FloatLit(f64, NumSuffix),
    BoolLit(bool),
    /// Phase 8 slice 8.STR.1: string literal. Payload is the decoded
    /// UTF-8 bytes (escape sequences already processed by the lexer).
    /// Type is `Ty::Str`; codegen emits the bytes as a static
    /// `private unnamed_addr constant` and constructs a `{ptr, len}`
    /// fat-pointer struct at the use site.
    StrLit(String),
    /// `c"..."` C-string literal. Decoded payload (NUL appended at codegen).
    /// Type is `*u8` — a bare pointer to a NUL-terminated `.rodata` blob, for
    /// FFI. Safe to *form* (it's a pointer to static data); dereferencing it
    /// needs `unsafe` like any raw pointer.
    CStrLit(String),
    /// Phase 8 slice 8.STR.B.1: interpolated string literal —
    /// `"hello ${name}, n is ${n}"`. Alternating Lit and Expr parts.
    /// Type is `Ty::String` (owned). Sema requires every Expr part's
    /// type to satisfy `ToString` (blessed for primitives + `str`).
    /// Codegen lowers to `__string_concat`: compute total length, one
    /// malloc, memcpy each part in turn.
    InterpStr {
        parts: Vec<InterpStrPart>,
    },
    Ident(String),
    Block(Block),
    /// Slice 10.FFI.3: `unsafe { ... }` block. Same body shape as a
    /// regular Block; presence marks the enclosed code as permitted to
    /// perform operations that the borrow checker / type system can't
    /// verify (pointer deref, extern fn calls, `str_from_raw_parts`).
    /// Outside an unsafe block, those operations fire E0801.
    Unsafe(Block),
    /// v0.0.3 Phase 5 Slice 5E.1: prefix `await EXPR`. The inner
    /// expression must evaluate to a `Future[T]`; the surrounding fn
    /// must be `async`. Sema enforces both. Codegen (5E.3) lowers to
    /// `llvm.coro.suspend` plus the resume/return branches.
    Await(Box<Expr>),
    /// v0.0.4 Phase 4 Slice 4A: `yield EXPR` — produce one value from
    /// a generator. The surrounding fn must be `gen`. Sema enforces
    /// the value type matches the iterator's T. Codegen lowers to
    /// `store EXPR -> promise; llvm.coro.suspend(non-final)` with the
    /// resume arm returning to the next-statement.
    Yield(Box<Expr>),
    If {
        cond: Box<Expr>,
        then: Block,
        else_branch: Option<Box<Expr>>, // must be Block or another If
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        /// Slice 7GEN.5b: explicit `::[T1, T2]` turbofish at a generic-fn
        /// call site. Empty when the call is to a non-generic fn or when
        /// type-args are inferred (slice 7GEN.5a's path). When non-empty,
        /// the count must match the callee's `generic_params` arity;
        /// sema substitutes these directly instead of inferring from
        /// argument types.
        type_args: Vec<Type>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: Type,
    },
    /// A path like `Color::Red`. Phase 2A allows exactly two segments
    /// (enum name + variant); future phases extend to N for modules.
    Path {
        segments: Vec<Ident>,
    },
    /// Struct literal: `Point { x: 1, y: 2 }`. Phase 2B.
    StructLit {
        name: Ident,
        fields: Vec<StructLitField>,
    },
    /// Slice 7GEN.5c: generic struct literal —
    /// `Pair[i32, bool] { first: 7, second: true }`. `name` is the
    /// generic template name; `type_args` is the list of concrete
    /// type arguments. Sema resolves this to the same `Ty::Struct(id)`
    /// that `TypeKind::Generic { name, args }` produces; monomorphize
    /// later rewrites this node to a regular `StructLit` with the
    /// mangled name.
    GenericStructLit {
        name: Ident,
        type_args: Vec<Type>,
        fields: Vec<StructLitField>,
    },
    /// Slice 7GEN.5d: generic enum constructor call —
    /// `Option[i32]::Some(7)`, `Result[i32, string]::Err("e")`.
    /// `enum_name` is the generic enum template; `type_args` are the
    /// concrete type args; `variant` is the variant name; `args` is
    /// the payload expression list (may be empty for payload-less
    /// variants like `Option[i32]::None`). Sema synthesizes a
    /// concrete EnumDef per `(enum_name, type_args)` pair via
    /// `resolve_generic_instantiation_enum`. Monomorphize rewrites
    /// this node to a regular `Path { [mangled_enum, variant] }`-call
    /// or path expression.
    GenericEnumCall {
        enum_name: Ident,
        type_args: Vec<Type>,
        variant: Ident,
        args: Vec<Expr>,
    },
    /// Field access: `expr.name`. Phase 2B.
    Field {
        receiver: Box<Expr>,
        name: Ident,
    },
    /// Array literal: `[1, 2, 3]`. Phase 2D.
    ArrayLit {
        elements: Vec<Expr>,
    },
    /// v0.0.11 Phase 3: fill-array literal `[EXPR; N]`. Shorthand for
    /// an N-element array where every slot is initialized to a clone of
    /// `EXPR`. Lowering: codegen emits one `memset` for byte-valued
    /// fills, otherwise an enumerated store loop. The
    /// motivating consumer is `vendor/static-arena`'s 16KB / 64KB / etc.
    /// stack-allocated buffer fields, which can't be written as 16384
    /// enumerated literals.
    ///
    /// v0.0.13: `N` may also be a non-negative integer `const` name. The
    /// parser records it in `count_name` with `count` a placeholder `0`; the
    /// lower pass `resolve_const_array_lengths` folds the const value into
    /// `count` (clearing `count_name`) before sema, so every later pass still
    /// sees a plain `u32`.
    ArrayFill {
        fill: Box<Expr>,
        count: u32,
        count_name: Option<String>,
    },
    /// Indexing: `expr[index]`. Phase 2D.
    Index {
        receiver: Box<Expr>,
        index: Box<Expr>,
    },
    /// v0.0.5 Phase 3 Slice 3B: tuple literal `(a, b, ...)`. Arity ≥ 2;
    /// `(a)` is grouping (handled in parse_primary as a pass-through),
    /// `()` is the unit literal. Sema looks up the synthesized tuple
    /// struct for `(T_a, T_b, ...)` and rewrites this node to a struct
    /// literal with fields `_0`, `_1`, ... bound to the element exprs.
    TupleLit {
        elements: Vec<Expr>,
    },
    /// `match SCRUTINEE { Pat => arm, ... }`. Phase 3I.
    /// Scrutinee is an enum value; arms are checked for exhaustiveness by
    /// sema. Each arm's body is parsed as either an expression followed by
    /// `,` (short form) or a `Block` (no trailing `,` required). The parser
    /// normalizes both to `Expr` so codegen treats them uniformly.
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// v0.0.6 Slice 1A: `include_bytes!("relative/path")` compiler builtin.
    /// `path` is the raw string-literal payload (lexer-decoded). Sema
    /// resolves it relative to the containing source file, reads the bytes
    /// at compile time, stashes them in the compile-time-blob table, and
    /// assigns type `*const [u8; N]`. Codegen emits a private constant
    /// `[N x i8]` global and returns its address. The `!` token after
    /// `include_bytes` marks the form as a compiler builtin — no
    /// user-defined macros.
    IncludeBytes {
        path: String,
    },
    /// v0.0.7 Slice 3.1: `include_str!("relative/path")` companion to
    /// `include_bytes!`. Same path resolution + same compile-time read,
    /// but the bytes are UTF-8-validated at sema time (E0875 on invalid)
    /// and the result type is `str` (the fat-pointer view). Codegen
    /// shares the underlying `[N x i8]` global with any `include_bytes!`
    /// call on the same path and builds the `{ ptr, i64 }` aggregate.
    IncludeStr {
        path: String,
    },
    /// v0.0.8 Phase 4: `env!("NAME")` compile-time environment-variable
    /// read. Resolves at sema time via `std::env::var(name)`. Errors:
    ///   - **E0871** at parse time — non-string-literal argument.
    ///   - **E0876** at sema time — environment variable not set in the
    ///     compiler's environment at build time.
    /// Result type is `str` (a `.rodata` global plus its UTF-8 byte
    /// length). Same dedup behavior as `include_str!` — two `env!("X")`
    /// calls on the same name share one underlying byte global.
    EnvVar {
        name: String,
    },
    /// v0.0.10 Phase 4: `#name(args)` compiler-intrinsic call. The `#`
    /// sigil routes the name through a hardcoded intrinsic-dispatch table
    /// in sema (E0905 on unknown name). Replaces the inconsistent mix of
    /// `!`-suffix (`include_bytes!`) and bare-name (`addr_of`) intrinsics
    /// from earlier cycles. Supports:
    ///   - turbofish type args: `#size_of::[T]()`
    ///   - optional return-type ascription: `#msg_send(recv, "sel") -> T`
    /// The optional `ret_ty` is mainly load-bearing for Phase 4B
    /// (`#msg_send`) where the C-ABI return-type can't be inferred from
    /// the receiver. Other intrinsics ignore it.
    Intrinsic {
        name: String,
        type_args: Vec<Type>,
        args: Vec<Expr>,
        ret_ty: Option<Type>,
    },
    /// v0.0.14 inline asm Tier 2: `#asm("tmpl {a},{b}", a = in(reg) x,
    /// b = out(reg) y, clobber("cc"))`. Tier 1 (`#asm("dmb ish")`) is the
    /// degenerate case with no operands and no clobbers. The `template` is a
    /// string literal; `{name}` placeholders bind to operands by name.
    Asm {
        template: String,
        operands: Vec<AsmOperand>,
        clobbers: Vec<String>,
    },
}

/// One operand of a Tier 2 `#asm`. `name` is the `{name}` placeholder; `dir`
/// is the data direction; `reg` is `reg` (compiler-chosen) or an explicit
/// register/constraint string; `value` is the input expression (for `In`) or
/// the output place / read-write place (for `Out`/`InOut`).
#[derive(Debug, Clone, PartialEq)]
pub struct AsmOperand {
    pub name: String,
    pub dir: AsmDir,
    pub reg: AsmReg,
    pub value: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AsmDir {
    In,
    Out,
    InOut,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AsmReg {
    /// `reg` — any general register the compiler picks.
    Any,
    /// An explicit LLVM constraint register token, e.g. `"x0"` (the `{...}` /
    /// `=`/`+` decoration is added by codegen from `dir`).
    Explicit(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// `_` — matches anything, no binding.
    Wildcard,
    /// `name` — matches anything, binds the scrutinee to `name` in the arm.
    /// Distinguished from a variant pattern only at sema time: if `name`
    /// is a known variant of the scrutinee's type, it's parsed as a
    /// variant pattern; otherwise as a binding. The parser produces both
    /// in their natural forms (Binding for bare identifier; Variant for
    /// `Enum::Variant(...)`); no ambiguity at parse time.
    Binding(Ident),
    /// `Enum::Variant` or `Enum::Variant(p1, p2, ...)`. Phase 3I patterns
    /// are one nesting level: payload patterns must themselves be
    /// `Wildcard` or `Binding` — no nested variant patterns yet.
    ///
    /// `type_args` carries `Option[i32]::Some(v)`-style generic enum
    /// instantiation arguments at pattern position (Phase 7 slice
    /// 7GEN.5e). Empty for non-generic enums and for unqualified
    /// patterns (`Option::Some(v)`) that rely on type-directed
    /// resolution from the scrutinee's type. Never holds the
    /// internal monomorphized mangled name — that's an
    /// implementation detail invisible at the source level.
    Variant {
        enum_name: Ident,
        type_args: Vec<Type>,
        variant_name: Ident,
        payload: Vec<Pattern>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructLitField {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

/// Phase 8 slice 8.STR.B.1: one piece of an interpolated string literal.
/// Lit holds decoded bytes (escapes + `$$` already processed). Expr holds
/// a parsed expression — sema requires its type to satisfy `ToString`.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpStrPart {
    Lit(String),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    AddWrap,
    SubWrap,
    MulWrap,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    Ref { mutable: bool },
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    ShlAssign,
    ShrAssign,
}
