# Guide

## A screen, not a line buffer

The model is a grid: `rows * cols` cells holding one Unicode scalar each, with
the cursor addressed by row and column. Rows that scroll off the top of the
primary screen are appended to scrollback as logical lines. `view()` is
scrollback followed by the live grid.

This replaced a scrollback transcript with a cursor on one live line. That
earlier model was right about the things it was built for, and a grid keeps
every one of them for free — but a line buffer has nowhere to put `ESC[5;10H`,
`ESC[2J`, or the alternate screen, and two ordinary commands showed it:

- `clear` sends `ESC[H ESC[2J ESC[3J`. All three were stripped, so the command
  did nothing at all.
- `top` sends `ESC[?1049h` (alternate screen), `ESC[1;24r` (scroll region), and
  absolute addressing (`ESC[<n>d`, `ESC[<n>G`). With no screen to address, every
  repaint appended, and quitting left the wreckage behind.

The byte handling that mattered before still holds, because a grid gives it
naturally:

- carriage return sets the column to 0; it does not end a line. A shell repaints
  its prompt with `\r<prompt>` on every keystroke, so treating CR as a line break
  produces one transcript line per keypress;
- backspace and delete move the cursor left and erase nothing. The shell's erase
  sequence is `\b space \b`, and the space does the blanking;
- cursor motion steps whole scalars, so the cursor never lands inside a
  character.

What the grid added: absolute addressing (`CUP`, `HPA`, `VPA`, `CHA`), erase
(`ED`, `EL`, `ECH`), insert and delete of characters and lines (`ICH`, `DCH`,
`IL`, `DL`), scroll regions (`DECSTBM`) and explicit scrolling (`SU`, `SD`),
the alternate screen (`?47`, `?1047`, `?1049`), save and restore cursor
(`DECSC`/`DECRC`), index and reverse index, tab stops every eight columns, DEC
Special Graphics (`ESC ( 0`) so ncurses frames draw as lines rather than
`lqqqk`, and the modes a key path depends on — `DECCKM`, `DECAWM`, `DECTCEM`,
bracketed paste, insert mode.

Still deliberately not modelled: per-cell colour and attributes (SGR is parsed
and dropped), mouse reporting, and sixel/DCS payloads. A cell is a scalar, so a
renderer draws one uniform run of text; adding attributes means widening the
cell and emitting attribute runs, which is a separate change.

## Deferred wrap

Writing to the last column does not wrap. The cursor stays there with the wrap
pending, and the NEXT printable scalar wraps first. This is what keeps a row
filled exactly to the width, followed by a newline, from leaving a blank row
behind.

A row that genuinely wrapped is marked, and `view()` joins it to the next row
without a newline. Copying a long command line out of the pane therefore gives
back one line. A row that merely filled the width exactly is not marked, because
the wrap never fired.

## Newline mode

A strict VT moves DOWN on `\n` and returns to column 0 only on `\r`, which is
why bare-LF text staircases across a real terminal. That never shows up on the
pseudo-terminal path — the line discipline turns `\n` into `\r\n` before the
screen sees it — so honouring it by default would only punish callers feeding
this model captured pipe output, which it explicitly accepts. LNM is therefore
ON by default; `ESC[20l` turns it off for a program that means the staircase.

## Bounded scrollback

`max_scrollback` caps retained history, with a floor of 1024 bytes. When the cap
is exceeded, the front is cut, preferring a line boundary within the next 4096
bytes and otherwise cutting at a scalar boundary so the new first byte is never
a continuation byte.

The cap bounds SCROLLBACK, not `view()`: the live screen is always retained, so
`count()` is bounded by the cap plus the grid.

## Trailing blanks

`view()` excludes trailing blanks on each row. A real terminal renders the blank
cell that `\b space \b` leaves as nothing, so reporting it would show erased
characters as lingering whitespace. Blank rows below the last written one are
dropped too, but never below the cursor's row — a terminal draws its cursor even
on an empty line.

A consequence for renderers: `cursor()` can point one past the end of `view()`,
and legitimately so, because the cursor sits on a blank that the view drops. A
renderer pads that cell back rather than clamping the cursor onto the last
character, which would place it one cell to the left of the truth. On an
interior row the cursor's column is padded out instead, so the offset always
lands where the cursor really is.

## Resizing

Columns are clipped or padded and rows are added or dropped from the top;
wrapped lines are NOT reflowed. Two things make that acceptable rather than
lossy: the shell repaints its prompt on `SIGWINCH`, and scrollback holds logical
lines rather than wrapped ones, so nothing already banked can be truncated by a
narrower screen.

