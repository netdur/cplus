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
    Unexpected {
        found: String,
        expected: &'static str,
    },
    UnexpectedEof {
        expected: &'static str,
    },
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
                write!(
                    f,
                    "comparison operators are non-chainable; use `&&` between comparisons"
                )
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
        Self {
            tokens,
            pos: 0,
            no_struct_lit: false,
        }
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

    fn peek_kind_n(&self, n: usize) -> &TokenKind {
        let idx = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    /// Slice 7GEN.5c: starting from the current position (which should
    /// be the opening `[`), scan past the matching `]` accounting for
    /// nested brackets. Returns the index of the token *after* the
    /// matching `]`, or `None` if no match was found before EOF.
    fn scan_past_bracket(&self) -> Option<usize> {
        debug_assert!(matches!(self.peek_kind(), TokenKind::LBracket));
        let mut depth: u32 = 0;
        let mut i = self.pos;
        while i < self.tokens.len() {
            match &self.tokens[i].kind {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                TokenKind::Eof => return None,
                _ => {}
            }
            i += 1;
        }
        None
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
        if self.at(k) {
            self.bump();
            true
        } else {
            false
        }
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
                Ok(Ident {
                    name: n,
                    span: tok.span,
                })
            }
            _ => Err(self.err_at_peek("identifier")),
        }
    }

    fn err_at_peek(&self, expected: &'static str) -> ParseError {
        let t = self.peek();
        if matches!(t.kind, TokenKind::Eof) {
            ParseError {
                kind: ParseErrorKind::UnexpectedEof { expected },
                span: t.span,
            }
        } else {
            ParseError {
                kind: ParseErrorKind::Unexpected {
                    found: tok_name(&t.kind).into(),
                    expected,
                },
                span: t.span,
            }
        }
    }

    // ---- top-level ----

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        // `import` declarations may only appear at the very top of the file,
        // before any item. Anything else after an item is parsed as an item
        // (so a stray `import` later produces "expected item" — see
        // `parse_item`).
        let mut imports = Vec::new();
        while matches!(self.peek_kind(), TokenKind::Import) {
            imports.push(self.parse_import_decl()?);
        }
        let mut items = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            items.push(self.parse_item()?);
        }
        Ok(Program { imports, items })
    }

    fn parse_import_decl(&mut self) -> Result<ImportDecl, ParseError> {
        let start = self.expect(&TokenKind::Import, "`import`")?.span;
        let path_tok = self.peek().clone();
        let path = match &path_tok.kind {
            TokenKind::Str(s) => s.clone(),
            _ => return Err(self.err_at_peek("string literal (import path)")),
        };
        self.bump();
        self.expect(&TokenKind::As, "`as`")?;
        let as_name = self.expect_ident()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(ImportDecl {
            path,
            as_name,
            span: start.merge(end),
        })
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        // Attribute prefix (slice 5ATTR.1) — zero or more `#[...]` blocks.
        // Attributes attach to the item that follows; impl blocks themselves
        // don't carry attributes in Phase 5 (per-method placement instead),
        // so an attribute followed by `impl` is rejected here.
        let attributes = self.parse_attributes()?;
        // Optional `pub` prefix (slice 4B). Stays attached to the item that
        // follows; `impl` blocks themselves don't take `pub` (per-method
        // `pub` instead), so a `pub impl` is rejected.
        let is_pub = self.eat(&TokenKind::Pub);
        match self.peek_kind() {
            TokenKind::Fn => self.parse_function(is_pub, attributes),
            // v0.0.3 Phase 5 Slice 5E.1: `async fn` item — `parse_function`
            // handles the `async`-prefix peek itself, but the top-level
            // item dispatch needs to recognise `async` as opening a fn item.
            TokenKind::Async => self.parse_function(is_pub, attributes),
            // v0.0.4 Phase 4 Slice 4A: `gen fn` item — generator coroutine.
            TokenKind::Gen => self.parse_function(is_pub, attributes),
            // Slice 10.FFI.1: `extern fn name(params) -> ret;` declarations.
            // Item-level only — no body, terminated by `;`. The lexer's
            // `Extern` keyword token has existed since Phase 1; this is
            // its first real consumer.
            TokenKind::Extern => self.parse_extern_fn(is_pub, attributes),
            TokenKind::Enum => self.parse_enum_decl(is_pub, attributes),
            TokenKind::Struct => self.parse_struct_decl(is_pub, attributes),
            // Slice 7GEN.3: interface declarations.
            TokenKind::Interface => self.parse_interface_decl(is_pub, attributes),
            // Phase 11 polish (2026-05-13): type aliases.
            TokenKind::TypeKw => self.parse_type_alias(is_pub, attributes),
            // v0.0.9 Phase 4: module-scope `const NAME: Ty = LIT;`.
            TokenKind::Const => self.parse_const_decl(is_pub, attributes),
            // v0.0.9 Phase 4: module-scope `static mut? NAME: Ty = LIT;`.
            TokenKind::Static => self.parse_static_decl(is_pub, attributes),
            TokenKind::Impl => {
                if is_pub {
                    return Err(self.err_at_peek(
                        "item — `impl` blocks don't take `pub`; mark individual methods inside the block instead",
                    ));
                }
                if !attributes.is_empty() {
                    return Err(self.err_at_peek(
                        "item — `impl` blocks don't carry attributes in Phase 5; attach the attribute to individual methods inside instead",
                    ));
                }
                self.parse_impl_block()
            }
            // `import` after the file's leading import block is a hard
            // error — call it out by name so the diagnostic explains the
            // restriction.
            TokenKind::Import => Err(self.err_at_peek(
                "item (`fn`, `enum`, `struct`, `impl`, or `interface`) — `import` declarations must appear at the top of the file before any item",
            )),
            _ => Err(self.err_at_peek("item (`fn`, `enum`, `struct`, `impl`, or `interface`)")),
        }
    }

    /// Parse zero or more `#[...]` attribute blocks at the current position.
    /// Each block is a single Attribute; the resulting Vec preserves source
    /// order. Slice 5ATTR.1.
    fn parse_attributes(&mut self) -> Result<Vec<Attribute>, ParseError> {
        let mut attrs = Vec::new();
        while matches!(self.peek_kind(), TokenKind::Pound) {
            attrs.push(self.parse_one_attribute()?);
        }
        Ok(attrs)
    }

    fn parse_one_attribute(&mut self) -> Result<Attribute, ParseError> {
        let start = self.expect(&TokenKind::Pound, "`#`")?.span;
        self.expect(&TokenKind::LBracket, "`[` after `#` (attribute opener)")?;
        let path = self.expect_ident()?;
        let mut args: Vec<AttrArg> = Vec::new();
        if self.eat(&TokenKind::LParen) {
            while !self.at(&TokenKind::RParen) {
                args.push(self.parse_attr_arg()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "`)` (attribute argument list)")?;
        } else if self.eat(&TokenKind::Eq) {
            // Phase 11 / ObjC interop: `#[name = "value"]` key-value form.
            // Used by `#[link_name = "..."]` for FFI symbol aliasing.
            // The value lowers to a single positional AttrArg::Str (or
            // AttrArg::Ident) so attrs-validation reads the same shape
            // it would from `#[name("value")]`.
            match self.peek_kind() {
                TokenKind::Str(_) => {
                    let tok = self.peek().clone();
                    let TokenKind::Str(s) = tok.kind else {
                        unreachable!()
                    };
                    self.bump();
                    args.push(AttrArg::Str(s, tok.span));
                }
                TokenKind::Ident(_) => {
                    let id = self.expect_ident()?;
                    args.push(AttrArg::Ident(id));
                }
                _ => {
                    return Err(
                        self.err_at_peek("string literal or identifier after `=` in attribute")
                    )
                }
            }
        }
        let end = self
            .expect(&TokenKind::RBracket, "`]` (attribute close)")?
            .span;
        Ok(Attribute {
            path,
            args,
            span: start.merge(end),
        })
    }

    fn parse_attr_arg(&mut self) -> Result<AttrArg, ParseError> {
        // Four shapes: bare ident, string literal, integer literal, or
        // `name = VALUE`. (Integer literal added in v0.0.7 Slice 1.3
        // for `#[unroll(N)]` / `#[vectorize_width(N)]`.)
        match self.peek_kind() {
            TokenKind::Int(..) => {
                let tok = self.peek().clone();
                let TokenKind::Int(v, _) = tok.kind else {
                    unreachable!()
                };
                self.bump();
                // Token's payload is u64; AttrArg::Int is i64 (loop
                // hints don't need values > i64::MAX). Cast is safe
                // since attribute ranges sema-validates downstream.
                Ok(AttrArg::Int(v as i64, tok.span))
            }
            TokenKind::Str(_) => {
                let tok = self.peek().clone();
                let TokenKind::Str(s) = tok.kind else {
                    unreachable!()
                };
                self.bump();
                Ok(AttrArg::Str(s, tok.span))
            }
            TokenKind::Ident(_) => {
                let name = self.expect_ident()?;
                if self.eat(&TokenKind::Eq) {
                    let value = match self.peek_kind() {
                        TokenKind::Str(_) => {
                            let tok = self.peek().clone();
                            let TokenKind::Str(s) = tok.kind else {
                                unreachable!()
                            };
                            self.bump();
                            AttrValue::Str(s, tok.span)
                        }
                        TokenKind::Ident(_) => AttrValue::Ident(self.expect_ident()?),
                        _ => {
                            return Err(self
                                .err_at_peek("string literal or identifier (attribute key=value)"))
                        }
                    };
                    Ok(AttrArg::KeyValue(name, value))
                } else {
                    Ok(AttrArg::Ident(name))
                }
            }
            _ => Err(self.err_at_peek(
                "attribute argument (identifier, string literal, or integer literal)",
            )),
        }
    }

    fn parse_impl_block(&mut self) -> Result<Item, ParseError> {
        let start = self.expect(&TokenKind::Impl, "`impl`")?.span;
        let first_ident = self.expect_ident()?;
        // Slice 7GEN.3: `impl Interface for Target { ... }` syntax.
        // The two ident shapes:
        //   - `impl Target { ... }`            → inherent impl
        //   - `impl Interface for Target { ... }` → interface impl
        // Disambiguate by peeking for the `for` keyword. C+ doesn't
        // have a free-standing `for` keyword in item position other
        // than as the for-loop statement opener inside fn bodies, so
        // `for` here unambiguously means interface-impl syntax.
        //
        // Slice 7GEN.5e: `impl Generic[T] { ... }` — the target carries
        // generic params. Type args appear on inherent impls only;
        // interface impls (`impl Iface for Target`) keep their plain
        // form for now (generic interface impls deferred).
        let (interface_name, target, target_generic_params) = if self.eat(&TokenKind::For) {
            let target = self.expect_ident()?;
            (Some(first_ident), target, Vec::new())
        } else {
            // Optional `[T, U]` after the target ident, declaring
            // impl-level generic params bound for all methods inside.
            let params = if self.at(&TokenKind::LBracket) {
                self.bump(); // `[`
                let mut params = Vec::new();
                while !self.at(&TokenKind::RBracket) {
                    let pname = self.expect_ident()?;
                    // Re-use the same bound-list shape as fn generic params.
                    let mut bounds = Vec::new();
                    if self.eat(&TokenKind::Colon) {
                        bounds.push(self.expect_ident()?);
                        while self.eat(&TokenKind::Plus) {
                            bounds.push(self.expect_ident()?);
                        }
                    }
                    let pspan = pname.span;
                    params.push(GenericParam {
                        name: pname,
                        bounds,
                        span: pspan,
                    });
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBracket, "`]`")?;
                if params.is_empty() {
                    return Err(
                        self.err_at_peek("expected at least one generic param inside `[ ]`")
                    );
                }
                params
            } else {
                Vec::new()
            };
            (None, first_ident, params)
        };
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut methods = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            methods.push(self.parse_method()?);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Item {
            kind: ItemKind::Impl(ImplBlock {
                target,
                target_generic_params,
                methods,
                interface_name,
            }),
            span: start.merge(end),
            origin_file: None,
        })
    }

    /// Slice 7GEN.3: parse an `interface Name { ... }` declaration.
    /// The body is a sequence of method-signature declarations
    /// `fn name(self, ...) -> T;` — note the trailing `;` in place of
    /// a method body. Receivers (`self` / `mut self` / `move self`)
    /// are admitted; no-receiver methods (associated functions) are
    /// also legal. `Self` may appear in parameter / return types
    /// (parsed as `TypeKind::Path("Self")` — sema substitutes at
    /// impl-resolution).
    fn parse_interface_decl(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        let start = self.expect(&TokenKind::Interface, "`interface`")?.span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut methods = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            methods.push(self.parse_interface_method()?);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Item {
            kind: ItemKind::Interface(InterfaceDecl {
                name,
                methods,
                is_pub,
                attributes,
            }),
            span: start.merge(end),
            origin_file: None,
        })
    }

    fn parse_interface_method(&mut self) -> Result<InterfaceMethod, ParseError> {
        let start = self.expect(&TokenKind::Fn, "`fn`")?.span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen, "`(`")?;
        let receiver = self.try_parse_receiver()?;
        let mut params = Vec::new();
        if receiver.is_some() {
            if self.eat(&TokenKind::Comma) {
                while !self.at(&TokenKind::RParen) {
                    params.push(self.parse_param()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
        } else {
            while !self.at(&TokenKind::RParen) {
                params.push(self.parse_param()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        let return_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        // Interface methods end with `;` — no body.
        let end = self
            .expect(
                &TokenKind::Semi,
                "`;` (interface method signatures have no body)",
            )?
            .span;
        Ok(InterfaceMethod {
            name,
            receiver,
            params,
            return_type,
            span: start.merge(end),
        })
    }

    fn parse_method(&mut self) -> Result<Method, ParseError> {
        // Per-method attributes (slice 5ATTR.1) then per-method `pub` (slice 4B).
        let attributes = self.parse_attributes()?;
        let is_pub = self.eat(&TokenKind::Pub);
        // v0.0.4 Phase 4 Slice 4E: `gen` (and `async`, when we land
        // async methods) modifier before `fn`. Methods can be generators
        // so `Vec[T]::iter(self) -> T` reads naturally.
        let is_async = self.eat(&TokenKind::Async);
        let is_gen = self.eat(&TokenKind::Gen);
        let start = self.expect(&TokenKind::Fn, "`fn`")?.span;
        let name = self.expect_ident()?;
        // Slice 7GEN.5e: optional `[T, U: Bound]` after the method name,
        // declaring method-level generic params. Re-uses the same shape
        // as `parse_generic_params` (which is fn-only); inlined here
        // to share the parsing logic.
        let generic_params = self.parse_generic_params()?;
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
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
        } else {
            while !self.at(&TokenKind::RParen) {
                params.push(self.parse_param()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
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
        Ok(Method {
            name,
            generic_params,
            receiver,
            params,
            return_type,
            body,
            span,
            is_pub,
            attributes,
            is_async,
            is_gen,
        })
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

    /// Phase 8 slice 8.STR.B.1: convert the lexer's raw interpolation parts
    /// into AST parts. Each `Expr` part is re-lexed + parsed as a fresh
    /// expression. Spans on the inner tokens are *relative to the inner
    /// source*, not the parent file — accepted as a v1 limitation; the
    /// design doc flags it (the parent token's span points at the whole
    /// literal, which most diagnostics will resolve to anyway).
    fn parse_interp_parts(
        &mut self,
        lex_parts: Vec<crate::lexer::InterpPart>,
        whole_span: Span,
    ) -> Result<Vec<crate::ast::InterpStrPart>, ParseError> {
        use crate::lexer::{tokenize, InterpPart};
        let mut out = Vec::with_capacity(lex_parts.len());
        for part in lex_parts {
            match part {
                InterpPart::Lit(s) => {
                    out.push(crate::ast::InterpStrPart::Lit(s));
                }
                InterpPart::Expr { source, span: _ } => {
                    let inner_tokens = tokenize(&source).map_err(|_e| ParseError {
                        // Use the whole-string span; precise inner-source
                        // span needs offset adjustment which v1 skips.
                        kind: ParseErrorKind::Unexpected {
                            found: "invalid expression".to_string(),
                            expected: "well-formed expression inside `${...}`",
                        },
                        span: whole_span,
                    })?;
                    let mut sub = Parser::new(inner_tokens);
                    let expr = sub.parse_expr()?;
                    // Confirm the parser consumed the full source —
                    // a trailing token would mean we accepted something
                    // like `${ 1 + 1 }; rest` which is malformed.
                    if !matches!(sub.peek_kind(), TokenKind::Eof) {
                        return Err(ParseError {
                            kind: ParseErrorKind::Unexpected {
                                found: "trailing tokens".to_string(),
                                expected: "end of expression inside `${...}`",
                            },
                            span: whole_span,
                        });
                    }
                    out.push(crate::ast::InterpStrPart::Expr(Box::new(expr)));
                }
            }
        }
        Ok(out)
    }

    /// Phase 11 polish (2026-05-13): `type Foo = Bar;` parser.
    /// Attributes are admitted at the surface but rejected here for now —
    /// there's no Phase-11 attribute that makes sense on aliases.
    fn parse_type_alias(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        if !attributes.is_empty() {
            return Err(self.err_at_peek("item — type aliases don't take attributes"));
        }
        let start = self.expect(&TokenKind::TypeKw, "`type`")?.span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Eq, "`=`")?;
        let target = self.parse_type()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Item {
            kind: ItemKind::TypeAlias(crate::ast::TypeAlias {
                name,
                target,
                is_pub,
            }),
            span: start.merge(end),
            origin_file: None,
        })
    }

    /// v0.0.9 Phase 4: parse `pub? const NAME: Ty = LIT;`.
    /// Type annotation is mandatory (E0X31); initializer must be a
    /// literal expression — the literal-shape check happens in sema
    /// (`check_const_static_inits`, E0X30) so the parser stays
    /// uniform and accepts any expression here.
    fn parse_const_decl(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        if !attributes.is_empty() {
            return Err(self.err_at_peek("item — `const` items don't take attributes in v0.0.9"));
        }
        let start = self.expect(&TokenKind::Const, "`const`")?.span;
        let name = self.expect_ident()?;
        // Type annotation is required — sema can't infer the binding's
        // type without an initializer, and we want the const's type to
        // be unambiguous at the declaration site for cross-file readers.
        self.expect(&TokenKind::Colon, "`:` (const requires explicit type annotation)")?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Eq, "`=`")?;
        let value = self.parse_expr()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Item {
            kind: ItemKind::Const(ConstDecl {
                name,
                ty,
                value,
                is_pub,
                attributes,
            }),
            span: start.merge(end),
            origin_file: None,
        })
    }

    /// v0.0.9 Phase 4: parse `pub? static mut? NAME: Ty = LIT;`.
    /// Same shape as `const` with an optional `mut` modifier between
    /// `static` and the name. Type annotation mandatory; literal-only
    /// initializer enforced by sema.
    fn parse_static_decl(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        if !attributes.is_empty() {
            return Err(self.err_at_peek("item — `static` items don't take attributes in v0.0.9"));
        }
        let start = self.expect(&TokenKind::Static, "`static`")?.span;
        let is_mut = self.eat(&TokenKind::Mut);
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Colon, "`:` (static requires explicit type annotation)")?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Eq, "`=`")?;
        let value = self.parse_expr()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Item {
            kind: ItemKind::Static(StaticDecl {
                name,
                ty,
                value,
                is_mut,
                is_pub,
                attributes,
            }),
            span: start.merge(end),
            origin_file: None,
        })
    }

    fn parse_struct_decl(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        let start = self.expect(&TokenKind::Struct, "`struct`")?.span;
        let name = self.expect_ident()?;
        // Slice 7GEN.2: optional generic-parameter list.
        let generic_params = self.parse_generic_params()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            // Per-field attributes (slice 5ATTR.1) then per-field `pub` (slice 4B).
            let field_attrs = self.parse_attributes()?;
            let field_pub = self.eat(&TokenKind::Pub);
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.parse_type()?;
            let span = fname.span.merge(ty.span);
            fields.push(StructField {
                name: fname,
                ty,
                span,
                is_pub: field_pub,
                attributes: field_attrs,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Item {
            kind: ItemKind::Struct(StructDecl {
                name,
                fields,
                is_pub,
                attributes,
                generic_params,
            }),
            span: start.merge(end),
            origin_file: None,
        })
    }

    fn parse_enum_decl(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        let start = self.expect(&TokenKind::Enum, "`enum`")?.span;
        let name = self.expect_ident()?;
        // Slice 7GEN.2: optional generic-parameter list.
        let generic_params = self.parse_generic_params()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut variants = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            // Per-variant attributes (slice 5ATTR.1).
            let variant_attrs = self.parse_attributes()?;
            let variant_name = self.expect_ident()?;
            // Optional positional payload: `Variant(T1, T2, ...)`.
            let payload = if self.eat(&TokenKind::LParen) {
                let mut tys = Vec::new();
                while !self.at(&TokenKind::RParen) {
                    tys.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "`)`")?;
                tys
            } else {
                Vec::new()
            };
            let v_span = variant_name.span;
            let span = match payload.last() {
                Some(t) => v_span.merge(t.span),
                None => v_span,
            };
            variants.push(EnumVariant {
                name: variant_name,
                payload,
                span,
                attributes: variant_attrs,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Item {
            kind: ItemKind::Enum(EnumDecl {
                name,
                variants,
                is_pub,
                attributes,
                generic_params,
            }),
            span: start.merge(end),
            origin_file: None,
        })
    }

    /// Slice 10.FFI.1: `extern fn name(params) -> ret;` — item-level
    /// foreign-function declaration, no body, terminated by `;`.
    /// Calling convention is C (`ccc`) — Phase 10 hardening will admit
    /// future explicit conventions like `extern "fastcall" fn ...`.
    /// Generic params and `pub` are rejected: an extern fn isn't
    /// monomorphizable (the C symbol is concrete), and `pub` is
    /// orthogonal to FFI visibility (the symbol is always reachable
    /// at the link level).
    fn parse_extern_fn(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        let start = self.peek().span;
        // Phase 11 / ObjC interop: attributes are now allowed on extern fns
        // (motivated by `#[link_name = "..."]`). Attribute-shape validation
        // runs in attrs::check; sema enforces extern-only semantic constraints
        // (e.g. link_name requires extern, repr does not apply to fns at all).
        self.expect(&TokenKind::Extern, "`extern`")?;
        self.expect(&TokenKind::Fn, "`fn`")?;
        let name = self.expect_ident()?;
        // Reject generic params explicitly; an extern symbol is concrete.
        if self.at(&TokenKind::LBracket) {
            return Err(self.err_at_peek(
                "extern fn — generic parameters are not allowed on extern declarations",
            ));
        }
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        // Slice 10.FFI.4: optional `, ...` after the last fixed param
        // makes this a variadic extern fn. `...` can only appear at
        // the end, and only on extern fns (no varargs for in-language
        // fns — they're a C ABI concession).
        let mut is_variadic = false;
        while !self.at(&TokenKind::RParen) {
            if self.at(&TokenKind::Ellipsis) {
                self.bump();
                is_variadic = true;
                break;
            }
            params.push(self.parse_param()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            // After the comma, allow `...` as the next thing.
            if self.at(&TokenKind::Ellipsis) {
                self.bump();
                is_variadic = true;
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        let return_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        // Phase 5 Slice 5.C: `pub extern fn NAME(...) { body }` defines an
        // export with C ABI. The two shapes are mutually exclusive:
        //   - `extern fn name(...);`       → import (declaration, no body)
        //   - `pub extern fn name(...) {}` → export (definition, has body)
        // Non-`pub extern fn ... {}` is rejected — the `pub` flag is what
        // distinguishes an export from an accidental body on an import.
        // Variadic + body is rejected because C+ has no `va_list` API to
        // walk the extra args; `printf`-style varargs is import-only.
        let (body, end_span, is_extern_def) = if self.at(&TokenKind::LBrace) {
            if !is_pub {
                return Err(self.err_at_peek(
                    "extern fn — only `pub extern fn` may have a body (exports a C-callable definition); plain `extern fn` is a declaration ending in `;`",
                ));
            }
            if is_variadic {
                return Err(self.err_at_peek(
                    "extern fn — variadic exports are not supported (C+ has no va_list); use `extern fn` with `;` to import a variadic C symbol instead",
                ));
            }
            let body = self.parse_block()?;
            let span = body.span;
            (body, span, true)
        } else {
            // Plain declaration form: `extern fn name(...);`. `pub` is
            // meaningful only on definitions (5.C exports). If the user
            // wrote `pub extern fn name(...);` they likely forgot a body.
            if is_pub {
                return Err(self.err_at_peek(
                    "extern fn — `pub` introduces a C-callable export and requires a body `{ ... }`; a declaration ends in `;` and must not be `pub`",
                ));
            }
            let semi = self.expect(&TokenKind::Semi, "`;` (extern declarations have no body; use `pub extern fn name(...) { body }` to define a C-callable export)")?;
            // Synthesize an empty block for the body field so every existing
            // AST-walking site keeps working without an Option<Block> migration.
            let body = Block {
                stmts: Vec::new(),
                tail: None,
                span: semi.span,
            };
            (body, semi.span, false)
        };
        Ok(Item {
            kind: ItemKind::Function(Function {
                name,
                params,
                return_type,
                body,
                // `pub` is meaningful when defining an export (5.C); it has
                // no effect on an import declaration (the C symbol is
                // always reachable). We preserve it on definitions so
                // sema can route the export-only checks, and on
                // declarations we drop it (pre-5.C behavior).
                is_pub: is_extern_def,
                is_extern: true,
                is_variadic,
                attributes,
                generic_params: Vec::new(),
                is_async: false,
                is_gen: false,
            }),
            span: start.merge(end_span),
            origin_file: None,
        })
    }

    fn parse_function(
        &mut self,
        is_pub: bool,
        attributes: Vec<Attribute>,
    ) -> Result<Item, ParseError> {
        let start = self.peek().span;
        // v0.0.3 Phase 5 Slice 5E.1: optional `async` modifier before `fn`.
        // `async fn` declares a coroutine whose declared return type T is
        // implicitly wrapped to `Future[T]` at sema (Slice 5E.2).
        let is_async = self.eat(&TokenKind::Async);
        // v0.0.4 Phase 4 Slice 4A: `gen fn` modifier. Mutually exclusive
        // with `async` — a fn is either coroutine-with-future-return or
        // coroutine-with-iterator-return, not both.
        let is_gen = self.eat(&TokenKind::Gen);
        self.expect(&TokenKind::Fn, "`fn`")?;
        let name = self.expect_ident()?;

        // Slice 7GEN.1: optional generic-parameter list `[T, U: Ord]`.
        let generic_params = self.parse_generic_params()?;

        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) {
            params.push(self.parse_param()?);
            if !self.eat(&TokenKind::Comma) {
                break;
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
        Ok(Item {
            kind: ItemKind::Function(Function {
                name,
                params,
                return_type,
                body,
                is_pub,
                is_extern: false,
                is_variadic: false,
                attributes,
                generic_params,
                is_async,
                is_gen,
            }),
            span,
            origin_file: None,
        })
    }

    /// Slice 7GEN.1: parse an optional generic-parameter list
    /// `[T, U: Bound1, V: Bound1 + Bound2]`. Returns an empty `Vec` when
    /// the next token isn't `[` (the common non-generic case).
    ///
    /// Each parameter is an identifier optionally followed by `:` and a
    /// `+`-separated list of bound identifiers. Trailing commas
    /// admitted. Empty `[]` rejected (parse error — would create an
    /// ambiguous syntactic placeholder for no purpose).
    fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>, ParseError> {
        if !self.at(&TokenKind::LBracket) {
            return Ok(Vec::new());
        }
        self.bump(); // `[`
        let mut params = Vec::new();
        while !self.at(&TokenKind::RBracket) {
            let pname = self.expect_ident()?;
            let mut bounds = Vec::new();
            if self.eat(&TokenKind::Colon) {
                bounds.push(self.expect_ident()?);
                while self.eat(&TokenKind::Plus) {
                    bounds.push(self.expect_ident()?);
                }
            }
            let span = bounds
                .last()
                .map_or(pname.span, |b| pname.span.merge(b.span));
            params.push(GenericParam {
                name: pname,
                bounds,
                span,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBracket, "`]`")?;
        if params.is_empty() {
            // Empty `[]` is a parse error — a generic parameter list
            // must declare at least one parameter (or be absent).
            return Err(self.err_at_peek("at least one generic parameter inside `[...]`"));
        }
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        // Optional ownership prefixes: `mut x: T`, `move x: T`,
        // `borrow x: T`, `restrict x: *T`, or combinations. Order
        // doesn't matter at the syntax layer; sema checks the
        // combination (E0334 for `mut` + `move` and analogous
        // `borrow` conflicts; E0411 for `restrict` on a
        // non-pointer type).
        //
        // The `borrow` marker is the v0.0.9 follow-up addition. It
        // distinguishes the type-position `borrow REGION T` (used
        // for region-annotated borrow types) from a parameter
        // marker — in this loop we only consume `borrow` when it
        // sits BEFORE the parameter name (look-ahead at the next
        // token to confirm it's an Ident or another marker).
        let mut mutable = false;
        let mut move_ = false;
        let mut restrict = false;
        let mut borrow_ = false;
        let start = self.peek().span;
        loop {
            match self.peek_kind() {
                TokenKind::Mut if !mutable => {
                    self.bump();
                    mutable = true;
                }
                TokenKind::Move if !move_ => {
                    self.bump();
                    move_ = true;
                }
                TokenKind::Restrict if !restrict => {
                    self.bump();
                    restrict = true;
                }
                TokenKind::Borrow if !borrow_ => {
                    // In parameter-prefix position `borrow` is
                    // unambiguously the param marker — the
                    // region-annotated `borrow REGION T` form only
                    // appears in TYPE position (after the colon),
                    // which parse_type handles. No look-ahead needed.
                    self.bump();
                    borrow_ = true;
                }
                _ => break,
            }
        }
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Colon, "`:`")?;
        let ty = self.parse_type()?;
        // Slice 6BC.5: `move x: borrow A T` is a parse error.
        // Ownership transfer doesn't borrow — the region annotation
        // is meaningless on a `move`-parameter. Reject here rather
        // than at sema so the diagnostic points at the syntax site.
        if move_ {
            if matches!(&ty.kind, TypeKind::Borrowed { .. }) {
                return Err(ParseError {
                    kind: ParseErrorKind::Unexpected {
                        found: "borrow region annotation".into(),
                        expected: "an unannotated type after `move` (borrow regions cannot apply to moved parameters)",
                    },
                    span: ty.span,
                });
            }
        }
        let span = start.merge(ty.span);
        Ok(Param {
            name,
            ty,
            mutable,
            move_,
            restrict,
            borrow_,
            span,
        })
    }

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        let tok = self.peek().clone();
        match &tok.kind {
            // Slice 6BC.5: `borrow REGION T` opens a region-annotated
            // borrow type. The region is an identifier (no specific
            // case rule — convention is short uppercase, e.g. `A`,
            // `B`, or descriptive `BUF`). Inner type is recursively
            // parsed so `borrow A [T; N]` and `borrow A prefix::T`
            // both work. Composition with parameter markers
            // (`mut`/`move`) happens at the param-parsing level, not
            // here — this only handles the type itself.
            TokenKind::Borrow => {
                let start = self.bump().span;
                let region = self.expect_ident()?;
                let inner = Box::new(self.parse_type()?);
                let end = inner.span;
                return Ok(Type {
                    kind: TypeKind::Borrowed {
                        region: region.name,
                        inner,
                    },
                    span: start.merge(end),
                });
            }
            // Slice 10.FFI.1: raw pointer `*T`. The `*` token is the
            // multiply operator everywhere else, but in *type position*
            // it unambiguously starts a pointer type (binary `*` lives
            // in expression position, never as a type). Composes
            // recursively: `**i32` is `*(*i32)`.
            TokenKind::Star => {
                let start = self.bump().span;
                let inner = Box::new(self.parse_type()?);
                let end = inner.span;
                return Ok(Type {
                    kind: TypeKind::RawPtr(inner),
                    span: start.merge(end),
                });
            }
            // Slice 11.FN_PTR: function pointer type — `fn(T1, T2, ...) -> R`
            // or `fn(...)` with implicit unit return. The `fn` token here
            // unambiguously starts a fn-pointer type because parse_type is
            // never called at an item-declaration site (item parsing handles
            // `fn name(...)` via its own path).
            // v0.0.5 Phase 3 Slice 3B: tuple type `(T1, T2, ...)`. The
            // unit type `()` is spelled `Path("()")` elsewhere in the
            // codebase; we only enter the LParen branch for arity ≥ 2.
            TokenKind::LParen => {
                let start = self.bump().span;
                let mut elems: Vec<Type> = Vec::new();
                while !self.at(&TokenKind::RParen) {
                    elems.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self
                    .expect(&TokenKind::RParen, "`)` (tuple type element list)")?
                    .span;
                if elems.len() < 2 {
                    return Err(ParseError {
                        kind: ParseErrorKind::Unexpected {
                            found: ")".to_string(),
                            expected: "tuple type element (arity ≥ 2)",
                        },
                        span: start.merge(end),
                    });
                }
                return Ok(Type {
                    kind: TypeKind::Tuple(elems),
                    span: start.merge(end),
                });
            }
            TokenKind::Fn => {
                let start = self.bump().span;
                self.expect(&TokenKind::LParen, "`(` after `fn` in fn-pointer type")?;
                let mut params: Vec<Type> = Vec::new();
                while !self.at(&TokenKind::RParen) {
                    params.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let rparen = self.expect(&TokenKind::RParen, "`)` (fn-pointer parameter list)")?;
                let mut end_span = rparen.span;
                let return_type = if self.eat(&TokenKind::Arrow) {
                    let rt = self.parse_type()?;
                    end_span = rt.span;
                    Some(Box::new(rt))
                } else {
                    None
                };
                return Ok(Type {
                    kind: TypeKind::FnPtr {
                        params,
                        return_type,
                    },
                    span: start.merge(end_span),
                });
            }
            TokenKind::Ident(s) => {
                let mut name = s.clone();
                let mut end_span = tok.span;
                self.bump();
                // Cross-file types use `prefix::Type` syntax (Phase 4). Store
                // the source form verbatim with `::` separators; the resolver
                // pass rewrites this to the qualified target during the
                // multi-file merge.
                while matches!(self.peek_kind(), TokenKind::ColonColon) {
                    self.bump();
                    let seg = self.expect_ident()?;
                    name.push_str("::");
                    name.push_str(&seg.name);
                    end_span = seg.span;
                }
                // Slice 7GEN.5c: `Name[T1, T2]` — generic-type instantiation
                // in type position. Disambiguates from array `[T; N]` by
                // virtue of being suffix to an ident; arrays start at `[`.
                //
                // Phase 11 polish (2026-05-14): `Name[]` (empty brackets)
                // is a *slice type* — a fat-pointer view of an unknown-
                // length contiguous run. Same surface position as Generic
                // but distinguished by the absence of args.
                if matches!(self.peek_kind(), TokenKind::LBracket) {
                    self.bump();
                    if self.at(&TokenKind::RBracket) {
                        let end = self.bump().span;
                        let elem = Type {
                            kind: TypeKind::Path(name),
                            span: tok.span.merge(end_span),
                        };
                        return Ok(Type {
                            kind: TypeKind::Slice(Box::new(elem)),
                            span: tok.span.merge(end),
                        });
                    }
                    let mut args = Vec::new();
                    while !self.at(&TokenKind::RBracket) {
                        args.push(self.parse_type()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                    return Ok(Type {
                        kind: TypeKind::Generic { name, args },
                        span: tok.span.merge(end),
                    });
                }
                Ok(Type {
                    kind: TypeKind::Path(name),
                    span: tok.span.merge(end_span),
                })
            }
            // Slice 7GEN.4: `Self` is a magic type name in `interface` and
            // `impl` contexts. Parse it as a path; sema (with the
            // self_type_stack / type_params_stack) decides whether it
            // resolves to a concrete type, stays abstract, or errors with
            // E0508 outside any impl/interface context.
            TokenKind::SelfUpper => {
                let span = self.bump().span;
                Ok(Type {
                    kind: TypeKind::Path("Self".to_string()),
                    span,
                })
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
                Ok(Type {
                    kind: TypeKind::Array { elem, len },
                    span: start.merge(end),
                })
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
            // v0.0.7 Slice 1.3: statement-level attributes precede a
            // loop keyword. Accept `#[NAME(ARGS)] while|loop|for ...`;
            // route the attribute list into the corresponding stmt-kind
            // variant so codegen can attach `!llvm.loop` metadata.
            if matches!(self.peek_kind(), TokenKind::Pound) {
                let attrs = self.parse_attributes()?;
                match self.peek_kind() {
                    TokenKind::While
                        if matches!(
                            self.tokens.get(self.pos + 1).map(|t| &t.kind),
                            Some(TokenKind::Let)
                        ) =>
                    {
                        // while-let attributes — lower to `loop { match }`
                        // attaches the attrs to the synthesized loop in
                        // crate::lower.
                        let mut s = self.parse_while_let_stmt()?;
                        if let StmtKind::WhileLet { .. } = &s.kind {
                            // attach via a wrapping Loop after lowering;
                            // for v0.0.7 we surface a parse error to
                            // keep the loop-attr surface minimal.
                            let _ = &attrs;
                            return Err(ParseError {
                                kind: ParseErrorKind::Unexpected {
                                    found: "while-let".into(),
                                    expected: "while|loop|for after #[loop-attr] (while-let not supported in v0.0.7)",
                                },
                                span: s.span,
                            });
                        }
                        let _ = &mut s;
                        unreachable!();
                    }
                    TokenKind::While => {
                        let s = self.parse_while_stmt_with_attrs(attrs)?;
                        stmts.push(s);
                        continue;
                    }
                    TokenKind::Loop => {
                        let s = self.parse_loop_stmt_with_attrs(attrs)?;
                        stmts.push(s);
                        continue;
                    }
                    TokenKind::For => {
                        let s = self.parse_for_stmt_with_attrs(attrs)?;
                        stmts.push(s);
                        continue;
                    }
                    _ => {
                        return Err(ParseError {
                            kind: ParseErrorKind::Unexpected {
                                found: tok_name(self.peek_kind()).into(),
                                expected: "while|loop|for after statement-level `#[...]`",
                            },
                            span: self.peek().span,
                        });
                    }
                }
            }
            // Statements introduced by a keyword always end with `;`.
            match self.peek_kind() {
                TokenKind::Let => {
                    stmts.push(self.parse_let_stmt()?);
                    continue;
                }
                TokenKind::Return => {
                    stmts.push(self.parse_return_stmt()?);
                    continue;
                }
                TokenKind::While
                    if matches!(
                        self.tokens.get(self.pos + 1).map(|t| &t.kind),
                        Some(TokenKind::Let)
                    ) =>
                {
                    stmts.push(self.parse_while_let_stmt()?);
                    continue;
                }
                TokenKind::While => {
                    stmts.push(self.parse_while_stmt()?);
                    continue;
                }
                TokenKind::For => {
                    stmts.push(self.parse_for_stmt()?);
                    continue;
                }
                TokenKind::Defer => {
                    stmts.push(self.parse_defer_stmt()?);
                    continue;
                }
                TokenKind::Guard => {
                    stmts.push(self.parse_guard_let_stmt()?);
                    continue;
                }
                TokenKind::Loop => {
                    stmts.push(self.parse_loop_stmt()?);
                    continue;
                }
                TokenKind::Break => {
                    stmts.push(self.parse_break_stmt()?);
                    continue;
                }
                TokenKind::Continue => {
                    stmts.push(self.parse_continue_stmt()?);
                    continue;
                }
                TokenKind::Assert => {
                    stmts.push(self.parse_assert_stmt()?);
                    continue;
                }
                // `if let PAT = E { ... }` is a statement (slice 4A.5);
                // plain `if EXPR { ... }` keeps its existing expression
                // path. The split is decided by lookahead one token after
                // `if`.
                TokenKind::If
                    if matches!(
                        self.tokens.get(self.pos + 1).map(|t| &t.kind),
                        Some(TokenKind::Let)
                    ) =>
                {
                    stmts.push(self.parse_if_let_stmt()?);
                    continue;
                }
                _ => {}
            }
            // Otherwise, parse an expression and decide stmt vs tail.
            let expr = self.parse_expr()?;
            if self.eat(&TokenKind::Semi) {
                let span = expr.span;
                stmts.push(Stmt {
                    kind: StmtKind::Expr(expr),
                    span,
                });
            } else if self.at(&TokenKind::RBrace) {
                tail = Some(Box::new(expr));
            } else if is_block_like(&expr.kind) {
                let span = expr.span;
                stmts.push(Stmt {
                    kind: StmtKind::Expr(expr),
                    span,
                });
            } else {
                return Err(self.err_at_peek("`;` or `}`"));
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Block {
            stmts,
            tail,
            span: start.merge(end),
        })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Let, "`let`")?.span;
        let mutable = self.eat(&TokenKind::Mut);
        let name = self.expect_ident()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        // Two forms: `let x: T = expr;` (initialized) and `let x: T;`
        // (uninitialized — sema's definite-assignment analysis verifies
        // every read is preceded by an assignment). Without a type
        // annotation the init form is required (sema can't infer).
        let init = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            },
            span: start.merge(end),
        })
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Return, "`return`")?.span;
        let value = if self.at(&TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Return(value),
            span: start.merge(end),
        })
    }

    fn parse_defer_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Defer, "`defer`")?.span;
        let expr = self.parse_expr()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Defer(expr),
            span: start.merge(end),
        })
    }

    /// `assert EXPR;` — Phase 5 slice 5ATTR.3. The expression must be a
    /// `bool` (verified at sema time). Codegen branches on the value and
    /// traps on the false path.
    fn parse_assert_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Assert, "`assert`")?.span;
        let expr = self.parse_expr()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Assert(expr),
            span: start.merge(end),
        })
    }

    /// `if let PATTERN = EXPR { BODY }` and the two-arm form with `else`.
    /// Slice 4A.5; lowered to `match` before sema.
    fn parse_if_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::If, "`if`")?.span;
        self.expect(&TokenKind::Let, "`let`")?;
        let pattern = self.parse_pattern()?;
        self.expect(&TokenKind::Eq, "`=`")?;
        // Same struct-lit disambiguation as `if EXPR { ... }`.
        let scrutinee = self.with_no_struct_lit(|p| p.parse_expr())?;
        let body = self.parse_block()?;
        let (else_body, end) = if self.eat(&TokenKind::Else) {
            // Two-arm `if let`: `else { ... }`. `else if` is not yet
            // supported on `if let` (the natural workaround is `else {
            // match scrutinee { ... } }` or a chain of `if let`s).
            let blk = self.parse_block()?;
            let s = blk.span;
            (Some(blk), s)
        } else {
            (None, body.span)
        };
        Ok(Stmt {
            kind: StmtKind::IfLet {
                pattern,
                scrutinee,
                body,
                else_body,
            },
            span: start.merge(end),
        })
    }

    /// `guard let PATTERN = EXPR else { ELSE };`
    /// `guard let PATTERN = EXPR else |COMPLEMENT_PATTERN| { ELSE };`
    /// Slice 4A.5.
    fn parse_guard_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Guard, "`guard`")?.span;
        self.expect(&TokenKind::Let, "`let`")?;
        let pattern = self.parse_pattern()?;
        self.expect(&TokenKind::Eq, "`=`")?;
        let scrutinee = self.with_no_struct_lit(|p| p.parse_expr())?;
        self.expect(&TokenKind::Else, "`else`")?;
        // Optional complement pattern: `else |PAT| { ... }`.
        let complement = if self.eat(&TokenKind::Pipe) {
            let p = self.parse_pattern()?;
            self.expect(&TokenKind::Pipe, "`|`")?;
            Some(p)
        } else {
            None
        };
        let else_body = self.parse_block()?;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::GuardLet {
                pattern,
                scrutinee,
                complement,
                else_body,
            },
            span: start.merge(end),
        })
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.parse_while_stmt_with_attrs(Vec::new())
    }

    fn parse_while_stmt_with_attrs(
        &mut self,
        attributes: Vec<Attribute>,
    ) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::While, "`while`")?.span;
        let cond = self.with_no_struct_lit(|p| p.parse_expr())?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Stmt {
            kind: StmtKind::While { cond, body, attributes },
            span,
        })
    }

    /// `while let PATTERN = SCRUTINEE { BODY }` — slice 4-end carry-forward
    /// from 4A.5. Lowered (in `crate::lower`) to `loop { match ... }`.
    fn parse_while_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::While, "`while`")?.span;
        self.expect(&TokenKind::Let, "`let`")?;
        let pattern = self.parse_pattern()?;
        self.expect(&TokenKind::Eq, "`=`")?;
        let scrutinee = self.with_no_struct_lit(|p| p.parse_expr())?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Stmt {
            kind: StmtKind::WhileLet {
                pattern,
                scrutinee,
                body,
            },
            span,
        })
    }

    /// `loop { BODY }` — infinite loop. Exits only via `break`,
    /// `return`, or a no-return call.
    fn parse_loop_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.parse_loop_stmt_with_attrs(Vec::new())
    }

    fn parse_loop_stmt_with_attrs(
        &mut self,
        attributes: Vec<Attribute>,
    ) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Loop, "`loop`")?.span;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Ok(Stmt {
            kind: StmtKind::Loop(body, attributes),
            span,
        })
    }

    /// `break;` — slice 4-end. Phase 4 has no labelled-break / break-with-value.
    fn parse_break_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Break, "`break`")?.span;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Break,
            span: start.merge(end),
        })
    }

    /// `continue;` — slice 4-end.
    fn parse_continue_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Continue, "`continue`")?.span;
        let end = self.expect(&TokenKind::Semi, "`;`")?.span;
        Ok(Stmt {
            kind: StmtKind::Continue,
            span: start.merge(end),
        })
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.parse_for_stmt_with_attrs(Vec::new())
    }

    fn parse_for_stmt_with_attrs(
        &mut self,
        attributes: Vec<Attribute>,
    ) -> Result<Stmt, ParseError> {
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
                Some(Box::new(Stmt {
                    kind: StmtKind::Expr(e),
                    span,
                }))
            };
            self.expect(&TokenKind::Semi, "`;` in for header")?;
            let cond = if self.at(&TokenKind::Semi) {
                None
            } else {
                Some(self.parse_expr()?)
            };
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
                kind: StmtKind::For(
                    ForLoop::CStyle {
                        init,
                        cond,
                        update,
                        body,
                    },
                    attributes,
                ),
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
            kind: StmtKind::For(ForLoop::Range { var, iter, body }, attributes),
            span,
        })
    }

    fn parse_let_no_semi(&mut self) -> Result<Stmt, ParseError> {
        // Same as parse_let_stmt but without consuming a trailing `;` —
        // the for-header's `;` separator is consumed by the caller. The
        // for-header form always requires an initializer (the natural
        // pattern is `for (let mut i: i32 = 0; ...)`); uninitialized lets
        // inside a for-header would be useless.
        let start = self.expect(&TokenKind::Let, "`let`")?.span;
        let mutable = self.eat(&TokenKind::Mut);
        let name = self.expect_ident()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq, "`=`")?;
        let init = self.parse_expr()?;
        let span = start.merge(init.span);
        Ok(Stmt {
            kind: StmtKind::Let {
                mutable,
                name,
                ty,
                init: Some(init),
            },
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
                kind: ExprKind::Assign {
                    op,
                    target: Box::new(lhs),
                    value: Box::new(rhs),
                },
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
            kind: ExprKind::Range {
                start: Some(Box::new(lhs)),
                end: rhs,
                inclusive,
            },
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
                kind: ExprKind::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
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
                kind: ExprKind::Binary {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_bit_or()?;
        let Some(op) = peek_cmp_op(self.peek_kind()) else {
            return Ok(lhs);
        };
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
            kind: ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
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
                kind: ExprKind::Binary {
                    op: BinOp::BitOr,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
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
                kind: ExprKind::Binary {
                    op: BinOp::BitXor,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
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
                kind: ExprKind::Binary {
                    op: BinOp::BitAnd,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
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
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
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
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
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
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_cast(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_unary()?;
        while matches!(self.peek_kind(), TokenKind::As) {
            self.bump();
            let ty = self.parse_type()?;
            let span = e.span.merge(ty.span);
            e = Expr {
                kind: ExprKind::Cast {
                    expr: Box::new(e),
                    ty,
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek().span;
        // v0.0.3 Phase 5 Slice 5E.1: prefix `await EXPR`. Parse as a
        // unary-precedence operator so `await foo().bar` parses as
        // `await (foo().bar)`, not `(await foo()).bar`. Sema (5E.2)
        // enforces that the inner expr resolves to a `Future[T]` and
        // that the surrounding fn is `async`.
        if matches!(self.peek_kind(), TokenKind::Await) {
            self.bump();
            let operand = self.parse_unary()?;
            let span = start.merge(operand.span);
            return Ok(Expr {
                kind: ExprKind::Await(Box::new(operand)),
                span,
            });
        }
        // v0.0.4 Phase 4 Slice 4A: `yield EXPR` — same precedence shape
        // as `await`. Sema enforces that the surrounding fn is `gen` and
        // that the value type matches the iterator's T. Lexer-keyword;
        // not usable as an identifier.
        if matches!(self.peek_kind(), TokenKind::Yield) {
            self.bump();
            let operand = self.parse_unary()?;
            let span = start.merge(operand.span);
            return Ok(Expr {
                kind: ExprKind::Yield(Box::new(operand)),
                span,
            });
        }
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
                kind: ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
                span,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_primary()?;
        loop {
            // Slice 7GEN.5b: detect the `::[T1, T2]` turbofish.
            // Slice 7GEN.5e: widened from `Ident`-only to also admit
            // `Path` (assoc-fn call: `Type::method::[T]()`) and
            // `Field` (method call: `v.method::[T]()`) callees.
            // Plain bracket-expression accesses (`a[i]`) don't trigger
            // the turbofish path because they don't see `::` in front.
            if matches!(self.peek_kind(), TokenKind::ColonColon)
                && matches!(
                    e.kind,
                    ExprKind::Ident(_) | ExprKind::Path { .. } | ExprKind::Field { .. }
                )
                && self.peek_kind_n(1) == &TokenKind::LBracket
            {
                self.bump(); // `::`
                self.bump(); // `[`
                let mut type_args = Vec::new();
                while !self.at(&TokenKind::RBracket) {
                    type_args.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBracket, "`]`")?;
                // After the turbofish, a `(args)` is required.
                self.expect(&TokenKind::LParen, "`(` after `::[...]` turbofish")?;
                let mut args = Vec::new();
                while !self.at(&TokenKind::RParen) {
                    args.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect(&TokenKind::RParen, "`)`")?.span;
                let span = e.span.merge(end);
                e = Expr {
                    kind: ExprKind::Call {
                        callee: Box::new(e),
                        args,
                        type_args,
                    },
                    span,
                };
                continue;
            }
            match self.peek_kind() {
                TokenKind::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.at(&TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(&TokenKind::RParen, "`)`")?.span;
                    let span = e.span.merge(end);
                    e = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(e),
                            args,
                            type_args: Vec::new(),
                        },
                        span,
                    };
                }
                TokenKind::Dot => {
                    self.bump();
                    // v0.0.5 Phase 3 Slice 3B: accept numeric field
                    // access (`pair.0`, `pair.1`) by rewriting the
                    // literal to a synthetic `_N` ident matching the
                    // tuple struct's field names. Decimal-only — no
                    // `0x` / `0b` prefixes, no suffixes (sema rejects
                    // suffix-bearing tokens here implicitly via the
                    // expect_ident fall-through).
                    let tok = self.peek().clone();
                    let name = if let TokenKind::Int(n, _) = &tok.kind {
                        self.bump();
                        Ident {
                            name: format!("_{}", n),
                            span: tok.span,
                        }
                    } else {
                        self.expect_ident()?
                    };
                    let span = e.span.merge(name.span);
                    e = Expr {
                        kind: ExprKind::Field {
                            receiver: Box::new(e),
                            name,
                        },
                        span,
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr()?;
                    let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                    let span = e.span.merge(end);
                    e = Expr {
                        kind: ExprKind::Index {
                            receiver: Box::new(e),
                            index: Box::new(index),
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// Parse the body of a struct literal — `{ field: value, ... }` — given
    /// a type-name `Ident` that was just consumed. `start_span` is the span
    /// of the first source token of the literal (used to anchor the overall
    /// `Expr` span). Caller has already verified that the next token is `{`
    /// and that `no_struct_lit` is off.
    /// Slice 7GEN.5c: parse the `{ field: value, ... }` body of a
    /// generic struct literal `Pair[i32, bool] { ... }`. The caller
    /// has already consumed the type-arg brackets.
    fn parse_generic_struct_lit_body(
        &mut self,
        name: Ident,
        type_args: Vec<Type>,
        start_span: Span,
    ) -> Result<Expr, ParseError> {
        self.bump(); // consume `{`
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let value = self.parse_expr()?;
            let span = fname.span.merge(value.span);
            fields.push(StructLitField {
                name: fname,
                value,
                span,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        let span = start_span.merge(end);
        Ok(Expr {
            kind: ExprKind::GenericStructLit {
                name,
                type_args,
                fields,
            },
            span,
        })
    }

    fn parse_struct_lit_body(&mut self, name: Ident, start_span: Span) -> Result<Expr, ParseError> {
        self.bump(); // consume `{`
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let value = self.parse_expr()?;
            let span = fname.span.merge(value.span);
            fields.push(StructLitField {
                name: fname,
                value,
                span,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        let span = start_span.merge(end);
        Ok(Expr {
            kind: ExprKind::StructLit { name, fields },
            span,
        })
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::Int(v, suf) => {
                let v = *v;
                let suf = *suf;
                self.bump();
                Ok(Expr {
                    kind: ExprKind::IntLit(v, suf),
                    span: tok.span,
                })
            }
            TokenKind::Float(v, suf) => {
                let v = *v;
                let suf = *suf;
                self.bump();
                Ok(Expr {
                    kind: ExprKind::FloatLit(v, suf),
                    span: tok.span,
                })
            }
            TokenKind::True => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::BoolLit(true),
                    span: tok.span,
                })
            }
            TokenKind::False => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::BoolLit(false),
                    span: tok.span,
                })
            }
            TokenKind::Str(s) => {
                let s = s.clone();
                self.bump();
                Ok(Expr {
                    kind: ExprKind::StrLit(s),
                    span: tok.span,
                })
            }
            TokenKind::InterpStr(lex_parts) => {
                let lex_parts = lex_parts.clone();
                self.bump();
                let parts = self.parse_interp_parts(lex_parts, tok.span)?;
                Ok(Expr {
                    kind: ExprKind::InterpStr { parts },
                    span: tok.span,
                })
            }
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
                // v0.0.6 Slice 1A / v0.0.7 Slice 3.1: `include_bytes!`
                // and `include_str!` compiler builtins. Routed before any
                // other postfix handling. The `!` marks the (only) builtin
                // macro form; sema/codegen treat the result as either an
                // opaque pointer-to-byte-array constant (bytes) or a `str`
                // fat-pointer view (str). The form is strict: `Ident(name)
                // + ! + ( + StringLit + )`. Any deviation is a parse error.
                let is_include_bytes = n == "include_bytes";
                let is_include_str = n == "include_str";
                let is_env_var = n == "env";
                if (is_include_bytes || is_include_str || is_env_var)
                    && matches!(self.peek_kind(), TokenKind::Bang)
                {
                    let macro_name = if is_include_bytes {
                        "include_bytes"
                    } else if is_include_str {
                        "include_str"
                    } else {
                        "env"
                    };
                    let open_paren_msg = if is_include_bytes {
                        "`(` after `include_bytes!`"
                    } else if is_include_str {
                        "`(` after `include_str!`"
                    } else {
                        "`(` after `env!`"
                    };
                    let arg_msg = if is_include_bytes {
                        "string literal path in `include_bytes!`"
                    } else if is_include_str {
                        "string literal path in `include_str!`"
                    } else {
                        "string literal environment variable name in `env!`"
                    };
                    let close_paren_msg = if is_include_bytes {
                        "`)` after `include_bytes!` path"
                    } else if is_include_str {
                        "`)` after `include_str!` path"
                    } else {
                        "`)` after `env!` name"
                    };
                    self.bump(); // `!`
                    self.expect(&TokenKind::LParen, open_paren_msg)?;
                    let path_tok = self.peek().clone();
                    let arg_str = match &path_tok.kind {
                        TokenKind::Str(s) => {
                            let s = s.clone();
                            self.bump();
                            s
                        }
                        _ => {
                            return Err(ParseError {
                                kind: ParseErrorKind::Unexpected {
                                    found: tok_name(&path_tok.kind).into(),
                                    expected: arg_msg,
                                },
                                span: path_tok.span,
                            })
                        }
                    };
                    let end = self.expect(&TokenKind::RParen, close_paren_msg)?.span;
                    let _ = macro_name;
                    let kind = if is_include_bytes {
                        ExprKind::IncludeBytes { path: arg_str }
                    } else if is_include_str {
                        ExprKind::IncludeStr { path: arg_str }
                    } else {
                        ExprKind::EnvVar { name: arg_str }
                    };
                    return Ok(Expr {
                        kind,
                        span: tok.span.merge(end),
                    });
                }
                // Slice 7GEN.5b: if the next tokens are `::[`, leave them
                // for `parse_postfix` to consume as a turbofish — don't
                // start a path. A bare `Ident` with the cursor still on
                // `::` works as long as parse_postfix runs immediately.
                if matches!(self.peek_kind(), TokenKind::ColonColon)
                    && self.peek_kind_n(1) == &TokenKind::LBracket
                {
                    return Ok(Expr {
                        kind: ExprKind::Ident(n),
                        span: tok.span,
                    });
                }
                // Path expression: `Foo::Bar` (and future N-segment paths).
                if matches!(self.peek_kind(), TokenKind::ColonColon) {
                    let mut segments = vec![Ident {
                        name: n,
                        span: tok.span,
                    }];
                    // Slice 7GEN.5e: stop segment collection at `::[`
                    // so `parse_postfix` can pick it up as a turbofish
                    // on the path callee (`Foo::bar::[i32](...)`).
                    while matches!(self.peek_kind(), TokenKind::ColonColon)
                        && self.peek_kind_n(1) != &TokenKind::LBracket
                    {
                        self.bump(); // `::`
                        segments.push(self.expect_ident()?);
                    }
                    let last_span = segments.last().unwrap().span;
                    let span = tok.span.merge(last_span);
                    // Slice 4C: cross-file struct literal `prefix::Type { ... }`.
                    // After collecting all `::`-segments, if the next token
                    // is `{` and we're not in a struct-lit-suppressing head
                    // position, treat the qualified name as a struct literal
                    // type. The resolver later splits the `::`-joined name
                    // and applies the import-alias rewrite.
                    if !self.no_struct_lit && matches!(self.peek_kind(), TokenKind::LBrace) {
                        let qualified: String = segments
                            .iter()
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join("::");
                        let name_ident = Ident {
                            name: qualified,
                            span,
                        };
                        return self.parse_struct_lit_body(name_ident, tok.span);
                    }
                    // v0.0.3 1P.1: qualified generic constructor —
                    // `mod::Enum[A, B]::Variant(args)` (enum) or
                    // `mod::Struct[A, B] { ... }` (struct lit). After
                    // collecting all `::`-segments (last is the enum/struct
                    // name), peek past the matching `]` for `::` (enum) or
                    // `{` (struct lit). Mirrors the bare-Ident paths at
                    // lines 1707 and 1816 below; they only triggered on
                    // unqualified names before this slice.
                    if matches!(self.peek_kind(), TokenKind::LBracket) {
                        if let Some(after_bracket) = self.scan_past_bracket() {
                            if matches!(&self.tokens[after_bracket].kind, TokenKind::ColonColon) {
                                self.bump(); // `[`
                                let mut type_args = Vec::new();
                                while !self.at(&TokenKind::RBracket) {
                                    type_args.push(self.parse_type()?);
                                    if !self.eat(&TokenKind::Comma) {
                                        break;
                                    }
                                }
                                self.expect(&TokenKind::RBracket, "`]`")?;
                                self.expect(&TokenKind::ColonColon, "`::`")?;
                                let variant = self.expect_ident()?;
                                let mut args = Vec::new();
                                let end_span = if self.eat(&TokenKind::LParen) {
                                    while !self.at(&TokenKind::RParen) {
                                        args.push(self.parse_expr()?);
                                        if !self.eat(&TokenKind::Comma) {
                                            break;
                                        }
                                    }
                                    self.expect(&TokenKind::RParen, "`)`")?.span
                                } else {
                                    variant.span
                                };
                                let qualified: String = segments
                                    .iter()
                                    .map(|s| s.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join("::");
                                return Ok(Expr {
                                    kind: ExprKind::GenericEnumCall {
                                        enum_name: Ident {
                                            name: qualified,
                                            span,
                                        },
                                        type_args,
                                        variant,
                                        args,
                                    },
                                    span: tok.span.merge(end_span),
                                });
                            }
                            if !self.no_struct_lit
                                && matches!(&self.tokens[after_bracket].kind, TokenKind::LBrace)
                            {
                                self.bump(); // `[`
                                let mut type_args = Vec::new();
                                while !self.at(&TokenKind::RBracket) {
                                    type_args.push(self.parse_type()?);
                                    if !self.eat(&TokenKind::Comma) {
                                        break;
                                    }
                                }
                                let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                                let qualified: String = segments
                                    .iter()
                                    .map(|s| s.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join("::");
                                let name_ident = Ident {
                                    name: qualified,
                                    span,
                                };
                                let lit_span = tok.span.merge(end);
                                return self.parse_generic_struct_lit_body(
                                    name_ident, type_args, lit_span,
                                );
                            }
                        }
                    }
                    return Ok(Expr {
                        kind: ExprKind::Path { segments },
                        span,
                    });
                }
                // Slice 7GEN.5d: generic enum constructor `Option[i32]::Some(7)`
                // in expression position. Detected by `Ident[type_args]::`
                // peek pattern — must be a type-position generic followed by
                // a variant path. Distinguished from indexing (`a[i]` has no
                // `::` follow-up).
                if matches!(self.peek_kind(), TokenKind::LBracket) {
                    if let Some(after_bracket) = self.scan_past_bracket() {
                        if matches!(&self.tokens[after_bracket].kind, TokenKind::ColonColon) {
                            // `Ident[args]::Variant` — generic enum call.
                            self.bump(); // `[`
                            let mut type_args = Vec::new();
                            while !self.at(&TokenKind::RBracket) {
                                type_args.push(self.parse_type()?);
                                if !self.eat(&TokenKind::Comma) {
                                    break;
                                }
                            }
                            self.expect(&TokenKind::RBracket, "`]`")?;
                            self.expect(&TokenKind::ColonColon, "`::`")?;
                            let variant = self.expect_ident()?;
                            let mut args = Vec::new();
                            let end_span = if self.eat(&TokenKind::LParen) {
                                while !self.at(&TokenKind::RParen) {
                                    args.push(self.parse_expr()?);
                                    if !self.eat(&TokenKind::Comma) {
                                        break;
                                    }
                                }
                                self.expect(&TokenKind::RParen, "`)`")?.span
                            } else {
                                variant.span
                            };
                            return Ok(Expr {
                                kind: ExprKind::GenericEnumCall {
                                    enum_name: Ident {
                                        name: n,
                                        span: tok.span,
                                    },
                                    type_args,
                                    variant,
                                    args,
                                },
                                span: tok.span.merge(end_span),
                            });
                        }
                    }
                }
                // Slice 7GEN.5c: generic struct literal `Pair[i32, i32] { ... }`
                // in expression position. Distinguished from `arr[i]` indexing
                // by peeking past the matching `]` for a `{`. If found, the
                // brackets carry type args, not an index expression.
                if !self.no_struct_lit && matches!(self.peek_kind(), TokenKind::LBracket) {
                    if let Some(after_bracket) = self.scan_past_bracket() {
                        if matches!(&self.tokens[after_bracket].kind, TokenKind::LBrace) {
                            // Consume `[`, parse type args, consume `]`, then
                            // bake the generic name and call struct-lit body.
                            self.bump(); // `[`
                            let mut args = Vec::new();
                            while !self.at(&TokenKind::RBracket) {
                                args.push(self.parse_type()?);
                                if !self.eat(&TokenKind::Comma) {
                                    break;
                                }
                            }
                            let end = self.expect(&TokenKind::RBracket, "`]`")?.span;
                            // Encode the generic name as a synthetic Ident
                            // whose `name` field carries the mangled
                            // post-monomorphize identifier — but we don't
                            // yet know the mangled form at parse time.
                            // Instead, embed a special marker the resolver
                            // and sema will recognize: `__generic_lit__`
                            // is a sentinel here is overkill. Cleaner:
                            // extend ExprKind with a new variant.
                            //
                            // For 7GEN.5c MVP: synthesize the AST shape
                            // by parsing the body, then wrap into a
                            // dedicated generic-struct-lit form. We'll
                            // add `ExprKind::GenericStructLit` for this.
                            let name_ident = Ident {
                                name: n,
                                span: tok.span,
                            };
                            let span = tok.span.merge(end);
                            return self.parse_generic_struct_lit_body(name_ident, args, span);
                        }
                    }
                }
                // Struct literal: `Foo { field: value, ... }` — only outside
                // the head of `if`/`while`/`for-in`, where `{` starts the body.
                if !self.no_struct_lit && matches!(self.peek_kind(), TokenKind::LBrace) {
                    let name_ident = Ident {
                        name: n,
                        span: tok.span,
                    };
                    return self.parse_struct_lit_body(name_ident, tok.span);
                }
                Ok(Expr {
                    kind: ExprKind::Ident(n),
                    span: tok.span,
                })
            }
            TokenKind::LParen => {
                let lparen = self.bump().span;
                // v0.0.5 Phase 3 Slice 3B: distinguish tuple literal
                // `(a, b, ...)` from grouping `(a)`. Look ahead one
                // expression and check for `,`: if present, it's a
                // tuple. `()` (empty parens) would be the unit value
                // but the parser doesn't accept that today — `()` is
                // strictly a type-position spelling.
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Comma) {
                    let mut elements: Vec<Expr> = vec![first];
                    while !self.at(&TokenKind::RParen) {
                        elements.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(&TokenKind::RParen, "`)`")?.span;
                    return Ok(Expr {
                        kind: ExprKind::TupleLit { elements },
                        span: lparen.merge(end),
                    });
                }
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(first)
            }
            TokenKind::LBracket => {
                let start = self.bump().span;
                let mut elements = Vec::new();
                while !self.at(&TokenKind::RBracket) {
                    elements.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
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
                Ok(Expr {
                    kind: ExprKind::Block(block),
                    span,
                })
            }
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            // Slice 10.FFI.3: `unsafe { ... }` block expression.
            // Body parses like a regular block; the marker tells sema
            // to allow pointer deref / extern calls / etc. inside.
            TokenKind::Unsafe => {
                let start = self.expect(&TokenKind::Unsafe, "`unsafe`")?.span;
                let block = self.parse_block()?;
                let span = start.merge(block.span);
                Ok(Expr {
                    kind: ExprKind::Unsafe(block),
                    span,
                })
            }
            _ => Err(self.err_at_peek("expression")),
        }
    }

    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::Match, "`match`")?.span;
        // Scrutinee parses with struct-lit disambiguation off (so `match x {`
        // doesn't greedy-eat the brace as a struct literal). Same trick as
        // `if`/`while`/`for` head parsing.
        let scrutinee = self.with_no_struct_lit(|p| p.parse_expr())?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            arms.push(self.parse_match_arm()?);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span;
        Ok(Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: start.merge(end),
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pat = self.parse_pattern()?;
        self.expect(&TokenKind::FatArrow, "`=>`")?;
        // Two arm-body forms: `block` (no trailing `,` required) or
        // `expr ,` (short form).
        let (body, arm_end) = if matches!(self.peek_kind(), TokenKind::LBrace) {
            let block = self.parse_block()?;
            let span = block.span;
            let expr = Expr {
                kind: ExprKind::Block(block),
                span,
            };
            // Optional trailing comma after block.
            let _ = self.eat(&TokenKind::Comma);
            (expr, span)
        } else {
            let expr = self.parse_expr()?;
            let span = expr.span;
            // Comma required between expr arms unless this is the last arm
            // before `}`.
            if !self.at(&TokenKind::RBrace) {
                self.expect(&TokenKind::Comma, "`,`")?;
            }
            (expr, span)
        };
        let arm_span = pat.span.merge(arm_end);
        Ok(MatchArm {
            pattern: pat,
            body,
            span: arm_span,
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.peek_kind() {
            TokenKind::Underscore => {
                let tok = self.bump();
                Ok(Pattern {
                    kind: PatternKind::Wildcard,
                    span: tok.span,
                })
            }
            TokenKind::Ident(_) => {
                // Either a bare binding `name`, a path `Enum::Variant`,
                // a cross-file path `prefix::Enum::Variant`, or — slice
                // 7GEN.5e — a generic-enum pattern `Option[i32]::Some(v)`.
                // v0.0.3 1P.1 extends to `mod::Enum[args]::Variant(...)`.
                let first = self.expect_ident()?;
                // Read optional `[type_args]` between the *outermost*
                // ident and `::`. Pattern position has no `[i]` indexing,
                // so `[` is unambiguously a type-arg list. The collected
                // args belong to whichever ident immediately preceded
                // them — usually the enum, but it might be the module
                // alias if the user mistakenly wrote `mod[T]::Enum::...`
                // (which is malformed; sema will reject).
                let mut type_args: Vec<Type> = if self.at(&TokenKind::LBracket) {
                    self.bump(); // `[`
                    let mut args = Vec::new();
                    while !self.at(&TokenKind::RBracket) {
                        args.push(self.parse_type()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::RBracket, "`]`")?;
                    args
                } else {
                    Vec::new()
                };
                if self.eat(&TokenKind::ColonColon) {
                    let second = self.expect_ident()?;
                    // After the second ident, look for *its* `[type_args]`
                    // (the `mod::Enum[T]` shape: type args attach to the
                    // enum, not the module alias). Only consume if
                    // `type_args` is still empty — pre-fix users wrote
                    // `Enum[T]::Variant` with args before the `::`.
                    if type_args.is_empty() && self.at(&TokenKind::LBracket) {
                        self.bump(); // `[`
                        let mut args = Vec::new();
                        while !self.at(&TokenKind::RBracket) {
                            args.push(self.parse_type()?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect(&TokenKind::RBracket, "`]`")?;
                        type_args = args;
                    }
                    // Three-segment shape: `prefix::Enum::Variant(...)` or
                    // `prefix::Enum[args]::Variant(...)`. Collapse
                    // `prefix::Enum` into one `enum_name` string so the
                    // resolver can rewrite it via the import-alias path.
                    let (enum_ident, variant_name) = if self.eat(&TokenKind::ColonColon) {
                        let third = self.expect_ident()?;
                        let qualified = format!("{}::{}", first.name, second.name);
                        let enum_span = first.span.merge(second.span);
                        let enum_ident = Ident {
                            name: qualified,
                            span: enum_span,
                        };
                        (enum_ident, third)
                    } else {
                        (first.clone(), second)
                    };
                    let payload = if self.eat(&TokenKind::LParen) {
                        let mut pats = Vec::new();
                        while !self.at(&TokenKind::RParen) {
                            pats.push(self.parse_pattern()?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect(&TokenKind::RParen, "`)`")?;
                        pats
                    } else {
                        Vec::new()
                    };
                    let span = first.span.merge(variant_name.span);
                    Ok(Pattern {
                        kind: PatternKind::Variant {
                            enum_name: enum_ident,
                            type_args,
                            variant_name,
                            payload,
                        },
                        span,
                    })
                } else if !type_args.is_empty() {
                    // `Option[i32]` with no `::Variant` segment — not
                    // a valid pattern shape.
                    Err(self.err_at_peek(
                        "expected `::Variant` after generic-enum name in pattern (e.g. `Option[i32]::Some(v)`)"
                    ))
                } else {
                    let span = first.span;
                    Ok(Pattern {
                        kind: PatternKind::Binding(first),
                        span,
                    })
                }
            }
            _ => Err(self.err_at_peek("pattern")),
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
                Some(Box::new(Expr {
                    kind: ExprKind::Block(b),
                    span,
                }))
            }
        } else {
            None
        };
        let end = match &else_branch {
            Some(e) => e.span,
            None => then.span,
        };
        Ok(Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then,
                else_branch,
            },
            span: start.merge(end),
        })
    }
}

// ---- helpers ----

fn is_block_like(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::Block(_) | ExprKind::Unsafe(_) | ExprKind::If { .. } | ExprKind::Match { .. }
    )
}

fn can_start_expr(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Int(_, _)
            | TokenKind::Float(_, _)
            | TokenKind::Str(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Ident(_)
            | TokenKind::LParen
            | TokenKind::LBrace
            | TokenKind::If
            | TokenKind::Match
            | TokenKind::Unsafe
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
        TokenKind::Str(_) => "string literal",
        TokenKind::Ident(_) => "identifier",
        TokenKind::Import => "`import`",
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
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("expected fn");
        };
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
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("expected fn");
        };
        let tail = f.body.tail.as_ref().unwrap();
        match &tail.kind {
            ExprKind::Binary {
                op: BinOp::Add,
                lhs,
                rhs,
            } => {
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
        let p =
            parse_src("fn main() -> i32 { let mut a: i32 = 0; let mut b: i32 = 0; a = b = 1; 0 }")
                .unwrap();
        // The third stmt is `a = (b = 1)`.
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("expected fn");
        };
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
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("expected fn");
        };
        match &f.body.stmts[0].kind {
            StmtKind::Let {
                init: Some(init), ..
            } => match &init.kind {
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
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("expected fn");
        };
        match &f.body.stmts[0].kind {
            StmtKind::For(ForLoop::Range { iter, .. }, _) => match &iter.kind {
                ExprKind::Range {
                    end: Some(end),
                    inclusive: false,
                    ..
                } => match &end.kind {
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
        let m = first_method(
            "struct P { x: i32 } impl P { fn consume(move self) -> i32 { return self.x; } }",
        );
        assert_eq!(m.receiver, Some(Receiver::Move));
    }

    #[test]
    fn self_receiver_parses() {
        let m =
            first_method("struct P { x: i32 } impl P { fn read(self) -> i32 { return self.x; } }");
        assert_eq!(m.receiver, Some(Receiver::Read));
    }

    #[test]
    fn mut_self_receiver_parses() {
        let m = first_method("struct P { x: i32 } impl P { fn write(mut self) { self.x = 0; } }");
        assert_eq!(m.receiver, Some(Receiver::Mut));
    }

    #[test]
    fn mut_move_self_is_parse_error() {
        let err =
            parse_src("struct P { x: i32 } impl P { fn bad(mut move self) -> i32 { return 0; } }")
                .unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn move_mut_self_is_parse_error() {
        let err =
            parse_src("struct P { x: i32 } impl P { fn bad(move mut self) -> i32 { return 0; } }")
                .unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn import_decl_parses() {
        let p =
            parse_src(r#"import "math.cplus" as math; fn main() -> i32 { return 0; }"#).unwrap();
        assert_eq!(p.imports.len(), 1);
        assert_eq!(p.imports[0].path, "math.cplus");
        assert_eq!(p.imports[0].as_name.name, "math");
        assert_eq!(p.items.len(), 1);
    }

    #[test]
    fn multiple_imports_parse_in_order() {
        let src = r#"
            import "a.cplus" as a;
            import "sub/b.cplus" as b;
            fn main() -> i32 { return 0; }
        "#;
        let p = parse_src(src).unwrap();
        assert_eq!(p.imports.len(), 2);
        assert_eq!(p.imports[0].as_name.name, "a");
        assert_eq!(p.imports[1].path, "sub/b.cplus");
    }

    #[test]
    fn import_without_as_clause_is_parse_error() {
        let err = parse_src(r#"import "math.cplus"; fn main() -> i32 { return 0; }"#).unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn import_without_path_string_is_parse_error() {
        let err = parse_src(r#"import math as math; fn main() -> i32 { return 0; }"#).unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn pub_fn_parses_with_flag() {
        let p = parse_src("pub fn f() -> i32 { return 0; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        assert!(f.is_pub);
    }

    #[test]
    fn private_fn_default_no_flag() {
        let p = parse_src("fn f() -> i32 { return 0; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        assert!(!f.is_pub);
    }

    #[test]
    fn pub_struct_with_mixed_field_visibility() {
        let p = parse_src("pub struct S { pub x: i32, y: i32 }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!();
        };
        assert!(s.is_pub);
        assert!(s.fields[0].is_pub);
        assert!(!s.fields[1].is_pub);
    }

    #[test]
    fn pub_enum_flag() {
        let p = parse_src("pub enum C { A, B }").unwrap();
        let ItemKind::Enum(e) = &p.items[0].kind else {
            panic!();
        };
        assert!(e.is_pub);
    }

    #[test]
    fn pub_method_flag() {
        let p = parse_src("struct S { x: i32 } impl S { pub fn f(self) -> i32 { return 0; } fn g(self) -> i32 { return 0; } }").unwrap();
        let ItemKind::Impl(b) = &p.items[1].kind else {
            panic!();
        };
        assert!(b.methods[0].is_pub);
        assert!(!b.methods[1].is_pub);
    }

    #[test]
    fn pub_on_impl_block_rejected() {
        let err = parse_src("pub impl S { }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    // ---- 6BC.5 — borrow REGION T syntax ----

    #[test]
    fn borrow_region_parses_in_param_and_return() {
        let p = parse_src(
            "fn longest(xs: borrow A string, ys: borrow A string) -> borrow A string { return xs; }"
        ).unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(f.params.len(), 2);
        assert!(matches!(&f.params[0].ty.kind, TypeKind::Borrowed { region, .. } if region == "A"));
        assert!(matches!(&f.params[1].ty.kind, TypeKind::Borrowed { region, .. } if region == "A"));
        let ret = f.return_type.as_ref().expect("has return type");
        assert!(matches!(&ret.kind, TypeKind::Borrowed { region, .. } if region == "A"));
    }

    #[test]
    fn borrow_region_parses_with_mut_marker() {
        let p = parse_src("fn modify(mut x: borrow A B) -> borrow A B { return x; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.params[0].mutable);
        assert!(matches!(&f.params[0].ty.kind, TypeKind::Borrowed { region, .. } if region == "A"));
    }

    #[test]
    fn move_with_borrow_region_rejected() {
        // `move x: borrow A T` is a parse error per §4.2: ownership
        // transfer doesn't borrow, so the region annotation is
        // meaningless on a `move`-parameter.
        let err = parse_src("fn take(move x: borrow A B) { return; }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn borrow_region_in_struct_field() {
        let p = parse_src("struct Cursor { buf: borrow A Buffer, pos: usize }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!()
        };
        assert!(matches!(&s.fields[0].ty.kind, TypeKind::Borrowed { region, .. } if region == "A"));
        assert!(matches!(&s.fields[1].ty.kind, TypeKind::Path(name) if name == "usize"));
    }

    #[test]
    fn borrow_region_nests_into_array() {
        let p = parse_src("fn f(xs: borrow A [B; 4]) { return; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        match &f.params[0].ty.kind {
            TypeKind::Borrowed { region, inner } => {
                assert_eq!(region, "A");
                assert!(matches!(&inner.kind, TypeKind::Array { len: 4, .. }));
            }
            other => panic!("expected borrow region around array, got {other:?}"),
        }
    }

    // ---- 7GEN.1 — generic function parsing ----

    #[test]
    fn non_generic_fn_has_empty_generic_params() {
        let p = parse_src("fn main() -> i32 { return 0; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.generic_params.is_empty());
    }

    #[test]
    fn generic_fn_single_param_no_bounds() {
        let p = parse_src("fn id[T](x: T) -> T { return x; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(f.generic_params.len(), 1);
        assert_eq!(f.generic_params[0].name.name, "T");
        assert!(f.generic_params[0].bounds.is_empty());
    }

    #[test]
    fn generic_fn_multiple_params_no_bounds() {
        let p = parse_src("fn pair[A, B](a: A, b: B) -> A { return a; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        let names: Vec<&str> = f
            .generic_params
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        assert_eq!(names, vec!["A", "B"]);
        for gp in &f.generic_params {
            assert!(gp.bounds.is_empty());
        }
    }

    #[test]
    fn generic_fn_single_bound() {
        let p = parse_src("fn max[T: Ord](a: T, b: T) -> T { return a; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(f.generic_params.len(), 1);
        let bounds: Vec<&str> = f.generic_params[0]
            .bounds
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(bounds, vec!["Ord"]);
    }

    #[test]
    fn generic_fn_multiple_bounds_plus_separated() {
        let p = parse_src("fn sorted_max[T: Ord + Eq + Clone](xs: T) -> T { return xs; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        let bounds: Vec<&str> = f.generic_params[0]
            .bounds
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(bounds, vec!["Ord", "Eq", "Clone"]);
    }

    #[test]
    fn generic_fn_mixed_bounds_per_param() {
        let p = parse_src("fn f[T: Ord, U, V: Eq + Clone](x: T) -> T { return x; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(f.generic_params.len(), 3);
        let bounds0: Vec<&str> = f.generic_params[0]
            .bounds
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        let bounds1: Vec<&str> = f.generic_params[1]
            .bounds
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        let bounds2: Vec<&str> = f.generic_params[2]
            .bounds
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(bounds0, vec!["Ord"]);
        assert!(bounds1.is_empty());
        assert_eq!(bounds2, vec!["Eq", "Clone"]);
    }

    #[test]
    fn generic_fn_with_pub_and_attributes() {
        let p = parse_src("#[test]\npub fn id[T](x: T) -> T { return x; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.is_pub);
        assert_eq!(f.attributes.len(), 1);
        assert_eq!(f.generic_params.len(), 1);
        assert_eq!(f.generic_params[0].name.name, "T");
    }

    #[test]
    fn empty_generic_params_rejected() {
        // `fn f[]() ...` is a parse error — empty brackets are
        // syntactically ambiguous noise.
        let err = parse_src("fn f[]() { return; }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn trailing_comma_in_generic_params_admitted() {
        // Style flexibility — same as fn params.
        let p = parse_src("fn f[T,](x: T) -> T { return x; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(f.generic_params.len(), 1);
    }

    // ---- 7GEN.2 — generic struct / enum parsing ----

    #[test]
    fn generic_struct_single_param() {
        let p = parse_src("struct Holder[T] { value: T }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(s.generic_params.len(), 1);
        assert_eq!(s.generic_params[0].name.name, "T");
    }

    #[test]
    fn generic_struct_multiple_params() {
        let p = parse_src("struct Pair[A, B] { first: A, second: B }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!()
        };
        let names: Vec<&str> = s
            .generic_params
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        assert_eq!(names, vec!["A", "B"]);
    }

    #[test]
    fn generic_struct_with_bound() {
        let p = parse_src("struct SortedList[T: Ord] { items: T }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!()
        };
        let bounds: Vec<&str> = s.generic_params[0]
            .bounds
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(bounds, vec!["Ord"]);
    }

    #[test]
    fn non_generic_struct_has_empty_generic_params() {
        let p = parse_src("struct Point { x: i32, y: i32 }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!()
        };
        assert!(s.generic_params.is_empty());
    }

    #[test]
    fn generic_enum_option_shape() {
        let p = parse_src("enum Option[T] { Some(T), None }").unwrap();
        let ItemKind::Enum(e) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(e.generic_params.len(), 1);
        assert_eq!(e.generic_params[0].name.name, "T");
    }

    #[test]
    fn generic_enum_result_shape() {
        let p = parse_src("enum Result[T, E] { Ok(T), Err(E) }").unwrap();
        let ItemKind::Enum(e) = &p.items[0].kind else {
            panic!()
        };
        let names: Vec<&str> = e
            .generic_params
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        assert_eq!(names, vec!["T", "E"]);
    }

    #[test]
    fn non_generic_enum_has_empty_generic_params() {
        let p = parse_src("enum Color { Red, Green, Blue }").unwrap();
        let ItemKind::Enum(e) = &p.items[0].kind else {
            panic!()
        };
        assert!(e.generic_params.is_empty());
    }

    #[test]
    fn generic_struct_pub_combo() {
        let p = parse_src("pub struct Vec[T] { data: T }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!()
        };
        assert!(s.is_pub);
        assert_eq!(s.generic_params.len(), 1);
    }

    // ---- 7GEN.3 — interface declarations + impl Interface for Type ----

    #[test]
    fn interface_decl_parses_with_single_method() {
        let p = parse_src("interface Ord { fn compare(self, other: i32) -> i32; }").unwrap();
        let ItemKind::Interface(i) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(i.name.name, "Ord");
        assert_eq!(i.methods.len(), 1);
        assert_eq!(i.methods[0].name.name, "compare");
        assert_eq!(i.methods[0].receiver, Some(Receiver::Read));
        assert!(i.methods[0].return_type.is_some());
    }

    #[test]
    fn interface_decl_multiple_methods() {
        let p = parse_src(
            "interface Eq {\n\
                 fn eq(self, other: i32) -> bool;\n\
                 fn ne(self, other: i32) -> bool;\n\
             }",
        )
        .unwrap();
        let ItemKind::Interface(i) = &p.items[0].kind else {
            panic!()
        };
        let names: Vec<&str> = i.methods.iter().map(|m| m.name.name.as_str()).collect();
        assert_eq!(names, vec!["eq", "ne"]);
    }

    #[test]
    fn interface_method_with_no_return_type() {
        let p = parse_src("interface Logger { fn log(self, msg: i32); }").unwrap();
        let ItemKind::Interface(i) = &p.items[0].kind else {
            panic!()
        };
        assert!(i.methods[0].return_type.is_none());
    }

    #[test]
    fn interface_method_requires_semicolon_not_body() {
        // Interface methods declare signatures, not implementations —
        // they must end with `;`, not `{ ... }`.
        let err = parse_src("interface X { fn f(self) -> i32 { return 0; } }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn interface_pub_combo() {
        let p = parse_src("pub interface Ord { fn compare(self, other: i32) -> i32; }").unwrap();
        let ItemKind::Interface(i) = &p.items[0].kind else {
            panic!()
        };
        assert!(i.is_pub);
    }

    #[test]
    fn impl_interface_for_target_parses() {
        let p = parse_src(
            "struct Point { x: i32, y: i32 }\n\
             impl Ord for Point {\n\
                 fn compare(self, other: Point) -> i32 { return 0; }\n\
             }",
        )
        .unwrap();
        let ItemKind::Impl(b) = &p.items[1].kind else {
            panic!()
        };
        assert_eq!(b.target.name, "Point");
        assert!(b.interface_name.is_some());
        assert_eq!(b.interface_name.as_ref().unwrap().name, "Ord");
        assert_eq!(b.methods.len(), 1);
    }

    #[test]
    fn plain_impl_target_still_works() {
        // Inherent impl without `for Interface` continues to work — no
        // interface_name set.
        let p = parse_src(
            "struct Point { x: i32 }\n\
             impl Point { fn x(self) -> i32 { return self.x; } }",
        )
        .unwrap();
        let ItemKind::Impl(b) = &p.items[1].kind else {
            panic!()
        };
        assert_eq!(b.target.name, "Point");
        assert!(b.interface_name.is_none());
    }

    #[test]
    fn interface_method_with_mut_self_receiver() {
        let p = parse_src("interface Counter { fn inc(mut self) -> i32; }").unwrap();
        let ItemKind::Interface(i) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(i.methods[0].receiver, Some(Receiver::Mut));
    }

    #[test]
    fn interface_associated_fn_no_receiver() {
        // `fn default() -> Self;` — no `self` receiver, like Rust's
        // `Default::default`.
        let p = parse_src("interface Default { fn default() -> i32; }").unwrap();
        let ItemKind::Interface(i) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(i.methods[0].receiver, None);
    }

    #[test]
    fn qualified_struct_literal_parses() {
        // `prefix::Type { ... }` should parse as a StructLit with the
        // qualified name string (resolver splits on `::` and rewrites).
        let p = parse_src(
            r#"import "g.cplus" as g; fn main() -> i32 { let p = g::Point { x: 1, y: 2 }; return 0; }"#,
        )
        .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        // Walk to the `let` statement's init expression.
        let StmtKind::Let {
            init: Some(init), ..
        } = &f.body.stmts[0].kind
        else {
            panic!();
        };
        let ExprKind::StructLit { name, fields } = &init.kind else {
            panic!("expected StructLit, got {:?}", init.kind);
        };
        assert_eq!(name.name, "g::Point");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name.name, "x");
        assert_eq!(fields[1].name.name, "y");
    }

    #[test]
    fn qualified_struct_literal_suppressed_in_if_head() {
        // In an `if EXPR { ... }` head, the trailing `{` opens the body —
        // not a struct literal. `prefix::Type` should fall back to a Path
        // expression in that position, same rule as the unqualified case.
        let p = parse_src(
            r#"import "g.cplus" as g; fn main() -> i32 { if g::flag { return 1; } return 0; }"#,
        )
        .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        // First stmt is the `if`-as-expression-statement; condition is a Path.
        let StmtKind::Expr(e) = &f.body.stmts[0].kind else {
            panic!();
        };
        let ExprKind::If { cond, .. } = &e.kind else {
            panic!();
        };
        let ExprKind::Path { segments } = &cond.kind else {
            panic!("expected Path, got {:?}", cond.kind);
        };
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].name, "g");
        assert_eq!(segments[1].name, "flag");
    }

    #[test]
    fn qualified_type_path_parses() {
        let p = parse_src(
            r#"import "math.cplus" as math; fn use_it(p: math::Point) -> i32 { return 0; }"#,
        )
        .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        let TypeKind::Path(s) = &f.params[0].ty.kind else {
            panic!();
        };
        assert_eq!(s, "math::Point");
    }

    #[test]
    fn import_after_item_is_parse_error() {
        let err = parse_src(r#"fn main() -> i32 { return 0; } import "math.cplus" as math;"#)
            .unwrap_err();
        // "expected item, found `import`" — the message text is brittle to
        // assert directly; the variant is what we care about.
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    // ---- Phase 5 slice 5ATTR.1: attribute parsing ----

    #[test]
    fn bare_attribute_on_fn_parses() {
        let p = parse_src("#[test] fn foo() { return; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        assert_eq!(f.attributes.len(), 1);
        assert_eq!(f.attributes[0].path.name, "test");
        assert!(f.attributes[0].args.is_empty());
    }

    #[test]
    fn multiple_attributes_collect_in_order() {
        let p = parse_src("#[foo] #[bar] fn f() { return; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        assert_eq!(f.attributes.len(), 2);
        assert_eq!(f.attributes[0].path.name, "foo");
        assert_eq!(f.attributes[1].path.name, "bar");
    }

    #[test]
    fn attribute_with_ident_arg_parses() {
        let p = parse_src("#[repr(C)] struct P { v: i32 }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!();
        };
        assert_eq!(s.attributes.len(), 1);
        assert_eq!(s.attributes[0].args.len(), 1);
        let AttrArg::Ident(id) = &s.attributes[0].args[0] else {
            panic!("expected ident arg, got {:?}", s.attributes[0].args[0])
        };
        assert_eq!(id.name, "C");
    }

    #[test]
    fn attribute_with_string_arg_parses() {
        let p = parse_src(r#"#[deprecated("gone")] fn old() { return; }"#).unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        let AttrArg::Str(s, _) = &f.attributes[0].args[0] else {
            panic!("expected string arg")
        };
        assert_eq!(s, "gone");
    }

    #[test]
    fn attribute_with_keyvalue_arg_parses() {
        let p = parse_src(r#"#[link(name = "z")] fn f() { return; }"#).unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        let AttrArg::KeyValue(key, AttrValue::Str(val, _)) = &f.attributes[0].args[0] else {
            panic!("expected name=\"z\" key-value arg")
        };
        assert_eq!(key.name, "name");
        assert_eq!(val, "z");
    }

    #[test]
    fn attribute_on_method_parses() {
        let p = parse_src(
            "struct X { v: i32 }\n\
             impl X { #[test] fn m(self) { return; } }",
        )
        .unwrap();
        let ItemKind::Impl(b) = &p.items[1].kind else {
            panic!();
        };
        assert_eq!(b.methods[0].attributes.len(), 1);
        assert_eq!(b.methods[0].attributes[0].path.name, "test");
    }

    #[test]
    fn attribute_on_struct_field_parses() {
        let p = parse_src("struct X { #[hint] v: i32 }").unwrap();
        let ItemKind::Struct(s) = &p.items[0].kind else {
            panic!();
        };
        assert_eq!(s.fields[0].attributes.len(), 1);
        assert_eq!(s.fields[0].attributes[0].path.name, "hint");
    }

    #[test]
    fn attribute_on_enum_variant_parses() {
        let p = parse_src("enum E { #[note] A, B }").unwrap();
        let ItemKind::Enum(e) = &p.items[0].kind else {
            panic!();
        };
        assert_eq!(e.variants[0].attributes.len(), 1);
        assert_eq!(e.variants[0].attributes[0].path.name, "note");
        assert!(e.variants[1].attributes.is_empty());
    }

    #[test]
    fn attribute_before_pub_on_fn_parses() {
        // Per the design note, attributes appear before `pub`. The parser
        // accepts that ordering; the reverse (`pub #[test]`) is a parse
        // error since `pub` consumes and then expects a keyword like
        // `fn`/`struct`/`enum`, not `#`.
        let p = parse_src("#[test] pub fn f() { return; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        assert!(f.is_pub);
        assert_eq!(f.attributes.len(), 1);
    }

    #[test]
    fn attribute_on_impl_block_itself_rejected() {
        // The design note (§6 open question 6) defers impl-level attributes;
        // we reject them at the parser to keep the option open.
        let err = parse_src("#[test] impl X { }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    // ---- Phase 5 slice 5ATTR.3: `assert EXPR;` ----

    #[test]
    fn assert_stmt_parses() {
        let p = parse_src("fn main() -> i32 { assert 1 == 1; return 0; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!();
        };
        let StmtKind::Assert(e) = &f.body.stmts[0].kind else {
            panic!("expected Assert stmt, got {:?}", f.body.stmts[0].kind);
        };
        // The expression is `1 == 1`.
        let ExprKind::Binary { op, .. } = &e.kind else {
            panic!()
        };
        assert!(matches!(op, BinOp::Eq));
    }

    #[test]
    fn assert_stmt_requires_semicolon() {
        // `assert EXPR` (no `;`) — sees `return` after and errors.
        let err = parse_src("fn main() -> i32 { assert true return 0; }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    // ---- Phase 10 slice 10.FFI.1: extern fn + raw pointers ----

    // ---- v0.0.3 Phase 5 Slice 5E.1: async fn + await ----

    #[test]
    fn async_fn_sets_is_async_flag() {
        let p = parse_src("async fn fetch() -> i32 { return 0; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.is_async, "async fn must set is_async=true");
        assert_eq!(f.name.name, "fetch");
    }

    #[test]
    fn pub_async_fn_parses() {
        let p = parse_src("pub async fn fetch() -> i32 { return 0; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.is_async);
        assert!(f.is_pub);
    }

    #[test]
    fn plain_fn_has_is_async_false() {
        let p = parse_src("fn sync() -> i32 { return 0; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(!f.is_async);
    }

    #[test]
    fn await_as_prefix_expression() {
        let p = parse_src(
            "async fn fetch() -> i32 { return 0; } fn main() -> i32 { return await fetch(); }",
        )
        .unwrap();
        let ItemKind::Function(main_fn) = &p.items[1].kind else {
            panic!()
        };
        // body tail expr — the `return await fetch();` statement
        let stmts = &main_fn.body.stmts;
        assert_eq!(stmts.len(), 1);
        // Look at the inner expression of the return statement.
        let crate::ast::StmtKind::Return(Some(e)) = &stmts[0].kind else {
            panic!("not a return-with-value: {:?}", stmts[0].kind)
        };
        assert!(
            matches!(e.kind, ExprKind::Await(_)),
            "expected ExprKind::Await for `await fetch()`, got: {:?}",
            e.kind
        );
    }

    #[test]
    fn await_binds_tighter_than_method_call() {
        // `await foo.bar()` should parse as `await (foo.bar())`,
        // not `(await foo).bar()`. The parser routes await at unary
        // precedence, so the method-call postfix runs *inside* the
        // inner expression.
        let p = parse_src(
            "struct S { x: i32 } \
             async fn one() -> S { return S { x: 1 }; } \
             fn main() -> i32 { return (await one()).x; }",
        )
        .unwrap();
        let ItemKind::Function(main_fn) = &p.items[2].kind else {
            panic!()
        };
        // The return value is `(await one()).x` — a Field expression
        // whose receiver is the `await` expression.
        let crate::ast::StmtKind::Return(Some(ret_e)) = &main_fn.body.stmts[0].kind else {
            panic!()
        };
        let ExprKind::Field { receiver, .. } = &ret_e.kind else {
            panic!("expected Field expr, got {:?}", ret_e.kind);
        };
        assert!(
            matches!(receiver.kind, ExprKind::Await(_)),
            "expected await as field receiver, got {:?}",
            receiver.kind
        );
    }

    #[test]
    fn extern_fn_parses() {
        let p = parse_src("extern fn abs(x: i32) -> i32;").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.is_extern);
        assert!(!f.is_pub);
        assert_eq!(f.name.name, "abs");
        assert_eq!(f.params.len(), 1);
        assert!(f.body.stmts.is_empty());
        assert!(f.body.tail.is_none());
    }

    #[test]
    fn extern_fn_no_return_type_parses() {
        let p = parse_src("extern fn free(p: i32);").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.is_extern);
        assert!(f.return_type.is_none());
    }

    #[test]
    fn extern_fn_with_body_requires_pub() {
        // Phase 5 Slice 5.C: a plain `extern fn` is an import declaration
        // and must end in `;`. Bodies require `pub` (export form).
        let err = parse_src("extern fn abs(x: i32) -> i32 { return x; }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn extern_fn_decl_with_pub_rejected() {
        // Phase 5 Slice 5.C: `pub` on a declaration (no body) is rejected
        // — the user likely forgot the body block. The diagnostic
        // suggests the export form.
        let err = parse_src("pub extern fn abs(x: i32) -> i32;").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn pub_extern_fn_with_body_parses() {
        // Phase 5 Slice 5.C: the C-callable export form.
        let p = parse_src("pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        assert!(f.is_pub, "expected is_pub on the export");
        assert!(f.is_extern, "expected is_extern on the export");
        assert!(!f.is_variadic);
        assert_eq!(f.params.len(), 2);
        assert!(!f.body.stmts.is_empty(), "expected a non-empty body");
    }

    #[test]
    fn pub_extern_fn_variadic_with_body_rejected() {
        // No `va_list` API in C+, so variadic exports can't be defined
        // (only imported). Variadic + body is a parser-level reject.
        let err = parse_src("pub extern fn p(x: i32, ...) { return; }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn extern_fn_with_generic_params_rejected() {
        let err = parse_src("extern fn ident[T](x: T) -> T;").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn raw_pointer_type_parses() {
        let p = parse_src("extern fn strlen(s: *u8) -> usize;").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        let TypeKind::RawPtr(inner) = &f.params[0].ty.kind else {
            panic!("expected RawPtr, got {:?}", f.params[0].ty.kind);
        };
        let TypeKind::Path(name) = &inner.kind else {
            panic!()
        };
        assert_eq!(name, "u8");
    }

    #[test]
    fn raw_pointer_nested_parses() {
        // `**i32` = `*(*i32)`.
        let p = parse_src("extern fn f(p: **i32) -> i32;").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        let TypeKind::RawPtr(outer) = &f.params[0].ty.kind else {
            panic!()
        };
        let TypeKind::RawPtr(inner) = &outer.kind else {
            panic!("expected RawPtr inside RawPtr")
        };
        let TypeKind::Path(name) = &inner.kind else {
            panic!()
        };
        assert_eq!(name, "i32");
    }

    // ---- Phase 7 slice 7GEN.5e: generic-enum patterns ----

    fn first_match_arms(p: &Program) -> &[MatchArm] {
        let f = match &p.items[0].kind {
            ItemKind::Function(f) => f,
            _ => panic!("expected fn"),
        };
        let ret = match &f.body.stmts.last().unwrap().kind {
            StmtKind::Return(Some(e)) => e,
            _ => panic!("expected return"),
        };
        match &ret.kind {
            ExprKind::Match { arms, .. } => arms,
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn generic_enum_pattern_with_type_args_parses() {
        // `Option[i32]::Some(v)` in pattern position.
        let p = parse_src(
            "fn f(o: i32) -> i32 { \
                return match o { \
                    Option[i32]::Some(v) => v, \
                    Option[i32]::None => 0, \
                }; \
            }",
        )
        .unwrap();
        let arms = first_match_arms(&p);
        let PatternKind::Variant {
            enum_name,
            type_args,
            variant_name,
            payload,
        } = &arms[0].pattern.kind
        else {
            panic!("expected variant pattern, got {:?}", arms[0].pattern.kind);
        };
        assert_eq!(enum_name.name, "Option");
        assert_eq!(type_args.len(), 1);
        assert_eq!(variant_name.name, "Some");
        assert_eq!(payload.len(), 1);
        // Payload-less variant.
        let PatternKind::Variant {
            type_args: ta2,
            payload: pl2,
            ..
        } = &arms[1].pattern.kind
        else {
            panic!();
        };
        assert_eq!(ta2.len(), 1);
        assert_eq!(pl2.len(), 0);
    }

    #[test]
    fn generic_enum_pattern_two_type_args_parses() {
        // `Result[i32, string]::Ok(v)` — multi-arg.
        let p = parse_src(
            "fn f(o: i32) -> i32 { \
                return match o { \
                    Result[i32, string]::Ok(v) => v, \
                    _ => 0, \
                }; \
            }",
        )
        .unwrap();
        let arms = first_match_arms(&p);
        let PatternKind::Variant { type_args, .. } = &arms[0].pattern.kind else {
            panic!()
        };
        assert_eq!(type_args.len(), 2);
    }

    #[test]
    fn unqualified_variant_pattern_keeps_empty_type_args() {
        // `Option::Some(v)` — no `[...]`. Type-directed resolution
        // happens in sema; parser leaves `type_args` empty.
        let p = parse_src(
            "fn f(o: i32) -> i32 { \
                return match o { \
                    Option::Some(v) => v, \
                    _ => 0, \
                }; \
            }",
        )
        .unwrap();
        let arms = first_match_arms(&p);
        let PatternKind::Variant {
            enum_name,
            type_args,
            ..
        } = &arms[0].pattern.kind
        else {
            panic!()
        };
        assert_eq!(enum_name.name, "Option");
        assert!(type_args.is_empty());
    }

    #[test]
    fn impl_block_with_target_generic_params_parses() {
        // Slice 7GEN.5e: `impl Vec[T] { ... }` parses, with `T`
        // recorded on `target_generic_params`.
        let p = parse_src("impl Vec[T] { fn len(self) -> usize { return 0; } }").unwrap();
        let ItemKind::Impl(b) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(b.target.name, "Vec");
        assert_eq!(b.target_generic_params.len(), 1);
        assert_eq!(b.target_generic_params[0].name.name, "T");
        assert_eq!(b.methods.len(), 1);
    }

    #[test]
    fn impl_block_with_two_target_generic_params_parses() {
        let p = parse_src("impl Pair[A, B] { fn first(self) -> A { return self.a; } }").unwrap();
        let ItemKind::Impl(b) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(b.target_generic_params.len(), 2);
        assert_eq!(b.target_generic_params[0].name.name, "A");
        assert_eq!(b.target_generic_params[1].name.name, "B");
    }

    #[test]
    fn impl_block_target_generic_param_with_bound_parses() {
        let p = parse_src("impl Sorted[T: Ord] { fn len(self) -> usize { return 0; } }").unwrap();
        let ItemKind::Impl(b) = &p.items[0].kind else {
            panic!()
        };
        assert_eq!(b.target_generic_params.len(), 1);
        assert_eq!(b.target_generic_params[0].bounds.len(), 1);
        assert_eq!(b.target_generic_params[0].bounds[0].name, "Ord");
    }

    #[test]
    fn impl_block_empty_target_generic_brackets_rejected() {
        let err = parse_src("impl Vec[] { }").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn method_call_turbofish_parses() {
        // Slice 7GEN.5e: `v.method::[i32](x)` parses as a Call with
        // a Field callee and non-empty type_args.
        let p = parse_src(
            "fn main() -> i32 { let p: Point = Point { x: 0, y: 0 }; return p.cast::[i32](7); }",
        )
        .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        let StmtKind::Return(Some(e)) = &f.body.stmts.last().unwrap().kind else {
            panic!()
        };
        let ExprKind::Call {
            callee, type_args, ..
        } = &e.kind
        else {
            panic!("expected Call, got {:?}", e.kind);
        };
        assert_eq!(type_args.len(), 1);
        assert!(
            matches!(callee.kind, ExprKind::Field { .. }),
            "expected Field callee for method turbofish, got {:?}",
            callee.kind
        );
    }

    #[test]
    fn assoc_call_turbofish_parses() {
        // Slice 7GEN.5e: `Type::method::[i32](x)` parses as a Call
        // with a Path callee and non-empty type_args.
        let p = parse_src("fn main() -> i32 { return Vec::new::[i32](); }").unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!()
        };
        let StmtKind::Return(Some(e)) = &f.body.stmts.last().unwrap().kind else {
            panic!()
        };
        let ExprKind::Call {
            callee, type_args, ..
        } = &e.kind
        else {
            panic!("expected Call, got {:?}", e.kind);
        };
        assert_eq!(type_args.len(), 1);
        let ExprKind::Path { segments } = &callee.kind else {
            panic!("expected Path callee, got {:?}", callee.kind);
        };
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].name, "Vec");
        assert_eq!(segments[1].name, "new");
    }

    #[test]
    fn generic_enum_pattern_without_variant_rejected() {
        // `Option[i32]` alone in pattern position — must be followed
        // by `::Variant`. Otherwise the parser errors.
        let err = parse_src(
            "fn f(o: i32) -> i32 { \
                return match o { \
                    Option[i32] => 0, \
                    _ => 0, \
                }; \
            }",
        )
        .unwrap_err();
        assert!(
            matches!(err.kind, ParseErrorKind::Unexpected { .. }),
            "expected parse error, got {:?}",
            err.kind
        );
    }

    // ---- v0.0.6 Slice 1A: include_bytes! ----

    #[test]
    fn include_bytes_macro_produces_include_bytes_node() {
        let p = parse_src("fn main() -> i32 { let x = include_bytes!(\"foo.bin\"); return 0; }")
            .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("fn");
        };
        let StmtKind::Let { init, .. } = &f.body.stmts[0].kind else {
            panic!("let");
        };
        match &init.as_ref().unwrap().kind {
            ExprKind::IncludeBytes { path } => assert_eq!(path, "foo.bin"),
            other => panic!("expected IncludeBytes, got {other:?}"),
        }
    }

    #[test]
    fn include_bytes_with_non_literal_arg_is_parse_error() {
        let err = parse_src("fn main() -> i32 { let x = include_bytes!(some_var); return 0; }")
            .unwrap_err();
        assert!(
            matches!(err.kind, ParseErrorKind::Unexpected { .. }),
            "expected Unexpected, got {:?}",
            err.kind
        );
    }

    #[test]
    fn include_bytes_without_bang_is_regular_call() {
        // Without the `!`, `include_bytes("foo.bin")` parses as a
        // normal function call (which sema will reject as undefined).
        let p = parse_src("fn main() -> i32 { let x = include_bytes(\"foo.bin\"); return 0; }")
            .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("fn");
        };
        let StmtKind::Let { init, .. } = &f.body.stmts[0].kind else {
            panic!("let");
        };
        match &init.as_ref().unwrap().kind {
            ExprKind::Call { .. } => {}
            other => panic!("expected Call, got {other:?}"),
        }
    }

    // ---- v0.0.7 Slice 3.1: include_str! ----

    #[test]
    fn include_str_macro_produces_include_str_node() {
        let p = parse_src("fn main() -> i32 { let x = include_str!(\"foo.txt\"); return 0; }")
            .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("fn");
        };
        let StmtKind::Let { init, .. } = &f.body.stmts[0].kind else {
            panic!("let");
        };
        match &init.as_ref().unwrap().kind {
            ExprKind::IncludeStr { path } => assert_eq!(path, "foo.txt"),
            other => panic!("expected IncludeStr, got {other:?}"),
        }
    }

    #[test]
    fn include_str_with_non_literal_arg_is_parse_error() {
        let err = parse_src("fn main() -> i32 { let x = include_str!(some_var); return 0; }")
            .unwrap_err();
        assert!(
            matches!(err.kind, ParseErrorKind::Unexpected { .. }),
            "expected Unexpected, got {:?}",
            err.kind
        );
    }

    #[test]
    fn include_str_without_bang_is_regular_call() {
        // Without the `!`, `include_str("foo.txt")` parses as a normal
        // function call (which sema will reject as undefined).
        let p = parse_src("fn main() -> i32 { let x = include_str(\"foo.txt\"); return 0; }")
            .unwrap();
        let ItemKind::Function(f) = &p.items[0].kind else {
            panic!("fn");
        };
        let StmtKind::Let { init, .. } = &f.body.stmts[0].kind else {
            panic!("let");
        };
        match &init.as_ref().unwrap().kind {
            ExprKind::Call { .. } => {}
            other => panic!("expected Call, got {other:?}"),
        }
    }

    // ---- v0.0.9 Phase 4: module-scope const + static ----

    #[test]
    fn const_int_decl_parses() {
        let p = parse_src("const HEADER_BYTES: usize = 176;").unwrap();
        let ItemKind::Const(c) = &p.items[0].kind else {
            panic!("expected ItemKind::Const, got {:?}", p.items[0].kind);
        };
        assert_eq!(c.name.name, "HEADER_BYTES");
        assert!(!c.is_pub);
        let TypeKind::Path(name) = &c.ty.kind else {
            panic!("expected Path type");
        };
        assert_eq!(name, "usize");
        match &c.value.kind {
            ExprKind::IntLit(v, _) => assert_eq!(*v, 176),
            other => panic!("expected IntLit, got {other:?}"),
        }
    }

    #[test]
    fn pub_const_string_decl_parses() {
        let p = parse_src("pub const VERSION: str = \"0.0.9\";").unwrap();
        let ItemKind::Const(c) = &p.items[0].kind else {
            panic!("expected ItemKind::Const");
        };
        assert_eq!(c.name.name, "VERSION");
        assert!(c.is_pub);
        match &c.value.kind {
            ExprKind::StrLit(s) => assert_eq!(s, "0.0.9"),
            other => panic!("expected StrLit, got {other:?}"),
        }
    }

    #[test]
    fn const_without_type_annotation_rejected() {
        let err = parse_src("const FOO = 5;").unwrap_err();
        assert!(
            matches!(err.kind, ParseErrorKind::Unexpected { .. }),
            "expected Unexpected, got {:?}",
            err.kind
        );
    }

    #[test]
    fn const_without_initializer_rejected() {
        let err = parse_src("const FOO: i32;").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }

    #[test]
    fn static_int_decl_parses() {
        let p = parse_src("static RNG_STATE: u32 = 305419896;").unwrap();
        let ItemKind::Static(s) = &p.items[0].kind else {
            panic!("expected ItemKind::Static, got {:?}", p.items[0].kind);
        };
        assert_eq!(s.name.name, "RNG_STATE");
        assert!(!s.is_mut);
        assert!(!s.is_pub);
        let TypeKind::Path(name) = &s.ty.kind else {
            panic!("expected Path type");
        };
        assert_eq!(name, "u32");
    }

    #[test]
    fn static_mut_decl_parses() {
        let p = parse_src("static mut COUNTER: i32 = 0;").unwrap();
        let ItemKind::Static(s) = &p.items[0].kind else {
            panic!("expected ItemKind::Static");
        };
        assert_eq!(s.name.name, "COUNTER");
        assert!(s.is_mut);
        assert!(!s.is_pub);
    }

    #[test]
    fn pub_static_mut_decl_parses() {
        let p = parse_src("pub static mut GLOBAL_TICK: u64 = 0;").unwrap();
        let ItemKind::Static(s) = &p.items[0].kind else {
            panic!("expected ItemKind::Static");
        };
        assert_eq!(s.name.name, "GLOBAL_TICK");
        assert!(s.is_mut);
        assert!(s.is_pub);
    }

    #[test]
    fn static_without_type_annotation_rejected() {
        let err = parse_src("static FOO = 5;").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Unexpected { .. }));
    }
}
