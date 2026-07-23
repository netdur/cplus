# The Component, the Right Way

Read this before writing a facet component; re-read it before each new one.
Live guideline: `examples/hello_facet` (map at the bottom).

## Model

Think DOM. `build(ref this)` runs once and returns the tree, like a page's
initial HTML. After that, `facet::find(key)` is `getElementById`: get the
live element, mutate it in place. State is the struct's fields, and the
struct IS the component: it lives as long as its owner holds it, so leaving
the tree loses nothing. Detach is not destroy — a parked screen keeps its
fields, keeps listening, keeps updating; only `present` and `nav::go` drop
the instance, and state that must survive those lives in the service. A
handler writes a field, then pushes the new value into the keyed view that
shows it. Nothing redraws on its own.

## Skeleton

State is the fields; the service that keeps it is one of them. A click
handler is a method `(ref this, sender: *u8)`; a bus listener is a method
`(ref this, payload: str)`. Both funnel into one `set_count`, which writes
the field, saves through the service, then pushes the value to the keyed
view (`show_count`).
The component subscribes once, at first attach — never in `new()`: a bound
listener registers the receiver's address, and the value `new` returns
still moves; the address is stable only once the component is mounted. It
keeps listening while off the tree: an event that arrives then still
updates the field and the file — `find` misses, and the next `on_attach`
pushes state back into the views.
`events::on` returns an owning `Subscription` handle; stored in fields, the
handles drop with the component and cancel their registrations — no
teardown code. Any module can drive it with `events::emit("counter:inc")`,
without importing the component. The service pushes through the same
channel: its file watcher emits `"counter:changed"` after a reload, and
`changed_event` syncs the field from the service — it reads and shows,
never saves, or every save would re-trigger itself through the watcher.
The `stdlib/text` import is required by `"${...}"` interpolation; the
owned `Text` it builds passes anywhere `str` is expected. `CounterService`
is defined in the next section.

```cplus
import "facet/facet" as facet;
import "stdlib/text" as text;
import "stdlib/result" as result;
import "events/events" as events;
import "./counter_service" as csvc;

struct Counter {
    n: i32,
    svc: csvc::CounterService,
    sub_inc: events::Subscription,
    sub_decr: events::Subscription,
    sub_changed: events::Subscription,
}

impl Counter {
    fn new() -> Counter {
        return Counter {
            n: 0,
            svc: csvc::CounterService::new(),
            sub_inc: events::Subscription::none(),
            sub_decr: events::Subscription::none(),
            sub_changed: events::Subscription::none(),
        };
    }

    fn show_count(ref this) {
        let _h: facet::Handle = facet::find("counter:count")
         .set_text("count ${this.n}");
        return;
    }

    fn set_count(ref this, n: i32) {
        this.n = n;
        let _r: result::Result[usize, result::IoError] = this.svc.save(this.n);
        this.show_count();
        return;
    }

    fn inc(ref this, sender: *u8) {
        this.set_count(this.n + 1);
        return;
    }

    fn decr(ref this, sender: *u8) {
        this.set_count(this.n - 1);
        return;
    }

    fn inc_event(ref this, payload: str) {
        this.set_count(this.n + 1);
        return;
    }

    fn decr_event(ref this, payload: str) {
        this.set_count(this.n - 1);
        return;
    }

    fn changed_event(ref this, payload: str) {
        this.n = this.svc.count;
        this.show_count();
        return;
    }

    fn loaded(ref this) {
        this.n = this.svc.count;
        this.show_count();
        return;
    }
}

impl Counter: facet::Component {
    fn build(ref this) -> facet::Node {
        var b: facet::Builder = facet::Builder::new();
        b.add(facet::label("count ${this.n}").key("counter:count"));
        b.add(facet::button("+1").on_click(this.inc));
        b.add(facet::button("-1").on_click(this.decr));
        return facet::column(b).padding(24.0f64).gap(8.0f64);
    }
}

impl Counter: facet::Lifecycle {
    fn on_attach(ref this) {
        if !this.sub_inc.active() {
            this.sub_inc = events::on("counter:inc", this.inc_event);
            this.sub_decr = events::on("counter:decr", this.decr_event);
            this.sub_changed = events::on("counter:changed", this.changed_event);
            this.svc.watch();
            this.svc.load_async(this.loaded);
        }
        this.show_count();
        return;
    }
    fn on_detach(ref this) { return; }
}
```

