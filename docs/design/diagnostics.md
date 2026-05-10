# Diagnostics — Design Note

> Status: draft
> Lands in: Phase 1, before sema (sema is the biggest producer of diagnostics)
> Spec authority: plan.md §5.2

## 1. Problem

C+ is AI-native. Tools (LSP, AI agents, CI parsers) need machine-readable errors with machine-applicable fix suggestions. Bolting structured output on later means rewriting every error site. Designing it in now costs a few hundred lines.

The format is also part of the language's stable interface: once tools depend on the JSON shape, changing it is a breaking change.

## 2. Diagnostic shape

Every error, warning, or note flows through one type:

```rust
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagCode,
    pub message: String,           // one-line summary
    pub primary: SourceSpan,       // where the error occurred
    pub labels: Vec<Label>,        // additional spans with explanations
    pub notes: Vec<String>,        // informational lines, no spans
    pub suggestions: Vec<Suggestion>,
}

pub enum Severity { Error, Warning, Note }

pub struct DiagCode(pub &'static str);  // "E0102", "W0003"

pub struct SourceSpan {
    pub file: PathBuf,
    pub start: Position,
    pub end: Position,
}

pub struct Position {
    pub line: u32,    // 1-based
    pub col: u32,     // 1-based, in chars (not bytes)
    pub byte: u32,    // 0-based byte offset, for tools that prefer byte spans
}

pub struct Label {
    pub span: SourceSpan,
    pub message: String,
}

pub struct Suggestion {
    pub description: String,        // what this fix does
    pub span: SourceSpan,           // what region to replace
    pub replacement: String,        // new text
    pub applicability: Applicability,
}

pub enum Applicability {
    MachineApplicable,    // safe to apply without review
    MaybeIncorrect,       // probably right; suggests review
    HasPlaceholders,      // replacement contains placeholders the user must fill
    Unspecified,          // no claim
}
```

All of these derive `Serialize` (for JSON output) and `Deserialize` (so tools can round-trip).

## 3. Error code allocation

A `DiagCode` is a stable identifier — e.g. `E0102` always means "expected `;` after expression." Codes are documented; tools can suppress, link, or auto-fix by code.

Allocation by compiler phase:

| Range          | Phase                          |
|----------------|--------------------------------|
| `E0001`–`E0099` | Lexer                          |
| `E0100`–`E0299` | Parser                         |
| `E0300`–`E0599` | Sema (name res + type check)   |
| `E0600`–`E0799` | Borrow checker                 |
| `E0800`–`E0899` | Codegen                        |
| `E0900`–`E0999` | Driver / link / IO             |
| `W0001`–`W0099` | Warnings (any phase)           |

Codes are never reused. Retiring a code marks it deprecated in docs but the slot stays empty. Each new code is added to `docs/diagnostics/INDEX.md` (lazily — start when we hit ~10 codes).

## 4. Output formats

`cpc` accepts `--diagnostics=<mode>`. Three modes for now:

- **`human`** (default): Rust-style rendering. Source snippet, ANSI color, carets, side-by-side fix preview. Pretty for terminals.
- **`json`**: NDJSON — one `Diagnostic` per line. Easy to stream, line-by-line parseable, no end-of-stream marker required. The format every tool consumes.
- **`short`**: one line per diagnostic, `file:line:col: severity[code]: message`. Compatible with most editor regex parsers as a fallback.

Both `human` and `json` are produced from the same `Diagnostic` struct. The renderer is a thin layer; the data model is the contract.

### NDJSON example

```
{"severity":"error","code":"E0102","message":"expected `;` after expression","primary":{"file":"foo.cplus","start":{"line":12,"col":5,"byte":234},"end":{"line":12,"col":5,"byte":234}},"labels":[],"notes":[],"suggestions":[{"description":"insert `;`","span":{"file":"foo.cplus","start":{"line":12,"col":5,"byte":234},"end":{"line":12,"col":5,"byte":234}},"replacement":";","applicability":"machine_applicable"}]}
```

