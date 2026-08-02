# Issue 07 — Move the view-diagnostic family (E0513/E0515/E0516) into borrowck

- Status: PARTIAL 2026-08-02, commit dc5aa6e — step 6 (the dead codegen net)
  done and step 1 (the inventory) below; steps 2-5, the emission port itself,
  NOT done. See "Why the port is not in this commit".
- Type: structural consolidation (finishes the borrowck rework)
- Area: `cplus-core/src/sema.rs` → `borrowck.rs`; dead-code fallout in `codegen.rs`
- Effort: L
- Retires / prevents: the silent-unsoundness drift seam where sema hand-mirrors the
  NEGATION of borrowck's coverage; the position-enumerated E0365 patch family
- Master report: `core-drift-audit-2026-08-01.md` (§3, §6 Tier 1 #7)

## Problem

The borrowck rework made borrowck the model-bearing pass (memory-model.md is normative),
but sema still emits half the view family and encodes, by hand, exactly where it believes
borrowck's flow analysis will tie a view instead. If borrowck's coverage ever SHRINKS,
sema's skip conditions stay — and the result is NO diagnostic anywhere (silent
unsoundness), not a wrong one. If coverage GROWS, users get double-denies. Two passes
co-owning one rule family, synchronized by belief, is the largest remaining pre-rework
residue.

## Current state

Emission split: sema emits E0513 at ~19 sites, E0515, E0516 (`check_returned_borrow`
sema.rs:15102-15180, `flag_view_leaves` 15212-15262, `check_view_store_escape`
15443-15545, `check_raw_store_declaration` 15419-15441); borrowck emits E0514 at ~23
sites plus one E0513.

The drift engine (the exact code to eliminate):

- sema.rs:15511-15515 — skip the E0515 deny when
  `target_is_receiver && (current_fn_keeps_this || current_method_concrete)`;
- sema.rs:15521-15523 — skip when `current_freefn_exported`;
- the flags documented at sema.rs:1388-1408; the complementary coverage claims at
  borrowck.rs:928-940.
- Second must-agree twin: `method_produces_view` (sema.rs:15655) self-described as
  "matching borrowck's shape-based `detect_method_view`".
- Accretion tell: `root_is_param_view` (sema.rs:15498-15505) is a 3-disjunct predicate,
  each disjunct from a distinct bug.

Related family to fold in as a second phase — the capture-taint escape analysis
(E0365): fixpoint `collect_receiver_capturing_methods` sema.rs:2291-2330,
`capture_sources_inner` 2481-2540, `update_capture_taint` 2556-2605 (exactly 3 statement
shapes), and E0365 emitted at three separately-patched escape POSITIONS: return (2410),
assignment (15558 — the bug file is literally titled "e0365-catches-the-return-but-not-
the-assignment"), call-arg (15627). Position-enumeration means a fourth escape position
needs a fourth patch; as a borrow class judged at frame exit in borrowck, positions stop
mattering.

Dead code this unlocks in codegen (verify then delete): the auto-clone-on-return net
(codegen.rs:10093-10101) + its `borrowed_params` feeder (inserts at 6584, 6604, 8236,
8249, 8491, 8503; sole consumer 10095) — the audit verified borrowck now rejects the
guarded pattern (E0337) so the net is unreachable.

## Target design

Borrowck owns emission AND lift for E0513/E0515/E0516: each sema rule is re-stated as an
explicit borrowck rule against memory-model.md (most are "a view store/return that the
flow pass cannot tie is denied" — i.e., the deny becomes the flow pass's own
fall-through, which is the correct shape: one analysis, deny-where-untied, instead of two
analyses negotiating). Sema keeps NO view diagnostics and NO knowledge of borrowck's
coverage.

## Migration plan

1. Inventory: table of every sema view-rule with its lift condition and the e2e tests
   pinning it (grep e2e for E0513/E0515/E0516).
2. Port rule-by-rule into borrowck, each with its pinned tests moved/kept green. Where a
   sema lift said "borrowck covers this", the port DELETES the split: the flow pass ties
   or denies.
3. Flip sema's emission sites to debug-assert-only (a transition release: assert fires
   if borrowck missed something sema would have caught).
4. Delete sema's emission, the lift flags (`current_fn_keeps_this`,
   `current_method_concrete`, `current_freefn_exported` plumbing), `method_produces_view`.
