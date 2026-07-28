# Guide

## A transcript, not a screen

The model is a scrollback transcript with a cursor on the current line, not a
VT screen with addressable cells. A shell does not edit text; it moves a cursor
and overwrites. `_text` holds committed scrollback and the line being edited,
`_line_start` marks where that live line begins, and `_col` is the cursor's
byte offset within it. Keeping both in one buffer makes `view()` a borrow with
no assembly step.

That distinction decides the byte handling:

- carriage return moves the cursor to column 0; it does not end a line. A shell
  repaints its prompt with `\r<prompt>` on every keystroke, so treating CR as a
  line break produces one transcript line per keypress;
- line feed commits the current line and starts a new one;
- backspace and delete move the cursor left and erase nothing. The shell's
  erase sequence is `\b space \b`, and the space does the blanking;
- tab writes four spaces at the cursor rather than a tab stop;
- `ESC[<n>C` and `ESC[<n>D` move the cursor right and left by whole scalars;
  `ESC[K` truncates from the cursor to the end of the line;
- every other CSI sequence and every OSC string is stripped. Colour and mode
  changes are discarded rather than rendered.

Cursor motion steps whole UTF-8 scalars, so the cursor never lands inside a
character and overwriting a multi-byte scalar with a one-byte one leaves no
continuation byte behind.

What this model does not do: alternate-screen switching, cell-accurate cursor
addressing, mouse reporting, and per-cell 256/true-colour rendering. Shells,
build output, REPLs and command panes work. A full-screen TUI needs a VT screen
engine, which is not claimed here.

## Bounded scrollback

`max_bytes` caps retained bytes. When the cap is exceeded, `trim` cuts from the
front, preferring a line boundary within the next 4096 bytes and otherwise
cutting at a scalar boundary so the new first byte is never a continuation
byte. The live line's start and the cursor shift with the cut, so the cursor
never addresses committed scrollback.

The cap is a floor of 1024 bytes; smaller values are raised.

## Trailing blanks

`view()` excludes trailing spaces on the uncommitted line. A real terminal
renders the blank cell that `\b space \b` leaves as nothing, so reporting it
would show erased characters as lingering whitespace. Committed lines are
returned verbatim.

A consequence for renderers: `cursor()` can point one past the end of `view()`,
and legitimately so, because the cursor sits on a blank that the view drops. A
renderer pads that cell back rather than clamping the cursor onto the last
character, which would place the cursor one cell to the left of the truth.

## Rendering the cursor

The AppKit widget draws the cursor as a block: the cell under the cursor is
given a background colour of the text colour and a foreground colour of the
background colour, which is reverse video. Attributes are used rather than the
selection, so selecting text with the mouse and copying it still works, and the
empty selection means `⌘C` with nothing selected copies nothing.

Two conversions matter. `NSRange` counts UTF-16 units while the transcript
counts bytes; the two agree only while the text is ASCII, and a single accented
character in a prompt desynchronises them. The widget converts by measuring the
tail from the cursor to the end of the text, which is the rest of the live line,
rather than rescanning the whole scrollback. An out-of-bounds `NSRange` raises
an Objective-C exception, so the computed location is clamped against the
string's real length before it is used.

## Reading the pseudo-terminal

A libdispatch read source on the main queue drains the master descriptor,
feeds the transcript, and updates the view. The descriptor is non-blocking, so
the handler reads until `EAGAIN`. `EIO` is how Darwin reports that the
pseudo-terminal peer closed; it is treated as end of stream, not as an error.

Because reads land on the main queue, the transcript is only ever mutated on
the main thread, and a snapshot taken from a handler cannot tear.

## Keyboard and focus

Keys are delivered through the view's `keyDown:`, forwarded to the
pseudo-terminal, and never inserted into the view. Special keys are translated
to the byte sequences a terminal sends: `\r` for Return, `\t` for Tab, `\x7f`
for Delete, `\x1b` for Escape, and the usual CSI sequences for arrows, Home,
End, Page Up, Page Down and forward delete. Control characters arrive as the
event's own characters and pass straight through, which is why `^C` works
without a special case.

Focus is a window-level responder change, not a property of the view, and a
view mounted through facet's native escape hatch is not addressable by key.
`facet::find(key)` therefore cannot focus a terminal, and the widget owns
`focus()` and `has_focus()` instead. Both need a window, so they answer `false`
until the widget is mounted.

## Resizing

The widget observes its own frame. On every change it divides the scroll view's
content size by the font's advancement and line height and sends `TIOCSWINSZ`,
with floors of two columns and one row. A program in the pane sees the same
`SIGWINCH` and `stty size` it would see in any terminal.

## Lifetime and shutdown

The widget owns the session. `stop()` cancels the read source and closes the
pseudo-terminal; dropping the widget does the same. Closing sends `SIGHUP`,
reaps with `WNOHANG`, and escalates to `SIGKILL` if the child ignores the
hangup, so destruction stays deterministic and never blocks the main thread
indefinitely.

Cancellation is asynchronous: the dispatch cancel handler runs later and frees
the context. A widget dropped before that handler runs marks the context for
disposal, and whichever side finishes last performs it.

`TERM` and `COLORTERM` are exported in the parent before the fork, not in the
child, because only async-signal-safe calls are legal between fork and exec and
`setenv` allocates. The child does `chdir` and `execl`, nothing else.

## Portability

Only the macOS backend exists today. The session seam is deliberately narrow —
start, read, write, resize, close — so a Linux backend (`forkpty` plus `epoll`)
or a Windows one (ConPTY) can be added without changing the transcript model or
the widget's API. `terminal/terminal` is already platform-neutral and is tested
as such.
