# Bug 08 — `Iterator__` substring hijack of user types (ICE or silent miscompile)

- Status: FIXED 2026-08-01, commit cb93037 — identity via `#[lang("iterator")]` /
  `#[lang("future")]`, loud-miss back-ported
- Status (original): reproduced 2026-08-01 with `target/release/cpc` (panic at codegen.rs:16536)
- Severity: ICE; silent miscompile when `Option[T]` happens to be instantiated
- Area: codegen (`cplus-core/src/codegen.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B8)

Context for the fixer: the compiler synthesizes `Iterator[T]` / `Future[T]` instantiations
for `gen`/`async` functions and lowers their blessed methods (`next()`, awaiting) as
coroutine intrinsics. Codegen identifies those synthesized types by SUBSTRING-matching the
mangled name. Generic mangling is `Base__Arg`, so any user generic whose base name ends in
`Iterator` collides. Build `cargo build --release`; binary `target/release/cpc`; tests
`cargo test -p cplus-core`, `cargo test -p cpc --test e2e`. Line numbers from 2026-08-01.

## Reproduction

`main.cplus`:

```cplus
struct Token { v: i32 }

struct LineIterator[T] { cur: T }

impl LineIterator[T] {
    fn next(this) -> T {
        return this.cur;
    }
}

fn main() -> i32 {
    let it: LineIterator[Token] = LineIterator[Token] { cur: Token { v: 7 } };
    let t: Token = it.next();
    return t.v;
}
```

```
$ target/release/cpc check main.cplus
thread 'main' panicked at cplus-core/src/codegen.rs:16536:
no Option instantiation found for iterator element Struct(StructId(0))
```

Expected: compiles; `main` returns 7. If the program also instantiates `Option[Token]`,
the panic is avoided and codegen instead emits `llvm.coro.done` against a non-coroutine
struct — a silent miscompile. Any user generic base name ending in `Iterator`
(`TokenIterator`, `RowIterator`, …) is affected; the `Future__` twin has the same disease.

## Root cause

- `unwrap_iterator_ty` (codegen.rs:16351): `name.rfind("Iterator__")` classifies ANY
  mangled name containing the substring as a synthesized coroutine iterator.
- The blessed `next()` dispatch arm (codegen.rs:14922-14926) fires BEFORE user-method
  dispatch, so the user's own `next` never resolves.
- Same pattern: `ty_from_future_name` for `Future__`.
- Adjacent hazard to fix in passing: `lookup_future_ty` / `lookup_iterator_ty`
  (codegen.rs:893-896, 910-913) fall back to `Ty::Struct(StructId(0))` on a miss —
  silently whichever struct was collected first. Their sibling `lookup_option_ty` was
  hardened to a loud panic (codegen.rs:16532-16536, "Fail loud rather than silently
  returning EnumId(0)") and the hardening was never back-ported.

## Fix

1. At type-collection time, mark compiler-synthesized `Iterator`/`Future` instantiations
   with a structural flag on `StructInfo` (the exact pattern already exists in-file:
   `is_lang_string`, codegen.rs:16857). The information is available where the synthesized
   instantiations are created (mono/sema hand them over by name — carry an origin marker in
   the collected type table).
2. Dispatch the blessed `next()`/await lowering on the flag, not on substring matches.
   Delete the `rfind("Iterator__")` / `Future__` classification.
3. Back-port the loud-miss panic to `lookup_future_ty` and `lookup_iterator_ty`.

Companion: `issue-06-lang-item-registry.md` (the same identity-by-structure move for
stdlib Option/Iterator/Future on the sema side).

## Note on what the marker does and does not replace

The marker replaces the CLASSIFICATION only. Recovering the element type U from the
mangled suffix stays, because mono records the type argument only in the name — and by
the time the suffix is parsed the marker has already established that the name IS
`Iterator__<U>`, so the parse can no longer be handed a foreign shape. Where it used to
`return None` on an unparseable name it now panics: reaching that arm would mean a
`#[lang("iterator")]` struct that mono did not mangle, which is a compiler bug, not a
program error.

`#[lang]` needed no attrs-pass change — its spec is already `ExactlyOneStr` on structs,
and sema reacts only to the `"string"` value.

## Verification

1. DONE: the repro compiles and returns 7.
2. DONE: full e2e suite green, including the existing gen/async tests.
3. DONE: `user_generic_named_iterator_is_not_a_coroutine` in cpc/tests/e2e.rs — a user
   `LineIterator[T]` with its own `next()` AND a real `gen fn` in one program, asserting
   both dispatch correctly.
4. DONE: full suites green; `cd vendor/stdlib && cpc test` — 290 passed (the stdlib
   source is what carries the two new markers).
