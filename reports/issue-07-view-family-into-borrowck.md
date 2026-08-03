# Issue 07 — Move the view-diagnostic family (E0513/E0515/E0516) into borrowck

- Status: DONE 2026-08-03. Steps 1-4 and 6 in `f4e5a62`..`62d0213` (9 commits,
  2026-08-02); step 5, the E0365 capture-taint port, in `59e8a73`..`53235d5`
  (7 commits, 2026-08-03). Step 6 landed earlier in `dc5aa6e`. Sema now holds
  no view or capture diagnostic and no belief about borrowck's coverage.
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

## Step 5 — the capture-taint / E0365 port, DONE 2026-08-03

Same shape as steps 2-4, one rule per commit, sema silent and asserting from
`752afd3`, sema's detection gone in `53235d5`.

| Commit | What | Code |
| --- | --- | --- |
| `59e8a73` | the `receiver_capturing` fixpoint, the taint dataflow, the classifier, the return sink | E0365 |
| `413237f` | the store sink (`static` / `ref` target) | E0365 |
| `cc846b2` | the call-argument sink | E0365 |
| `752afd3` | transition: sema records, debug-asserts borrowck agreed | — |
| `b8e5c35` | the gap the assert found — a by-value Copy parameter | E0365 |
| `7079f48` | the over-fire probing found — an enum constructor argument | E0365 |
| `53235d5` | deletion: sema's detection, the taint map, the fixpoint, `check_returned_borrow` | — |

The rules live in `borrowck.rs` inside `ViewRules`, under their own heading.
One source classifier, one ownership gate, and the sinks are places the walk
already visits: `walk_stmt`'s `Return`, `walk_expr`'s `Assign`, and a `Call`
arm that asks each argument before descending into it. The `update_capture_taint`
hook runs at the top of `walk_stmt`, where sema ran it. A fourth escape
position is now a place the walk already goes, which was the whole argument
for moving it.

### What the transition assert found

One hole, on the first probe aimed at it, and it is a good one:
**a by-value parameter of a Copy type**.

The gate had been ported as `owns_value`, which is what sema's own prose
says. But sema's `owns_value` for a parameter is `param.move_ || is_copy(ty)`,
and the second half is exactly the capture question: a by-value `Copy`
parameter is the frame's OWN copy, so a handler bound to it points at this
stack slot, which is gone at return. The view family never noticed, because
it filters that case out one gate later — `root_dies_at_return` requires
`owns_value && !is_copy`, since a Copy root owns no heap to free.

So the two questions read identically in prose and disagree on exactly one
shape. They now say so in the code instead of sharing a helper. Controls: a
NON-Copy by-value parameter names the caller's storage and stays legal, and
a `this` receiver never widens — `this` names the caller's object however it
is passed, and widening there would reject every handler in every component.

Nothing exercised this: not the 16 sema unit tests, not the new e2e corpus,
not one of the 274 vendor and examples sources. A probe found it, which is
the third time in this issue that the transition assert has earned the
release it costs.

### What probing found, in the direction the assert cannot see

The assert only checks sema ⊆ borrowck. The other direction — borrowck
denying what sema allowed — needs the A/B sweep, and it caught one
**over-fire**: the argument sink was hung on every `ExprKind::Call`, and
`Enum::Variant(payload)` parses as a call. There is no callee there to keep
anything, so `var h: Holder = Holder::Some(c.build());` — where `h` dies
with the frame — was denied when sema allowed it. `SigTable::enums` already
existed for exactly this distinction on the view side. The payload is still
judged where the value it built actually escapes.

### One deliberate widening, and a pre-existing ICE it uncovered

A capture through a GENERIC receiver is now denied where sema could not see
it: sema's `place_ty_quiet` yields the instantiation, whose name does not
match the `impl Cell[T]` target key its fixpoint recorded, so the lookup
missed. Borrowck reads `TypeKind::Generic`'s base name, like the rest of the
pass, and finds it. It is a true positive — `var c: Cell[i32] = ...;
return c.build();` returns a value holding `&c`.

Probing that shape surfaced an unrelated pre-existing bug: a bound-method
reference inside a generic impl (`take_handler(this.tap)` in `impl Cell[T]`)
panics in codegen at `codegen.rs:2233` with "sema validated". Confirmed
orthogonal — the pre-port and post-port binaries ICE identically on a
program with no capture escape (`this.c.build()` from a field). The port
only masks it for programs that also have an escape. Not fixed here; worth
its own bug file.

### Corrections to the handoff and to this report

1. **`docs/errors.toml` has no E0365 entry.** The handoff said both it and
   the generated `docs/ERRORS.md` carry an `emit_site` for E0365 to update at
   the deletion commit. Neither mentions the code at all — the catalog jumps
   E0364 → E0384. Nothing to update, and nothing was: `gen_errors.py` was not
   run. Adding the missing entry is a separate, real gap (the catalog calls
   itself the single source of truth).