(Pretty-printed for readability; on the wire it's one line.)

## 5. Suggestions and applicability

Every suggestion is a `(span, replacement)` pair plus an `Applicability` claim. Examples:

| Error                          | Suggestion span        | Replacement | Applicability        |
|-------------------------------|------------------------|-------------|----------------------|
| missing `;`                    | empty range at err pos | `;`         | `MachineApplicable`  |
| `let x = 1; x = 2;` (immut)    | the `let`              | `let mut`   | `MachineApplicable`  |
| `if 1 { … }`                   | `1`                    | `1 != 0`    | `MaybeIncorrect`     |
| typo `lenght`                  | the typo               | `length`    | `MaybeIncorrect`     |
| missing return type            | range after `)`        | `-> i32`    | `HasPlaceholders`    |

`MachineApplicable` means "an AI or formatter can apply this fix and the result is correct C+ that resolves the diagnostic." `MaybeIncorrect` means "probably right but may change semantics." Tools choose which to auto-apply based on confidence policy.

Multiple suggestions per error are allowed (alternatives). The first is the recommended one.

## 6. How errors flow through the compiler

Each compiler phase produces a `Result<T, Vec<Diagnostic>>` or accumulates into a `DiagSink` (TBD pick one — probably `DiagSink` so we can collect multiple errors per file and continue past the first).

```rust
pub trait DiagSink {
    fn emit(&mut self, d: Diagnostic);
}
```

For Phase 1, a simple `Vec<Diagnostic>` collector suffices. Later phases can add a streaming sink for the LSP path.

The legacy `LexError` and `ParseError` types implement `Into<Diagnostic>`. Eventually they're replaced by direct `Diagnostic` construction; for now they bridge the existing code.

## 7. Span representation

The lexer already produces `Span { start: u32, end: u32 }` byte offsets. The diagnostic emitter converts these to `SourceSpan` with line/col by walking the source once and building a `LineMap`.

```rust
pub struct LineMap {
    line_starts: Vec<u32>,  // byte offset of the start of each line
}
```

Built once per compilation unit, queried per diagnostic. O(log n) per query via binary search.

## 8. Stability

The JSON shape is **stable from Phase 1**. Adding fields is OK if they're optional. Removing or renaming fields is a breaking change and bumps the diagnostic-format version.

The CLI flag for JSON output is committed: `--diagnostics=json`. Renaming it would break every tool ever written.

The error code namespace is stable. A specific code's *message* may improve over time; its *meaning* must not change.

## 9. Phase 1 scope

Implement the full structure. Connect:

- `LexError` → `Diagnostic` (covers ~5 codes: `E0001`–`E0010` range)
- `ParseError` → `Diagnostic` (~10 codes: `E0100`–`E0120` range)
- A `DiagSink` collector
- `cpc --diagnostics=json` produces NDJSON for any compilation
- `cpc --diagnostics=human` (default) produces human output (basic; polish in later phases)

Tests:

- Unit: `Diagnostic` round-trips through JSON.
- Unit: `LineMap` correctly maps byte offsets to line/col.
- Negative: each Phase-1 error case from `phase1-grammar.md` §7.2 produces a diagnostic with the expected code.
- Snapshot: a known-bad program emits a known JSON sequence (frozen).

## 10. Out of scope (later phases)

- Color customization, themes
- Multi-file diagnostics (waits for Phase 4 modules)
- Streaming sink for LSP (Phase 4)
- Diagnostic suppression / `#[allow(...)]` attributes
- IDE-specific extensions (related code-actions, quick-fix metadata beyond the basic suggestion)
- Internationalization

## 11. Open questions

- `DiagSink` trait vs concrete `Vec<Diagnostic>`. Lean: trait, but start with `Vec` and refactor when LSP arrives.
- Whether `Position::col` is in chars or grapheme clusters. Lean: chars (UTF-8 codepoint count). Clusters needed when emoji/CJK matters; defer.
- Whether to emit a `version` field in JSON output. Lean: yes, single global `"diagnostics_version": 1` once per compilation, separate from the per-diagnostic stream. Or always include in every line. TBD.
- Mapping from internal phase-specific error enums to `DiagCode` — central registry vs inline. Lean: inline (each error site picks its code), central index doc.

## 12. Sample human-mode rendering

Target shape (mimics rustc; refined later):

```
error[E0102]: expected `;` after expression
  --> foo.cplus:12:5
   |
12 |     let x = 1
   |              ^ expected `;`
   |
help: insert `;`
   |
12 |     let x = 1;
   |              +
```

Implementation in Phase 1 can be simpler — just file:line:col + message + caret. The full format is the eventual target.
