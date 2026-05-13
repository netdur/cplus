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
    /// Fixed-size array type: `[T; N]`. Length stored as a u32 (Phase 2D
    /// requires an integer literal; const expressions come later).
    Array { elem: Box<Type>, len: u32 },
    /// Slice 6BC.5: region-annotated borrow type — `borrow REGION T`.
    /// The region is a region-name identifier local to the enclosing
    /// signature (or struct definition); the inner type is the
    /// underlying place's type. Sema and codegen treat this as a
    /// transparent wrapper for the inner type — region info is
    /// metadata that only the borrow checker reads. Composes with
    /// parameter markers: `xs: borrow A T` is a shared borrow,
    /// `mut xs: borrow A T` an exclusive borrow; `move x: borrow A T`
    /// is a parse error (ownership transfer doesn't borrow).
    Borrowed { region: String, inner: Box<Type> },
    /// Slice 7GEN.5c: generic type instantiation — `Pair[i32, bool]`.
    /// `name` is the generic type's declared name; `args` is the list
    /// of concrete type arguments. Sema's `resolve_type` synthesizes
    /// a concrete StructDef per unique instantiation and returns the
    /// matching `Ty::Struct(id)`. Monomorphize rewrites every
    /// `TypeKind::Generic` reference to `TypeKind::Path(mangled_name)`
    /// before codegen so codegen only sees concrete struct paths.
    Generic { name: String, args: Vec<Type> },
    /// Slice 10.FFI.1: raw pointer `*T`. Maps to LLVM `ptr` (opaque,
    /// 8 bytes on 64-bit). Copy semantics (it's just an address). No
    /// borrow checking — caller is responsible for lifetime and
    /// aliasing. Phase-10 first cut: the pointer type parses and
    /// flows through the type system; deref / index / arithmetic
    /// operations land in a follow-up slice (10.FFI.2).
    RawPtr(Box<Type>),
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
    },
    For(ForLoop),
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
    /// from end-of-body to start-of-body.
    Loop(Block),
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
    Ident(String),
    Block(Block),
    /// Slice 10.FFI.3: `unsafe { ... }` block. Same body shape as a
    /// regular Block; presence marks the enclosed code as permitted to
    /// perform operations that the borrow checker / type system can't
    /// verify (pointer deref, extern fn calls, `str_from_raw_parts`).
    /// Outside an unsafe block, those operations fire E0801.
    Unsafe(Block),
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
    /// Indexing: `expr[index]`. Phase 2D.
    Index {
        receiver: Box<Expr>,
        index: Box<Expr>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    AddWrap, SubWrap, MulWrap,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    BitAnd, BitOr, BitXor,
    Shl, Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg, Not, BitNot,
    Ref { mutable: bool },
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign, SubAssign, MulAssign, DivAssign, ModAssign,
    BitAndAssign, BitOrAssign, BitXorAssign, ShlAssign, ShrAssign,
}
