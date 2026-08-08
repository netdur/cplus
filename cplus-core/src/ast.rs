use crate::lexer::{NumSuffix, Span};

impl Span {
    pub fn merge(self, other: Span) -> Span {
        // File-aware: a merged span keeps the real file id when either
        // side has one (synthesized 0-spans merge transparently).
        let file = if self.file != 0 {
            self.file
        } else {
            other.file
        };
        Span::in_file(file, self.start.min(other.start), self.end.max(other.end))
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
    /// visibility. The entry binary's items carry `Some(entry_file_id)` — the
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
    /// An integer-literal value in the `name = VALUE` form —
    /// `#[watch(history = 10)]`. Without this the key-value shape could only
    /// carry a quoted count (`history = "10"`), which reads as a string and
    /// pushes the parse of a number into attribute validation.
    Int(i64, Span),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Function(Function),
    Enum(EnumDecl),
    Struct(StructDecl),
    Impl(ImplBlock),
    /// Slice 7GEN.3: `interface Name { fn ... }` declaration. Lists the
    /// method signatures (no bodies) that implementing types must
    /// provide. `This` inside method signatures refers to the
    /// implementing type at `impl`-resolution time.
    Interface(InterfaceDecl),
    /// Phase 11 polish (2026-05-13): `type Foo = Bar;` — transparent
    /// type alias. The aliased name resolves to the same `Ty` as the
    /// target everywhere it's used. No new type, no nominal distinction.
    /// Cross-file visible per the usual name-based rules.
    TypeAlias(TypeAlias),
    /// v0.0.9 Phase 4: `export? const NAME: Ty = LIT;` module-scope named
    /// literal. Lowered by `crate::lower` — every use-site path
    /// expression that resolves to a const is rewritten to a clone of
    /// the initializer expression before sema runs its expression-level
    /// checks. No LLVM global emitted. Initializer must be a literal
    /// (sema enforces, E0911); a type annotation is required (parser
    /// enforces).
    Const(ConstDecl),
    /// v0.0.9 Phase 4: `export? static NAME: Ty = LIT;` module-scope
    /// global with a real address. A `static` is a mutable, C-facing
    /// global: it lowers to LLVM `@NAME = global <ty> <lit>` in `.data`.
    /// (A `const`-style immutable global instead uses `const`, which
    /// lowers to `@NAME = constant <ty> <lit>` in `.rodata`.) Reads and
    /// writes of a `static` are bare (the read/write-accountability codes
    /// were dropped in v0.0.24) — the borrow checker can't prove absence of
    /// data races for module-scope mutable state; that's the author's
    /// responsibility.
    Static(StaticDecl),
    /// v0.0.15: module-scope `#asm("...");` → LLVM `module asm "..."`. Raw
    /// assembly emitted at module top level, outside any function — the
    /// item-scope counterpart of the function-body `#asm(...)` intrinsic
    /// (`ExprKind::Asm`). No operands or clobbers: those bind to SSA values,
    /// which don't exist at module scope. The template is emitted verbatim
    /// (used for raw module-level symbols/data, e.g. a hand-written global
    /// symbol or a `.section` directive). Carried through every pass inert —
    /// no name resolution, no type-check, no monomorphization.
    ModuleAsm(ModuleAsm),
}

/// v0.0.15: module-scope `#asm("...");` declaration. See [`ItemKind::ModuleAsm`].
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleAsm {
    /// The raw assembly template, verbatim from the string literal.
    pub template: String,
    pub span: Span,
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
    /// the literal-only rule with E0911. The accepted shapes are
    /// `IntLit` / `FloatLit` / `BoolLit` / `StrLit` plus optional
    /// `Unary { op: Neg, operand: <numeric lit> }` for negative
    /// numeric constants. Anything else is a hard error before the
    /// substitution pass runs.
    pub value: Expr,
    pub is_pub: bool,
    pub attributes: Vec<Attribute>,
}

/// v0.0.9 Phase 4: module-scope `static NAME: Ty = LIT;` declaration.
/// Survives through lowering and reaches codegen, which emits one LLVM
/// global per declaration (read via load, written via store — every
/// `static` is mutable in v0.0.24).
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
    /// v0.0.27: `type UserId = distinct i64;` — a NOMINAL alias. Same
    /// representation and ABI as the target, but a distinct type to sema:
    /// not interchangeable with the base or with other distinct aliases;
    /// conversion is an explicit `as` in either direction. `false` for the
    /// transparent `type Foo = Bar;` form.
    pub is_distinct: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<EnumVariant>,
    /// Slice 4B (v0.0.24 #10): `true` when the enum is marked `export`,
    /// placing its name AND all its variants on the C-ABI / header surface.
    /// There is no per-variant marker (variants inherit the enum's). General
    /// module visibility is name-based — a leading `_` is module-private,
    /// everything else is public — independent of this flag.
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
    /// v0.0.27 FFI enums: explicit discriminant — `Variant = 4`. The parser
    /// stores the written expression in `value_expr`; lower const-evaluates
    /// it into `value` (E0921 on a non-constant); sema and codegen read only
    /// `value`. Both `None` for auto-assigned (previous + 1, C rules) and
    /// for payload-carrying variants (which reject the `=` form, E0923).
    pub value: Option<i64>,
    pub value_expr: Option<Box<Expr>>,
}

/// v0.0.27 FFI enums: the representation a `#[repr(...)]` attribute pins a
/// PLAIN enum to. `None` = the historical default (`i32`). `#[repr(C)]` on
/// an enum also means `i32` (the C default on every target cpc supports).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumRepr {
    pub bits: u8,
    pub signed: bool,
}

pub fn enum_repr_of(attributes: &[Attribute]) -> Option<EnumRepr> {
    for a in attributes {
        if a.path.name != "repr" {
            continue;
        }
        for arg in &a.args {
            let AttrArg::Ident(id) = arg else { continue };
            let r = match id.name.as_str() {
                "i8" => Some(EnumRepr { bits: 8, signed: true }),
                "i16" => Some(EnumRepr { bits: 16, signed: true }),
                "i32" | "C" => Some(EnumRepr { bits: 32, signed: true }),
                "i64" => Some(EnumRepr { bits: 64, signed: true }),
                "u8" => Some(EnumRepr { bits: 8, signed: false }),
                "u16" => Some(EnumRepr { bits: 16, signed: false }),
                "u32" => Some(EnumRepr { bits: 32, signed: false }),
                "u64" => Some(EnumRepr { bits: 64, signed: false }),
                _ => None,
            };
            if r.is_some() {
                return r;
            }
        }
    }
    None
}

