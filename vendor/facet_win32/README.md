# facet_win32

facet's Win32 backend — the Windows counterpart of
[`facet_appkit`](../facet_appkit), [`facet_gtk`](../facet_gtk) and
[`facet_android`](../facet_android).

facet owns the description tree and **all** layout (the shared `flex_layout`
engine). This package answers the contract's verbs with user32/gdi32/comctl32,
or records in [MANIFEST.md](MANIFEST.md) that Win32 cannot — and the second
half is the point: a verb that is neither implemented nor argued is a bug.

```
facet_win32  the registration surface: install(), and nothing else
views        the Renderer's five verbs
controls     one HWND body per node kind
input        the READ half — WM_COMMAND / WM_NOTIFY, the gesture and key bands
subclass     the messages a system control keeps to itself — Enter, the caret
paint        the shared band — background / radius / opacity / enabled / tooltip
geometry     computed frames -> SetWindowPos placements, and a scroll's extent
measure      intrinsic size, in each control's own font
fonts        the font cache, and which props are typography per kind
scheduler    schedule + the Scheduler service, over the message queue
window       the host HWND, the WNDPROC every window shares, and the menus
recycler     list / collection / tree, virtualised over a scrolling panel
imaging      the bitmap decoder — GDI+, and a GIF's own frame delays
anim         the two things that move on their own: a progress tween, a page slide
dnd          receiving a drop, without COM
menus        HMENU, and the one command-id table
dialogs      alert / choose / prompt as facet trees, and the file chooser
sys          constants, and the UTF-8 <-> UTF-16 door every string goes through
observers    size observation, for the resource tier
```

## What makes this backend different

One fact, and three consequences that shape every file: **a Win32 child control
is a WINDOW, not a view.**

1. **The parent owns the event.** A `BUTTON` does not call you back; it sends
   `WM_COMMAND` to its parent, and a trackbar sends `WM_HSCROLL`. So the read
   half is a WNDPROC on the HOST rather than a controller per control — the one
   place this package's shape genuinely departs from facet_gtk's.
2. **A control HWND is not paintable by us.** It draws itself. The shared
   band's decorative half is answered on the container BEHIND it. Where a verb
   needs the control's own pixels, this package owner-draws — and where it
   cannot, MANIFEST.md says so rather than faking it.
3. **Z-order IS the child order.** There is no child list to splice: `insert`
   is `SetParent` plus a `SetWindowPos` naming the sibling to follow. Paint
   order and `component::raise` are the same mechanism.

Origin is top-left with y growing downward — as in flex and facet_gtk, and
unlike facet_appkit — so there is no flip anywhere in this package.

## Status is a measurement

```
$ python3 ../facet_gtk/tools/parity.py win32

362 prop bits declared across facet's kind modules
  gtk       358 / 362    98%
  appkit    336 / 362    92%
  win32     290 / 362    80%   <-- this package

 68 declared handlers
  gtk        68 / 68    100%
  win32      53 / 68     77%   <-- this package

 21 shared-band bits
  win32      16 / 21     76%   <-- this package

Nothing unanswered is unrecorded: every gap is either built or argued in MANIFEST §1.
```

That closing line is the number that matters more than the percentage. Every
kind reads either `n/n` or `decided absent:` — there is no `not yet:` row.

**Read [MANIFEST.md](MANIFEST.md) before trusting any adjective here, including
that one.** And run the tool rather than quoting it: four separate times this
package's own §1 prose was what made a verb read as absent. The tool takes any
backticked identifier in §1 as "decided absent" and keeps only the leaf, so a
true sentence about a themed `button` also condemned `text_button`, which is
owner-drawn and can simply be given a border. The rule that came out of it:
name a verb in §1 only where the sentence decides that verb across the whole
backend; when a kind is the exception, describe it without backticks.

## Three ways to check it

```bash
cd vendor/facet_win32 && ../../target/release/cpc test      # 111 tests, no window needed

cd playground/win32_probe         && ../../target/release/cpc build   # the SEAM
cd playground/win32_runtime_probe && ../../target/release/cpc build   # the FACADE
```

The suite pins what the backend DECIDES — the mappings, the style bits, the
arithmetic, the encoders — and opens no window, because a suite that needs a
desktop session is a suite that stops running.

The probes are the other half. `win32_probe` calls the backend directly and
proves the seam; `win32_runtime_probe` goes through `runtime::App` and proves
the facade. Between them they cover what a test cannot: whether the slider
reported while it was dragged, whether the field kept its caret, whether a
carousel's page slide reads as a flick rather than a jump.

### ...and the third: ask it

Both probes serve the agent surface behind `FACET_PROBE_AGENT`, which is the
answer to "an agent has no eyes":

```bash
FACET_PROBE_AGENT=1 ./target/debug/win32_probe
# facet: agent surface on http://127.0.0.1:9352/ — 25 verbs (11 core, 14 armed)
```

`describe_tree` then answers what was actually built — keys, kinds and frames —
so a claim like "the grouped collection has two section headers" is a query
rather than a squint. That is how the grouping arithmetic in `recycler` was
confirmed on live data, and how a bug that made collections never rebuild was
found.

**It does not return list ROWS**, and that is facet's shape rather than this
backend's — a virtualised row is realised without becoming a child of the list
node, so the tree walk never reaches it. MANIFEST §3 has the detail.

## Diagnostics

```bash
FACET_WIN32_ROWS=1 ./target/debug/win32_probe    # what the recycler is holding
# [rows] model=20000 visible=0..9 cells=10 live=10
```

Ten cells for a twenty-thousand-row model, and the number does not move with
the model. That is the property `recycler.cplus` exists to have, and it is a
number rather than an adjective because facet_gtk's handoff ended on exactly
this measurement — "GtkListView keeps 205 cells for a viewport that holds
seven".

```bash
FACET_WIN32_THEME=1 ./target/debug/win32_probe   # which comctl32 actually loaded
```

`IsAppThemed()` is NOT the signal — it answers 0 on a process demonstrably
running comctl32 6.0, and 0 on a C program drawing themed controls at the time.
The loaded module's PATH is, which is what this prints. Believing the API cost
an afternoon.

## The rules worth knowing before editing

**One code path for create and apply.** `create` configures a new control with
`paint::all_bits()`, the same call `apply` makes with the node's real dirty
word. A prop honoured on update is therefore honoured on create, and the two
cannot drift. Every write is gated on its bit.

**The bits are PER KIND.** A generated module numbers its `P_` constants from 1
in declaration order, so the same verb is a different bit on each kind —
`list::P_COUNT` is exactly `collection::P_SELECTED_INDEX`. This has been hit
three times here: fonts, then the recycler, then the sequence applier. A table
that reads one kind's numbers for another compiles, runs, and quietly does
nothing.

**Text goes out WIDE.** The `*A` entry points decode the process ANSI codepage
and a C+ string is UTF-8. `sys::to_wide` / `sys::from_wide` are the door; a
direct `*A` call with anything above ASCII is mojibake, and it showed up in a
window title before it was fixed.

**A child window owns its pixels.** The parent does not paint underneath it, so
a control that skips its own erase does not become transparent — it keeps
whatever it drew last. That is one bug (a label reading `unche…ed` beside its
own ghost) and one absent verb (`icon_button.is_opaque`), from one fact.
