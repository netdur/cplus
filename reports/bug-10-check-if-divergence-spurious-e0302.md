# Bug 10 — `check_if` uses a weak private divergence predicate: spurious E0302

- Status: FIXED 2026-08-02, commit 5d0fcaa — private predicates deleted; `check_if` uses
  `crate::lower`
- Status (original): reproduced 2026-08-01 with `target/release/cpc check`
- Severity: false error (rejects valid programs); latent false E0335 via move-joins
- Area: sema (`cplus-core/src/sema.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B10)

Context for the fixer: "does this expression/block diverge (return/break/continue on every
path)" is asked when typing `if`/`match` used as expressions: a diverging arm imposes no
type constraint. The compiler has THREE implementations of this predicate; `check_if` uses
the weakest. Build `cargo build --release`; binary `target/release/cpc`; tests
`cargo test -p cplus-core`, `cargo test -p cpc --test e2e`. Line numbers from 2026-08-01.

## Reproduction

`m3.cplus`:

```cplus
enum E { A, B }

fn f(c: bool, e: E) -> i32 {
    let v: i32 = 7;
    let x: i32 = if c {
        match e { E::A => { return 1; }, E::B => { return 2; } }
    } else {
        v
    };
    return x;
}

fn main() -> i32 { return f(false, E::B); }
```

```
$ target/release/cpc check m3.cplus
error[E0302]: type mismatch ... '()' vs 'i32'
```

Expected: compiles (the then-arm diverges through the match, so `x` takes the else-arm's
type). The same match in fn-tail position already compiles — that path uses the correct
predicate.

## Root cause

Three divergence predicates exist:

- sema.rs:21193-21218 — private `block_diverges`/`expr_diverges`: no `Match`, `Await`, or
  `Yield` arms, and no recursion into a trailing statement. Used by `check_if` at
  sema.rs:10506 and 10520. This is the weak one.
- sema.rs:21073-21102 — `body_returns_or_diverges`, a third variant.
- lower.rs:2316-2360 — `crate::lower::expr_diverges`, the canonical implementation,
  already used by sema for match arms (sema.rs:9696) and fn-tail checking (sema.rs:7244).

Secondary effect: `check_if`'s move-join treats the weakly-judged then-branch as
converging, so moves inside a genuinely diverging branch stay unioned into the fall-through
state — a latent spurious E0335 ("use of moved value") for code after the `if`.

## Fix

1. Delete sema's private `block_diverges`/`expr_diverges` pair (21193-21218).
2. At both `check_if` call sites (10506, 10520), call `crate::lower::expr_diverges` /
   its block form instead.
3. Review `body_returns_or_diverges` (21073): if its behavior differences are not
   load-bearing, fold it into the lower predicate too; if they are, add a comment saying
   exactly which construct needs the difference.

## Verification

1. DONE: `m3.cplus` compiles. It returns **7**, not 2 — the report's expected value is a
   slip: `main` calls `f(false, E::B)`, which takes the ELSE arm (`v` = 7). `f(true, E::B)`
   is what returns 2. Both are pinned by
   `if_branch_diverging_through_a_match_imposes_no_type` in cpc/tests/e2e.rs.
2. The secondary move-join effect could NOT be reproduced, before or after the fix. The
   shape it predicts — move a value in a diverging-via-match then-branch, use it after the
   `if` — compiles on the pre-fix binary too. Reason: the match's OWN arm handling
   (sema.rs, `arm_diverges`) already uses the canonical predicate and drops a diverging
   arm's moves, so those moves never reach `moved_after_then` for `check_if` to union in.
   The claim is theoretically right about the code but has no observable case today.
3. DONE: full suites green.

## Third predicate

`body_returns_or_diverges` (step 3) was NOT folded in: its `StmtKind::Loop` arm — a
break-less `loop` never falls through — is load-bearing for E0306 and has no counterpart
in the lower predicate, which deliberately treats a loop as fall-through (the conservative
answer for move-flow). A comment at its definition now says exactly that.
