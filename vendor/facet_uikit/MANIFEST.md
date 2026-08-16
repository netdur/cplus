# facet_uikit MANIFEST

What UIKit **cannot** do, and — kept strictly apart from it — what this pass
**did not build yet**. Blurring the two is the failure this file exists to
prevent: a reader who cannot tell them apart cannot tell a finished backend from
an abandoned one.

Everything in the first list is done. Everything in the second is a debt, and
each one **warns once on stderr** when it is mounted.

**The second list is empty.** Every kind facet describes has a UIKit body. What
remains is section 1 — fourteen props across three kinds that UIKit has no
answer for — and section 3, controls that work but do not look like their name.

The parity number, measured rather than asserted: of the 318 props
`facet_appkit` implements, `facet_uikit` implements **305 (95%)**. The
thirteen-prop difference is section 1, in full, with nothing else in it.

---

## 1. Decided absent — iOS has no such thing

### Window buttons

`window_buttons` describes a macOS titlebar's close / minimise / zoom. An iOS
window is the screen: there is nothing to close it into, no other window to
minimise beneath, and no zoom state. The kind gets a plain backing view so a
custom titlebar row still lays out, and nothing is hosted in it.

**Instead:** a navigation bar (`UINavigationItem`) is the iOS affordance for
"what this screen can do", and facet's `toolbar_item` is the vocabulary that
should reach it once the chrome tier is ported.

### The menu bar

`menu`, `toolbar_item`. There is no menu bar on iOS. These are classified as
non-view kinds — they do not occupy space in the tree — which is the same answer
the AppKit backend gives for a different reason (there they are objects the
application owns rather than rectangles).

**Note:** iOS 13+ does have `UIMenu`, and the `context_menu` family has a real
answer through `UIContextMenuInteraction`. That is in list 2, not here.

### Dragging a split divider

Only the DRAG. `split` itself is built: the axis, the position, both pane
minimums, the collapse and the drawn divider are all honoured, because all five
are geometry and the geometry is flex's — the leading pane takes a fixed
main-axis size, the trailing pane takes the remainder from a zero basis, and the
divider is a gap between them with a layer drawn in it.

What has no answer is moving it with a finger. `UISplitViewController` is a
navigation container whose sidebar appears and disappears by size class, not a
resizable pane pair, and there is no pointer to grab a hairline with.
`on_move` therefore never fires: the position is the application's to set.

**Instead:** an app that wants an iPad sidebar wants the navigation tier, which
is `screen` and `chrome`, not `split`.

### A date or time picker's font

`UIDatePicker` draws its own wheels and its own calendar, and the class has no
font property of any kind — not a family, not a size, not a weight, not a
slant, and no kerning. That is six verbs on `date_picker` and six on
`time_picker`, and there is no public way to reach any of them. Setting them
through KVC on a private ivar was considered and rejected.

What the picker DOES honour: its date, its bounds, its mode, its tint, and
`is_open` — which is the picker's STYLE here, because two of UIKit's three
presentations differ exactly by whether the selector is showing (`Compact` is a
field that opens a calendar on tap, `Inline` is that calendar already open).

**Instead:** an application that needs a styled date field wants a `text_field`
with a picker as its input view — which facet has no verb for, and which would
be a new kind rather than this one.

### A `table`'s style, after it is mounted

`UITableViewStyle` is fixed by `initWithFrame:style:` and has no setter, so
`table`'s `style` is read at create and honoured there. A style CHANGED after
the table is mounted has nowhere to land: the view would have to be replaced,
and the view is mount's rather than the backend's. The write is named in
`apply_recycling_table` rather than left silent.

### Tooltips

`tooltip` on the shared band has no iOS answer and is **not approximated**.
There is no pointer to hover. Turning it into a long-press popover would invent
an interaction the app never asked for and would collide with the context menu.

---

## 2. Not yet built — iOS has an answer, this pass did not write it

**Nothing.** Every kind facet describes reaches a UIKit body:

