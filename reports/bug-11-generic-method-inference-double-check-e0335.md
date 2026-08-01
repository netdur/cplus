# Bug 11 — Generic-method inference type-checks each argument twice: false E0335

- Status: FIXED 2026-08-01 (commit pending) — fix option 1 (snapshot/restore the
  probe pass), landed with bug-01
- Status (original): reproduced 2026-08-01 with `target/release/cpc check`
- Severity: false error (rejects valid programs)
- Area: sema (`cplus-core/src/sema.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B11)

Context for the fixer: `check_expr` in sema is SIDE-EFFECTING — checking a consuming call
marks its argument moved. Checking the same expression twice double-marks the move and
fires E0335 ("use of moved value") on legal code. Build `cargo build --release`; binary
`target/release/cpc`; tests `cargo test -p cplus-core`, `cargo test -p cpc --test e2e`.
Line numbers from 2026-08-01.

## Reproduction

`g1.cplus`:

```cplus
struct R { n: i64 }
impl R { fn drop(ref this) { } }
fn eat(take r: R) -> i32 { return 1; }

struct S { m: i32 }
impl S { fn g[T](this, v: T) -> T { return v; } }

fn main() -> i32 {
    let s = S { m: 1 };
    let r = R { n: 2 };
    let x = s.g(eat(r));
    let _ = x;
    return 0;
}
```

```
$ target/release/cpc check g1.cplus
error[E0335]: use of moved value 'r'
```

Expected: compiles — `r` is consumed exactly once by `eat`. Controls: a concrete method
with the same body compiles; the turbofish spelling `s.g::[i32](eat(r))` compiles.

## Root cause

`check_generic_method_call`'s inference branch checks every argument twice:

1. sema.rs:12563-12575 — `check_expr(arg, None)` to produce a type for unification;
2. sema.rs:12597-12599 — `check_expr(a, Some(expected_ty))` to enforce the substituted
   parameter type.

Pass 1 already marks `r` moved through the nested `eat(r)`; pass 2 sees the move and
errors. The codebase knows this hazard: the `fnptr_field_names` pre-filter exists
precisely because a probing double-check "double-marking moves (false E0335)"
(sema.rs:1305-1311). The same double-eval was reintroduced here.

## Fix

Choose one (first is smallest):

1. Snapshot the move/init state before the inference pass and restore it after, so pass 1
   is observationally side-effect-free; or
2. infer from a non-side-effecting type probe (`place_ty_quiet`-style) where the argument
   shape allows, falling back to checked evaluation once; or
3. check each argument exactly once: run `check_expr(arg, None)`, unify from the produced
   type, then validate the substituted expectation against that type WITHOUT re-walking
   the expression (this is the shape the eventual single-arg-path refactor wants — see
   `issue-05-sema-call-gate-unification.md`).

Whichever is chosen, the invariant to enforce: each argument expression is move-checked
exactly once per call.

## Correction to the repro

`g1.cplus` as written ALSO fails with an unrelated, pre-existing E0337 inside the
method body: `fn g[T](this, v: T) -> T { return v; }` returns a bare by-value
(borrowed) parameter. Verified against the pre-fix binary — both errors were
present. The `take v: T` spelling isolates the double-check bug and is the shape
used in the regression test.

## Verification

1. `g1.cplus` compiles and returns 0.
2. Real double-use still rejected: `s.g(eat(r)); eat(r);` → E0335 (add as negative e2e).
3. Inference still works: the repro without turbofish resolves `T = i32`.
4. Full suites; grep e2e for E0335 tests.
