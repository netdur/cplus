# Issue 18 — Generic impl-method bodies: check them, then decide what to report

- Status: PARTIAL 2026-08-03 — the RECORDING half is done and closes five
  ICEs plus bug-27 shape 4; the REPORTING half is open and now blocked on
  exactly ONE question (E0337 in the raw-pointer containers), the other three
  classes having been closed or shown to be measurement artifacts.
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

**Measured properly (2026-08-03):** the first count was taken by checking each
stdlib file standalone and reading the whole output, which mixed in errors
those files produce out of package context anyway — `io.cplus` alone reports a
duplicate `println` on the PRE-PORT binary too. The honest number is the delta
between this pass reporting and not, per file. It is:

| Class | Count | Status |
| --- | --- | --- |
| E0337 | 93 | the real question, below |
| E0324 | 24 → **0** | fixed: the hash containers now declare `K: Hash + Eq + Copy` |
| E0306 | 2 → **0** | fixed: **bug-30**, and it was never about generics |
| E0301 / E0302 | — | measurement artifact, not from this pass |

So one class remains. What it was, and what each turned out to be:

1. **E0337 (238)** — `let v: T = { *p };`, the move-out-of-raw-pointer that
   IS what a container does. `Box::into_inner`, `Vec::pop`,
   `Vec::swap_remove`, and the `Rc`/`Arc` refcount paths are all this shape.
   Under an abstract `T`, `is_copy` is false and the rule fires. Either the
   rule has to learn that a container moving a value out of storage it owns
   and is about to free is the sanctioned case, or these have to be written
   differently.
2. **E0324, CLOSED** — `no method 'hash' / 'eq' on type 'type-param'`. The
   first guess was that this pass dropped the bounds, and an impl block DOES
   re-declare a target's parameters without them, so the merge was written
   (it is in, and it is correct in general). It was not the cause.
   `struct HashMap[K: Copy, V: Copy]` never declared `Hash` or `Eq` at all —
   the module's header comment said "`K.hash()` and `K.eq(other)` are
   required" as PROSE and the body called them anyway, which worked only
   because these bodies were never checked. The bounds are declared now
   (`K: Hash + Eq + Copy`), which bug-21's alignment made expressible; the
   274-file sweep is unchanged, so no caller loses.
3. **E0306, CLOSED — and it was never about generics.** `expr_can_break`
   treated every unnamed expression kind as able to `break`, and intrinsics
   were unnamed, so a `loop` with no `break` was judged breakable the moment
   it mentioned one and the function was asked for an unreachable `return`.
   Reduced to a concrete eight-line program that fails on the pre-port binary
   too: **bug-30**. The two instances here were only its first sighting,
   because the shape lives in `channel.cplus`'s generic bodies.

(1) is what is left, and it is a language question, not a cleanup: **the
stdlib's container primitives do not satisfy sema's own move rules, and
nothing has been checking.** (2) was the same shape one level up — a generic
contract asserted in a comment instead of in the type. Both are what the
audit keeps finding: a rule and its only real consumer drifted apart with no
pass in between.

## What is still open

- Decide (1): a container exception in the move rules, or a rewrite of the
  raw-pointer primitives in the seven stdlib files. Then delete the
  `sink.truncate` and let the pass report. To measure progress, comment out
  that one line and re-run the sweep — deliberately not an env var, because a
  compiler with a hidden diagnostic mode is worse than one that needs a
  rebuild.
- With reporting on, `check_generic_method_body_names` becomes redundant —
  real typing subsumes name resolution — and should go.

## Verification (as run)

- The seven probe families above, each with its generic-free-fn control.
- `cargo test -p cplus-core` 1869 + 8; `cargo test -p cpc` 618 + 16 + 5 + 6
  (1 new e2e pinning five of the shapes end to end).
- Vendor + examples `cpc check` over all 274 sources: **byte-identical**
  against a binary built at `3a7601d`, which is what record-only buys.