The widget observes its own frame, divides the scroll view's content size by the
font's advancement and line height, and sends both `TIOCSWINSZ` and the model
resize. Both halves matter: a program drawing to a screen of one size while the
model lays it out at another puts every absolute cursor address on the wrong
cell, which renders as diagonal wreckage.

## Answering the program

Some sequences are questions. `ESC[6n` asks where the cursor is, `ESC[c` asks
what the terminal is, `ESC[5n` asks whether it is well. A program that asks and
never hears back simply waits. The screen accumulates answers in a reply buffer;
the host drains it after each feed and writes it back to the pseudo-terminal.

## Command marks, and why the shell has to send them

A terminal cannot tell where one command ends and the next begins, and it
certainly cannot know an exit code: what arrives on the wire is output, with
nothing in it to say which command produced it or how it fared. The SHELL knows
both.

`terminal/pty` installs zsh hooks through the generated `ZDOTDIR`. `preexec`
fires when a line is accepted and emits `OSC 133;C;<command>`; `precmd` fires
before the next prompt and emits `OSC 133;D;<status>`, then `OSC 7` with the
working directory, then `OSC 133;A`. The screen turns those into the ledger that
`command_state`, `exit_code`, `command_line`, `cwd` and `output` report.

### How the terminal knows a command ended

It does not detect it. It is told, a moment BEFORE the prompt comes back.

Nothing in the byte stream marks a prompt — a prompt is text the shell prints,
`%` or `$` or whatever theme the user has, and recognising it is guesswork.
`precmd` is the shell's own "I am about to hand the cursor back" moment, which
is why the mark goes there. On the wire, one command looks like this:

    ESC]133;C;printf "AAA\n" BEL     preexec: line accepted, output starts
    AAA CR LF                        the command's own output
    ESC]133;D;0 BEL                  precmd: it ended, status 0
    ESC]7;file://host/path BEL       and here is the working directory
    ESC]133;A BEL                    the prompt is starting
    (base) adel@192 scratchpad %     ...only now is the cursor back

`finished_count` increments and `on_command_end` fires at the `D`, before the
prompt is drawn at all.

Three separate parties each know one link of the chain, and only the middle one
had no way to speak:

| link | mechanism |
|---|---|
| kernel to shell | the shell forks and execs the command, then blocks in `waitpid`; the kernel wakes it with the exit status, which becomes `$?` |
| shell to terminal | `precmd` fires and emits `OSC 133;D;<status>` |
| terminal to application | the screen parses that into the ledger |

What follows from asking the shell rather than reading the pane:

- output that looks like a prompt ends nothing;
- a REPL reads as still Running. `python` draws its own prompt, but zsh's
  `precmd` does not fire, so no mark arrives — which is the truth;
- a program that ignores `SIGINT` reports nothing, for the same reason. No
  prompt comes back, so no mark. Also the truth;
- `^C` IS reported. A shell whose foreground job dies of `SIGINT` still returns
  to a prompt, so precmd still runs, and 130 arrives.

Three details are load-bearing:

- `precmd` captures `$?` on its first line, or the shell's own bookkeeping has
  already overwritten the status.
- A `__cplus_ran` flag suppresses the mark on the FIRST prompt. precmd runs
  before any command too, and a bare `D;0` there reads downstream as "the last
  build succeeded" on a pane nobody has typed into.
- `preexec` reports `$1`, what the user typed, rather than zsh's normalised
  `$2` — except when `$1` is multi-line, since a newline inside an OSC payload
  terminates the sequence early and sprays the rest across the screen.

The marks are the same vocabulary iTerm2 and VS Code use, so a shell already
configured for one of those is undisturbed.

For a shell with no integration, `run()` falls back to bracketing the command
with marks of its own, sent as separate lines. It works in any POSIX shell; the
cost is that the two `printf` lines are echoed where the user can see them.

## Captured output is what reached the SCREEN

`output()` is taken from the grid, not from the byte stream, and the difference
is not cosmetic. zsh emits its partial-line marker — a reverse-video `%` padded
to the full width — BEFORE running precmd, and erases it with `ESC[J` only
AFTER. So it falls inside the command's region on the wire while never being
visible for an instant. Capturing printable bytes put that marker on the end of
every command's output.

The captured region runs from where the command started to where the CURSOR is
when the finish mark arrives — not to the end of the cursor's row, which is
where the marker sits. Rows that scroll off while the command runs are banked as
they go. The region is frozen at the finish mark, because the shell keeps
writing after it: the working directory, the prompt mark, the prompt itself.

## Rendering the cursor

The AppKit widget draws the cursor as a block: the cell under the cursor is
given a background colour of the text colour and a foreground colour of the
background colour, which is reverse video. Unfocused, it is a thick underline
instead — the two states differ in SHAPE, not shade, which survives any palette
and any vision. A program that hid the cursor (`ESC[?25l`, which every
full-screen program does while repainting) gets no block at all.

