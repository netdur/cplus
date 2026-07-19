# Services

> Entry path: [tutorial.md](tutorial.md) · [guide.md](guide.md) · [ref.md](ref.md)

A service is a plain struct that owns some data and sources it — from a file,
a database, the network. Screens hold a service and read it through its
accessors; the service is the single place the data lives.

The slow part must never run on the main thread. The `Service` interface is
that threading contract, and `load_service` is the whole pipeline:

```cplus
interface Service {
    fn produce(ref this);   // the slow read — runs on a worker thread
    fn apply(ref this);     // install the result — runs on the main thread
}

fn load_service[S: Service](ref svc: S,
                            on_ready: fn(*u8) = service_ready_noop,
                            ctx: *u8 = 0) 
```

`load_service` returns immediately. It runs `produce` on a worker thread,
then hops to the main thread and runs `apply` followed by `on_ready(ctx)`.
Because `apply` and every reader run on the main thread, the service's
fields are never observed half-written. Rules:

- `produce` must not touch anything the UI reads (do the slow work; stage
  results in fields the UI never looks at, or produce them into locals the
  `apply` step installs).
- The service must **outlive the flight** — it is reached through its
  address. A service owned by a screen that lives for the window satisfies
  this.
- Without a backend installed there is no main thread to hop to; the result
  is never applied.

## A service

```cplus
struct StringsService {
    delay_ms: i64,
    items: vec::Vec[text::Text],
}

impl StringsService: facet::Service {
    fn produce(ref this) {
        block_ms(this.delay_ms);            // the slow read
        return;
    }
    fn apply(ref this) {
        this.items.remove_all();            // install, on the main thread
        this.items.append(text::from_str("alpha"));
        return;
    }
}

impl StringsService {
    fn load(ref this) { this.produce(); this.apply(); return; }   // blocking form
    fn load_async(ref this, on_ready: fn(*u8), ctx: *u8 = 0 as *u8) {
        facet::load_service(this, on_ready, ctx: ctx);
        return;
    }
    fn count(ref this) -> usize { return this.items.count(); }
    fn at(ref this, i: usize) -> option::Option[*text::Text] { return this.items.at_ptr(i); }
}
```

## A screen consuming it

The screen kicks the load in `on_attach` and fills its UI by key when the
data lands. `on_ready` is a bound method: the adjacent `ctx` parameter takes
the receiver automatically.

```cplus
struct ScreenX { title: str, svc: StringsService }

impl ScreenX: facet::Lifecycle {
    fn on_attach(ref this) {
        if this.svc.count() > (0 as usize) {   // the service HOLDS its data:
            this.strings_ready();              // a revisit fills instantly
            return;
        }
        this.svc.load_async(this.strings_ready);
        return;
    }
    fn on_detach(ref this) { return; }
}

impl ScreenX {
    fn strings_ready(ref this) {
        let list: facet::Handle = facet::find("${this.title}:list");
        if !list.found() { return; }           // delivered after navigating away
        var i: usize = 0;
        while i < this.svc.count() {
            match this.svc.at(i) {
                option::Option[*text::Text]::Some(p) => {
                    let _h: facet::Handle = list.add_child(facet::label({ (*p).view() }));
                }
                option::Option[*text::Text]::None => { }
            };
            i = i + 1;
        }
        return;
    }
}
```

Two properties fall out of the shape:

- **Revisits are instant.** The data survives in the service, so a screen
  shown again fills from memory instead of re-reading.
- **Stale deliveries are harmless.** A load finishing after the user
  navigated away still applies to the service (data kept for next time), but
  the UI writes go through the departed screen's namespaced keys, which
  miss — and a verb on a missing handle no-ops. `found()` makes the miss
  explicit when you want to log or skip early.

## `run_on_main`

```cplus
fn run_on_main(work: fn(*u8), ctx: *u8)
```

The primitive underneath `load_service`: schedule `work(ctx)` on the main
thread, non-blocking. Available directly for custom threading; the same
ownership rule applies — without a backend the work is dropped, so a ctx
that carries ownership must be reclaimed by `work`, never by the scheduler.

## Async tasks: `spawn_ui`

```cplus
fn spawn_ui[T: Send](take f: future::Future[T])
```

For time- and I/O-shaped logic there is a second tool: hand an async task to
the UI. The task's body runs eagerly to its first suspension at the call
site; the rest resumes on the main thread as its awaits complete — timers
(`time::sleep`) and async fd I/O wake it through the backend's reactor
watch, awaited async fns through the executor. After any await the tree may
have changed; the usual discipline applies (namespaced keys miss harmlessly,
`attached(this)` / `found()` to know).

```cplus
fn on_click(ref this, sender: *u8) { facet::spawn_ui(this.refresh()); }

async fn refresh(ref this) {
    await time::sleep(milliseconds: 150 as u64);
    if !facet::attached(this) { return; }
    facet::find("status").set_text("refreshed");
    return;
}
```

Never `block_on` in a handler — it blocks the main thread for the whole
drive, which is the freeze services and `spawn_ui` both exist to avoid.
There is no cancellation; a task in flight at teardown finds its keys gone
and falls through.

For BLOCKING work inside an async task there is the worker bridge:

```cplus
async fn on_worker[I: Send, O: Send](take input: I, f: fn(take I) -> O) -> O
```

`await on_worker(input, f)` runs `f(input)` on a worker thread and resumes
the awaiting task with the result (the values cross boxed, over a pipe the
reactor waits on). A service can expose its read this way instead of — or
alongside — `load_async`; `Service`'s produce/apply remains the plain
callback-shaped story.
