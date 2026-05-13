# Phase 4 Slice 4D — `cpc fmt`

> Status: design note, not yet implemented.
> Scope: a single, canonical, opinionated formatter for `.cplus` source. Runs as `cpc fmt FILE` or `cpc fmt PATH/`; in-place by default; `--check` mode for CI; `--stdin` for editor integration.
> Out of scope: comment reflowing of `//` line-comment runs (preserve as-is), per-project style configuration (the formatter is single-canonical, no knobs), import-graph-aware sorting (alphabetic only).

---

## 1. Problem

`cpc fmt` is the second §5.4 built-in subcommand to land after `cpc build`. §5.9 calls it out as a load-bearing piece of the AI-recovery loss function: **canonical formatter output keeps diffs between AI revisions small because formatting doesn't drift**. Two LLM passes over the same code should produce textually identical output if the structure is the same. With `cpc fmt` between them, they do.

Three concrete properties we need:

1. **Determinism.** Same AST → same bytes. Idempotent: `fmt(fmt(x)) == fmt(x)`.
2. **Opinionated.** No `.fmt.toml`, no `--style=k_and_r`, no per-project overrides. The §5.3 principle ("same inputs → byte-identical outputs across machines") only holds if the formatter is the same everywhere.
3. **Comment-preserving.** Comments are not in the AST; they live in the source token stream. A formatter that drops them is a non-starter, so the trivia layer is a design point, not an implementation detail.

The big risk to dodge: a formatter that fights the user. `rustfmt`'s reputation for re-flowing chained-call sites in surprising ways is the canonical anti-pattern. C+ stays conservative — when in doubt, preserve.

---

## 2. CLI surface

```
cpc fmt FILE.cplus              format FILE in place
cpc fmt PATH/                   format every .cplus file under PATH/ recursively
cpc fmt --check FILE            exit non-zero if FILE would change; print diff to stderr
cpc fmt --stdin < a > b         format stdin → stdout (editor integration)
cpc fmt --emit FILE             print formatted output to stdout; leave FILE alone
```

Defaults: in-place rewrite, no backup file. The user runs `cpc fmt` after `git add` if they care about the pre-state.

`--check` exit code:
- `0` — already formatted; no changes.
- `1` — would reformat; diff to stderr.
- `2` — fatal error (file unreadable, parse error in input).

`--check` does NOT modify files. Designed for CI; suitable for a pre-commit hook.

Parse errors in the input are reported via the structured `Diagnostic` pipeline same as `cpc build` (so `cpc fmt --check --diagnostics=json` works in CI). Formatting an unparseable file is undefined and the diagnostic exits with code 2.

---

## 3. Style decisions (locked in)

These are not configurable. They were picked to match the §2.8a style migration's existing conventions where possible and to minimize diff churn otherwise.

### 3.1 Indentation

