//! Structured diagnostics infrastructure.
//!
//! Every error and warning flows through `Diagnostic`. Renderers (human,
//! NDJSON, short-form) are downstream of this single data model. See
//! `docs/design/diagnostics.md` for the full design.

use crate::lexer::{LexError, LexErrorKind, Span as ByteSpan};
use crate::parser::{ParseError, ParseErrorKind};
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity { Error, Warning, Note }

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct DiagCode(pub &'static str);

impl fmt::Display for DiagCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.0) }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Position {
    pub line: u32,    // 1-based
    pub col: u32,     // 1-based, in chars
    pub byte: u32,    // 0-based byte offset
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceSpan {
    pub file: PathBuf,
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Label {
    pub span: SourceSpan,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
    HasPlaceholders,
    Unspecified,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Suggestion {
    pub description: String,
    pub span: SourceSpan,
    pub replacement: String,
    pub applicability: Applicability,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagCode,
    pub message: String,
    pub primary: SourceSpan,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Label>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
}

impl Diagnostic {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Diagnostic serialization is infallible")
    }

    pub fn render_short(&self) -> String {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        format!(
            "{}:{}:{}: {sev}[{}]: {}",
            self.primary.file.display(),
            self.primary.start.line,
            self.primary.start.col,
            self.code,
            self.message
        )
    }

    /// Phase-1 human renderer: short form plus optional source snippet. Polished
    /// later (caret underline, suggestion preview, ANSI color).
    pub fn render_human(&self, src: &str) -> String {
        let mut out = self.render_short();
        if let Some(snippet) = render_snippet(&self.primary, src) {
            out.push('\n');
            out.push_str(&snippet);
        }
        // Phase 11 polish (2026-05-13): render secondary labels as
        // "note: <message>" lines with their own file:line:col anchor
        // plus a source snippet. Borrow-conflict diagnostics use this
        // to surface the "borrowed here" / "moved here" partner span
        // so users see both ends of the conflict.
        for l in &self.labels {
            out.push_str(&format!(
                "\n  {}:{}:{}: note: {}",
                l.span.file.display(),
                l.span.start.line,
                l.span.start.col,
                l.message,
            ));
            if let Some(snippet) = render_snippet(&l.span, src) {
                out.push('\n');
                out.push_str(&snippet);
            }
        }
        for n in &self.notes {
            out.push_str(&format!("\n  = note: {n}"));
        }
        for s in &self.suggestions {
            out.push_str(&format!(
                "\n  = help: {} (replace with {:?})",
                s.description, s.replacement
            ));
        }
        out
    }
}

fn render_snippet(span: &SourceSpan, src: &str) -> Option<String> {
    let line_idx = (span.start.line as usize).checked_sub(1)?;
    let line = src.lines().nth(line_idx)?;
    Some(format!("  | {line}"))
}

// ---- LineMap: byte offset → line/col ----

#[derive(Debug, Clone)]
pub struct LineMap {
    line_starts: Vec<u32>,
}

impl LineMap {
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        Self { line_starts }
    }

    pub fn position(&self, byte: u32, src: &str) -> Position {
        let line_idx = self.line_starts.partition_point(|&s| s <= byte).saturating_sub(1);
        let line_start = self.line_starts[line_idx];
        let line_byte = (byte as usize).min(src.len());
        let line_text = &src[line_start as usize..line_byte];
        let col = line_text.chars().count() as u32 + 1;
        Position { line: (line_idx + 1) as u32, col, byte }
    }

    pub fn span(&self, file: &PathBuf, span: ByteSpan, src: &str) -> SourceSpan {
        SourceSpan {
            file: file.clone(),
            start: self.position(span.start, src),
            end: self.position(span.end, src),
        }
    }
}

// ---- DiagSink: accumulate diagnostics during a compilation pass ----

#[derive(Debug, Default, Clone)]
pub struct DiagSink {
    diags: Vec<Diagnostic>,
}

impl DiagSink {
    pub fn new() -> Self { Self::default() }
    pub fn emit(&mut self, d: Diagnostic) { self.diags.push(d); }
    pub fn diagnostics(&self) -> &[Diagnostic] { &self.diags }
    pub fn into_vec(self) -> Vec<Diagnostic> { self.diags }
    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| matches!(d.severity, Severity::Error))
    }
}

