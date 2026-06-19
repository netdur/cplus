use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    /// v0.0.22 file-aware spans: the intern id of the file this span
    /// points into (see [`intern_file`]), stamped at tokenize time.
    /// `0` = no file info — synthesized spans and single-file mode —
    /// which falls back to the consumer's entry/default context.
    pub file: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end, file: 0 }
    }

    pub fn in_file(file: u32, start: u32, end: u32) -> Self {
        Self { start, end, file }
    }
}

/// Process-global file intern table for [`Span::file`]. Ids start at 1
/// (0 is the no-info sentinel). Interning is idempotent per name, so a
/// long-lived process (the LSP) grows the table only with its workspace.
static FILE_TABLE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn intern_file(id: &str) -> u32 {
    let mut table = FILE_TABLE.lock().unwrap();
    if let Some(pos) = table.iter().position(|e| e == id) {
        return (pos + 1) as u32;
    }
    table.push(id.to_string());
    table.len() as u32
}

/// The file id interned as `file`, or `None` for the 0 sentinel /
/// unknown ids.
pub fn interned_file(file: u32) -> Option<String> {
    if file == 0 {
        return None;
    }
    let table = FILE_TABLE.lock().unwrap();
    table.get((file - 1) as usize).cloned()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// v0.0.22 DSL.1: true when this token is the first on its source
    /// line — a newline (possibly among other trivia) separates it from
    /// the previous token, or it is the first token of the input.
    /// Stamped centrally by `tokenize_inner`. Builder blocks consult it
    /// to tell a leading-dot modifier line (`.font = bigger`) from
    /// ordinary same-line postfix access; no other grammar reads it.
    pub nl_before: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumSuffix {
    None,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    Isize, Usize,
    F16, F32, F64,
}

/// Phase 8 slice 8.STR.B.1: one piece of an interpolated string literal.
/// `Lit` carries decoded text (escapes processed, `$$` → `$`). `Expr`
/// carries the raw inner source plus its byte range in the parent file —
/// the parser sub-lexes this for the embedded expression.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    Lit(String),
    Expr { source: String, span: Span },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // literals
    Int(u64, NumSuffix),
    Float(f64, NumSuffix),
    /// Double-quoted string literal. Phase 4: used for `import "path" as name;`
    /// path strings. Payload is the unescaped contents (currently no escape
    /// sequences are processed — path strings don't need them).
    Str(String),
    /// `c"..."` C-string literal — a NUL-terminated `*u8` to a `.rodata`
    /// blob, for FFI (libc / JNI / Cocoa) without the `"...\0"` workaround.
    /// Payload is the decoded contents (same escapes as `Str`); the NUL is
    /// appended at codegen. No interpolation.
    CStr(String),
    /// Phase 8 slice 8.STR.B.1: interpolated string literal —
    /// `"hello ${name}, n is ${n}"`. The lexer splits the payload into
    /// alternating literal segments (escapes decoded; `$$` becomes `$`)
    /// and embedded expression source. Each expression is captured as
    /// raw source text plus its span in the parent file; the parser
    /// recursively re-lexes + parses it. Spans on the expression
    /// tokens point back into the parent file so diagnostics render at
    /// the right location.
    InterpStr(Vec<InterpPart>),
    Ident(String),
    /// `// ...` line comment. Payload excludes the `//` marker and the
    /// terminating newline. Only emitted by `tokenize_with_trivia`; the
    /// default `tokenize` filters these out so existing consumers see
    /// the same token stream as before. Used by `cpc fmt` (slice 4D) to
    /// preserve comments in the formatted output.
    LineComment(String),
    /// `/* ... */` block comment. Payload includes the interior verbatim
    /// (including any internal newlines and surrounding whitespace).
    /// Same trivia-mode rule as `LineComment`.
    BlockComment(String),

    // keywords (active in Phase 1)
    Fn, Let, Mut, Const, Static, If, Else, While, For, In, Return,
    True, False, As, Unsafe, Extern,
    // keywords (reserved for future phases)
    Struct, Enum, Union, Match, Trait, Impl, Pub, Export, Use, Mod, Import,
    SelfLower, SelfUpper, Defer, Try, Break, Continue, Loop, Move, Restrict, Opaque, Guard, Assert,
    /// v0.0.3 Phase 5 Slice 5E.1: `async` fn modifier + `await` prefix
    /// expression. Lexed unconditionally; sema/parser gate the
    /// allowed contexts (`async fn` declarations only, `await` only
    /// inside an `async fn` body).
    Async, Await,
    /// v0.0.4 Phase 4 Slice 4A: `gen` fn modifier + `yield` expression.
    /// `gen fn name() -> T` declares a generator coroutine whose body
    /// uses `yield V;` to produce successive `T` values. Sema rewrites
    /// the declared return type from `T` to `Iterator[T]`; codegen
    /// lowers the body to an LLVM coroutine.
    Gen, Yield,
    /// Slice 6BC.5: `borrow` keyword. Opens a region-annotated borrow
    /// type: `borrow A T` (shared) or `mut x: borrow A T` (exclusive).
    Borrow,
    /// Slice 7GEN.3: `interface` keyword — opens an interface
    /// declaration `interface Name { fn ... }`. Phase 7's bounded-
    /// polymorphism surface.
    Interface,
    /// Phase 11 polish (2026-05-13): `type` keyword — opens a type
    /// alias declaration `type Foo = Bar;`. Transparent: aliased name
    /// resolves to the same `Ty` as the target.
    TypeKw,

    // wildcard
    Underscore,

    // single-char punctuation
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Comma, Semi, Colon, Dot,
    /// `#` — opens an attribute (`#[...]`). Phase 5 slice 5ATTR.1.
    Pound,
    /// `@` — opens a contextual builder block `@ctx { ... }`. v0.0.22
    /// DSL.1. No other surface uses `@`.
    At,

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
    /// Slice 10.FFI.4: `...` for varargs in extern fn signatures.
    /// `extern fn printf(fmt: *u8, ...) -> i32;`. Lexed greedily after
    /// `..`: a third `.` upgrades DotDot to Ellipsis.
    Ellipsis,
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
    UnterminatedString,
    InvalidNumber(String),
    InvalidNumSuffix(String),
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            LexErrorKind::UnexpectedChar(c) => write!(f, "unexpected character '{c}'"),
            LexErrorKind::UnterminatedBlockComment => write!(f, "unterminated block comment"),
            LexErrorKind::UnterminatedString => write!(f, "unterminated string literal"),
            LexErrorKind::InvalidNumber(s) => write!(f, "invalid number literal: {s}"),
            LexErrorKind::InvalidNumSuffix(s) => write!(f, "invalid numeric type suffix: {s}"),
        }
    }
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    tokenize_inner(src, false, 0)
}

