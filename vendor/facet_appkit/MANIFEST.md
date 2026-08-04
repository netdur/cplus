# facet_appkit — the manifest

INTENT.md's rule: for each thing facet declares, this package either implements
it with what AppKit offers, or **states plainly that AppKit cannot**. This file
is where every "cannot" is written down, so anyone can answer "how does that
happen on a Mac?" by reading it.

A row here is a commitment, not a note. Nothing is left implicit: a verb that
is neither implemented nor listed below is a gap, and the gap is a bug.

Status: **Stage 4 item 1 complete**, item 2 at 30 kinds of 42. Items are in
progress; this file grows a row each time something is decided either way.

---

## AppKit cannot

The doctrine is the same one an UNSUPPORTED control verb follows: the contract
still declares it, because the contract is readable as a whole and a backend's
gaps are not the vocabulary's business.

### The control tint colours

| Verb | Why not |
|---|---|
| `toggle(on_color:)` `off_color:` `thumb_color:` | NSSwitch paints from the system accent colour and exposes no per-instance tint. |
| `slider(minimum_track_color:)` `maximum_track_color:` `thumb_color:` | NSSlider's track and knob are drawn by the cell; there is no colour API. |
| `progress(progress_color:)` | NSProgressIndicator's bar is the system accent colour. |
| `checkbox(color:)` `spinner(color:)` | Same: the mark and the spinner are system-drawn. |

A tinted layer under each was considered and rejected. It would stop tracking
the system appearance (the whole point of these being platform controls), and
it would drift the moment Apple changes the control's shape. An application
that must have a specific colour draws it itself with `box` and `gesture`,
which is a visible choice rather than a lie.

### `radio(group:)` does not name the group

AppKit groups NSButtons of type Radio **by superview**: radios sharing a
superview deselect each other automatically, and there is no group name.
facet's `group` is the portable model, so a group that does not match the
tree's shape is not honoured here. Put the radios of one group in one
container and the behaviour is what the contract says.

### One corner radius per layer

`corner_radius` carries four corners (`Corners { top_leading, top_trailing,
bottom_leading, bottom_trailing }`). Core Animation has one `cornerRadius`
per layer, so the **largest** of the four is used. Per-corner radii would
need a mask layer per view, which is real cost for a case no consumer has
asked for yet.

### The tab strip's colours

`tabs(bar_background:)` `bar_background_color:` `bar_text_color:`
`selected_tab_color:` `unselected_tab_color:` are not honoured. NSTabView
draws its own strip and offers no colour API — the same reason as the other
system-drawn controls above.

### `context_menu_item(destructive:)`

NSMenuItem has no destructive style — macOS marks a destructive action by
wording ("Delete…") and by confirming it, not by colouring the row. The flag
is carried and ignored rather than approximated with a red attributed title,
which would look like a system convention that does not exist.

### The `bordered` stroke family

`bordered(stroke_dash:)` `stroke_dash_offset:` `stroke_cap:` `stroke_join:`
`stroke_miter_limit:` `stroke_shape:` are not honoured. A layer border is a
solid rectangle of one width and one colour; dashes, caps, joins and an
arbitrary stroke shape are CAShapeLayer territory — a second layer per
bordered node, sized and re-pathed on every layout pass. `stroke` and
`stroke_width` ARE honoured, and a gradient `Brush` uses its start colour
(a gradient border needs the same mask layer).

### The touch-gesture controls

| Control | Why not, and what to use instead |
|---|---|
| `refreshable` | Pull-to-refresh does not exist on macOS. There is no scroll-past-the-top gesture to hang it on, and inventing one would fight every other scroll view on the machine. A desktop app puts a Refresh command on a menu or a toolbar button (⌘R by convention). |
| `swipeable` / `swipe_item` | Swipe-to-reveal-actions is a touch idiom. macOS reveals per-row actions through a **context menu** (right-click) or a hover affordance. |

Both are recorded rather than approximated because a half-working gesture is
worse than an absent one: it teaches an application a shape that will not
survive on the platform its users are actually on.

