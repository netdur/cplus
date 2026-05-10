use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumSuffix {
    None,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    Isize, Usize,
    F32, F64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // literals
    Int(u64, NumSuffix),
    Float(f64, NumSuffix),
    Ident(String),

    // keywords (active in Phase 1)
    Fn, Let, Mut, Const, If, Else, While, For, In, Return,
    True, False, As, Unsafe, Extern,
    // keywords (reserved for future phases)
    Struct, Enum, Union, Match, Trait, Impl, Pub, Use, Mod,
    SelfLower, SelfUpper, Defer, Try, Break, Continue, Loop, Move,

    // wildcard
    Underscore,

    // single-char punctuation
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Comma, Semi, Colon, Dot,

    // operators
    Plus, Minus, Star, Slash, Percent,
    PlusPercent, MinusPercent, StarPercent,   // wrapping
    Eq, EqEq, Bang, BangEq,
    Lt, Le, Gt, Ge,
    Amp, AmpAmp, Pipe, PipePipe, Caret, Tilde,
    Shl, Shr,
    PlusEq, MinusEq, StarEq, SlashEq, PercentEq,
    AmpEq, PipeEq, CaretEq, ShlEq, ShrEq,
    Arrow,        // ->
    FatArrow,     // =>
    DotDot, DotDotEq,
    ColonColon,

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub kind: LexErrorKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LexErrorKind {
    UnexpectedChar(char),
    UnterminatedBlockComment,
    InvalidNumber(String),
    InvalidNumSuffix(String),
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            LexErrorKind::UnexpectedChar(c) => write!(f, "unexpected character '{c}'"),
            LexErrorKind::UnterminatedBlockComment => write!(f, "unterminated block comment"),
            LexErrorKind::InvalidNumber(s) => write!(f, "invalid number literal: {s}"),
            LexErrorKind::InvalidNumSuffix(s) => write!(f, "invalid numeric type suffix: {s}"),
        }
    }
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        match lx.next_token()? {
            t if matches!(t.kind, TokenKind::Eof) => {
                out.push(t);
                return Ok(out);
            }
            t => out.push(t),
        }
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self { src: src.as_bytes(), pos: 0 }
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.src.get(self.pos).copied()?;
        self.pos += 1;
        Some(b)
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start as u32, self.pos as u32)
    }

    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b' ' | b'\t' | b'\n' | b'\r'), _) => { self.pos += 1; }
                (Some(b'/'), Some(b'/')) => {
                    while let Some(c) = self.peek(0) {
                        if c == b'\n' { break; }
                        self.pos += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    let start = self.pos;
                    self.pos += 2;
                    let mut depth: u32 = 1;
                    while depth > 0 {
                        match (self.peek(0), self.peek(1)) {
                            (Some(b'/'), Some(b'*')) => { self.pos += 2; depth += 1; }
                            (Some(b'*'), Some(b'/')) => { self.pos += 2; depth -= 1; }
                            (Some(_), _) => { self.pos += 1; }
                            (None, _) => return Err(LexError {
                                kind: LexErrorKind::UnterminatedBlockComment,
                                span: Span::new(start as u32, self.pos as u32),
                            }),
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_trivia()?;
        let start = self.pos;
        let Some(c) = self.peek(0) else {
            return Ok(Token { kind: TokenKind::Eof, span: self.span_from(start) });
        };

        // identifiers / keywords / `_`
        if is_ident_start(c) {
            return Ok(self.lex_ident(start));
        }

        // numbers
        if c.is_ascii_digit() {
            return self.lex_number(start);
        }

        // operators / punctuation
        self.lex_op_or_punct(start)
    }

    fn lex_ident(&mut self, start: usize) -> Token {
        while let Some(c) = self.peek(0) {
            if is_ident_continue(c) { self.pos += 1; } else { break; }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        let kind = match text {
            "_" => TokenKind::Underscore,
            "fn" => TokenKind::Fn,
            "let" => TokenKind::Let,
            "mut" => TokenKind::Mut,
            "const" => TokenKind::Const,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "as" => TokenKind::As,
            "unsafe" => TokenKind::Unsafe,
            "extern" => TokenKind::Extern,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "union" => TokenKind::Union,
            "match" => TokenKind::Match,
            "trait" => TokenKind::Trait,
            "impl" => TokenKind::Impl,
            "pub" => TokenKind::Pub,
            "use" => TokenKind::Use,
            "mod" => TokenKind::Mod,
            "self" => TokenKind::SelfLower,
            "Self" => TokenKind::SelfUpper,
            "defer" => TokenKind::Defer,
            "try" => TokenKind::Try,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "loop" => TokenKind::Loop,
            "move" => TokenKind::Move,
            _ => TokenKind::Ident(text.to_string()),
        };
        Token { kind, span: self.span_from(start) }
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, LexError> {
        // base prefix
        let (radix, body_start) = match (self.peek(0), self.peek(1)) {
            (Some(b'0'), Some(b'x' | b'X')) => { self.pos += 2; (16, self.pos) }
            (Some(b'0'), Some(b'b' | b'B')) => { self.pos += 2; (2, self.pos) }
            (Some(b'0'), Some(b'o' | b'O')) => { self.pos += 2; (8, self.pos) }
            _ => (10, start),
        };

        let mut digits = String::new();
        let mut has_digit = false;
        while let Some(c) = self.peek(0) {
            if c == b'_' { self.pos += 1; continue; }
            let ch = c as char;
            let valid = match radix {
                2  => matches!(c, b'0'..=b'1'),
                8  => matches!(c, b'0'..=b'7'),
                10 => c.is_ascii_digit(),
                16 => c.is_ascii_hexdigit(),
                _ => unreachable!(),
            };
            if !valid { break; }
            digits.push(ch);
            has_digit = true;
            self.pos += 1;
        }
        if !has_digit {
            return Err(LexError {
                kind: LexErrorKind::InvalidNumber(self.text_from(start).to_string()),
                span: self.span_from(start),
            });
        }

        // float? only base-10 supports floats
        let mut is_float = false;
        if radix == 10 {
            // `1.x` where x is digit → float; `1..` → int (range follows)
            if self.peek(0) == Some(b'.') && matches!(self.peek(1), Some(c) if c.is_ascii_digit()) {
                is_float = true;
                self.pos += 1; // .
                digits.push('.');
                while let Some(c) = self.peek(0) {
                    if c == b'_' { self.pos += 1; continue; }
                    if c.is_ascii_digit() { digits.push(c as char); self.pos += 1; }
                    else { break; }
                }
            }
            if matches!(self.peek(0), Some(b'e' | b'E')) {
                is_float = true;
                digits.push(self.peek(0).unwrap() as char);
                self.pos += 1;
                if matches!(self.peek(0), Some(b'+' | b'-')) {
                    digits.push(self.peek(0).unwrap() as char);
                    self.pos += 1;
                }
                let exp_start = self.pos;
                while let Some(c) = self.peek(0) {
                    if c == b'_' { self.pos += 1; continue; }
                    if c.is_ascii_digit() { digits.push(c as char); self.pos += 1; }
                    else { break; }
                }
                if self.pos == exp_start {
                    return Err(LexError {
                        kind: LexErrorKind::InvalidNumber(self.text_from(start).to_string()),
                        span: self.span_from(start),
                    });
                }
            }
        }

        // optional type suffix glued to the digits
        let suffix = if let Some(c) = self.peek(0) {
            if is_ident_start(c) {
                let suf_start = self.pos;
                while let Some(c) = self.peek(0) {
                    if is_ident_continue(c) { self.pos += 1; } else { break; }
                }
                let s = std::str::from_utf8(&self.src[suf_start..self.pos]).unwrap();
                Some((s.to_string(), Span::new(suf_start as u32, self.pos as u32)))
            } else { None }
        } else { None };

        let suf = match suffix {
            None => NumSuffix::None,
            Some((s, span)) => match parse_suffix(&s) {
                Some(ns) => ns,
                None => return Err(LexError {
                    kind: LexErrorKind::InvalidNumSuffix(s),
                    span,
                }),
            },
        };

        let _ = body_start;
        let kind = if is_float || matches!(suf, NumSuffix::F32 | NumSuffix::F64) {
            let v: f64 = digits.parse().map_err(|_| LexError {
                kind: LexErrorKind::InvalidNumber(digits.clone()),
                span: self.span_from(start),
            })?;
            TokenKind::Float(v, suf)
        } else {
            let v = u64::from_str_radix(&digits, radix).map_err(|_| LexError {
                kind: LexErrorKind::InvalidNumber(digits.clone()),
                span: self.span_from(start),
            })?;
            TokenKind::Int(v, suf)
        };
        Ok(Token { kind, span: self.span_from(start) })
    }

    fn text_from(&self, start: usize) -> &str {
        std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("")
    }

    fn lex_op_or_punct(&mut self, start: usize) -> Result<Token, LexError> {
        let c = self.bump().unwrap();
        let kind = match c {
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b',' => TokenKind::Comma,
            b';' => TokenKind::Semi,
            b'~' => TokenKind::Tilde,
            b':' => if self.peek(0) == Some(b':') { self.pos += 1; TokenKind::ColonColon }
                    else { TokenKind::Colon },
            b'.' => match self.peek(0) {
                Some(b'.') => {
                    self.pos += 1;
                    if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::DotDotEq }
                    else { TokenKind::DotDot }
                }
                _ => TokenKind::Dot,
            },
            b'-' => match self.peek(0) {
                Some(b'>') => { self.pos += 1; TokenKind::Arrow }
                Some(b'=') => { self.pos += 1; TokenKind::MinusEq }
                Some(b'%') => { self.pos += 1; TokenKind::MinusPercent }
                _ => TokenKind::Minus,
            },
            b'=' => match self.peek(0) {
                Some(b'=') => { self.pos += 1; TokenKind::EqEq }
                Some(b'>') => { self.pos += 1; TokenKind::FatArrow }
                _ => TokenKind::Eq,
            },
            b'!' => if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::BangEq }
                    else { TokenKind::Bang },
            b'<' => match self.peek(0) {
                Some(b'=') => { self.pos += 1; TokenKind::Le }
                Some(b'<') => {
                    self.pos += 1;
                    if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::ShlEq }
                    else { TokenKind::Shl }
                }
                _ => TokenKind::Lt,
            },
            b'>' => match self.peek(0) {
                Some(b'=') => { self.pos += 1; TokenKind::Ge }
                Some(b'>') => {
                    self.pos += 1;
                    if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::ShrEq }
                    else { TokenKind::Shr }
                }
                _ => TokenKind::Gt,
            },
            b'+' => match self.peek(0) {
                Some(b'=') => { self.pos += 1; TokenKind::PlusEq }
                Some(b'%') => { self.pos += 1; TokenKind::PlusPercent }
                _ => TokenKind::Plus,
            },
            b'*' => match self.peek(0) {
                Some(b'=') => { self.pos += 1; TokenKind::StarEq }
                Some(b'%') => { self.pos += 1; TokenKind::StarPercent }
                _ => TokenKind::Star,
            },
            b'/' => if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::SlashEq }
                    else { TokenKind::Slash },
            b'%' => if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::PercentEq }
                    else { TokenKind::Percent },
            b'&' => match self.peek(0) {
                Some(b'&') => { self.pos += 1; TokenKind::AmpAmp }
                Some(b'=') => { self.pos += 1; TokenKind::AmpEq }
                _ => TokenKind::Amp,
            },
            b'|' => match self.peek(0) {
                Some(b'|') => { self.pos += 1; TokenKind::PipePipe }
                Some(b'=') => { self.pos += 1; TokenKind::PipeEq }
                _ => TokenKind::Pipe,
            },
            b'^' => if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::CaretEq }
                    else { TokenKind::Caret },
            other => return Err(LexError {
                kind: LexErrorKind::UnexpectedChar(other as char),
                span: self.span_from(start),
            }),
        };
        Ok(Token { kind, span: self.span_from(start) })
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn parse_suffix(s: &str) -> Option<NumSuffix> {
    Some(match s {
        "i8" => NumSuffix::I8,
        "i16" => NumSuffix::I16,
        "i32" => NumSuffix::I32,
        "i64" => NumSuffix::I64,
        "u8" => NumSuffix::U8,
        "u16" => NumSuffix::U16,
        "u32" => NumSuffix::U32,
        "u64" => NumSuffix::U64,
        "isize" => NumSuffix::Isize,
        "usize" => NumSuffix::Usize,
        "f32" => NumSuffix::F32,
        "f64" => NumSuffix::F64,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn empty_emits_eof() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn whitespace_and_comments_skipped() {
        let src = "  // line\n  /* block /* nested */ */ \t fn ";
        assert_eq!(kinds(src), vec![TokenKind::Fn, TokenKind::Eof]);
    }

    #[test]
    fn unterminated_block_comment_errors() {
        assert!(matches!(
            tokenize("/* hello").unwrap_err().kind,
            LexErrorKind::UnterminatedBlockComment
        ));
    }

    #[test]
    fn keywords_and_idents() {
        let src = "fn foo let mut return if else while for in true false";
        assert_eq!(kinds(src), vec![
            TokenKind::Fn, TokenKind::Ident("foo".into()),
            TokenKind::Let, TokenKind::Mut,
            TokenKind::Return, TokenKind::If, TokenKind::Else,
            TokenKind::While, TokenKind::For, TokenKind::In,
            TokenKind::True, TokenKind::False, TokenKind::Eof,
        ]);
    }

    #[test]
    fn underscore_is_wildcard() {
        assert_eq!(kinds("_"), vec![TokenKind::Underscore, TokenKind::Eof]);
        // _x is a normal identifier
        assert_eq!(kinds("_x"), vec![TokenKind::Ident("_x".into()), TokenKind::Eof]);
    }

    #[test]
    fn integers_with_bases_and_separators() {
        assert_eq!(kinds("42"), vec![TokenKind::Int(42, NumSuffix::None), TokenKind::Eof]);
        assert_eq!(kinds("1_000_000"), vec![TokenKind::Int(1_000_000, NumSuffix::None), TokenKind::Eof]);
        assert_eq!(kinds("0xDEAD_BEEF"), vec![TokenKind::Int(0xDEAD_BEEF, NumSuffix::None), TokenKind::Eof]);
        assert_eq!(kinds("0b1010"), vec![TokenKind::Int(0b1010, NumSuffix::None), TokenKind::Eof]);
        assert_eq!(kinds("0o17"), vec![TokenKind::Int(0o17, NumSuffix::None), TokenKind::Eof]);
    }

    #[test]
    fn integer_with_suffix() {
        assert_eq!(kinds("42i32"), vec![TokenKind::Int(42, NumSuffix::I32), TokenKind::Eof]);
        assert_eq!(kinds("100u64"), vec![TokenKind::Int(100, NumSuffix::U64), TokenKind::Eof]);
    }

    #[test]
    fn invalid_suffix_errors() {
        assert!(matches!(
            tokenize("42xyz").unwrap_err().kind,
            LexErrorKind::InvalidNumSuffix(_)
        ));
    }

    #[test]
    fn float_literals() {
        let ks = kinds("1.5 2.0e10 3.14f32");
        assert_eq!(ks.len(), 4);
        match &ks[0] { TokenKind::Float(v, NumSuffix::None) => assert!((v - 1.5).abs() < 1e-9), _ => panic!() }
        match &ks[1] { TokenKind::Float(v, NumSuffix::None) => assert!((v - 2.0e10).abs() < 1.0), _ => panic!() }
        match &ks[2] { TokenKind::Float(v, NumSuffix::F32) => assert!((v - 3.14).abs() < 1e-6), _ => panic!() }
    }

    #[test]
    fn dotdot_does_not_eat_into_float() {
        // `1..2` must lex as Int, DotDot, Int — not as `1.` then `.2`
        assert_eq!(kinds("1..2"), vec![
            TokenKind::Int(1, NumSuffix::None),
            TokenKind::DotDot,
            TokenKind::Int(2, NumSuffix::None),
            TokenKind::Eof,
        ]);
        assert_eq!(kinds("1..=10"), vec![
            TokenKind::Int(1, NumSuffix::None),
            TokenKind::DotDotEq,
            TokenKind::Int(10, NumSuffix::None),
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn operators_disambiguate() {
        assert_eq!(kinds("== = => -> -% +% *% <= >= != <<= >>="), vec![
            TokenKind::EqEq, TokenKind::Eq, TokenKind::FatArrow, TokenKind::Arrow,
            TokenKind::MinusPercent, TokenKind::PlusPercent, TokenKind::StarPercent,
            TokenKind::Le, TokenKind::Ge, TokenKind::BangEq,
            TokenKind::ShlEq, TokenKind::ShrEq, TokenKind::Eof,
        ]);
    }

    #[test]
    fn factorial_sample_lexes() {
        let src = include_str!("../../docs/examples/factorial.cplus");
        let toks = tokenize(src).unwrap();
        // sanity: ends with Eof, contains the keywords we expect
        assert!(matches!(toks.last().unwrap().kind, TokenKind::Eof));
        let kinds: Vec<_> = toks.iter().map(|t| &t.kind).collect();
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::Fn)));
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::Ident(s) if s == "factorial")));
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::Arrow)));
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::Le)));
    }

    #[test]
    fn all_samples_lex_clean() {
        for path in &[
            "../docs/examples/factorial.cplus",
            "../docs/examples/fibonacci.cplus",
            "../docs/examples/sum_range.cplus",
            "../docs/examples/c_for.cplus",
        ] {
            let src = std::fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path))
                .unwrap_or_else(|e| panic!("read {path}: {e}"));
            tokenize(&src).unwrap_or_else(|e| panic!("lex {path}: {e}"));
        }
    }

    #[test]
    fn span_tracks_position() {
        let toks = tokenize("fn  foo").unwrap();
        assert_eq!(toks[0].span, Span::new(0, 2));
        assert_eq!(toks[1].span, Span::new(4, 7));
    }
}
