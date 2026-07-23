# Tutorial

## Watch in one call

`fswatch::watch` owns the thread and the poll loop; you keep the returned
`WatchTask` for as long as the watch should run (dropping it stops the
thread):

```cplus
import "fswatch/fswatch" as fswatch;
import "stdlib/result" as result;

fn changed(event: fswatch::Change, ctx: *u8) {
    return;
}

var options: fswatch::Options = fswatch::Options::new();
var task: fswatch::WatchTask =
    match fswatch::watch("config.json", options, changed, deliver: facet::run_on_main) {
        result::Result[fswatch::WatchTask, fswatch::WatchError]::Ok(t) => t,
        result::Result[fswatch::WatchTask, fswatch::WatchError]::Err(e) => { return; }
    };
```

`deliver` is any `(work, ctx)` executor; `facet::run_on_main` lands the
callback on the main thread (the change's strings are copied for the
flight), and omitting it runs the callback on the watcher thread.
`fswatch::WatchTask::none()` is the inert value for fields that start
watching later; `task.stop()` ends the watch explicitly.

The sections below drive the loop by hand — use them when you already own
a loop or an executor.

## Watch one file

```cplus
import "fswatch/fswatch" as fswatch;
import "events/events" as events;
import "stdlib/result" as result;

fn changed(event: fswatch::Change, ctx: *u8) {
    if event.kind == fswatch::ChangeKind::Modified {
        // Reload event.path here. The string is borrowed for this call.
    }
    return;
}

var options: fswatch::Options = fswatch::Options::new();
var watcher: fswatch::Watcher = match fswatch::Watcher::new("config.json", options) {
    result::Result[fswatch::Watcher, fswatch::WatchError]::Ok(w) => w,
    result::Result[fswatch::Watcher, fswatch::WatchError]::Err(e) => { return; }
};
var sub: events::SignalSubscription[fswatch::Change] = watcher.on_change(changed);

// Call this on each turn of your existing loop.
let delivered = watcher.poll();
```

The file's parent is also watched, so editor-style atomic replacement is
reported even though the original file descriptor refers to the old inode.

## Watch a directory recursively

```cplus
var options: fswatch::Options = fswatch::Options::new();
options.depth(fswatch::WatchDepth::Recursive);
let _a = options.ignore(".git/**");
let _b = options.ignore("target/**");
let _c = options.ignore("*.tmp");

var watcher: fswatch::Watcher = match fswatch::Watcher::new(".", options) {
    result::Result[fswatch::Watcher, fswatch::WatchError]::Ok(w) => w,
    result::Result[fswatch::Watcher, fswatch::WatchError]::Err(e) => { return; }
};
var sub: events::SignalSubscription[fswatch::Change] = watcher.on_change(changed);
```

Use `WatchDepth::Shallow` (the default) when only the root and its immediate
children matter.

## Async pumping

`run()` repeatedly sleeps and drains native notifications without blocking the
owning executor:

```cplus
facet::spawn_ui(this.watcher.run(interval_ms: 50 as u64));
```

The watcher must remain at a stable address while that task is alive. Call
`watcher.stop()` during teardown. A callback may also call `stop()`; it takes
effect before the next iteration.

## Unsubscribe

The handle owns the registration: keep `sub` while you want callbacks,
and dropping it unsubscribes. Explicitly:

```cplus
let _c: bool = sub.cancel();     // unsubscribe now
sub.detach();                    // fire-and-forget: registration outlives the handle
watcher.off_ctx(receiver_address);   // bulk: everything for one receiver
```

Bound methods work because the package uses the same function-plus-context
listener convention as `events` and `facet`. The watcher must not move
while handles to it are live.
