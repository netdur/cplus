# Reference

Four modules:

```cplus
import "terminal/widget" as terminal;      // portable facet-facing widget
import "terminal/appkit" as terminal_ui;   // the same widget in AppKit types
import "terminal/terminal" as terminal;    // platform-neutral VT screen
import "terminal/pty" as pty;              // pseudo-terminal session
```

Applications normally import `terminal/widget`.

## `terminal/widget`

### `supported`

```cplus
fn supported() -> bool
```

Whether a terminal backend exists on this platform.

### `start`

```cplus
fn start(
    shell: str = "/bin/zsh",
    cwd: str = "",
    max_scrollback: usize = 1048576 as usize,
    font_size: f64 = 13.0f64,
    save_history: bool = false,
) -> option::Option[Widget]
```

Opens a pseudo-terminal, launches `shell` in it as a login shell, and builds the
view. An empty `cwd` inherits the process's working directory. `max_scrollback`
bounds retained history and is raised to a floor of 1024. `save_history: false`
keeps the pane out of the user's global shell history. `None` means the
pseudo-terminal, the dispatch source, or the allocation failed; nothing is left
running.

Call on the AppKit main thread.

### `Widget`

```cplus
struct Widget
```

Owns the session and the view. Keep it alive for as long as the terminal should
stay interactive; dropping it stops the shell.

#### `Widget::node`

```cplus
fn node(this, key: str = "") -> facet::Node
```

The terminal as one portable facet node, sized with `grow`, `frame` and the rest
of the node verbs. The node holds its own retain on the native view; the widget
still owns the session.

#### `Widget::send`

```cplus
fn send(this, bytes: str)
```

Writes bytes to the pseudo-terminal as a keypress would. A shell terminates a
line on `\r`. No-op once the session has been stopped. For running a command and
finding out how it ended, use `run` instead.

#### `Widget::focus`, `Widget::has_focus`

```cplus
fn focus(this) -> bool
fn has_focus(this) -> bool
```

`focus` makes the terminal the window's first responder, so typing goes to the
shell. `false` means the widget is not on screen yet, so there is no window to
hold a first responder: call it from `on_attach` or a handler, not from `build`.
Call it again at the end of a handler that runs one of the application's own
controls; clicking a button can take the first responder.

#### `Widget::text`

```cplus
fn text(this) -> text::Text
```

A snapshot of the whole pane: scrollback followed by the live screen, control
traffic removed, UTF-8 payload intact, trailing blanks on each row excluded.

#### `Widget::is_running`, `Widget::stop`

```cplus
fn is_running(this) -> bool
fn stop(this)
```

`stop` cancels the read source and closes the session. Idempotent. Dropping the
widget does the same; `stop` exists so an application can put shutdown at a named
seam, such as `on_detach`.

### Running commands

A terminal that can only be typed into is not enough for an application that
runs builds in one: it has to know when the build finished and whether it
worked. The shell reports both through prompt and command marks that
`terminal/pty` installs, and these are where they surface.

```cplus
let id: u64 = term.run("cpc build");
// ...later, from on_command_end or a poll:
match term.exit_code() {
    option::Option[i32]::Some(0) => { /* built */ }
    option::Option[i32]::Some(_) => { show(term.output()); }
    option::Option[i32]::None    => { /* still running */ }
}
```

#### `Widget::run`

```cplus
fn run(this, command: str) -> u64
```

Runs `command`. Returns the ledger id it will be given, or `0` if it was not
sent — a stopped session, or one whose previous command is still running.

Refusing while busy is what makes the returned id meaningful: ids are handed out
when the SHELL reports a command starting, which has not happened yet when `run`
returns, so "the next id" is only a sound promise if nothing else can claim it
first.

With no shell integration the command is bracketed with marks of `run`'s own, as
separate lines. It works in any POSIX shell; the two `printf` lines are echoed
where the user can see them.

#### `Widget::interrupt`

```cplus
fn interrupt(this)
```

Interrupts whatever is running, as `^C` does.

#### `Widget::clear`

```cplus
fn clear(this)
```

Empties the pane — the visible screen and the scrollback both — without sending
the shell anything at all.

An app that runs builds in a pane calls it before the first command of a run, so
what the user reads afterwards is that run and nothing older. That is why it
does not go through the shell: running `clear` arrives too late to spare the
command line it was typed on and wipes that as well, leaving a pane whose output
nobody can attribute to a command.

A command that is still running is NOT stopped, and its screen is emptied under
it — which is wrong for a full-screen program the user is in the middle of. Ask
`command_state` first when the pane is one a user can type into.

