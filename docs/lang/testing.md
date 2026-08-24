# Testing — what to write, where it runs, and what it can't check

The decision this page settles: **which of the three test surfaces a fact
belongs in**, and where the boundary of what a test can assert actually
sits. Signatures in [ref.md](ref.md); the commands in
[tooling.md](tooling.md).

There is no separate test framework and no `tests/` directory. A test is a
`#[test]` function next to the code it covers, and `cpc test` compiles a
harness `main` around every one it finds.

## 1. The three surfaces

| Surface | Written as | Runs when | Use it for |
|---|---|---|---|
| unit / e2e / negative tests | `#[test] fn` in the module | `cpc test` | everything that can be asserted |
| doctests | a fence in a `///` comment | `cpc test` | the example in the docs, kept honest |
| probe apps | a program under `playground/` | by hand, by a person | what an agent has no hands for |

The house bar is **full coverage per module — unit, e2e, and negative.** A
negative test is not optional garnish: it is how you pin that a wrong
program still fails, and it is the only test that catches a check quietly
being removed.

## 2. `#[test]` functions

```cplus
#[test]
fn append_grows_the_buffer() {
    var t: text::Text = text::new();
    let s = t.append("abc");
    assert s == status::Status::Ok;
    assert t.view().count() == (3 as usize);
}

#[test]
fn parse_rejects_a_bad_digit() -> i32 {      // the other legal signature
    return match "12x".to_i64() {
        option::Option[i64]::Some(_v) => { 1 }      // parsed — that is the bug
        option::Option[i64]::None    => { 0 }
    };
}
```

- The signature is `fn()` or `fn() -> i32`. Anything else — a parameter, a
  non-`i32` return — is **E0358**.
- A `fn() -> i32` test **fails on a nonzero return**. A `fn()` test fails
  only by trapping.
- `assert cond;` is the workhorse: it traps on false, and a trap is a
  failure the harness reports by name.
- `#[test]` goes on **free functions only**. On a method it is **E0356**;
  on an `export` function it is **E0359**. To test a method, call it from a
  free test function.
- Tests are ordinary code in the module, so they see `_`-private items —
  which is the point. Testing through the public surface only is a choice
  you make, not one the language makes for you.

Names in the report are qualified by their origin:
`plat::src::main::my_test`.

The code graph knows which functions are tests: `cpc query complete` and
`cpc query scope-at` (the completion queries) omit them, `cpc query symbols`
marks them `is_test: true`,
and `callers` still reports a test as a caller of what it exercises — which
is how you find out a helper is only reachable from the suite.

## 3. Doctests

A fence inside a `///` comment becomes a test. The body is the whole test
body — there is no wrapper and no implicit `main`:

```cplus
/// Adds two numbers.
///
/// ```
/// assert add(2, 3) == 5;
/// ```
fn add(a: i32, b: i32) -> i32 { return a +% b; }
```

`cpc test` reports it as `DOC_TEST::add::0` — the item's name and the
fence's index within its comment block.

**A fence opens only on a line that is exactly three backticks.** A
` ```cplus ` fence — the tagged form used everywhere else in this repo's
prose — is *not* extracted, and the example inside it silently never runs.
This is the trap worth remembering: a doctest that does not run looks
exactly like a doctest that passes.

Other rules that follow from how extraction works:

- The synthesized function is appended to the end of the file, so a
  doctest sees the file's imports and every item in it, but nothing it
  declares itself outlives the fence.
- The item name comes from the next `fn` / `struct` / `enum` / `impl`
  header after the comment; a comment with no item after it is named
  `anon_l<line>`.
- An unterminated fence is dropped silently.
- `cpc doc FILE` emits the same comments as Markdown to
  `target/doc/<basename>.md`, so one fence is documentation and test at
  once.

## 4. Where `cpc test` starts

Single-file mode when you name a file:

```bash
cpc test src/thing.cplus       # no manifest read; the file must have no imports
```

Project mode otherwise. The entry is resolved by a ladder, first match
wins:

1. `src/test_main.cplus` — a dedicated test root that imports the surface
   the suite should cover. No manifest key; the file's existence is the
   declaration. A package with one means it, even when the package is also
   an app.
