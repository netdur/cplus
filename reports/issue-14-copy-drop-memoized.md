# Issue 14 — Copy/Drop classification: replace the fixpoint + settled-flag with memoized derivation

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
