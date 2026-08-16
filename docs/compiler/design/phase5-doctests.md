# Phase 5 — Doctests

> Status: design note. Implementation lands after `cpc test` (which lands after attributes — see Phase 5 sequencing in [plan.md](../../plan.md) §3).
> Depends on: [phase5-attributes.md](phase5-attributes.md) (the `#[test]` attribute machinery and the `cpc test` runner).

---

## 1. Problem

§5.6 commits to doctests: `///` doc comments may contain `assert` lines that the test runner extracts, compiles, and runs. The reason — also from §5.6 — is "AI is excellent at writing doc comments with examples; this gives those examples teeth." Doc comments without verification rot. The first time someone refactors `parse(s)` from returning `i32` to returning `Result[i32]`, every `assert parse("1") == 1;` in a doc comment becomes silently false; without compile-and-run extraction, no one notices until a reader copies the example into a program and is confused.

The competing constraint is §2.8: no macros, no compile-time AST transformation, no expression-level compile-time computation. Doctests sit in tension with that — extracting code from a comment string and turning it into a compiled function is, mechanically, the kind of "the source isn't what runs" pattern §2.8 rejects. The resolution is the same as §2.8d for attributes: doctests are processed by a **separate pre-compile pass** that emits ordinary C+ source (synthesized `#[test]` functions) for the regular pipeline. The extractor is a compiler tool, not user-writable, not a language feature; it is exactly as transparent as `cpc fmt` or `cpc parse`.

This note specifies extraction syntax, scope rules, error attribution, and interactions.

---

## 2. Syntax

### 2.1 Doc-comment marker

`///` (triple-slash) starts a doc comment. The marker reuses Rust's; it is conceptually distinct from `//` (ordinary line comment) and recognized at the lexer level. A doc comment attaches to the item that immediately follows it.

```cp
/// Returns the larger of two i32s.
///
/// ```
/// assert max(1, 2) == 2;
/// assert max(-5, -10) == -5;
/// ```
pub fn max(a: i32, b: i32) -> i32 {
    if a > b { return a; } else { return b; }
}
```

`/**...*/` block doc comments are **not** in Phase 5. They can be added later if a real use case appears (the multi-line `///` form covers every case we've seen and is simpler to lex).

### 2.2 Doctest fence

Examples live inside a triple-backtick fence inside the doc comment. Outside the fence, the doc comment is human prose. Inside the fence, the content is C+ source.

Fences:
- ```` ``` ```` opens; ```` ``` ```` on its own line closes. No language tag — `cplus` is implied; future flavors (e.g. ```` ```ignore ````, ```` ```no_run ````) can be added when the runner needs them, with each tag getting an entry in this note.
- Fenced content may span multiple `///`-prefixed lines. The fence delimiters themselves must each be on their own `///`-prefixed line.
- A doc comment may contain zero, one, or multiple fenced blocks. Each fenced block becomes one synthesized test.

### 2.3 What goes inside the fence

Each fenced block is a complete C+ function body. The extractor wraps it in:

```cp
#[test]
fn DOC_TEST_NAME() {
    // (the user's fenced content, with `///` prefixes stripped)
}
```

Source inside the fence has access to:
- The item the doc comment attached to (`max` in the example above), under its qualified name.
- Items from the same file as the documented item, regardless of `pub` — doctests run as if they live in the same file. See §3.1.
- Any `use`-equivalent re-imports written *inside* the fence. Doctests inherit nothing from the surrounding file's imports; if a doctest needs `math::sqrt`, the fence must contain `import "math.cplus" as math;`.

`assert EXPR;` is the most common form, but anything that compiles as a C+ statement sequence is admitted. `let` bindings, `if`/`match`, `for` loops, function calls — all legal. The test passes iff every `assert` in it succeeds (§4.5 of the attributes note).

Stream-of-consciousness rationale: the fence-content-is-a-function-body model is the cleanest match for the §2.8d declarative-only constraint. We are not "running user prose as code"; we are extracting a block of C+ source from one location (the comment) and compiling it at another (a synthesized test function). The user wrote the C+; the compiler did no synthesis.

---

## 3. Semantics

### 3.1 Scope and access

A doctest attached to item `foo` in file `src/util.cplus` is treated, for resolution purposes, as **a synthesized `#[test]` function in `src/util.cplus`**. This means:

