use crate::ast::*;
use crate::lexer::{Span, Token, TokenKind};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseErrorKind {
    Unexpected { found: String, expected: &'static str },
    UnexpectedEof { expected: &'static str },
    NonChainableComparison,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ParseErrorKind::Unexpected { found, expected } => {
                write!(f, "expected {expected}, found {found}")
            }
            ParseErrorKind::UnexpectedEof { expected } => {
                write!(f, "unexpected end of input, expected {expected}")
            }
            ParseErrorKind::NonChainableComparison => {
                write!(f, "comparison operators are non-chainable; use `&&` between comparisons")
            }
        }
    }
}

pub fn parse(tokens: Vec<Token>) -> Result<Program, ParseError> {
    let mut p = Parser::new(tokens);
    p.parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// True while parsing the head of an `if`/`while`/`for-in <iter>` —
    /// in those positions an `Ident` followed by `{` is the cond/iter
    /// followed by the body block, NOT a struct literal. Force the literal
    /// by parenthesizing.
    no_struct_lit: bool,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0, no_struct_lit: false }
    }

    /// Run `f` with `no_struct_lit` flipped on, restoring it afterward.
    fn with_no_struct_lit<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let prev = self.no_struct_lit;
        self.no_struct_lit = true;
        let r = f(self);
        self.no_struct_lit = prev;
        r
    }

    // ---- token cursor ----

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn at(&self, k: &TokenKind) -> bool {
        std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(k)
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if !matches!(t.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, k: &TokenKind) -> bool {
        if self.at(k) { self.bump(); true } else { false }
    }

    fn expect(&mut self, k: &TokenKind, what: &'static str) -> Result<Token, ParseError> {
        if self.at(k) {
            Ok(self.bump())
        } else {
            Err(self.err_at_peek(what))
        }
    }

    fn expect_ident(&mut self) -> Result<Ident, ParseError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::Ident(name) => {
                let n = name.clone();
                self.bump();
                Ok(Ident { name: n, span: tok.span })
            }
            _ => Err(self.err_at_peek("identifier")),
        }
    }

    fn err_at_peek(&self, expected: &'static str) -> ParseError {
        let t = self.peek();
        if matches!(t.kind, TokenKind::Eof) {
            ParseError { kind: ParseErrorKind::UnexpectedEof { expected }, span: t.span }
        } else {
            ParseError {
                kind: ParseErrorKind::Unexpected { found: tok_name(&t.kind).into(), expected },
                span: t.span,
            }
        }
    }

    // ---- top-level ----

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut items = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            items.push(self.parse_item()?);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek_kind() {
            TokenKind::Fn => self.parse_function(),
            TokenKind::Enum => self.parse_enum_decl(),
            TokenKind::Struct => self.parse_struct_decl(),
            TokenKind::Impl => self.parse_impl_block(),
            _ => Err(self.err_at_peek("item (`fn`, `enum`, `struct`, or `impl`)")),
        }
    }

    fn parse_impl_block(&mut self) -> Result<Item, ParseError> {
        let start = self.expect(&TokenKind::Impl, "`impl`")?.span;
        let target = self.expect_ident()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut methods = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            methods.push(self.parse_method()?);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Item {
            kind: ItemKind::Impl(ImplBlock { target, methods }),
            span: start.merge(end),
        })
    }

    fn parse_method(&mut self) -> Result<Method, ParseError> {
        let start = self.expect(&TokenKind::Fn, "`fn`")?.span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen, "`(`")?;

        // Optional receiver as the first param: `self`, `&self`, `&mut self`.
        let receiver = self.try_parse_receiver()?;

        // Remaining params: zero or more, separated by commas. If we had a
        // receiver, the next token is either `)` or `,` (then more params).
        let mut params = Vec::new();
        if receiver.is_some() {
            if self.eat(&TokenKind::Comma) {
                // more params follow
                while !self.at(&TokenKind::RParen) {
                    params.push(self.parse_param()?);
                    if !self.eat(&TokenKind::Comma) { break; }
                }
            }
        } else {
            while !self.at(&TokenKind::RParen) {
                params.push(self.parse_param()?);
                if !self.eat(&TokenKind::Comma) { break; }
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;

        let return_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Method { name, receiver, params, return_type, body, span })
    }

    /// Try to parse a receiver (`self`, `mut self`, or `move self`) at the
    /// start of a method's parameter list. Returns None and rewinds if no
    /// receiver. The combinations `mut move self` and `move mut self` are
    /// rejected as parse errors (mutually exclusive modifiers).
    fn try_parse_receiver(&mut self) -> Result<Option<Receiver>, ParseError> {
        match self.peek_kind() {
            TokenKind::SelfLower => {
                self.bump();
                Ok(Some(Receiver::Read))
            }
            TokenKind::Mut => {
                let save = self.pos;
                self.bump(); // consume `mut`
                if matches!(self.peek_kind(), TokenKind::Move) {
                    // `mut move self` — invalid combination on a receiver.
                    let tok = self.peek().clone();
                    return Err(ParseError {
                        kind: ParseErrorKind::Unexpected {
                            found: tok_name(&tok.kind).into(),
                            expected: "`self` (`mut` and `move` are mutually exclusive)",
                        },
                        span: tok.span,
                    });
                }
                if matches!(self.peek_kind(), TokenKind::SelfLower) {
                    self.bump();
                    Ok(Some(Receiver::Mut))
                } else {
                    // `mut` followed by something else — not a receiver.
                    self.pos = save;
                    Ok(None)
                }
            }
            TokenKind::Move => {
                let save = self.pos;
                self.bump(); // consume `move`
                if matches!(self.peek_kind(), TokenKind::Mut) {
                    // `move mut self` — invalid combination on a receiver.
                    let tok = self.peek().clone();
                    return Err(ParseError {
                        kind: ParseErrorKind::Unexpected {
                            found: tok_name(&tok.kind).into(),
                            expected: "`self` (`move` and `mut` are mutually exclusive)",
                        },
                        span: tok.span,
                    });
                }
                if matches!(self.peek_kind(), TokenKind::SelfLower) {
                    self.bump();
                    Ok(Some(Receiver::Move))
                } else {
                    // `move` followed by something else — not a receiver.
                    self.pos = save;
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn parse_struct_decl(&mut self) -> Result<Item, ParseError> {
        let start = self.expect(&TokenKind::Struct, "`struct`")?.span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.parse_type()?;
            let span = fname.span.merge(ty.span);
            fields.push(StructField { name: fname, ty, span });
            if !self.eat(&TokenKind::Comma) { break; }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Item {
            kind: ItemKind::Struct(StructDecl { name, fields }),
            span: start.merge(end),
        })
    }

    fn parse_enum_decl(&mut self) -> Result<Item, ParseError> {
        let start = self.expect(&TokenKind::Enum, "`enum`")?.span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut variants = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            variants.push(self.expect_ident()?);
            if !self.eat(&TokenKind::Comma) { break; }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Item {
            kind: ItemKind::Enum(EnumDecl { name, variants }),
            span: start.merge(end),
        })
    }

    fn parse_function(&mut self) -> Result<Item, ParseError> {
        let start = self.peek().span;
        self.expect(&TokenKind::Fn, "`fn`")?;
        let name = self.expect_ident()?;

        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) {
            params.push(self.parse_param()?);
            if !self.eat(&TokenKind::Comma) { break; }
        }
        self.expect(&TokenKind::RParen, "`)`")?;

        let return_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Item {
            kind: ItemKind::Function(Function { name, params, return_type, body }),
            span,
        })
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        // Optional ownership prefixes: `mut x: T`, `move x: T`, or both
        // (rejected by sema as E0334). Order doesn't matter at the syntax
        // layer; sema checks the combination.
        let mut mutable = false;
        let mut move_ = false;
        let start = self.peek().span;
        loop {
            match self.peek_kind() {
                TokenKind::Mut if !mutable => { self.bump(); mutable = true; }
                TokenKind::Move if !move_ => { self.bump(); move_ = true; }
                _ => break,
            }
        }
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Colon, "`:`")?;
        let ty = self.parse_type()?;
        let span = start.merge(ty.span);
        Ok(Param { name, ty, mutable, move_, span })
    }

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::Ident(s) => {
                let name = s.clone();
                self.bump();
                Ok(Type { kind: TypeKind::Path(name), span: tok.span })
            }
            TokenKind::LBracket => {
                let start = self.bump().span;
                let elem = Box::new(self.parse_type()?);
                self.expect(&TokenKind::Semi, "`;` in array type")?;
                let len_tok = self.peek().clone();
                let len = match &len_tok.kind {
                    TokenKind::Int(v, _) => {
                        if *v > u32::MAX as u64 {
                            return Err(self.err_at_peek("array length fitting in u32"));
                        }
                        self.bump();
                        *v as u32
                    }
                    _ => return Err(self.err_at_peek("integer array length")),
                };
                let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                Ok(Type { kind: TypeKind::Array { elem, len }, span: start.merge(end) })
            }
            _ => Err(self.err_at_peek("type")),
        }
    }

    // ---- blocks and statements ----

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        let start = self.expect(&TokenKind::LBrace, "`{`")?.span;
        let mut stmts = Vec::new();
        let mut tail: Option<Box<Expr>> = None;

        while !self.at(&TokenKind::RBrace) {
            if matches!(self.peek_kind(), TokenKind::Eof) {
                return Err(self.err_at_peek("`}`"));
            }
            // Statements introduced by a keyword always end with `;`.
            match self.peek_kind() {
                TokenKind::Let => { stmts.push(self.parse_let_stmt()?); continue; }
                TokenKind::Return => { stmts.push(self.parse_return_stmt()?); continue; }
                TokenKind::While => { stmts.push(self.parse_while_stmt()?); continue; }
                TokenKind::For => { stmts.push(self.parse_for_stmt()?); continue; }
                _ => {}
            }
            // Otherwise, parse an expression and decide stmt vs tail.
            let expr = self.parse_expr()?;
            if self.eat(&TokenKind::Semi) {
                let span = expr.span;
                stmts.push(Stmt { kind: StmtKind::Expr(expr), span });
            } else if self.at(&TokenKind::RBrace) {
                tail = Some(Box::new(expr));
            } else if is_block_like(&expr.kind) {
                let span = expr.span;
                stmts.push(Stmt { kind: StmtKind::Expr(expr), span });
            } else {
                return Err(self.err_at_peek("`;` or `}`"));
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Block { stmts, tail, span: start.merge(end) })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Let, "`let`")?.span;
        let mutable = self.eat(&TokenKind::Mut);
        let name = self.expect_ident()?;
        let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()?) } else { None };
        self.expect(&TokenKind::Eq, "`=`")?;
        let init = self.parse_expr()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Let { mutable, name, ty, init },
            span: start.merge(end),
        })
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Return, "`return`")?.span;
        let value = if self.at(&TokenKind::Semi) { None } else { Some(self.parse_expr()?) };
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt { kind: StmtKind::Return(value), span: start.merge(end) })
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::While, "`while`")?.span;
        let cond = self.with_no_struct_lit(|p| p.parse_expr())?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Stmt { kind: StmtKind::While { cond, body }, span })
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::For, "`for`")?.span;
        if self.eat(&TokenKind::LParen) {
            // C-style: for (init; cond; update) body
            let init = if self.at(&TokenKind::Semi) {
                None
            } else if matches!(self.peek_kind(), TokenKind::Let) {
                Some(Box::new(self.parse_let_no_semi()?))
            } else {
                let e = self.parse_expr()?;
                let span = e.span;
                Some(Box::new(Stmt { kind: StmtKind::Expr(e), span }))
            };
            self.expect(&TokenKind::Semi, "`;` in for header")?;
            let cond = if self.at(&TokenKind::Semi) { None } else { Some(self.parse_expr()?) };
            self.expect(&TokenKind::Semi, "`;` in for header")?;
            let mut update = Vec::new();
            if !self.at(&TokenKind::RParen) {
                update.push(self.parse_expr()?);
                while self.eat(&TokenKind::Comma) {
                    update.push(self.parse_expr()?);
                }
            }
            self.expect(&TokenKind::RParen, "`)`")?;
            let body = self.parse_block()?;
            let span = start.merge(body.span);
            return Ok(Stmt {
                kind: StmtKind::For(ForLoop::CStyle { init, cond, update, body }),
                span,
            });
        }
        // Range form: for ident in expr body
        let var = self.expect_ident()?;
        self.expect(&TokenKind::In, "`in`")?;
        let iter = self.with_no_struct_lit(|p| p.parse_expr())?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Stmt {
            kind: StmtKind::For(ForLoop::Range { var, iter, body }),
            span,
        })
    }

    fn parse_let_no_semi(&mut self) -> Result<Stmt, ParseError> {
        // Same as parse_let_stmt but without consuming a trailing `;` —
        // the for-header's `;` separator is consumed by the caller.
        let start = self.expect(&TokenKind::Let, "`let`")?.span;
        let mutable = self.eat(&TokenKind::Mut);
        let name = self.expect_ident()?;
        let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()?) } else { None };
        self.expect(&TokenKind::Eq, "`=`")?;
        let init = self.parse_expr()?;
        let span = start.merge(init.span);
        Ok(Stmt {
            kind: StmtKind::Let { mutable, name, ty, init },
            span,
        })
    }

    // ---- expressions: precedence climbing from low to high ----

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_assign()
    }

    fn parse_assign(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_range()?;
        if let Some(op) = peek_assign_op(self.peek_kind()) {
            self.bump();
            let rhs = self.parse_assign()?;
            let span = lhs.span.merge(rhs.span);
            return Ok(Expr {
                kind: ExprKind::Assign { op, target: Box::new(lhs), value: Box::new(rhs) },
                span,
            });
        }
        Ok(lhs)
    }

    fn parse_range(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_or()?;
        let inclusive = match self.peek_kind() {
            TokenKind::DotDot => false,
            TokenKind::DotDotEq => true,
            _ => return Ok(lhs),
        };
        self.bump();
        // RHS may or may not be present (e.g. `a..` is open-ended).
        let rhs = if can_start_expr(self.peek_kind()) {
            Some(Box::new(self.parse_or()?))
        } else {
            None
        };
        let span = match &rhs {
            Some(r) => lhs.span.merge(r.span),
            None => lhs.span,
        };
        Ok(Expr {
            kind: ExprKind::Range { start: Some(Box::new(lhs)), end: rhs, inclusive },
            span,
        })
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek_kind(), TokenKind::PipePipe) {
            self.bump();
            let rhs = self.parse_and()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op: BinOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_cmp()?;
        while matches!(self.peek_kind(), TokenKind::AmpAmp) {
            self.bump();
            let rhs = self.parse_cmp()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op: BinOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_bit_or()?;
        let Some(op) = peek_cmp_op(self.peek_kind()) else { return Ok(lhs); };
        self.bump();
        let rhs = self.parse_bit_or()?;
        if peek_cmp_op(self.peek_kind()).is_some() {
            return Err(ParseError {
                kind: ParseErrorKind::NonChainableComparison,
                span: self.peek().span,
            });
        }
        let span = lhs.span.merge(rhs.span);
        Ok(Expr {
            kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) },
            span,
        })
    }

    fn parse_bit_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_bit_xor()?;
        while matches!(self.peek_kind(), TokenKind::Pipe) {
            self.bump();
            let rhs = self.parse_bit_xor()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op: BinOp::BitOr, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_bit_xor(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_bit_and()?;
        while matches!(self.peek_kind(), TokenKind::Caret) {
            self.bump();
            let rhs = self.parse_bit_and()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op: BinOp::BitXor, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_bit_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_shift()?;
        while matches!(self.peek_kind(), TokenKind::Amp) {
            // Note: in expression context, `&` is bitwise. `&x` (reference) is handled in unary.
            self.bump();
            let rhs = self.parse_shift()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op: BinOp::BitAnd, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_shift(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_add()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Shr => BinOp::Shr,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_add()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                TokenKind::PlusPercent => BinOp::AddWrap,
                TokenKind::MinusPercent => BinOp::SubWrap,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_cast()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                TokenKind::StarPercent => BinOp::MulWrap,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_cast()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_cast(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_unary()?;
        while matches!(self.peek_kind(), TokenKind::As) {
            self.bump();
            let ty = self.parse_type()?;
            let span = e.span.merge(ty.span);
            e = Expr { kind: ExprKind::Cast { expr: Box::new(e), ty }, span };
        }
        Ok(e)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek().span;
        let op = match self.peek_kind() {
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Bang => Some(UnaryOp::Not),
            TokenKind::Tilde => Some(UnaryOp::BitNot),
            TokenKind::Star => Some(UnaryOp::Deref),
            TokenKind::Amp => {
                // `&` or `&mut` as a reference operator
                self.bump();
                let mutable = self.eat(&TokenKind::Mut);
                let operand = self.parse_unary()?;
                let span = start.merge(operand.span);
                return Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Ref { mutable },
                        operand: Box::new(operand),
                    },
                    span,
                });
            }
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let operand = self.parse_unary()?;
            let span = start.merge(operand.span);
            return Ok(Expr {
                kind: ExprKind::Unary { op, operand: Box::new(operand) },
                span,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek_kind() {
                TokenKind::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.at(&TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) { break; }
                    }
                    let end = self.expect(&TokenKind::RParen, "`)`")?.span;
                    let span = e.span.merge(end);
                    e = Expr { kind: ExprKind::Call { callee: Box::new(e), args }, span };
                }
                TokenKind::Dot => {
                    self.bump();
                    let name = self.expect_ident()?;
                    let span = e.span.merge(name.span);
                    e = Expr { kind: ExprKind::Field { receiver: Box::new(e), name }, span };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr()?;
                    let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                    let span = e.span.merge(end);
                    e = Expr {
                        kind: ExprKind::Index { receiver: Box::new(e), index: Box::new(index) },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::Int(v, suf) => {
                let v = *v; let suf = *suf;
                self.bump();
                Ok(Expr { kind: ExprKind::IntLit(v, suf), span: tok.span })
            }
            TokenKind::Float(v, suf) => {
                let v = *v; let suf = *suf;
                self.bump();
                Ok(Expr { kind: ExprKind::FloatLit(v, suf), span: tok.span })
            }
            TokenKind::True => { self.bump(); Ok(Expr { kind: ExprKind::BoolLit(true), span: tok.span }) }
            TokenKind::False => { self.bump(); Ok(Expr { kind: ExprKind::BoolLit(false), span: tok.span }) }
            TokenKind::SelfLower => {
                self.bump();
                // `self` in an expression is just an ident lookup; sema
                // registers it as a local inside method bodies.
                Ok(Expr {
                    kind: ExprKind::Ident("self".to_string()),
                    span: tok.span,
                })
            }
            TokenKind::Ident(s) => {
                let n = s.clone();
                self.bump();
                // Path expression: `Foo::Bar` (and future N-segment paths).
                if matches!(self.peek_kind(), TokenKind::ColonColon) {
                    let mut segments = vec![Ident { name: n, span: tok.span }];
                    while self.eat(&TokenKind::ColonColon) {
                        segments.push(self.expect_ident()?);
                    }
                    let span = tok.span.merge(segments.last().unwrap().span);
                    return Ok(Expr { kind: ExprKind::Path { segments }, span });
                }
                // Struct literal: `Foo { field: value, ... }` — only outside
                // the head of `if`/`while`/`for-in`, where `{` starts the body.
                if !self.no_struct_lit && matches!(self.peek_kind(), TokenKind::LBrace) {
                    self.bump(); // consume `{`
                    let mut fields = Vec::new();
                    while !self.at(&TokenKind::RBrace) {
                        let fname = self.expect_ident()?;
                        self.expect(&TokenKind::Colon, "`:`")?;
                        let value = self.parse_expr()?;
                        let span = fname.span.merge(value.span);
                        fields.push(StructLitField { name: fname, value, span });
                        if !self.eat(&TokenKind::Comma) { break; }
                    }
                    let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
                    let span = tok.span.merge(end);
                    return Ok(Expr {
                        kind: ExprKind::StructLit {
                            name: Ident { name: n, span: tok.span },
                            fields,
                        },
                        span,
                    });
                }
                Ok(Expr { kind: ExprKind::Ident(n), span: tok.span })
            }
            TokenKind::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(e)
            }
            TokenKind::LBracket => {
                let start = self.bump().span;
                let mut elements = Vec::new();
                while !self.at(&TokenKind::RBracket) {
                    elements.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) { break; }
                }
                let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                Ok(Expr {
                    kind: ExprKind::ArrayLit { elements },
                    span: start.merge(end),
                })
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                let span = block.span;
                Ok(Expr { kind: ExprKind::Block(block), span })
            }
            TokenKind::If => self.parse_if_expr(),
            _ => Err(self.err_at_peek("expression")),
        }
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::If, "`if`")?.span;
        let cond = self.with_no_struct_lit(|p| p.parse_expr())?;
        let then = self.parse_block()?;
        let else_branch = if self.eat(&TokenKind::Else) {
            if matches!(self.peek_kind(), TokenKind::If) {
                Some(Box::new(self.parse_if_expr()?))
            } else {
                let b = self.parse_block()?;
                let span = b.span;
                Some(Box::new(Expr { kind: ExprKind::Block(b), span }))
            }
        } else {
            None
        };
        let end = match &else_branch {
            Some(e) => e.span,
            None => then.span,
        };
        Ok(Expr {
            kind: ExprKind::If { cond: Box::new(cond), then, else_branch },
            span: start.merge(end),
        })
    }
}

// ---- helpers ----

fn is_block_like(kind: &ExprKind) -> bool {
    matches!(kind, ExprKind::Block(_) | ExprKind::If { .. })
}

fn can_start_expr(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Int(_, _)
            | TokenKind::Float(_, _)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Ident(_)
            | TokenKind::LParen
            | TokenKind::LBrace
            | TokenKind::If
            | TokenKind::Minus
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::Star
            | TokenKind::Amp
    )
}

