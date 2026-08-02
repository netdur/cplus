# Issue 10 — Merge the twin method-monomorphization paths; Self as an ordinary subst key

- Status: DONE 2026-08-02, commit <pending> — with one part deliberately left,
  see "What is still open"
- Type: structural consolidation
- Area: `cplus-core/src/monomorphize.rs` (+ a sema note)
- Effort: M
- Retires / prevents: bug-07 (structurally); the documented "the two paths must mirror —
  test the combo" obligation; the propagation-closure quadruplication
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #10, §2 method-mono row)

## Problem

Generic methods are expanded by two parallel paths — concrete-struct impls and
generic-struct synthesized impls — that duplicate ~80 lines and must mirror each other.
The mirror obligation is documented in-code because a divergence already caused a codegen
panic once. The Self-substitution walker (bug-07's partial clone) exists only because
expansion is split this way.

## Current state

- Path A (concrete struct): `rewrite_item_calls`' `ItemKind::Impl` arm,
  monomorphize.rs:1973-2005.
- Path B (generic struct): `synthesize_generic_typed_impls`, monomorphize.rs:711-762,
  confessing at 720-724: "The non-generic-struct path does the same at the ItemKind::Impl
  arm; this is its generic-struct counterpart."
- Both do: match `method_instantiations`, `build_subst`, `mangle_name`, clear
  `generic_params`, subst params/ret, rewrite body.
- Hidden trap: the shared `BTreeSet<(String, String, Vec<Ty>)>` mixes key universes —
  Path A matches by SOURCE struct name, Path B by MANGLED instance name
  (sema.rs:1162-1164 documents that sema records by the receiver instance's mangled
  name). Disjointness holds only because E0917 reserves interior `__` in user names.
- Self-rewrite second walker: `rewrite_block_with_self` (870-892) driving
  `rewrite_stmt_self`/`rewrite_expr_self` (909-944, 972-1102) — the partial clone behind
  bug-07.
- Same-file cleanups to fold in:
  - Propagation discovery closure duplicated 4x inside `propagate_fn_instantiations`
    (1462-1486, 1528-1549, 1580-1601, 1621-1639) driving two sequential worklists;
    `INSTANTIATION_LIMIT` is checked in loops 1 and 4 only — the seeding scans insert
    uncapped. One worklist + one closure; scans just seed it.
  - The whole fixpoint runs TWICE per compile: `check_instantiation_bounds` (1672-1686,
    driver pre-check for E0910) and `monomorphize` (141-147) both call
    `propagate_fn_instantiations` on the same inputs. If either site's arguments drift,
    the E0910 hang-guard stops matching the real expansion. Run once in the driver,
    thread the set in (or stash in MonoInfo). Delete the dead `_call_monos` param (1415).
  - Receiver-blind generic-method detection: 2541-2544 and 2599-2601 match
    `mname == name && margs == args` IGNORING the struct — any type's same-named method
    with the same arg types triggers the mangle; safe today only via per-impl method
    namespaces + E0917. Fix by recording `(type, method)` per span in sema's
    `method_instantiations` (coordinate the key change with both paths' matching).
  - Parity fossil 682-694 (`let _ = &mangled_from_info; // kept for parity` then
    re-fetching the same value with `.expect`).

## Target design

```rust
/// Expand one generic method for one instantiation.
/// self_target: the concrete mangled instance name when expanding inside a
/// generic-struct impl (Path B); None for concrete-struct impls (Path A).
fn expand_generic_method(m: &Function, subst: &Subst, mangled: &str,
                         self_target: Option<&str>) -> Function
```

Both paths call it. `Self` is handled by the MAIN rewrite walker as an ordinary
substitution key (`Self -> self_target-or-owner`), which deletes the second walker pair
entirely (bug-07's structural fix). If `issue-01-generic-ast-walker.md` has landed, the
body rewrite is one rewriter invocation.

## Migration plan

1. Extract `expand_generic_method` from Path A verbatim; Path A calls it. Green.
2. Path B calls it with `self_target`; delete B's duplicate body. Green (the "combo"
   tests — generic struct × generic method — are the guard; grep mono/e2e tests for
   generic-method tests and run all).
3. Self-as-subst-key in the main walker; delete `rewrite_block_with_self` + the pair.
   bug-07's repro as a test.
4. Worklist unification + single fixpoint run + fossil deletions (each its own small
   commit).
5. (With sema coordination) `(type, method)` keys for method_instantiations; document
   the key universes until then with an assert that no source struct name contains `__`.

## Verification

- Full suites after each step; specifically the generic-struct + generic-method combo
  tests and E0910 instantiation-limit tests (the run-once change must keep the
  pre-check's answer identical — add a test asserting a bounded-explosion program still
  E0910s).
- bug-07 repro green at step 3.

## Risks and constraints

- Do not change mangled-name output at any step (symbol stability); the extraction is
  behavior-preserving until step 5's key change, which alters only INTERNAL matching.

## Outcome

```rust
fn expand_generic_method(
    m: &Method,
    outer_subst: &HashMap<String, Ty>,  // the impl block's own subst; empty for a concrete struct
    margs: &[Ty],                       // the method-level instantiation
    self_target: Option<&str>,          // the instance name `Self` stands for, inside a generic impl
    ..ctx..
) -> Method
```

Both expansion paths call it: the concrete-struct arm in `rewrite_item_calls`
(`outer_subst` empty, `self_target` `None`) and `synthesize_generic_typed_impls`
(the impl's subst, `Some(mangled_instance)`). The ~40 duplicated lines apiece —
each carrying a comment naming the other and saying they must mirror — are one
function.

Step 3 (`Self` as a substitution key) had already landed in the bug tier:
`StructLookup::with_self` carries it, and the walker pair bug-07 came from is
gone. `expand_generic_method` uses that mechanism for bodies and
`rewrite_self_in_type` for signature types, which is the only place the two
paths still differ — and now they differ by one `Option`, in one function.

Also in this commit, from the same-file cleanup list:

- **One entry into the fixpoint.** `propagate_all_instantiations(program, mono)`
  is what both callers use — `check_instantiation_bounds`, the driver's E0910
  pre-check, and `monomorphize` itself. They each used to spell out five
  arguments, and the report's hazard is exactly that: if the two argument lists
  drift, the hang-guard stops describing the expansion it guards. (The fixpoint
  still RUNS twice; that is a cost, not a hazard, and removing it means
  threading the result through `monomorphize`'s public signature.)
- **The dead `_call_monos` parameter** is gone.
- **The parity fossil**: `let _ = &mangled_from_info; // kept for parity with
  prior shape`, followed by re-fetching the same value from the same two maps
  with `.expect("instantiation present (just iterated)")`. It uses the value it
  already had.
- **The key-universe hazard is asserted.** `method_instantiations` is one set
  holding two key universes — the source struct name (path A) and the mangled
  instance name (path B) — disjoint only because E0917 reserves interior `__`.
  A `debug_assert` says so at the matching site.

## What is still open

The receiver-blind match (`mname == name && margs == args`, ignoring the struct)
needs sema to record `(type, method)` per span; changing the key touches both
paths' matching and sema's recording, and it is safe today for the reason the
assert now states. Left as its own change, per the report's own step 5.

## Verification (as run)

- `--emit-ll` over 40 `docs/examples` plus the ABI probes: byte-identical
  through the merge and through the cleanups — mangled names unchanged, which
  is the report's hard constraint.
- `cargo test -p cplus-core` 1847 + 8, `cargo test -p cpc` 608 + 16 + 5 + 6
  (the generic-struct × generic-method combo tests are in there), `cpc test` in
  `vendor/stdlib` 290 green.