The same `build` written in the `@facet` DSL. A component has exactly one
`build`; the two forms appear here side by side only for documentation.

```cplus
    fn build(ref this) -> facet::Node {
        return @facet {
            column {
                label("count ${this.n}").key("counter:count")
                button("+1").on_click(this.inc)
                button("-1").on_click(this.decr)
            }
            .padding(24.0f64)
            .gap(8.0f64)
        };
    }
```

## Service

Anything that is not UI lives in a service, in its own file — here, keeping
the count on disk. A service conforms to `facet::Service`, and that
interface is the threading contract: `produce` is the slow read and, under
`load_async`, runs on a worker thread where it must not touch state the UI
reads; `apply` installs the produced result and runs on the main thread.
`load()` is the synchronous path, `produce` then `apply` in place — for
tests and command-line tools only. UI code always loads through
`load_async`: nothing may hold the main thread, even slightly, and that
includes a small local file. `new()` constructs; `on_attach` loads. A
missing or unreadable file loads as 0: a fresh counter. Persistence is one call each
way: `fs::read_to_string` in, `fs::write_string` out — `save` forwards the
call's `Result` untouched. `guard let` unwraps or bails early, as in
`produce`; use `match` when both arms matter, as in `watch()`.

The service can also push. `watch()` is one `fswatch::watch` call: the
package owns the polling thread, and `deliver: facet::run_on_main` lands
`file_changed` on the main thread, where it reloads and emits
`"counter:changed"` on the shared bus — the component's `changed_event`
picks it up. Editing the file externally updates the window. The returned
`WatchTask` is an owning handle kept as a service field: when the
component drops, the service drops, the task drops, and the watcher thread
stops. The component's own saves re-trigger the watcher too; the cycle
stops because `changed_event` only reads. `file_changed` is a bound
method, like every other listener in this document: passing
`this.file_changed` registers the service's address as the callback
context, which is why `watch()` runs from `on_attach` (inside the
subscribe-once guard) — the same address-stability rule as the
subscriptions.

```cplus
import "stdlib/text" as text;
import "stdlib/fs" as fs;
import "stdlib/result" as result;
import "stdlib/option" as option;
import "facet/facet" as facet;
import "events/events" as events;
import "fswatch/fswatch" as fswatch;

extern fn atoi(s: *u8) -> i32;

struct CounterService {
    path: str,
    staged: i32,
    count: i32,
    task: fswatch::WatchTask,
}

impl CounterService {
    fn new() -> CounterService {
        return CounterService {
            path: "/tmp/counter_state.txt",
            staged: 0,
            count: 0,
            task: fswatch::WatchTask::none(),
        };
    }

    fn load(ref this) {
        this.produce();
        this.apply();
        return;
    }

    fn load_async(ref this, on_ready: fn(*u8), ctx: *u8 = 0 as *u8) {
        facet::load_service(this, on_ready, ctx: ctx);
        return;
    }

    fn save(ref this, n: i32) -> result::Result[usize, result::IoError] {
        this.count = n;
        return fs::write_string(this.path, "${n}");
    }

    fn watch(ref this) {
        if this.task.active() { return; }
        if !fs::exists(this.path) {
            let _r: result::Result[usize, result::IoError] = this.save(this.count);
        }
        var opts: fswatch::Options = fswatch::Options::new();
        match fswatch::watch(this.path, opts, this.file_changed, deliver: facet::run_on_main) {
            result::Result[fswatch::WatchTask, fswatch::WatchError]::Ok(t) => { this.task = t; }
            result::Result[fswatch::WatchTask, fswatch::WatchError]::Err(e) => { }
        }
        return;
    }

    fn file_changed(ref this, event: fswatch::Change) {
        this.load();
        events::emit("counter:changed");
        return;
    }
}

impl CounterService: facet::Service {
    fn produce(ref this) {
        this.staged = 0;
        guard let result::Result[text::Text, result::IoError]::Ok(t) =
            fs::read_to_string(this.path) else { return; };
        guard let option::Option[text::CString]::Some(cp) = t.c_str() else { return; };
        this.staged = { atoi(cp.as_ptr()) };
        return;
    }

    fn apply(ref this) {
        this.count = this.staged;
        return;
    }
}
```

