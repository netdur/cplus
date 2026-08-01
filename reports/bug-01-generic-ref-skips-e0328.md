# Bug 01 — Generic call paths skip the `ref`-argument writability rule (E0328)

- Status: FIXED 2026-08-01, commit c071b39 — all three generic spellings now emit E0328
- Status (original): reproduced 2026-08-01 with `target/release/cpc` (program compiles and exits 99; concrete control correctly errors)
- Severity: soundness (immutable binding mutated at runtime)
- Area: sema (`cplus-core/src/sema.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B1)

Context for the fixer: the compiler lives in `cplus-core/src/`. Build with `cargo build --release`;
always use the binary `target/release/cpc`. Tests: `cargo test -p cplus-core` and
`cargo test -p cpc --test e2e`. In C+, `let` is a frozen local and `var` is mutable; a
`ref x: T` parameter writes back to the caller's place, so the argument must be a writable
place — that is rule E0328. Line numbers below are from 2026-08-01; if they have drifted,
locate by the quoted symbol names.

## Summary

E0328 ("a `ref` parameter writes back to the caller, so the argument must be a mutable
(`var`) place") is enforced only inside `check_arg_with_move`, which the concrete call path
uses. The generic call paths (`check_generic_named_call`, both its inference and turbofish
branches, and `check_generic_method_call`) hand-roll their own argument loops and never run
the rule. A generic `ref` parameter therefore accepts a frozen `let` and mutates it at
runtime.

## Reproduction

`refhole.cplus`:

```cplus
fn bump_g[T: Copy](ref x: T, v: T) { x = v; }
fn main() -> i32 {
    let y = 5;
    bump_g(y, 99);
    return y;
}
```

```
$ target/release/cpc refhole.cplus -o refhole && ./refhole; echo $?
99
```

Expected: compile error E0328 (the argument `y` is a `let`). Observed: compiles, runs,
returns 99 — the frozen binding was mutated. The turbofish spelling
`bump_g::[i32](y, 99)` behaves identically.

Concrete control (`frozen.cplus`) — this behavior must NOT regress:

```cplus
fn bump(ref x: i32) { x = 99; }
fn main() -> i32 {
    let y = 5;
    bump(y);
    return y;
}
```

```
error[E0328]: a `ref` parameter writes back to the caller, so the argument must be a mutable (`var`) place
```

## Root cause

The only enforcement site is inside `check_arg_with_move` (sema.rs:14710-14722):

```rust
if expected.mutable && !expected.move_ && !expected.borrow_
    && !self.is_writable_place_quiet(arg) { self.err("E0328", ...) }
```

The generic paths never reach it:

- `check_generic_named_call`, both branches (sema.rs:11663-11694 inference,
  11764-11770 turbofish): run `check_expr` + a consume step only.
- `check_generic_method_call` (sema.rs:12596-12620): `check_expr` + `consume_value_arg` only.

The file records that this drift class shipped holes before: the generic path went out
without E0327/E0328/E0308/move-consume (comment near 11899-11902) and the inference branch
without take-consumption (comment near 11793-11800, on `consume_generic_take_arg`, which is
itself a near-clone of `consume_value_arg` at 14740).

## Fix

1. In each generic path, after the parameter type is substituted (the concrete `Ty` for the
   type parameter is known at that point), build the same `ParamSig`-shaped view of the
   parameter the concrete path uses (`ty`, `move_`, `mutable`, `borrow_`).
2. Route each argument through `check_arg_with_move` with that substituted signature,
   replacing the hand-rolled `check_expr` + consume sequences in:
   - `check_generic_named_call` inference branch,
   - `check_generic_named_call` turbofish branch,
   - `check_generic_method_call`.
3. Delete `consume_generic_take_arg` once nothing calls it (its logic is inside
   `check_arg_with_move`'s consume step).
4. While here, remove the duplicated type-mismatch re-check in the turbofish branch
   (sema.rs:11665-11678) and its twin at 12599-12612: `check_expr(arg, Some(expected))`
   already emits the central E0302, so the second comparison produces a duplicate
   diagnostic that also leaks the resolver-qualified function name (diagnostics must never
   show internal qualified names).

The structural companion is `issue-05-sema-call-gate-unification.md`; this report's steps
are its phase 2 and can land alone.

## Verification

1. `refhole.cplus` now fails with E0328 in both inference and turbofish spellings.
2. `frozen.cplus` still fails with E0328; a `var y` version of both compiles and returns 99.
3. Add negative e2e tests (cpc/tests/e2e.rs — grep for an existing `E0328` test to copy the
   harness pattern): generic-fn inference, generic-fn turbofish, generic-method.
4. `cargo test -p cplus-core && cargo test -p cpc --test e2e` — expect a few existing tests
   to change if they asserted the duplicated diagnostic; update those, do not weaken E0328
   coverage.

## Notes

- Related reproduced bug in the same region: bug-11 (inference branch double-checks
  arguments and double-marks moves). Fixing both together is natural: route through one
  path, check each argument exactly once.
