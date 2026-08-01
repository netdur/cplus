# The C+ view-lifetime contract

> Status: normative. This page states the invariant the compiler promises for
> views, the boundaries where the promise ends, and the declarations required
> at those boundaries. Checker work is tested against the clauses of this page;
> a safe-code use-after-free is a bug in the implementation of a clause, never
> a new category. Implementation notes live in
> [phase5-borrow-shared.md](phase5-borrow-shared.md) and
> [phase6-borrow-exclusive.md](phase6-borrow-exclusive.md); the running audit
> ledger is `plans/memory-model-hardening.md`.

## 1. Terms

- **View** — a value that reads memory owned by another value: `str`, slice
  types `T[]`.
- **Carrier** — a struct or enum that transitively contains a view field or
  payload. A carrier has the same obligations as a bare view.
- **Owner** — the value whose drop releases the bytes a view reads: a `Text`,
  a `Vec[T]`, any Drop value a view was projected from. String literals have
  static storage and no owner.
- **Sink** — a place a view can outlive the expression that produced it: a
  `return` value, a receiver field, a `ref` parameter target, a `static`, a
  container element.

## 2. The invariant

A view must not outlive its owner. Concretely, the compiler rejects a program
in which a view (or carrier) is readable after the owner of its bytes has been
dropped, moved, or has left scope.

The check is flow-insensitive about conditions: a flow that happens on any
path counts as happening. A store under `if` ties lifetimes whether or not the
branch runs. This is the same posture as the rest of the borrow model
(borrows are lexical) and is the deliberate cost of having no lifetime
annotations.

## 3. What the compiler enforces

1. **Locally**: a view of a local owner cannot be returned (E0513), stored
   into a static or through a `ref` target that outlives it (E0513/E0515),
   or kept live across the owner's move (E0372).
2. **Across calls**: a function's effect on view lifetimes is a set of flows
   from sources (parameters, receiver) to sinks (return, receiver, `ref`
   parameters, statics). Callers apply those flows: a returned view ties the
   result to the argument's owner; a kept view ties the receiver to the
   argument's owner. Flows are derived from the function body where the body
   is readable, and from declarations (§5) where it is not. A view parameter
   stored into an escaping sink without a declared or computed flow is
   rejected at the definition (E0515).
3. **At scope exit**: a binding may not die while a live binding in an outer
   scope holds a view of it (E0514). Assigning a view or carrier outward and
   letting the owner fall out of scope is an error, the same as moving the
   owner.

Current enforcement state (2026-08-01, closing pass): readable bodies need
no declarations, anywhere. The flow pass analyzes concrete AND generic
impl methods (the store structure is type-agnostic; instantiations gate at
call sites via substitution) plus free fns; unannotated bindings resolve
through structural inference; cross-module calls resolve because the
resolver collapses `prefix::item` to qualified idents. The E0515 deny
remains in exactly three places, each for a stated reason: statics (no
owner to tie — intern or own), free fns whose ADDRESS is taken (indirect
calls through fn pointers carry no computed flows), and, with E0516, any
store the analysis cannot see through (the raw seam). `#[keeps(...)]`
is required only there — the opaque boundary — as §5 always said.

## 4. Out of contract

Tracking stops, by design, at three boundaries. Code behind them is the
author's responsibility, and the API surface in front of them carries the
declarations of §5.

- **Raw pointers.** Anything reached through `*T` is untracked (`opaque`
  field accountability covers who frees; nothing covers who outlives).
- **Erased contexts.** A view that crosses an erased `*u8` boundary (callback
  context slots, `#addr_of`) is invisible. APIs that store keys or labels for
  later use must own (`Text`), intern, or document static-only input.
- **`extern` functions.** No body exists to read. The default assumption is
  that an extern function keeps nothing; an extern that stores a pointer must
  be declared (§5). Views do not cross the C ABI as views (the ABI surface is
  `*u8` + length), so in practice this clause governs raw pointers, not views.

## 5. Declarations at the boundary

A function whose body the compiler cannot read through (raw-pointer stores of
view-typed data, extern) must declare its flows. Declarations are trusted, not
verified, exactly like `opaque` on a raw-pointer field: the trusted surface is
small, explicit, and reviewable.

- `#[keeps(this)]` — view arguments survive inside the receiver after the
  call. Callers tie the receiver to each view argument's owner.
- `#[keeps(nothing)]` — the function copies what it needs; its return value
  borrows no argument. This suppresses the default conservative tie on a
  view-returning function (`text::intern` is the canonical case: the returned
  view points at the intern table, not at the argument).

Silence is not neutral where it matters: a function that stores view-typed
data through a raw pointer without a declaration is an error, mirroring the
raw-pointer field rule (drop-or-`opaque`, E0510). One accountability doctrine,
two questions: `opaque` answers who frees, `keeps` answers who outlives.

## 6. The sanctioned ways to keep a string

For API authors, the whole contract reduces to one decision per stored
string:

| The field is | Declare it as | Cost |
|---|---|---|
| Mutable, arbitrary input | `Text` | owns, copies on set |
| Set once, arbitrary input | `str` + `intern()` | one copy for the process |
| Set once, literals only | `str` | free; non-literal input is a compile error at the caller |

## 7. Non-goals

- No lifetime annotations, ever. The two `#[keeps]` forms are the entire
  declaration surface, confined to opaque boundaries.
- No runtime tracking: no drop flags, no reference counts behind safe types.
  Where a static answer would need runtime information, the pattern is
  rejected instead.
- No condition analysis. The checker never proves a branch true or false.

## 8. Test discipline

Every clause above maps to e2e coverage: positive tests (sound programs that
must keep compiling) and negative tests (each sink × each source route must
fire). The ASan sweep is the auditor: a safe-subset program that compiles and
faults under ASan is a contract-implementation bug and its reduction becomes
a permanent e2e case.
