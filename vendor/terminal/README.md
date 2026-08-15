# terminal

A macOS-first, portable-shaped terminal widget for C+.

The package provides:

- `terminal/terminal`: a platform-neutral VT screen with scrollback.
- `terminal/pty`: a real macOS pseudo-terminal session backed by `forkpty`,
  with shell integration installed into zsh.
- `terminal/appkit`: a live AppKit widget with automatic main-queue PTY reads,
  keyboard forwarding, paste, resize propagation, and bounded scrollback.
- `terminal/widget`: the portable facet-facing wrapper applications should
  normally import.

The model is a grid of cells addressed by row and column, so `clear`, the
alternate screen, scroll regions and absolute cursor addressing all work —
`top`, `vim` and `less` take the pane and give it back. Per-cell colour and
attributes are not modelled: SGR is parsed and dropped, so a renderer draws one
uniform run of text. Mouse reporting and sixel are not claimed.

## Core use

```cplus
import "terminal/terminal" as terminal;

var screen = terminal::new(rows: 24 as u16, cols: 80 as u16);
screen.feed(bytes_from_a_pty);
show(screen.view());
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

`term.send(bytes)` writes programmatic input, `term.text()` snapshots the pane,
and `term.is_running()` reports whether the PTY is still active.

## Running commands

An application that builds and runs a project in its pane has to know when the
build finished and whether it worked. A terminal cannot infer either from the
byte stream — the shell is the only party that knows — so `terminal/pty`
installs zsh hooks that report it, and the widget surfaces the result.

```cplus
let id: u64 = term.run("cpc build");     // 0 = refused: stopped, or still busy

match term.exit_code() {
    option::Option[i32]::Some(0) => { /* built */ }
    option::Option[i32]::Some(_) => { show(term.output()); }
    option::Option[i32]::None    => { /* still running */ }
}
```

`output()` is that command's own output as it reached the SCREEN — no prompt, no
echo of the command, and none of the markers a shell writes and then erases.
`on_command_end(handler, ctx)` fires on the main thread once per completion;
`command_state()`, `finished_count()`, `command_line()` and `cwd()` are the
pollable form. `interrupt()` is `^C`.

With a shell other than zsh, `has_integration()` is false and `run` falls back
to bracketing the command with marks of its own — still reporting the exit code,
at the cost of two echoed `printf` lines.

## Typing

The pane takes keys directly — printable characters, Return, Tab, Delete,
arrows, Home/End, Page Up/Down, Escape, and control characters such as `^C`.
Arrows switch to the application form (`ESC O A`) when a full-screen program
asks for it, and paste is bracketed when one asks for that. `⌘C` / `⌘V` / `⌘A`
copy, paste and select; paste goes to the shell, never into the view, because
the view is read-only.

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

## Shell history

Off by default: an app's pane must not write the user's global shell history,
which macOS caps at 1000 entries. `save_history: true` opts into the user's real
history. Either way the shell loads the user's own rc files.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — quick usage
- [docs/guide.md](docs/guide.md) — the model, and what it does not claim
- [docs/ref.md](docs/ref.md) — API reference

## Status

macOS is the only PTY/UI backend at the moment. The public session seam is kept
to `start/read/write/resize/close/poll_exit` so Linux (`forkpty`/`epoll`) and
Windows (`ConPTY`) can be added without changing the screen model.

Run the tests with:

```sh
cd vendor/terminal
cpc test
```
