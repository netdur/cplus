# Phase 5 — Attributes (`#[...]`)

> Status: design note. Implementation pending; lands after the borrow checker per the Phase 5 sequencing block in [plan.md](../../plan.md) §3.
> Locks in: the `#[test]` vs `test fn` open question (resolved at Phase-5 kickoff in favor of `#[test]`); plan.md §2.8d (attributes are declarative-only).
> First instantiation: `#[test]`. Other attributes (`#[inline]`, `#[repr(C)]`, `#[deprecated]`) get their own design notes when they land.

---

## 1. Problem

The §11 carry-forward asked: how does C+ mark a function as a test? The two candidates were a marker keyword (`test fn foo() { ... }`) and an attribute (`#[test] fn foo() { ... }`). The same question recurs for every future "this item carries metadata" case: optimizer hints (`#[inline]`), ABI markers (`#[repr(C)]`), diagnostic flags (`#[deprecated("...")]`), FFI calling-convention overrides. The marker-keyword path needs a new keyword every time and burns identifier space; the attribute path is one piece of syntax that scales.

The risk with attributes is that they invite the very thing §2.8 rejects: macros, decorators, compile-time AST transformation. Rust's `#[derive(...)]` synthesizes whole method bodies; Python's `@decorator` rewrites the function it decorates. That's a wide door for "magic" — exactly the surface complexity LLM-readability optimizes against.

This note resolves both questions:
1. C+ adopts `#[...]` as a *general* extension point.
2. The hard rule from plan.md §2.8d: **attributes carry no expression-level compile-time computation.** They are declarative metadata read by the compiler or by tools. They do not generate code, transform the AST, or run user-written logic at compile time.

`#[test]` is the first concrete attribute and exercises every part of the machinery (parser, AST, sema lookup, downstream tool consumption).

---

## 2. Syntax

Attribute syntax mirrors Rust / Swift / C# at the surface — outer attributes appearing on a line of their own immediately before the item they decorate.

```cp
#[test]
fn add_one_plus_two_is_three() {
    assert add(1, 2) == 3;
}

#[inline]
fn hot_inner_loop(n: i32) -> i32 {
    return n + 1;
}

#[repr(C)]
struct Point { x: i32, y: i32 }

#[deprecated("use parse_v2 instead")]
fn parse_v1(s: string) -> ParseResult {
    return parse_v2(s);
}

#[test]
#[ignore]
fn slow_test_disabled_for_now() {
    expensive_thing();
}
```

Form:

- `#[NAME]` — bare attribute, no arguments. (`#[test]`, `#[inline]`, `#[ignore]`.)
- `#[NAME(ARGS)]` — attribute with parenthesized arguments. Arguments are a comma-separated list of:
  - bare identifiers (`#[repr(C)]`),
  - string literals (`#[deprecated("...")]`),
  - key=value pairs where the value is a string literal or bare identifier (`#[link(name = "z", kind = "static")]` — speculative; not landing in Phase 5).
- Multiple attributes on the same item are written one per line, in any order. Order is not semantically significant in Phase 5; if any future attribute proves order-sensitive, the rule it introduces is recorded with that attribute.

Attributes attach to **items only** in Phase 5: `fn`, `struct`, `enum`, methods inside `impl`, struct fields, and enum variants are the legal targets. Statement-level attributes (Rust's `#[allow(...)]` on a `let`), expression-level attributes, and outer-module attributes (Rust's `#![...]`) are not in Phase 5. They're not principle-rejected — they just need a motivating use case.

Phase 5 ships exactly one attribute: `#[test]`. The parser admits the *general* syntax for unknown attributes (see §3.4), but each named attribute is a separate language extension that gets its own design note.

---

## 3. Semantics

### 3.1 Parsing

`#` becomes a token. `#[` opens an attribute; the matching `]` closes it. The body parses as an attribute path + optional arg list. Lexer changes are minimal — `#` is a single-char punctuation token; the `[` / `]` reuse existing tokens.

Attributes are collected by the parser into a `Vec<Attribute>` on each item AST node. New AST shape:

```rust
struct Attribute {
    span: Span,
    path: String,           // "test", "inline", "repr", etc.
    args: Vec<AttrArg>,
}

enum AttrArg {
    Ident(String),
    Str(String),
    KeyValue(String, AttrValue),  // for forms like `name = "..."` — not used by any Phase 5 attribute
}

enum AttrValue {
    Ident(String),
    Str(String),
}
```

