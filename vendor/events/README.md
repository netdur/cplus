# events

Typed signals and a string-keyed event bus.

```toml
[dependencies]
events = "*"
```

```cplus
import "events/events" as events;
```

## The shared bus (module verbs)

The common case is pointer-free: module verbs forward to one app-wide bus.
String event names, `str` payload — for decoupled cross-module messaging
where sender and receiver share no type and no import.

```cplus
// receiver module
fn open_file(path: str, ctx: *u8) { /* ... */ return; }
let id: u64 = events::on("file:open", open_file);

// sender module (doesn't import the receiver)
events::emit("file:open", payload: "src/main.cplus");

let removed: bool = events::off(id);       // unsubscribe by token
events::off_all("file:open");              // drop one event's listeners
let n: usize = events::count("file:open");
```

`events::once(name, f)` auto-removes after the first delivery.
`events::shared() -> *Bus` exposes the instance itself for advanced use;
`Bus::new()` makes an independent bus with the same methods.

## Signal[T]

A typed multicast event: any number of listeners, payload checked at compile
time. Use one `Signal` per event, as a component field or a module static.

```cplus
var opened: events::Signal[str] = events::Signal[str]::new();

fn on_opened(path: str, ctx: *u8) { /* ... */ return; }

let id: u64 = opened.on(on_opened);           // subscribe
let _one: u64 = opened.once(on_opened);       // auto-removes after one delivery
opened.emit("notes.md");                      // deliver to every listener
let removed: bool = opened.off(id);           // unsubscribe by token
```

## Listeners

A listener is a fn pointer plus an opaque `ctx` — the convention facet
handlers use. A component method binds directly; the compiler wires `this`
as the ctx:

```cplus
impl StatusBar {
    fn on_open(ref this, path: str) { /* ... */ return; }
}
// with a StatusBar in scope:
opened.on(bar.on_open);                  // typed signal
events::on("file:open", bar.on_open);    // shared bus — same idiom
```

A free-fn listener takes the payload plus the ctx it registered with:
`fn f(payload: str, ctx: *u8)`. When no identity is needed, leave ctx off
at registration and ignore the parameter.

## Ownership, lifetimes, leaks

The package is designed so registration cannot leak:

- `ctx` is borrowed identity, never owned: the signal stores the pointer and
  hands it back to the listener; it never allocates for it and never frees
  it. Registering allocates only the listener record itself, which
  `off` / `off_all` / `off_ctx` / `remove_all` (and the owner's drop) free.
- Payloads are borrowed for the duration of `emit` — nothing to free after,
  nothing retained.
- Tokens are plain `u64`s; losing one strands nothing (the listener is still
  reachable through `off_ctx`, `off_all`, or `remove_all`). Token `0` is
  never issued; `off(0)` is a false no-op.

The one rule that remains: **a listener's receiver must outlive its
registration**. A bound method's ctx is the receiver's address; if the
receiver goes away while registered, the next emit writes freed memory.
Keep receivers long-lived (module statics or retained component fields —
the facet pattern), and detach in teardown:

```cplus
let removed: usize = opened.off_ctx(#addr_of(STATUS_BAR) as *u8);
```

`off_ctx` removes every listener registered with that identity in one call
(on the shared bus: across all event names).

## Delivery semantics

- Listeners fire in subscription order.
- A listener added during an emit does not fire in that emit.
- A listener removed during an emit does not fire after its removal.
- A `once` listener is removed before it fires; re-subscribing from inside
  the handler is safe (the new registration waits for the next emit).
- Nested emits on the same signal complete independently.

Dispatch is single-threaded and allocation-free; each delivery step rescans
the listener list (quadratic in listener count, which is small for events).
Cross-thread delivery composes with `stdlib/channel`: send on the worker,
drain and `emit` on the owning thread.

## Tests

```
cpc test
```
