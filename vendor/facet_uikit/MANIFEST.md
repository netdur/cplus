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

**TWO NUMBERS, because a backend has two surfaces.** A prop is a WRITE — the
application saying something to a control — and a handler is a READ, the
control saying something back. Both are measured by `tools/parity.py`:

| | appkit | uikit | |
|---|---|---|---|
| props | 318 | 305 | 95% |
| handlers | 66 | 64 | 96% |

The prop number was the only one for a while, and it was misleading in a
specific way: a `text_button` whose every prop bit was honoured was armed,
tapped, and called nothing, because `fire_primary` routed only `K_BUTTON`. Same
for `icon_button`, for the return key on all three text kinds, and for pull to
refresh. Every one of those counted as done on the prop axis. They were found
by tapping the gallery, not by any check here — which is why the handler axis
now exists and why both are printed together.

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

### A split divider's `on_move`

Covered by "dragging a split divider" above: nothing moves it, so nothing can
report that it moved. The handler is real and is never called.

### A web view's `on_web_resource_requested`

A `WKURLSchemeHandler` is the answer and it is not written. It is the one
handler in the package that is UNBUILT rather than unanswerable — reachable,
public, and simply not done — so it belongs in section 2 and is repeated there.

### A `table`'s style, after it is mounted

`UITableViewStyle` is fixed by `initWithFrame:style:` and has no setter, so
`table`'s `style` is read at create and honoured there. A style CHANGED after
the table is mounted has nowhere to land: the view would have to be replaced,
and the view is mount's rather than the backend's. The write is named in
`apply_recycling_table` rather than left silent.

### A scroll view clips; the paint band used to un-clip it

`masksToBounds` on a layer IS `clipsToBounds` on its view, and the paint band
wrote `false` into it for every node without a corner radius. That is right for
a plain UIView, whose default is false, and WRONG for a UIScrollView, whose
default is true and whose entire purpose is to be a window onto something
bigger — so every scroll, list, table, tree, collection and carousel had its
clipping switched off by the paint pass, and their content drew outside them.

The "off" state is not `false`, it is what the view does when nobody has asked,
which the class knows and `apply_corner_radius` now asks it.

### Hovering a canvas — NOT absent

`GraphicsView` names seven interaction verbs. Five are touches and are wired
(`on_press`, `observe_drag_interaction`, `on_release`, `on_cancel`). The other
two are hover, and "iOS has no hover" was the obvious thing to write and is not
true: `UIHoverGestureRecognizer` reports a pointer or an Apple Pencil moving
above the screen, and it is installed. On a device with neither it never fires,
which is correct behaviour rather than a gap.

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

### The one unbuilt handler

`hybrid_web`'s `on_web_resource_requested` needs a `WKURLSchemeHandler`
registered on the configuration before the view exists, the way
`facet_appkit/web.cplus` does it. The message channel in the other direction
(`on_raw_message_received`, a page calling out through
`window.webkit.messageHandlers.facet.postMessage`) IS built.

### `observe_size` is filled

"A view learns it was resized in `layoutSubviews`, which needs a synthesized
subclass" was the reason this seam was left empty, and it turned out to be the
whole implementation rather than the obstacle. AppKit posts a frame
notification and `facet_appkit` rides it; UIKit posts nothing because it CALLS
a method instead — so the observer is `layoutSubviews`, added to the observed
view's own class through `object_setClass`, one subclass per observed view.
Per view rather than one shared class because the base differs: a scroll view
has to stay a scroll view.

`examples/facet_gallery_ios`'s Responsive demo reads the real width through it.
The desktop gallery's equivalent simulates the width with three buttons.

### The facade refuses nothing

`runtime_ios.cplus` had twelve entries that printed "not yet" and returned
false. It now has none:

| verb | what it is here |
|---|---|
| `alert` / `choose` / `prompt` | `UIAlertController`, presented on the topmost controller. NOT blocking — iOS has no nested modal loop, and the answer arrives in a handler |
| `nav::push` / `nav::pop` | a `UINavigationController` at the window root. The back button and the SWIPE-BACK gesture come with the stack rather than being drawn |
| `present_window` | a modal sheet. A push is a journey and has a back button; a presentation is an interruption and is dismissed — facet's two verbs mean that difference, so they get the two UIKit shapes |
| `observe_backgrounding` / `observe_resumed` / `observe_stopped` | the app delegate's lifecycle edges |
| `observe_window_active` / `observe_window_inactive` | `didBecomeActive` / `willResignActive` — the same question a desktop asks of a window, asked of the app |
| `observe_window_size` | `services::observe_size` over the window's root node |

