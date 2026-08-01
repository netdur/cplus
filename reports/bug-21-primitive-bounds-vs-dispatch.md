# Bug 21 — `i32` has `hash()` for dispatch but does not satisfy `[T: Hash]` bounds

- Status: probed 2026-08-01 during the audit (E0502 at the bounded call); re-verify before fixing
- Severity: inconsistency (bounded generics unusable at primitives)
- Area: sema (`cplus-core/src/sema.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B21)

Context for the fixer: type params accept only the blessed bounds
(Hash/Eq/Ord/Copy/Drop/Clone/ToText…). Whether a primitive satisfies a bound and whether
a primitive RECEIVER dispatches the blessed method are decided by two different tables
that disagree. Build `cargo build --release`; binary `target/release/cpc`. Line numbers
from 2026-08-01.

## Reproduction

```cplus
fn hash_it[T: Hash](v: T) -> u64 { return v.hash(); }

fn main() -> i32 {
    let x: i32 = 5;
    let direct: u64 = x.hash();     // compiles — blessed dispatch accepts i32
    let bounded: u64 = hash_it(5);  // E0502 — bound says i32 is not Hash
    let _ = direct; let _ = bounded;
    return 0;
}
```

Observed in audit: the direct call compiles, the bounded call errors E0502. Two answers
to "does i32 have hash()".

## Root cause

- `satisfies_bound` (sema.rs:3436-3466): primitives fall to `_ => false` for
  Hash/Eq/Ord/Clone/ToText.
- Blessed receiver tables + dispatch arms (sema.rs:12718-12777, arms at 12104/12182/
  12197): accept primitives for the same methods.
- The comment at sema.rs:12177-12181 admits the workaround: generic `HashMap[K,V]` relies
  on the blessed path "after K is monomorphized to a primitive" — reachable only if K is
  left UNBOUNDED, i.e. the bound system cannot be used exactly where it is wanted.

## Fix

1. Make `satisfies_bound` consult the same blessed receiver tables the dispatch arms use:
   `Hash` → the blessed-hash receiver predicate, `Eq`/`Ord`/`Clone`/`ToText` likewise.
   One source of truth; the tables are already maintained for dispatch.
2. Design note for the owner (state it in the commit message): this makes "primitive
   satisfies blessed bound" official. The alternative — removing primitive blessed
   dispatch — would break existing code; aligning in the permissive direction matches how
   the stdlib containers already behave post-mono.
3. Check the interaction with mono's bound pre-check (E0910 path) so a newly-satisfying
   instantiation actually expands.

Companion: `issue-06-lang-item-registry.md` (single blessed-capability registry is the
end state; this fix is the sema-side alignment).

## Verification

1. The repro compiles; `hash_it(5)` returns the same value as `5.hash()` (runtime e2e).
2. A genuinely unsatisfied bound still rejects: `hash_it(SomeStructWithoutHash {...})` →
   E0502 (negative e2e).
3. `HashMap[K, V]` with a BOUNDED K now compiles if written that way; existing unbounded
   usage unchanged.
4. Full suites.