// ---- Conversions from existing error types ----

pub fn from_lex(err: &LexError, file: &PathBuf, lm: &LineMap, src: &str) -> Diagnostic {
    let primary = lm.span(file, err.span, src);
    let (code, message, suggestions) = match &err.kind {
        LexErrorKind::UnexpectedChar(c) => (
            DiagCode("E0001"),
            format!("unexpected character `{c}`"),
            Vec::new(),
        ),
        LexErrorKind::UnterminatedBlockComment => (
            DiagCode("E0002"),
            "unterminated block comment".to_string(),
            // We don't know where the user wanted to close it; skip suggestion.
            Vec::new(),
        ),
        LexErrorKind::UnterminatedString => (
            DiagCode("E0005"),
            "unterminated string literal".to_string(),
            Vec::new(),
        ),
        LexErrorKind::InvalidNumber(s) => (
            DiagCode("E0003"),
            format!("invalid number literal: {s}"),
            Vec::new(),
        ),
        LexErrorKind::InvalidNumSuffix(s) => (
            DiagCode("E0004"),
            format!("invalid numeric type suffix `{s}`; expected one of i8/i16/i32/i64/u8/u16/u32/u64/isize/usize/f32/f64"),
            Vec::new(),
        ),
    };
    Diagnostic {
        severity: Severity::Error,
        code,
        message,
        primary,
        labels: Vec::new(),
        notes: Vec::new(),
        suggestions,
    }
}

