# Issue 18 — Generic impl-method bodies: check them, then decide what to report

- Status: PARTIAL 2026-08-03 — the RECORDING half is done and closes five
  ICEs plus bug-27 shape 4; the REPORTING half is open and scoped below.
- Type: bug family, then a rules decision
- Area: `cplus-core/src/sema.rs` (`check_methods` / `check_generic_impl_methods`)
- Master report: `core-drift-audit-2026-08-01.md`

## The seam

Sema fills eleven span-keyed side tables WHILE CHECKING A BODY — the
`MonoInfo` contract that monomorphize and codegen read back. Generic
impl-method bodies were never checked: `check_methods` routed them to
`check_generic_method_body_names`, a name-resolution pass with no typing.

So for those bodies every record was missing, and each consumer that expected
one crashed at the last pass with `.expect("sema validated")`.

Generic FREE fn bodies do not have this problem — they are checked with their
type parameters pushed and `Ty::Param` standing in for each, and the same
comment in `check_function` records that they were given that treatment after
the same class of crash. This is the impl-method half of that fix.

## What it cost, measured

One probe per span-keyed record, each written twice — inside a generic impl
body, and inside a generic free fn as the control:

| Feature inside a generic impl body | Before |
| --- | --- |
| `#env("...")` | ICE `codegen.rs:11465` |
| `#include_str("...")` | ICE `codegen.rs:11442` |
| inferred struct literal `{ x: 1 }` | ICE `codegen.rs:10901` |
| inferred generic call `ident(7)` | ICE `codegen.rs:13547` |
| inferred tuple literal `(this.v, 1)` | ICE `codegen.rs:11133` — **bug-27 shape 4** |
| turbofish `ident[i32](7)` | false `E0300: undefined name 'i32'` |
| bound method reference | ICE `codegen.rs:2233` — **bug-29** |

Every one of the free-fn controls compiled and ran correctly. Nine other
recorded features (default splices, assoc-fn dispatch, generic struct / enum /
method instantiation, selectors) already worked, because their records are
made outside body checking.

## What was done

`check_generic_impl_methods` instantiates the impl target at its OWN
parameters — `impl Cell[T]` becomes `Cell[Param("T")]` — which gives `this` a
real `StructId` to resolve fields and methods against while every `T`-typed
value stays abstract. The placeholder instantiation is dropped from `MonoInfo`
by the `ty_contains_param` filter that already exists for exactly this kind of
placeholder. Then the ordinary `check_method` / `check_enum_method` runs.

All five ICEs are closed, and the turbofish case now gives the same E0312 a
generic free fn gives for the same source instead of a false "undefined name".

**The diagnostics from the typed pass are discarded** (`sink.truncate`). The
name-resolution pass keeps its own — it runs first and is what users see
today. That is the whole difference between this and the reporting half.

## Why reporting is not turned on

Because it does not pass. Enabling it emits **~250 diagnostics against the
stdlib itself**, in `box.cplus`, `vec.cplus`, `rc.cplus`, `arc.cplus`,
`channel.cplus`, `hash_map.cplus` and `hash_set.cplus`, in four classes:

1. **E0337 (238)** — `let v: T = { *p };`, the move-out-of-raw-pointer that
   IS what a container does. `Box::into_inner`, `Vec::pop`,
   `Vec::swap_remove`, and the `Rc`/`Arc` refcount paths are all this shape.
   Under an abstract `T`, `is_copy` is false and the rule fires. Either the
   rule has to learn that a container moving a value out of storage it owns
   and is about to free is the sanctioned case, or these have to be written
   differently.
2. **E0324 (12)** — `no method 'hash' on type 'type-param'` in the hash
   containers. The bound lives on the STRUCT template's generic parameters
   (`struct HashMap[K: Hash, V]`); the impl block re-declares `K` without it,
   so the bound frame this pass pushes is empty. This one is a straightforward
   gap in the port: merge the template's bounds into the impl's.
3. **E0509 (12)** — moving a field out of a Drop container, same family as (1).
4. **E0306 (1)** — one body's tail is not a `return`.

(2) is a bug in this change and should be fixed first, because it is noise
hiding the real signal. (1) and (3) are the actual question, and it is a
language question, not a cleanup: **the stdlib's container primitives do not
satisfy sema's own move rules, and nothing has been checking.** That is worth
knowing on its own — it is the same shape as the audit's other findings, a
rule and its only real consumer having drifted apart with no pass in between.

## What is still open

- Merge the struct template's generic-param bounds into the impl block's when
  pushing type params (closes the E0324 class).
- Decide (1)/(3): a container exception in the move rules, or a rewrite of the
  seven stdlib files. Then delete the `sink.truncate` and let the pass report.
- With reporting on, `check_generic_method_body_names` becomes redundant —
  real typing subsumes name resolution — and should go.

## Verification (as run)

- The seven probe families above, each with its generic-free-fn control.
- `cargo test -p cplus-core` 1869 + 8; `cargo test -p cpc` 618 + 16 + 5 + 6
  (1 new e2e pinning five of the shapes end to end).
- Vendor + examples `cpc check` over all 274 sources: **byte-identical**
  against a binary built at `3a7601d`, which is what record-only buys.
