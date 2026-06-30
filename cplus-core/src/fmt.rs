//! Canonical formatter for C+ source (Phase 4 slice 4D).
//!
//! Slice 4D.1: a **preserving formatter**. The user's structural line
//! breaks are kept; the formatter normalizes within-line whitespace and
//! indentation, and preserves comments at their original lexical
//! positions. Reformatting (collapsing incidental wraps, forcing
//! multi-line on overflows) is slice 4D.2 work.
//!
//! Approach: tokenize the input *with trivia* (lexer emits LineComment /
//! BlockComment tokens), then walk the token stream and emit a
//! canonicalized version. Newlines between tokens come from counting
//! newlines in the source byte gap; indentation comes from a
//! multi-line-bracket stack; same-line spacing comes from a small lookup
//! table over the previous and current token kinds.
//!
//! See `docs/design/phase4-fmt.md` for the style decisions.

use crate::diagnostics::Diagnostic;
use crate::lexer::{self, Token, TokenKind};

/// Format a source string. Returns the formatted output. The input must
/// be lex-clean; the parser is not invoked (a non-parseable source can
/// still be formatted at the lexical level, which is occasionally
/// useful — but a lex error is fatal).
pub fn format_source(src: &str) -> Result<String, FormatError> {
    let toks = lexer::tokenize_with_trivia(src).map_err(FormatError::Lex)?;
    let mut p = Printer::new(src);
    p.print(&toks);
    Ok(p.finish())
}

#[derive(Debug)]
pub enum FormatError {
    Lex(lexer::LexError),
}

impl FormatError {
    /// Render as a structured `Diagnostic` so the driver can route it
    /// through `--diagnostics=json|short|human`.
    pub fn to_diagnostic(&self, file: &std::path::Path, src: &str) -> Diagnostic {
        match self {
            FormatError::Lex(e) => {
                let lm = crate::diagnostics::LineMap::new(src);
                crate::diagnostics::from_lex(e, &file.to_path_buf(), &lm, src)
            }
        }
    }
}

// ---- printer ----

const INDENT: &str = "    "; // 4 spaces per §3.1

struct Printer<'a> {
    src: &'a str,
    out: String,
    /// Stack of open brackets we've emitted but not closed. `true` if
    /// multi-line (matching `}` will be on a different output line).
    /// Used to compute indent (depth = count of multi-line entries) and
    /// to size before-`}` spacing.
    brackets: Vec<BracketCtx>,
    /// Byte offset in `src` past the previous emitted token. Used to
    /// count newlines in the gap before the next token.
    prev_end: usize,
    /// Kind of the previous emitted token, for spacing decisions.
    prev_kind: Option<TokenKind>,
    /// Kind two tokens back. Slice 10.FFI.2 needs this to detect
    /// `*` in type position (e.g. `: *u8` vs `a * b`): when the
    /// previous token is `*` and the one before is `:` / `->` /
    /// another `*`, we know we're in `*T` type position and the
    /// space-after-`*` rule should suppress.
    prev_prev_kind: Option<TokenKind>,
    /// True once we've emitted at least one token (used to suppress
    /// leading whitespace).
    started: bool,
}