5. Phase 2: port capture-taint (E0365) as a borrow class at frame exit; delete the three
   position patches and the name-string dataflow.
6. Delete the dead codegen net (verify first per above; `unreachable!` for one release
   if cautious).

## Verification

- The E051x e2e corpus stays green throughout; add the audit's residual shapes
  (view-carrying aggregates, bare-coercion lets, free-fn ties) if not already pinned.
- Error-ORDER churn: sema errors bail the pipeline before borrowck runs
  (cpc/src/main.rs:2624-2652) — programs with BOTH a sema error and a view error will
  now report the sema error first and the view error only after it is fixed. Audit e2e
  tests that assert multiple errors from one compile.
- Full suites + vendor suites; run the memory-model e2e groups
  (`memory_model_aliasing_hardening`, `str_view_cannot_outlive_owner`,
  `str_view_coercion_and_free_fn_ties` — names from the project's hardening notes).

## Risks and constraints

- borrowck's EXISTING rules are recently reworked and sound — this issue moves sema's
  rules INTO borrowck; it must not modify current borrowck behavior except by adding
  rules.
- Highest-stakes refactor in the set; the phase-3 assert release is the safety net.
  Do not skip it.

## Step 6 — the dead codegen net, deleted

Verified first, as the report required. `fn echo(x: Text) -> Text { return x; }`
— the exact shape the auto-clone-on-return net existed to compensate for — is
now rejected at check time:

```
error[E0337]: cannot move `x` into an owned value: it is a borrowed binding
(a `borrow`/`mut`/`self` parameter, or a payload matched from a borrowed value)
whose owner still drops it — the move would create a second owner (double-free).
```

Sema/borrowck errors bail before codegen runs, so the net could only fire for a
program the checker let through — and a compensating deep clone for such a
program hides the hole rather than closing it, which is the shape this project
distrusts (the same reasoning that removed the unsound `noalias`). Deleted: the
match arm in `StmtKind::Return`, the `borrowed_params` field, and all six
insert sites across the definition emitters. `clone_string_aggregate` stays —
it has another caller.

## Step 1 — the inventory (so the port can start cold)

Sema still emits the family at these sites, all in `sema.rs`:

| Site | Code | What it denies |
| --- | --- | --- |
| `check_returned_borrow` (~15363) | E0513 | returning a view rooted in a local |
| `flag_view_leaves` (~15473) | E0513 | a view leaking through a carrier |
| `check_raw_store_declaration` (~15680) | E0516 | a raw store whose declaration cannot be tied |
| `check_view_store_escape` (~15704) | E0513, E0515 | a view stored somewhere outliving its owner |
| `method_produces_view` (~15916) | — | the shape twin of borrowck's `detect_method_view` |

The lift conditions that encode a belief about borrowck's coverage
(`current_fn_keeps_this`, `current_method_concrete`, `current_freefn_exported`,
and the three-disjunct `root_is_param_view`) are all inside
`check_view_store_escape`. Borrowck emits E0514 at ~23 sites and no E0513.

The pinning tests: 44 assertions across `cpc/tests/e2e.rs` mention E0513/E0515/
E0516, plus 5 in-sema unit tests around line 25576 (static store of a
view-carrying literal, the ref-target whole-struct store, the alias return of a
rooted carrier, the builtin sub-view chain return). Those are the corpus the
port must keep green — and, per the report, the error-ORDER note applies: sema
bails the pipeline before borrowck, so any test asserting a sema error AND a
view error from one compile will see them one at a time after the move.

## Why the port is not in this commit

Steps 2-5 move a live, load-bearing diagnostic family between passes and change
which pass reports it, with a transition release of debug-asserts as the safety
net (step 3), and the report itself calls it the highest-stakes refactor in the
set. It is not a change to land at the end of a session alongside eight others:
it wants its own pass with the memory-model e2e groups run at each rule, and the
assert release actually shipped rather than compressed into the same commit.
The inventory above is what a cold start needs; nothing in this commit
constrains how the port is done.

## Verification (as run, for step 6)

- The compensated shape reproduces as E0337 at check time on the current binary.
- `cargo test -p cplus-core` 1847 + 8, `cargo test -p cpc` 607 + 16 + 5 + 6;
  `cpc test` in `vendor/stdlib` 290 green in debug and `--release` — the runs
  that would surface a double-free the net had been hiding.