fn peek_cmp_op(kind: &TokenKind) -> Option<BinOp> {
    Some(match kind {
        TokenKind::EqEq => BinOp::Eq,
        TokenKind::BangEq => BinOp::Ne,
        TokenKind::Lt => BinOp::Lt,
        TokenKind::Le => BinOp::Le,
        TokenKind::Gt => BinOp::Gt,
        TokenKind::Ge => BinOp::Ge,
        _ => return None,
    })
}

fn peek_assign_op(kind: &TokenKind) -> Option<AssignOp> {
    Some(match kind {
        TokenKind::Eq => AssignOp::Assign,
        TokenKind::PlusEq => AssignOp::AddAssign,
        TokenKind::MinusEq => AssignOp::SubAssign,
        TokenKind::StarEq => AssignOp::MulAssign,
        TokenKind::SlashEq => AssignOp::DivAssign,
        TokenKind::PercentEq => AssignOp::ModAssign,
        TokenKind::AmpEq => AssignOp::BitAndAssign,
        TokenKind::PipeEq => AssignOp::BitOrAssign,
        TokenKind::CaretEq => AssignOp::BitXorAssign,
        TokenKind::ShlEq => AssignOp::ShlAssign,
        TokenKind::ShrEq => AssignOp::ShrAssign,
        _ => return None,
    })
}

