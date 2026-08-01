# Bug 05 — Match arm `=> { x }` bypasses the E0337 borrowed-payload escape check (double-free)

- Status: reproduced 2026-08-01 with `target/release/cpc` (bare `=> x` rejected, `=> { x }` accepted)
- Severity: soundness (double-free)
- Area: sema (`cplus-core/src/sema.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B5)

Context for the fixer: compiler in `cplus-core/src/`; build `cargo build --release`; binary
`target/release/cpc`; tests `cargo test -p cplus-core` and `cargo test -p cpc --test e2e`.
E0337 rejects moving a Drop-carrying payload out of a match on a borrowed scrutinee (the
owner still drops the field; a bit-copy out means two drops). Line numbers from
2026-08-01; locate by symbol name if drifted.

## Reproduction

`m5.cplus`:

```cplus
struct R { n: i64 }

impl R {
    fn drop(ref this) { }
}

enum Holder { Some(R), None }

struct Bag { h: Holder }

fn peek(b: Bag) -> R {
    let t: R = match b.h {
        Holder::Some(x) => x,
        Holder::None => R { n: 0 },
    };
    return t;
}

fn peek2(b: Bag) -> R {
    let t: R = match b.h {
        Holder::Some(x) => { x },
        Holder::None => R { n: 0 },
    };
    return t;
}

fn main() -> i32 { return 0; }
```

```
$ target/release/cpc check m5.cplus
```

Observed: exactly one E0337, on `peek` (`=> x`). `peek2` (`=> { x }`) passes with no
diagnostic, then codegen bit-copies the Drop payload out of `b.h` — the caller's `Bag`
still drops the field, so the payload drops twice.

Expected: both functions rejected identically.

## Root cause

The escape check at sema.rs:9727-9760 recognizes only a bare identifier arm body:

```rust
let returned = match &arm.body.kind { ExprKind::Ident(n) => Some(n.clone()), _ => None };
```

The comment claims every other escape route hits a consuming site; a value-transparent
block tail (`{ x }`) hits none. Any other transparent wrapper the language grows would
also slip through.

## Fix

1. Replace the bare-Ident sniff with the leaf collector that already exists in the same
   file for exactly this purpose: `collect_value_leaves` (sema.rs:18263, used by
   `mark_moved_through_wrappers`). Run it over the arm body and treat any leaf that is a
   payload binding as the escape.
2. Keep the diagnostic text and span behavior of the current E0337 for the bare case.

## Verification

1. `m5.cplus`: both `peek` and `peek2` now fail with E0337; deleting the `impl R { fn drop }`
   block makes both compile (no Drop, no hazard).
2. Add negative e2e tests for `=> { x }` and a nested block `=> { { x } }`.
3. If a runtime double-free proof is wanted before fixing: give `R.drop` a counter and
   drive `peek2` from `main`; ASan (`cpc test --asan` equivalent or direct clang -fsanitize)
   flags the double drop.
4. Full suites; grep e2e for existing E0337 tests to confirm they still pass.

## Notes

- Must not regress the accepted cases: arms that construct a fresh value
  (`Holder::None => R { n: 0 }`) stay legal.
- The audit classed this with the drift family "checks keyed on one syntactic shape";
  `collect_value_leaves` is the in-file cure for that family.