Every item-bearing AST node (`Function`, `StructDecl`, `EnumDecl`, `Method`, `StructField`, `EnumVariant`) gains an `attributes: Vec<Attribute>` field. Items without attributes carry an empty vec — no Option wrapper, keeps consumer code simple.

Attribute syntax does not appear in expression position, statement position, or type position. The parser only looks for `#[` at item-start positions.

### 3.2 Validation

After parsing, before sema, an `attribute_check` pass walks every collected attribute and validates:

- The attribute name is in the *known-attributes* set. Unknown attributes → **E0354** ("unknown attribute `#[foo]`"). Phase 5's known set is `{test}`. (Strict rejection rather than warn-and-ignore — see §3.4 for rationale.)
- The attribute's argument shape matches its specification. E.g. `#[test]` takes no arguments; `#[test(slow)]` → **E0355** ("attribute `#[test]` takes no arguments").
- The attribute is on a legal target. `#[test]` on a struct → **E0356** ("attribute `#[test]` may only appear on functions").
- The attribute is not duplicated where uniqueness is required. `#[test] #[test] fn foo() {}` → **E0357** ("duplicate attribute `#[test]`"). Some future attributes will permit repetition (`#[link(...)]` can stack); the spec for each attribute states which.

E0354–E0357 numbering reserves four codes; if validation grows orthogonal rules we extend the range.

### 3.3 Consumption

Each attribute belongs to exactly one consumer (compiler stage or external tool):

- `#[test]` is consumed by `cpc test`. Test discovery is the new pass that walks every parsed function in the project, filters to those carrying `#[test]`, validates the test-function signature (`fn() -> i32` or `fn()` — see §4.2), and emits a generated test-driver binary.
- Future `#[inline]` will be consumed by codegen (read the flag → set the LLVM `inlinehint` attribute on the emitted `define`).
- Future `#[repr(C)]` will be consumed by struct-layout codegen.
- Future `#[deprecated]` will be consumed by sema (warn on every use site).

**Attributes are never consumed by user code at compile time.** No "if `#[test]` is present, generate ..." logic written in C+. The discriminating test (§2.8d): does the attribute's effect *write a function body* or *rewrite the surrounding declaration*? `#[test]` does neither — `cpc test` reads a flag and decides whether to invoke the function from the generated driver.

### 3.4 Unknown attributes: strict rejection, not warn-and-ignore

Rust's choice was warn-and-ignore for unknown attributes (originally for forward-compatibility with custom derive crates). C+ rejects unknown attributes outright (E0354). Reasons:

1. **No custom attributes ever.** Rust's tolerance exists because procedural macros let third-party crates introduce new attributes. C+ has no macros and never will (§2.8). Every attribute that exists is in the compiler's known-set.
2. **Typos become silent bugs under warn-and-ignore.** `#[tset]` instead of `#[test]` would silently skip the function. With strict rejection the compiler points at the misspelling. did-you-mean suggestion fires for distance ≤ 2 (same machinery as E0401 module imports).
3. **The §5.9 AI-recovery story prefers structured errors.** A precise "unknown attribute" diagnostic with a suggestion is easier for an agent to repair than a buried warning.

The cost is that every new attribute is a compiler change — but every attribute is a deliberate language extension anyway (each needs its own design note per the closing rule of §2.8d), so the change is happening regardless.

### 3.5 Lowering

Attributes survive parsing and validation as data on the AST. They do not get lowered to anything; they are passed through to whichever consumer reads them. The `lower` pass (which currently rewrites `if let` / `guard let` / `while let` to `match`-using forms) does not touch attributes.

---

## 4. The `#[test]` instantiation

This is the concrete first attribute. Detailed because it exercises every layer.

### 4.1 Syntax

```cp
#[test]
fn arithmetic_is_associative() {
    assert (1 + 2) + 3 == 1 + (2 + 3);
}
```

### 4.2 Test function signature

Two accepted signatures, in priority order:

1. **`fn NAME()`** — no return type. Test passes iff every `assert` in the body passes (no `assert` failure ran `llvm.trap`). Most common form.
2. **`fn NAME() -> i32`** — explicit exit-code return. Test passes iff the function returns `0`. Returning non-zero is a fail. Useful when the test wants explicit control.