- **4 spaces.** No tabs anywhere — including string contents the formatter writes (it doesn't). The lexer accepts tabs; the formatter normalizes them out.
- Block contents indent one level past the brace's column.
- Continuation indent (a `let` initializer wrapped across lines, a long parameter list) is one extra level — 4 more spaces.

### 3.2 Line length

- **Target 100 columns.** Soft target — the formatter prefers a line under 100 but does not force-break expressions that exceed it. Long string literals, long path identifiers, and chained calls that don't have a natural break point can exceed the target.
- The formatter never breaks inside an identifier, string literal, or operator.

### 3.3 Braces

- **K&R.** Opening brace on the same line as the function / struct / `if` / `while` / `for` / `impl` / `match`. Closing brace on its own line at the construct's indent.
- One space before the opening brace: `fn f() {`, `if c {`, `} else {`.
- Empty blocks render `{}` on one line: `fn nothing() {}`, `impl T {}`.

### 3.4 Operators and punctuation

- One space around binary operators (`+`, `-`, `*`, `/`, `%`, `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`, `|`, `&`, `^`, `<<`, `>>`, `=`, `+=`, …). Same for `as`, range `..` / `..=`.
- No space after unary prefix operators (`-x`, `!x`, `~x`).
- No space inside parens or brackets: `f(x)`, `a[i]`. One space after `,`.
- Space after `:` in `let x: T`, `field: T`, struct literal `f: e`. No space before.
- `::` is tight on both sides: `Color::Red`, `math::square`. No spaces.
- `.` is tight on both sides: `p.x`, `v.method()`. No spaces.

### 3.5 Statements

- One statement per line. Exception: very short `if x { return y; }` on one line is **not** allowed — always expand to multiple lines. The cost is a few extra rows; the win is a stable diff when the body grows.
- `;` immediately follows the preceding token. No space before.
- Trailing comma on the last element of a multi-line list (function params, struct literal fields, array literals, match arms, struct fields, enum variants). Single-line lists do NOT carry a trailing comma. Rationale: this is the Rust idiom and it keeps diffs short when the list gets a new element.

### 3.6 Items at file top

Ordering for items at the top of a file:

1. `import` declarations, sorted lexicographically by path string. One per line. Blank line after the last import.
2. Type definitions (`struct`, `enum`) in source order — the formatter does NOT reorder these. Rationale: items have semantic dependencies (one struct may reference another); reordering would break source-order documentation conventions and confuses readers.
3. `impl` blocks in source order.
4. `fn` declarations in source order.

Blank lines between top-level items: exactly one. The formatter normalizes runs of blank lines to single blanks at the top level.

### 3.7 Vertical whitespace inside function bodies

- The formatter preserves the user's blank-line placement inside function bodies *up to a maximum of one consecutive blank line*. `\n\n\n\n` collapses to `\n\n`.
- No blank line immediately after `{` or immediately before `}`.

### 3.8 No alignment

The formatter does NOT align struct field colons, variable colons, comments, or anything else. Rationale: alignment columns shift when one element changes, producing large diffs for small changes. The samples in `docs/examples/` already follow this convention.

### 3.9 Comments

Comments live in three positions: their column placement and content are **preserved verbatim**.

- **Trailing comments** (after a token on the same line): kept on that line with one space between the last token and `//`.
- **Leading comments** (above a statement / item): kept above, at the same indent as the following construct.
- **Floating comments** (a comment block separated from any item by blank lines): kept where they are, indented to the surrounding scope.

The formatter NEVER rewraps `//` runs to fit line length, NEVER converts `//` to `/* */`, NEVER reflows `///` doc-comment text. Comments are a write-only protocol from the user's perspective.

Block comments `/* ... */` are preserved character-for-character (including internal indentation).

### 3.10 Specific construct rules

**Function definitions** —

```cp
fn f(x: i32, y: i32) -> i32 {
    return x + y;
}
```

Multi-line param list (when the single-line form exceeds 100 columns):

```cp
fn long_name(
    first_param: SomeType,
    second_param: AnotherType,
    third_param: YetAnotherType,
) -> ResultType {
    ...
}
```

**Struct literals** — single line if it fits; otherwise one field per line with trailing comma.

```cp
let p = Point { x: 1, y: 2 };
let big = SomeStruct {
    field_a: 1,
    field_b: 2,
    field_c: 3,
};
```

**`match`** — block-arm bodies always on multiple lines; short-form arm bodies (single expression followed by `,`) stay one-line when the whole arm fits.

```cp
match m {
    Maybe::Some(v) => v,
    Maybe::None => 0,
}
```

```cp
match shape {
    Shape::Circle(r) => {
        let area = r * r * 3;
        return area;
    }
    Shape::Square(s) => s * s,
}
```

The formatter does not "promote" a block-arm to short-form even if its body is trivial. User intent wins.

**`if` / `else`** — `else` on the same line as the preceding `}`. `else if` is a tight chain.

```cp
if c {
    return 1;
} else if d {
    return 2;
} else {
    return 3;
}
```

**`import`** —

```cp
import "math.cplus" as math;
import "util/strings.cplus" as strings;
```

Sorted by path string. One blank line between the import block and the first item.

**`guard let` / `if let`** — same rules as `if` / `else`. Multi-line bodies.

```cp
guard let Ok(content) = read_file(path) else {
    return Err(io_error());
};
```

---

## 4. Comment-handling architecture

This is the part that needs design work, not just decisions.

### 4.1 The trivia problem

The current lexer discards whitespace and comments — they're not in the token stream. The parser sees a clean token list with no idea where comments were. A formatter built on the AST alone cannot recover comment positions.

Two general approaches:

- **(A) Comment-aware AST** — attach leading/trailing trivia to each AST node. The parser builds this from the token stream.
- **(B) Token-stream walker** — keep the AST as-is. Re-lex the input alongside formatting, and emit comment tokens at their original byte positions.

**Decision: (B), augmented.** Approach (A) is invasive (every AST node changes), and it makes the parser bigger. Approach (B) localizes the change to the formatter and the lexer.

Concretely:
- **Lexer change**: optionally emit `TokenKind::LineComment(String)` and `TokenKind::BlockComment(String)` tokens instead of skipping them. A new `tokenize_with_trivia(src)` entry point keeps the existing `tokenize` clean. Spans + bytes already work.
- **Formatter**: parse the source into AST as today; also re-lex with trivia. Build a sorted `Vec<(Span, CommentKind, String)>`. While printing the AST, before emitting any token whose source span starts at byte B, drain comments whose end-byte ≤ B and emit them at the right indent.

This preserves the user's lexical comment placement without polluting the AST.

### 4.2 Trivia ↔ AST sequencing

Three placements emerge naturally from the byte-position queue:

- A comment whose span sits **immediately before** an item / statement (no source code between them, only whitespace) → emitted as a **leading** comment.
- A comment whose span starts **on the same source line** as a token already emitted → emitted as a **trailing** comment after that line.
- A comment **separated by ≥ 1 blank line** from any item → emitted as a **floating** comment block at the surrounding indent.

These rules are mechanically derivable from byte spans + the formatter's per-line state.

### 4.3 Inside expressions

Comments inside an expression (e.g., between a function call's args) are weird but legal source. The formatter places them on the line where they appeared in the input, with whitespace adjusted to fit the formatted surrounding code. This may produce slightly unusual output but is preferable to silently dropping the comment or aborting.

---

## 5. Implementation plan

### 5.1 New module: `cplus-core/src/fmt.rs`

Public entry:

```rust
pub fn format_source(src: &str) -> Result<String, FormatError>;
pub fn format_check(src: &str) -> Result<Diff, FormatError>;  // Phase 4D.2
```

`Diff` is a small data structure carrying line-level deltas, suitable for human rendering and CI consumption.

`FormatError` wraps lex/parse failures via the existing `Diagnostic` machinery — no new error codes; formatter inherits E00xx / E01xx.

### 5.2 Lexer extension

- Add `TokenKind::LineComment(String)` and `TokenKind::BlockComment(String)` (content excludes the comment markers).
- Add `pub fn tokenize_with_trivia(src: &str) -> Result<Vec<Token>, LexError>`.
- Existing `tokenize` calls `tokenize_with_trivia` and filters out trivia tokens. Zero behavior change for current consumers.

### 5.3 Printer

Plain string-builder approach. Recursive AST walk; per-node `print_*` functions. State: current indent, current column, output buffer, trivia queue (peek and drain by byte position).

Pretty-printing libraries (Wadler's, Hughes-style) are tempting but overkill here. The C+ AST is shallow and the layout rules are local — a hand-written printer is shorter to write, easier to predict, and trivially deterministic.

### 5.4 Driver: `cpc fmt`

Three flags from §2:

```
cpc fmt PATH                    in-place
cpc fmt --check PATH            no-write, diff to stderr, exit code
cpc fmt --stdin                 stdin → stdout
cpc fmt --emit FILE             print to stdout
```

Recursive directory walking respects `.gitignore` only insofar as we honor `target/` — a hardcoded skip list (`target/`, `node_modules/`, `.git/`) keeps things predictable in Phase 4. Real ignore-file support is a polish item.

### 5.5 Idempotence test

Every sample in `docs/examples/` and every multi-file sample under `docs/examples/projects/` runs through:

```
format_source(format_source(src)) == format_source(src)
```

This is a unit test, not e2e — bytes-equal comparison is fast and the fail-mode is obvious. Failure means there's a bug in the printer.

Additionally, all in-tree samples that already compile MUST be unchanged by formatting. The samples were written in the §2.8a style; if `cpc fmt` produces a diff against any of them, either the sample or the formatter has drifted — both worth catching.

### 5.6 Slice split

- **4D.1** — Lexer trivia extension; `fmt::format_source` for the common cases (functions, struct literals, `match`, `if`/`else`, `import` block, basic statements). Idempotence test on every existing sample.
- **4D.2** — `cpc fmt` driver wiring (`--check`, `--stdin`, `--emit`, in-place). Diff struct. CI-ready.
- **4D.3** — Edge cases: comments inside expressions, very long parameter lists, multi-line `match` arm bodies, indented `else if` chains.

If 4D.1 lands and proves the architecture, 4D.2 is mechanical. 4D.3 is iterative polish driven by what real C+ code does once we have more of it.

---

## 6. Interactions

### 6.1 Module system (slice 4A)

The formatter respects the module note's stance that directories are organizational only. `cpc fmt PATH/` walks the filesystem; whether files are part of any actual `cpc build` is irrelevant — formatter touches them anyway. (`cpc fmt --check` in CI catches stale, unparseable, or unused `.cplus` files too.)

`import` declarations sort lexicographically by their path string, not by the `as` name. Rationale: the path is the file identity; the alias is the local-only handle. Sorting by path keeps the import block stable across `as`-renames.

### 6.2 Diagnostics

`cpc fmt` and `cpc build` share the diagnostic pipeline. A formatter that runs on unparseable input emits the same `Diagnostic` that `cpc build` would; no formatter-specific error codes.

### 6.3 LSP (slice 4E, future)

The same `fmt::format_source` function powers an LSP `textDocument/formatting` request. Compiler-as-library (§5.1) pays off here — the LSP doesn't reimplement layout, it just calls the library.

### 6.4 `cpc fmt` ↔ samples

After 4D.1 lands, run `cpc fmt --check` on every sample in `docs/examples/`. Any diff = decide whether the sample or the formatter is wrong, then fix one of them. (Pre-emptively, the samples were written in §2.8a style and should round-trip cleanly.)

---

## 7. Resolved decisions (locked in 2026-05-11)

User confirmed the §3 style block and the leans below in one motion ("go ahead — I like the formatting in docs/examples"). The samples in `docs/examples/` are now the de facto specification: `cpc fmt --check` over every sample must produce zero diff.

- **Line-length target: 100 columns.** Soft target; the formatter never breaks inside an identifier / literal / operator. Long lines that have no natural break point are left alone.
- **Trailing-comma policy: trailing on multi-line, none on single-line.** Rust idiom; locked.
- **Alignment: never align anything.** Following `rustfmt`. Diffs stay minimal when one element of a list changes.
- **Collapse user's multi-line wraps when result fits the line target.** Predictability beats preservation. If a binary expression wraps to two lines but the formatter can fit it on one ≤ 100 columns, it goes on one line. Multi-line stays multi-line only at "real" delimiters (commas in arg/field lists, `=>` in `match` arms).
- **Import sort: by quoted path string.** The path is the file identity; the `as` alias is local. Path sort stays stable across `as`-renames.
- **`--check` output: unified-diff format on stderr.** Matches `gofmt -d` / `rustfmt --check`. Tools already parse this; nothing new to learn.

---

## 8. Non-goals

- No configurability. No `cpc.fmt.toml`. The §5.3 determinism principle eats any "let users tweak the style" wishlist.
- No comment reflow. We don't rewrap `//` line comments to fit a column. Users write what they mean.
- No magic — no expression-rewriting, no automatic `if x { ... } else { ... }` → `let x = if ... else ...;` conversion, nothing the user didn't ask for.
- No re-sorting of struct fields, enum variants, function arguments, or top-level item ordering (beyond the `import` block sort in §3.6). Source order is preserved.
- No "smart" line breaking heuristics. The two rules are: fits-on-one-line vs doesn't. If it doesn't fit, break at the natural delimiters (commas in argument/field lists, `=>` in match arms).

---

## 9. Summary

A single, opinionated, non-configurable formatter. Approach: parse + re-lex with trivia, then walk the AST and a sorted comment queue together to produce output. Idempotent. Three slices: lexer trivia + printer (4D.1) → driver wiring (4D.2) → expression-internal edge cases (4D.3). The hard part is comment preservation; the rest is bookkeeping.

Bytes spent on this saved a hundredfold in not bikeshedding style debates later.
