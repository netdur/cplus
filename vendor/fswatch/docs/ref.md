# Reference

Import:

```cplus
import "fswatch/fswatch" as fswatch;
```

## `WatchDepth`

```cplus
enum WatchDepth { Shallow, Recursive }
```

`Shallow` is the default and includes immediate children. `Recursive` includes
every non-ignored descendant.

## `ChangeKind`

```cplus
enum ChangeKind {
    Created, Modified, Removed, Renamed, Metadata, Overflow
}
```

## `Change`

```cplus
struct Change {
    kind: ChangeKind,
    path: str,
    relative_path: str,
    previous_path: str,
    previous_relative_path: str,
    is_directory: bool,
}
```

All strings are borrowed for callback duration. Previous paths are non-empty
only for `Renamed`.

## `WatchError`

```cplus
enum WatchError {
    NotFound,
    PermissionDenied,
    InvalidPath,
    OutOfMemory,
    Backend,
    Busy,
}
```

## `Options`

### `Options::new`

```cplus
fn new() -> Options
```

Creates shallow options with no ignores.

### `depth`

```cplus
fn depth(ref this, value: WatchDepth)
```

### `ignore`

```cplus
fn ignore(ref this, pattern: str) -> Status
```

Copies and stores a non-empty glob. Empty patterns return `InvalidInput`; an
allocation failure returns `OutOfMemory`.

## `Watcher`

### `Watcher::new`

```cplus
fn new(path: str, take options: Options) -> Result[Watcher, WatchError]
```

Copies `path`, creates the initial filtered snapshot, opens a private kqueue,
and registers the visible nodes. The target must exist initially.

### `watch`

```cplus
fn watch(path: str) -> Result[Watcher, WatchError]
```

Convenience constructor using shallow/no-ignore options.

### `root`

```cplus
fn root(this) -> str
```

Borrowed normalized root path, valid while the watcher lives.

### `on_change` / `once`

```cplus
fn on_change(ref this, f: fn(Change, *u8), ctx: *u8 = 0 as *u8,
             once: bool = false) -> events::SignalSubscription[Change]
fn once(ref this, f: fn(Change, *u8), ctx: *u8 = 0 as *u8) -> events::SignalSubscription[Change]
```

Return the owning subscription handle (see the events package): dropping
it unsubscribes; `detach()` opts out. An inert handle (id `0`) indicates
allocation failure. The watcher must not move while handles are live.

### `off` / `off_ctx`

```cplus
fn off(ref this, id: u64) -> bool
fn off_ctx(ref this, ctx: *u8) -> usize
```

### Error listeners

```cplus
fn on_error(ref this, f: fn(WatchError, *u8), ctx: *u8 = 0 as *u8) -> events::SignalSubscription[WatchError]
fn off_error(ref this, id: u64) -> bool
fn off_error_ctx(ref this, ctx: *u8) -> usize
```

The async pump emits an error and stops if `poll()` fails.

### `poll`

```cplus
fn poll(ref this) -> Result[usize, WatchError]
```

Non-blockingly drains current native records. When records exist, rescans once,
emits the normalized diff, refreshes registrations, and returns the number of
change events emitted. An empty queue returns `Ok(0)`.

### `run`

```cplus
async fn run(ref this, interval_ms: u64 = 50)
```

Cooperative owner-thread pump. A zero interval is clamped to one millisecond.
Only one run may be active.

### `stop` / `is_running`

```cplus
fn stop(ref this)
fn is_running(this) -> bool
```

`stop` affects `run`; it does not disable manual polling.

---

## `watch`

```cplus
fn watch(path: str, take options: Options, f: fn(Change, *u8), ctx: *u8 = 0 as *u8,
         deliver: fn(fn(*u8), *u8) = deliver_inline, interval_ms: u64 = 100 as u64)
    -> result::Result[WatchTask, WatchError]
```

One-call background watching. Validates `path` NOW (errors return before
any thread exists), then owns a thread that polls a `Watcher` every
`interval_ms` and hands each change to `f` through `deliver`, an executor
of the `(work, ctx)` shape. With `facet::run_on_main` as `deliver`,
callbacks land on the main thread; the change's strings are copied for the
flight and freed after the callback returns. The default executor runs the
callback on the watcher thread. Poll errors inside the thread are dropped —
use the low-level `Watcher` when error observation matters.

## `WatchTask`

The owner of one background watch. Dropping the task stops the thread.

```cplus
fn none() -> WatchTask          // inert value for fields that watch later
fn active(this) -> bool
fn stop(ref this)               // idempotent; ends the thread after its current sleep
```

`stop` (and drop) do not flush deliveries already queued with the executor.