/// v0.0.27 FFI enums: the discriminant value of every variant, C rules —
/// an explicit `= N` sets the counter, an unadorned variant takes
/// previous + 1, the first defaults to 0. THE shared rule: sema's
/// validation and codegen's emission both call this, so they cannot
/// drift on what a variant's runtime value is.
pub fn enum_value_plan(decl: &EnumDecl) -> Vec<i64> {
    let mut out = Vec::with_capacity(decl.variants.len());
    let mut next: i64 = 0;
    for v in &decl.variants {
        let val = v.value.unwrap_or(next);
        out.push(val);
        next = val.wrapping_add(1);
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    pub name: Ident,
    pub fields: Vec<StructField>,
    /// Slice 4B (v0.0.24 #10): `true` when the struct is marked `export`,
    /// placing the type-name on the C-ABI / header surface. General module
    /// visibility is name-based instead — a leading `_` is module-private,
    /// everything else public. Field visibility follows the same name rule
    /// (`_field` is private); fields are never reachable cross-file unless
    /// the struct itself is exported.
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
    /// Slice 4B. Vestigial, always `false` since v0.0.24 #10: field
    /// visibility is name-based — a `_`-prefixed field is module-private,
    /// every other field is visible to cross-file struct-literal
    /// construction and field access (when the struct type itself is
    /// exported). The old per-field `pub` marker is retired (the parser
    /// rejects it with a hint), so this flag is never set.
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
    /// Slice 7GEN.3: when present, this `impl Type: Interface`
    /// block claims that `target` implements `interface_name`'s method
    /// set. Sema validates method-coverage / signature-match (E0503 /
    /// E0504 / E0505) and coherence (E0507). `None` for plain inherent
    /// `impl Type { ... }` blocks.
    pub interface_name: Option<Ident>,
}

/// Slice 7GEN.3: an interface declaration. The body holds method
/// signatures with bodies elided (`fn name(this, ...) -> T;` — note
/// the trailing `;` instead of a body block). `This` appearing
/// anywhere in a method signature is a placeholder for the
/// implementing type.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceDecl {
    pub name: Ident,
    pub methods: Vec<InterfaceMethod>,
    /// Slice 4B: `true` when the interface is marked `export`, like other
    /// items (general visibility is name-based, `_`-prefix = private).
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
    /// Slice 7GEN.5e: method-level generic parameters. `fn cast[T](this) -> T`
    /// records `T` here. Distinct from the enclosing impl block's
    /// `target_generic_params` (which apply to all methods in the
    /// block); these apply only to this method.
    pub generic_params: Vec<GenericParam>,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Block,
    /// A body-less declaration: `fn name(params) -> T;` with no `{ ... }`.
    ///
    /// This is the header form a binary package ships in `lib/include/`. The
    /// signature is real — sema type-checks calls against it — but the body
    /// lives in a prebuilt archive, so codegen emits `declare` instead of
    /// `define` and the linker resolves it from the bundled `.a`.
    ///
    /// Distinct from `is_extern`, which additionally means "C ABI, bare
    /// unmangled symbol name". A declaration keeps the ordinary C+ mangled
    /// name and calling convention, so it matches what the package's own
    /// build emitted — which is what lets a consumer call it with the same
    /// syntax whether the package shipped source or a binary.
    pub is_declaration: bool,
    pub span: Span,
    /// Slice 4B (v0.0.24 #10): `true` when the method is marked `export`
    /// (C-ABI / linker surface). Method visibility itself is name-based —
    /// a `_`-prefixed method is module-private (callable only inside the
    /// declaring file), every other method is callable cross-file when its
    /// type is reachable — same logic as private fields.
    pub is_pub: bool,
    /// Slice 5ATTR.1: attributes attached to this method. Per the design
    /// note `#[test]` is rejected inside `impl` (E0360); validation lives
    /// in the post-parse attribute_check pass, not here.
    pub attributes: Vec<Attribute>,
    /// v0.0.4 Phase 4 Slice 4E: `async fn` / `gen fn` method modifiers.
    /// Currently only `is_gen = true` exercises a real lowering path
    /// (async methods land alongside non-Copy `this` async).
    pub is_async: bool,
    pub is_gen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// `this` — read-only access; lowered to a pointer parameter.
    Read,
    /// `ref this` — mutable access; lowered to a pointer parameter; the
    /// caller's place must be writable.
    Mut,
    /// `take this` — ownership transfer; lowered to a pointer parameter;
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
    /// A body-less declaration: `fn name(params) -> T;` with no `{ ... }`.
    ///
    /// This is the header form a binary package ships in `lib/include/`. The
    /// signature is real — sema type-checks calls against it — but the body
    /// lives in a prebuilt archive, so codegen emits `declare` instead of
    /// `define` and the linker resolves it from the bundled `.a`.
    ///
    /// Distinct from `is_extern`, which additionally means "C ABI, bare
    /// unmangled symbol name". A declaration keeps the ordinary C+ mangled
    /// name and calling convention, so it matches what the package's own
    /// build emitted — which is what lets a consumer call it with the same
    /// syntax whether the package shipped source or a binary.
    pub is_declaration: bool,
    /// Slice 10.FFI.4: variadic-arg extern fn (e.g.
    /// `extern fn printf(fmt: *u8, ...) -> i32;`). Valid only when
    /// `is_extern` is true. Codegen emits `(<fixed params>, ...)` in
    /// the LLVM `declare` and routes call sites through varargs ABI.
    pub is_variadic: bool,
    /// Slice 4B (v0.0.24 #10): `true` when the fn is marked `export`,
    /// placing it on the C-ABI / header surface. General cross-file
    /// visibility is name-based (a `_`-prefixed fn is module-private).
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
impl Function {
    /// issue-04: a SYNTHESIZED free function — concrete, private, non-variadic,
    /// no attributes, defined here rather than parsed. Compiler-built bridges
    /// and desugars go through this so a construction site names only what it
    /// decides; the eight flags used to be spelled out positionally at every
    /// one, where a transposition is silent.
    pub fn synth(
        name: Ident,
        params: Vec<Param>,
        return_type: Option<Type>,
        body: Block,
    ) -> Function {
        Function {
            name,
            params,
            return_type,
            body,
            is_extern: false,
            is_declaration: false,
            is_variadic: false,
            is_pub: false,
            attributes: Vec::new(),
            generic_params: Vec::new(),
            is_async: false,
            is_gen: false,
        }
    }
}

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
    /// `ref x: T` — exclusive borrow for non-Copy types; mutable local
    /// binding for Copy types. Mutually exclusive with `move_`.
    pub mutable: bool,
    /// `take x: T` — ownership transfer. Mutually exclusive with `mutable`.
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
    /// Vestigial flag, always `false` since v0.0.24. The `borrow` keyword
    /// it once recorded was retired: a bare parameter `x: T` is already a
    /// read-only borrow, so the explicit opt-in marker is gone (the parser
    /// now rejects a Rust-habit `borrow` with a hint at the bare form).
    /// The field is kept to avoid churning every AST-construction site.
    pub borrow_: bool,
    /// Optional default value: `name: T = EXPR`. Spliced in at call sites that
    /// omit this argument (`lower` does the splice; see
    /// docs/design/named-params-and-defaults.md). `None` for a required
    /// parameter. Only valid on C+ functions (never `extern fn`), and a
    /// defaulted parameter must not be followed by a required one.
    pub default: Option<Box<Expr>>,
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
        /// v0.0.27 const expressions: `[T; CAP * 2]` — an inline constant
        /// expression length. Parsed here, evaluated (at `usize`) and folded
        /// into `len` by lower's `resolve_const_array_lengths`; every pass
        /// after lower sees `None`.
        len_expr: Option<Box<Expr>>,
    },
    /// Slice 6BC.5: region-annotated borrow type, historically written
    /// `borrow REGION T`. No source path constructs this anymore: the
    /// `borrow` keyword (both the region-annotated type and the parameter
    /// prefix) was retired in v0.0.24, so the variant is unreachable from
    /// the surface syntax. The region was a region-name identifier local to
    /// the enclosing signature (or struct definition); the inner type was
    /// the underlying place's type. Sema and codegen treat it as a
    /// transparent wrapper for the inner type — region info is metadata
    /// that only the borrow checker reads.
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
        /// v0.0.24 #9: per-param ownership marker — `true` for a `take`
        /// (consuming) param, `false` for a bare read-only borrow. Same length
        /// as `params`. `fn(R)` borrows; `fn(take R)` consumes. The pointed-to
        /// function's param conventions must match this (checked at coercion).
        param_takes: Vec<bool>,
        /// Handle-projection Tier 2: per-param `ref` marker — `true` for a
        /// `fn(ref R)` (exclusive write-back) param. Same length as `params`;
        /// mutually exclusive with the `take` marker at parse time. Mirrors
        /// `Param`'s `mutable`/`move_` pair. A `ref` slot is pointer-passed
        /// for every type (same ABI as a named `ref x: T` param).
        param_refs: Vec<bool>,
        return_type: Option<Box<Type>>,
    },
    /// Phase 11 polish (2026-05-14): slice type `T[]` — fat-pointer
    /// view `{ptr, len}` over a contiguous run of `T`. Copy semantics
    /// (a view, not an owner). Constructed via `slice_from_raw_parts`
    /// (a raw-pointer operation) or — pending follow-up — via an
    /// array→slice conversion. Indexing `s[i]` is bounds-checked at
    /// runtime; element access via `#slice_ptr(s)` / `#slice_len(s)`
    /// intrinsics is safe.
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
    /// `let TYPE { f1, f2, ... } = INIT;` (and the `var` form) — struct
    /// destructuring. Consumes `init` as a whole and moves each named field
    /// into its own binding; `mutable` (from `let` vs `var`) applies to ALL
    /// bound fields (no per-field mutability — matches the let/var model).
    ///
    /// Sound where a bare field move (E0509) is not: the whole value is
    /// decomposed at one point (`init`'s source is marked moved, each field is
    /// re-owned by a binding), so each field drops exactly once and the
    /// struct's own whole-value drop never runs. The field list must be
    /// exhaustive (every field of `TYPE` named, in any order); a struct with an
    /// explicit `fn drop` is rejected (its destructor must run as a unit).
    LetDestructure {
        mutable: bool,
        type_name: Ident,
        fields: Vec<Ident>,
        init: Expr,
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
    /// executes — so `defer #println(x)` reads x's final value, not its
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
        /// `if var PAT = E { ... }` — the pattern bindings are mutable
        /// inside BODY. Lowering renames each binding to a fresh temp in
        /// the match pattern and prepends `var NAME = TEMP;` rebinds to
        /// the success arm's body.
        mutable: bool,
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
        /// `while var PAT = E { ... }` — same rebind lowering as `if var`;
        /// the bindings are fresh (and mutable) each iteration.
        mutable: bool,
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
        /// `guard var PAT = E else { ... };` — the enclosing-scope binding
        /// is mutable (the lowering's synthesized `let` becomes a `var`).
        /// The complement binding (if any) stays immutable: it is an
        /// ordinary match-arm binding scoped to the diverging else block.
        mutable: bool,
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

/// Whether a `match` scrutinee is read **in place** (no copy) rather than
/// evaluated into a temporary. THE shared predicate behind the match ownership
/// model: sema's `classify_scrutinee` and codegen's `enum_scrutinee_ptr` /
/// consume logic both consult it, so they cannot drift on which scrutinees
/// alias storage (the drift that caused the v0.0.23 match double-free class).
///
/// Only a bare whole binding or a field/index projection is read in place
/// (codegen `gen_place`). Everything else — a block, `if`,
/// nested `match`, call, constructor, **or a raw-pointer deref `*p`** — is
/// evaluated by value into a temp (codegen `gen_expr` + spill). A `*p` is
/// deliberately NOT in-place: spilling it bit-copies the pointee, which sema
/// then forbids for a non-Copy pointee (E0337), so it never reaches a buggy
/// drop path.
pub fn scrutinee_reads_in_place(e: &Expr) -> bool {
    matches!(
        &e.kind,
        ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
    )
}

/// Whether a `match` **binds a name** anywhere in its patterns — a catch-all
/// binding (`x => ...`) or a payload binding (`E::A(v) => ...`). The second
/// half of the match ownership model, and — like `scrutinee_reads_in_place` —
/// shared so sema and codegen cannot drift.
///
/// A match that binds nothing reads only the discriminant: no payload leaves
/// the scrutinee's storage, so matching does not consume an owned binding and
/// its scope-exit drop stays armed. That makes `E::A(_) => ...` the
/// **non-consuming presence check**, re-matchable and readable afterwards.
///
/// A match that binds any name moves the scrutinee: codegen disarms the
/// source's drop before the switch (which dominates every arm), so the whole
/// match consumes it even though only one arm runs — and sema marks the
/// binding moved, making a later read E0335. The granularity is per-match, not
/// per-arm, precisely because the disarm is pre-switch.
///
/// Deliberately name-based, not `_`-prefix-based: `_v` is this language's
/// privacy convention, not a wildcard marker, so `E::A(_v)` binds and consumes
/// like any other name.
///
/// Note this is about the *scrutinee's* storage only. An owned **temporary**
/// scrutinee (`match f() { ... }`) has no source binding to keep alive, so the
/// match must tear it down regardless of what its patterns bind.
pub fn match_binds_a_name(arms: &[MatchArm]) -> bool {
    arms.iter().any(|arm| match &arm.pattern.kind {
        // A literal pattern binds nothing; `lower` desugars it away entirely.
        PatternKind::Wildcard | PatternKind::Lit(_) => false,
        PatternKind::Binding(_) => true,
        PatternKind::Variant { payload, .. } => payload
            .iter()
            .any(|p| matches!(p.kind, PatternKind::Binding(_))),
    })
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
    /// is a bare raw-pointer deref like any other (self-flagging at the deref
    /// site — there is no `unsafe` block wrapping).
    CStrLit(String),
    /// Phase 8 slice 8.STR.B.1: interpolated string literal —
    /// `"hello ${name}, n is ${n}"`. Alternating Lit and Expr parts.
    /// Type is `Ty::String` (owned). Sema requires every Expr part's
    /// type to satisfy `ToText` (blessed for primitives + `str`).
    /// Codegen lowers to `__string_concat`: compute total length, one
    /// malloc, memcpy each part in turn.
    InterpStr {
        parts: Vec<InterpStrPart>,
    },
    Ident(String),
    Block(Block),
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
        /// Named-argument labels, parallel to `args`. INVARIANT: either empty
        /// (every argument is positional — the common case and what every
        /// synthetic call site produces) or exactly `args.len()` long, with
        /// `None` for a positional slot and `Some(name)` for `name: value`.
        /// The parser fills this; sema's call checker desugars labels into
        /// positional order (and splices defaults), then clears it — so no
        /// pass after sema's argument-matching ever sees a label.
        arg_labels: Vec<Option<Ident>>,
        /// Slice 7GEN.5b: explicit `::[T1, T2]` turbofish at a generic-fn
        /// call site. Empty when the call is to a non-generic fn or when
        /// type-args are inferred (slice 7GEN.5a's path). When non-empty,
        /// the count must match the callee's `generic_params` arity;
        /// sema substitutes these directly instead of inferring from
        /// argument types.
        type_args: Vec<Type>,
    },
    /// Value-turbofish: a reference to a generic function instantiated at
    /// explicit type args, taken as a fn-pointer VALUE (no call) — `f::[T]`
    /// with no following `(...)`. Monomorphize rewrites it to `Ident(mangled)`
    /// (the concrete instantiation's symbol), so no pass after mono sees it.
    /// `callee` is the function name (an `Ident`); `type_args` is the turbofish.
    FnRef {
        callee: Box<Expr>,
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
    /// v0.0.27: checked narrowing — `x as? u8` evaluates to `Option[u8]`:
    /// `Some(converted)` when the value is representable in the target
    /// integer type, `None` otherwise. Integer source + integer target
    /// only; the infallible `as` keeps its truncating semantics for the
    /// cases where truncation is intended.
    CastChecked {
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
    /// v0.0.24 de-Rust: type-inferred struct literal — `{ x: 1, y: 2 }` with
    /// no leading type name. The struct type is taken from the expected type
    /// at the use site (`let a: A = { x: 1 };`, `return { x: 1 };`,
    /// argument/field positions). Sema resolves the expected type to a
    /// concrete struct, validates the fields exactly like `check_struct_lit`,
    /// and records the resolved struct NAME in `MonoInfo::inferred_struct_lits`
    /// keyed by this node's span. Monomorphize then rewrites the node to a
    /// plain `StructLit` with that name (the same convert-in-mono / panic-in-
    /// codegen discipline as `GenericStructLit`), so codegen never sees it.
    InferredStructLit {
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
    /// `Option[i32]::Some(7)`, `Result[i32, Text]::Err("e")`.
    /// `enum_name` is the generic enum template; `type_args` are the
    /// concrete type args; `variant` is the variant name; `args` is
    /// the payload expression list (may be empty for payload-less
    /// variants like `Option[i32]::None`). Sema synthesizes a
    /// concrete EnumDef per `(enum_name, type_args)` pair via
    /// `resolve_generic_instantiation_enum`. Monomorphize rewrites
    /// this node to a regular `Path { [mangled_enum, variant] }`-call
    /// or path expression.
    ///
    /// `method_type_args` carries a method-level turbofish when the node is
    /// a generic-struct ASSOCIATED-fn call with its own type params —
    /// `Box[i32]::make::[bool](x)`. Empty for enum variant constructors and
    /// for the inferred form (`Box[i32]::make(x)`, where sema infers the
    /// method args). Only meaningful on the struct-assoc-call path.
    GenericEnumCall {
        enum_name: Ident,
        type_args: Vec<Type>,
        variant: Ident,
        method_type_args: Vec<Type>,
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
        /// v0.0.27 const expressions: `[v; CAP * 2]` — inline constant
        /// expression count, folded by lower exactly like `len_expr`.
        count_expr: Option<Box<Expr>>,
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
    /// v0.0.6 Slice 1A: `#include_bytes("relative/path")` compiler
    /// intrinsic. `path` is the raw string-literal payload (lexer-decoded).
    /// Sema resolves it relative to the containing source file, reads the
    /// bytes at compile time, stashes them in the compile-time-blob table,
    /// and assigns type `*[u8; N]`. Codegen emits a private constant
    /// `[N x i8]` global and returns its address. Spelled as a `#name(...)`
    /// intrinsic (§12) — C+ has no macros; the legacy `include_bytes!(...)`
    /// form is a parse error.
    IncludeBytes {
        path: String,
    },
    /// v0.0.7 Slice 3.1: `#include_str("relative/path")` companion to
    /// `#include_bytes`. Same path resolution + same compile-time read,
    /// but the bytes are UTF-8-validated at sema time (E0875 on invalid)
    /// and the result type is `str` (the fat-pointer view). Codegen
    /// shares the underlying `[N x i8]` global with any `#include_bytes`
    /// call on the same path and builds the `{ ptr, i64 }` aggregate.
    IncludeStr {
        path: String,
    },
    /// v0.0.8 Phase 4: `#env("NAME")` compile-time environment-variable
    /// read. Resolves at sema time via `std::env::var(name)`. Errors:
    ///   - **E0871** at parse time — non-string-literal argument.
    ///   - **E0876** at sema time — environment variable not set in the
    ///     compiler's environment at build time.
    ///
    /// Result type is `str` (a `.rodata` global plus its UTF-8 byte
    /// length). Same dedup behavior as `#include_str` — two `#env("X")`
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
    ///
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
    /// v0.0.22 DSL.1–4: contextual builder block. Two surface forms share
    /// this node:
    ///   - the root `@ctx { ... }` (`container` is `None`): `context` is
    ///     the explicit `::`-separated path (`@view`, `@ui::view`);
    ///   - a bare container element `name { ... }` written *inside* a
    ///     builder block (`container` is `Some(name)`, DSL.4): `context`
    ///     is empty at parse time and filled with the *enclosing* block's
    ///     context during resolution (a `vstack` inside `@view` builds
    ///     `view::vstack`, and its children resolve in `view` too).
    ///
    /// The body is a dedicated representation, not a reused `Block`,
    /// because leading-dot modifier lines and `if`/`for` item-control are
    /// not general C+ statements. `lower::desugar_builder_block` (run in
    /// the resolver walk for projects, the lowering pass for single-file,
    /// always before sema) rewrites the node to an ordinary block over
    /// the fixed protocol: the root finishes with `ctx::Builder::finish()`,
    /// a container finishes with `ctx::name(builder)`. No later pass sees
    /// one.
    BuilderBlock {
        context: Vec<Ident>,
        body: BuilderBlock,
        /// `None` for a root `@`-block; `Some(name)` for a bare
        /// container element (DSL.4).
        container: Option<Ident>,
        /// DSL.5 (2026-08-04): arguments written on a container element —
        /// `card(title: t) { ... }`. Always empty for a root `@`-block.
        /// The desugar keeps the Builder as the FIRST argument
        /// (`ctx::card(__b, title: t)`), so the existing finisher
        /// contract (`fn column(take b: Builder, key: str = "")`) is a
        /// zero-arg special case of the same shape.
        container_args: Vec<Expr>,
        /// Labels parallel to `container_args`; same invariant as
        /// `Call::arg_labels` (empty, or exactly `container_args.len()`).
        container_arg_labels: Vec<Option<Ident>>,
    },
}

/// v0.0.22 DSL.1: the body of a builder block — an ordered list of
/// entries (item lines with modifiers, `let` setup, and DSL.4 `if`/`for`
/// item-control).
#[derive(Debug, Clone, PartialEq)]
pub struct BuilderBlock {
    pub entries: Vec<BuilderEntry>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BuilderEntry {
    /// `let NAME = EXPR;` — ordinary local setup. Carries a full `Stmt`
    /// (always `StmtKind::Let`) so lowering can splice it through
    /// unchanged. Boxed to keep `BuilderEntry` small — a bare `Stmt` is
    /// far larger than the other variants.
    Let(Box<Stmt>),
    /// One item expression (`text("title")`, a bare container block, ...)
    /// plus the leading-dot modifier lines that follow it. The
    /// modifiers apply to the item value before it is added to the
    /// builder.
    Item {
        expr: Expr,
        modifiers: Vec<BuilderModifier>,
    },
    /// v0.0.22 DSL.4: `if COND { ENTRIES } [else { ENTRIES }]` — Flutter
    /// "collection-if". Each branch is itself a sequence of builder
    /// entries; the branch's items are added to the enclosing builder
    /// only when the condition selects it. No `else` is required (the
    /// zero-or-more-items case). `else if` is represented as an `else_`
    /// holding a single nested `If` entry.
    If {
        cond: Expr,
        then: Vec<BuilderEntry>,
        else_: Option<Vec<BuilderEntry>>,
    },
    /// v0.0.22 DSL.4: `for VAR in ITER { ENTRIES }` — Flutter
    /// "collection-for". The body's items are added to the enclosing
    /// builder once per iteration.
    For {
        var: Ident,
        iter: Expr,
        body: Vec<BuilderEntry>,
    },
}

/// One leading-dot modifier line inside a builder block:
/// `.field = value` or `.method(args)`.
#[derive(Debug, Clone, PartialEq)]
pub struct BuilderModifier {
    pub name: Ident,
    pub kind: BuilderModifierKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BuilderModifierKind {
    /// `.field = value` — a field assignment on the current item.
    Assign(Expr),
    /// `.method(args)` — a method call on the current item; the result
    /// is discarded.
    Call(Vec<Expr>),
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
    /// A literal value: `0`, `-1`, `true`. Holds the literal as an ordinary
    /// `Expr` so the equality test the lowering builds has its operand
    /// already, and so literal typing follows the same rules it does in
    /// expression position.
    ///
    /// Nothing after `lower` sees this: a `match` containing literal arms is
    /// desugared there into a temp binding plus an if/else chain, the same
    /// way `if let` is desugared into a `match` (reports/bug-25).
    Lit(Box<Expr>),
    Variant {
        enum_name: Ident,
        type_args: Vec<Type>,
        variant_name: Ident,
        payload: Vec<Pattern>,
    },
}

impl Param {
    /// issue-04: a SYNTHESIZED parameter — a plain by-borrow binding with no
    /// default and no modifiers. See [`Function::synth`].
    pub fn synth(name: Ident, ty: Type, span: Span) -> Param {
        Param {
            name,
            ty,
            mutable: false,
            move_: false,
            restrict: false,
            borrow_: false,
            default: None,
            span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructLitField {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

/// Phase 8 slice 8.STR.B.1: one piece of an interpolated string literal.
/// Lit holds decoded bytes (escapes + `$$` already processed). Expr holds
/// a parsed expression — sema requires its type to satisfy `ToText`.
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

// ---------------------------------------------------------------------------
// Generic AST traversal (issue-01)
// ---------------------------------------------------------------------------
//
// Every pass that rewrites the AST used to hand-roll its own match over
// `ExprKind` / `StmtKind` with an `other => other.clone()` fallthrough. A node
// kind missing from one of those matches is not a compile error — it is a
// silently un-rewritten subtree, which is how the tuple-literal, interpolated-
// string and Self-in-loop ICEs (reports/bug-04, bug-06, bug-07) each shipped.
// The walkers below are the single place that knows the shape of the tree:
// they match EXHAUSTIVELY, with no catch-all arm, so a new `ExprKind` variant
// fails to compile until it is handled here, once.
//
// A rewriter overrides only the node kinds it cares about. Returning
// `Some(node)` REPLACES the node and its children are not visited (the
// override is responsible for recursing into whatever children it keeps —
// call `walk_expr` with the same rewriter). Returning `None` keeps the node
// and recurses into every child.

/// The hooks a rewriting walk calls. Every method defaults to "no change,
/// recurse into children", so an implementor writes only the arms it changes.
pub trait ExprRewriter {
    fn visit_expr(&mut self, _e: &Expr) -> Option<Expr> {
        None
    }
    fn visit_stmt(&mut self, _s: &Stmt) -> Option<Stmt> {
        None
    }
    fn visit_type(&mut self, _t: &Type) -> Option<Type> {
        None
    }
    fn visit_pattern(&mut self, _p: &Pattern) -> Option<Pattern> {
        None
    }
}

/// Rewrite one expression: ask the rewriter first, otherwise reconstruct the
/// node with rewritten children. Spans are preserved exactly — the span-keyed
/// side tables sema hands to monomorphize are keyed on them.
pub fn walk_expr<R: ExprRewriter + ?Sized>(e: &Expr, r: &mut R) -> Expr {
    if let Some(replacement) = r.visit_expr(e) {
        return replacement;
    }
    Expr {
        kind: walk_expr_kind(e, r),
        span: e.span,
    }
}

/// The child half of [`walk_expr`]: rebuild this node's KIND with rewritten
/// children, without re-offering the node itself to the rewriter. A hook that
/// keeps the node but changes something about the node itself (its span, say)
/// calls this to recurse without looping.
pub fn walk_expr_kind<R: ExprRewriter + ?Sized>(e: &Expr, r: &mut R) -> ExprKind {
    match &e.kind {
        // Leaves: no child nodes of any kind.
        ExprKind::IntLit(v, s) => ExprKind::IntLit(*v, *s),
        ExprKind::FloatLit(v, s) => ExprKind::FloatLit(*v, *s),
        ExprKind::BoolLit(b) => ExprKind::BoolLit(*b),
        ExprKind::StrLit(s) => ExprKind::StrLit(s.clone()),
        ExprKind::CStrLit(s) => ExprKind::CStrLit(s.clone()),
        ExprKind::Ident(n) => ExprKind::Ident(n.clone()),
        ExprKind::Path { segments } => ExprKind::Path {
            segments: segments.clone(),
        },
        ExprKind::IncludeBytes { path } => ExprKind::IncludeBytes { path: path.clone() },
        ExprKind::IncludeStr { path } => ExprKind::IncludeStr { path: path.clone() },
        ExprKind::EnvVar { name } => ExprKind::EnvVar { name: name.clone() },

        ExprKind::InterpStr { parts } => ExprKind::InterpStr {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpStrPart::Expr(inner) => {
                        InterpStrPart::Expr(Box::new(walk_expr(inner, r)))
                    }
                    InterpStrPart::Lit(s) => InterpStrPart::Lit(s.clone()),
                })
                .collect(),
        },
        ExprKind::Block(b) => ExprKind::Block(walk_block(b, r)),
        ExprKind::Await(inner) => ExprKind::Await(Box::new(walk_expr(inner, r))),
        ExprKind::Yield(inner) => ExprKind::Yield(Box::new(walk_expr(inner, r))),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => ExprKind::If {
            cond: Box::new(walk_expr(cond, r)),
            then: walk_block(then, r),
            else_branch: else_branch
                .as_ref()
                .map(|b| Box::new(walk_expr(b, r))),
        },
        ExprKind::Call {
            callee,
            args,
            arg_labels,
            type_args,
        } => ExprKind::Call {
            callee: Box::new(walk_expr(callee, r)),
            args: args.iter().map(|a| walk_expr(a, r)).collect(),
            arg_labels: arg_labels.clone(),
            type_args: type_args.iter().map(|t| walk_type(t, r)).collect(),
        },
        ExprKind::FnRef { callee, type_args } => ExprKind::FnRef {
            callee: Box::new(walk_expr(callee, r)),
            type_args: type_args.iter().map(|t| walk_type(t, r)).collect(),
        },
        ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
            op: *op,
            lhs: Box::new(walk_expr(lhs, r)),
            rhs: Box::new(walk_expr(rhs, r)),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: *op,
            operand: Box::new(walk_expr(operand, r)),
        },
        ExprKind::Range {
            start,
            end,
            inclusive,
        } => ExprKind::Range {
            start: start.as_ref().map(|s| Box::new(walk_expr(s, r))),
            end: end.as_ref().map(|s| Box::new(walk_expr(s, r))),
            inclusive: *inclusive,
        },
        ExprKind::Assign { op, target, value } => ExprKind::Assign {
            op: *op,
            target: Box::new(walk_expr(target, r)),
            value: Box::new(walk_expr(value, r)),
        },
        ExprKind::CastChecked { expr, ty } => ExprKind::CastChecked {
            expr: Box::new(walk_expr(expr, r)),
            ty: walk_type(ty, r),
        },
        ExprKind::Cast { expr, ty } => ExprKind::Cast {
            expr: Box::new(walk_expr(expr, r)),
            ty: walk_type(ty, r),
        },
        ExprKind::StructLit { name, fields } => ExprKind::StructLit {
            name: name.clone(),
            fields: walk_struct_lit_fields(fields, r),
        },
        ExprKind::InferredStructLit { fields } => ExprKind::InferredStructLit {
            fields: walk_struct_lit_fields(fields, r),
        },
        ExprKind::GenericStructLit {
            name,
            type_args,
            fields,
        } => ExprKind::GenericStructLit {
            name: name.clone(),
            type_args: type_args.iter().map(|t| walk_type(t, r)).collect(),
            fields: walk_struct_lit_fields(fields, r),
        },
        ExprKind::GenericEnumCall {
            enum_name,
            type_args,
            variant,
            method_type_args,
            args,
        } => ExprKind::GenericEnumCall {
            enum_name: enum_name.clone(),
            type_args: type_args.iter().map(|t| walk_type(t, r)).collect(),
            variant: variant.clone(),
            method_type_args: method_type_args.iter().map(|t| walk_type(t, r)).collect(),
            args: args.iter().map(|a| walk_expr(a, r)).collect(),
        },
        ExprKind::Field { receiver, name } => ExprKind::Field {
            receiver: Box::new(walk_expr(receiver, r)),
            name: name.clone(),
        },
        ExprKind::ArrayLit { elements } => ExprKind::ArrayLit {
            elements: elements.iter().map(|el| walk_expr(el, r)).collect(),
        },
        ExprKind::ArrayFill {
            fill,
            count,
            count_name,
            count_expr,
        } => ExprKind::ArrayFill {
            fill: Box::new(walk_expr(fill, r)),
            count: *count,
            count_name: count_name.clone(),
            count_expr: count_expr.as_ref().map(|e| Box::new(walk_expr(e, r))),
        },
        ExprKind::Index { receiver, index } => ExprKind::Index {
            receiver: Box::new(walk_expr(receiver, r)),
            index: Box::new(walk_expr(index, r)),
        },
        ExprKind::TupleLit { elements } => ExprKind::TupleLit {
            elements: elements.iter().map(|el| walk_expr(el, r)).collect(),
        },
        ExprKind::Match { scrutinee, arms } => ExprKind::Match {
            scrutinee: Box::new(walk_expr(scrutinee, r)),
            arms: arms
                .iter()
                .map(|a| MatchArm {
                    pattern: walk_pattern(&a.pattern, r),
                    body: walk_expr(&a.body, r),
                    span: a.span,
                })
                .collect(),
        },
        ExprKind::Intrinsic {
            name,
            type_args,
            args,
            ret_ty,
        } => ExprKind::Intrinsic {
            name: name.clone(),
            type_args: type_args.iter().map(|t| walk_type(t, r)).collect(),
            args: args.iter().map(|a| walk_expr(a, r)).collect(),
            ret_ty: ret_ty.as_ref().map(|t| walk_type(t, r)),
        },
        ExprKind::Asm {
            template,
            operands,
            clobbers,
        } => ExprKind::Asm {
            template: template.clone(),
            operands: operands
                .iter()
                .map(|o| AsmOperand {
                    name: o.name.clone(),
                    dir: o.dir.clone(),
                    reg: o.reg.clone(),
                    value: Box::new(walk_expr(&o.value, r)),
                    span: o.span,
                })
                .collect(),
            clobbers: clobbers.clone(),
        },
        // Builder blocks are desugared to ordinary calls before sema, so no
        // pass that rewrites should meet one. Recursed anyway: a walker with
        // a blind spot is the bug this module exists to prevent.
        ExprKind::BuilderBlock {
            context,
            body,
            container,
            container_args,
            container_arg_labels,
        } => ExprKind::BuilderBlock {
            context: context.clone(),
            body: walk_builder_block(body, r),
            container: container.clone(),
            container_args: container_args.iter().map(|a| walk_expr(a, r)).collect(),
            container_arg_labels: container_arg_labels.clone(),
        },
    }
}

fn walk_struct_lit_fields<R: ExprRewriter + ?Sized>(
    fields: &[StructLitField],
    r: &mut R,
) -> Vec<StructLitField> {
    fields
        .iter()
        .map(|f| StructLitField {
            name: f.name.clone(),
            value: walk_expr(&f.value, r),
            span: f.span,
        })
        .collect()
}

fn walk_builder_block<R: ExprRewriter + ?Sized>(b: &BuilderBlock, r: &mut R) -> BuilderBlock {
    BuilderBlock {
        entries: b.entries.iter().map(|e| walk_builder_entry(e, r)).collect(),
        span: b.span,
    }
}

fn walk_builder_entry<R: ExprRewriter + ?Sized>(e: &BuilderEntry, r: &mut R) -> BuilderEntry {
    match e {
        BuilderEntry::Let(s) => BuilderEntry::Let(Box::new(walk_stmt(s, r))),
        BuilderEntry::Item { expr, modifiers } => BuilderEntry::Item {
            expr: walk_expr(expr, r),
            modifiers: modifiers
                .iter()
                .map(|m| BuilderModifier {
                    name: m.name.clone(),
                    kind: match &m.kind {
                        BuilderModifierKind::Assign(v) => {
                            BuilderModifierKind::Assign(walk_expr(v, r))
                        }
                        BuilderModifierKind::Call(args) => BuilderModifierKind::Call(
                            args.iter().map(|a| walk_expr(a, r)).collect(),
                        ),
                    },
                    span: m.span,
                })
                .collect(),
        },
        BuilderEntry::If { cond, then, else_ } => BuilderEntry::If {
            cond: walk_expr(cond, r),
            then: then.iter().map(|t| walk_builder_entry(t, r)).collect(),
            else_: else_
                .as_ref()
                .map(|es| es.iter().map(|t| walk_builder_entry(t, r)).collect()),
        },
        BuilderEntry::For { var, iter, body } => BuilderEntry::For {
            var: var.clone(),
            iter: walk_expr(iter, r),
            body: body.iter().map(|t| walk_builder_entry(t, r)).collect(),
        },
    }
}

pub fn walk_block<R: ExprRewriter + ?Sized>(b: &Block, r: &mut R) -> Block {
    Block {
        stmts: b.stmts.iter().map(|s| walk_stmt(s, r)).collect(),
        tail: b.tail.as_ref().map(|t| Box::new(walk_expr(t, r))),
        span: b.span,
    }
}

pub fn walk_stmt<R: ExprRewriter + ?Sized>(s: &Stmt, r: &mut R) -> Stmt {
    if let Some(replacement) = r.visit_stmt(s) {
        return replacement;
    }
    let kind = match &s.kind {
        StmtKind::Let {
            mutable,
            name,
            ty,
            init,
        } => StmtKind::Let {
            mutable: *mutable,
            name: name.clone(),
            ty: ty.as_ref().map(|t| walk_type(t, r)),
            init: init.as_ref().map(|e| walk_expr(e, r)),
        },
        StmtKind::LetDestructure {
            mutable,
            type_name,
            fields,
            init,
        } => StmtKind::LetDestructure {
            mutable: *mutable,
            type_name: type_name.clone(),
            fields: fields.clone(),
            init: walk_expr(init, r),
        },
        StmtKind::Return(e) => StmtKind::Return(e.as_ref().map(|e| walk_expr(e, r))),
        StmtKind::While {
            cond,
            body,
            attributes,
        } => StmtKind::While {
            cond: walk_expr(cond, r),
            body: walk_block(body, r),
            attributes: attributes.clone(),
        },
        StmtKind::For(f, attributes) => StmtKind::For(walk_for_loop(f, r), attributes.clone()),
        StmtKind::Expr(e) => StmtKind::Expr(walk_expr(e, r)),
        StmtKind::Defer(e) => StmtKind::Defer(walk_expr(e, r)),
        StmtKind::Assert(e) => StmtKind::Assert(walk_expr(e, r)),
        StmtKind::Break => StmtKind::Break,
        StmtKind::Continue => StmtKind::Continue,
        StmtKind::Loop(body, attributes) => {
            StmtKind::Loop(walk_block(body, r), attributes.clone())
        }
        StmtKind::IfLet {
            pattern,
            scrutinee,
            body,
            else_body,
            mutable,
        } => StmtKind::IfLet {
            pattern: walk_pattern(pattern, r),
            scrutinee: walk_expr(scrutinee, r),
            body: walk_block(body, r),
            else_body: else_body.as_ref().map(|b| walk_block(b, r)),
            mutable: *mutable,
        },
        StmtKind::WhileLet {
            pattern,
            scrutinee,
            body,
            mutable,
        } => StmtKind::WhileLet {
            pattern: walk_pattern(pattern, r),
            scrutinee: walk_expr(scrutinee, r),
            body: walk_block(body, r),
            mutable: *mutable,
        },
        StmtKind::GuardLet {
            pattern,
            scrutinee,
            complement,
            else_body,
            mutable,
        } => StmtKind::GuardLet {
            pattern: walk_pattern(pattern, r),
            scrutinee: walk_expr(scrutinee, r),
            complement: complement.as_ref().map(|p| walk_pattern(p, r)),
            else_body: walk_block(else_body, r),
            mutable: *mutable,
        },
    };
    Stmt { kind, span: s.span }
}

pub fn walk_for_loop<R: ExprRewriter + ?Sized>(f: &ForLoop, r: &mut R) -> ForLoop {
    match f {
        ForLoop::Range { var, iter, body } => ForLoop::Range {
            var: var.clone(),
            iter: walk_expr(iter, r),
            body: walk_block(body, r),
        },
        ForLoop::CStyle {
            init,
            cond,
            update,
            body,
        } => ForLoop::CStyle {
            init: init.as_ref().map(|s| Box::new(walk_stmt(s, r))),
            cond: cond.as_ref().map(|c| walk_expr(c, r)),
            update: update.iter().map(|u| walk_expr(u, r)).collect(),
            body: walk_block(body, r),
        },
    }
}

pub fn walk_type<R: ExprRewriter + ?Sized>(t: &Type, r: &mut R) -> Type {
    if let Some(replacement) = r.visit_type(t) {
        return replacement;
    }
    let kind = match &t.kind {
        TypeKind::Path(n) => TypeKind::Path(n.clone()),
        TypeKind::Array {
            elem,
            len,
            len_name,
            len_expr,
        } => TypeKind::Array {
            elem: Box::new(walk_type(elem, r)),
            len: *len,
            len_name: len_name.clone(),
            len_expr: len_expr.as_ref().map(|e| Box::new(walk_expr(e, r))),
        },
        TypeKind::Borrowed { region, inner } => TypeKind::Borrowed {
            region: region.clone(),
            inner: Box::new(walk_type(inner, r)),
        },
        TypeKind::Generic { name, args } => TypeKind::Generic {
            name: name.clone(),
            args: args.iter().map(|a| walk_type(a, r)).collect(),
        },
        TypeKind::RawPtr(inner) => TypeKind::RawPtr(Box::new(walk_type(inner, r))),
        TypeKind::FnPtr {
            params,
            param_takes,
            param_refs,
            return_type,
        } => TypeKind::FnPtr {
            params: params.iter().map(|p| walk_type(p, r)).collect(),
            param_takes: param_takes.clone(),
            param_refs: param_refs.clone(),
            return_type: return_type.as_ref().map(|rt| Box::new(walk_type(rt, r))),
        },
        TypeKind::Slice(inner) => TypeKind::Slice(Box::new(walk_type(inner, r))),
        TypeKind::Tuple(elems) => {
            TypeKind::Tuple(elems.iter().map(|e| walk_type(e, r)).collect())
        }
    };
    Type { kind, span: t.span }
}

pub fn walk_pattern<R: ExprRewriter + ?Sized>(p: &Pattern, r: &mut R) -> Pattern {
    if let Some(replacement) = r.visit_pattern(p) {
        return replacement;
    }
    let kind = match &p.kind {
        PatternKind::Wildcard => PatternKind::Wildcard,
        PatternKind::Binding(i) => PatternKind::Binding(i.clone()),
        PatternKind::Lit(e) => PatternKind::Lit(Box::new(walk_expr(e, r))),
        PatternKind::Variant {
            enum_name,
            type_args,
            variant_name,
            payload,
        } => PatternKind::Variant {
            enum_name: enum_name.clone(),
            type_args: type_args.iter().map(|t| walk_type(t, r)).collect(),
            variant_name: variant_name.clone(),
            payload: payload.iter().map(|sp| walk_pattern(sp, r)).collect(),
        },
    };
    Pattern { kind, span: p.span }
}

/// Read-only traversal: invoke `f` on every expression node in the subtree,
/// parents before children. Implemented as an adapter over [`walk_expr`] so
/// discovery and rewriting provably visit the SAME node set — a construct one
/// walker descends into and the other does not is precisely reports/bug-04
/// (the call was discovered, the instantiation was synthesized, and the call
/// site kept the template name).
pub fn visit_exprs(e: &Expr, f: &mut impl FnMut(&Expr)) {
    let mut v = ReadOnly { f };
    let _ = walk_expr(e, &mut v);
}

/// Read-only traversal of a block; see [`visit_exprs`].
pub fn visit_exprs_in_block(b: &Block, f: &mut impl FnMut(&Expr)) {
    let mut v = ReadOnly { f };
    let _ = walk_block(b, &mut v);
}

struct ReadOnly<'a, F: FnMut(&Expr)> {
    f: &'a mut F,
}

impl<F: FnMut(&Expr)> ExprRewriter for ReadOnly<'_, F> {
    fn visit_expr(&mut self, e: &Expr) -> Option<Expr> {
        (self.f)(e);
        // `None` keeps the walk going into the children; the reconstructed
        // tree is dropped by the caller. Returning a leaf placeholder here to
        // skip the reconstruction was measured and is not faster — the walk
        // still rebuilds one level per node either way.
        None
    }
}

#[cfg(test)]
mod walker_tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    /// One program per construct family the walkers have to descend into.
    /// Parse-only — none of this has to type-check.
    const SAMPLE: &str = r#"
struct P[T] { v: T }
enum E { A, B }
fn helper(x: i32) -> i32 { return x; }
fn id[U](take u: U) -> U { return u; }
fn sample[T](take t: T, p: *i32, cb: fn(i32) -> i32) -> i32 {
    let pair: (i32, i32) = (helper(1), 2);
    let arr: [i32; 3] = [helper(1), 2, 3];
    let fill: [i32; 4] = [helper(0); 4];
    var acc: i32 = pair.0 + arr[1] - fill[0];
    let s: Text = "a ${helper(2)} b ${acc} c";
    let q: P[i32] = P[i32] { v: helper(3) };
    let g: fn(i32) -> i32 = id::[i32];
    let n: i32 = acc as i32;
    let neg: i32 = -acc;
    let r: bool = acc > 1 && acc < 9;
    while acc < 3 { acc = acc + 1; }
    for i in 0..3 { acc = acc + i; }
    for (var j: i32 = 0; j < 2; j = j + 1) { acc = acc + j; }
    loop { break; }
    if acc > 1 { acc = helper(4); } else { acc = 0; }
    let m: i32 = match acc { _ => helper(5) };
    match acc { 0 => { acc = 1; }, _ => { acc = 2; } }
    defer helper(6);
    assert acc >= 0;
    let blk: i32 = { helper(7) };
    acc = acc + *p;
    acc = acc + cb(8);
    let never: i32 = #size_of::[i32]() as i32;
    return acc + m + n + neg + blk;
}
"#;

    fn sample_fn_bodies() -> Vec<Block> {
        let toks = tokenize(SAMPLE).expect("lex sample");
        let prog = parse(toks).expect("parse sample");
        prog.items
            .into_iter()
            .filter_map(|i| match i.kind {
                ItemKind::Function(f) => Some(f.body),
                _ => None,
            })
            .collect()
    }

    struct NoOp;
    impl ExprRewriter for NoOp {}

    /// The identity property the whole design rests on: a rewriter that
    /// changes nothing reproduces the tree exactly — same nodes, same spans.
    /// A walker arm that forgets a child (or invents one) fails here.
    #[test]
    fn walking_with_a_noop_rewriter_reproduces_the_tree() {
        let bodies = sample_fn_bodies();
        assert!(bodies.len() >= 3, "sample lost its functions");
        for body in &bodies {
            let walked = walk_block(body, &mut NoOp);
            assert_eq!(&walked, body, "no-op walk changed the tree");
        }
    }

    struct RenameIdents;
    impl ExprRewriter for RenameIdents {
        fn visit_expr(&mut self, e: &Expr) -> Option<Expr> {
            let ExprKind::Ident(n) = &e.kind else {
                return None;
            };
            Some(Expr {
                kind: ExprKind::Ident(format!("{n}__seen")),
                span: e.span,
            })
        }
    }

    /// Discovery and rewriting must traverse the same node set — the
    /// asymmetry between them is what reports/bug-04 and bug-06 were. The
    /// read-only visitor is an adapter over the same walk, so this holds by
    /// construction; the test pins it against a future divergence.
    #[test]
    fn read_only_visit_sees_every_node_the_rewrite_walk_rewrites() {
        for body in &sample_fn_bodies() {
            let mut seen = 0usize;
            visit_exprs_in_block(body, &mut |e| {
                if matches!(e.kind, ExprKind::Ident(_)) {
                    seen += 1;
                }
            });
            let rewritten = walk_block(body, &mut RenameIdents);
            let mut renamed = 0usize;
            visit_exprs_in_block(&rewritten, &mut |e| {
                if let ExprKind::Ident(n) = &e.kind {
                    assert!(n.ends_with("__seen"), "missed an ident: {n}");
                    renamed += 1;
                }
            });
            assert_eq!(seen, renamed, "walk and visit disagree on node count");
            assert!(seen > 0, "sample has no idents to rewrite");
        }
    }

    /// Idents nested in the places hand-rolled walkers historically forgot:
    /// interpolated-string parts, tuple elements, array fills, match arms,
    /// defer bodies, C-style for updates.
    #[test]
    fn the_walk_reaches_the_arms_hand_rolled_walkers_forgot() {
        let bodies = sample_fn_bodies();
        let body = bodies.last().expect("sample fn");
        let rewritten = walk_block(body, &mut RenameIdents);
        let mut in_interp = 0usize;
        let mut in_tuple = 0usize;
        let mut in_fill = 0usize;
        visit_exprs_in_block(&rewritten, &mut |e| match &e.kind {
            ExprKind::InterpStr { parts } => {
                for p in parts {
                    if let InterpStrPart::Expr(inner) = p {
                        visit_exprs(inner, &mut |x| {
                            if matches!(x.kind, ExprKind::Ident(_)) {
                                in_interp += 1;
                            }
                        });
                    }
                }
            }
            ExprKind::TupleLit { elements } => {
                for el in elements {
                    visit_exprs(el, &mut |x| {
                        if matches!(x.kind, ExprKind::Ident(_)) {
                            in_tuple += 1;
                        }
                    });
                }
            }
            ExprKind::ArrayFill { fill, .. } => {
                visit_exprs(fill, &mut |x| {
                    if matches!(x.kind, ExprKind::Ident(_)) {
                        in_fill += 1;
                    }
                });
            }
            _ => {}
        });
        assert!(in_interp > 0, "interp-string parts were not walked");
        assert!(in_tuple > 0, "tuple elements were not walked");
        assert!(in_fill > 0, "array-fill value was not walked");
    }
}
