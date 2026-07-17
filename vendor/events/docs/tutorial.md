# Tutorial

Quick path: depend, import, subscribe, emit, unsubscribe. Read this when you
want to use the package in minutes. Deeper rationale and gotchas live in
[guide.md](guide.md); signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
events = "*"
```

```cplus
import "events/events" as events;
```

## Typed signal

```cplus
fn on_opened(path: str, ctx: *u8) {
    return;
}

var opened: events::Signal[str] = events::Signal[str]::new();
let id: u64 = opened.on(on_opened);
opened.emit("notes.md");
let removed: bool = opened.off(id);
```

Fire once, then auto-detach:

```cplus
let _one: u64 = opened.once(on_opened);
opened.emit("a.md");   // runs
opened.emit("b.md");   // already gone
```

## Shared bus (string names)

No shared type between modules — only an event name and a `str` payload:

```cplus
fn open_file(path: str, ctx: *u8) {
    return;
}

let id: u64 = events::on("file:open", open_file);
events::emit("file:open", payload: "src/main.cplus");
events::off(id);
events::off_all("file:open");
let n: usize = events::count("file:open");
```

`events::once(name, f)` is the one-shot form. Need an isolated bus (tests,
subsystem)? Use `Bus::new()` with the same methods instead of the module verbs.

## Bound methods

Same shape as facet handlers — the receiver is `ctx`:

```cplus
impl StatusBar {
    fn on_open(ref this, path: str) {
        return;
    }
}

// bar: StatusBar
opened.on(bar.on_open);
events::on("file:open", bar.on_open);
```

## Teardown

```cplus
// drop every listener registered with this receiver
opened.off_ctx(#addr_of(bar) as *u8);
events::off_ctx(#addr_of(bar) as *u8);   // shared bus: all names
```

Keep the receiver alive until you `off` / `off_ctx` / `remove_all`.

## Rules you need on day one

- Listeners run in subscription order.
- Payload is borrowed for the duration of `emit` — do not store it.
- Bus event names are borrowed `str`s (use literals); they must outlive the
  subscription — same rule as `ctx`.
- `off(0)` is always a no-op (`0` is never a live token).
- Single-threaded: emit on the thread that owns the listeners.
