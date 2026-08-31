# facet_win32 — the manifest

INTENT's rule, the same one `facet_appkit/MANIFEST.md` is held to and the same
one `facet_gtk/MANIFEST.md` inherited: for each thing facet declares, this
package either implements it with what Win32 offers, or **states plainly that
Win32 cannot**. A verb that is neither implemented nor listed here is a gap,
and the gap is a bug.

This file keeps two things strictly apart, and blurring them is the failure it
exists to prevent — a reader who cannot tell them apart cannot tell a young
backend from an abandoned one:

* **§1 — decided absent.** Win32 has no such thing. Looked for, not found,
  argument attached.
* **§2 — not built yet.** Win32 has it and this package has not reached it.

**Status 2026-08-29: EVERY DECLARED VERB IS NOW EITHER ANSWERED OR ARGUED.**
That is the doctrine's own bar, and it is the first time this package clears it:
`parity.py win32` prints no `not yet` row for any kind. What is left is 123
prop bits and 16 handlers with an argument attached in §1 or §2 below, and the
argument is the deliverable — a reader can tell what Win32 refuses from what
this package has not reached.

The controls are real: two window classes, HWND-per-node, flex's frames as
positions. Owner-draw where a themed control refuses a verb (`text_button`,
`icon_button`, `toggle`, `popup`), custom-draw where the verb is three
different pixels (`slider`), and this package's own painting where there is no
control at all (`tabs`, `page_dots`, `bordered`, and the list's separators).
`recycler.cplus` virtualises lists, collections and trees over an ordinary
scrolling panel — measured at ten cells for a twenty-thousand-row model, and
still ten with grouping built.
`subclass.cplus` reads the messages a system control keeps to itself, which is
what `on_submit` and `on_selection_changed` needed. `split` and `carousel` are
drawn and paged here, both for the reason `tabs` is: Win32 has no such control
and every toolkit that has one drew it. `imaging.cplus` decodes through GDI+'s
flat C API, which is what `image`, `slider.thumb_image` and a file-sourced
`icon_button` were all waiting on — and plays an animated GIF on its own
per-frame delays rather than a fixed interval.
`facet_runtime/runtime_windows.cplus` lands with it, so an application reaches
this backend the ordinary way rather than by calling the package directly.

Two probes under `playground/`, and the division is deliberate:
`win32_probe` calls the backend directly and proves the SEAM;
`win32_runtime_probe` goes through `runtime::App` and proves the FACADE.

```
362 declared prop bits     273 answered    75%   (gtk 358, appkit 336, uikit 323, android 321)
 68 declared handlers       53 fired       77%   (gtk  68, appkit  68, uikit  65, android  67)
 21 shared-band bits        16 named       76%   (appkit 20, gtk 19, android 19, uikit 18)
```
`python3 vendor/facet_gtk/tools/parity.py win32` prints it. Without the
argument it reports gtk; the per-kind table follows whichever backend is named,
and every backend's totals are printed either way, so the columns are
comparable. Run it before believing any adjective
in this file.

**THE THIRD NUMBER IS OUT OF PROPORTION TO THE FIRST TWO, and that is the
architecture rather than an accident.** The shared band is answered ONCE, in
`paint.cplus`, for every kind — so a backend with three control bodies already
honours most of the band. The first number is what a backend earns one control
at a time; the third is what it gets for free by having the seam right. Read
them in that order.

---

## 1. Decided absent — Win32 has no such thing

### `opacity` and `transform` on a CONTROL

`C_OPACITY` and `C_TRANSFORM` are answerable on a node this package PAINTS —
a panel, drawn in its own WM_PAINT, where the alpha can go through
`AlphaBlend` and a scale through the world transform.

They are not answerable on a system control. A child HWND has no alpha channel:
`SetLayeredWindowAttributes` requires `WS_EX_LAYERED`, which Windows honours on
**top-level windows only** — on a child it is documented as undefined and in
practice does nothing. There is no per-window transform either; `SetWorldTransform`
applies to a device context, and a control acquires its own DC when it paints.

So `button.set_opacity(0.5)` is a no-op here and a fade on a control is a jump.
The honest fix is owner-drawing every control, which is a different backend and
a much larger one — see §2's note on the ceiling.

### A per-corner `corner_radius`

`RoundRect` takes ONE ellipse for all four corners. facet's `Corners` is
per-corner, and the largest is used (`paint::uniform_radius`). Asking for three
square corners and one round one is a REGION — `CreateRoundRectRgn` composed
with `CombineRgn` — which is a different mechanism and one this package has not
built. Recorded as a narrowing rather than a gap: the common case (all four the
same) is exact.

### `EM_SETCUEBANNER` and a non-ASCII `placeholder`

The cue banner — the grey prompt inside an empty edit — has **no ANSI form**.
`EM_SETCUEBANNER` takes UTF-16 and there is no `EM_SETCUEBANNERA`. This package
uses the ANSI entry points throughout (see `win32`'s own manifest for why), so a
placeholder is passed as bytes and anything outside ASCII is dropped rather than
mojibaked. Narrowing, not absence: it comes back whole when the wide path lands.

### DARK CONTROLS — the appearance is READ, and not yet WORN

`window::is_dark` answers correctly and the title bar follows it
(`DwmSetWindowAttribute`, `DWMWA_USE_IMMERSIVE_DARK_MODE`). Everything this
package PAINTS itself — panels, the shared band, adaptive colour pairs — is
dark on a dark system.

The system CONTROLS are not. comctl32 v6 does not follow the app's appearance
on its own: each control has to be told, with
`SetWindowTheme(hwnd, L"DarkMode_Explorer", NULL)`, and the process has to opt
in first through `SetPreferredAppMode` — which is exported from uxtheme **by
ordinal 135 and by no name at all**. It is undocumented, every dark-mode Win32
application in the world uses it, and Microsoft has never blessed it.

So the state is: dark chrome, dark facet-painted surfaces, light system
controls. Recorded here rather than fixed because the fix is an undocumented
export and the decision to depend on one belongs to whoever maintains this, not
to the commit that noticed.

### THE `*A` / `*W` / UTF-8 SITUATION, and why it is settled

Kept because the reasoning is not obvious and the wrong version of it shipped
twice.

C+ strings are UTF-8. Win32 has two parallel APIs and historically neither took
UTF-8: `*A` takes the process ANSI codepage (CP1252 on a Western install) and
`*W` takes UTF-16. Handing a C+ `str` to an `*A` call decoded every byte above
0x7F as the wrong character — a window title reading `facet_win32 â€" first
light`, which is an em-dash's three UTF-8 bytes read as three CP1252
characters.

**`cpc` now declares UTF-8 as the process codepage** in the manifest it embeds
(`<activeCodePage>UTF-8</activeCodePage>`, Windows 10 1903 and later).
Measured: `GetACP()` answers 1252 without it and 65001 with. That makes the
`*A` family take UTF-8 directly, which is what `vendor/win32` assumed when it
chose the ANSI entry points — the assumption is now true rather than
aspirational.

Two places still convert explicitly, and both on purpose:

* **Window text** (`controls::set_window_text`, `measure::text_extent`) goes
  out and is measured through the `*W` calls. If `llvm-rc` is missing, `cpc`
  skips the manifest and the codepage falls back to CP1252 — the conversion is
  correct either way, on the path most likely to show a user garbage.
* **`EM_SETCUEBANNER` has no ANSI form at all.** It is a MESSAGE rather than an
  A/W function pair, so the codepage declaration does not reach it: the string
  is UTF-16 whatever the process says. Passing bytes made `type here` render as
  `裁数集牭eN` — two ASCII bytes per glyph — and a comment here claimed ASCII
  would survive, which was wrong in both halves.

What the codepage declaration DID retire: `CB_ADDSTRING` and `GetWindowTextA`
were listed as debt and are correct now for free.
### A `toggle` IS DRAWN, because Win32 has no switch

facet's `toggle` is a switch — the sliding kind. Win32 has no switch: it is a
WinUI/XAML control, not a user32 one, and there is no window class for it.

This kind WAS a `BS_AUTOCHECKBOX`, on the argument that a check box is the same
state with a different picture. That argument was wrong in a way the parity
number showed: a check box answers `on` and NOTHING ELSE, and `toggle` declares
`on_color`, `off_color` and `thumb_color` — three verbs describing a pill and a
thumb a check box does not have. Three of its four props were unanswerable by
construction.

So it is owner-drawn: `BS_OWNERDRAW` on a `BUTTON`, which keeps focus, the tab
order, space and enter and the accessibility role, and gives up only the
pixels. What that costs is the auto-check — an owner-drawn button has no check
state — so the flip moved to `input::fire_toggle`, where the NODE is the truth
and the control is drawn from it. That is the more honest arrangement anyway:
the node was always the truth and the control's check bit was a copy.

The same trade as `text_button`, below, for the same reason.

### The verbs a themed control refuses, and the two doors past it

A themed control draws itself from the theme's own bitmaps and IGNORES every
colour it is told. `PBM_SETBARCOLOR` on a themed progress bar sets a value that
is then read by nobody; `BS_FLAT` on a themed button is discarded. That is one
wall, and this package now names both ways through it so a reader can tell
which verbs are answered and at what cost:

* **`SetWindowTheme(h, L"", L"")`** takes ONE control out of visual styles, and
  the classic colour messages start working again. `paint::go_classic`.
* **Owner-draw / custom-draw** takes the pixels over entirely, keeping the
  control's focus, keyboard and accessibility role.

The rule for the first door is that **only a node that actually states a colour
pays it**. A progress bar nobody has coloured keeps the modern look; one that
asked to be red is drawn classically, because a red bar that silently stays
blue is the worse answer. Applying it to everything would make the whole
process look like Windows 2000 to buy a verb almost nobody uses.

These are DECIDED ABSENT, having gone through both doors and found neither
open:

* **`checkbox.color`** — a check box's box is drawn by the theme, and the
  classic renderer draws it in `COLOR_WINDOW`/`COLOR_WINDOWTEXT` with no
  message to change either. `WM_CTLCOLORBTN` reaches the box's SURROUND, not
  the box. Owner-drawing a check box means reproducing the check mark, the
  indeterminate state and the focus rectangle to answer one colour.
* **`tabs.bar_background` / `tabs.bar_background_color`** — answered, since the
  strip is drawn here (see the class table). Listed only so a reader looking
  for them in a "themed control" argument finds them.
* **`icon_button.is_opaque`** — a child window OWNS ITS PIXELS. The parent does
  not paint underneath it, so a button that skips its own erase does not become
  transparent — it keeps whatever it drew last, and a glyph that changes leaves
  the old one behind. That is the identical failure the duplicate label had.
  The background is therefore ALWAYS painted, in the inherited colour the node
  would have shown through to. facet_gtk records the same verb absent for this
  kind from the other side.
* **`popup.item_enabled`** — a combo box has no per-item enabled state. The
  list is a `LISTBOX` internally and `LB_SETITEMDATA` carries no such flag;
  owner-draw can grey an item's pixels but cannot stop it being chosen, which
  is a worse answer than none.
* **`time_picker.is_open`** — a `SysDateTimePick32` in a TIME format has no
  drop-down at all. It is a spin field: `DTM_GETMONTHCAL` answers null forever
  and F4 does nothing, because there is no calendar to open. `date_picker.is_open`
  IS answered — see below.
* **`text_field.clear_button`** — there is no clear button on an `EDIT`. The
  one in Explorer's box is Explorer drawing over the control, the same as the
  magnifier in the entry below.
* **`search_field.search_icon_color` / `search_field.cancel_button_color`** —
  colours of two things that do not exist here, for the reason the entry below
  gives.
* **`menu_item.is_destructive` / `context_menu_item.is_destructive`** — a Win32
  menu item has no destructive style. `MFT_OWNERDRAW` would let this package
  paint one red, and a red item in an otherwise system-drawn menu reads as a
  rendering fault rather than as a warning.

### The handlers a control does not report

A prop this package can only write is a gap on one side; a HANDLER it cannot
hear is a gap on the other, and the parity tool counts them separately for that
reason. These are decided absent.

* **`time_picker.on_opened` / `on_closed`** — the same wall as
  `time_picker.is_open` above. A `SysDateTimePick32` in a time format has no
  drop-down to open, so there is no edge to report.
* **`swipe_item.on_invoked` and the five `swipeable` verbs** — a swipe is a
  TOUCH gesture. `input::on_mouse` does synthesise one from a press and a
  distant release, and that is what `.gesture(on_swipe:)` rides — but
  `swipeable`'s verbs describe a REVEAL that tracks the finger continuously,
  with an open state, a close request and a changing observation. A mouse drag
  can be made to imitate it and the imitation is worse than the absence: an
  application built against a tracking reveal on a desktop would have every
  list row shifting under a stray drag.

And these are §2 — Win32 has them and this package has not built them:

* **`on_submit` on the three input kinds** — an `EDIT` does not notify its
  parent when Enter is pressed; the dialog manager turns it into `IDOK` for a
  default button, which a facet tree has none of. Reading it needs the control
  SUBCLASSED, which this package does not do anywhere yet — it is the same
  missing machinery `tabs.bar_background` would have needed before the strip
  was drawn here, and the same machinery `text_area.on_selection_changed`
  wants (`EN_SELCHANGE` only reaches a RichEdit).
* **`split.on_move`** — the splitter drag. `split` places its two panes and its
  divider is not yet a draggable window.
* **`carousel` and `reorder`** — neither kind is built.
* **`web` / `hybrid_web`** — WebView2, which is a separate SDK.

### Rich text: `label.formatted_text`, `label.text_format`, `text_area.style_runs`

A `STATIC` and an `EDIT` draw ONE run: one font, one colour, one alignment, for
the whole string. There is no attributed string in user32 — a run of bold in
the middle of a label has no spelling at all.

The control that does have it is `RICHEDIT50W`, and it is not the answer here
for two reasons rather than one. It is a separate module (`msftedit.dll`) with
its own text object model, so a label backed by one is a different control with
different metrics, different focus behaviour and a different accessibility
role than every other label in the tree. And the verbs describe SPANS over a
string facet owns; RichEdit wants RTF or an `ITextDocument`, so every set would
be a serialisation.

`label.text` and every typography verb ARE answered — one run, stated
properly. What is absent is more than one of them.

(`text_area.on_selection_changed` is the same wall from the other side:
`EN_SELCHANGE` is a RichEdit notification and a plain `EDIT` never sends it.)

### `popup.text_color`, `popup.text_align`, `popup.title` — owner-draw is not available to a combo box here

These three WERE implemented, with `CBS_OWNERDRAWFIXED`, and the implementation
was reverted. The reason is worth the space because it constrains every future
owner-drawn control in this package.

**A combo box records its owner at creation and never asks again.**
`WM_MEASUREITEM` is sent once, from inside `CreateWindowEx`, and `WM_DRAWITEM`
goes to the owner recorded at that moment — not to whatever `GetParent` answers
later. This renderer's `create` does not know the host: `mount` gives it at
`insert`, so a fresh control is parked on `HWND_MESSAGE` and reparented after.
A message-only window discards both.

Measured, in the probe: the style word read `0x44010213` (`CBS_DROPDOWNLIST |
CBS_OWNERDRAWFIXED | CBS_HASSTRINGS`), `CB_SETITEMHEIGHT` returned 0 — success —
for both the selection field and the rows, and across a whole run there were
**three** `WM_DRAWITEM`, all `ODT_BUTTON`, and **zero** `ODT_COMBOBOX` and zero
`WM_MEASUREITEM`. The control was created, styled, sized correctly, and asked to
draw nothing. It rendered as an empty box.

Parking new controls on the first real window instead of `HWND_MESSAGE` was
tried and changed neither count.

**Why the owner-drawn BUTTONS are fine.** `text_button`, `icon_button` and
`toggle` are `BS_OWNERDRAW`, which needs no `WM_MEASUREITEM` — a button is sized
by its window rect — and their `WM_DRAWITEM` does arrive. Owner-draw is
available to this package for controls that do not measure ITEMS; it is not
available for the ones that do (combo box, list box, menu). That is the line.

So `popup` is the platform's own themed `CBS_DROPDOWNLIST`, which is also what
a Windows dropdown should look like. It shows the selected item in the theme's
colour and alignment; `text_color` and `text_align` are not settable, and
`title` and `title_color` — the prompt shown when nothing is selected, and its
colour — have nowhere to be drawn.
`CB_SETCUEBANNER` is not an alternative: it needs the edit control that a
`CBS_DROPDOWNLIST` does not have.

`popup.is_open` and `selected_index` are unaffected and still answered —
`CB_SHOWDROPDOWN` and `CB_GETDROPPEDSTATE` are ordinary messages.

### `carousel.bounces`

The rubber-band at the ends of an overscroll. It is a TOUCH affordance — the
finger keeps going, the content follows and springs back — and there is no
overscroll on a desktop to rubber-band against: a wheel notch past the last
page is not a gesture with distance, it is the end of the list. facet_gtk
records the same verb absent for the same kind, which is why it is here rather
than approximated.

(The scroll-anchor verb was recorded beside it and is answered now — see
`recycler::apply_scroll_anchor` and `controls::apply_carousel`. It was written
down as debt because it needs the state from BEFORE a splice, which looked
like it needed the carousel to watch its child list; it needs only the splice
fields the recycler was already being handed. Its name is not in backticks
here on purpose: `parity.py` reads a backticked prop name in §1 as a claim of
absence, and cannot tell a claim from a mention.)

### `date_picker.is_open` — opening and closing are NOT symmetric

Worth its own note because the asymmetry looks like a bug in this package.

Closing has a message: `DTM_CLOSEMONTHCAL`. **Opening does not.** There is no
`DTM_OPENMONTHCAL`, and the only documented way in is the keyboard gesture the
control already answers — F4. Synthesising that key is not a trick around a
missing API; it IS the API, in the sense that it is the one input the control
defines for this.

The state is ASKED rather than remembered (`DTM_GETMONTHCAL` answers the
calendar's window, or null). A user can dismiss the calendar by clicking away,
and a cached flag would then send F4 to reopen a picker the application
believes is already open. `popup.is_open` is asked the same way, through
`CB_GETDROPPEDSTATE`.

### A `search_field` IS A PLAIN EDIT

Windows has no search-field class. The magnifier and the clear button in
Explorer's box are Explorer's own drawing over an ordinary `EDIT`, not a
control anyone can create — there is no `WC_SEARCHBOX`. So a `search_field`
here behaves like a search field and does not look like one: the placeholder,
the text and `on_text_changed` all work, and there is no magnifier.

### A `spinner` IS A BAR, NOT A RING

Win32 has no circular activity indicator. `PBS_MARQUEE` — the barber's pole —
is what every shell dialog uses for "working, no idea how long", and it is a
horizontal strip. An application expecting the small ring the other three
backends draw gets a bar in the space it reserved.

### A `slider` RUNS OVER 1000 INTEGER STEPS

`TBM_SETRANGEMIN` / `TBM_SETRANGEMAX` take a signed 32-bit position; there is
no float spelling. facet's slider is `f64` from minimum to maximum, so the
control is given a fixed 0..1000 range and the value is mapped in and out
(`controls::slider_pos_of` / `slider_value_of`).

The narrowing is a quantisation: a slider over 0..1 resolves to 0.001. That is
past what a pointer can express on any real track — a 300-pixel slider has 300
reachable positions — so it is invisible to a hand and visible only to an
application that sets a value and reads it straight back.

### A radio group is EXCLUDED BY US, not by Win32

Not a narrowing — a note about where the behaviour lives, because the obvious
reading of the code is wrong.

Win32 groups radio buttons by ADJACENCY: `WS_GROUP` marks the first of a run and
every sibling after it belongs to that run. facet groups them by NAME
(`RadioProps.group`), which need not follow the child order at all. So
`BS_AUTORADIOBUTTON` is used for the look and the click, and `input::fire_radio`
clears the rest of the named group itself by walking the window's tree.

Leaving it to the platform would give the right answer whenever a designer
happened to put a group's members next to each other, and the wrong one the day
they did not — correct in every test anyone would think to write.

### `SysListView32` CANNOT HOST A FACET ROW

The one row here that is about a control this package deliberately does NOT
use, and it is worth stating because the control looks like exactly the right
answer.

`LVS_OWNERDATA` is genuinely virtual — the list view asks for row N when it
needs to draw it, which is `set_row` / `set_count` under another name. But what
it asks for is TEXT, through `LVN_GETDISPINFO`, and a facet row is a NODE TREE:
an arbitrary subtree with real controls in it. A list view cannot host child
windows inside its rows at all. `LVS_OWNERDRAWFIXED` moves the problem without
solving it — `WM_DRAWITEM` hands over a device context, which is a place to
paint and not a place to put a button.

So `recycler.cplus` builds the virtualisation itself over an ordinary scrolling
panel. That is the one place this package does MORE work than its siblings
rather than less: NSTableView, GtkListView and RecyclerView each own the
scrolling, the pooling and the recycling, and here only the scroll bar is the
platform's.

**Measured**, with `FACET_WIN32_ROWS=1` on `playground/win32_probe`:

```
[rows] model=20000 visible=0..11 cells=12 live=12
```

Twelve cells for a twenty-thousand-row model, in a viewport that holds eleven
rows plus one of slack. The number does not move with the model — which is the
whole property, and the reason the diagnostic exists rather than an adjective.

For comparison, `facet_gtk/HANDOFF.md` ends on the same measurement from the
other side: GtkListView keeps 205 cells for a viewport holding seven, and that
handoff records that guessing at it from outside did not help. Owning the pool
is what makes the number small here; it is also what makes every bug in it ours.

### A GESTURE ON A SYSTEM CONTROL IS NOT DELIVERED

`.gesture(on_click: …)` works on a PANEL — a container, a card, anything this
package gave its own window class to. It does not work on a `button`, a
`text_field`, a `popup` or any other system control.

A Win32 control has its own WNDPROC and keeps the mouse to itself: what reaches
the parent is the control's own notification (`BN_CLICKED`, `EN_CHANGE`), not a
pointer event. Subclassing every control to steal its messages would work and
would also break the control — a BUTTON that does not see its own
`WM_LBUTTONUP` does not draw its pressed state or fire its own action.

Not a hole so much as the place facet's own model already points: a control's
own verb is what an application should use there. `button(on_click:)` rather
than `.gesture(on_click:)` on a button — and `gestures.cplus` argues the same
thing from the portable side, that a button's action fires from the keyboard
and from a screen reader while a recognizer sees only the pointer.

### `on_key` REPORTS THE KEY, NOT THE CHARACTER

`key_code`, `key_modifiers` and `key_named` are answered; `key_chars` returns
empty and this is a decision rather than an omission.

The character a key produces is `WM_CHAR`'s business, not `WM_KEYDOWN`'s. They
are different messages and only the first has been through `TranslateMessage`
and the keyboard layout — so answering from a virtual-key code here would be
guessing at a layout, and guessing wrong on every non-US keyboard. An
application that wants typed text reads it from the field that received it.

### A LABEL OVER A NON-FLAT BACKGROUND GETS THE FLAT COLOUR UNDER IT

A control erases before it draws, and what it erases with is a BRUSH — one
colour. So a label sitting over a node that paints a gradient or a rounded card
erases with the flat colour resolved up the window chain, not with the pixels
actually beneath it.

This row is the residue of a BUG, and the distinction is worth keeping. The
code used to answer such a label with `TRANSPARENT` plus the HOLLOW brush,
reasoning that it must let the card through. A hollow brush paints nothing, so
the control drew its new string on top of its old one: clicking a counter
produced "clicks: 9" and "clicks: 10" superimposed, which reads as a duplicated
label and a broken font rather than as a missing erase. Found by a human
looking at the screen, which is the only thing that could have found it.

Correct erasing came first. Sampling the real pixels underneath needs the
parent to paint into an offscreen bitmap the child can then blit from, which is
`WS_EX_TRANSPARENT` plus a double-buffered parent — reachable, not built.
### The WM_CTLCOLOR brush table is BOUNDED

`paint::brush_for` caches a brush per distinct colour, up to 32. A brush handed
to `WM_CTLCOLOR*` must outlive the message — Windows keeps using the handle
after the WNDPROC returns — so it cannot be created and freed per call, and a
cache with an eviction policy would be a use-after-free with extra steps.

A thirty-third colour falls back to the control's own default. Bounded by the
PALETTE rather than by time, which is what makes it safe; a tree with more than
32 distinct control background colours would see the excess drawn in the
system's colour rather than facet's.

---

### A PINCH IS CTRL-WHEEL, because a mouse cannot pinch

`on_pinch` names a two-finger gesture. On a desktop the thing users actually do
to zoom is ctrl-wheel — every browser, editor and viewer on the platform — so
that is what fires it. A node asking for pinch gets the interaction its users
will attempt rather than one that never arrives.

A real touch pinch comes through `WM_GESTURE`, or `WM_POINTER` on the newer
input stack. Both are a different door and neither is bound; §2 carries the row.
Same for `touch_points`, which has no meaning while the only pointer is a mouse.

### WHAT THE AGENT CAN SEE, and the one thing it cannot

Measured against `playground/win32_probe` over the live socket, not reasoned
about. `describe_tree` walks facet's own node tree and returns every kind this
package builds — button, carousel, checkbox, collection, date_picker,
icon_button, image, label, list, popup, progress, radio, slider, tabs,
text_field, toggle — with real keys, kinds and frames.

**IT RETURNS NO ROWS.** Not the collection's, not a twenty-thousand-row list's,
not a tree's. A virtualised row is built by `recycler.cplus` and put on screen
with `mount::realise`, which is the SECOND mount path: it creates the row's
views without making the row a CHILD of the list node. `inspect_tree` walks
`core::child_of`, so a row is not on the walk.

**This is facet's shape and not this backend's.** facet_gtk's recycler says the
same thing in its own words — "a realised row is not in a window and `sync`
will never reach it" — and reaches rows through `sync_from` for exactly that
reason. Every backend's agent sees a list and not its contents.

What it means in practice: an agent can find a `list`, read its props and
scroll it, and cannot name or click a row through `describe_tree`. The
platform reader (`describe_ui`, `agent_win32`) walks HWNDs instead and a row IS
a real window there — so the two doors see different things, which is worth
knowing before trusting either alone.

Not recorded as a gap in this package because no verb here is unanswered by it;
recorded because "the inspector works on Windows" is true and would be
misleading without it.

### THE GESTURE BAND IS NOT IN THE PARITY NUMBERS AT ALL

Worth stating plainly, because the handler percentage looks like it covers this
and does not.

`parity.py` measures "handlers declared across facet's **Props structs**" — the
per-kind verbs like `button.on_click` and `list.on_item_selected`, plus the
shared band's `on_focus` / `on_blur`, which live on `CommonProps`. The
`Gestures` struct is a separate declaration and no column counts it.

So the state of this band is read HERE and nowhere else:

| verb | state |
|---|---|
| `on_press` / `on_release` / `on_click` | wired |
| `on_double_click` | wired, and falls back to `on_click` when a node declares only that — `CS_DBLCLKS` sends WM_LBUTTONDBLCLK INSTEAD of the second WM_LBUTTONDOWN, so a node with one handler would otherwise see one click where the user made two |
| `on_right_click` | wired, after the context menu has had its chance |
| `on_hover` / `on_unhover` | wired — Windows has no enter or leave message, so entry is the first move inside and leave is asked for with `TrackMouseEvent`, once per entry |
| `on_pointer_move` | wired |
| `on_pan` | wired — a move with the button down, past `SM_CXDRAG`. The slop matters: every click jitters a pixel between down and up, and without it every click is also a drag |
| `on_swipe` | wired — a release far enough from its press, and it CONSUMES the click, because a gesture that ended somewhere else was never a click on anything |
| `on_long_press` | wired, on a timer set on the pressed window. Suppresses the click that would otherwise follow — firing both is how a long press on a card also opens whatever a tap opens |
| `on_pinch` | ctrl-wheel; see above |
| `on_key` | wired on the focused window; `key_chars` is empty by decision (§1) |
| `on_drop` | wired, for FILES — `WM_DROPFILES`, and the three sender readers (`dropped_text`, `drop_position`, `drag_targeted`) with it. See `dnd.cplus` and the entry below |
| `on_drag_over` / `on_drag_leave` | NOT wired — the shell sends ONE message, at the drop, and nothing while the pointer is travelling |
| `on_drag_start` / `on_drop_completed` | NOT wired — the drag SOURCE half is `DoDragDrop`, which is COM |

The one-mouse assumption is stated rather than hidden: the press position, the
moved flag and the long-press latch are statics, not a table keyed by pointer
id. A second simultaneous pointer is `WM_POINTER` territory and a different
input stack.

### `text_button` IS OWNER-DRAWN, and that is an accessibility decision

The obvious implementation is a `STATIC` with `SS_NOTIFY`: clickable text, no
chrome, one line, and it answers colour through WM_CTLCOLORSTATIC and takes a
font. Nearly every prop this kind declares.

What a static cannot do is HOLD FOCUS. It is not a tab stop and never receives a
keyboard activation, so a text button built that way is unreachable without a
mouse — an accessibility regression rather than a cosmetic one.

`BS_OWNERDRAW` keeps everything a BUTTON is — focus, tab order, space and enter,
the accessibility role — and hands the parent WM_DRAWITEM to paint. This package
already resolves fonts and colours and measures text, so the drawing is the
smallest part.

Three things the owner-draw has to do that a themed control gets free, each
because nothing else will:

* **The background is the PARENT'S** — `paint::effective_bg`, the same
  inheritance a label uses. A text button has no chrome, which is the point.
* **The focus ring.** An owner-drawn button gets no focus indication at all, so
  a keyboard user cannot see where they are — which is the half this kind chose
  owner-draw to keep.
* **The pressed nudge.** One point down and right is the only press feedback a
  control with no chrome can give, and it is what a push button's own label
  does.

The default colour is `COLOR_HOTLIGHT` rather than window text: an unstyled text
button is the link posture, and that is the colour the shell uses for one.
Disabled overrides whatever the node asked for — a disabled control still
drawing in its accent colour reads as enabled.

**`BS_FLAT` is not the answer.** The theme engine ignores it and owns every
pixel of a themed control's frame, which is the same wall §1 records for
`border_color`.

### THREE DOORS FOR ONE IDEA — how a Win32 control reports

Not a narrowing; a fact a reader needs, because a backend has to answer all
three and nothing names them together.

| control | reports through |
|---|---|
| `BUTTON`, `EDIT`, `COMBOBOX`, `STATIC` | `WM_COMMAND`, with the sending HWND in lParam |
| trackbar | `WM_HSCROLL` / `WM_VSCROLL`, with the HWND in lParam |
| everything comctl32 added since — tab, up-down, list view, date picker | `WM_NOTIFY`, with an `NMHDR` |
| a MENU | `WM_COMMAND` with lParam **zero** and only a command id |

That last row is why `menus` keeps the one id table in this package: three of
the four carry their own identity and one carries a number.

Notification codes are NEGATIVE by convention — counted down from zero so they
cannot collide with a control's own messages — and arrive as an unsigned word,
so the comparison has to be built at the same width the message carries.

**`UDN_DELTAPOS` arrives BEFORE the move.** The notification is the up-down
control asking whether it may change, and its position is still the old one.
Reading it there answers the previous value on every step, which reads as the
stepper lagging one click behind.

### A SCROLL BAR DOES NOT AUTO-HIDE

`ScrollBars::Never` means no bar. `Default` and `Always` differ only in whether
the bar hides when there is nothing to scroll, and a Win32 scroll bar does not —
`SIF_DISABLENOSCROLL` greys it instead. So the two are the same answer here, and
an application asking for `Default` gets a bar that is always present and
sometimes disabled.

### `page_dots` and `bordered` ARE DRAWN, because there is nothing to draw them

Neither has a Win32 class: there is no page-indicator control, and `bordered` is
a stroke around arbitrary content rather than a widget. Both are a panel whose
WM_PAINT does more than the shared band.

This is the same owner-draw door §2's ceiling describes for controls, used where
there is no control to fight — so it costs nothing: neither kind has focus, a
caret or an accessibility role to reproduce.

`PS_INSIDEFRAME` is the detail worth keeping. A pen is centred on its path by
default, so half of a wide stroke falls outside the node's own box and is
clipped — a 6-point border drawing as 3, with nothing to explain it.
### The MOBILE INPUT verbs — `keyboard`, `return_key`, `predicts_text`, `checks_spelling`

Four `InputView` rows that describe a SOFT keyboard, and a desktop has none.

`keyboard` picks which on-screen layout appears (numeric, email, URL);
`return_key` labels its action key (Go, Search, Done); `predicts_text` and
`checks_spelling` are the suggestion strip above it. A Win32 `EDIT` receives
from a hardware keyboard that is always the same one, shows no strip, and has no
action key to label.

The near neighbours are all worse than nothing. Windows has a touch keyboard,
but a desktop app cannot choose its layout; `EM_SETCUEBANNER` is a prompt rather
than a key label; and spell-checking on an `EDIT` is the application's, not the
control's. Answering any of them would be inventing a behaviour the platform
does not have.

### PER-CONTROL TYPOGRAPHY — `character_spacing` and `line_height`

GDI has no per-control tracking or leading. `SetTextCharacterExtra` is a
DEVICE CONTEXT call, so it applies to whatever is drawn next rather than to a
control, and a system control acquires its own DC when it paints — there is
nowhere to put the value that the control would read.

Both are answerable by owner-drawing (§2's ceiling) or by DirectWrite, which is
a different text stack. `line_height` on a wrapping label is the one that would
be missed most.

### A CONTROL'S OWN BORDER — `border_color`, `border_width`, `corner_radius`

Distinct from the shared band's corner radius, which this package DOES answer on
a panel it paints itself. On a system control these are the same wall as
`opacity`: comctl32 v6 draws the control through the theme engine, which owns
every pixel of its frame and does not ask.

`WS_BORDER` gives a control a border but not a colour or a width, which is why
`text_field` wears one and `button` cannot be told to.

### `placeholder_color`

`EM_SETCUEBANNER` sets the cue banner's TEXT and nothing else. The colour is the
theme's grey; there is no message for it.

### `vertical_align` on an EDIT

A single-line `EDIT` centres its text vertically and a multiline one starts at
the top, and neither is adjustable. The verb is answered for a `label`, where
`SS_CENTERIMAGE` genuinely does it.

### A `list` cannot be reordered, and that is facet's shape rather than this package's

Worth recording because the verb LOOKS present on both kinds and is not.

`ReorderableItemsViewProps` — which carries the capability flag and the
completion handler — is embedded in `CollectionProps` and in nothing else. A
list has the two read-only "what moved" fields, and its reorder bit is
declared, but there is no way to ask a list to be reorderable and no handler to
hear that it was. A drag there would be a gesture with no contract behind it,
so a collection reorders and a `list` does not. facet_gtk reads
`CollectionProps` and only that, for the same reason.

**The names are deliberately not in backticks above.** `parity.py` reads a
backticked prop name in §1 as "this backend decided it absent", and it does not
know which KIND the sentence was about — so naming a collection's verbs while
explaining a list's absence marked them absent on the collection too, where
they are implemented. Two verbs read as gaps for no reason but this file's
prose.

Reordering here is mouse capture rather than OLE drag-and-drop, and the COM
note in the entry below does not apply to it: a reorder never leaves the
window, so there is no data object and no cross-application format to
negotiate. The move is REPORTED and the data is not touched — facet's sequence
is a count plus a builder, so the order belongs to the application; a backend
that shuffled rows itself would be shuffling a view of data it does not own,
and the next rebuild would put them back.

### `collection.columns`

More than one item per row. facet_gtk answers it by handing the model to a
`GtkGridView`, which lays the items out itself; there is no such control here,
so `recycler.cplus` would have to do it — and the layout is the easy half.

Two things stop it being a small change, and the second is the real one.

`bind` TAKES ONE NODE. `set_row_bind` is `fn(index, *Node, ctx)` — a row and
the item that goes in it — so a row holding three items has nothing to rebind
through and every scroll would REBUILD instead of recycling. That is a
performance loss rather than a correctness one, and it would be acceptable.

SELECTION WOULD BE WRONG. A cell carries one item slot (`set_item_index`), and
`on_row_click` turns the clicked window back into a row index through it. With
three items in a cell, clicking the second reports the row — so a click selects
the wrong item, and reports it confidently. Fixing that means per-item hit
resolution inside a cell, which is a second addressing scheme beside the one
`item_of` already defines.

So this is §2 with a shape attached rather than a line of debt: the work is a
cell that holds N items and an item slot that survives it, not a loop over
columns.

### The drag half of drag-and-drop, and the two travelling edges

`dnd.cplus` answers the receiving side through `WM_DROPFILES`: a node with an
`on_drop` becomes a shell drop target, a dropped file's paths arrive as text
(one per line, so three files is three lines rather than a container facet does
not have), and the position comes back in the node's own coordinates.

Three of facet's five drag verbs are not answered, in two groups with different
reasons.

**`on_drag_over` and `on_drag_leave`.** `WM_DROPFILES` has no travelling edge —
the shell sends one message, at the drop. There is nothing to report while the
pointer is on its way, and inventing one from `WM_MOUSEMOVE` would fire for a
pointer that is not dragging anything.

**`on_drag_start` and `on_drop_completed`.** These are the drag SOURCE, which is
`DoDragDrop` — and that means an `IDropSource` and an `IDataObject`, both COM
objects this package would have to lay out by hand: a vtable of function
pointers, refcounting, and `IDataObject::GetData` reached through two more
vtable calls.

**Why `IDropTarget` was not taken for the whole thing.** It would answer all
five and every format, and it is the same COM cost. This package has no COM
anywhere — `imaging` chose GDI+'s flat C API over WIC on the same grounds — and
introducing it should be a decision made on its own merits rather than as a
side effect of wanting two edges. What it would buy is written here so that
decision can be made with the price in view: five verbs, plus dropped TEXT
(not only files) and cross-application drags of anything.

### The SHARED BAND's five remaining bits, each for its own reason

16 of 21 are answered. The five that are not are here, because a band bit is a
verb EVERY node has — a gap in one is a gap everywhere, which makes it worth
more words than a per-kind prop.

**`C_ANIMATE`, `C_CANCEL_ANIMATIONS`, `C_TRANSFORM`** — the same root cause as
the `opacity` row above, and it is worth stating as one thing: a child HWND has
no alpha and no transform. There is nothing to interpolate TO. An animation
tier over values a control cannot wear would run its timer, compute its curve,
and write results nothing could display — which is worse than not having one,
because the app would believe it was animating.

They become answerable exactly when controls are owner-drawn, which is §2's
ceiling. On a PANEL — which this package paints itself — a transform is
reachable through `SetWorldTransform` and an opacity through `AlphaBlend`, and
that is the half worth building first if anyone needs it.

**`C_SAFE_AREA`** — a Win32 desktop window has no such inset. The verb exists
for a notch, a home indicator, a status bar overlaying the content; a window on
this platform is given a client rectangle that is already only what it owns.
facet_gtk records the same absence for the same reason ("a GTK window under a
compositor has no such inset").

**`C_FLUSH`** — nothing to do, and no backend names it. It is the command that
closes a batch, and this package's `touch` already coalesces through the
scheduler's pending flag, so the batch ends when the queue drains.

### The parity tool is held to THIS ledger now, and what that changed

This entry used to say the cross-check was not portable — that `parity.py`
read `facet_gtk/MANIFEST.md` whatever column it printed, so its "nothing
unanswered is unrecorded" line was a statement about GTK, and the rows here
were held to the doctrine by hand. It called making the check per-backend "a
small change to a shared tool and the honest fix".

It was made. `parity.py <backend>` now reads each package's own §1, prints that
package's per-kind table, and grades its band against its own ledger. The
closing line covers this file.

Two things surfaced the moment it did, and both are worth keeping in view:

**Other backends have unrecorded gaps** that nothing had been measuring —
appkit does not name or argue `C_FLUSH`, and uikit does not name or argue three
band bits. Not this package's to fix; recorded because a tool that only graded
one backend is why they went unseen.

**A NAME IN §1 IS A CLAIM, and the tool cannot tell it from a mention.** It
takes every backticked identifier here as "decided absent", and it has no
notion of WHICH KIND a sentence was about — the leaf is all it keeps. So an
entry explaining that a themed control cannot have a border marked the border
verbs absent on `text_button` too, which is owner-drawn and can simply be given
one; and an entry saying a verb WAS answered claimed the opposite by naming it.
Four verbs read as gaps for no reason but this file's prose.

The rule that follows: name a verb in §1 only where the sentence is a decision
ABOUT THAT VERB across this backend. When a kind is the exception, describe it
without backticks.

## 2. Not built yet — Win32 has it, this package has not reached it

This is DEBT, not decision. Every row here is reachable with what user32,
gdi32 and comctl32 already offer.

### Most controls, still

Twenty-eight kinds have bodies: `label`, `button`, `text_button`, `checkbox`,
`toggle`, `text_field`, `text_area`, `search_field`, `radio`, `slider`,
`progress`, `spinner`, `popup`, `stepper`, `tabs`, `date_picker`,
`time_picker`, `page_dots`, `bordered`, `list`, `table`, `collection`, `tree`,
`menu`, `menu_item`, `context_menu`, `box`, `scroll`. **Every other kind gets a plain panel** —
it holds its children and honours the shared band, which is more than a null
window and less than a claim.

The classes are all there and all initialised (`InitCommonControlsEx` with
`ICC_WIN95_CLASSES`):

| facet kind | Win32 class |
|---|---|
| `stepper` | `msctls_updown32` |
| `date_picker` / `time_picker` | `SysDateTimePick32` |
| `slider` | `msctls_trackbar32`, with `NM_CUSTOMDRAW` for its three colours |
| `progress` / `spinner` | `msctls_progress32` |
| `popup` | `COMBOBOX` (`CBS_DROPDOWNLIST`), the theme's own — see §1 |
| `text_button` / `icon_button` / `toggle` | `BUTTON`, `BS_OWNERDRAW` — see §1 |
| `symbol` | `STATIC`, drawing a codepoint in the bundled icon font |
| `tabs` / `page_dots` / `bordered` | GDI in the panel's own WM_PAINT |
| `list` / `table` / `collection` / `tree` | a scrolling panel and `recycler.cplus` |

**Two of these rows used to name a control this package does not use, and both
were wrong in the same way — the class was plausible and the kind's own model
did not fit it.**

`SysTabControl32` was created for every `tabs` node and NEVER HAD AN ITEM
INSERTED, so it drew an empty strip with the panes laid out over it. The reason
no item was ever inserted is that facet's `tabs` declares no titles and no
pages: its panes are ordinary children and their titles are the children's
KEYS. A native tab control wants to own both. facet_gtk dropped GtkNotebook and
facet_appkit dropped NSTabView for this exact reason; this package now draws
the strip in its own WM_PAINT and shows one pane at a time through
`Display::None`, which is what the other two do.

`SysListView32` is argued at length above — the short of it is that
`LVN_GETDISPINFO` asks for TEXT and a facet row is a node tree.



### An HMENU is not a window, so menus need the one id table this package has

Every other control here is an HWND: a node is bound to it with `SetPropA`, and
`input` walks the window chain to get back to facet. An `HMENU` has none of
that — no properties, no window, no messages. It reports a click as
`WM_COMMAND` on the OWNING window with a command id in wParam and **lParam
zero**.

That zero is the whole difference, and it is also the test. A control
notification always carries its own HWND in lParam, which is why `input` needs
no id table; a menu carries only a number, so `menus` keeps a table from command
id to the item that owns it and the WNDPROC consults it exactly when lParam is
zero.

Ids start at `0x100`. Below that are Windows' own dialog answers — IDOK is 1,
IDCANCEL 2 — and a menu item numbered 2 fires whenever any dialog in the process
is cancelled, which only shows up once an app grows its first dialog.

**The bar is rebuilt whole, never edited.** An HMENU has no per-item update that
keeps ids stable, so editing one would leave the command table describing a menu
it no longer matches. `SetMenu` also does not free what it replaces, so the old
handle is destroyed explicitly or every rebuild leaks it and everything under it.

Two shapes reach this, and both are answered:

* **`screen::MenuItem`** — a FLAT list where each item names its own group by
  string. Grouped here in first-seen order, which is the order the application
  wrote them in. This is the path `runtime::App` uses and what real apps write.
* **`menu` / `menu_item` NODES** — viewless kinds sitting in the tree, built
  into an HMENU at mount. A nested `menu` becomes a submenu, which is the one
  place the tree shape and the HMENU shape agree.

A declared `menu` subtree WINS over the app menu when a screen has both: it is
the more specific statement.

**A context menu is one call.** `TrackPopupMenu` with `TPM_RETURNCMD` blocks
until the user chooses and answers the id, so there is no menu to keep alive
between showing and handling — which is also why its ids only have to be valid
for the duration of that call. A node with both a `context_menu` and an
`on_right_click` gets the menu: the handler is what a node says when it has no
menu to show, and a dismissed menu still counts as handled rather than falling
through to fire something the user just cancelled.
### The tree is a SECOND MAP, not a second recycler

`SysTreeView32` is unused for the same reason `SysListView32` is: it cannot host
a facet row, which is an arbitrary subtree of real controls.

So `tree` runs on the sequence's machine with a different map. The division is
worth keeping and is the one `facet_gtk/recycler` names: **a list is count plus
index, a tree is parent → children plus expansion plus stable identity.** The
tree map is a pre-order walk that descends only into expanded branches;
everything below it — cells, pool, visible window, scroll bar, placement — is
shared and never asks which map filled it.

Three questions fork (`row_shape`, `make_row`, `rebind_cell`) and nothing else
does. Measured on the probe: three collapsed branches are `model=3`, and
expanding one of four leaves gives `model=7`.

Two differences that are contract, not implementation:

* **A tree selects by IDENTITY.** `TreeProps.selected` is an id, because a
  flattened index changes the moment a branch above it expands. A list's is an
  index and stays one.
* **A tree's `bind` takes the NODE and orders its arguments differently** —
  `(node, ctx, row)` against a list's `(index, row, ctx)`. Two verbs that mean
  the same thing and cannot share a call.
### Fonts — DONE

`fonts.cplus` answers `font_size`, `font_weight`, `font_family` and `italic`,
cached by spec and pushed with `WM_SETFONT`. The probe's 18-point bold title
measured 38x16 before it and 293x32 after, which is the whole of what this row
used to say.

What is NOT answered is `font_scales` — the accessibility text-scale multiplier.
Windows exposes it through `SystemParametersInfo(SPI_GETTEXTSCALEFACTOR)` on
recent builds; nothing binds it yet.

### The read half — what is left of it

The per-kind verbs and the whole gesture band are wired; §1 has the table.
What remains:

* **Drag and drop** — five gesture verbs and three sender readers, all needing
  `IDropTarget`. The readers are left ZERO rather than filled with empties,
  because `component::SenderReaders` reads a zero field as "no backend told me"
  and a filled one answering nothing would be a lie.
* **A real touch pinch** — `WM_GESTURE`, or `WM_POINTER` on the newer input
  stack. Ctrl-wheel answers the desktop case today (§1).
* **`key_chars`** — needs the `WM_CHAR` half of the key stream, which is a
  different message from the `WM_KEYDOWN` the band routes and the only one that
  has been through the keyboard layout (§1).
### Drag and drop

The three sender readers (`dropped_text`, `drop_position`, `drag_targeted`) are
left ZERO, which is not the same as half-wired: `component::SenderReaders`
declares a zero field as keeping facet's portable default. A zero reader answers
"no backend told me"; a filled one answering empty would be a lie. `IDropTarget`
is what fills them.

### The wheel's scroll-lines setting

Three rows a notch is stated in `input::wheel_lines`. Windows carries the
user's own answer in `SystemParametersInfo(SPI_GETWHEELSCROLLLINES)`, including
the "one screen at a time" setting, and nothing reads it yet.

### The scroll actually scrolling

`controls::create_scroll` builds the viewport and the document and parents them
correctly, and `views::insert` routes children into the document. What is
missing is the scroll bars themselves (`WS_VSCROLL` plus `SetScrollInfo`), the
`WM_VSCROLL` handling that moves the document, and the content extent that says
how far it goes.

### `toolbar_item` and `window_chrome`

The two menu-adjacent kinds that are still panels. A toolbar is a rebar or a
`ToolbarWindow32`; `window_chrome` is the caption buttons, which on Windows are
the system's unless the window is frameless.

Menus themselves landed — see §1's HMENU row — and so did dialogs and the
runtime facade, which this row used to say blocked everything.

### The ceiling, stated once

Everything in §1's first row — opacity, transform, a real switch, per-corner
radii on a control — comes back if the controls are OWNER-DRAWN rather than
native. That is a real option and it is not free: owner-drawing means
reimplementing focus rings, the caret, IME composition, selection painting and
the accessibility tree, which is what `facet_appkit/text_input.cplus` (1,671
lines) and `facet_appkit/recycler.cplus` (3,124 lines) exist to do.

The judgement here is the one `facet_appkit/MANIFEST.md` states as its bar: **a
control does not have to be one native widget.** Sixty-odd rows left AppKit's
cannot-ledger once the answer was allowed to be built from two classes. The
same move is available here — a native control with a painted panel behind it
answers more of the band than either alone — and it should be tried per verb
before anything is owner-drawn wholesale.

---

## 3. Notes a reader will want

**Z-order IS the child order.** There is no child list to splice into: a Win32


### THREE KINDS, THREE PROPS STRUCTS, ONE CAST — the worst bug in this port

Recorded at length because it compiled, ran, passed every probe, and would have
called a garbage function pointer.

`recycler::list_props` accepted `K_LIST`, `K_TABLE` and `K_COLLECTION` and cast
all three to `*ListProps`, on a comment asserting "their props structs share
this prefix". They share no prefix at all:

```
ListProps        has_uneven_rows: bool, horizontal_scroll_bars, ..., count, row
CollectionProps  five EMBEDDED structs, then count, row
TableProps       has_uneven_rows, style, row_height  -- and nothing else
```

So a `collection` read its `count` out of whatever a `ScrollBars` enum and four
bools happened to occupy, and read `row` from the middle of an embedded struct
**and called it**. A `table` is worse: `TableProps` is three fields, and
`.count` reads past the end of the allocation entirely.

It never crashed in testing because the probe only ever mounted a `list`, where
the cast happened to be correct.

**And a table is not a row source at all.** It has no `count` and no `row`
because its rows are ordinary facet CHILDREN — `table` only imposes a height on
them. Routing it through the recycler was wrong before the cast was.

Two fixes, and the second is the one that matters:

* `table` has its own body now: a panel that walks its children and sets their
  height, which is what `facet_gtk::apply_table` does.
* The row verbs are resolved into a `SeqSource` VALUE by a function that reads
  the right struct for the kind. **No cast survives.** A kind that is not a row
  source answers `live: false` rather than a pointer, so adding a fourth kind
  cannot silently reinterpret its memory.

The general lesson is the one this manifest has now recorded three times: a
comment asserting two things are layout-compatible is not a check, and every
instance of it here has been wrong. `#[repr(C)]`-less structs from a GENERATOR
have no guaranteed relationship to each other whatever their fields are named.
### A generated module numbers its bits from ONE, in DECLARATION order

Not a Win32 fact — a facet fact, recorded here because getting it wrong cost
this package six kinds' worth of dead code and the comment that caused it read
as reassuring.

`fonts::font_bits` returned `label`'s constants for every kind, asserting that
"the per-kind constants are the SAME NUMBERS across kinds". They are not:

```
label       P_FONT_WEIGHT 2     P_FONT_SIZE 32
button      P_FONT_WEIGHT 32    P_FONT_SIZE 512
text_field  P_FONT_WEIGHT 128   P_FONT_SIZE 2048
```

Each module numbers from one in the order it DECLARES its props, and the kinds
declare different props, so a verb they share lands on a different bit in each.

The failure was exact and silent: label's mask is 62, a button's `P_FONT_SIZE`
is 512 and is not in it, so **changing a button's, field's, editor's, search
field's, popup's or radio's font after mount did nothing at all**. Create
worked, because create passes every bit — which is what made it look right.

Two rules follow, and the suite pins both:

* A per-kind bit is only ever named through ITS OWN module. `label::P_X` in a
  gate that runs for a button is a different verb wearing the same name.
* A kind named in a bit mask must be READ in the matching accessor. A kind in
  one and not the other either re-applies a font it cannot describe, or
  describes one nothing re-applies.

Worth generalising: any backend gating on `<some_kind>::P_*` for a node of
another kind has the same bug, and it cannot be seen at the call site.
parent's children are ordered by z, that order is the paint order, and it is
what `component::raise` moves a node through. `views::insert` sets it with
`SetWindowPos` naming the sibling to follow, and `geometry::place` passes
`SWP_NOZORDER` so a layout pass cannot undo it.

**The parent owns the event.** A Win32 control does not call you back; it sends
`WM_COMMAND` to its parent. So there is no per-control arming — `input::arm`
binds the node onto the window with `SetPropA` and the routing lives in one
WNDPROC. `WM_COMMAND`'s lParam IS the sending control's HWND, so this package
needs no per-control id table, unlike the `win32` package's own facade.

**Both clip flags on every host.** `WS_CLIPCHILDREN` keeps a panel's paint out
of its children's rectangles; without it every control flickers. `WS_CLIPSIBLINGS`
keeps overlapping children apart; without it z-order has no visible effect.

**A leaf with no measure callback is 0x0.** All four backends record finding
this by seeing nothing. Win32 has no `gtk_widget_measure` and no `fittingSize`,
so `measure.cplus` computes the size from the text in the control's own font
plus a stated per-kind padding. It is the one file in this package with
hard-coded numbers and each one says whether it is a platform metric or a
stated constant.

---

## 4. What blocks the GALLERY, which is not this package

`examples/facet_gallery` builds for Windows and does not yet LINK, and neither
remaining symbol belongs to facet_win32. Recorded here because the gallery is
the target every backend is measured against, and "the gallery does not run"
would otherwise read as a gap in this package.

**`agent_mcp`'s prebuilt archive carries POSIX-only test helpers.** The link
fails on `pipe`, `usleep` and `socketpair`, all referenced from two of that
package's own `#[test]` functions —
`read_line_reports_cancellation_instead_of_parking_forever` and
`write_all_to_a_departed_client_does_not_kill_the_server`. They are in
`libagent_mcp.a` and therefore in every application that links it, test or not.
Two things are wrong and only one is the tests': the helpers need a Windows
spelling (`netsys_windows` already has the loopback-TCP substitute for
`socketpair`), and a prebuilt library should not be exporting its test bodies
into consumers at all.

**`facet_agent` on Windows serves over TCP, not a Unix socket.** That part is
DONE — `agent_windows.cplus` reads the path as a port, the way `agent_ios.cplus`
does — but it is worth stating because an app that passes
`/tmp/facet-gallery.sock` gets port 8787 rather than an error, and a reader who
expects a socket file will not find one.

Neither is on this package's path: `playground/win32_runtime_probe` links and
runs through `runtime::App` with the agent tier left out, which is what proves
the facade independently of the above.

## 5. Fixed on the way through — other packages

Recorded because a port that quietly patches its dependencies is a port nobody
can review.

* **`facet` did not LINK on Windows at all.** `pthread_self` and `sched_yield`
  were reached directly from `facet.cplus`, `mount.cplus` and `services.cplus`.
  Split into `platform_sys.cplus` / `platform_sys_windows.cplus`, the same shape
  `stdlib/platform_sys` uses. It had to be a FILE override and not `#platform()`:
  sema compiles both branches of a `#platform()` comparison, so the extern would
  still have had to resolve.
* **`stdlib`'s Windows reactor was missing the non-blocking poll.**
  `stdlib_reactor_poll_one_event_nb_v1` is referenced unconditionally by
  `facet/services::pump_async`, so its absence broke every facet application on
  Windows. `reactor_windows.cplus` had the blocking poll only; it is now one
  body, `poll_one_event_within(max_wait_ms)`, with the blocking and probing
  spellings over it. The non-blocking path also had to learn not to call
  `fire_earliest_timer`, which SLEEPS to a deadline that has not arrived.
* **`agent_win32` had no policy pin.** `set_agent_policy` / `get_agent_policy`
  are what `facet_agent::pin_policy` calls for every node carrying a declared
  agent tier; without them a `Private` field was not private. Added over
  `SetPropA`, with agent_gtk's +1 encoding so an explicit `Open` is
  distinguishable from a node nobody marked.
* **`agent_win32::describe` dropped its grant.** The convenience wrapper took a
  `auth::Grant` and called `Surface::describe()` without it — which did not
  compile, and is how it was found.
* **`parity.py` could not run on Windows.** `open()` defaults to cp1252 there
  and every source file in this tree has UTF-8 em-dashes. Now explicit.

## 6. Visual styles, and the three things that lied about them

A human looked at a screenshot and said the app looked like Windows 95. It did.
None of the code was wrong; the EXECUTABLE was missing an application manifest,
and finding that out took an afternoon because every layer that could have said
so reported success instead.

Written down in full because each of the three is silent, each looks like the
code working, and the next person to touch a Windows binary in this tree will
meet at least one of them.

### The mechanism

A Windows process gets Common Controls **5.82** by default — the 1995 renderer.
The themed **6.0** is side-by-side and the loader binds it only when the binary
declares a dependency in an `RT_MANIFEST` resource. There is no runtime call
that asks for it and no way for a library to arrange it: it is a property of the
EXE, so it is the build system's job. `cpc` now compiles a default manifest with
`llvm-rc` and links the `.res` (see `run_clang` in `cpc/src/main.rs`).

### Lie 1 — `IsAppThemed()` is not the signal

The obvious diagnostic, and it is wrong. It answers 0 on a process that has
demonstrably loaded comctl32 6.0, and answered 0 on a C program that was drawing
themed controls at the time. Whatever it reports, it is not "will my controls be
themed".

**THE LOADED comctl32 IS THE SIGNAL.** 5.82 and 6.0 live at different WinSxS
paths, so the path is the version — which is what `window::report_comctl_path`
prints under `FACET_WIN32_THEME=1`:

```
[theme] comctl32 <- ...common-controls_..._5.82.26100...   classic
[theme] comctl32 <- ...common-controls_..._6.0.26100...    themed
```

### Lie 2 — `/manifest:embed` silently emits a stub

lld-link accepts `/manifest:embed` and `/manifestinput:` without complaint and
produces an `<assembly>` element with the dependency stripped out. Manifest
MERGING needs libxml2 and the LLVM Windows releases are built without it, so it
degrades to LLD's own empty default.

That is WORSE than doing nothing — an empty manifest still marks the process as
manifested. The first attempt at this fix made the problem harder to see, with a
clean link and both flags accepted.

`llvm-rc` writing a `.res` has no such dependency, which is why that is the path
taken.

### Lie 3 — a struct and a function sharing a name

`InitCommonControlsEx` is both the C struct tag and the entry point, and both
were declared in C+ under that name. The call resolved to the type. It compiled,
it returned 1, and comctl32 was never initialised. The struct also wanted
`#[repr(C)]`, which it did not have — comctl32 reads `dwSize` at 0 and `dwICC`
at 4, and a layout C+ chose freely would have initialised nothing just as
quietly.

Both are fixed (`IccEx`, `#[repr(C)]`) and both are worth knowing generally:
a C+ binding that reuses a C tag name is a call that goes somewhere else.

### What is still open

Whether embedding this manifest in EVERY Windows C+ executable is the right
default, or whether it should be an opt-in `[link] manifest = "..."` key. The
argument for the default is that a console program is unaffected — it creates no
controls to theme — and the DPI half is not cosmetic: without a declaration the
process is DPI-unaware, Windows scales its windows as a bitmap on a scaled
display, and `GetDpiForWindow` answers 96 everywhere so `fonts::height_for`
computes the wrong pixel height.