pub fn from_parse(err: &ParseError, file: &PathBuf, lm: &LineMap, src: &str) -> Diagnostic {
    let primary = lm.span(file, err.span, src);
    let (code, message, suggestions) = match &err.kind {
        ParseErrorKind::Unexpected { found, expected } => (
            DiagCode("E0100"),
            format!("expected {expected}, found {found}"),
            // Common case: missing `;`. Suggest insertion at the current position.
            if *expected == "`;`" || expected.contains("`;`") {
                vec![Suggestion {
                    description: "insert `;`".to_string(),
                    span: SourceSpan {
                        file: primary.file.clone(),
                        start: primary.start,
                        end: primary.start,
                    },
                    replacement: ";".to_string(),
                    applicability: Applicability::MaybeIncorrect,
                }]
            } else {
                Vec::new()
            },
        ),
        ParseErrorKind::UnexpectedEof { expected } => (
            DiagCode("E0101"),
            format!("unexpected end of input, expected {expected}"),
            Vec::new(),
        ),
        ParseErrorKind::NonChainableComparison => (
            DiagCode("E0102"),
            "comparison operators are non-chainable; use `&&` between comparisons".to_string(),
            // We could suggest `a < b && b < c` but synthesis is fragile;
            // leave as a note instead.
            Vec::new(),
        ),
    };
    Diagnostic {
        severity: Severity::Error,
        code,
        message,
        primary,
        labels: Vec::new(),
        notes: Vec::new(),
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb(s: &str) -> PathBuf { PathBuf::from(s) }

    #[test]
    fn line_map_basic() {
        let src = "a\nbb\nccc";
        let lm = LineMap::new(src);
        assert_eq!(lm.position(0, src), Position { line: 1, col: 1, byte: 0 });
        assert_eq!(lm.position(1, src), Position { line: 1, col: 2, byte: 1 });
        assert_eq!(lm.position(2, src), Position { line: 2, col: 1, byte: 2 });
        assert_eq!(lm.position(4, src), Position { line: 2, col: 3, byte: 4 });
        assert_eq!(lm.position(5, src), Position { line: 3, col: 1, byte: 5 });
    }

    #[test]
    fn line_map_handles_eof() {
        let src = "abc";
        let lm = LineMap::new(src);
        // Querying byte == len (one past end, span end) should not panic.
        let p = lm.position(3, src);
        assert_eq!(p.line, 1);
        assert_eq!(p.col, 4);
    }

    #[test]
    fn diag_serializes_to_json_and_round_trips_shape() {
        let d = Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0102"),
            message: "expected `;` after expression".to_string(),
            primary: SourceSpan {
                file: pb("foo.cplus"),
                start: Position { line: 12, col: 5, byte: 234 },
                end: Position { line: 12, col: 5, byte: 234 },
            },
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: vec![Suggestion {
                description: "insert `;`".to_string(),
                span: SourceSpan {
                    file: pb("foo.cplus"),
                    start: Position { line: 12, col: 5, byte: 234 },
                    end: Position { line: 12, col: 5, byte: 234 },
                },
                replacement: ";".to_string(),
                applicability: Applicability::MachineApplicable,
            }],
        };
        let json = d.to_json();
        // Spot-check: known-good substrings.
        assert!(json.contains("\"severity\":\"error\""));
        assert!(json.contains("\"code\":\"E0102\""));
        assert!(json.contains("\"applicability\":\"machine_applicable\""));
        assert!(json.contains("\"replacement\":\";\""));
        // Fields with empty vecs are omitted.
        assert!(!json.contains("\"labels\""));
        assert!(!json.contains("\"notes\""));
    }

    #[test]
    fn from_lex_assigns_correct_code() {
        let src = "@";
        let err = crate::lexer::tokenize(src).unwrap_err();
        let lm = LineMap::new(src);
        let d = from_lex(&err, &pb("test.cplus"), &lm, src);
        assert_eq!(d.code, DiagCode("E0001"));
        assert_eq!(d.severity, Severity::Error);
    }

    #[test]
    fn from_parse_chainable_cmp() {
        let toks = crate::lexer::tokenize("fn main() -> i32 { 1 < 2 < 3 }").unwrap();
        let err = crate::parser::parse(toks).unwrap_err();
        let lm = LineMap::new("fn main() -> i32 { 1 < 2 < 3 }");
        let d = from_parse(&err, &pb("test.cplus"), &lm, "fn main() -> i32 { 1 < 2 < 3 }");
        assert_eq!(d.code, DiagCode("E0102"));
    }

    #[test]
    fn from_parse_missing_semi_suggests_fix() {
        let src = "fn main() -> i32 { let x = 1 0 }";
        let toks = crate::lexer::tokenize(src).unwrap();
        let err = crate::parser::parse(toks).unwrap_err();
        let lm = LineMap::new(src);
        let d = from_parse(&err, &pb("test.cplus"), &lm, src);
        assert_eq!(d.code, DiagCode("E0100"));
        assert_eq!(d.suggestions.len(), 1);
        assert_eq!(d.suggestions[0].replacement, ";");
    }

    #[test]
    fn diag_sink_accumulates() {
        let mut sink = DiagSink::new();
        assert!(!sink.has_errors());
        sink.emit(Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0001"),
            message: "x".into(),
            primary: SourceSpan {
                file: pb("a"),
                start: Position { line: 1, col: 1, byte: 0 },
                end: Position { line: 1, col: 1, byte: 0 },
            },
            labels: Vec::new(), notes: Vec::new(), suggestions: Vec::new(),
        });
        assert!(sink.has_errors());
        assert_eq!(sink.diagnostics().len(), 1);
    }

    #[test]
    fn render_short_format() {
        let d = Diagnostic {
            severity: Severity::Error,
            code: DiagCode("E0001"),
            message: "boom".to_string(),
            primary: SourceSpan {
                file: pb("foo.cplus"),
                start: Position { line: 12, col: 5, byte: 0 },
                end: Position { line: 12, col: 5, byte: 0 },
            },
            labels: Vec::new(), notes: Vec::new(), suggestions: Vec::new(),
        };
        assert_eq!(d.render_short(), "foo.cplus:12:5: error[E0001]: boom");
    }
}
