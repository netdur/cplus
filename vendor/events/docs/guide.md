# Guide

How the package is meant to be used, why the two layers exist, and the
gotchas that bite. For a fast start see [tutorial.md](tutorial.md); for
signatures see [ref.md](ref.md).

## Why two layers

| | `Signal[T]` | `Bus` / `events::on`·`emit` |
|---|---|---|
| Payload | any `T`, compile-checked | always `str` |
| Coupling | share the signal value | share only a string name |
| Home | field or module static | app-wide shared bus (or a private `Bus`) |

**Use `Signal[T]`** when both ends already share a type — a component field
`opened: Signal[str]`, a module static, a small mediation module. You get
typed payloads and an obvious place to subscribe.

**Use the shared bus** when sender and receiver must not import each other
(plugins, menus, CLI → UI). Coordination is the event name only.

**Use a private `Bus`** (`Bus::new()`) when you want string routing without
the process-wide instance: unit tests, a modal stack, a subsystem boundary.

There is no third pub/sub fabric. High-frequency or multi-threaded fan-out
is out of scope; compose with `stdlib/channel` and emit on the owning
thread (see [Threading](#threading)).

## The listener shape (and why it matches facet)

Every registration is a function pointer plus an opaque `ctx`:

```text
fn(payload, *u8)  +  ctx: *u8
```

That is the same convention facet handlers use, so a component method binds
directly:

```cplus
sig.on(this.on_file_open);           // this rides as ctx
events::on("file:open", this.on_open);
```

Free functions take the ctx parameter explicitly. When you need no
identity, omit ctx at registration (default null) and ignore the argument.

## Naming bus events

Names are plain equality — no hierarchy, no wildcards.

- Prefer a stable `"domain:action"` form: `"file:open"`, `"app:quit"`.
- Put data in the **payload**, not the name.
- Document the app's name set; synonyms (`"openFile"` vs `"file:open"`) are
  silent bugs.
- `off_all("file:open")` clears that name only.

## Ownership and lifetimes

What the package guarantees:

- **`ctx` is borrowed.** Stored and passed back; never allocated or freed
  by the signal/bus.
- **Bus event names are borrowed `str`s.** Stored by view, not copied.
  Literals are ideal; a buffer must outlive the subscription (same rule
  as `ctx`). Names are never mutated through the bus.
- **Payloads are borrowed** for the duration of `emit`. Keeping a pointer
  into the payload after return is undefined for the caller.
- **Tokens do not own listeners.** Losing a token does not leak: the
  listener is still reachable via `off_ctx`, `off_all`, or `remove_all`.
- Registration storage is freed by `off` / `off_all` / `off_ctx` /
  `remove_all` (or dropping the signal/bus). Registering on the bus does
  **not** allocate for the name.

What you must guarantee:

> The receiver must **outlive** its registration.  
> A bus event **name** must **outlive** its registration.

A bound method's ctx is the receiver's address. Free the receiver while
still registered → the next emit is a use-after-free. Patterns that work:

1. Long-lived module statics.
2. Component-owned signals and fields (facet-style retain).
3. Explicit detach before free:

```cplus
let _n: usize = events::off_ctx(#addr_of(bar) as *u8);
// bar may now be destroyed
```

### Gotcha: `off_ctx(0)`

`ctx == 0` matches listeners registered **without** a ctx. It does **not**
mean "remove everyone". Null-ctx free functions are one cohort; bound
methods are another.

### Gotcha: stale tokens after `remove_all`

Token numbering is not reset. After `remove_all`, every previously issued
token stays dead (`off` returns `false`). That is intentional: a stale
`off` must never hit a recycled id.

### Gotcha: token `0`

`0` is never issued as a live listener. `on` returns `0` only if storage
failed to grow. Treat `0` as failure / "no listener"; `off(0)` is a false
no-op.

## Delivery under mutation

These are part of the contract (covered by the package tests):

1. **Order** — ascending token (subscription order).
2. **Add during emit** — new listener has id ≥ the emit's ceiling → does
   **not** run until a later emit.
3. **Remove during emit** — once removed, does not run in the rest of this
   emit.
4. **`once`** — detached **before** the body runs. Re-`once` from inside
   the handler is safe; the new registration waits for the next emit.
5. **Nested emit** — a re-entrant `emit` is a full independent pass; it
   does not corrupt the outer walk.
6. **Empty emit** — silent no-op.

### Why it feels "quadratic"

Each delivery step re-scans the list so mid-dispatch `on`/`off` stays
correct without allocating a snapshot. Cost is fine for UI/app event
counts; this is not a hot-path bus.

## Unsubscribe strategies

| Goal | Tool |
|---|---|
| Drop one listener | keep the token → `off(id)` |
| Drop one bus name | `off_all(name)` |
| Drop everything for a component | `off_ctx(receiver_addr)` (bus: all names) |
| Reset a signal/bus | `remove_all()` |
| Clear the shared bus | `events::remove_all()` |

Prefer **tokens** when one subscription has a different lifetime from its
siblings. Prefer **`off_ctx`** for component teardown so you do not store
every id.

## Shared bus vs private bus

| | Module verbs (`events::on`) | `Bus::new()` |
|---|---|---|
| Scope | whole process | whoever holds the value |
| Tests | leftover names can leak across cases | fresh bus per test |
| API surface | pointer-free verbs | same methods on a value |

`events::shared()` always returns the same `*Bus`. Prefer verbs in app
code; reach for `shared()` only when an API needs a `*Bus`.

## Threading

Dispatch is **single-threaded**. Concurrent `on`/`off`/`emit` on the same
signal or bus is not supported.

Cross-thread pattern:

1. Worker `send`s on a `stdlib/channel`.
2. Owning thread drains and calls `emit`.

Listener bodies then run next to the receivers they touch (UI thread,
component thread). There is no async listener API — schedule work from the
handler and re-check liveness when it completes.

## Facet integration

Buttons and similar widgets already use `(fn, ctx)`. The events package
matches that shape so a component can:

- own a `Signal[T]` field and accept `other.method` subscribers;
- or publish on the shared bus while the UI subscribes with
  `events::on("…", this.handler)`.

On detach, pair facet lifecycle with teardown so parked components never
receive late events:

```cplus
fn on_detach(ref this) {
    let _n: usize = events::off_ctx(#addr_of(this) as *u8);
    return;
}
```

If the component owns a `Signal`, either `remove_all` / drop it, or ensure
subscribers used `off_ctx` against their own identities.

## Payload choices

| Choice | When |
|---|---|
| `Signal[str]` | paths, labels — still a typed channel |
| `Signal[i64]` / numeric | counters, indices, small enums |
| `Signal[Struct]` | only if passing by value is cheap; interiors still must outlive the emit if retained |
| Bus `str` | cross-module serialization point; keep it simple (path, id, or `""`) |

Listeners run **synchronously** inside `emit`. Heavy work should hop to a
task/channel and leave the dispatch stack quickly.

## Decision tree

```
Share a type / field with the other end?
  yes → Signal[T]
  no  → string name enough, str payload ok?
          yes → events::on / emit   (or Bus::new for a private scope)
          no  → put a Signal[T] in a small shared mediation module
```