Attributes are used rather than the selection, so selecting text with the mouse
and copying it still works, and the empty selection means `⌘C` with nothing
selected copies nothing.

Two conversions matter. `NSRange` counts UTF-16 units while the screen counts
bytes; the two agree only while the text is ASCII, and a single accented
character in a prompt desynchronises them. The widget converts by measuring the
tail from the cursor to the end of the text rather than rescanning the whole
scrollback. An out-of-bounds `NSRange` raises an Objective-C exception, so the
computed location is clamped against the string's real length before use.

The wipe before painting is not optional and has to happen on every pass:
`-[NSTextView setString:]` stamps the view's TYPING attributes onto the new
text, and NSTextView derives those from the character at the insertion point,
which the renderer parks directly on the cell it just styled. Without the wipe
the cursor's own attribute becomes the typing attribute and the next render
paints it across the whole string.

## Reading the pseudo-terminal

A libdispatch read source on the main queue drains the master descriptor, feeds
the screen, flushes any replies, updates the view, and delivers callbacks. The
descriptor is non-blocking, so the handler reads until `EAGAIN`. `EIO` is how
Darwin reports that the pseudo-terminal peer closed; it is treated as end of
stream, not as an error.

Because reads land on the main queue, the screen is only ever mutated on the
main thread, and a snapshot taken from a handler cannot tear. Callbacks fire on
the main thread for the same reason.

Command-end callbacks are counted rather than edge-triggered: one read can carry
several commands' worth of marks, and a handler that ran once per chunk would
miss some and repeat others.

## Keyboard and focus

Keys are delivered through the view's `keyDown:`, forwarded to the
pseudo-terminal, and never inserted into the view. Special keys are translated to
the byte sequences a terminal sends. Arrows and Home/End change SHAPE with
`DECCKM`: `ESC O A` rather than `ESC [ A`. Every full-screen program turns it
on, and one that gets the wrong form sees a literal `[` followed by an `A` —
which is how arrow keys "do nothing but insert junk" inside `top`, `less` and
vim.

Paste is bracketed when the program asked for it (`ESC[?2004h`). Without the
brackets an editor cannot tell pasted text from typing, so it auto-indents every
line of a pasted block, and a pasted newline runs as a command.

Focus is a window-level responder change, not a property of the view, and a view
mounted through facet's native escape hatch is not addressable by key.
`facet::find(key)` therefore cannot focus a terminal, and the widget owns
`focus()` and `has_focus()` instead. Both need a window, so they answer `false`
until the widget is mounted.

## Shell history

An embedded terminal must not write the user's GLOBAL shell history: what is
typed in an app's pane is the app's business, and macOS keeps only
`SAVEHIST=1000` entries, so a chatty pane evicts the commands the user wants.

Exporting `HISTFILE` does not work for zsh on macOS. `/etc/zshrc` contains
`HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` unconditionally and runs after our
environment is in place. What that line does honour is `ZDOTDIR`, which is the
lever: point it at a directory of ours whose rc files source the user's real ones
and then turn history off.

`save_history: true` keeps the shim — the command marks are not a history
feature — but repairs the `HISTFILE` the shim itself caused: `/etc/zshrc`
derives it from `ZDOTDIR`, which is ours, so without the repair the user's
history would quietly move into a temp directory. The test is exact, firing only
on the value `/etc/zshrc` produces, so a user who sets their own `HISTFILE`
keeps it.

The two dispositions get separate directories. They used to share one that was
rewritten on every `start`, which is fine until an app opens a pane of each
kind: the setting silently became "whichever pane started last".

## Lifetime and shutdown

The widget owns the session. `stop()` cancels the read source and closes the
pseudo-terminal; dropping the widget does the same. Closing reaps first, then
sends `SIGHUP`, then escalates to `SIGKILL` if the child ignores the hangup, so
destruction stays deterministic and never blocks the main thread indefinitely.
The exit status is remembered, because `waitpid` hands one over exactly once and
both `poll_exit` and `close` reap.

Cancellation is asynchronous: the dispatch cancel handler runs later and frees
the context. A widget dropped before that handler runs marks the context for
disposal, and whichever side finishes last performs it.

`TERM` and `COLORTERM` are exported in the parent before the fork, not in the
child, because only async-signal-safe calls are legal between fork and exec and
`setenv` allocates. The child does `chdir` and `execl`, nothing else.

## Portability

Only the macOS backend exists today. The session seam is deliberately narrow —
start, read, write, resize, close, poll_exit — so a Linux backend (`forkpty`
plus `epoll`) or a Windows one (ConPTY) can be added without changing the screen
model or the widget's API. `terminal/terminal` is platform-neutral and is tested
as such.
