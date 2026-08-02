# Issue 07 — Move the view-diagnostic family (E0513/E0515/E0516) into borrowck

- Status: DONE 2026-08-02, commits `f4e5a62`..`62d0213` (9 commits) — steps 1-4
  and 6 done; step 5 (the E0365 capture-taint port) deliberately NOT done and
  still open, scoped below. Step 6 landed earlier in `dc5aa6e`.
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

## Outcome — steps 2-4, as landed

One rule per commit, sema silent and asserting from `7dd8de9`, sema's
detection gone in `62d0213`.

| Commit | Rule | Code |
| --- | --- | --- |
| `f4e5a62` | `check_returned_borrow`'s root walk | E0513 |
| `b5e46b2` | `flag_view_leaves` (aggregate leaf, incl. the coercion route) | E0513 |
| `17bad67` | `check_raw_store_declaration` | E0516 |
| `ae0ca8b` | `check_view_store_escape` | E0513, E0515 |
| `1f5b084` | `flag_view_of_temp` — **not in the step-1 inventory** | E0513 |
| `7dd8de9` | transition: sema records, debug-asserts borrowck agreed | — |
| `87a34c2`, `6508285` | the three gaps the assert found | E0513 |
| `62d0213` | deletion: emission, detection, lift flags, `method_produces_view` | — |

They live in `borrowck.rs` as `ViewRules`, a syntax-directed pass over every
body, run from `analyze_with_diags` beside the existing E0384 pass. It reads
the same `SigTable` the flow pass publishes, so there is one answer about
what ties.

### The lift, deleted as the report asked