fn tok_name(k: &TokenKind) -> &'static str {
    match k {
        TokenKind::Int(_, _) => "integer literal",
        TokenKind::Float(_, _) => "float literal",
        TokenKind::Ident(_) => "identifier",
        TokenKind::Fn => "`fn`",
        TokenKind::Let => "`let`",
        TokenKind::Mut => "`mut`",
        TokenKind::If => "`if`",
        TokenKind::Else => "`else`",
        TokenKind::While => "`while`",
        TokenKind::For => "`for`",
        TokenKind::In => "`in`",
        TokenKind::Return => "`return`",
        TokenKind::True => "`true`",
        TokenKind::False => "`false`",
        TokenKind::As => "`as`",
        TokenKind::LParen => "`(`",
        TokenKind::RParen => "`)`",
        TokenKind::LBrace => "`{`",
        TokenKind::RBrace => "`}`",
        TokenKind::Comma => "`,`",
        TokenKind::Semi => "`;`",
        TokenKind::Colon => "`:`",
        TokenKind::Dot => "`.`",
        TokenKind::Arrow => "`->`",
        TokenKind::Eq => "`=`",
        TokenKind::EqEq => "`==`",
        TokenKind::Eof => "end of input",
        _ => "token",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn parse_src(src: &str) -> Result<Program, ParseError> {
        let toks = tokenize(src).expect("lex");
        parse(toks)
    }

    #[test]
    fn empty_program() {
        let p = parse_src("").unwrap();
        assert!(p.items.is_empty());
    }

    #[test]
    fn fn_returning_int_literal() {
        let p = parse_src("fn main() -> i32 { 0 }").unwrap();
        assert_eq!(p.items.len(), 1);
        let ItemKind::Function(f) = &p.items[0].kind else { panic!("expected fn"); };
        assert_eq!(f.name.name, "main");
        assert!(f.params.is_empty());
        match &f.return_type.as_ref().unwrap().kind {
            TypeKind::Path(s) => assert_eq!(s, "i32"),
            other => panic!("expected TypeKind::Path, got {other:?}"),
        }
        match &f.body.tail.as_ref().unwrap().kind {
            ExprKind::IntLit(0, _) => {}
            other => panic!("expected IntLit(0), got {other:?}"),
        }
    }

    #[test]
    fn factorial_parses() {
        let src = include_str!("../../docs/examples/factorial.cplus");
        let p = parse_src(src).unwrap();
        assert_eq!(p.items.len(), 2);
    }

    #[test]
    fn fibonacci_parses() {
        let src = include_str!("../../docs/examples/fibonacci.cplus");
        parse_src(src).unwrap();
    }

    #[test]
    fn sum_range_parses() {
        let src = include_str!("../../docs/examples/sum_range.cplus");
        parse_src(src).unwrap();
    }

    #[test]
    fn c_for_parses() {
        let src = include_str!("../../docs/examples/c_for.cplus");
        parse_src(src).unwrap();
    }

    #[test]
    fn non_chainable_comparison_rejected() {
        let err = parse_src("fn main() -> i32 { let r = 1 < 2 < 3; 0 }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::NonChainableComparison));
    }

    #[test]
    fn arithmetic_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let p = parse_src("fn main() -> i32 { 1 + 2 * 3 }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else { panic!("expected fn"); };
        let tail = f.body.tail.as_ref().unwrap();
        match &tail.kind {
            ExprKind::Binary { op: BinOp::Add, lhs, rhs } => {
                assert!(matches!(lhs.kind, ExprKind::IntLit(1, _)));
                match &rhs.kind {
                    ExprKind::Binary { op: BinOp::Mul, .. } => {}
                    other => panic!("rhs not Mul: {other:?}"),
                }
            }
            other => panic!("expected Add at top, got {other:?}"),
        }
    }

    #[test]
    fn assignment_is_right_associative() {
        // Note: parse-only; sema may reject this since chained assignment
        // typically requires a value-producing inner assign. Phase 1 is loose.
        let p = parse_src("fn main() -> i32 { let mut a: i32 = 0; let mut b: i32 = 0; a = b = 1; 0 }").unwrap();
        // The third stmt is `a = (b = 1)`.
        let ItemKind::Function(f) = &p.items[0].kind else { panic!("expected fn"); };
        let s = &f.body.stmts[2];
        match &s.kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Assign { value, .. } => match &value.kind {
                    ExprKind::Assign { .. } => {}
                    other => panic!("inner not Assign: {other:?}"),
                },
                other => panic!("outer not Assign: {other:?}"),
            },
            _ => panic!("not an expr stmt"),
        }
    }

    #[test]
    fn if_as_expression_in_let() {
        let p = parse_src("fn main() -> i32 { let x: i32 = if true { 1 } else { 2 }; x }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else { panic!("expected fn"); };
        match &f.body.stmts[0].kind {
            StmtKind::Let { init, .. } => match &init.kind {
                ExprKind::If { .. } => {}
                other => panic!("expected If in let init, got {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn if_at_stmt_position_no_semi_needed() {
        // Block-like exprs at stmt position need no `;`.
        parse_src("fn main() -> i32 { if true { } 0 }").unwrap();
    }

    #[test]
    fn range_lower_precedence_than_arithmetic() {
        // 0..n+1 must parse as 0..(n+1)
        let p = parse_src("fn main() -> i32 { for i in 0..10+1 { } 0 }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else { panic!("expected fn"); };
        match &f.body.stmts[0].kind {
            StmtKind::For(ForLoop::Range { iter, .. }) => match &iter.kind {
                ExprKind::Range { end: Some(end), inclusive: false, .. } => match &end.kind {
                    ExprKind::Binary { op: BinOp::Add, .. } => {}
                    other => panic!("range end not Add: {other:?}"),
                },
                other => panic!("not a Range: {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn missing_semicolon_errors() {
        let err = parse_src("fn main() -> i32 { let x = 1 0 }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn unmatched_brace_errors() {
        let err = parse_src("fn main() -> i32 { ").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedEof { .. }));
    }

    // ----- Phase 3 slice 3A: ownership markers on params and receivers -----

    fn first_function_params(src: &str) -> Vec<Param> {
        let prog = parse_src(src).unwrap();
        match &prog.items[0].kind {
            ItemKind::Function(f) => f.params.clone(),
            _ => panic!("expected Function"),
        }
    }

    fn first_method(src: &str) -> Method {
        let prog = parse_src(src).unwrap();
        for item in &prog.items {
            if let ItemKind::Impl(ib) = &item.kind {
                return ib.methods[0].clone();
            }
        }
        panic!("no Impl block found");
    }

    #[test]
    fn plain_param_has_no_ownership_markers() {
        let params = first_function_params("fn f(x: i32) -> i32 { return x; }");
        assert!(!params[0].mutable);
        assert!(!params[0].move_);
    }

    #[test]
    fn mut_param_sets_mutable_flag() {
        let params = first_function_params("fn f(mut x: i32) -> i32 { return x; }");
        assert!(params[0].mutable);
        assert!(!params[0].move_);
    }

    #[test]
    fn move_param_sets_move_flag() {
        let params = first_function_params("fn f(move x: i32) -> i32 { return x; }");
        assert!(!params[0].mutable);
        assert!(params[0].move_);
    }

    #[test]
    fn move_mut_param_sets_both_flags() {
        // Combo is permitted at parse time; sema rejects with E0334.
        let params = first_function_params("fn f(move mut x: i32) -> i32 { return x; }");
        assert!(params[0].mutable);
        assert!(params[0].move_);
    }

    #[test]
    fn mut_move_param_sets_both_flags() {
        let params = first_function_params("fn f(mut move x: i32) -> i32 { return x; }");
        assert!(params[0].mutable);
        assert!(params[0].move_);
    }

    #[test]
    fn move_self_receiver_parses() {
        let m = first_method("struct P { x: i32 } impl P { fn consume(move self) -> i32 { return self.x; } }");
        assert_eq!(m.receiver, Some(Receiver::Move));
    }

    #[test]
    fn self_receiver_parses() {
        let m = first_method("struct P { x: i32 } impl P { fn read(self) -> i32 { return self.x; } }");
        assert_eq!(m.receiver, Some(Receiver::Read));
    }

    #[test]
    fn mut_self_receiver_parses() {
        let m = first_method("struct P { x: i32 } impl P { fn write(mut self) { self.x = 0; } }");
        assert_eq!(m.receiver, Some(Receiver::Mut));
    }

    #[test]
    fn mut_move_self_is_parse_error() {
        let err = parse_src(
            "struct P { x: i32 } impl P { fn bad(mut move self) -> i32 { return 0; } }",
        )
        .unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn move_mut_self_is_parse_error() {
        let err = parse_src(
            "struct P { x: i32 } impl P { fn bad(move mut self) -> i32 { return 0; } }",
        )
        .unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }
}