#[derive(Debug, Clone, Copy)]
struct BracketCtx {
    open: BracketKind,
    multi_line: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BracketKind {
    Paren,
    Bracket,
    Brace,
}

impl<'a> Printer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            out: String::with_capacity(src.len() + src.len() / 8),
            brackets: Vec::new(),
            prev_end: 0,
            prev_kind: None,
            prev_prev_kind: None,
            started: false,
        }
    }

    fn finish(mut self) -> String {
        // Trim trailing spaces from final line; ensure file ends with a
        // single newline.
        while self.out.ends_with(' ') || self.out.ends_with('\t') {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        // Collapse trailing blank lines to a single newline.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        self.out
    }

    fn indent_depth(&self) -> usize {
        self.brackets.iter().filter(|b| b.multi_line).count()
    }

    fn print(&mut self, toks: &[Token]) {
        // Walk every token except trailing Eof.
        for i in 0..toks.len() {
            let t = &toks[i];
            if matches!(t.kind, TokenKind::Eof) {
                break;
            }
            self.emit(t, toks, i);
        }
    }

    fn emit(&mut self, tok: &Token, all: &[Token], idx: usize) {
        let start = tok.span.start as usize;
        let end = tok.span.end as usize;
        let newlines = count_newlines(self.src, self.prev_end, start);

        // ----- whitespace before this token -----
        let is_trailing_comment = newlines == 0
            && self.started
            && matches!(
                tok.kind,
                TokenKind::LineComment(_) | TokenKind::BlockComment(_)
            );

        if !self.started {
            // First token: emit at indent 0; ignore any source leading whitespace.
        } else if newlines > 0 {
            // Newlines in source → emit at most one blank-line separator (= 2 newlines).
            let n = newlines.min(2);
            for _ in 0..n {
                self.out.push('\n');
            }
            // For statement-starting tokens use computed indent. For
            // continuation lines (inside a multi-line expression, after
            // `,` or a binary op or an open bracket) preserve the user's
            // literal leading whitespace — they may have aligned the
            // continuation by hand and we don't want to flatten that.
            if self.is_statement_start_after_newline(&tok.kind) {
                self.emit_indent_for_token(&tok.kind);
            } else {
                self.preserve_line_leading_whitespace(start);
            }
        } else if is_trailing_comment {
            // Preserve the user's literal whitespace gap before a same-line
            // comment — samples align trailing comments and we honor that
            // (slice 4D.1 exception to whitespace normalization; see
            // design note §3.9).
            let gap = &self.src[self.prev_end..start];
            // Pull just the spaces/tabs (no newlines — newlines==0 here).
            for c in gap.chars() {
                if c == ' ' || c == '\t' {
                    self.out.push(c);
                }
            }
            // Guarantee at least one space — even if the source had none.
            if !self.out.ends_with(' ') && !self.out.ends_with('\t') {
                self.out.push(' ');
            }
        } else {
            // Normal same-line spacing.
            let prev = self.prev_kind.as_ref().expect("started but no prev_kind");
            // `|` is bitwise-or (spaced) EXCEPT the two delimiters of an
            // `else |Pat|` complement pattern, which hug the pattern. Decide from
            // the pipe's neighbours: `idx` when the pipe is `curr`, `idx - 1` when
            // it's `prev` (same-line, so `prev` is the immediately preceding token).
            let space = if matches!(tok.kind, TokenKind::Pipe)
                && matches!(pipe_role(all, idx), PipeRole::PatternClose)
            {
                false // `…)|` — tight before the closing delimiter
            } else if matches!(prev, TokenKind::Pipe)
                && matches!(pipe_role(all, idx - 1), PipeRole::PatternOpen)
            {
                false // `|Pat` — tight after the opening delimiter
            } else {
                needs_space_between_ctx(self.prev_prev_kind.as_ref(), prev, &tok.kind)
            };
            if space {
                self.out.push(' ');
            }
        }

        // ----- emit the token text -----
        let token_text = token_text(&tok.kind, self.src, start, end);
        // For multi-line block comments, ensure the body is re-indented
        // line-by-line at the current indent. Slice 4D.1 keeps it simple:
        // emit verbatim. Re-indenting multi-line block comments is a
        // polish item.
        self.out.push_str(&token_text);

        // ----- post-emit state updates -----
        self.update_bracket_stack(&tok.kind, all, idx);
        self.prev_end = end;
        self.prev_prev_kind = self.prev_kind.clone();
        self.prev_kind = Some(tok.kind.clone());
        self.started = true;
    }

    /// Heuristic: should a newline-prefixed token be indented at the
    /// computed depth (statement-start) or preserve the user's literal
    /// leading whitespace (continuation)?
    ///
    /// Rule of thumb: if the previous non-trivia code token was one of
    /// `;`, `}`, `{` — or this is the closing `}` / `)` / `]` of the
    /// enclosing bracket — the new line starts a fresh statement / list
    /// element / block, so use computed indent. Otherwise, the new line
    /// continues an expression and the user's manual indentation
    /// (typically aligned with operands) should be preserved.
    fn is_statement_start_after_newline(&self, curr: &TokenKind) -> bool {
        use TokenKind::*;
        // Closing brackets always re-anchor to their enclosing scope's
        // indent.
        if matches!(curr, RBrace | RParen | RBracket) {
            return true;
        }
        // Look at the immediately preceding token.
        match &self.prev_kind {
            None => true, // very first token
            Some(prev) => matches!(
                prev,
                Semi | RBrace | LBrace
                // Comma terminates a list element — its successor is the
                // next element, which is a "fresh start" at the list's
                // standard indent.
                | Comma
                // `=>` in a match arm anchors the arm body; not a typical
                // continuation site, treat as fresh.
                | FatArrow
                // Line comments are trivia — re-check the kind before them.
                // For simplicity, treat after-comment as fresh — comments
                // don't typically end a continuation.
                | LineComment(_) | BlockComment(_)
            ),
        }
    }

    /// Emit the literal leading whitespace from the source line that
    /// `start` sits on, normalizing tabs to 4 spaces. Used for
    /// continuation lines where the user picked an alignment we want
    /// to honor.
    fn preserve_line_leading_whitespace(&mut self, start: usize) {
        // Find the newline preceding `start` in source.
        let src = self.src.as_bytes();
        let mut line_start = start;
        while line_start > 0 && src[line_start - 1] != b'\n' {
            line_start -= 1;
        }
        // Emit the run of spaces/tabs from line_start up to start.
        for &b in &src[line_start..start] {
            match b {
                b' ' => self.out.push(' '),
                b'\t' => self.out.push_str(INDENT),
                _ => break, // stop at first non-whitespace (shouldn't happen)
            }
        }
    }

    /// Compute the indent for a token that's the first on its line.
    /// Special-case: a `}` / `)` / `]` that closes a multi-line bracket
    /// pops one level from the indent (the closing line is unindented
    /// relative to the body inside the brackets).
    fn emit_indent_for_token(&mut self, kind: &TokenKind) {
        let mut depth = self.indent_depth();
        let pop_one = match kind {
            TokenKind::RBrace
                if self
                    .brackets
                    .last()
                    .map(|b| b.open == BracketKind::Brace && b.multi_line)
                    .unwrap_or(false) =>
            {
                true
            }
            TokenKind::RParen
                if self
                    .brackets
                    .last()
                    .map(|b| b.open == BracketKind::Paren && b.multi_line)
                    .unwrap_or(false) =>
            {
                true
            }
            TokenKind::RBracket
                if self
                    .brackets
                    .last()
                    .map(|b| b.open == BracketKind::Bracket && b.multi_line)
                    .unwrap_or(false) =>
            {
                true
            }
            _ => false,
        };
        if pop_one {
            depth = depth.saturating_sub(1);
        }
        for _ in 0..depth {
            self.out.push_str(INDENT);
        }
    }

    fn update_bracket_stack(&mut self, kind: &TokenKind, all: &[Token], idx: usize) {
        match kind {
            TokenKind::LBrace => {
                let multi_line = is_open_multi_line(all, idx, self.src);
                self.brackets.push(BracketCtx {
                    open: BracketKind::Brace,
                    multi_line,
                });
            }
            TokenKind::LParen => {
                let multi_line = is_open_multi_line(all, idx, self.src);
                self.brackets.push(BracketCtx {
                    open: BracketKind::Paren,
                    multi_line,
                });
            }
            TokenKind::LBracket => {
                let multi_line = is_open_multi_line(all, idx, self.src);
                self.brackets.push(BracketCtx {
                    open: BracketKind::Bracket,
                    multi_line,
                });
            }
            TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => {
                self.brackets.pop();
            }
            _ => {}
        }
    }
}