Sema decided the E0515 lift by hand — skip for a receiver store in a
concrete method, skip for a `ref`-param store in a concrete free fn whose
address is untaken. The port asks the summary instead:
`SigTable::effective_keeps(entry)[i]` for a receiver store,
`computed_ref_flows.contains(&(src, dst))` for a `ref`-param store, and a
`static` is never tied. A store the flow pass does not cover is denied,
which is the answer that fails safe — the failure mode the issue was written
about (coverage shrinks, sema's skip stays, nothing denies) is now
structurally impossible.

Three denies survive, exactly the three memory-model.md §3 names: a
`static`, an address-taken fn, and any store the analysis cannot see through
(the raw seam, E0516). One difference from sema's hand-encoding, in the
right direction: a method with its OWN generic params is now denied, because
`compute_receiver_flows` skips it. Sema's "concrete method" test happened to
agree there by accident rather than by asking.

`root_is_param_view` lost a disjunct: `str`/slice is already
`type_contains_view`, so three collapse to two with no change of answer.

### Corrections to this report

1. **The step-1 inventory missed a fifth rule.** `flag_view_of_temp`
   (sema.rs, E0513 on a view of an unnamed temporary receiver) was emitted
   from four call sites — let, return, static assign, assign. Leaving it
   behind would have lost the coverage at the deletion. Ported in `1f5b084`.

2. **`method_produces_view` was not only a twin — it was a slightly wrong
   one.** Sema asked its own tables; the port asks borrowck's recorded
   `detect_method_view` verdict. One consequence: a `#[keeps(nothing)]`
   method no longer counts as producing a receiver view, which sema's copy
   got wrong.

3. **The anticipated error-ORDER churn did not materialise as predicted.**
   No test asserted a sema error and a view error from one compile, so
   nothing needed updating. The churn shows up in the opposite direction:
   two probe programs (a view of a loop-body local stored into a `static`)
   now report E0513 **and** a complementary E0514, because sema's E0513 used
   to bail the pipeline before borrowck could say the second thing. More
   information, not less.

4. **E0512 went with the port.** Its only trigger was the returned-borrow
   root walk, and the `borrow REGION T` syntax it read was retired in
   v0.0.24 #9, so `current_fn_param_regions` has been permanently empty and
   the check could not fire. It had no test and no `errors.toml` entry.
   E0511 (the signature-level region rule) is untouched.

5. **`docs/errors.toml` was stale in two ways.** E0513's `emit_site` pointed
   at `sema.rs:12259`, a line number that had drifted; E0515's `test` field
   named `view_param_stored_into_ref_this_rejected_e0515`, which does not
   exist. Both fixed, along with the generated rows in `docs/ERRORS.md`.

### What the transition assert found

The step-3 assert is the reason this issue insisted on a transition release,
and it earned that insistence three times. Each of these was a program the
pre-port binary rejected and the ported rules accepted:

- **match payloads** (`87a34c2`) — sema types payload bindings and knows
  whether the match owns the value they came from; borrowck bound them
  untyped and borrowing, so `match take_it() { Opt::S(b) => return b.view() }`
  compiled. Ownership now comes from the scrutinee (a call result or
  constructor is a temporary the match owns; a projection names storage
  owned elsewhere, so matching a FIELD and returning a view of the payload
  stays legal), and types from the enum's declared payloads with type
  arguments substituted.
- **tuple returns** (`6508285`) — `CopyOracle::type_contains_view` answers
  for NAMED types, and a tuple has no name until monomorphize synthesizes
  its struct, so `(str, i32)` read as carrying nothing. `return (s, 1);`
  where `s` views a dying local produced no diagnostic at all. Handled with
  a tuple arm in the view rules rather than in the oracle, so no existing
  rule starts tying on a shape it never did — see the follow-up below.
- **index write targets** (`6508285`) — nothing typed `a[i]`, so
  `A[0] = b.view()` into an array `static` walked past the store rules.
  Write targets now resolve step by step: deref, index, field.

None of the three was caught by the 44 pinned e2e assertions or by any
vendor or examples program. They were found by aiming probe programs at the
assert. Compressing step 3 into the port commits, as the report warned,
would have shipped all three as silent coverage losses.

### Follow-up this surfaced (not done)

`CopyOracle::type_contains_view` returns `false` for `TypeKind::Tuple`.
That is a gap in borrowck's EXISTING rules too, not only in the ported ones:
a fn returning `(str, i32)` built from a `str` parameter does not tie its
result to the argument's owner under Rule E-VIEW-FN. Repro shape:

```
fn pack(s: str) -> (str, i32) { return (s, 1); }
```

Fixing it means widening the oracle, which changes what the existing tie
rules do — out of scope for a port that was required not to modify them, and
worth its own change with its own sweep. The view rules cover themselves via
a local `carries_view` wrapper in the meantime.

## Step 5 — the capture-taint / E0365 port, NOT done

Skipped deliberately, as the report allows. It is separable: E0365 is the
capture-taint family (`&local` reaching a handler context), not the view
family, and it shares nothing with what moved except the `owns_value` gate
and `place_root_name`. Its three position-patched escape sites (return
`sema.rs`'s `flag_escaping_local_receivers`, assignment
`check_capture_store_escape`, call-arg `check_capture_arg_escape`) and the
`collect_receiver_capturing_methods` / `capture_sources_inner` /
`update_capture_taint` fixpoint are all still in sema and all still working.
`check_returned_borrow` survives as the return-position hook and does
nothing else.

The case for doing it is unchanged and still good: position enumeration
means a fourth escape position needs a fourth patch, and as a borrow class
judged at frame exit the positions stop mattering. The case for not doing it
in this session is that steps 2-4 moved a live diagnostic family between
passes and that deserved the whole budget, including a transition release
that found three real holes.

## Verification (as run)

At every commit: `cargo test -p cplus-core` (1869 at the end, 15 new tests),
`cargo test -p cpc` (612 + 16 + 5 + 6, 4 new e2e), the stdlib suite in debug
and release (290 each). From the transition commit on, everything ran in
BOTH modes with the assert live in debug.

Differential, against a `cpc` built at `a60a355` in a separate worktree:

- vendor-wide `cpc check` over all 225 sources of all 54 packages —
  byte-identical, in release and in debug, with no panic in either;
- every `examples/*` source (46 files) — byte-identical;
- 31 purpose-built probe programs covering loops, `defer`, destructuring,
  nested scopes, shadowing, generic owners, slices, raw derefs, tuples,
  array elements, match payloads, and every escape sink — identical
  code-for-code except the two E0514 additions noted above.