Tests cannot take parameters in Phase 5. (Parameterized tests / property-tests are a Phase 5+ extension that gets its own attribute, e.g. `#[test_each(0, 1, 2, 5)]` — design later.)

Sema validates the signature when an `#[test]` is present. Wrong signature → **E0358** ("test function must have signature `fn() -> i32` or `fn()`"). Test functions cannot have explicit visibility — `pub` on a `#[test]` fn is **E0359** ("test functions cannot be `pub`"). Tests cannot live in `impl` blocks — **E0360** ("`#[test]` may not appear inside `impl`"). The constraint is that test functions are project-internal helpers discovered by the runner, not part of the project's exported API surface.

### 4.3 Discovery

A new `test_discovery` pass runs after sema completes (with all attributes already validated). It walks the merged `Program` produced by `resolver::load_project` (or the single-file program in single-file mode) and collects every function whose attribute list contains `#[test]`. The collected set is `Vec<TestFnQualifiedName>`.

Test functions live alongside ordinary functions in the source — the same file can have both. They are not segregated to a `tests/` directory in Phase 5 (the manifest's `[[bin]]` doesn't grow a `[[test]]` companion yet). Phase 5+ can add a directory convention if real projects ask for it.

### 4.4 Driver generation

`cpc test` works by generating a synthesized "test driver" binary that links the project plus a generated `main` function. The driver runs each `#[test]` function in turn, captures its result, prints structured output (and a `--json` form), and exits with the count of failures.

Pseudocode for the generated driver `main`:

```cp
fn main() -> i32 {
    let mut passed: i32 = 0;
    let mut failed: i32 = 0;

    // for each discovered test, in source order:
    let ok_1 = run_test("project::arithmetic_is_associative", project::arithmetic_is_associative);
    if ok_1 { passed = passed + 1; } else { failed = failed + 1; }
    // ... one block per test

    println(passed);
    println(failed);
    return failed;
}
```

`run_test` is a runtime support function (small, codegen-emitted, not user-visible). For `fn() -> i32` tests it calls the test and returns `result == 0`. For `fn()` tests, the implementation depends on how we report assertion failures — see §4.5.

`cpc test --json` emits a JSON object per test on stdout:

```json
{"name": "project::arithmetic_is_associative", "result": "pass", "duration_ms": 0}
{"name": "project::other_test", "result": "fail", "duration_ms": 1, "reason": "..."}
```

Stable shape; part of the §5.2 structured-diagnostics commitment extended to test output. AI agents iterate on test feedback by reading this JSON, not by parsing human-readable runner output.

### 4.5 `assert` and failure reporting

Phase 5 ships the minimum: `assert EXPR;` is a new statement that lowers (in the `lower` pass) to:

```cp
if !(EXPR) {
    // halt the test with failure
    cpc_runtime_assert_failed();
}
```

`cpc_runtime_assert_failed` is a compiler intrinsic. For non-test builds it lowers to `llvm.trap` (the assertion-failed path traps — same as overflow / div-by-zero). For test builds it sets a thread-local "current test failed" flag and longjmps back to `run_test`'s call site — or simpler: it sets the flag and `return`s through normal control flow, with `run_test` reading the flag afterward.