/// "Multi-line bracket" iff there's a newline in the source between the
/// open bracket and its closest non-trivia following token. We look at
/// the byte gap from the open's end to the next token's start.
fn is_open_multi_line(all: &[Token], idx: usize, src: &str) -> bool {
    let open_end = all[idx].span.end as usize;
    let next_start = all
        .get(idx + 1)
        .map(|t| t.span.start as usize)
        .unwrap_or(src.len());
    src[open_end..next_start].contains('\n')
}

fn count_newlines(src: &str, from: usize, to: usize) -> usize {
    src[from..to].bytes().filter(|&b| b == b'\n').count()
}

fn token_text(kind: &TokenKind, src: &str, start: usize, end: usize) -> String {
    match kind {
        // Comments: re-emit with their markers, body verbatim.
        TokenKind::LineComment(body) => format!("//{body}"),
        TokenKind::BlockComment(body) => format!("/*{body}*/"),
        // Strings, numbers, identifiers, everything else: emit the
        // original source slice. This preserves number-base formatting
        // (`0xDEAD_BEEF`), string contents, identifier case.
        _ => src[start..end].to_string(),
    }
}

/// Same-line spacing rule between two consecutive tokens. Returns true
/// if a single space goes between them.
/// Slice 10.FFI.2: same as `needs_space_between` but with context for
/// the type-position `*T` detection. `prev_prev` is the token two
/// positions back. Returns false (tight) when:
/// - curr is `*` and prev is `:` or `->` (we're emitting `: *T`)
/// - prev is `*` and prev_prev is `:` or `->` or `*` (between
///   a type-position star and its inner type, no space)
fn needs_space_between_ctx(
    prev_prev: Option<&TokenKind>,
    prev: &TokenKind,
    curr: &TokenKind,
) -> bool {
    use TokenKind::*;
    // Type-position `*` is tight on both sides.
    if matches!(curr, Star) && matches!(prev, Colon | Arrow) {
        // Leave the space after `:` / `->` (handled by the existing
        // rule) but suppress the binary-op spacing that would
        // otherwise insert a space before the `*`.
        // Actually the existing rule emits ` ` after Colon/Arrow,
        // which IS the leading-space we want. Returning true here
        // is correct — keep that space.
        return true;
    }
    if matches!(prev, Star) {
        // Type-position anchors: `:` / `->` / `*` / `as` / `[`.
        // The `as` case covers `expr as *T` cast targets (Phase 11.INTPTR
        // + raw-pointer reinterpretation casts). The `[` case covers
        // turbofish type-arg lists like `size_of::[*u8]()`.
        let in_type_pos = matches!(
            prev_prev,
            Some(Colon) | Some(Arrow) | Some(Star) | Some(As) | Some(LBracket)
        );
        if in_type_pos {
            return false;
        }
        // Unary-prefix `*p` (deref): `*` is tight to the right when the
        // token before it can start an expression — i.e. a place where a
        // unary prefix could legally appear. Approximated as: prev_prev
        // is an operator / opener / statement-boundary token.
        let unary_prefix = matches!(
            prev_prev,
            None | Some(LParen)
                | Some(LBrace)
                | Some(Comma)
                | Some(Semi)
                | Some(Eq)
                | Some(EqEq)
                | Some(BangEq)
                | Some(Lt)
                | Some(Le)
                | Some(Gt)
                | Some(Ge)
                | Some(Plus)
                | Some(Minus)
                | Some(Slash)
                | Some(Percent)
                | Some(AmpAmp)
                | Some(PipePipe)
                | Some(Return)
                | Some(If)
                | Some(Else)
                | Some(While)
                | Some(FatArrow)
        );
        if unary_prefix {
            return false;
        }
    }
    // Slice 11.FN_PTR: `fn(...)` in type position is tight — no space
    // between `fn` and `(`. The declaration form (`fn name(...)`) has an
    // ident between, so this rule only ever fires on fn-pointer types.
    if matches!(prev, Fn) && matches!(curr, LParen) {
        return false;
    }
    needs_space_between(prev, curr)
}

