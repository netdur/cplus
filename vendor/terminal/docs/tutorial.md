# Tutorial

## A shell in a window

`terminal/widget` is the module an application imports. `start` opens a
pseudo-terminal, launches a login shell in it, and returns a widget that owns
both. Keep the widget alive for as long as the terminal should stay
interactive; dropping it hangs the shell up.

```cplus
import "terminal/widget" as terminal;
import "facet/facet" as facet;
import "facet/runtime" as runtime;
import "stdlib/option" as option;

struct Console {
    term: terminal::Widget,
}

impl Console: facet::Component {
    fn build(ref this) -> facet::Node {
        var col: facet::Builder = facet::Builder::new();
        col.add(this.term.node().grow(1.0f64));
        return facet::vstack(col);
    }
}

impl Console: facet::Lifecycle {
    fn on_attach(ref this) {
        let _focused: bool = this.term.focus();
        return;
    }
    fn on_detach(ref this) {
        this.term.stop();
        return;
    }
}

fn main() -> i32 {
    var console: Console = match terminal::start() {
        option::Option[terminal::Widget]::Some(w) => Console { term: w },
        option::Option[terminal::Widget]::None => { return 1; }
    };
    let _final: Console = runtime::run_component(console, title: "console");
    return 0;
}
```

`node()` returns a portable facet node, so the terminal composes with the rest
of a tree: put it in a split, give it a toolbar, size it with `grow` or
`frame`. The node holds its own retain on the native view, but the widget is
what owns the session.

Create, use, and drop the widget on the AppKit main thread.

## Typing

The pane takes keys directly once it holds the window's first responder:
printable characters, Return, Tab, Delete, arrows, Home/End, Page Up/Down,
Escape, and control characters such as `^C`. `⌘C`, `⌘V` and `⌘A` copy, paste
and select all; pasted text goes to the shell rather than into the transcript,
because the view is read-only.

Focus is the application's decision, so it is an explicit call:

```cplus
fn on_attach(ref this) {
    let focused: bool = this.term.focus();   // false: not on screen yet
    return;
}
```

Call `focus()` again at the end of a handler that runs one of the
application's own controls. Clicking a button can take the first responder,
and `has_focus()` reports where it currently is.

## Running a command and finding out how it went

An application that builds and runs a project in its terminal pane needs more
than the ability to type into it: it needs to know when the build finished, and
whether it worked. `run` sends a command; the shell reports back through prompt
and command marks, and the result shows up on the widget.

```cplus
let id: u64 = this.term.run("cpc build");
if id == (0 as u64) {
    // Refused: the pane is stopped, or the previous command is still running.
}
```

Ask when it is done, either by polling or from a callback:

```cplus
fn build_finished(ctx: *u8) {
    let app: *App = { ctx as *App };
    match { (*app).term.exit_code() } {
        option::Option[i32]::Some(0) => { { (*app).run_it(); } }
        option::Option[i32]::Some(_) => { { (*app).show_errors(); } }
        option::Option[i32]::None    => {}      // still running
    }
    return;
}

// once, after start:
this.term.on_command_end(build_finished, #addr_of(this) as *u8);
```

`output()` is that command's own output — what reached the SCREEN, so no prompt,
no echo of the command, and none of the markers a shell writes and then erases.
It is the text a build's errors are in:

```cplus
let errors: text::Text = this.term.output();
```

`interrupt()` is `^C`. `command_state()` is `Idle`, `Running` or `Done`, and
`finished_count()` counts completions, which is the easiest thing to poll.
`cwd()` reports where the shell is after every command.

None of this reads the pane. The terminal does not detect that a command ended;
the SHELL says so, from the hook that runs just before it draws the next prompt,
and the mark arrives before the prompt does. `docs/guide.md` traces one command
byte by byte under "How the terminal knows a command ended", along with what
follows from it — why a REPL correctly reads as still running, and why a program
that ignores `^C` reports nothing.

This rests on shell integration that `terminal/pty` installs into zsh. With a
different shell, `has_integration()` is false and `run` falls back to bracketing
the command itself — still reporting the exit code, but echoing two extra lines
where the user can see them.

## Writing to the shell from code

`send` writes bytes to the pseudo-terminal exactly as a keypress would, with no
ledger involvement. A shell reading a line terminates it on carriage return:

```cplus
this.term.send("ls -la\r");
this.term.send("\x03");        // ^C, a byte on its own
```

## Reading the pane

`text()` snapshots the whole pane — scrollback and the live screen — with
control sequences removed and UTF-8 payload intact. It is ordinary text and can
be logged, searched, or asserted on in a test.

```cplus
let session: text::Text = this.term.text();
let ended: bool = !this.term.is_running();
```

`alternate_screen()` reports whether a full-screen program such as `vim` or
`top` currently has the pane, which is worth asking before sending the shell a
command it will not see.

## Without facet

`terminal/appkit` exposes the same widget with AppKit types for applications
that mount views themselves. `view()` returns the `NSScrollView` as an
`ak::View`, `native_handle()` returns a retained raw handle for
`facet::native`, and `node()` returns a `flex_layout` leaf.

```cplus
import "terminal/appkit" as terminal_ui;

let widget: terminal_ui::Widget = ...;
win.set_content_view(widget.view());
```

## The screen on its own

`terminal/terminal` is the platform-neutral model underneath: a VT screen with
scrollback. It has no dependency on AppKit or on a pseudo-terminal, so it can be
fed captured bytes from anywhere.

```cplus
import "terminal/terminal" as terminal;

var screen: terminal::Screen = terminal::new(rows: 24 as u16, cols: 80 as u16);
screen.feed(chunk_from_a_pipe);
show(screen.view());
```

Give it the size the program thinks it has. A program drawing to a screen of one
size while the model lays it out at another puts every absolute cursor address
on the wrong cell:

```cplus
screen.resize(rows, cols);
```

If the bytes come from a real pseudo-terminal, hand back anything the program
asked for, or a program that queried the cursor position will simply wait:

```cplus
if screen.has_reply() {
    let _w: isize = session.write(screen.reply());
    screen.clear_reply();
}
```