/// v0.0.22 file-aware spans: like `tokenize`, with every span stamped
/// with `file` (an id from [`intern_file`]). The resolver lexes each
/// project file through this so diagnostics and monomorphization route
/// by the span itself instead of per-item `origin_file` bookkeeping.
pub fn tokenize_with_file(src: &str, file: u32) -> Result<Vec<Token>, LexError> {
    tokenize_inner(src, false, file)
}

/// Like `tokenize`, but emits `LineComment` and `BlockComment` tokens
/// instead of silently discarding comments. Used by `cpc fmt` (slice 4D)
/// to preserve comments while reformatting. Whitespace is still
/// discarded — the formatter recovers newline placement by inspecting
/// byte gaps between consecutive token spans against the original source.
pub fn tokenize_with_trivia(src: &str) -> Result<Vec<Token>, LexError> {
    tokenize_inner(src, true, 0)
}

fn tokenize_inner(src: &str, keep_comments: bool, file: u32) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer::new(src);
    lx.keep_comments = keep_comments;
    lx.file = file;
    let bytes = src.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut prev_end: usize = 0;
    let mut first = true;
    loop {
        let mut t = lx.next_token()?;
        // v0.0.22 DSL.1: stamp the line-start bit. The gap between the
        // previous token's end and this token's start is all trivia
        // (whitespace/comments); a newline anywhere in it means this
        // token begins a line.
        let start = (t.span.start as usize).min(bytes.len());
        t.nl_before = first || bytes[prev_end.min(start)..start].contains(&b'\n');
        first = false;
        prev_end = (t.span.end as usize).min(bytes.len());
        let is_eof = matches!(t.kind, TokenKind::Eof);
        out.push(t);
        if is_eof {
            return Ok(out);
        }
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    /// Slice 4D: when true, `//` and `/* */` produce `LineComment` /
    /// `BlockComment` tokens instead of being silently skipped. Default
    /// false to preserve the existing token-stream contract.
    keep_comments: bool,
    /// v0.0.22: the [`intern_file`] id stamped into every produced span.
    file: u32,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self { src: src.as_bytes(), pos: 0, keep_comments: false, file: 0 }
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
        Span::in_file(self.file, start as u32, self.pos as u32)
    }

    /// Skip whitespace and (when `keep_comments` is false) comments.
    /// When `keep_comments` is true, this only skips whitespace — comments
    /// are returned to `next_token` so they can be emitted as tokens.
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b' ' | b'\t' | b'\n' | b'\r'), _) => { self.pos += 1; }
                (Some(b'/'), Some(b'/')) if !self.keep_comments => {
                    while let Some(c) = self.peek(0) {
                        if c == b'\n' { break; }
                        self.pos += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) if !self.keep_comments => {
                    self.skip_block_comment()?;
                }
                _ => return Ok(()),
            }
        }
    }

    /// Consume a block comment, returning Ok with cursor positioned past
    /// the closing `*/`. Errors on unterminated input. Used in both
    /// trivia-skipping and trivia-keeping modes; in the latter, the
    /// caller captures the body span before calling.
    fn skip_block_comment(&mut self) -> Result<(), LexError> {
        let start = self.pos;
        self.pos += 2;   // consume `/*`
        let mut depth: u32 = 1;
        while depth > 0 {
            match (self.peek(0), self.peek(1)) {
                (Some(b'/'), Some(b'*')) => { self.pos += 2; depth += 1; }
                (Some(b'*'), Some(b'/')) => { self.pos += 2; depth -= 1; }
                (Some(_), _) => { self.pos += 1; }
                (None, _) => return Err(LexError {
                    kind: LexErrorKind::UnterminatedBlockComment,
                    span: Span::in_file(self.file, start as u32, self.pos as u32),
                }),
            }
        }
        Ok(())
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_trivia()?;
        let start = self.pos;
        let Some(c) = self.peek(0) else {
            return Ok(Token { kind: TokenKind::Eof, span: self.span_from(start), nl_before: false });
        };

        // Comments — only reachable in trivia-keeping mode; `skip_trivia`
        // consumes them silently otherwise.
        if self.keep_comments && c == b'/' && self.peek(1) == Some(b'/') {
            self.pos += 2;
            let body_start = self.pos;
            while let Some(c) = self.peek(0) {
                if c == b'\n' { break; }
                self.pos += 1;
            }
            let body = std::str::from_utf8(&self.src[body_start..self.pos]).unwrap_or("").to_string();
            return Ok(Token {
                kind: TokenKind::LineComment(body),
                span: self.span_from(start),
                nl_before: false,
            });
        }
        if self.keep_comments && c == b'/' && self.peek(1) == Some(b'*') {
            let body_start = start + 2;
            self.skip_block_comment()?;
            // body excludes the opening `/*` and closing `*/`.
            let body_end = self.pos - 2;
            let body = std::str::from_utf8(&self.src[body_start..body_end]).unwrap_or("").to_string();
            return Ok(Token {
                kind: TokenKind::BlockComment(body),
                span: self.span_from(start),
                nl_before: false,
            });
        }

        // `c"..."` C-string literal — checked before the identifier branch,
        // since `c` is an identifier start.
        if c == b'c' && self.peek(1) == Some(b'"') {
            self.pos += 1; // consume the `c`
            return self.lex_cstring(start);
        }

        // identifiers / keywords / `_`
        if is_ident_start(c) {
            return Ok(self.lex_ident(start));
        }

        // numbers
        if c.is_ascii_digit() {
            return self.lex_number(start);
        }

        // strings
        if c == b'"' {
            // Multi-line `"""..."""` — verbatim, no escape/indent magic.
            if self.peek(1) == Some(b'"') && self.peek(2) == Some(b'"') {
                return self.lex_triple_string(start);
            }
            return self.lex_string(start);
        }

        // v0.0.9 Phase 2: character literals — `'a'`, `'\n'`, `'\xFF'`.
        // Lower to `TokenKind::Int(byte as u64, NumSuffix::U8)` so the
        // existing u8-literal codegen path takes over; no new AST or
        // sema surface needed. See plan.md Phase 2 for the locked design.
        if c == b'\'' {
            return self.lex_char(start);
        }

        // operators / punctuation
        self.lex_op_or_punct(start)
    }

    /// v0.0.9 Phase 2: lex a single-byte character literal. Accepted
    /// shapes:
    ///
    ///   `'a'`     — any printable ASCII byte (0x20..=0x7E except `'` and `\`)
    ///   `'\n'` `'\t'` `'\r'` `'\\'` `'\''` `'\0'` `'\"'`  — backslash escapes
    ///   `'\xHH'`  — hex byte escape (00..FF)
    ///
    /// Returns `TokenKind::Int(byte_value as u64, NumSuffix::U8)` — the
    /// parser routes that to `ExprKind::IntLit(_, U8)` and everything
    /// downstream (sema, codegen, pattern matching) treats it as a
    /// regular u8 literal.
    ///
    /// Errors:
    ///   - `''` (empty) → E0X20 reported via UnexpectedChar('\'')
    ///   - `'ab'` (two bytes between the quotes) → UnexpectedChar
    ///   - `'á'` (non-ASCII byte) → UnexpectedChar (the byte > 0x7F)
    ///   - Missing closing quote → UnterminatedString
    fn lex_char(&mut self, start: usize) -> Result<Token, LexError> {
        self.pos += 1; // opening '
        let byte: u8 = match self.peek(0) {
            // Empty literal `''` — the closing quote can't be the same
            // token as the opening one; treat as a parse error.
            Some(b'\'') => {
                return Err(LexError {
                    kind: LexErrorKind::UnexpectedChar('\''),
                    span: self.span_from(start),
                                    });
            }
            Some(b'\\') => {
                self.pos += 1;
                match self.peek(0) {
                    Some(b'n')  => { self.pos += 1; b'\n' }
                    Some(b't')  => { self.pos += 1; b'\t' }
                    Some(b'r')  => { self.pos += 1; b'\r' }
                    Some(b'\\') => { self.pos += 1; b'\\' }
                    Some(b'\'') => { self.pos += 1; b'\'' }
                    Some(b'"')  => { self.pos += 1; b'"'  }
                    Some(b'0')  => { self.pos += 1; b'\0' }
                    Some(b'x')  => {
                        self.pos += 1;
                        let hi = self.peek(0).and_then(hex_digit);
                        let lo = self.peek(1).and_then(hex_digit);
                        match (hi, lo) {
                            (Some(h), Some(l)) => {
                                self.pos += 2;
                                (h << 4) | l
                            }
                            _ => {
                                return Err(LexError {
                                    kind: LexErrorKind::UnexpectedChar(
                                        self.peek(0).unwrap_or(b'\'') as char,
                                    ),
                                    span: self.span_from(start),
                                                                    });
                            }
                        }
                    }
                    Some(other) => {
                        return Err(LexError {
                            kind: LexErrorKind::UnexpectedChar(other as char),
                            span: Span::in_file(self.file, self.pos as u32, self.pos as u32 + 1),
                        });
                    }
                    None => {
                        return Err(LexError {
                            kind: LexErrorKind::UnterminatedString,
                            span: self.span_from(start),
                                                    });
                    }
                }
            }
            Some(b) if b >= 0x80 => {
                // Non-ASCII byte (start of a UTF-8 multi-byte sequence).
                // Rejected at lex time — the char-literal type is `u8`,
                // not a full Unicode code point; for UTF-8 use a `str`.
                return Err(LexError {
                    kind: LexErrorKind::UnexpectedChar(b as char),
                    span: self.span_from(start),
                                    });
            }
            Some(b) => {
                self.pos += 1;
                b
            }
            None => {
                return Err(LexError {
                    kind: LexErrorKind::UnterminatedString,
                    span: self.span_from(start),
                                    });
            }
        };
        // Require the closing quote. A `'ab'`-style multi-byte literal
        // surfaces here as "expected `'`, found `b`".
        match self.peek(0) {
            Some(b'\'') => {
                self.pos += 1;
                Ok(Token {
                    kind: TokenKind::Int(byte as u64, NumSuffix::U8),
                    span: self.span_from(start),
                    nl_before: false,
                })
            }
            Some(other) => Err(LexError {
                kind: LexErrorKind::UnexpectedChar(other as char),
                span: Span::in_file(self.file, self.pos as u32, self.pos as u32 + 1),
            }),
            None => Err(LexError {
                kind: LexErrorKind::UnterminatedString,
                span: self.span_from(start),
                            }),
        }
    }

    /// Multi-line triple-quoted string `"""..."""`. **Verbatim**: the value is
    /// exactly the bytes between the opening and closing `"""` — no indentation
    /// stripping, no escape processing, no `${}` interpolation, literal newlines
    /// included. The developer controls layout; the compiler adds no magic
    /// (decided 2026-06-07). Emits a plain `Str` token, so it flows through the
    /// same path as a single-line literal (including the `Text` let coercion).
    fn lex_triple_string(&mut self, start: usize) -> Result<Token, LexError> {
        self.pos += 3; // opening """
        let mut decoded: Vec<u8> = Vec::new();
        loop {
            match self.peek(0) {
                None => {
                    return Err(LexError {
                        kind: LexErrorKind::UnterminatedString,
                        span: self.span_from(start),
                                            });
                }
                Some(b'"')
                    if self.peek(1) == Some(b'"') && self.peek(2) == Some(b'"') =>
                {
                    self.pos += 3; // closing """
                    let body = String::from_utf8(decoded).unwrap_or_default();
                    return Ok(Token {
                        kind: TokenKind::Str(body),
                        span: self.span_from(start),
                        nl_before: false,
                    });
                }
                Some(byte) => {
                    decoded.push(byte);
                    self.pos += 1;
                }
            }
        }
    }

    fn lex_string(&mut self, start: usize) -> Result<Token, LexError> {
        // Phase 8 slice 8.STR.B.1: interpolated string literal.
        // Two-phase scan:
        //   1. Accumulate bytes into `decoded`, processing `\n`/`\t`/... escapes.
        //   2. On `${`, flush the current `decoded` as a Lit part, scan
        //      forward counting braces until the matching `}`, capture
        //      the inner source as an Expr part, then resume scanning.
        //   3. On `$$`, decode as a literal `$`.
        //   4. On `$` followed by anything else, error (E0611-ish; raised
        //      as LexError::UnexpectedChar for now — parser-level mapping
        //      to E0611 in slice 8.STR.B's sema work).
        // If no `${...}` segments appear, emit a regular `Str` token —
        // existing consumers see the same shape as before.
        self.pos += 1; // opening "
        let mut decoded: Vec<u8> = Vec::new();
        let mut parts: Vec<InterpPart> = Vec::new();
        loop {
            match self.peek(0) {
                None | Some(b'\n') => {
                    return Err(LexError {
                        kind: LexErrorKind::UnterminatedString,
                        span: self.span_from(start),
                                            });
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek(0) {
                        Some(b'n')  => { decoded.push(b'\n'); self.pos += 1; }
                        Some(b't')  => { decoded.push(b'\t'); self.pos += 1; }
                        Some(b'r')  => { decoded.push(b'\r'); self.pos += 1; }
                        Some(b'\\') => { decoded.push(b'\\'); self.pos += 1; }
                        Some(b'"')  => { decoded.push(b'"');  self.pos += 1; }
                        Some(b'0')  => { decoded.push(b'\0'); self.pos += 1; }
                        // v0.0.9 follow-up: `\xHH` hex byte escape. Two
                        // hex nibbles → one byte. Used for ANSI control
                        // sequences (`\x1b[36m`), protocol literals,
                        // etc. — anywhere a string literal needs a
                        // non-printable byte without per-call mallocs.
                        //
                        // **ASCII range only (0x00..0x7F).** String
                        // tokens carry their payload as Rust `String`
                        // (UTF-8 required); a stray byte ≥ 0x80 would
                        // produce invalid UTF-8. For non-ASCII bytes,
                        // either embed the UTF-8 sequence directly in
                        // the literal or build the byte array manually
                        // and call `str_from_raw_parts` under `unsafe`.
                        Some(b'x')  => {
                            self.pos += 1;
                            let hi = self.peek(0).and_then(hex_digit);
                            let lo = self.peek(1).and_then(hex_digit);
                            match (hi, lo) {
                                (Some(h), Some(l)) => {
                                    self.pos += 2;
                                    let byte = (h << 4) | l;
                                    if byte >= 0x80 {
                                        return Err(LexError {
                                            kind: LexErrorKind::UnexpectedChar(byte as char),
                                            span: self.span_from(start),
                                                                                    });
                                    }
                                    decoded.push(byte);
                                }
                                _ => {
                                    return Err(LexError {
                                        kind: LexErrorKind::UnexpectedChar(
                                            self.peek(0).unwrap_or(b'"') as char,
                                        ),
                                        span: self.span_from(start),
                                                                            });
                                }
                            }
                        }
                        Some(other) => {
                            return Err(LexError {
                                kind: LexErrorKind::UnexpectedChar(other as char),
                                span: Span::in_file(self.file, self.pos as u32, self.pos as u32 + 1),
                            });
                        }
                        None => {
                            return Err(LexError {
                                kind: LexErrorKind::UnterminatedString,
                                span: self.span_from(start),
                                                            });
                        }
                    }
                }
                Some(b'$') => {
                    match self.peek(1) {
                        Some(b'$') => {
                            // `$$` → literal `$`.
                            decoded.push(b'$');
                            self.pos += 2;
                        }
                        Some(b'{') => {
                            // `${...}` — interpolation segment. Flush the
                            // current decoded bytes as a Lit part.
                            if !decoded.is_empty() {
                                let lit = String::from_utf8(std::mem::take(&mut decoded))
                                    .unwrap_or_default();
                                parts.push(InterpPart::Lit(lit));
                            }
                            self.pos += 2; // skip `${`
                            let inner_start = self.pos;
                            let mut brace_depth = 1usize;
                            while brace_depth > 0 {
                                match self.peek(0) {
                                    None => {
                                        return Err(LexError {
                                            kind: LexErrorKind::UnterminatedString,
                                            span: self.span_from(start),
                                                                                    });
                                    }
                                    Some(b'{') => { brace_depth += 1; self.pos += 1; }
                                    Some(b'}') => { brace_depth -= 1; if brace_depth > 0 { self.pos += 1; } }
                                    Some(b'"') => {
                                        // Nested string inside `${...}` not
                                        // supported in v1 — would need a
                                        // recursive lex_string call. The
                                        // design doc spells this out.
                                        return Err(LexError {
                                            kind: LexErrorKind::UnexpectedChar('"'),
                                            span: Span::in_file(self.file, self.pos as u32, self.pos as u32 + 1),
                                        });
                                    }
                                    Some(_) => { self.pos += 1; }
                                }
                            }
                            let inner_end = self.pos;   // points at `}`
                            let inner_source = std::str::from_utf8(&self.src[inner_start..inner_end])
                                .unwrap_or("")
                                .to_string();
                            parts.push(InterpPart::Expr {
                                source: inner_source,
                                span: Span::in_file(self.file, inner_start as u32, inner_end as u32),
                            });
                            self.pos += 1; // skip the closing `}`
                        }
                        _ => {
                            // v0.0.22: a bare `$` not followed by `{` or `$`
                            // is a literal `$`. Previously an error, so no
                            // existing program changes meaning; `$$` stays
                            // the escape for a `$` that *is* followed by
                            // `{`. Needed for JNI descriptors of nested
                            // Java classes (`android/view/View$OnClickListener`).
                            decoded.push(b'$');
                            self.pos += 1;
                        }
                    }
                }
                Some(b'"') => {
                    self.pos += 1; // closing "
                    if parts.is_empty() {
                        // No interpolation — emit a plain Str token to
                        // preserve the existing token shape.
                        let body = String::from_utf8(decoded).unwrap_or_default();
                        return Ok(Token { kind: TokenKind::Str(body), span: self.span_from(start), nl_before: false });
                    }
                    // Flush any trailing literal segment.
                    if !decoded.is_empty() {
                        let lit = String::from_utf8(decoded).unwrap_or_default();
                        parts.push(InterpPart::Lit(lit));
                    }
                    return Ok(Token { kind: TokenKind::InterpStr(parts), span: self.span_from(start), nl_before: false });
                }
                Some(b) => { decoded.push(b); self.pos += 1; }
            }
        }
    }

    /// Lex a `c"..."` C-string literal. On entry, `self.pos` is at the opening
    /// `"` (the `c` was already consumed). Same escapes as `lex_string`
    /// (`\n \t \r \\ \" \0 \xHH`, ASCII-only), but no `${...}` interpolation —
    /// a `$` is a literal `$`. The NUL terminator is appended at codegen.
    fn lex_cstring(&mut self, start: usize) -> Result<Token, LexError> {
        self.pos += 1; // opening "
        let mut decoded: Vec<u8> = Vec::new();
        loop {
            match self.peek(0) {
                None | Some(b'\n') => {
                    return Err(LexError {
                        kind: LexErrorKind::UnterminatedString,
                        span: self.span_from(start),
                                            });
                }
                Some(b'"') => {
                    self.pos += 1; // closing "
                    let body = String::from_utf8(decoded).unwrap_or_default();
                    return Ok(Token {
                        kind: TokenKind::CStr(body),
                        span: self.span_from(start),
                        nl_before: false,
                    });
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek(0) {
                        Some(b'n') => { decoded.push(b'\n'); self.pos += 1; }
                        Some(b't') => { decoded.push(b'\t'); self.pos += 1; }
                        Some(b'r') => { decoded.push(b'\r'); self.pos += 1; }
                        Some(b'\\') => { decoded.push(b'\\'); self.pos += 1; }
                        Some(b'"') => { decoded.push(b'"'); self.pos += 1; }
                        Some(b'0') => { decoded.push(b'\0'); self.pos += 1; }
                        Some(b'x') => {
                            self.pos += 1;
                            let hi = self.peek(0).and_then(hex_digit);
                            let lo = self.peek(1).and_then(hex_digit);
                            match (hi, lo) {
                                (Some(h), Some(l)) => {
                                    self.pos += 2;
                                    let byte = (h << 4) | l;
                                    if byte >= 0x80 {
                                        return Err(LexError {
                                            kind: LexErrorKind::UnexpectedChar(byte as char),
                                            span: self.span_from(start),
                                                                                    });
                                    }
                                    decoded.push(byte);
                                }
                                _ => {
                                    return Err(LexError {
                                        kind: LexErrorKind::UnexpectedChar(
                                            self.peek(0).unwrap_or(b'"') as char,
                                        ),
                                        span: self.span_from(start),
                                                                            });
                                }
                            }
                        }
                        Some(other) => {
                            return Err(LexError {
                                kind: LexErrorKind::UnexpectedChar(other as char),
                                span: Span::in_file(self.file, self.pos as u32, self.pos as u32 + 1),
                            });
                        }
                        None => {
                            return Err(LexError {
                                kind: LexErrorKind::UnterminatedString,
                                span: self.span_from(start),
                                                            });
                        }
                    }
                }
                Some(b) => { decoded.push(b); self.pos += 1; }
            }
        }
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
            "static" => TokenKind::Static,
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
            "export" => TokenKind::Export,
            "use" => TokenKind::Use,
            "mod" => TokenKind::Mod,
            "import" => TokenKind::Import,
            // v0.0.24 de-Rust: `this`/`This` are the receiver / enclosing-type
            // spellings (C++ `this`). The Rust `self`/`Self` spellings are no
            // longer accepted — they lex as plain identifiers and are rejected
            // downstream (the receiver parser and `This`-type path emit a
            // "use `this`/`This`" hint).
            "this" => TokenKind::SelfLower,
            "This" => TokenKind::SelfUpper,
            "defer" => TokenKind::Defer,
            "try" => TokenKind::Try,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "loop" => TokenKind::Loop,
            "move" => TokenKind::Move,
            "restrict" => TokenKind::Restrict,
            "guard" => TokenKind::Guard,
            "assert" => TokenKind::Assert,
            "borrow" => TokenKind::Borrow,
            "opaque" => TokenKind::Opaque,
            "interface" => TokenKind::Interface,
            "type" => TokenKind::TypeKw,
            "async" => TokenKind::Async,
            "gen"   => TokenKind::Gen,
            "yield" => TokenKind::Yield,
            "await" => TokenKind::Await,
            _ => TokenKind::Ident(text.to_string()),
        };
        Token { kind, span: self.span_from(start), nl_before: false }
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
                Some((s.to_string(), Span::in_file(self.file, suf_start as u32, self.pos as u32)))
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
        let kind = if is_float || matches!(suf, NumSuffix::F16 | NumSuffix::F32 | NumSuffix::F64) {
            // F32-suffixed literals parse directly to f32, then widen to f64
            // for AST storage. The widen is lossless (f32 → f64 is exact),
            // and codegen's `(*v as f32).to_bits()` recovers the exact f32.
            // Going through f64 first would double-round (decimal → f64 →
            // fptrunc-to-f32), one ULP off from the IEEE-correct value for
            // any non-exact literal.
            let v: f64 = if matches!(suf, NumSuffix::F32) {
                let f: f32 = digits.parse().map_err(|_| LexError {
                    kind: LexErrorKind::InvalidNumber(digits.clone()),
                    span: self.span_from(start),
                })?;
                f as f64
            } else {
                digits.parse().map_err(|_| LexError {
                    kind: LexErrorKind::InvalidNumber(digits.clone()),
                    span: self.span_from(start),
                })?
            };
            TokenKind::Float(v, suf)
        } else {
            let v = u64::from_str_radix(&digits, radix).map_err(|_| LexError {
                kind: LexErrorKind::InvalidNumber(digits.clone()),
                span: self.span_from(start),
            })?;
            TokenKind::Int(v, suf)
        };
        Ok(Token { kind, span: self.span_from(start), nl_before: false })
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
                    // Slice 10.FFI.4: `...` → Ellipsis.
                    if self.peek(0) == Some(b'.') { self.pos += 1; TokenKind::Ellipsis }
                    else if self.peek(0) == Some(b'=') { self.pos += 1; TokenKind::DotDotEq }
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
            b'#' => TokenKind::Pound,
            b'@' => TokenKind::At,
            other => return Err(LexError {
                kind: LexErrorKind::UnexpectedChar(other as char),
                span: self.span_from(start),
                            }),
        };
        Ok(Token { kind, span: self.span_from(start), nl_before: false })
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