fn needs_space_between(prev: &TokenKind, curr: &TokenKind) -> bool {
    use TokenKind::*;

    // No space at any open-bracket boundary.
    if matches!(prev, LParen | LBracket) {
        return false;
    }
    if matches!(curr, RParen | RBracket | Comma | Semi) {
        return false;
    }

    // `{` and `}` for inline blocks (e.g. struct literal `Point { x: 1 }`)
    // get one space on the inside. The multi-line case never hits this
    // function because there's a newline between.
    if matches!(prev, LBrace) {
        // Inline open: space before content, unless content is `}` (empty block).
        return !matches!(curr, RBrace);
    }
    if matches!(curr, RBrace) {
        // Inline close: space before, unless preceded by `{` (empty).
        return !matches!(prev, LBrace);
    }

    // `::` and `.` are tight on both sides.
    if matches!(prev, ColonColon | Dot) {
        return false;
    }
    if matches!(curr, ColonColon | Dot) {
        return false;
    }

    // Slice 10.FFI.5: attribute prefix `#[` is tight — `#` immediately
    // adjoins `[`. (Re-checked in pre-comments-special-case below.)
    if matches!(prev, Pound) {
        return false;
    }

    // v0.0.22 DSL.1: builder-block opener `@ctx` is tight — `@`
    // immediately adjoins the context path. Full builder-block layout is
    // DSL.4; this only keeps the marker glued to its name.
    if matches!(prev, At) {
        return false;
    }

    // Range operators `..` and `..=` are tight on both sides: `0..5`,
    // `1..=10`. Same convention as Rust.
    if matches!(prev, DotDot | DotDotEq) {
        return false;
    }
    if matches!(curr, DotDot | DotDotEq) {
        return false;
    }

    // `|` (Pipe) gets normal binary-op spacing via `is_binary_op` below — it's
    // the bitwise-or operator. The one exception, the `else |Pat|` complement
    // pattern, is handled by `pipe_role` at the emit site (the two pattern pipes
    // are forced tight there), so by the time a Pipe reaches here it is bitwise.

    // `:` in `name: T` — no space before, one after.
    if matches!(curr, Colon) {
        return false;
    }
    if matches!(prev, Colon) {
        return true;
    }

    // Function-call / index-call boundary: ident or `)` or `]` immediately
    // followed by `(` or `[` is tight.
    if matches!(curr, LParen | LBracket) {
        if matches!(
            prev,
            Ident(_) | SelfLower | SelfUpper | RParen | RBracket | RBrace | Int(..) | Float(..)
        ) {
            return false;
        }
    }

    // v0.0.6 Slice 1A: `include_bytes!("path")` macro form. The
    // parser only accepts `Ident Bang LParen Str RParen` as a single
    // construct, so any `Ident Bang` sequence is the macro form —
    // print it tight. `!=` is `BangEq` (a separate token) so this rule
    // doesn't affect comparisons.
    if matches!(prev, Ident(_)) && matches!(curr, Bang) {
        return false;
    }

    // After `,` always one space.
    if matches!(prev, Comma) {
        return true;
    }
    // After `;` (rare in inline contexts: `for (init; cond; update)`) → space.
    if matches!(prev, Semi) {
        return true;
    }

    // Unary operators (always prefix): `!`, `~`, and `-` in operand
    // position. After unary → no space.
    if is_unary_prefix(prev) {
        return false;
    }

    // Reference / bitwise `&` and `|` — these are binary in C+ (no `&T`
    // syntax). Always one space around.
    // Same for all other binary operators.
    if is_binary_op(prev) || is_binary_op(curr) {
        return true;
    }

    // Comments: leading trailing-space handling is special-cased in the
    // caller; reaching here means newline-separated, no space needed.
    if matches!(prev, LineComment(_) | BlockComment(_)) {
        return false;
    }
    if matches!(curr, LineComment(_) | BlockComment(_)) {
        return false;
    }

    // Keyword → ident/literal/keyword: one space.
    // Ident → keyword: one space.
    // Catch-all default: one space.
    true
}