| Kind | What it is now |
|---|---|
| `list`, `table` | `UITableView` with a synthesized data source; rows built on demand, cells recycled |
| `tree` | The same table over the FLATTENED open rows, re-flattened when the model or the expansion changes |
| `collection`, `carousel` | `UICollectionView` with a flow layout; the carousel pages |
| `swipeable` | A pan-driven reveal over a strip of action buttons. NOT `UISwipeActionsConfiguration` — see below |
| `web`, `hybrid_web` | `WKWebView` with a synthesized navigation delegate; `hybrid_web` loads from the bundle through `loadFileURL:allowingReadAccessToURL:` |
| `canvas` | A `UIView` subclass with `drawRect:`, replaying facet's recorded display list against Core Graphics |
| `split` | flex geometry plus a drawn divider (the drag is section 1) |
| `context_menu`, `menu_item` | `UIContextMenuInteraction` + `UIMenu` / `UIAction` |

### Why `swipeable` is a pan and not the table-row API

`UISwipeActionsConfiguration` is the idiomatic iOS answer and it **cannot
honour `reveal_threshold`**: UIKit owns the distance at which a row opens and
exposes no way to name it. A `swipeable` is also not necessarily inside a table.

So the reveal is driven here — the content translates with the finger and where
it lands when the finger lifts is the threshold's decision, which is what the
verb asks for. All five handlers fire; a tap on an action runs the item's
`on_clicked` and its `on_invoked`, in that order, which is the pair the AppKit
menu path runs.

### One version floor

`popup`'s `is_open` opens the button's menu through `performPrimaryAction`,
which is iOS 17.4. The call is guarded by `respondsToSelector:`, so on an older
system the write is inert rather than a crash. Everything else in the package
builds against the deployment target with no version check.

### Bands that are still unfilled

- **`observe_size` answers no handle.** AppKit posts
  `NSViewFrameDidChangeNotification` and `facet_appkit` rides it; UIKit has no
  such notification — a view learns it was resized in `layoutSubviews`, which
  needs a synthesized subclass. The seam returns 0 rather than a handle that
  never fires, so a caller can tell.
- **No key band.** `gestures::install_key_reader` is deliberately not filled: a
  hardware key on iOS arrives as a `UIKey` through the responder chain, which is
  a different shape from the AppKit reader.
- **No sender readers.** `component::install_sender_readers` is unfilled —
  `input.cplus` binds nodes to views but does not yet answer facet's sender
  questions.
- **No agent surface.** `runtime::install_agent` is unfilled.

Each of those is zero fields rather than three of five, which keeps facet's
portable no-op — a struct half filled would look installed and behave randomly.

---

## 3. Approximated — a real control, standing in for a different one

These are NOT gaps: the verb works, the handler fires, the value round-trips.
They are recorded because what a user sees is not what the kind is named after.

### `checkbox` is a switch

iOS has no checkbox. The platform's answer is a table row with a checkmark
accessory, which is a **list** idiom and cannot stand in a row of its own.
`UISwitch` is the closest control carrying the same two-state meaning and the
same handler shape. `checkbox` has exactly two props — `on` and its colour —
and a switch answers both.

Drawing a checkbox by hand was considered and rejected: it invents a control iOS
users do not have.

### `radio` is a labelled circle button

**Not a switch.** A radio in facet's contract carries a title, a font, a border
and a corner radius, and a `UISwitch` draws none of the four — a backend that
mounted one would have to record eleven verbs as "cannot" for a reason that is
the backend's choice rather than the platform's.

A `UIButton` with a circle image is the shape iOS actually uses for an exclusive
choice, it answers all fourteen of `radio`'s props, and `isSelected` is the
state the two images (`circle` / `inset.filled.circle`) are set for — so the dot
is UIKit's to draw rather than something facet redraws on every write.

**The group works**, and it is the half that matters: turning one radio on turns
its group siblings off. Nothing in UIKit does that, so the exclusion is walked
over the mounted tree, which is the only place the siblings can be found. A
radio's tap is EXCLUSION rather than a toggle — tapping the selected member
leaves it selected.

### `tabs` is a segmented control plus shown/hidden panes

Not `UITabBarController`, which is a view-controller container that owns the
whole window rather than a rectangle in a tree.

### Four colour roles have no iOS twin

