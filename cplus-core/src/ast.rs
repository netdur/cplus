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
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Function(Function),
    Enum(EnumDecl),
    Struct(StructDecl),
    Impl(ImplBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    pub name: Ident,
    pub fields: Vec<StructField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: Ident,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    pub target: Ident,
    pub methods: Vec<Method>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    pub name: Ident,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Block,
    pub span: Span,
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
    pub body: Block,
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
        init: Expr,
    },
    Return(Option<Expr>),
    While {
        cond: Expr,
        body: Block,
    },
    For(ForLoop),
    Expr(Expr),
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
    Ident(String),
    Block(Block),
    If {
        cond: Box<Expr>,
        then: Block,
        else_branch: Option<Box<Expr>>, // must be Block or another If
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
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
