# fswatch

macOS filesystem watching with typed, owner-thread change events.

```toml
[dependencies]
fswatch = "*"
```

```cplus
import "fswatch/fswatch" as fswatch;
import "stdlib/result" as result;

fn changed(event: fswatch::Change, ctx: *u8) {
    // event.path is borrowed for this callback.
    return;
}

var options: fswatch::Options = fswatch::Options::new();
options.depth(fswatch::WatchDepth::Recursive);
let _a = options.ignore(".git/**");
let _b = options.ignore("*.tmp");

var task: fswatch::WatchTask =
    match fswatch::watch("src", options, changed, deliver: facet::run_on_main) {
        result::Result[fswatch::WatchTask, fswatch::WatchError]::Ok(t) => t,
        result::Result[fswatch::WatchTask, fswatch::WatchError]::Err(e) => { return; }
    };
// The task owns the background thread: `task.stop()` ends it, and dropping
// the task stops it the same way.
```

`watch` validates the root immediately, then polls on its own thread and
hands each change to the callback through `deliver` — any `(work, ctx)`
executor. `facet::run_on_main` lands callbacks on the main thread with the
change's strings copied for the flight; omit `deliver` and callbacks run on
the watcher thread. For a loop you drive yourself, use the low-level
`Watcher` (`Watcher::new` + `on_change` + `poll()` / `run()`).

## Scope

- macOS first, backed by `kqueue` vnode notifications;
- individual file or directory roots;
- shallow immediate-child or recursive nested snapshots;
- glob ignores with ignored-directory pruning;
- created, modified, removed, renamed, metadata, and overflow events;
- synchronous `poll()` and cooperative async `run()` delivery.

Callbacks execute on the thread that drives `poll()` / `run()`. The package
does not call `events::Signal` from a worker thread.

## Ignore patterns

Patterns match slash-normalized paths relative to the watched root.

| Pattern | Meaning |
|---|---|
| `*.tmp` | that basename pattern at any watched depth |
| `.git/**` | the `.git` directory and its complete tree |
| `build/*` | immediate entries below `build` |
| `**/*.swp` | swap files at the root or any nested depth |
| `src/?.cplus` | one-byte filename before `.cplus` |

`*` and `?` do not cross `/`; `**` does. Nothing is ignored by default.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — quick usage
- [docs/guide.md](docs/guide.md) — behavior and design constraints
- [docs/ref.md](docs/ref.md) — API reference

## Tests

```sh
cd vendor/fswatch
../../target/debug/cpc test
```

The package tests exercise real `kqueue` notifications in temporary paths.
The imported stdlib currently has an unrelated sandbox-sensitive TCP bind test;
the fswatch-specific tests are listed under `src::fswatch` and
`src::test_main`.