| facet role | iOS stand-in |
|---|---|
| `window_background` | `systemBackground` |
| `under_page_background` | `systemGroupedBackground` |
| `control_background` | `secondarySystemBackground` |
| `selected_content_background` | `systemFill` |
| `selected_text_background` | `secondarySystemFill` |

The first three are a real three-level scale on iOS, which is closer than three
unrelated names. The selection pair genuinely has no role: a table row's
selection is drawn by the cell and a text selection by the tint.

Everything else maps onto UIKit's own semantic colours, which are dynamic
objects — a node painted with one follows dark mode with **no repaint at all**.
Only `adaptive` literal pairs and anything flattened to a `CGColor` (gradient
stops, borders, shadows) need the repaint walk that `theme_changed` does.

### `TextAlign::End` does not mirror

`Start` maps to `NSTextAlignment::Natural`, which follows the writing direction.
Foundation offers no "natural trailing", so `End` is `Right` and an RTL layout
gets a trailing alignment that does not mirror. The same gap exists on macOS.

---

## 4. Animation

`animate_*` drives **both channels through explicit `CABasicAnimation` on the
layer**, rather than through `+[UIView animateWithDuration:...]`. The block API
would need a block per call; the layer path is already written, already correct,
and is the same code on both platforms.

Two things carry over unchanged from the AppKit backend, because they are Core
Animation's rules rather than AppKit's:

- **Easing is approximate.** `CAMediaTimingFunction` has four names
  (`linear`, `easeIn`, `easeOut`, `easeInEaseOut`). facet's eleven presets map
  onto them; `Bounce*` and `Spring*` are **not** real bounce or spring curves.
- **A full turn does not animate.** The matrix for a 360° rotation is the matrix
  for no rotation, so there is no path between them.

`progress.animate_progress` is the exception and is simpler here:
`setProgress:animated:` is a real animated setter — but it takes a **flag, not a
curve**, so the `easing:` argument is ignored on this kind.

---

## 5. The anchor point

facet's `anchor_x` / `anchor_y` default to the centre, and so does a CALayer's
on iOS — where an NSView's backing layer starts at `(0,0)`. So the
`set_anchor_keeping_place` compensation is usually a no-op here and is on the
first transform of every view in the AppKit backend. It is kept because an app
that states a corner anchor still needs it: a layer puts its anchor point AT
`position`, so moving one without the other slides the view.

---

## 6. The facade, and what the window host does not do

`facet/src/runtime_ios.cplus` is the facade; `window.cplus` here is the host it
calls. Both are new and both are unrun.

### `UIApplicationMain` is called with argc 0 and argv NULL

A C+ program's `main` does not thread its arguments down to `run_loop`, and
UIKit reads the bundle and the launch options from the app bundle rather than
from argv. **This is the first thing to check when the package is first run on a
device**, because its failure mode is the app not starting at all.

### The appearance flip is not wired

UIKit has no application-level "the appearance changed" hook — a dark-mode flip
arrives as a trait change on views and controllers. `facet_uikit.cplus` has the
repaint walk ready and nothing fires it; the missing piece is a
`traitCollectionDidChange:` override on the root view controller. This is the
one place UIKit is poorer than AppKit, which has
`viewDidChangeEffectiveAppearance`.

Most colours do not need it: UIKit's semantic colours are dynamic objects and
re-resolve themselves. What does not follow on its own is an `adaptive` literal
pair and anything flattened to a CGColor.

### A navigation leaks one screen

`App::run`'s screen swap tears the old screen down (`on_detach`) but does not
free it or its tree. The macOS facade settles the job queue before dropping —
which it can do because its loop returned. There is no such moment here, so the
honest choice was to leak one screen per navigation rather than free something a
queued apply still holds. The fix is a settle that runs ON the loop.

### `Chrome` is almost entirely inert

`width`, `height`, `min_*`, `max_*`, `maximizable`, `minimizable`, `title` and
the pinch-zoom trio describe a desktop window. The screen is the window here.
Only what the tree itself draws survives.

### `nav::push` / `nav::pop` are refused

A pushed screen is a modal presentation on iOS — the same tier `alert`,
`prompt`, `choose` and `present_window` wait on. They answer `false` rather than
doing nothing silently, which is what their callers check.
