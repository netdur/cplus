# facet_appkit — the manifest

INTENT.md's rule: for each thing facet declares, this package either implements
it with what AppKit offers, or **states plainly that AppKit cannot**. This file
is where every "cannot" is written down, so anyone can answer "how does that
happen on a Mac?" by reading it.

A row here is a commitment, not a note. Nothing is left implicit: a verb that
is neither implemented nor listed below is a gap, and the gap is a bug.

Status: **Stage 4 items 1-4 COMPLETE, item 5 at 52/17/13.** Every one of the 42 kinds has an
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

### The toolbar's colours and height

| Verb | Why not |
|---|---|
| `Toolbar.BarBackground` `BarTextColor` | NSToolbar draws its own strip and offers no colour API — the same reason as the tab strip. |
| `Toolbar.BarHeight` | NSToolbar sizes itself to its items and the window's style. |
| `Toolbar.IconColor` | NSToolbarItem images are template-tinted by the system. |
| `Toolbar.DynamicOverflowEnabled` | NSToolbar overflows into a chevron by itself; there is no switch to turn that off. |

### Mobile concepts, on a desktop

| Verb | Why not |
|---|---|
| `Toolbar.DrawerToggleVisible` | A drawer toggle has no desktop equivalent; a sidebar is a `split` pane. |
| `Toolbar.BackButtonEnabled` `BackButtonTitle` `BackButtonVisible` | A nav-bar back button is a phone idiom. A desktop app navigates with its own controls and ⌘[. |
| `ContentPage.HideSoftInputOnTapped` | There is no soft keyboard to hide. |
| `Page.IsBusy` | macOS has no app-wide busy indicator. A spinner is a control the application places where the waiting is. |

These are DECIDED, not deferred, and the tier ledger says so: guard 5b now
carries three dispositions rather than two, and prints them apart. "The
platform cannot" and "nobody has built it yet" are different facts and a
tracker that conflates them is not a tracker.

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

### Intrinsic size comes from the platform

flex sizes a LEAF from a measure callback, and `views::create` installs one:
the view's own `fittingSize`. That is what makes a button as wide as its title
and a label as tall as its text without facet knowing anything about type
setting. A bounded width is handed over first through
`preferredMaxLayoutWidth`, or a wrapping label measures as one long line and
gets clipped.

It goes on the kinds whose size is genuinely intrinsic —
`views::has_intrinsic_size` names them one by one. A scroll view, a web view
and a canvas have a fitting size too and it means something else: a surface is
sized by its container, and asking WKWebView how big it would like to be is
not a question with a useful answer.

Until 2026-08-04 nothing installed a measure at all, so every control without
an explicit width and height laid out 0x0 — present, correct and invisible.
It survived because the tests asserted PROPS and the examples sized everything
by hand; the guard now asserts a FRAME.

### The titlebar slots are read from the tree

`TitleBar.LeadingContent` / `TrailingContent` are subtrees under a reserved `@`
key — `window_chrome::titlebar_leading` / `_trailing` — and the window lifts
them into an `NSTitlebarAccessoryViewController`. Carried as a key rather than
a field on `Chrome` because Chrome is a VALUE struct, copied into every window
that opens, and a subtree is not a value.

Lifted OUT of the content: the slot's node gets `display: none`, which is
flex's own word for "not in this layout" and needs no backend concept. A node
laid out in two places is laid out wrong in one.

Portability, since it decides the shape: **GTK supports this best** —
`GtkHeaderBar` is exactly leading and trailing widget slots — AppKit has the
accessory controller, and Windows has neither: an app there extends the client
area and draws its own bar. So this is the GTK-shaped extra, and `Bar::Custom`
plus `window_buttons()` and `.window_drag()` remains the portable answer.

### `alert` is a facet tree in a sheet, and non-blocking

It used to be an NSAlert and `runModal`, which was two problems at once. It
BLOCKED — the caller did not return until the user answered — and nothing in an
NSAlert has a key, so AN AGENT COULD NOT REACH IT. A dialog an agent cannot
answer is a dialog an agent cannot get past.

So the sheet is an ordinary facet tree in an ordinary window, attached with
`beginSheet:`, and every part of it is addressable the way everything else is:
`alert:title`, `alert:message`, `alert:primary`, `alert:secondary`. The agent
clicks the button; there is no second path and no simulated one.

Non-blocking is the load-bearing half: a blocking call cannot be driven over
MCP, because the agent's own request is what would have to return first.

WINDOW-modal, not application-modal — it blocks its parent, which is what an
alert means to a person, while the rest of the app and the agent channel keep
running.

`alert_blocking` is kept for the one case a sheet cannot serve: a decision that
must be made before the process can go on, with no window to attach to. An
agent cannot reach it, which is exactly why it is not the default.

**The file pickers stay native, and that is a hole.** `choose_file` and
`choose_directory` are NSOpenPanel, which an agent cannot drive and which
cannot be reimplemented — the panel IS the sandbox door, and a facet-drawn
imitation would grant nothing. An application that needs an agent to choose a
file has to offer a path some other way.

### The window tier: what a native window is asked

Sixteen ledger rows landed together, and they share a shape — none of them can
be answered without a real window, so none could exist before this stage.

**`close_button_only` became `maximizable` + `minimizable`.** The old field
said exactly `maximizable: false, minimizable: false` in one word and could
say nothing else; the ledger asks for the two rows apart because a window may
well want one and not the other. The buttons are HIDDEN rather than disabled:
AppKit's own answer is to drop the style-mask bit, but the mask is what chose
the bar's style, and re-deriving it in one expression would put two decisions
in one place.

**Zero is unbounded for a maximum size.** An unset `max_width` must not become
a maximum of nothing. The minimum pair already reads that way, so the maximum
has to agree or the two fields mean different things by the same value.

**`is_window_active()` is `isKeyWindow`, not main.** A window can be main
while a panel holds key, and "is the user typing into this" is the question an
application means.

**The appearance override is undoable.** `Appearance::Unspecified` is not a
third colour scheme — it is "give the system back", and without it an override
could never be taken off. Setting one takes the same repaint path a system
light/dark flip already goes through, because colours resolved at paint time
do not re-resolve themselves.

**The app menu was built and never installed.** `App::build_menu()` produced
an AppMenu and nothing read it. facet's AppMenu is FLAT — each item names its
menu — and AppKit wants a tree, so the grouping happens in the backend, in
FIRST-MENTION order, which is the only order a flat list can have expressed.

A platform verb (Copy, Quit, Close) gets the platform's selector and **no
target**, so the responder chain decides. That is what greys Copy out with
nothing focused, without facet knowing anything about focus. An app command
carries a real target and wins over a token action, because a row with both
meant the callback.

**The toolbar is read from the tree.** A `toolbar_item` answers `wants_view`
false, so the items are collected from the NODES when the window opens — from
anywhere in the tree, because a screen describes its toolbar where the content
that owns it lives, which is rarely the top. A tree with no items asks for no
NSToolbar at all rather than an empty one.

### `web` and `hybrid_web` are one WKWebView with two wirings

`vendor/webkit` landed 2026-08-04 and both kinds are implemented. They share a
file and a delegate class, because WKNavigationDelegate and
WKScriptMessageHandler are both "the page told us something" and splitting them
would pin two objects to one view.

**The two reads are WRITTEN BACK.** `can_go_back()` and `can_go_forward()` are
contract reads whose truth lives in WKWebView's back-forward list, so the
delegate writes them into the props on every navigation. The cursor then
answers from the props like every other read, and an application never learns
that one answer came from the platform and another from the description.

**A `source` is read three ways**, because MAUI's `Source` is a WebViewSource
(URL or HTML) and facet reduced it to a `str`, so the string has to say which:
starts with `<` is markup, contains `://` is an absolute URL, anything else is
a file path — loaded with read access to its FOLDER, or its stylesheet is
refused and the page looks broken rather than blocked.

**`on_navigating` is `didStartProvisionalNavigation:`, not a policy decision.**
MAUI's `Navigating` is cancellable; facet's handler returns nothing, so it
could not cancel even if this were wired to
`decidePolicyForNavigationAction:`. Reporting the START of a navigation is the
honest thing the signature supports.

**The hybrid channel is named here**, because facet declares a raw message in
each direction and nothing about how it is carried:

| direction | how |
|---|---|
| page → facet | `window.webkit.messageHandlers.facet.postMessage(body)` |
| facet → page | `window.facet.onmessage(body)`, if the page defined it |

One word, `facet`, both ways. The outgoing body is escaped before it is
spliced into script — a quote or a newline in a message would otherwise end
the literal and run whatever followed.

**`on_web_resource_requested` is not honoured.** It needs a
`WKURLSchemeHandler` and a custom scheme, which is a second loading path
alongside `loadFileURL:`; nothing has asked for it yet, and a resource hook
that fires for some requests and not others would be worse than none.

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

| What | Stage 4 item | Size |
|---|---|---|
| The agent surface — written, never compiled | 6 | medium |
| 5 tier rows with no facet verb to carry them | 5 | decisions, not tasks |
| Examples revived on the new API | 7 | medium |

Everything else in items 1-5 has landed: all 42 kinds have bodies, the window
tier answers, the app menu and toolbar install, `web` and `hybrid_web` run on
`vendor/webkit`, and nothing takes the unimplemented-kind warning path.

### `list` and `tree` recycle; `collection` does not, and that is deliberate

`list` is an NSTableView with a data source. Rows are AppKit's: it asks for one
when it needs it, so a list of three and a list of ten thousand cost the same
at create. Each row's subtree is realised through `mount::realise` — the second
mount path, named in the seam rather than re-implemented here — into the cell
that owns it. The CELL owns the node, because in facet a node owns its views:
letting the node drop after building would empty the cell the moment it was
filled.

`tree` is an NSOutlineView, and it is a BETTER fit than the list: the data
source is item-based and facet's tree is already a model of items, so the item
IS the `*TreeNode`. Nothing maps an index onto a node and nothing can drift
when the model changes shape. Indentation is AppKit's here — the materialising
version made it a leading pad on the row because it had no other way, and
drawing a second gutter inside an outline view's own indent would double it.

facet's `expanded` set is the MODEL; the outline view's per-item state follows
it on reload. The walk stops at a closed branch, because an outline view has no
rows below one to expand.

`collection` still materialises every item as an ordinary child. That is not
an oversight: a collection is a grid whose items flex against each other, and
an NSTableView row is not that. It gets the recycling treatment when a
consumer has a collection long enough to need it.

**Two things make it fast, and one still does not.**

`row_height` skips measurement ENTIRELY — no build, no layout, no cache. When
an application knows its rows are uniform that is the cheapest a list can be.

The height cache exists because NSTableView asks `heightOfRow:` for EVERY row
whenever it recomputes geometry, not only the visible ones, and answering means
building the row and laying it out. Without it a scroll re-measures the whole
list on every pass, felt as a periodic hitch. The cache is keyed to the width
it was measured at, because a different width re-wraps every row.

**The bind step is what makes reuse real.** Without one, a recycled cell was
stripped and its subtree REBUILT — the cell was reused and the work was not, so
scrolling paid a full build per row. `set_row_bind` splits the two questions:
`row` describes the SHAPE once per cell, `bind` writes one row's data into it.
Scrolling then costs a handful of property writes.

It is OPTIONAL and its absence is not an error — a list that names no bind
rebuilds each row, which is what every backend did before the contract could
say otherwise. Setting one is a promise that every row has the same shape,
which recycling assumes anyway, said out loud.

Two things about it are worth knowing. A row is bound on its FIRST use too, so
the builder is responsible for shape only and a row can never show the data of
whichever row happened to build its cell. And MEASURING binds as well — heights
are asked for every row, so without a bind, measuring a thousand rows built a
thousand subtrees; with one it builds one.

A bind writes PROPS, which marks nodes dirty and touches nothing native, and a
realised row is outside the window walk that carries dirty bits across. So the
backend applies it immediately through `mount::sync_from` — the third verb this
path needed, after `realise` and `unrealise`.

**A resize coalesces into one reload.** A live drag changes the width on every
frame, and every width change invalidates every measurement, so reloading on
each one re-measures the whole list dozens of times a second. A reload is
scheduled instead and the next frame cancels the one before it: the list
re-measures once, when the drag settles.

Whether the viewport was at the bottom is read BEFORE anything moves, and
re-pinned after. A narrower width re-wraps rows taller, so the content grows
under the viewport and "at the bottom" has stopped being true by the time the
reload has run — which is why the question cannot be asked afterwards. It is
the chat behaviour, and the one that is invisible until it is missing.

The width a cell gets is the COLUMN's, not the table's frame, and that is a bug
rather than a detail: modern NSTableView styles inset cells horizontally, so
measuring wrapped rows at frame width under-measures them and clips the last
lines at certain widths. It presents as "scrolling down does not show the rest
of the message", which is nothing like the mistake that causes it.

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

The warning path still exists and is still the rule — a control kind with no
body renders as an empty backing view **and says so on stderr**, once per
kind. Nothing takes it today, and `every_kind_now_has_an_answer` is what keeps
that true.