They still get a BODY, and it matters that they do: a plain backing view, so
their content renders exactly as it would have (a `swipeable` shows its
content and simply reveals nothing), and SILENTLY — a decided kind must not
take the unimplemented path, because that path warns, and a warning for
something already answered would train a reader to ignore the warnings that
mean something.

This is also where the standing rule bites — **an agent has no hands**. A
gesture-only affordance must have a click path in the UI, never a new agent
verb. On this backend that click path is the substitute above, not a
simulated swipe.

### Deferred, mobile-only

The mobile-only handful the tier ledger deferred — soft-input policy, the
nav-bar back button — have no desktop equivalent and land here as "AppKit
cannot (mobile concept)" when item 5 reaches them.

## Implemented, with a deviation worth knowing

### A span is not a view either

`span` is a styled RUN inside a label, so it has nothing to apply onto. A
label with span children renders one attributed string built from them and
IGNORES its own `text` — a formatted label's text is its runs, and its own
`text` would be a second, contradictory answer to the same question. A label
with no spans renders `text` exactly as before.

### The menu kinds are not views

`context_menu` and `context_menu_item` answer `wants_view` FALSE: an NSMenu
is not an NSView, and a context menu decorates the node it sits under rather
than occupying a place of its own. The mount walk creates nothing for them
and they take no place in the native tree — the menu is read from the NODES
when the PARENT's view is built, and hung on that view with `setMenu:`.

Consequence: a `context_menu` node has no frame, no `native()`, and no
geometry. It is a description that resolves into its parent, which is the
only reading that matches what a context menu IS.

### Every backing view is flipped

facet's tree is top-left origin, because flex is. AppKit's NSView is not, by
default. Rather than carry a flip through the frame walk — the pre-regen
backend threaded a `parent_flipped` flag through every level and computed
`parent.height - (child.top - parent.top) - child.height` for the bottom-up
case — every view this backend creates has `isFlipped` true, and the window's
content view is flipped too.

Consequence for an application that drops beneath facet: a native view you
add yourself lands in a **flipped** superview, so its frame is read top-left.
That is the opposite of a hand-built AppKit window and it is deliberate.

### `blur` resigns first responder to the window

There is no null first responder on AppKit: passing nil makes the window
refuse the change rather than clear it. `blur` therefore makes the WINDOW the
first responder, which is what "nothing in the tree is focused" means on this
platform.

### The sync tick is a CFRunLoopObserver at before-waiting

facet has no loop of its own; M5 says the backend coalesces sync requests on
its run loop. This uses `kCFRunLoopBeforeWaiting` — the moment the loop has
finished everything it had and is about to sleep, which is the same flush
point Core Animation commits on. A hundred writes in one event cost one tick.

### A background needs a layer

`background_color` on a plain view has no AppKit equivalent without a backing
layer, so setting one turns `wantsLayer` on for that view. A node that sets no
background stays layer-free, so a tree that themes nothing pays for no layers.

## Not yet reached

These are unimplemented because their stage item has not landed, NOT because
AppKit cannot. They are listed so the difference is never ambiguous.

| What | Stage 4 item |
|---|---|
| The 42 per-kind `create`/`apply` bodies | 2 |
| `KeyReader` — key code / chars / modifiers / named | 4 |
| The gesture band and the decline chain | 4 |
| `SenderReaders` — raise / key_of / item_of / drop | 4 |
| The app menu, toolbar tier, titlebar content | 5 |
| Window sizing policy, density, modal stack | 5 |
| The agent surface | 6 |
| `web` / `hybrid_web` | deferred by the user, 2026-08-04 — needs a `vendor/webkit` bindgen package |
| The collection group | 2 (the heavy end) |

### The collection group

`list`, `collection`, `table`, `tree`, `carousel` are the heavy end of the
sweep: each needs recycling, a row-identity model, and a data source. The
pre-regen backend's recycling NSTableView (`git show
eb5b1b7:vendor/facet_appkit/src/ui.cplus`) is the reference and is worth
quarrying rather than re-deriving.

Until item 2 lands, a control kind with no body renders as an empty backing
view **and says so on stderr**, once per kind. A silent wrong view is the
failure this package exists to avoid.
