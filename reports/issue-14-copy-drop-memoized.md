# Issue 14 — Copy/Drop classification: replace the fixpoint + settled-flag with memoized derivation

- Status: PARTIAL 2026-08-02, commit <pending> — step 1 (the characterization
  harness) done; steps 2-5, the migration itself, NOT done. See "Why the
  migration is not in this commit".
- Type: structural consolidation (highest-risk item in the set — characterize first)
- Area: `cplus-core/src/sema.rs`
- Effort: M-L
- Retires / prevents: the ordering contract between the classification fixpoint and
  late-synthesized instantiations; the second copy of the Drop-derivation rule; the
  context-dependent `is_copy(Ty::Param)` answers
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 2; sema type-core audit F11/F12)

## Problem

Whether a type is Copy / carries Drop is computed by two regimes joined by a boolean:
an alternation fixpoint that runs during the main pass sequence, and a creation-time
finalizer for instantiations synthesized after the fixpoint, gated by
`copy_flags_settled`. Correctness requires (a) nothing instantiates between
`reconcile_drop_from_methods` and the settled flip, and (b) the late path re-deriving
the SAME Drop rule the fixpoint uses — it is a hand-maintained second copy. The
uncertainty is visible in the code's own comment: `is_copy: false, // recomputed by
compute_struct_copy_flags? not for late-synthesized` (sema.rs:17031).

## Current state

- Alternation loop: sema.rs:1001-1013 (ordering comment "step 4 before step 5";
  `reconcile_drop_from_methods` at 994 must precede — a documented ordering contract).
- Late finalizers: `finalize_late_struct_copy_flag` 2944-2972,
  `finalize_late_enum_copy_flag` 2977-2993 — inert until `copy_flags_settled` (2947,
  2978); the struct one independently re-derives Drop from `generic_impl_methods`
  (2951-2960) — the second copy of the rule.
- Context-coupled param answers: `is_copy(Ty::Param)` (3011, 3019-3026) reads the
  CURRENT `param_bounds_stack`, so the same Ty answers differently depending on which
  pass asks.
- Related triplication to fold in (same lifecycle problem): generic-instantiation method
  population exists three times — inline in `instantiate_struct_from_arg_tys`
  (17060-17111), verbatim copy `backfill_generic_struct_methods` (4123-4189), and
  `populate_generic_enum_methods` (17784+) with a `methods.is_empty()` lazy retry
  (17727-17729) plus an `enums_populating` reentrancy set. The `is_empty` proxy never
  repairs a PARTIALLY-populated table and rescans method-less templates forever.

## Target design

- One memoized, on-demand derivation:

```rust
fn is_copy(&self, ty: &Ty) -> bool      // memo + visiting-set for cycles
fn carries_drop(&self, ty: &Ty) -> bool // same walker, different predicate
```

walking fields/payloads with a visiting set (the in-file precedent for the shape is
`marker_blocked`). Delete the fixpoint loop, `copy_flags_settled`, and both late
finalizers: one rule, no ordering contract, late-synthesized instantiations get the same
answer as everything else because the answer is derived when asked.
- `is_copy(Ty::Param)` takes the bound context as an ARGUMENT (or resolves through the
  substitution in hand) instead of reading ambient stack state.
- One `populate_methods(owner_id)` helper + an explicit "templates registered" barrier
  after which instantiation always populates inline; delete the empty-check triggers and
  the reentrancy set.

## Migration plan (characterize FIRST)

1. Characterization harness: a debug flag or test hook dumping `(type, is_copy,
   carries_drop)` for every type in a compile; run it over the e2e corpus and snapshot.
2. Implement the memoized pair alongside the old regime; assert equality against the old
   answers across the corpus (both live for one commit).
3. Flip consumers to the memoized pair; delete the fixpoint + flag + finalizers.
4. Param-bound context threading.
5. Method-population unification (separately committable).

## Verification

- Step 2's equality assertion over the full corpus is the main gate; then full suites +
  vendor suites.
- Drop-count e2e tests (MADE == DROPS patterns) are the behavioral guard for Drop
  classification — run the memory-model groups.
- Pay attention to programs where Copy depends on Drop reconciliation order (a struct
  whose Copy-ness flips when a `fn drop` exists in a generic impl) — write one such test
  explicitly if none exists.

## Risks and constraints

- Wrong Copy/Drop answers are silent double-frees or leaks — hence the
  dual-running-with-assert step; do not skip it.
- Keep the blessed-bounds semantics unchanged (Copy-bound checking uses these answers;
  bug-21's fix may land near here — coordinate).

## Step 1 — the characterization harness

```rust
pub fn classify_types(program, entry_file, entry_src, files)
    -> Vec<(String /* type */, bool /* is_copy */, bool /* has explicit drop */)>
```

It runs the ordinary checker and reports what the current two-regime
classification concluded, sorted by name; `MonoInfo::type_classification`
carries the snapshot and nothing in the pipeline reads it. Step 2 — running the
memoized derivation alongside the old regime and asserting equality over a
corpus — is what this exists for, and the report is right that it is the gate:
a wrong Copy/Drop answer is a silent double-free or a leak, not a failed
assertion somewhere visible.

The unit test `copy_and_drop_classification_is_characterized` pins the answers
for the shapes the two regimes disagree about most easily: a Copy struct of Copy
fields, a struct with an explicit destructor, a struct that is non-Copy only
because a FIELD carries one (the fixpoint's job), a plain enum, a tagged enum
with a Drop payload, and — the reason the second regime exists at all — a
generic instantiation synthesized after the fixpoint has run, whose flags come
from the `copy_flags_settled` finalizer.

### One finding, worth carrying into the design

The harness first reported the recursive `ty_carries_drop` derivation and
promptly overflowed the stack on `mutually_recursive_structs_rejected_e0913`:
that walk has no visiting set, and although sema REJECTS mutually recursive
types (E0913), it does so after the tables are built, so the derivation is
reachable on a cyclic shape. The report's target design already specifies a
visiting set ("the in-file precedent for the shape is `marker_blocked`") — this
is the concrete demonstration that it is load-bearing rather than tidy, and the
memoized version must have it before it replaces anything. The harness reports
the stored flags instead.

## Why the migration is not in this commit

Steps 2-5 replace the classification regime for every type in the language. The
plan's own sequencing — implement the memoized pair ALONGSIDE the old one, assert
equality across a corpus for a full commit, and only then flip consumers and
delete the fixpoint — is not something to compress into the tail of a session
that has already landed thirteen other issues. A half-flip is precisely the
failure the report warns about: the answers are silent when wrong.

What the next session needs is now in the tree: the harness, a test that pins
the current answers, and the visiting-set finding above.

## Verification (as run)

`cargo test -p cplus-core` 1852 + 8, `cargo test -p cpc` 608 + 16 + 5 + 6,
`cpc test` in `vendor/stdlib` 290 green in debug and `--release`.
