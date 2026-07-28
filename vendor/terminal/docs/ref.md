# Reference

Four modules:

```cplus
import "terminal/widget" as terminal;      // portable facet-facing widget
import "terminal/appkit" as terminal_ui;   // the same widget in AppKit types
import "terminal/terminal" as terminal;    // platform-neutral transcript
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
) -> option::Option[Widget]
```

Opens a pseudo-terminal, launches `shell` in it as a login shell, and builds
the view. An empty `cwd` inherits the process's working directory.
`max_scrollback` bounds retained bytes and is raised to a floor of 1024.
`None` means the pseudo-terminal, the dispatch source, or the allocation
failed; nothing is left running.

Call on the AppKit main thread.

### `Widget`

```cplus
struct Widget
```

Owns the session and the view. Keep it alive for as long as the terminal
should stay interactive; dropping it stops the shell.

#### `Widget::node`

```cplus
fn node(this) -> facet::Node
```

The terminal as one portable facet node, sized with `grow`, `frame` and the
rest of the node verbs. The node holds its own retain on the native view; the
widget still owns the session.

#### `Widget::send`

```cplus
fn send(this, bytes: str)
```

Writes bytes to the pseudo-terminal as a keypress would. A shell terminates a
line on `\r`. No-op once the session has been stopped.

#### `Widget::focus`

```cplus
fn focus(this) -> bool
```

Makes the terminal the window's first responder, so typing goes to the shell.
`false` means the widget is not on screen yet, so there is no window to hold a
first responder: call it from `on_attach` or a handler, not from `build`.

Call it again at the end of a handler that runs one of the application's own
controls; clicking a button can take the first responder.

#### `Widget::has_focus`

```cplus
fn has_focus(this) -> bool
```

Whether the terminal currently holds the keyboard.

#### `Widget::text`

```cplus
fn text(this) -> text::Text
```

A snapshot of the cleaned transcript: control traffic removed, UTF-8 payload
intact, trailing blanks on the live line excluded.

#### `Widget::is_running`

```cplus
fn is_running(this) -> bool
```

Whether the pseudo-terminal is still active. Becomes `false` after `stop`, and
after the shell exits and the read source drains the close.

#### `Widget::stop`

```cplus
fn stop(this)
```

Cancels the read source and closes the session. Idempotent. Dropping the widget
does the same; `stop` exists so an application can put shutdown at a named
seam, such as `on_detach`.

## `terminal/appkit`

The same `Widget` with AppKit types, for applications that mount views
themselves. `supported`, `start`, `send`, `focus`, `has_focus`, `text`,
`is_running` and `stop` are identical to the widget module.

#### `Widget::view`

```cplus
fn view(this) -> ak::View
```

The owned `NSScrollView` as a view handle.

#### `Widget::native_handle`

```cplus
fn native_handle(this) -> *u8
```

A retained raw handle, for `facet::native(handle)`.

#### `Widget::node`

```cplus
fn node(this) -> flex::Node
```

A fixed flex leaf carrying the view as its payload.

## `terminal/terminal`

Platform-neutral. No AppKit and no pseudo-terminal: it accepts bytes from
anywhere.

### `Transcript`

```cplus
struct Transcript
```

#### `Transcript::new`

```cplus
fn new(max_bytes: usize = 1048576 as usize) -> Transcript
```

`max_bytes` bounds retained bytes, with a floor of 1024.

#### `Transcript::feed`

```cplus
fn feed(ref this, bytes: str)
```

Consumes arbitrary output bytes. CSI sequences and OSC strings are stripped;
CR, LF, BS, DEL, TAB and the cursor finals `C`, `D` and `K` are interpreted;
UTF-8 payload passes through unchanged. Trims scrollback to `max_bytes` at the
end of the call.

#### `Transcript::view`

```cplus
fn view(this) -> str
```

The transcript as a borrow of the internal buffer. Trailing spaces on the
uncommitted line are excluded; committed lines are returned verbatim. Read-only.

#### `Transcript::count`

```cplus
fn count(this) -> usize
```

Retained bytes, including the trailing blanks that `view` excludes.

#### `Transcript::cursor`

```cplus
fn cursor(this) -> usize
```

The cursor as a byte offset into the text: the cell the next write will land
on. It can point one past the end of `view()`, because the blank a shell leaves
under the cursor is trimmed from the view. A renderer pads that cell back
rather than clamping.

#### `Transcript::clear`

```cplus
fn clear(ref this)
```

Empties the buffer and resets the parser and the cursor.

#### `Transcript::finish`

```cplus
fn finish(ref this)
```

Marks the stream closed. Nothing is deferred in the current model, so it does
nothing; it is retained so callers keep a stable API.

### `is_utf8_continuation`

```cplus
fn is_utf8_continuation(b: u8) -> bool
```

Whether `b` is a UTF-8 continuation byte. Exposed because a renderer converting
byte offsets to UTF-16 units needs the same test.

The cursor mechanics that `feed` drives — `put_byte`, `commit_line`,
`cursor_left`, `cursor_right`, `erase_to_line_end`, `trim` — are the parser's
steps rather than an entry point. Feed bytes instead.

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
) -> option::Option[Session]
```

Forks a login shell on a new pseudo-terminal and makes the master descriptor
non-blocking. `TERM` is exported as `xterm-256color` and `COLORTERM` as
`truecolor` in the parent before the fork.

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

#### `Session::read_into`

```cplus
fn read_into(this, buffer: *u8, capacity: usize) -> ReadResult
```

One non-blocking read. Drain until `WouldBlock`.

#### `Session::write`

```cplus
fn write(this, bytes: str) -> isize
```

Bytes written, or `-1` on a closed session.

#### `Session::resize`

```cplus
fn resize(this, rows: u16, cols: u16) -> bool
```

Sets the window size (`TIOCSWINSZ`), which is what raises `SIGWINCH` in the
child.

#### `Session::master_fd`, `Session::pid`, `Session::is_open`

```cplus
fn master_fd(this) -> i32
fn pid(this) -> i32
fn is_open(this) -> bool
```

#### `Session::close`

```cplus
fn close(ref this)
```

Closes the master descriptor, sends `SIGHUP`, and reaps with `WNOHANG`. A child
that ignores the hangup is sent `SIGKILL` and reaped, so the call cannot block
indefinitely. Idempotent, and also run by `drop`.
