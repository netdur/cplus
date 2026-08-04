# facet_appkit — the manifest

INTENT.md's rule: for each thing facet declares, this package either implements
it with what AppKit offers, or **states plainly that AppKit cannot**. This file
is where every "cannot" is written down, so anyone can answer "how does that
happen on a Mac?" by reading it.

A row here is a commitment, not a note. Nothing is left implicit: a verb that
is neither implemented nor listed below is a gap, and the gap is a bug.

Status: **Stage 4 items 1 and 2 COMPLETE.** Every one of the 42 kinds has an
answer — a body, a recorded "AppKit cannot", or a named deferral — and
`every_kind_now_has_an_answer` in the suite is the guard. Items 3-7 are in
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

### `canvas` replays a recording; it does not call back per verb

facet's drawing vocabulary is RECORDED: a `Drawable` is handed a
`vocab::Canvas` and appends commands to it, and the backend replays the list.
So this package registers no per-verb hooks — `drawing.cplus` has one loop
over `Canvas::at(index:)`, executing into the CGContext AppKit hands
`drawRect:`.

`canvas` is the one control with a **view class of its own**
(`FacetCanvasView`, an NSView with `isFlipped` and `drawRect:`), and the one
control with **no layer** — `drawRect:` paints straight into the window's
backing store, and a layer would hold a second copy of the same pixels.

Two things about the replay are worth knowing:

**No text measurement at record time.** `ICanvas.GetStringSize` is not adopted
and could not be honoured if it were: measuring text needs a live platform
context, and while a Drawable is recording there is none. Text is placed by
giving it a box and an alignment. The one verb that still needs a width —
`draw_text(at:)`, which aligns a line ABOUT a point — measures at REPLAY time,
where a context does exist.

**`draw_text(at:)` places the top-left, not the baseline.** MAUI's
`DrawString(value, x, y, alignment)` treats `y` as the baseline. facet's box
model is top-left everywhere else, and a point that means something different
from every other point in the contract is worse than a small deviation from
MAUI. The block form has a box, so it does not arise there.

Everything else in ICanvas is honoured: the state setters, the state stack,
the transform, the clip (including `subtract_from_clip`, which CG has no call
for and which is done with an even-odd clip over the current clip bounds), the
shapes, paths with both winding rules, gradients via `set_fill_brush`, images,
and the three text verbs. `Blend` maps to `CGBlendMode` by ordinal, because
MAUI's enum was modelled on CG and matches it value for value.

**A shadow's offset is negated in y.** Core Graphics does not carry a shadow
offset through the CTM, so the flip that makes everything else top-left does
not reach it, and a positive `Shadow.offset.y` would throw the shadow UP.
MAUI does the same thing in the same place (`SetShadow`, `#if MONOMAC`).
`a_shadow_falls_DOWN_from_a_positive_y_offset` is the guard.

**An arc's `clockwise` reads backwards, and is right.** 0 degrees is 3 o'clock
and 90 is 12 o'clock, so `clockwise: true` from 0 to 90 takes the LONG way,
down through 6 and 9. The angles are measured the ordinary way while the
screen's y runs down, and MAUI resolves it identically — negated angles into
`AddArc(..., !clockwise)`. Pinned by `an_arc_sweeps_the_way_clockwise_says`
so nobody "corrects" it by eye. The transform that turns CG's circular arc
into an elliptical one is written out at `add_arc_path`.

`examples/canvas_probe` draws every verb once, including both sweeps side by
side, because an agent cannot see whether an arc went the right way.

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

### The gesture band moves the view's class

A gesture handler answers `bool`, and `false` means the event must carry on
exactly as if facet had never looked at it. Nothing on AppKit continues an
event except `[super mouseDown:]`, so the handlers have to BE the view's event
methods — and gestures are a modifier on ANY node, whose backing view is
whatever its kind needs.

So the class is made at runtime, per backing class, and the view's isa is moved
onto it. This is what KVO does, for the same reason. An NSButton with gestures
is still an NSButton; it gains overrides that call super when the app declines.
One subclass per backing class, cached on the class itself.

Consequence for an application that drops beneath facet: `object_getClass` on
a gesture-bearing view answers `FacetInput<n>`, not the class you expect.
`isKindOfClass:` is unaffected, which is the check that matters.

The per-view gate is never a method, and that is load-bearing: the class is
SHARED by every view of its kind, so adding `performDragOperation:` for one
drop zone would answer for every node of that kind. Drop selectors live on the
class from the start and `registerForDraggedTypes:` — AppKit's own gate — is
what a view opts in with.

### `on_pinch` is `magnifyWithEvent:`, not the window zoom

The gesture reports a trackpad magnify on the node. Window-level
pinch-to-zoom is a different feature with a different rule (scale only, never
reflow) and is not what this verb does.

### Scroll is not a gesture

facet routes the wheel through the `scroll` control, where the nested-axis rule
lives (`FacetScrollView` locks an axis per gesture). A `scrollWheel:` override
in the gesture band would fight it.

## Not yet reached

These are unimplemented because their stage item has not landed, NOT because
AppKit cannot. They are listed so the difference is never ambiguous.

| What | Stage 4 item |
|---|---|
| The 42 per-kind `create`/`apply` bodies | 2 |
| The app menu, toolbar tier, titlebar content | 5 |
| Window sizing policy, density, modal stack | 5 |
| The agent surface | 6 |
| `web` / `hybrid_web` | waiting on `vendor/webkit`, which the user has in progress as of 2026-08-04 |
| The collection group | 2 (the heavy end) |

### The collection group builds every row, not just the visible ones

`list` and `collection` MATERIALISE their rows as ordinary children of
themselves: `row(ctx, i)` is called for each i and the result is added with
the same structural verb anything else uses. From there facet's own machinery
does everything — mount creates the views, insert puts them in the scroll's
document view, the frame walk places them, teardown releases them. There is
no second lifetime model and no path a row can take that a child cannot.

The cost is real and is the one thing to know: a list builds every row up
front rather than the visible ones. AppKit's answer is NSTableView with a
data source, which recycles — and the pre-regen backend's recycling
NSTableView (`git show eb5b1b7:vendor/facet_appkit/src/ui.cplus`) is the
upgrade when a consumer actually has ten thousand rows. It was not worth
inventing a second, parallel mount path before then.

`tree` walks only through OPEN branches, which is what keeps a deep tree
cheap under that rule: a closed branch is not descended at all.

Until item 2 lands, a control kind with no body renders as an empty backing
view **and says so on stderr**, once per kind. A silent wrong view is the
failure this package exists to avoid.
