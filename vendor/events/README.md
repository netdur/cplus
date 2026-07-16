# events

Typed signals and a string-keyed event bus.

```toml
[dependencies]
events = "*"
```

```cplus
import "events/events" as events;
```

## Signal[T]

A typed multicast event: any number of listeners, payload checked at compile
time. Use one `Signal` per event, as a component field or a module static.

```cplus
var changed: events::Signal[i64] = events::Signal[i64]::new();

fn on_changed(v: i64, ctx: *u8) { /* ... */ return; }

let id: u64 = changed.on(on_changed);              // subscribe
let _one: u64 = changed.once(on_changed);          // auto-removes after one delivery
changed.emit(42);                                  // deliver to every listener
let removed: bool = changed.off(id);               // unsubscribe by token
```

A listener is a fn pointer plus an opaque `ctx` — the convention facet
handlers use — so a component method binds directly:

```cplus
impl Editor {
    fn on_saved(this, path: str) { /* ... */ return; }
}
// inside a method with `this` in scope:
saved.on(this.on_saved);        // ctx = this, bridged by the compiler
```

`on` returns a token (`u64`). `off(token)` removes by token, so any listener
can be removed — no function identity involved. Token `0` is never issued.

## Bus

The dynamic counterpart: string event names, `str` payload. For decoupled
cross-module messaging where sender and receiver share no type.

```cplus
fn open_file(path: str, ctx: *u8) { /* ... */ return; }

let bus: *events::Bus = events::shared();          // the app-wide instance
let id: u64 = { (*bus).on("file:open", open_file) };
{ (*bus).emit("file:open", payload: "src/main.cplus"); }
```

`Bus::new()` makes an independent instance. `off_all(name)` removes every
listener of one name; `count(name)` reports how many are registered.

## Delivery semantics

- Listeners fire in subscription order.
- A listener added during an emit does not fire in that emit.
- A listener removed during an emit does not fire after its removal.
- A `once` listener is removed before it fires; re-subscribing from inside
  the handler is safe (the new registration waits for the next emit).
- `emit` borrows the payload: listeners read it for the duration of the call
  and must not retain it.
- Nested emits on the same signal complete independently.

Dispatch is single-threaded and allocation-free; each delivery step rescans
the listener list (quadratic in listener count, which is small for events).
Cross-thread delivery composes with `stdlib/channel`: send on the worker,
drain and `emit` on the owning thread.

## Tests

```
cpc test
```