- The item being documented (and every other item in the same file) is reachable without qualification — same-file access ignores `pub` (per §2.5 of the plan and slice 4B). A doctest can therefore call private helpers and assert on their behavior, which is one of the use cases that makes doctests valuable (you can test internals without exposing them).
- Cross-file items must be imported inside the fence, exactly like ordinary cross-file references. The doctest does **not** inherit the documenting file's `import "..." as ...` lines. Reason: the doctest's example needs to be readable and runnable in isolation; relying on hidden imports of the parent file would make every doctest a stub that fails outside its context. Explicit imports inside the fence are pure verbosity — `§2.8c verbosity-exposes-rules`.
- `pub` of the documented item is irrelevant to doctest discovery. Even a private function may have doctests; they exist to verify the function's behavior, not to demonstrate its public API.

This is a deliberate design choice. Rust doctests resolve from outside the crate and require `use crate::foo::bar`. C+ doctests resolve from inside the file. Rationale: most C+ projects will be smaller than Rust crates for a while; the "test the private helper" use case is more common than the "external user reads the doctest and learns the API" use case at this stage. Revisitable in Phase 6+ once stdlib usage patterns inform the choice.

### 3.2 Name synthesis

Each doctest gets a synthesized name of the form:

```
DOC_TEST::<item_qualified_name>::<fence_index>
```

where `<item_qualified_name>` is the documented item's resolver-qualified name (e.g. `src.util.max`) and `<fence_index>` is the 1-based index of the fence within the doc comment. Single-fence cases drop the suffix:

- One fenced block on `pub fn max` in `src/util.cplus` → `DOC_TEST::src.util.max`
- Three fenced blocks on `pub fn parse` in `src/parser.cplus` → `DOC_TEST::src.parser.parse::1`, `::2`, `::3`

The name is what shows up in `cpc test`'s output. Stable, mechanical, identifies the documenting item exactly; an agent reading a test failure can `grep` for the function and find the broken example without ambiguity.

The `DOC_TEST::` prefix is a sentinel: ordinary user `#[test]` functions cannot start with `DOC_TEST` (validated by the discovery pass; collision → **E0361** "test name `DOC_TEST::...` is reserved for synthesized doctests"). This guarantees a hand-written `#[test] fn foo()` and an extracted doctest never collide in the test name space.

### 3.3 Extraction pass placement

The doctest extractor runs as a pre-parser pass on each file, conceptually:

1. Lexer tokenizes the file (or already-tokenized via the existing pipeline; see §6 for the implementation hook).
2. For each item that has an attached doc comment containing one or more fences:
   - Strip the `///` prefix and the fence delimiters from each fenced block.
   - Emit a synthesized `#[test] fn DOC_TEST::<name>() { <stripped_content> }` function into the file's AST, after the documented item.
3. The synthesized functions go through the regular pipeline (`lower`, `sema`, `codegen`) like any other test function.