#### `Widget::command_state`

```cplus
fn command_state(this) -> terminal::CommandState
```

```cplus
enum CommandState { Idle, Running, Done }
```

`Idle` at a prompt, `Running` between the start and finish marks, `Done` for the
instant between a command finishing and the next prompt.

#### `Widget::exit_code`

```cplus
fn exit_code(this) -> option::Option[i32]
```

The exit status of the last command that finished. `None` before any has, and
while a NEW one is running — but not merely because the shell has drawn its next
prompt. A command killed by a signal reports `128 + signal`, the number the
shell itself would report.

#### `Widget::finished_count`

```cplus
fn finished_count(this) -> u64
```

Commands that have run to completion. Poll this to notice one finishing without
having to catch `command_state` mid-transition.

#### `Widget::output`

```cplus
fn output(this) -> text::Text
```

What the running or last-finished command put on the SCREEN — the text a build's
errors are in. Not the byte stream: a shell writes things it then erases, and
they never belong to the command's output. Bounded; frozen when the command
finishes, so the prompt drawn afterwards does not grow into it.

#### `Widget::command_id`, `Widget::command_line`

```cplus
fn command_id(this) -> u64
fn command_line(this) -> text::Text
```

The id of the current or last command, and the command line as the user typed
it. A caller that remembers the id it saw can tell "the same command is still
running" from "another one began".

#### `Widget::cwd`, `Widget::title`

```cplus
fn cwd(this) -> text::Text
fn title(this) -> text::Text
```

The shell's working directory, as it reports it after every command, and the
window title the program last set.

#### `Widget::alternate_screen`

```cplus
fn alternate_screen(this) -> bool
```

Whether a full-screen program (`vim`, `top`) has the pane. Worth asking before
sending the shell a command it will not see.

#### `Widget::has_integration`

```cplus
fn has_integration(this) -> bool
```

Whether the shell reports prompt and command marks at all. `false` means
`command_state` never leaves `Idle` on its own and `run` falls back to
bracketing commands itself.

#### `Widget::on_command_end`, `Widget::on_exit`

```cplus
fn on_command_end(this, handler: fn(*u8), ctx: *u8)
fn on_exit(this, handler: fn(*u8), ctx: *u8)
```

`on_command_end` is called on the main thread once per command that finishes;
read the result off the widget from inside it. `on_exit` is called once when the
shell itself goes away. The handler carries only its own context pointer, which
must outlive the widget.

#### `Widget::shell_exit_code`

```cplus
fn shell_exit_code(this) -> option::Option[i32]
```

The shell's own exit status, once it has exited. `None` while it is still
running.

## `terminal/appkit`

The same `Widget` with AppKit types, for applications that mount views
themselves. Every verb above is identical.

#### `Widget::view`, `Widget::native_handle`, `Widget::node`

```cplus
fn view(this) -> ak::View
fn native_handle(this) -> *u8
fn node(this) -> flex::Node
```

The owned `NSScrollView` as a view handle, a retained raw handle for
`facet::native(handle)`, and a fixed flex leaf carrying the view as its payload.

## `terminal/terminal`

Platform-neutral. No AppKit and no pseudo-terminal: it accepts bytes from
anywhere.

### `Screen`

```cplus
struct Screen
```

### `new`

```cplus
fn new(
    rows: u16 = 24 as u16,
    cols: u16 = 80 as u16,
    max_scrollback: usize = 1048576 as usize,
    max_capture: usize = 1048576 as usize,
) -> Screen
```

`max_scrollback` bounds retained history and `max_capture` bounds a command's
captured output, both with a floor of 1024. Rows have a floor of 1 and columns
of 2.

### `Screen::feed`

```cplus
fn feed(ref this, bytes: str)
```

Consumes arbitrary output bytes. Handles the control characters, CSI cursor
motion and erase, insert and delete, scroll regions, the alternate screen,
charset selection, and OSC strings. SGR is parsed and dropped. UTF-8 payload
passes through unchanged, including a scalar split across two calls.

### `Screen::view`, `Screen::count`, `Screen::cursor`

```cplus
fn view(ref this) -> str
fn count(ref this) -> usize
fn cursor(ref this) -> usize
```

`view` is scrollback followed by the live grid, as a borrow of a cache rebuilt on
demand — valid until the next `feed` or `resize`. Trailing blanks on each row are
excluded, and so are blank rows below the last written one, except that the
cursor's row always survives.