fn is_unary_prefix(t: &TokenKind) -> bool {
    use TokenKind::*;
    // `!` and `~` are always unary prefix in C+ syntax.
    if matches!(t, Bang | Tilde) {
        return true;
    }
    // `-`: unary if it's in an operand-introducing position. Heuristic:
    // by the time we're calling this with prev=`-`, we'd need to know
    // what came BEFORE the `-`. We don't track that. Treat `Minus` here
    // as "could be unary or binary." Caller (the spacing rule) emits
    // space around it as binary by default, which is fine — same-side
    // tokens like `(` `[` `,` will use the "no space after open" rule
    // first.
    false
}

/// The role a `|` token plays, decided from its non-trivia neighbours. The two
/// delimiters of an `else |Pat|` complement pattern stay tight against the
/// pattern; every other `|` is bitwise-or and gets normal binary spacing.
enum PipeRole {
    /// Opening delimiter of `else |Pat|` (previous non-trivia token is `else`).
    PatternOpen,
    /// Closing delimiter of `else |Pat|` (next non-trivia token is `{`).
    PatternClose,
    /// Ordinary bitwise-or (`a | b`, NS_OPTIONS composition).
    Bitwise,
}

/// Classify the `|` at `idx`. Opening is detected before closing so a pipe right
/// after `else` is always treated as the pattern opener.
fn pipe_role(all: &[Token], idx: usize) -> PipeRole {
    use TokenKind::*;
    let is_trivia = |k: &TokenKind| matches!(k, LineComment(_) | BlockComment(_));
    let prev = all[..idx].iter().rev().map(|t| &t.kind).find(|k| !is_trivia(k));
    if matches!(prev, Some(Else)) {
        return PipeRole::PatternOpen;
    }
    let next = all[idx + 1..].iter().map(|t| &t.kind).find(|k| !is_trivia(k));
    if matches!(next, Some(LBrace)) {
        return PipeRole::PatternClose;
    }
    PipeRole::Bitwise
}

