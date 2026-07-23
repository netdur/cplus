# Tutorial

Quick path: depend, import, subscribe, emit, let go. Read this when you
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

`on` returns an owning subscription handle. The listener lives as long as
the handle: dropping it unsubscribes, `cancel()` unsubscribes now.

```cplus
fn on_opened(path: str, ctx: *u8) {
    return;
}

var opened: events::Signal[str] = events::Signal[str]::new();
var sub: events::SignalSubscription[str] = opened.on(on_opened);
opened.emit("notes.md");
let _c: bool = sub.cancel();
```

Fire once, then auto-detach (the handle is detached — the registration
must survive the current scope):

```cplus
var one: events::SignalSubscription[str] = opened.once(on_opened);
one.detach();
opened.emit("a.md");   // runs
opened.emit("b.md");   // already gone
```

## Shared bus (string names)

No shared type between modules — only an event name and a `str` payload.
Same handle contract, non-generic `Subscription`:

```cplus
fn open_file(path: str, ctx: *u8) {
    return;
}

var sub: events::Subscription = events::on("file:open", open_file);
events::emit("file:open", payload: "src/main.cplus");
let _c: bool = sub.cancel();
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
var s1: events::SignalSubscription[str] = opened.on(bar.on_open);
var s2: events::Subscription = events::on("file:open", bar.on_open);
```

## Pause and resume

A paused listener stays registered, keeps its place in the delivery
order, and is skipped by emit:

```cplus
let _p: bool = s2.pause();
events::emit("file:open", payload: "ignored.md");   // s2 skipped
let _r: bool = s2.resume();
```

## Teardown

Store the handles in the receiver's fields and teardown is automatic: when
the receiver drops, its handles drop, and each drop cancels its
registration. No explicit unsubscribe code.

```cplus
struct StatusBar {
    sub_open: events::Subscription,   // starts events::Subscription::none()
}
```

Escape hatches, in order of preference: `cancel()` (unsubscribe now),
`detach()` (fire-and-forget: registration outlives the handle),
`off_all(name)` / `off_ctx(ctx)` / `remove_all()` (bulk).

## Rules you need on day one

- KEEP the handle `on` returns. A discarded handle drops immediately —
  and unsubscribes with it.
- Listeners run in subscription order; paused listeners are skipped but
  keep their place.
- Payload is borrowed for the duration of `emit` — do not store it.
- Bus event names are borrowed `str`s (use literals); they must outlive the
  subscription — same rule as `ctx`.
- The signal/bus must outlive its handles (the shared bus always does).
- Single-threaded: emit on the thread that owns the listeners.
