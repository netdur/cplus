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

Current enforcement state (2026-08-04, erased-return pass): readable bodies
need no declarations, anywhere. The flow pass analyzes concrete AND generic
impl methods (the store structure is type-agnostic; instantiations gate at
call sites via substitution) plus free fns; unannotated bindings resolve
through structural inference; cross-module calls resolve because the
resolver collapses `prefix::item` to qualified idents. The E0515 deny
remains in exactly three places, each for a stated reason: statics (no
owner to tie — intern or own), free fns whose ADDRESS is taken (indirect
calls through fn pointers carry no computed flows), and, with E0516, any
store the analysis cannot see through (the raw seam). `#[keeps(...)]`
is required only there — the opaque boundary — as §5 always said.
A raw-pointer RETURN is not such a boundary: the body is readable, so a
computed flow that carries a view-capable parameter (or the receiver) into
the returned pointer is a return borrow like any other — callers tie the
result to the argument's owner even though the type no longer names the
view (`Sink{key} → box::new → into_raw() -> *u8`). `#[keeps(nothing)]`
opts out, and a fn whose computed flow is empty (a copying constructor)
ties nothing, no declaration needed.

**A DETACHING ARGUMENT is a third sink** (2026-09-11). §3.1 listed two places
a view can escape to — a `static` and a `ref` target — and both are WRITE
targets with a type to inspect. `executor::spawn_local(take f)` is neither: it
hands the value to the reactor and returns, so the value outlives every frame
from there to the end of the program, and the escape happens at an ARGUMENT.
Nor does the escaping value look like a borrow — a `Future[T]` is a
raw-pointer struct that happens to hold everything its `async fn` took by
pointer, so no type-driven rule sees it.

This needs no declaration. `spawn_local`'s body is readable; it just ends at
`#reactor_spawn_local`, an intrinsic the flow pass treated as neutral. Knowing
what its own intrinsics do with an argument is the compiler's job, the same
way it already knows their signatures, so `computed_detaches` is computed like
every other flow — and transitively, because `facet::spawn_ui` forwards to
`spawn_local` and its callers have to be checked too.

What it is NOT is "moving a borrower into a `take` position escapes":
`Future::wait(take this)` is a `take` position and is how every future is
normally consumed. Nor does a forward count — a wrapper handing on the `take`
parameter it owns is a move, judged by the same moved-out filter `check_return`
applies to `return out;`.

`thread::spawn_with` is the same sink and was not on the list until
2026-10-13: its body ends at `#thread_spawn_with`, and a dropped `JoinHandle`
detaches the worker, so a `str` view of a local handed to it was clean and the
worker read freed bytes. Both intrinsics are now `DETACHING_INTRINSICS`. A
handle that is joined does bound the worker, but nothing ties the handle to
the input; lending a local to a thread is `thread::scope`'s job.

**A `take` of a view is a borrow** (2026-10-13). `take` transfers ownership
of an owner; of a `str`, a slice, or a carrier it transfers a COPY of the
view, and the bytes stay with the caller's owner (§4 of the language memory
model already said `take` on a Copy value consumes nothing). Three passes read
`take` as "the callee owns it, nothing to tie", and each dropped a flow:

- the return rule (`detect_fn_view`) skipped `take` parameters, so
  `fn pass(take v: str) -> str { return v; }` returned an untied view;
- the flow pass seeded no `take v: T` of a generic impl as a source, so a
  generic container storing its element (`this.v = v`, `this._slots[i] = v`)
  exported no keep;
- a store through a raw pointer was invisible to the flow pass, so
  `*this._p = v` (`Box::set`), `*slot = v` with `slot` computed from
  `this._ctrl` (`Channel::send`, `MutexGuard::set`) kept nothing.

A generic `T` instantiated to `str` fell through all three — the shape of
rust-lang/rust#164250, an obligation checked against the written type and
bypassed by the instantiation. The rules now:

1. A `take` parameter whose type carries a view contributes to the return
   tie like a bare one.
2. A `take` of an impl's own generic parameter is a flow source; the call
   site's substitution decides which instantiations tie (`Box[str]` does,
   `Box[Text]` is a real move).
3. A raw store `*p = v` whose pointer DERIVES FROM THE RECEIVER — a field of
   `this`, pointer arithmetic or a cast over one, or a pointer loaded through
   one — is a store into the receiver. This is computed, not declared: the
   provenance is visible in the body. A store through a pointer of any other
   provenance stays at the raw seam (E0516 for a written view type).
4. A GENERIC free fn (not flow-analyzed) whose return type names its own
   parameter ties its result, at the call site, to each argument at a
   generic-typed position that carries a view — by the turbofish's
   substitution, or by the argument itself when there is none. `box::new`,
   `arc::new`.
5. Handles that SHARE storage — a non-Copy result carrying the receiver's
   storage, `m.lock()`, `a.clone()` — are aliases: a view kept through one is
   kept by all of them. An alias group lasts as long as its names are in
   scope (block exit, and a match arm's end for payload bindings), so a
   sibling block reusing a name starts a new group.

**`cpc headers` is one, and the list above did not name it** (2026-09-11).
Header generation replaces a concrete module's bodies with `;`. A DECLARED
`#[keeps(this)]` is text and survives; a COMPUTED flow is not, so it was
deleted — a consumer resolving the package through `lib/include/` computed an
empty flow and tied nothing, while the same consumer resolving it through
`src/` reported E0514. Since vendor packages are consumed from `~/.cplus`
store copies that carry headers, that was the normal path.

The resolution is not a new declaration but the rule the generator already
applies to generics and coroutines: **when a body is the only record of a
fact, the body ships.** `cpc headers` keeps the body of any item whose
computed flow lands on a position that is actually a borrow — a view-typed or
`ref` parameter — and strips everything else as before. This is cheap because
such sites are rare: across the 166 modules of stdlib, facet, facet_appkit,
terminal and events there are none. What the flow pass finds in bulk is Copy
values stored into a receiver (`grow_to(ref this, new_cap: usize)`), which tie
nothing and are filtered out.

Translating the flow into an attribute instead is not available today:
`#[keeps(...)]` takes only `this` or `nothing`, so it cannot name WHICH
parameter is kept, and it has no free-function form for the
`(src → ref dst)` flows at all.

## 4. Out of contract

Tracking stops, by design, at three boundaries. Code behind them is the
author's responsibility, and the API surface in front of them carries the
declarations of §5.

- **Raw pointers.** Anything reached through `*T` is untracked (`opaque`
  field accountability covers who frees; nothing covers who outlives).
- **Erased contexts.** A view that crosses an erased `*u8` boundary (callback
  context slots, `#addr_of`, a rooted pointer stored onward or passed to
  extern) is invisible. APIs that store keys or labels for later use must own
  (`Text`), intern, or document static-only input. A `*u8` merely *returned*
  from a readable body is not erased — computed return flows reach through it
  (§3).
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
