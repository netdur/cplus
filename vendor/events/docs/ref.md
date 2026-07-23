# Reference

Manual for the `events` package. Signatures and behavior only — no tutorials.
Import:

```cplus
import "events/events" as events;
```

---

## Listener convention

Used by every `on` / `once` entry point.

| Item | Definition |
|---|---|
| Free function | `fn(payload: T, ctx: *u8)` — on the bus, `T` is `str` |
| Bound method | `fn method(ref this, payload: T)` — receiver becomes `ctx` |
| Default `ctx` | `0 as *u8` when omitted |
| Handle | `on` / `once` return a subscription handle: `SignalSubscription[T]` from a signal, `Subscription` from a bus |
| Handle ownership | the handle owns the registration — dropping it cancels the listener; `detach()` opts out |
| Token | `u64` inside the handle (`handle.id()`), for the raw `off(id)` surface |
| Token `0` | never a live listener; a handle carrying id `0` is inert |
| `ctx` ownership | borrowed for the registration lifetime; not freed by signal/bus |
| Bus event `name` | borrowed `str` for the registration lifetime (not copied, not mutated) |
| Payload on `emit` | borrowed for the call; must not be retained by the listener |

---

## `Signal[T]`

Typed multicast event. One signal value = one event channel. Fields are
private.

### `Signal[T]::new`

```cplus
fn new() -> Signal[T]
```

Empty subscriber list. Next token starts at `1`.

### `on`

```cplus
fn on(ref this, f: fn(T, *u8), ctx: *u8 = 0 as *u8, once: bool = false) -> SignalSubscription[T]
```

Register `f` with `ctx`. Returns the owning subscription handle (inert —
id `0` — only if append failed). With `once: true`, the listener is removed
before its first delivery. The signal must not move while handles to it
are live.

### `once`

```cplus
fn once(ref this, f: fn(T, *u8), ctx: *u8 = 0 as *u8) -> SignalSubscription[T]
```

Same as `on(..., once: true)`.

### `off`

```cplus
fn off(ref this, id: u64) -> bool
```

Remove the listener with this token. Returns `true` if removed. Returns
`false` if `id` is `0`, unknown, or already removed.

### `has`

```cplus
fn has(this, id: u64) -> bool
```

Whether a listener with this token is still registered (paused or not).

### `pause` / `resume`

```cplus
fn pause(ref this, id: u64) -> bool
fn resume(ref this, id: u64) -> bool
```

Mute / unmute the listener with this token. A paused listener stays
registered, keeps its place in the delivery order, and is skipped by
`emit`. Returns `false` if the token is unknown.

### `off_ctx`

```cplus
fn off_ctx(ref this, ctx: *u8) -> usize
```

Remove every listener whose `ctx` equals this pointer. Returns how many
were removed. `ctx == 0` matches only listeners registered without a ctx.

### `emit`

```cplus
fn emit(ref this, value: T)
```

Deliver `value` to every listener registered before this call, in
subscription order (ascending token). Semantics under mid-dispatch
mutation:

- listener **added** during this emit does not run in this emit;
- listener **removed** during this emit does not run after removal;
- listener **paused** is skipped (it keeps its token and its place);
- **`once`** is removed before its body runs; a paused `once` stays
  registered and fires (once) after `resume`;
- nested `emit` on the same signal is an independent pass;
- no listeners → no-op.

Single-threaded. Payload is borrowed for the duration of the call.

### `count`

```cplus
fn count(this) -> usize
```

Number of registered listeners.

### `is_empty`

```cplus
fn is_empty(this) -> bool
```

Whether there are no listeners.

### `remove_all`

```cplus
fn remove_all(ref this)
```

Drop every listener. Previously issued tokens stay invalid; token
numbering is not reset.

---

## `SignalSubscription[T]`

The owning handle for one `Signal[T]` registration: its `drop` cancels the
registration, so the listener lives exactly as long as whoever holds the
handle. The signal must outlive the handle and must not move (the handle
reaches it by address). A handle with id `0` (`none()`, failed `on`, after
`cancel` / `detach`) is inert: every method returns `false`, drop is a
no-op.

### `SignalSubscription[T]::none`

```cplus
fn none() -> SignalSubscription[T]
```

The inert handle — the initial value for fields that subscribe later.

### `id`

```cplus
fn id(this) -> u64
```

The raw token, for the `off(id)` surface.

### `active`

```cplus
fn active(this) -> bool
```

Still registered (paused or not)?

### `cancel`

```cplus
fn cancel(ref this) -> bool
```

Remove the listener and make the handle inert. `false` when already
removed or inert.

### `detach`

```cplus
fn detach(ref this)
```

Keep the registration for the life of the signal, make the handle inert:
its drop no longer cancels. For fire-and-forget listeners.

### `pause` / `resume`

