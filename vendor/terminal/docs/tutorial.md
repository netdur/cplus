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

## Writing to the shell from code

`send` writes bytes to the pseudo-terminal exactly as a keypress would. A
shell reading a line terminates it on carriage return:

```cplus
this.term.send("ls -la\r");
this.term.send("\x03");        // ^C, a byte on its own
```

## Reading the transcript

`text()` snapshots the cleaned transcript: control sequences removed, UTF-8
payload intact. It is ordinary text and can be logged, searched, or asserted
on in a test.

```cplus
let session: text::Text = this.term.text();
let ended: bool = !this.term.is_running();
```

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

## The transcript on its own

`terminal/terminal` is the platform-neutral model underneath. It has no
dependency on AppKit or on a pseudo-terminal, so it can be fed captured bytes
from anywhere:

```cplus
import "terminal/terminal" as terminal;

var transcript: terminal::Transcript = terminal::Transcript::new(max_bytes: 1048576 as usize);
transcript.feed(chunk_from_a_pipe);
show(transcript.view());
```