2. the app entry for the current platform
3. the `[library]` target
4. `src/<package-name>.cplus` — the root module, for a plain library

From there, discovery walks the **resolved import tree** — which includes
your dependencies. Importing `stdlib/io` puts `stdlib`'s own `io` tests in
your run:

```
test plat::src::main::my_test ... ok
test stdlib::src::io::io_write_paths_do_not_trap ... ok
```

That is deliberate — the tests that ran cover the code actually in your
binary — but it means a red run may be pointing at a dependency, not at
you. Read the qualified name before you go looking in your own `src/`.

A package is testable from its own directory: `cd vendor/facet && cpc
test`. That is the unit of a suite in this repo.

## 5. Running

```bash
cpc test                     # this platform, debug
cpc test --release           # the same suite at -O3, wrapping arithmetic
cpc test --asan              # AddressSanitizer; --ubsan --tsan --msan too
cpc test --json              # one JSON object per test, then a summary
```

Exit status is **0** when everything passed and **2** when anything failed
— usable directly in CI.

`--json` emits NDJSON:

```json
{"name":"add_works","result":"pass"}
{"name":"DOC_TEST::add::0","result":"pass"}
{"passed":2,"failed":0}
```

**Run the suite in both modes.** Debug traps on overflow and release wraps;
`--release` also turns on optimizations that have caught real miscompiles
here. A green debug suite is half the evidence.

The sanitizers instrument cpc-emitted code exactly as clang instruments C,
and they are the tool for the raw tier — a `*T` field, an `extern fn`, an
`opaque` pointer. `--asan`, `--tsan`, and `--msan` are mutually exclusive;
`--ubsan` composes with any of them.

## 6. What a test cannot assert

UI *feel* — drag, wheel, momentum, focus follow — has no assertion. An
agent has no hands, and a headless run of an event loop is not the event
loop that ships. The discipline that replaces it:

1. **Pin the state machine in a test.** Whatever the gesture drives is a
   sequence of states, and that sequence is assertable without a mouse.
2. **Build a probe app under `playground/`** (git-ignored) that exercises
   the real path on the real backend.
3. **Ask a person to try it.** That is the verification step; there is no
   substitute and no point pretending otherwise.

Never add drag / pinch / swipe verbs to an agent surface to close this gap.
The gap is real and the verbs would lie about it.

## 7. Real-time contracts are checked, not tested

`#[no_alloc]`, `#[no_block]`, `#[bounded_recursion]`, `#[max_stack(N)]`,
and the `#[realtime]` bundle are compile-time call-graph checks, not
runtime assertions. Project-wide, `[profile.realtime]` in `Cplus.toml`
synthesizes them onto every function in *this* package (dependencies are
exempt):

```toml
[profile.realtime]
deny-alloc          = true
deny-block          = true
deny-unknown-extern = true
stack-limit         = 4096
```

`cpc --realtime-report` (or `=json`) prints the whole-project digest: the
profile, how many functions are under contract, and every
E0901/E0906/E0907/E0908 violation grouped by contract. It exits non-zero on
any — the CI shape for a real-time gate.

The keys are kebab-case and an unknown key is a hard parse error. That
matters more than it sounds: the snake_case spelling used to be silently
dropped, which meant a gate the author believed was on was off.

## 8. Gotchas

- **A ` ```cplus ` fence in a `///` comment is not a doctest.** Three bare
  backticks, or it never runs.
- **`cpc test FILE` cannot see imports.** Any file with an `import` needs
  project mode (E0852).
- **`examples/` is outside every suite.** Building the examples after an
  interface change is a manual step; nothing runs them for you.
- **A dependency's failing test fails your run.** Check the qualified name
  before assuming the bug is local.
- **A `#[test]` on a method is E0356, not a warning.** Call the method from
  a free function instead.
- **`assert` is the only hard stop in the language.** It is for "the
  program itself is wrong", in tests and in contract violations — never for
  input validation, which returns a `Status`, `Option`, or `Result`
  ([error-handling.md](error-handling.md)).
