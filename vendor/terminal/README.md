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

## Typing

The pane takes keys directly — printable characters, Return, Tab, Delete,
arrows, Home/End, Page Up/Down, Escape, and control characters such as `^C`.
`⌘C` / `⌘V` / `⌘A` copy, paste and select; paste goes to the shell, never into
the transcript, because the view is read-only.

Keys go to whichever element holds the window's first responder, so an app that
wants a terminal ready to type into says so:

```cplus
fn on_attach(ref this) {
    let focused: bool = this.term.focus();   // false = not on screen yet
    return;
}
```

Call `focus()` again at the end of a handler that runs one of the app's own
controls: clicking a button can take the first responder, and `has_focus()`
reports where it is. A view mounted through the native escape hatch is not
addressable by key, so `facet::find(key)` cannot do this — the widget owns the
verb.

Apps working directly with AppKit or `facet_appkit/ui` can instead import
`terminal/appkit` and use `view()`, `native_handle()`, or the flex `node()`.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — quick usage
- [docs/guide.md](docs/guide.md) — the model, and what it does not claim
- [docs/ref.md](docs/ref.md) — API reference

## Status

macOS is the only PTY/UI backend at the moment. The public session seam is kept to
`start/read/write/resize/close` so Linux (`forkpty`/`epoll`) and Windows
(`ConPTY`) can be added without changing the transcript model.

Run the platform-neutral tests with:

```sh
cd vendor/terminal
cpc test
```
