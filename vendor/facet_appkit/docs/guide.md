# Guide

How this backend answers facet's contract. Fast start:
[tutorial.md](tutorial.md). What it fills: [ref.md](ref.md). Per-verb
dispositions: [../MANIFEST.md](../MANIFEST.md).

## The rule this package is held to

For each thing facet declares, this package either implements it with what
AppKit offers, or states plainly that AppKit cannot. There is no third option
and no silent gap.

That is enforced rather than asserted. `python3 tools/verb_coverage.py --check`
sorts every declared verb into one disposition and fails on a verb in none of
them, and on a ledger row naming a verb that does not exist.

The dispositions:

| | |
|---|---|
| live | gated on the dirty bit; a later write lands |
| host-rendered | no view of its own; its host re-applies |
| derived | written BACK by the backend, or read by an observer |
| modifier | no write of its own; it changes what another write does |
| create-only | read when the object is built, never after |
| decided | AppKit cannot, and the ledger says why |

## The five verbs

facet asks a backend for `create`, `apply`, `insert`, `remove`, and the
`view_release` that pairs with a created view. That is the whole interface.

`create` builds a FULLY CONFIGURED view: the shared band, then the kind's own
body with every dirty bit set. It calls the same `apply_<kind>` that later
updates use, so a control cannot be correct on mount and wrong on its first
update.

`apply` re-reads a node whose dirty word is non-zero and applies only what the
bits name.

Per-kind work is `controls.cplus`. Kind-independent work is `paint.cplus`.
Neither is visible to facet, which never grows a verb table.

## Which nodes get a view

A control always does. A plain container does when it carries a key, because a
key is an address and `find` has to reach something.

An unkeyed pure-layout container gets nothing and passes its children to the
nearest host. That is how a column inside a scroll view costs no view.

Two different things get no view, and only one is still a box in the layout:

A pass-through container has no view and IS still a row or a column. Its
children are laid out by it.

A non-view KIND is not a rectangle at all. A `span` is a run inside its
label's attributed string; a `menu_item` is a row in an NSMenu. Those are set
`display: none` so they take no space and consume no gap.

## Layout and the frame walk

Layout is `flex_layout`'s. The backend runs it at the window's content size,
then walks the frames onto views.

Every backing view is FLIPPED, so the walk is top-left throughout and carries
no flip arithmetic.

The walk prunes: a node whose layout did not change means every absolute frame
under it is unchanged, so its subtree is skipped.

A control measures itself through its view. The measure asks AppKit's own
`fittingSize`, which is what makes a label as tall as its wrapped text without
facet knowing anything about typesetting.

A zero from `fittingSize` is a decline, not a measurement: several controls
have no intrinsic width because they take what they are given. Those are
floored to the offered space.

## Input, without gesture recognizers

A recognizer cannot decline. facet's handlers answer whether they TOOK the
event, and a control that declines must keep its own behaviour, so the
handlers are the view's own event methods.

They are reached by moving the view's isa onto a runtime subclass of its own
backing class: the KVO technique, one subclass per backing class. Arming is
idempotent, and the node pointer is re-associated every time because a node
that was detached and re-attached has a new address.

`input_transparent` is answered in the hit test, which is why such a node is
armed even with no gestures on it.

Scroll is not a gesture. It routes through the `scroll` control, where the
nested-axis rule lives.

## The sync tick

`Renderer.schedule` raises a flag and nudges the run loop. A CFRunLoopObserver
at before-waiting does the work: `mount::sync()` for the per-node applies, then
the layout pass.

That order matters. An apply can change what a node measures (a label's new
text), and the layout pass right after is what re-flows to fit.

Before-waiting is the flush point Core Animation itself commits on, so a batch
of writes from one event lands as one visual update.

## Recycling

`list` and `tree` recycle. A row is a subtree the platform owns: NSTableView
asks for one when it needs it, so a list of three and a list of ten thousand
cost the same at create.

Rows are realised through `mount::realise`, the second mount path, rather than
a walk this package re-implements. The CELL owns the node, because in facet a
node owns its views.

A `bind` is what makes reuse real. Without one a recycled cell is stripped and
its subtree rebuilt, so scrolling pays a full build per row; with one, `row`
describes the shape once per cell and `bind` writes one row's data into it.

`collection` and `table` materialise every row as ordinary children. That is a
real limit: fine for hundreds, not for ten thousand. The reason it is not
simply moved onto the recycler is `CanReorderItems`, whose reorder ends in
`mount::remove_child` / `insert_child` on real child nodes, and a recycling
collection has no persistent child to move.

## Where a control is built from two things

A control does not have to be one native widget. Where the contract declares a
verb and no single AppKit class answers it, the answer is built from two.

`vertical_align` replaces the cell's class to change `drawingRectForBounds:`.
`clear_button` is a cell that gives back the width it covers. A `date_picker`
that names `is_open` puts a graphical picker in an NSPopover. A `toggle` that
names an off or thumb colour is drawn, and only then, because a hue rotation
has nothing to rotate on grey.

Only a verb the PLATFORM has no concept of stays unimplemented.

## Substitutes, and what they are not

Some verbs describe a touch idiom. The GESTURE has no macOS equivalent; the
FEATURE usually does.

`refreshable` is a refresh strip with a button, and a spinner in its place
while refreshing. There is no scroll-past-the-top on macOS to hang a pull on.

`swipeable` turns its items into a right-click menu, and a trackpad drag strip
sits behind the content.

Both are live and their handlers fire. What is not claimed is the gesture.

## The agent surface

The tree is addressable by key, and the key is also the accessibility
identifier, so an agent, a test, and VoiceOver reach a control the same way.

The surface re-walks on every request: describe, click and set_text always see
the tree as it is now, and a view that has not mounted yet is simply absent.

The standing rule follows from this: a gesture-only affordance must also have a
click path, because an agent has no hands. Pinch zoom has `zoom::set_zoom`,
refresh has its button, swipe has its menu.

## Deviations worth knowing

These are implemented and do not match the portable meaning exactly. Each is
recorded in `MANIFEST.md` with its reasoning.

| | |
|---|---|
| `corner_radius` | Core Animation has one radius per layer; the largest of the four wins |
| `tabs` | a facet-drawn strip in the node's padding, not NSTabView |
| `date_picker` `format:` | the components the string names; order and separators stay locale |
| `table` | the ledger's TableView is a sectioned list, not a spreadsheet; four verbs, no columns |
| `keyboard` | the field editor's flags where they map; a soft-keyboard layout has no home |
| `font_scales` | the reader's preferred size as a multiplier on the named size |

## Reading MANIFEST.md

It is long because it is the record, not a summary. Each section states a verb
or family, what AppKit offers, and what was decided. The fenced ledger blocks
are machine-read by the coverage tool; the prose around them is the reasoning.

When something behaves unexpectedly, the verb's section there is the first
place to look: it usually says exactly which native property was chosen and
what was rejected.