## Rules

- `build()` runs once. Refresh = `find(key)` + mutate. Never rebuild.
- Detach is not destroy. Fields persist while parked; the service and its
  file persist across destroy. Never copy state out of a component "to
  keep it safe" — hold the component.
- `.key("...")` every view you will touch later. Namespace keys as
  `component:name` (`"counter:count"`); when instances can coexist, scope
  with instance data: `"${this.title}:list"`.
- Handlers are methods bound on `this`: `.on_click(this.inc)`. No free
  functions holding context, no module statics.
- `on_attach` / `on_detach` go in `impl X: facet::Lifecycle`. In the
  inherent impl they never fire.
- Subscribe once, in the first `on_attach`, and store the returned
  `Subscription` handles in fields — they drop with the component and
  cancel themselves; never discard one (a dropped handle unsubscribes
  immediately). Push state to the views in every `on_attach`. When a
  hidden component must not react at all, `pause()` the handles in
  `on_detach` and `resume()` them in `on_attach`.
- A service is a field, created in `new()`, not a local in `build()`.
- Write the field first, then update the view. Fields are the truth.
- Mutators return `Status`, reads return `Option`, a missing key returns a
  not-found `Handle`: check `found()`. Nothing panics.

## Slow work

- ALL loading is slow work. A component never reads a data source on the
  main thread — not a database, not the network, not a small file. `new()`
  constructs, `build()` renders the empty shell, `on_attach` loads.
- `build()` returns now: shell, a "Loading…" status, an empty keyed
  container. Then `svc.load_async(this.ready)` from `on_attach`.
- Threading belongs to the service: `facet::load_service` runs `produce` on
  a worker thread, then `apply` and the callback on the main thread. The
  component callback is pure UI.
- Guard late delivery: `!find(key).found()` means the screen detached;
  return. The data stays in the service.
- On re-attach, fill from the service if its data already landed; only
  otherwise `load_async`.
- Never block or sleep on the main thread.

## Screens

- Screen = component + `impl X: facet::Screen { fn chrome(this) -> facet::Chrome }`.
- `present(c)` shows a NEW screen; the old one detaches and drops.
- `switch_to(c)` cycles long-lived siblings (tabs, pagers): built once on
  first visit, then parked and re-attached; scroll and input survive.
- An outlet is an empty keyed container: `facet::find("shell:outlet").switch_to(child)`.
- From handlers: `nav::push(route, arg:)`, `nav::go(route)`, `nav::quit()`.

## Never

- Never re-call `build` to refresh. No layer redraws for you.
- No hidden reactive flow: no observed state, no auto-updating bindings.
  Every update is an explicit call you write.
- No module statics for state.
- No blocking the main thread; slow work lives in services.
- No panics: `Status` / `Option` / `Result` only.

## Live examples

| Pattern | File |
| --- | --- |
| ALL of the above in one app (state while parked, unread badges, fswatch transport) | `examples/inbox/` |
| Parked siblings holding state, self-wiring channels | `examples/inbox/src/screens/channel.cplus` |
| Fixed-set owner, outlet, wiring at first attach | `examples/inbox/src/screens/shell.cplus` |
| Smallest full component | `examples/hello_facet/src/counter.cplus` |
| Service as field, cache on re-attach, `found()` guard | `examples/hello_facet/src/screens/screen_x.cplus` |
| Outlet, `switch_to` pager, nav verbs, `Screen`/`Chrome` | `examples/hello_facet/src/screens/screen_y.cplus` |
| `facet::Service` conformance (`produce`/`apply`, `load_async`) | `examples/hello_facet/src/services/strings_service.cplus` |
| `Signal[T]` + `Bus` delivery semantics, subscription handles | `vendor/events/src/test_main.cplus` |
| File watching (typed change events, `poll`/`run`) | `vendor/fswatch/docs/tutorial.md` |
| Composition into panels | `examples/hello_facet/src/screens/workspace/` |
