# terminal

A macOS-first, portable-shaped terminal widget for C+.

The package currently provides:

- `terminal/terminal`: a platform-neutral, bounded ANSI-aware transcript.
- `terminal/pty`: a real macOS pseudo-terminal session backed by `forkpty`.
- `terminal/appkit`: a live AppKit widget with automatic main-queue PTY reads,
  keyboard forwarding, paste, resize propagation, and bounded scrollback.
- `terminal/widget`: the portable facet-facing wrapper applications should
  normally import.

This initial model is intentionally a terminal **transcript**, not a complete
xterm emulator. It is suitable for shells, build output, REPLs, and command
panes. Alternate-screen TUIs, cell-accurate cursor addressing, mouse reporting,
and per-cell 256/true-color rendering require a VT screen engine and are not
claimed yet.

## Core use

```cplus
import "terminal/terminal" as terminal;

var transcript = terminal::Transcript::new(max_bytes: 1024 * 1024);
transcript.feed(bytes_from_a_pty);
show(transcript.view());
```

## AppKit widget

Keep the `Widget` alive for as long as the terminal should remain interactive.
Create, use, and drop it on the AppKit main thread. It owns the shell and shuts
it down on `stop()` or drop.

```cplus
import "terminal/widget" as terminal;

var term = match terminal::start(cwd: project_path) {
    option::Option[terminal::Widget]::Some(w) => w,
    option::Option[terminal::Widget]::None => { /* show an error */ }
};

// Portable facet node. Keep `term` alive beside the mounted tree.
let node = term.node().grow(1.0f64);
```

`term.send(bytes)` writes programmatic input, `term.text()` snapshots
the cleaned transcript, and `term.is_running()` reports whether the PTY is
still active.

Apps working directly with AppKit or `facet_appkit/ui` can instead import
`terminal/appkit` and use `view()`, `native_handle()`, or the flex `node()`.

## Status

macOS is the only PTY/UI backend at the moment. The public session seam is kept to
`start/read/write/resize/close` so Linux (`forkpty`/`epoll`) and Windows
(`ConPTY`) can be added without changing the transcript model.

Run the platform-neutral tests with:

```sh
cd vendor/terminal
cpc test
```
