# events

Typed signals and a string-keyed event bus.

```toml
[dependencies]
events = "*"
```

```cplus
import "events/events" as events;
```

## Shared bus (common case)

Pointer-free module verbs on one app-wide bus. String names, `str` payload —
for decoupled cross-module messaging. `on` returns an owning
`Subscription` handle: keep it while you want to listen — dropping it
unsubscribes.

```cplus
fn open_file(path: str, ctx: *u8) { /* ... */ return; }
var sub: events::Subscription = events::on("file:open", open_file);

events::emit("file:open", payload: "src/main.cplus");

let _p: bool = sub.pause();     // muted, keeps its place
let _r: bool = sub.resume();
let _c: bool = sub.cancel();    // removed; the handle is now inert
```

A handle stored in a component's field unsubscribes automatically when the
component drops. For a fire-and-forget listener, `sub.detach()` keeps the
registration alive and makes the handle inert. `events::once(name, f)`
auto-removes after the first delivery. `Bus::new()` builds an independent
bus; `events::shared()` exposes the app-wide instance.

## Signal[T]

Typed multicast event — compile-checked payload, one value per event. Same
handle contract, typed: `SignalSubscription[T]`.

```cplus
var opened: events::Signal[str] = events::Signal[str]::new();
fn on_opened(path: str, ctx: *u8) { /* ... */ return; }

var sub: events::SignalSubscription[str] = opened.on(on_opened);
opened.emit("notes.md");
let _c: bool = sub.cancel();
```

Bound methods work on both layers (`opened.on(bar.on_open)`,
`events::on("file:open", bar.on_open)`).

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — how / why / gotchas
- [docs/ref.md](docs/ref.md) — API manual

## Tests

Unit tests live in `src/test_main.cplus`.

```
cd vendor/events && cpc test
```
