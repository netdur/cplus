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
for decoupled cross-module messaging:

```cplus
fn open_file(path: str, ctx: *u8) { /* ... */ return; }
let id: u64 = events::on("file:open", open_file);

events::emit("file:open", payload: "src/main.cplus");

let removed: bool = events::off(id);
events::off_all("file:open");
let n: usize = events::count("file:open");
```

`events::once(name, f)` auto-removes after the first delivery.
`Bus::new()` builds an independent bus; `events::shared()` exposes the
app-wide instance.

## Signal[T]

Typed multicast event — compile-checked payload, one value per event:

```cplus
var opened: events::Signal[str] = events::Signal[str]::new();
fn on_opened(path: str, ctx: *u8) { /* ... */ return; }

let id: u64 = opened.on(on_opened);
opened.emit("notes.md");
let removed: bool = opened.off(id);
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