fn is_binary_op(t: &TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        t,
        Plus | Minus
            | Star
            | Slash
            | Percent
            | PlusPercent
            | MinusPercent
            | StarPercent
            | EqEq
            | BangEq
            | Lt
            | Le
            | Gt
            | Ge
            | AmpAmp
            | PipePipe
            | Amp
            | Pipe
            | Caret
            | Shl
            | Shr
            | Eq
            | PlusEq
            | MinusEq
            | StarEq
            | SlashEq
            | PercentEq
            | AmpEq
            | PipeEq
            | CaretEq
            | ShlEq
            | ShrEq
            | Arrow
            | FatArrow
            | As
            | In
    )
    // `Pipe` is the bitwise-or operator (`a | b`, NS_OPTIONS composition) and
    // gets normal binary spacing here. Its other role — the `else |Pat|`
    // complement-pattern delimiter — is kept tight by `pipe_role` at the emit
    // site, which overrides the spacing for those two specific pipes.
    // Excluded here (handled separately):
    //   `DotDot`, `DotDotEq` — range operators; tight on both sides.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_source(src).expect("format")
    }

    #[test]
    fn idempotent_on_empty() {
        assert_eq!(fmt(""), "\n");
    }

    #[test]
    fn idempotent_simple_function() {
        let src = "fn main() -> i32 {\n    return 0;\n}\n";
        let once = fmt(src);
        let twice = fmt(&once);
        assert_eq!(once, twice, "format must be idempotent");
    }

    #[test]
    fn normalizes_operator_spacing() {
        let out = fmt("fn f() -> i32 { return 1+2 ; }\n");
        assert!(out.contains("return 1 + 2;"), "got: {out}");
    }

    #[test]
    fn bitwise_or_is_spaced() {
        // `|` is bitwise-or here; it must be spaced like any binary operator,
        // both inline and across a hand-wrapped continuation (NS_OPTIONS style).
        let out = fmt("fn f(a: u64, b: u64) -> u64 { return a|b; }\n");
        assert!(out.contains("return a | b;"), "inline bitwise-or:\n{out}");
        let wrapped = fmt("fn f() -> u64 {\n    let x: u64 = mask_a\n        | mask_b\n        | mask_c;\n    return x;\n}\n");
        assert!(wrapped.contains("| mask_b") && wrapped.contains("| mask_c"), "wrapped bitwise-or keeps space:\n{wrapped}");
        assert!(!wrapped.contains("|mask"), "no glued pipe:\n{wrapped}");
    }

    #[test]
    fn complement_pattern_pipes_stay_tight() {
        // The `else |Pat|` complement-pattern delimiters hug the pattern, even
        // though bitwise `|` is now spaced. `pipe_role` keeps these two tight.
        let src = "fn f() -> i32 {\n    guard let R::Ok(v) = run() else |R::Err(e)| {\n        return e;\n    }\n    return v;\n}\n";
        let out = fmt(src);
        assert!(out.contains("else |R::Err(e)| {"), "complement pattern stays tight:\n{out}");
    }

    #[test]
    fn preserves_line_comments() {
        let src = "// header\nfn main() -> i32 {\n    return 0; // trailing\n}\n";
        let out = fmt(src);
        assert!(out.contains("// header"), "got: {out}");
        assert!(out.contains("// trailing"), "got: {out}");
    }

    #[test]
    fn preserves_inline_struct_literal_braces() {
        // `Point { x: 1, y: 2 }` should stay one-line in slice 4D.1
        // (preserving formatter — user's line breaks win).
        let src = "struct Point { x: i32, y: i32 }\nfn main() -> i32 { return 0; }\n";
        let out = fmt(src);
        assert!(
            out.contains("struct Point { x: i32, y: i32 }"),
            "got: {out}"
        );
    }

    #[test]
    fn tight_path_separator() {
        let out =
            fmt("enum Color { Red, Blue }\nfn f() -> i32 { let c = Color::Red; return 0; }\n");
        assert!(out.contains("Color::Red"), "got: {out}");
        assert!(!out.contains("Color :: Red"), "got: {out}");
    }

    #[test]
    fn builder_block_at_marker_tight_and_idempotent() {
        // v0.0.22 DSL.1: `@` adjoins the context path, and a builder
        // block with modifier lines round-trips unchanged.
        let src = "fn f() -> i32 {\n    let v = @view {\n        text(\"a\")\n            .font = bigger\n    };\n    return 0;\n}\n";
        let out = fmt(src);
        assert!(out.contains("@view {"), "got: {out}");
        assert!(!out.contains("@ view"), "got: {out}");
        assert_eq!(
            fmt(&out),
            out,
            "format must be idempotent on builder blocks"
        );
    }

    #[test]
    fn builder_block_containers_and_flow_idempotent() {
        // v0.0.22 DSL.4: bare container elements and `if`/`for`
        // item-control round-trip unchanged (token-level formatting).
        let src = "fn f() -> i32 {\n    let v = @view {\n        vstack {\n            text(1)\n            if flag {\n                text(2)\n            }\n            for x in xs {\n                row(x)\n            }\n        }\n    };\n    return 0;\n}\n";
        let out = fmt(src);
        assert_eq!(
            fmt(&out),
            out,
            "format must be idempotent on DSL.4 builder blocks"
        );
    }

    #[test]
    fn collapses_runs_of_blank_lines() {
        let src = "fn f() -> i32 {\n\n\n\n    return 0;\n}\n";
        let out = fmt(src);
        // The inner three blank lines collapse to one blank.
        assert!(out.contains("\n\n    return"), "got: {out:?}");
        assert!(!out.contains("\n\n\n"), "got: {out:?}");
    }

    #[test]
    fn ensures_trailing_newline() {
        let out = fmt("fn f() -> i32 { return 0; }");
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn each_sample_round_trips() {
        // Slice 4D.1 contract: every sample in docs/examples/ formats to
        // itself. If any sample diffs, either the sample or the
        // formatter has drifted.
        use std::fs;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("docs/examples");
        let mut failures: Vec<String> = vec![];
        for entry in fs::read_dir(&dir).expect("read docs/examples").flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("cplus") {
                continue;
            }
            let src = fs::read_to_string(&p).expect("read sample");
            let out = match format_source(&src) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("{}: format error: {e:?}", p.display()));
                    continue;
                }
            };
            if out != src {
                failures.push(format!("{}: differs", p.display()));
            }
            // Idempotence on every sample.
            let again = format_source(&out).expect("re-format");
            if again != out {
                failures.push(format!("{}: not idempotent", p.display()));
            }
        }
        if !failures.is_empty() {
            panic!("formatter regressions:\n  {}", failures.join("\n  "));
        }
    }
}