```cplus
fn pause(this) -> bool
fn resume(this) -> bool
```

Mute / unmute the listener without removing it.

### `drop`

Cancels the registration. Runs automatically when the handle's owner
drops; inert handles no-op.

---

## `Bus`

String-keyed multicast bus. Payload type is always `str`. Tokens are unique
across all event names on this bus instance.

### `Bus::new`

```cplus
fn new() -> Bus
```

Independent bus. Next token starts at `1`.

### `on`

```cplus
fn on(ref this, name: str, f: fn(str, *u8), ctx: *u8 = 0 as *u8, once: bool = false) -> Subscription
```

Register `f` for event `name`. Stores `name` by view (borrowed `str`); the
bytes must outlive the registration. Returns the owning subscription
handle (inert — id `0` — only if append failed).

### `once`

```cplus
fn once(ref this, name: str, f: fn(str, *u8), ctx: *u8 = 0 as *u8) -> Subscription
```

Same as `on(..., once: true)`.

### `off`

```cplus
fn off(ref this, id: u64) -> bool
```

Remove the listener with this token (any name). Returns `false` if unknown
or `id == 0`.

### `has`

```cplus
fn has(this, id: u64) -> bool
```

Whether a listener with this token is still registered (paused or not).

### `pause` / `resume`

```cplus
fn pause(ref this, id: u64) -> bool
fn resume(ref this, id: u64) -> bool
```

Mute / unmute the listener with this token. Same semantics as
`Signal.pause` / `Signal.resume`.

### `off_all`

```cplus
fn off_all(ref this, name: str)
```

Remove every listener registered for `name`. Other names are untouched.

### `off_ctx`

```cplus
fn off_ctx(ref this, ctx: *u8) -> usize
```

Remove every listener with this `ctx` across all names. Returns how many
were removed.

### `emit`

```cplus
fn emit(ref this, name: str, payload: str = "")
```

Deliver `payload` to every listener of `name`. Same order and mid-dispatch
rules as `Signal.emit`. Default payload is `""`. Unknown name → no-op.

### `count`

```cplus
fn count(this, name: str) -> usize
```

Number of listeners registered for `name`.

### `remove_all`

```cplus
fn remove_all(ref this)
```

Drop every listener for every name. Issued tokens stay invalid.

---

## `Subscription`

The owning handle for one `Bus` registration — same contract as
`SignalSubscription[T]`, non-generic. Returned by `Bus.on` / `Bus.once`
and the module verbs. The bus must outlive the handle and must not move;
the shared bus always qualifies.

```cplus
fn none() -> Subscription
fn id(this) -> u64
fn active(this) -> bool
fn cancel(ref this) -> bool
fn detach(ref this)
fn pause(this) -> bool
fn resume(this) -> bool
```

`drop` cancels the registration; inert handles no-op.

---

## Shared bus

### `shared`

```cplus
fn shared() -> *Bus
```

Process-wide `Bus`. Lazily initialized on first use. Always the same
pointer.

---

## Module verbs

Pointer-free forwards to `shared()`. Same behavior as the corresponding
`Bus` methods.

### `on`

```cplus
fn on(name: str, f: fn(str, *u8), ctx: *u8 = 0 as *u8, once: bool = false) -> Subscription
```

### `once`

```cplus
fn once(name: str, f: fn(str, *u8), ctx: *u8 = 0 as *u8) -> Subscription
```

### `off`

```cplus
fn off(id: u64) -> bool
```

### `off_all`

```cplus
fn off_all(name: str)
```

### `off_ctx`

```cplus
fn off_ctx(ctx: *u8) -> usize
```

### `emit`

```cplus
fn emit(name: str, payload: str = "")
```

### `count`

```cplus
fn count(name: str) -> usize
```

### `remove_all`

```cplus
fn remove_all()
```

Drop every listener on the shared bus (every name).

---

## Internal types

Not constructed by app code; listed for completeness.

### `Listener[T]`

| Field | Type | Role |
|---|---|---|
| `f` | `fn(T, *u8)` | callback |
| `ctx` | `*u8` (opaque) | identity passed to `f` |
| `id` | `u64` | token |
| `once` | `bool` | auto-remove before first fire |
| `paused` | `bool` | skipped by emit while set |

### `BusListener`

| Field | Type | Role |
|---|---|---|
| `name` | `str` | borrowed event name (registration lifetime) |
| `f` | `fn(str, *u8)` | callback |
| `ctx` | `*u8` (opaque) | identity passed to `f` |
| `id` | `u64` | token |
| `once` | `bool` | auto-remove before first fire |
| `paused` | `bool` | skipped by emit while set |

---

## Package

| | |
|---|---|
| Package name | `events` |
| Module path | `events/events` |
| Dependencies | `stdlib` (`vec`, `option`, `status`) |
| Tests | `cpc test` (root: `src/test_main.cplus`) |