Two notes worth keeping. `observe_stopped` rides `applicationWillTerminate:`,
which iOS does NOT guarantee — a backgrounded app is usually killed without it,
so an app that must save state should save it on BACKGROUNDING. And
`present_window`'s `width` / `height` are read and ignored: a sheet is sized by
its presentation style and has no size of its own.

### Bands that are still unfilled


### The agent surface is served

`vendor/agent_uikit` is the reader — the sibling of `agent_appkit`, walking a
live UIView tree into agent_core's identity registry and filling agent_mcp's
`Backend`. `facet/agent_ios.cplus` installs it, so `runtime::install_agent` is
no longer empty and `in_app()` compiles on this platform.

**The transport is a PORT, not a path**, and that is the whole iOS story: a Unix
socket lives in the app's sandbox where nothing on the development machine can
reach it, and to a device there is no shared filesystem at all. So the string an
application passes to `agent_mcp(...)` is read as a port number (default 8787)
and served by `agent_mcp::serve_tcp`, bound to LOOPBACK. Reach it over USB with
usbmuxd — `iproxy 8787 8787` or `pymobiledevice3` — which is how Flutter's Dart
VM Service and Chrome's remote debugging are reached.

Two verbs report unsupported rather than answering wrongly. `set_caret` is
reachable (a UITextField's caret is a `UITextRange` from
`positionFromPosition:offset:`, which the text band already uses) and is not
written. `invoke_menu` is not reachable at all: iOS has no menu bar, and the
context-menu tier is a per-view interaction rather than an application-wide tree
with a path — there is nothing for "File/Save" to name.

`read_runs` is unsupported for now: `UITextView.textStorage` and
`UILabel.attributedText` are both NSAttributedString and the walk is portable,
so this is unbuilt rather than unanswerable.

### The key band and the sender readers ARE filled

Both were recorded here as unfillable and both were wrong, in the same way "iOS
has no hover" was wrong.

`install_sender_readers`: four of the six readers are the SAME CODE the AppKit
backend runs. `key_of` walks `accessibilityIdentifier` up the `superview`
chain — both properties are UIView's too — and `item_of` shares the walk.
`raise` is shorter here, because `bringSubviewToFront:` is a verb AppKit does
not have. The drag trio is left as zero fields on purpose: facet declares no
drag verbs, a phone's drag-and-drop is BETWEEN apps, and filling them would be
inventing a caller.

`install_key_reader`: `pressesBegan:withEvent:` on UIResponder, unpacked one
`UIKey` per press, with the four readers over UIKey's own properties. The
mapping tables — USB HID codes onto facet's named keys, UIKeyModifierFlags onto
facet's bits — are checked by the selftest, because a table is exactly the kind
of thing that is wrong silently.

**Who gets keys:** the responder chain, so something must be first responder.
On a phone with no keyboard this fires for nobody, which is correct rather than
broken — an iPad with a Magic Keyboard, a Mac running the iOS binary, or the
simulator with hardware-keyboard capture are where it is real.

### The gesture band was armed nowhere

`input::arm_tap` was written, idempotent, and had NO CALLER. A plain view
carrying `.gesture(on_click:)` got a backing view — `wants_view` says yes for
exactly that reason — and then no recogniser, so the handler could not fire.
Both bands are armed in `views::configure` now, which every apply reaches.

### The checks run now

`cpc test` builds a HOST binary and macOS has no UIKit, so this package had
never executed a test — every claim about it was a screenshot or a console line
I read by hand. That is how five bugs reached a person before they reached a
check, and one of them was declared fixed twice on a measurement that was
counting the wrong pixels.

`src/selftest.cplus` holds the checks and `examples/facet_uikit_tests` is an
iOS binary that runs them:

```
vendor/facet_uikit/tools/run_ios_tests.sh
```

It builds the runner, links it against the simulator SDK, installs, launches,
and **exits non-zero when anything failed** — verified both ways by putting the
clipping bug back and watching it report 8 passed / 1 failed.

Every check in it is a bug that happened. It is not a survey of the package.

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