The synthesized functions live in the same compilation unit as the documented item. They are visible to test discovery (§4 of the attributes note); they are invisible to a regular `cpc build` (the attribute-validation pass skips `#[test]` functions when not building a test driver — this is the natural behavior since `#[test]` doesn't generate code in non-test builds).

### 3.4 Error attribution

Diagnostic spans are the critical correctness property here: a doctest's `assert` that fails to compile must point at the user's source line, not at the synthesized function's wrapper.

Implementation: each token inside a fence carries its original source span (file path + byte offset, same as every other token). The extractor preserves these spans into the synthesized AST; the wrapper `fn DOC_TEST_...() { }` braces get synthetic spans pointing at the fence's open/close lines, but every token inside maps back to its actual location in the doc comment.

When `cpc test` reports a doctest failure, the message reads:

```
docs/util.cplus:14:5: error[E0xxx]: ...
   |
14 |     assert max(1, 2) == 3;
   |     ^^^^^^^^^^^^^^^^^^^^^
   = test DOC_TEST::src.util.max failed at this assert
```

The line/col are the user's; the synthesized-function context appears as a footnote. The diagnostic pipeline (§5.2) already supports this — every `Diagnostic` carries an originating `Span` independently of any wrapping context.

### 3.5 Build cost

Each doctest is a separate synthesized function. A file with twenty doctests grows by twenty extra `fn` definitions in the compilation unit. For test builds this is fine; for non-test builds the `#[test]`-marked functions are skipped before codegen, so the cost is parser + sema only. The acceptable upper bound: linear in total doctest count. We do not deduplicate identical doctests across files (each is its own test).

If profiling shows the parser/sema cost is meaningful, an optimization is to defer doctest extraction until `cpc test` is invoked (skip the extraction pass on `cpc build`). Deferred for after a real workload measures it.

---

## 4. Interactions

With Phase 1–4 features:

- **`cpc fmt` (slice 4D):** the formatter must preserve `///` doc-comment text exactly, including fenced content. Slice 4D.1's preserving formatter already keeps comments verbatim; doctests inherit this. The formatter does *not* format the content inside fences — that would require parsing the doctest body, which then has to handle malformed examples. Slice 4D.2 could add an optional pass that runs the formatter on each fence's content; deferred until users ask.
- **LSP (slice 4E):** doctests are surfaced as ordinary `#[test]` functions to the goto-definition / hover machinery. Slice 4E.3's identifier-jump works inside fences (clicking `max` in a doctest jumps to the `fn max` definition). A future code-lens "Run doctest" decoration on each fence is straightforward but not in this slice.
- **`cpc build` (slice 4A):** unaffected — doctest extraction emits `#[test]` functions, which `cpc build` already skips.
- **Modules (slice 4A/4B):** explicit imports inside the fence interact correctly with the file-id qualification — the resolver runs on the synthesized AST after extraction.
- **Diagnostics (§5.2):** doctest diagnostics flow through the existing pipeline; span attribution per §3.4 above.

With Phase 5 features:

- **Attributes ([phase5-attributes.md](phase5-attributes.md)):** synthesized doctest functions carry `#[test]`. Validation passes (test-signature check, no-`pub`, no-`impl`-placement) apply uniformly. The extractor must not write `pub` on the synthesized function (E0359 would fire); it produces bare `#[test] fn DOC_TEST_NAME() { ... }`. The `DOC_TEST::` name prefix is reserved (E0361).
- **`cpc test --json`:** doctest results appear in the same JSON stream as hand-written tests:
  ```json
  {"name": "DOC_TEST::src.util.max", "result": "pass", "duration_ms": 0}
  ```
  Stable shape per §5.9 AI-recovery / §5.2 structured diagnostics commitments. Agents read this; the `DOC_TEST::` prefix lets them filter doctest failures from hand-written-test failures if they want to.
- **Borrow checker ([phase5-borrow-shared.md](phase5-borrow-shared.md), pending):** doctest bodies are borrow-checked like any other function body. No carve-out. A doctest that tries to use a moved value fails compilation with E0335, with the diagnostic spans pointing at the user's doc comment.

With deliberate non-features:

- **No macros / no comptime (§2.8 / §1.2):** doctests do not violate either. The extractor is a compiler tool; the user-written code inside a fence is ordinary C+ that gets compiled by the regular pipeline. There is no compile-time evaluation of expressions to decide what code to extract — extraction is a syntactic operation on doc-comment text.
- **No decorators / no AST transformation by user code (§2.8 / §2.8d):** the extractor is built-in compiler behavior; users cannot write a `#[my_attribute]` that transforms an item's doc comments. The line is the same one §2.8d draws for attributes: compiler-blessed declarative passes are admitted; user-extensible meta-machinery is not.

---

## 5. Open questions

1. **Fence language tags.** `cplus` is implied. Future tags could be:
   - ```` ```ignore ```` — extract for syntax-check but do not run (useful when an example needs an external file or a network call).
   - ```` ```no_run ```` — compile but do not run.
   - ```` ```compile_fail ```` — example should *fail* to compile; useful for documenting "this would be a type error."
   - ```` ```text ```` / unknown tag — treat as plain text, skip extraction.
   Each tag would get an entry in §2.2. Phase 5 ships untagged-only. Add the others when documented motivating cases land — likely `ignore` first (skip-without-deleting-the-example).

2. **Re-importing the documenting item under a different prefix.** The doctest sees the documented item under its qualified name (`src.util.max` → callable as `max` because same-file). But if a doctest demonstrates *external usage*, the natural pattern is to import the file under a prefix and call `util::max(...)`. The extractor could synthesize this `import` automatically for the documented file. Deferred; the current rule (explicit `import` inside the fence) is simpler and more honest about what the doctest is doing. Revisit if real users find it tedious.

3. **`assert` source-line attribution.** The attributes note (§4.5) flagged this same question for hand-written tests. The fix is the same: thread span data through the `assert` lowering. Lands as a follow-up slice once the basic runner is working.

4. **Empty doc comments / fence-only comments.** Doc comments with no fences are pure documentation (no action). Doc comments with empty fences (```` ``` ``` ```` containing nothing) currently extract to `fn DOC_TEST_NAME() {}` — a vacuous test that always passes. Maybe **E0362** ("empty doctest fence")? Marginal; defer until someone writes one by accident.

5. **Doctests in macro-like contexts.** N/A — C+ has no macros. The §2.8 "no macros" rule resolves an entire category of doctest-extraction edge cases that Rust has to handle (doctests in `macro_rules!` bodies). Recording this so future contributors don't go looking for the complexity.

6. **`cargo test`-style hidden-imports convention.** Rust's doctests have a hidden mode where lines starting with `#` are stripped from the displayed example but compiled into the test. Lets you write `# use std::collections::HashMap; let m = HashMap::new();` and the reader only sees the second line. Deferred. The §2.8c verbosity stance arguably says don't add it (hiding code from the reader violates locally-complete source); but the trade-off is real (small examples become bigger than they need to be). Land if real C+ doctest authors ask.

---

## 6. Implementation hook (anticipatory; not binding)

The natural place to wire the extractor is between the lexer and the parser, in a new `cplus-core/src/doctest.rs` module. The lexer already has `tokenize_with_trivia` (added in slice 4D for the formatter) that emits `LineComment` tokens; extending this to recognize `///` as a distinct `DocComment(String)` token kind is the smallest lexer change. The doctest pass walks the token stream, collects each `DocComment` and the next item-start, emits synthesized AST nodes for the doctests, and hands the augmented token stream + synthesized items to the parser.

Alternative: do the extraction at the AST level after parsing. Cleaner for span management (we have a real AST with spans rather than a token stream), but requires the parser to attach doc-comment text to its items. Either path works; pick at implementation time. Recording both so the choice isn't relitigated.

Tests for the doctest slice itself, when it lands:
- Unit: extractor recognizes single-fence and multi-fence doc comments; correctly strips `///` prefix; correctly preserves spans pointing at the original `///`-prefixed lines.
- Sema: synthesized test function with a busted assert produces a diagnostic whose primary span is the original doc-comment line.
- E2E: a sample with `/// assert foo(1) == 2;` runs cleanly under `cpc test` and shows up in `--json` output as `{"name": "DOC_TEST::...", "result": "pass"}`.
- Negative: doctest using `move`-then-use produces E0335 with span attribution into the doc comment.
- Negative: `#[test] fn DOC_TEST_foo()` (user trying to use the reserved prefix) → E0361.