2. **The error-ORDER churn is real here, unlike in steps 2-4.** Several
   probe programs report one fewer diagnostic than before, always because
   they ALSO have a sema type error: sema bails the pipeline before borrowck,
   so the capture denial now appears only after the type error is fixed. Two
   of the shapes this hides are ordinary — calling a `ref this` method on a
   `let` binding or on a match payload is E0328 first — so it will be seen.

3. **Diagnostic ORDER changed** where a method and a free function both deny
   in one program: sema checked every function before every method, borrowck
   walks items in source order. Same content, different sequence. No test
   pinned it.

4. **The port does not follow a deref to find the capture root**, where the
   rest of borrowck's place handling does. `(*p).handler` binds a method to
   the pointee, whose lifetime is not this frame's question — that matches
   sema's `place_root_ident`, which also stopped at a deref, and it is now
   stated rather than incidental.

5. **The taint map is ordered** (`BTreeMap`, where sema used a `HashMap`).
   The "carried out by `x`" phrasing picks a carrier by iterating it, so with
   two carriers sema's message depended on hash order. It is now the same
   every run.

### What the deletion took with it

`check_returned_borrow` went, as the handoff predicted — the capture escape
was its last job. That left `setup_returned_borrow_ctx` recording state
nothing reads (param names, `ref` write targets, per-param regions, the
return region), so it is now `check_return_region_declared`: the E0511
signature rule, which is all it still does. `walk_expr_tree` and
`walk_block_exprs` had no other caller and went too; the port has its own
`for_each_expr`, scoped to the questions with no state to keep. So did the
`view_findings` transition hook — both asserts have now shipped their
release.

### Verification (as run, step 5)

At every commit: `cargo test -p cplus-core` (1869 at the end, 8 new tests),
`cargo test -p cpc` (616 + 16 + 5 + 6, no new e2e — the corpus was written
first, in `f3a5a4e`), the stdlib suite in debug and release (290 each). From
the transition commit on, everything ran in BOTH modes with the assert live
in debug.

Differential, against a `cpc` built at `3a7601d`:

- vendor + examples: all 274 `.cplus` sources — byte-identical `cpc check`
  output in release, and no assert or panic under the debug binary;
- 80 purpose-built probe programs covering every sink, loops (`while`,
  `for`-range, C-style, `loop`), `defer`, nested scopes, shadowing, match
  payloads (owned and field-borrowed), destructuring, tuples, array
  elements, derefs, generic receivers, enum constructors, associated
  calls, fn-pointer FIELDS vs bound method references, methods split
  across impl blocks, two-hop taint, and every ownership flavour of
  parameter and receiver — identical except the two documented above (the
  generic-receiver widening, and one pure ordering difference).

## Step 5 — the capture-taint / E0365 port, as it was scoped

Kept for the record: this is what the sizing said before the port ran, and
it held up — the function inventory, the trust boundary, and the insistence
on the transition assert were all accurate. The one thing it got wrong is
noted above: it said `owns_value` was shared between the two families, and
the assert proved the two gates differ on Copy parameters.

Skipped deliberately at the time, as the report allows. It is separable: E0365 is the
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

### What it will take, sized (2026-08-02)

Scoped after steps 2-4 landed, so the next session does not have to
re-measure. It is the same size as the view port, not a follow-up trim:

- ~13 functions, ~400 lines in `sema.rs`: the `receiver_capturing` fixpoint
  (`collect_receiver_capturing_methods`, `block_calls_capturing_on_this`,
  `block_binds_this_method`), the per-body `capture_taint` dataflow
  (`update_capture_taint`, `capture_sources_inner`, `capture_sources_flow`),
  the `local_dies_here` gate, and the three emission sites
  (`flag_escaping_local_receivers`, `check_capture_store_escape`,
  `check_capture_arg_escape`).
- 16 sema unit tests, 47 assertions on E0365. **The e2e corpus now exists**
  (`f3a5a4e`): `a_capture_of_a_local_escaping_the_frame_is_rejected_e0365`
  covers all four escape positions in direct and transitive form plus the
  builder route, and `a_capture_that_outlives_nothing_still_compiles` pins
  the controls — `this`-bound handlers and the binding site — that make the
  rule cost nothing in real code. Written against the pre-port binary, so it
  does not move when the emission does. That was the missing safety net; the
  sema unit tests are the ones that will move.
- One control there is a documented TRUST boundary, not a soundness claim:
  `var n: i32 = take_handler(c.clicked); return n;` compiles because the
  analysis does not read callees. A port must preserve it deliberately or
  change it on purpose.
- Everything it needs already exists on the borrowck side: `ViewRules`
  supplies the scoped walk, the `owns_value` gate `local_dies_here`
  duplicates, and `infer_ty` for `place_ty_quiet`'s field-vs-method
  disambiguation (`sigs.struct_fields` + `sigs.methods`). The
  `receiver_capturing` fixpoint is purely syntactic and ports as-is.
- Shape: ONE source classifier and ONE ownership gate, with the sinks being
  the walk's own structure rather than three patched positions. A fourth
  escape position is then a place the walk already visits.
- Do it with the transition assert. It is what caught the three holes in
  steps 2-4, none of which any test or vendor program exercised, and this
  family has thinner test coverage than the view one did.

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