/// v0.0.9 Phase 2: ASCII-hex-digit → nibble value. Used by `lex_char`
/// for the `'\xHH'` escape and (future-friendly) anywhere else a
/// hex-byte parse needs it. Returns `None` on a non-hex byte so the
/// caller can surface a precise diagnostic.
fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
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
        "f16" => NumSuffix::F16,
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
    fn const_and_static_keywords() {
        let src = "const FOO static BAR static mut BAZ";
        assert_eq!(kinds(src), vec![
            TokenKind::Const, TokenKind::Ident("FOO".into()),
            TokenKind::Static, TokenKind::Ident("BAR".into()),
            TokenKind::Static, TokenKind::Mut, TokenKind::Ident("BAZ".into()),
            TokenKind::Eof,
        ]);
    }

    // ---- v0.0.9 Phase 2: character literals ----

    #[test]
    fn char_literal_plain_ascii() {
        // Every char literal lowers to `Int(byte, U8)` — the u8-literal
        // codegen path takes over from there.
        assert_eq!(kinds("'a'"),  vec![TokenKind::Int(97,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'Z'"),  vec![TokenKind::Int(90,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'0'"),  vec![TokenKind::Int(48,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'{'"),  vec![TokenKind::Int(123, NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("' '"),  vec![TokenKind::Int(32,  NumSuffix::U8), TokenKind::Eof]);
    }

    #[test]
    fn char_literal_escapes() {
        assert_eq!(kinds("'\\n'"),  vec![TokenKind::Int(10,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\t'"),  vec![TokenKind::Int(9,   NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\r'"),  vec![TokenKind::Int(13,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\\\'"), vec![TokenKind::Int(92,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\''"),  vec![TokenKind::Int(39,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\\"'"), vec![TokenKind::Int(34,  NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\0'"),  vec![TokenKind::Int(0,   NumSuffix::U8), TokenKind::Eof]);
    }

    #[test]
    fn char_literal_hex_escape() {
        assert_eq!(kinds("'\\x00'"), vec![TokenKind::Int(0,   NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\x7F'"), vec![TokenKind::Int(127, NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\xff'"), vec![TokenKind::Int(255, NumSuffix::U8), TokenKind::Eof]);
        assert_eq!(kinds("'\\xab'"), vec![TokenKind::Int(171, NumSuffix::U8), TokenKind::Eof]);
    }

    #[test]
    fn char_literal_empty_rejected() {
        // `''` — opening immediately followed by closing.
        let err = tokenize("''").unwrap_err();
        assert!(matches!(err.kind, LexErrorKind::UnexpectedChar('\'')));
    }

    #[test]
    fn char_literal_multi_byte_rejected() {
        // `'ab'` — two bytes between the quotes. The first `a` is
        // consumed; the second `b` fails the closing-quote check.
        let err = tokenize("'ab'").unwrap_err();
        assert!(matches!(err.kind, LexErrorKind::UnexpectedChar('b')));
    }

    #[test]
    fn char_literal_utf8_rejected() {
        // `'á'` — the first byte of the UTF-8 encoding is 0xC3 (> 0x7F);
        // the char-literal type is `u8`, not a full Unicode code point.
        // For UTF-8 use a `str`.
        let err = tokenize("'á'").unwrap_err();
        assert!(matches!(err.kind, LexErrorKind::UnexpectedChar(_)));
    }

    #[test]
    fn char_literal_unterminated_rejected() {
        // `'a` with no closing quote.
        let err = tokenize("'a").unwrap_err();
        assert!(matches!(
            err.kind,
            LexErrorKind::UnterminatedString | LexErrorKind::UnexpectedChar(_)
        ));
    }

    #[test]
    fn char_literal_in_array_lit() {
        // `[b'{' as u8 = 123]`-style magic numbers can now be written
        // as `[123u8]` or with the new char literal `['{']`.
        // The token stream for `['{']` is `[ Int(123,U8) ]`.
        let toks = kinds("['{']");
        assert_eq!(toks, vec![
            TokenKind::LBracket,
            TokenKind::Int(123, NumSuffix::U8),
            TokenKind::RBracket,
            TokenKind::Eof,
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
    fn string_literal_basic() {
        assert_eq!(
            kinds(r#""hello""#),
            vec![TokenKind::Str("hello".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn string_literal_with_path() {
        assert_eq!(
            kinds(r#""util/strings.cplus""#),
            vec![TokenKind::Str("util/strings.cplus".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn string_empty() {
        assert_eq!(kinds(r#""""#), vec![TokenKind::Str(String::new()), TokenKind::Eof]);
    }

    #[test]
    fn string_unterminated_eof_errors() {
        assert!(matches!(
            tokenize(r#""oops"#).unwrap_err().kind,
            LexErrorKind::UnterminatedString
        ));
    }

    #[test]
    fn string_unterminated_newline_errors() {
        assert!(matches!(
            tokenize("\"oops\n\"").unwrap_err().kind,
            LexErrorKind::UnterminatedString
        ));
    }

    #[test]
    fn triple_quote_basic() {
        let toks = tokenize("\"\"\"hello\"\"\"").unwrap();
        assert_eq!(toks[0].kind, TokenKind::Str("hello".to_string()));
    }

    #[test]
    fn triple_quote_multiline_keeps_newlines_verbatim() {
        // Literal newlines are part of the value; leading newline after the
        // opening `"""` is NOT stripped (verbatim, no magic).
        let toks = tokenize("\"\"\"\nline one\nline two\n\"\"\"").unwrap();
        assert_eq!(
            toks[0].kind,
            TokenKind::Str("\nline one\nline two\n".to_string())
        );
    }

    #[test]
    fn triple_quote_no_indentation_stripping() {
        // Leading whitespace on each line stays in the value — the developer
        // controls layout, the compiler does not de-indent.
        let toks = tokenize("\"\"\"\n    indented\n    \"\"\"").unwrap();
        assert_eq!(
            toks[0].kind,
            TokenKind::Str("\n    indented\n    ".to_string())
        );
    }

    #[test]
    fn triple_quote_allows_inner_quotes_and_backslashes_raw() {
        // Single/double quotes inside are literal; backslashes are NOT escape
        // sequences (raw/verbatim).
        let toks = tokenize("\"\"\"say \"hi\" \\n done\"\"\"").unwrap();
        assert_eq!(
            toks[0].kind,
            TokenKind::Str("say \"hi\" \\n done".to_string())
        );
    }

    #[test]
    fn triple_quote_unterminated_errors() {
        assert!(matches!(
            tokenize("\"\"\"oops").unwrap_err().kind,
            LexErrorKind::UnterminatedString
        ));
    }

    #[test]
    fn string_escapes_decoded() {
        // Phase 8 slice 8.STR.1: `\n`, `\t`, `\r`, `\\`, `\"`, `\0` decode
        // to the corresponding bytes inside the token payload.
        let toks = tokenize(r#""a\nb\t\\""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::Str("a\nb\t\\".to_string()));
        let toks = tokenize(r#""\"""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::Str("\"".to_string()));
        let toks = tokenize(r#""\0""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::Str("\0".to_string()));
    }

    #[test]
    fn string_invalid_escape_rejected() {
        // Unknown escape character is a lex error.
        assert!(matches!(
            tokenize(r#""\q""#).unwrap_err().kind,
            LexErrorKind::UnexpectedChar('q')
        ));
    }

    #[test]
    fn string_hex_escape_decoded() {
        // v0.0.9 follow-up: `\xHH` in a string literal decodes to a
        // single byte. Used for ANSI control sequences (`\x1b[36m`)
        // and other non-printable ASCII bytes.
        let toks = tokenize(r#""\x1b[36m""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::Str("\x1b[36m".to_string()));
        // Two hex bytes in a row (NUL + ESC).
        let toks = tokenize(r#""\x00\x1b""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::Str("\u{00}\u{1b}".to_string()));
        // Uppercase hex digits.
        let toks = tokenize(r#""\x7F""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::Str("\u{7f}".to_string()));
    }

    #[test]
    fn f16_literal_suffix() {
        let toks = tokenize("1.5f16").unwrap();
        assert_eq!(toks[0].kind, TokenKind::Float(1.5, NumSuffix::F16));
        // An integer-form mantissa with the f16 suffix still lexes as a float.
        let toks = tokenize("3f16").unwrap();
        assert_eq!(toks[0].kind, TokenKind::Float(3.0, NumSuffix::F16));
    }

    #[test]
    fn cstring_literal_lexes_with_escapes() {
        // `c"..."` → a CStr token; same escapes as a normal string.
        let toks = tokenize(r#"c"hi\n""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::CStr("hi\n".to_string()));
        // A bare `c` (not followed by `"`) is still an identifier.
        let toks = tokenize("c + 1").unwrap();
        assert_eq!(toks[0].kind, TokenKind::Ident("c".to_string()));
        // An identifier ending in `c` is not mistaken for a c-string prefix.
        let toks = tokenize(r#"abc"x""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::Ident("abc".to_string()));
        assert_eq!(toks[1].kind, TokenKind::Str("x".to_string()));
    }

    #[test]
    fn string_hex_escape_missing_digits_rejected() {
        // `\x` followed by fewer than two hex digits is a lex error.
        assert!(tokenize(r#""\x""#).is_err());
        assert!(tokenize(r#""\x1""#).is_err());
        assert!(tokenize(r#""\xgg""#).is_err());
    }

    #[test]
    fn string_hex_escape_non_ascii_rejected() {
        // Bytes ≥ 0x80 would produce invalid UTF-8 in the lexer's
        // String payload. Lex-time reject with a clear pointer.
        assert!(tokenize(r#""\x80""#).is_err());
        assert!(tokenize(r#""\xff""#).is_err());
    }

    #[test]
    fn import_keyword() {
        assert_eq!(
            kinds("import"),
            vec![TokenKind::Import, TokenKind::Eof]
        );
    }

    #[test]
    fn guard_keyword() {
        assert_eq!(kinds("guard"), vec![TokenKind::Guard, TokenKind::Eof]);
    }

    #[test]
    fn import_decl_shape_lexes() {
        // Whole-statement lexing sanity check for `import "p" as n;`.
        let ks = kinds(r#"import "math.cplus" as math;"#);
        assert_eq!(ks, vec![
            TokenKind::Import,
            TokenKind::Str("math.cplus".into()),
            TokenKind::As,
            TokenKind::Ident("math".into()),
            TokenKind::Semi,
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn span_tracks_position() {
        let toks = tokenize("fn  foo").unwrap();
        assert_eq!(toks[0].span, Span::new(0, 2));
        assert_eq!(toks[1].span, Span::new(4, 7));
    }

    // ---- Phase 5 slice 5ATTR.1: `#` token for attributes ----

    #[test]
    fn pound_alone_lexes() {
        assert_eq!(kinds("#"), vec![TokenKind::Pound, TokenKind::Eof]);
    }

    #[test]
    fn attribute_opener_token_sequence() {
        // `#[test]` lexes as Pound, LBracket, Ident("test"), RBracket.
        assert_eq!(kinds("#[test]"), vec![
            TokenKind::Pound,
            TokenKind::LBracket,
            TokenKind::Ident("test".into()),
            TokenKind::RBracket,
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn assert_keyword() {
        // Phase 5 slice 5ATTR.3 — `assert` is a reserved keyword.
        assert_eq!(kinds("assert"), vec![TokenKind::Assert, TokenKind::Eof]);
    }

    #[test]
    fn borrow_keyword() {
        // Phase 6 slice 6BC.5 — `borrow` is a reserved keyword opening
        // a region-annotated type: `borrow A T`.
        assert_eq!(
            kinds("borrow A string"),
            vec![TokenKind::Borrow, TokenKind::Ident("A".into()),
                 TokenKind::Ident("string".into()), TokenKind::Eof]
        );
    }

    /// v0.0.22: a bare `$` in a string literal (not followed by `{` or
    /// `$`) is a literal `$` — JNI descriptors for nested Java classes
    /// need it (`android/view/View$OnClickListener`). `$$` stays the
    /// escape and `${...}` stays interpolation.
    /// v0.0.22 file-aware spans: `tokenize_with_file` stamps every span
    /// (including interpolation parts) with the given intern id; plain
    /// `tokenize` keeps the 0 sentinel; `intern_file` is idempotent.
    #[test]
    fn tokenize_with_file_stamps_every_span() {
        let f = intern_file("test/span_stamp.cplus");
        assert_eq!(intern_file("test/span_stamp.cplus"), f, "interning is idempotent");
        assert_eq!(interned_file(f).as_deref(), Some("test/span_stamp.cplus"));
        assert_eq!(interned_file(0), None, "0 is the no-info sentinel");

        let toks = tokenize_with_file("fn main() { let s = \"n is ${n}\"; }", f).unwrap();
        for t in &toks {
            assert_eq!(t.span.file, f, "token {:?} missing file stamp", t.kind);
            if let TokenKind::InterpStr(parts) = &t.kind {
                for part in parts {
                    if let InterpPart::Expr { span, .. } = part {
                        assert_eq!(span.file, f, "interp expr span missing file stamp");
                    }
                }
            }
        }
        // The default path keeps the sentinel.
        let toks = tokenize("fn f() {}").unwrap();
        assert!(toks.iter().all(|t| t.span.file == 0));
    }

    #[test]
    fn bare_dollar_in_string_is_literal() {
        assert_eq!(
            kinds("\"View$OnClickListener\""),
            vec![TokenKind::Str("View$OnClickListener".into()), TokenKind::Eof]
        );
        // `$` before the closing quote.
        assert_eq!(
            kinds("\"costs 5$\""),
            vec![TokenKind::Str("costs 5$".into()), TokenKind::Eof]
        );
        // Multiple bare dollars.
        assert_eq!(
            kinds("\"a$b$c\""),
            vec![TokenKind::Str("a$b$c".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn dollar_escape_and_interpolation_unchanged() {
        // `$$` still decodes to one `$`.
        assert_eq!(
            kinds("\"a$$b\""),
            vec![TokenKind::Str("a$b".into()), TokenKind::Eof]
        );
        // `${...}` still lexes as an interpolated string.
        let toks = kinds("\"n is ${n}\"");
        assert!(
            matches!(&toks[0], TokenKind::InterpStr(parts) if parts.len() == 2),
            "expected InterpStr, got {toks:?}"
        );
    }

    // ---- v0.0.22 DSL.1: `@` token + line-start stamping ----

    #[test]
    fn at_lexes() {
        assert_eq!(kinds("@"), vec![TokenKind::At, TokenKind::Eof]);
        // `@view {` — the builder-block opener shape.
        assert_eq!(kinds("@view {"), vec![
            TokenKind::At,
            TokenKind::Ident("view".into()),
            TokenKind::LBrace,
            TokenKind::Eof,
        ]);
    }

    #[test]
    fn nl_before_stamping() {
        let toks = tokenize("a b\n  .c d\n\ne").unwrap();
        let flags: Vec<(String, bool)> = toks
            .iter()
            .map(|t| (format!("{:?}", t.kind), t.nl_before))
            .collect();
        // First token of the input counts as starting a line.
        assert!(flags[0].1, "first token: {flags:?}");
        // `b` follows `a` on the same line.
        assert!(!flags[1].1, "same-line ident: {flags:?}");
        // `.` is the first token of line 2 (leading spaces are trivia).
        assert!(flags[2].1, "line-leading dot: {flags:?}");
        // `c` follows the dot on the same line.
        assert!(!flags[3].1, "same-line after dot: {flags:?}");
        assert!(!flags[4].1, "same-line `d`: {flags:?}");
        // Blank lines still mean line-start.
        assert!(flags[5].1, "after blank line: {flags:?}");
    }

    #[test]
    fn nl_before_counts_newlines_inside_comment_gaps() {
        // The gap between `a` and `b` is one block comment containing a
        // newline — `b` begins a line even though no bare `\n` separates
        // the tokens directly.
        let toks = tokenize("a /* x\n y */ b").unwrap();
        assert!(toks[1].nl_before, "token after newline-bearing comment gap");
        // Same-line comment gap does not set the flag.
        let toks = tokenize("a /* x */ b").unwrap();
        assert!(!toks[1].nl_before, "same-line comment gap");
    }
}