**Simplest Phase 5 implementation:** `assert` in a `fn()` test sets a per-test failure flag (a global `i32` since C+ has no thread-locals yet) and then `return`s from the current function. `run_test` checks the flag after the call. No longjmp, no unwinding, no FFI implication. This is the path consistent with §2.8b (no machinery the ABI doesn't carry).

The cost: an `assert` failure doesn't show *which* assertion failed in Phase 5's structured output — only that the test failed. Source-line attribution lands in Phase 5+ once we wire span data through the assert lowering. Acceptable for the first cut; the test name still says which test failed.

Outside test functions, `assert` lowers to a trap (the assertion-failed path is a bug, not a recoverable error). This is consistent with `assert`-in-doctests (§5.6 doctests, future slice) and with the §2.4 errors-are-values story — `assert` is for invariants, not for fallible operations.

### 4.6 `#[ignore]` (deferred)

The example in §2 used `#[ignore]` as a sibling attribute. Phase 5 ships `#[test]` only; `#[ignore]` is straightforward to add once the validation framework is in place, but it's a separate small slice. The mention in §2 was to show that multi-attribute stacking parses, not that `#[ignore]` is part of this slice.

---

## 5. Interactions

With every Phase 4 feature:

- **Modules (§2.5):** test functions belong to the file they're declared in; the resolver's file-id-qualification produces test names like `src.math_tests.foo_is_even` (or `project::foo_is_even` if we choose a `::` separator for test display). Cross-file calls from tests work via ordinary `pub` access — a private helper in `src/math.cplus` is not callable from a `#[test]` in `src/util.cplus`. Test functions themselves cannot be `pub` (E0359 — see §4.2).
- **`pub` visibility (slice 4B):** see above. Tests are project-internal regardless of where they live.
- **`if let` / `guard let` (slice 4A.5):** lowered before attribute consumption looks at function bodies. No interaction.
- **Loops + match (Phase 3/4):** unchanged. `#[test]` doesn't care what control flow the test body uses.
- **Formatter (slice 4D):** the formatter must preserve attribute placement (attribute line directly above the item, no blank line between). Slice 4D.1's preserving formatter handles this by treating attributes as ordinary tokens with their own newline-positioning rule. The lexer already emits `#` and `[` / `]`; the formatter prints `#[NAME]` tight, then a newline, then the item.
- **LSP (slice 4E):** new attribute tokens get a syntax-highlighting class in the TextMate grammar (already partly in place — `editors/vscode/syntaxes/cplus.tmLanguage.json` has an `attribute` pattern). Goto-definition on an attribute name is not in scope. Diagnostics for E0354–E0360 flow through the same pipeline as every other sema diagnostic.

With every Phase 5 feature (forward-looking):

- **Borrow checker:** test function bodies are borrow-checked like any other function body. No carve-out. A test that tries to use a moved value is rejected the same way production code would be.
- **Doctests (slice 5C):** doctest-extracted snippets become synthesized `#[test]` functions internally. The `assert` inside the doctest uses the same machinery. The §2.8d declarative-only constraint applies — the doctest extractor reads `///` comments and writes new top-level test functions; the attribute itself doesn't do the rewriting (a separate compiler pass does, before attribute discovery sees them).

With future attributes:

- `#[inline]` / `#[repr(C)]` / `#[deprecated]` get their own design notes. Each must justify against §2.8d: does the attribute write a function body or rewrite the declaration? If yes, the answer is no.
- The first proposed transforming attribute (anything that synthesizes code) is rejected by pointing at §2.8d. Structural auto-derive (already used for `Copy` per §2.9) is the alternative when Phase 7's interfaces land.

---

## 6. Open questions

1. **Attribute name separator for display.** Should the test runner display `project::foo` or `project.foo` or `foo` for a test named `foo` in `src/main.cplus`? Resolver's internal qualification uses `.`; sema treats imports as `prefix::Item`. Pick one consistent for human and JSON output. Phase 5 implementation will pick at the time of `cpc test` slice. Recommended: human output uses `::` (matches the source-level syntax users see); JSON output uses the resolver-qualified `.` form (stable, mechanical).
2. **Argument list grammar for the general case.** Phase 5 ships `#[test]` (no args), so the parser only needs the no-arg form. The argument-list grammar in §2 is sketched but not implemented. When the first attribute with arguments lands (`#[deprecated("...")]` is the likely first), the grammar gets nailed down in that attribute's design note. Punting until there's a real shape to fit.
3. **`assert` source-line attribution.** Phase 5 reports "test X failed"; it does not report which `assert` line. Wiring span data through the `assert` lowering is straightforward but a small project (changes the runtime support's signature and the JSON shape). Land in a Phase 5 follow-up slice once the basic runner is working.
4. **`#[ignore]`.** Ship later; trivially additive once the validation framework exists.
5. **Parameterized / property tests.** Not in Phase 5. The shape that fits §2.8d is a separate attribute (`#[test_each(VALUES)]` or similar) that the runner expands at discovery time. Picking the syntax can wait until someone hits a case where one parameterized test would be cleaner than ten copy-pasted ones.
6. **Attribute placement on `impl` block as a whole.** Phase 5 attaches attributes to items inside `impl`, not to the `impl` block itself. If a future attribute logically applies to "every method in this impl" (`#[inline]` could be such a case), the design note for that attribute decides whether `impl`-block-level placement is admitted.