`cursor` is the byte offset of the cursor in `view`. It can point one past the
end, because the blank a shell leaves under the cursor is trimmed. A renderer
pads that cell back rather than clamping.

### `Screen::resize`

```cplus
fn resize(ref this, rows: u16, cols: u16)
fn rows(this) -> u16
fn cols(this) -> u16
```

Columns are clipped or padded and rows are added or dropped from the top, which
banks them into scrollback. Wrapped lines are not reflowed.

### `Screen::scrollback`, `Screen::clear`

```cplus
fn scrollback(this) -> str
fn clear(ref this)
```

`clear` empties history and resets the screen, the parser and every mode.

### Replies

```cplus
fn reply(this) -> str
fn has_reply(this) -> bool
fn clear_reply(ref this)
```

Bytes the terminal owes the program, from a device-attributes or cursor-position
query. Write them to the pseudo-terminal and call `clear_reply`; a program that
asked and never hears back waits forever.

### State a host needs

```cplus
fn alternate_screen(this) -> bool
fn cursor_visible(this) -> bool
fn application_cursor_keys(this) -> bool
fn application_keypad(this) -> bool
fn bracketed_paste(this) -> bool
fn mouse_tracking(this) -> bool
fn bells(this) -> u64
fn title(this) -> str
fn cwd(this) -> str
fn cursor_row(this) -> u16
fn cursor_col(this) -> u16
```

`application_cursor_keys` decides whether arrows are sent as `ESC O A` or
`ESC [ A`; getting it wrong is why arrow keys insert junk inside a full-screen
program. `bracketed_paste` decides whether a paste is wrapped in
`ESC[200~`/`ESC[201~`.

### The command ledger

```cplus
fn has_integration(this) -> bool
fn command_state(this) -> CommandState
fn command_id(this) -> u64
fn command_line(this) -> str
fn exit_code(this) -> i32
fn finished_count(this) -> u64
fn output(this) -> text::Text
fn reset_command_ledger(ref this)
```

Driven by `OSC 133` marks. `exit_code` is `-1` until a command has finished or
when the shell reported no status.

### `Screen::finish`

```cplus
fn finish(ref this)
```

Marks the stream closed, flushing a scalar cut in half by the close.

### `is_utf8_continuation`

```cplus
fn is_utf8_continuation(b: u8) -> bool
```

Exposed because a renderer converting byte offsets to UTF-16 units needs the
same test.

## `terminal/pty`

macOS pseudo-terminal session. The surface is deliberately narrow so another
platform can replace the implementation without changing the widget.

### `supported`

```cplus
fn supported() -> bool
```

### `start`

```cplus
fn start(
    shell: str = "/bin/zsh",
    cwd: str = "",
    rows: u16 = 24 as u16,
    cols: u16 = 80 as u16,
    save_history: bool = false,
) -> option::Option[Session]
```

Forks a login shell on a new pseudo-terminal and makes the master descriptor
non-blocking. `TERM` is exported as `xterm-256color` and `COLORTERM` as
`truecolor` in the parent before the fork.

Shell integration is installed through a generated `ZDOTDIR` whose rc files
source the user's real ones: `OSC 133` command marks in both history modes, and
the history opt-out when `save_history` is false. If the shim cannot be written,
the shell runs unmodified and `Screen::has_integration` stays false.

### `ReadResult`

```cplus
enum ReadResult {
    Data(usize),
    WouldBlock,
    Closed,
    Failed,
}
```

`Closed` covers both a zero-length read and Darwin's `EIO`, which is how a
pseudo-terminal reports that its peer went away.

### `Session`

```cplus
fn read_into(this, buffer: *u8, capacity: usize) -> ReadResult
fn write(this, bytes: str) -> isize
fn resize(this, rows: u16, cols: u16) -> bool
fn master_fd(this) -> i32
fn pid(this) -> i32
fn is_open(this) -> bool
fn poll_exit(ref this) -> option::Option[i32]
fn has_exited(ref this) -> bool
fn close(ref this)
```

`read_into` is one non-blocking read; drain until `WouldBlock`. `resize` sets the
window size (`TIOCSWINSZ`), which is what raises `SIGWINCH` in the child.

`poll_exit` reports the shell's own exit status, `None` while it is still
running, and is safe to call in a loop: `waitpid` gives a status up exactly once,
so the answer is remembered. A shell killed by a signal reports `128 + signal`.

`close` reaps, hangs up, and escalates to `SIGKILL` if the child ignores the
hangup, so it cannot block indefinitely. Idempotent, and also run by `drop`.
