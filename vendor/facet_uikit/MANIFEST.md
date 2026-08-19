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

**MOVED OUT OF THIS SECTION.** `window_buttons` was listed here as decided
absent, on the reasoning that "an iOS window is the screen: there is nothing to
close it into, no other window to minimise beneath, and no zoom state". That
stopped being true on an iPad in iPadOS 26. It is answered now — §7 has it.

The menu-bar half of the old entry stands: a navigation bar (`UINavigationItem`)
is the iOS affordance for "what this screen can do", and facet's `toolbar_item`
is the vocabulary that should reach it once the chrome tier is ported.

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
`Backend`. `facet_agent/agent_ios.cplus` installs it, so `application::install_agent` is
no longer empty and `in_app()` compiles on this platform.

**The transport is a PORT, not a path**, and that is the whole iOS story: a Unix
socket lives in the app's sandbox where nothing on the development machine can
reach it, and to a device there is no shared filesystem at all. So the string an
application passes to `agent_mcp(...)` is read as a port number (default 8787)
and served by `agent_mcp::serve_tcp`, bound to LOOPBACK. Reach it over USB with
usbmuxd — `iproxy 8787 8787` or `pymobiledevice3` — which is how Flutter's Dart
VM Service and Chrome's remote debugging are reached.

`invoke_menu` is the one verb that reports unsupported, and it is unreachable
rather than unbuilt: iOS has no menu bar, and the context-menu tier is a
per-view interaction rather than an application-wide tree with a path. There is
nothing for "File/Save" to name.

`set_caret` and `read_runs` are BUILT. The caret is a `UITextRange` between two
`UITextPosition`s rather than an offset pair, so the range is constructed from
the document's beginning — and facet counts BYTES where UIKit counts UTF-16, so
the offset is converted rather than passed through. The runs walk
`UITextView.textStorage` / `UILabel.attributedText`, which are NSAttributedString
on both platforms; only the font traits differ, reached through a
UIFontDescriptor rather than the NSFontManager iOS does not have.

`invoke_menu` remains the one verb that is unreachable rather than unbuilt.

**It was served and it had never been RUN.** Pointing a client at it (the iOS
gallery, 2026-08-19) found four things, and none of them were in the protocol:

*The walk was one layout pass too early.* facet's own tick gives every mounted
view its frame; a UITableView realises its CELLS inside its own
`layoutSubviews`, on the turn after it is handed a size. A snapshot taken
between the two sees a table with no rows in it — and because the surface is a
snapshot, it goes on seeing none for the life of the screen. `attach_root` now
flushes the tick AND forces a layout before it walks.

*The rows had no names.* `facet_appkit` has tagged its outline cells from the
application's `row_id` since it was written and this backend never did. A tree
row is CONTENT, mounted inside a cell `recycler.cplus` owns, so no application
key ever lands on the node a click has to reach. `tag_tree_row` is the missing
half.

*Every verb ran on the socket thread.* `agent_appkit` hops to the main thread
and this package did not, which on iOS is the sharper omission — UIKit's main
thread checker traps a call from a worker rather than merely racing. The write
verbs now ride `performSelectorOnMainThread:withObject:waitUntilDone:`, with
one-`id` shims on UIView for the selectors that take anything else.

*A row could not be clicked at all.* A UITableViewCell is not a UIControl and
carries no recogniser — the table owns the touch. `click` now selects the row
AND calls `tableView:didSelectRowAtIndexPath:`, which is where UIKit differs
from AppKit: `selectRowIndexes:` posts a notification the delegate hears, while
`selectRowAtIndexPath:` is documented NOT to call the delegate, on the ground
that a programmatic selection is not a user selection. Stopping at the selection
would highlight the row and run none of the application's code.

`describe_ui` now re-walks the surface first, on the main thread, which is what
`agent_appkit::refresh_surface` has always done. Without it a `set_text` that
landed on screen read back empty, and a `click` that swapped an outlet's content
left the agent describing the screen it had just left. The protocol that falls
out is describe → act → describe → act.

`examples/facet_gallery_ios/tools/mcp_check.py` is the check: 25 assertions over
the transport, both describe modes, click, set_text, the caret, the runs, the
tier gate and every refusal. It runs unchanged against the simulator and against
a device behind a usbmuxd forward, and **both pass** — simulator 2026-08-19,
iPad Pro 11-inch (3rd gen) on iOS 26.6 the same day. The USB hop the deploying
notes called the one untaken link is taken.

A device run is only a device run if you PROVE it: a simulator shares the Mac's
network stack, so a gallery left running in one holds the same loopback port,
the forwarder fails to bind, and the check quietly grades the simulator. The
proof is to terminate the app ON THE DEVICE and watch the socket stop
answering.

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

`src/selftest.cplus` holds the checks and `tests/` is an
iOS binary that runs them:

```
vendor/facet_uikit/tools/run_ios_tests.sh
```

It builds the runner, links it against the simulator SDK, installs, launches,
and **exits non-zero when anything failed** — verified both ways by putting the
clipping bug back and watching it report 8 passed / 1 failed.

Every check in it is a bug that happened. It is not a survey of the package.

### The theme matrix runs on a device too

The appearance flip cannot be checked in `tests/`. That runner is in-process, so
the most it could do is set `overrideUserInterfaceStyle` on a window — which
exercises the path an app takes when it FORCES its own appearance, not the path
a system setting takes into a running app. The two do not share the code that
broke.

So there is a second harness: `themeprobe/` is a live app that paints flattened
swatches and prints what each one actually ended up filled with, and

```
vendor/facet_uikit/tools/run_theme_matrix.sh [device-udid]
```

changes the device's own settings with `xcrun simctl ui` and does the asserting
from outside. It restores every setting it touched on the way out, including
after a failure.

**Every assertion is read off a FLATTENED value.** UIKit's semantic colours are
dynamic objects that re-resolve themselves on a flip with no help from facet, so
a probe painted from semantic roles passes against a completely unwired
implementation — which is what nearly happened when this was verified by eye. The
three that can actually fail are an `adaptive` literal pair, an adaptive TEXT
colour (a per-kind prop, and the bug that hid behind the first one), and a
gradient stop that reached a CGColor. The semantic swatch is asserted as a
positive control only: it proves the harness is wired and is never evidence about
facet. The process id is on every reading, because a setting change that quietly
relaunched the app would otherwise produce a perfect before and after taken from
two different processes.

Green on an iPad and an iPhone, 2026-08-18.

Two things it measured that are not documented anywhere else:

- **`simctl ui <dev> increase_contrast enabled` does not drive
  `UITraitCollection.accessibilityContrast` on iOS 26.1.** It reads 1 ("normal")
  on a live app and on a cold start alike, while
  `UIAccessibilityDarkerSystemColorsEnabled()` flips correctly. For that axis the
  accessibility predicate is the door and the trait is not.
- **`UITraitCollection.currentTraitCollection` is not a reliable read outside a
  view update.** From a timer callback its axes come back unspecified. A view's
  own `traitCollection` is fully specified and is what UIKit resolves that view
  against. Note that `paint::is_dark_now()` reads the class property — which is
  correct there, because it is called from inside the repaint walk, where UIKit
  has set it. It would not be correct anywhere else.

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

### The appearance flip — WIRED

UIKit has no application-level "the appearance changed" hook: a dark-mode flip
arrives as a TRAIT CHANGE, delivered to views. So the signal is taken by an
invisible 0x0 sentinel subview whose `traitCollectionDidChange:` forwards to
`theme::notify_appearance_changed` — the same shape facet_appkit uses for
`viewDidChangeEffectiveAppearance`, which keeps one problem one design across
the two backends.

It filters on the light/dark axis. `traitCollectionDidChange:` also fires for
size class, dynamic type and layout direction, and a whole-tree repaint on every
rotation would be a real cost for no reason.

**Why this was invisible for so long, and worth remembering.** Most colours
never needed it: UIKit's semantic colours are dynamic objects that re-resolve
themselves, so a screen painted from facet's semantic roles followed a flip with
no help at all — and looked completely correct. What did NOT follow was anything
facet had flattened: an `adaptive` literal pair (token 254), which paint.cplus
resolves to one static UIColor for whichever trait collection was current when
it was applied, and anything that reached a CGColor.

Photographed before the fix: a row of `adaptive(light: red, dark: blue)` stayed
**red** through a flip to dark while the semantic row beside it went black on
its own. After: red → blue, live, same process id. The repaint walk had been
correct the whole time; nothing called it.

### A navigation leaks one screen

`App::run`'s screen swap tears the old screen down (`on_detach`) but does not
free it or its tree. The macOS facade settles the job queue before dropping —
which it can do because its loop returned. There is no such moment here, so the
honest choice was to leak one screen per navigation rather than free something a
queued apply still holds. The fix is a settle that runs ON the loop.

### `Chrome` is inert on a phone and mostly live on an iPad

This section used to say "almost entirely inert", and that sentence was true of
a phone and false of an iPad. Since iPadOS 26 an iPad app lives in a window the
user resizes and places beside other apps, and most of `Chrome` has somewhere to
land. What lands is decided per field by asking the running scene, never by
testing the device — `window.cplus` says why at length, and `device.cplus`
states the rule.

| field | iPad | iPhone | how |
|---|---|---|---|
| `title_text` | ✅ | — | `UIScene.title` |
| `subtitle_text` | ✅ | — | `UIScene.subtitle` |
| `min_width` / `min_height` | ✅ | — | `UISceneSizeRestrictions.minimumSize` |
| `max_width` / `max_height` | ✅ | — | `UISceneSizeRestrictions.maximumSize` |
| `bar` | ✅ | — | the scene's windowing control style |
| `minimizable` | ❌ | ❌ | see below |
| `maximizable` | ❌ | ❌ | see below |
| `width` / `height` | ❌ | ❌ | see below |
| the pinch-zoom trio | ✅ | ✅ | `zoom.cplus` — see below |

On a phone every one of those objects is nil, so the whole replay is a few null
checks and the screen is still the window.

### The pinch-zoom trio was recorded as absent, and that was wrong

It sat in the table above as ❌/❌ with the reason "a desktop window's
vocabulary". The mistake is visible in where it was written down: this is a table
of WINDOW CHROME fields, and beside `minimizable`, `maximizable` and
`width`/`height` — all genuinely desktop-window ideas — `zoomable` reads like
another one. It is not. It is CONTENT MAGNIFICATION, and a pinch belongs to a
touch screen more than to a trackpad. The touch reading was never considered.

`zoom.cplus` fills it. One platform difference and one platform trap:

- `NSMagnificationGestureRecognizer` reports magnification as an OFFSET since the
  gesture began, 0 meaning unchanged, so facet_appkit computes `base * (1 + m)`.
  `UIPinchGestureRecognizer` reports `scale` as a MULTIPLIER, 1.0 meaning
  unchanged. Copying appkit's arithmetic magnifies by 2x on the first callback of
  a pinch that has not moved.
- **appkit magnifies by shrinking the host's BOUNDS, and that does not port.** An
  NSView's frame and bounds are independent, so a smaller bounds over the same
  frame means "show less content in the same space" — magnification with no
  layout pass. UIKit keeps `frame.size` and `bounds.size` equal modulo the
  transform: measured, setting bounds to 100x50 on a 200x100 view moved the FRAME
  to 100x50 as well, so the view shrank and its content did not grow. This
  backend scales the LAYER TRANSFORM instead, which leaves the bounds — and
  therefore everything flex laid out — untouched. That is also why
  `scheduler::layout_window` keeps reading the content view's bounds where
  facet_appkit reads its frame: the two backends put the zoom in different places,
  so the layout has to read the one that does not move.

`set_zoom` is the programmatic path, and it is not optional: an agent has no
hands, so a pinch-only feature would be a feature no agent could reach.

`run_component` now forwards the trio too. It built a Chrome naming only
title/width/height, so magnification was unreachable from the simple entry on
BOTH platforms — an app had to adopt `screen::Screen` to ask for something that
has nothing to do with screens.

`bar` maps onto `UISceneWindowingControlStyle`: `Blended` and `Custom` →
`unified`, `Native` and `Hidden` → `minimal`, and nothing → `automatic`.

The rule is **`unified` only for an app that draws a top bar with room in it;
`minimal` for everything else** — and it is measured rather than reasoned.
Windowed on an iPad, only `minimal` puts the system's close/minimise/zoom pill
outside `safeAreaInsets`; under `unified` and `automatic` the pill is drawn over
the app's own top-left corner and nothing tells the app it happened. `automatic`
is unused because it is what UIKit picks when the delegate does not implement
the method at all. WINDOWING.md §4 and §9 have the numbers and the photographs.
The mapping is pinned by two checks rather than left to a reader.

**Three cannots, and they are cannots rather than not-yets.**

`minimizable` has nowhere to land ON IOS AT ALL. `UISceneWindowingBehaviors`
carries `closable` and `miniaturizable`, and its header describes them as the
buttons "on the NSWindow associated with this scene" — Mac Catalyst's window,
not an iPad's. Measured on iPadOS 26.1: `windowScene.windowingBehaviors` is
**nil**, on iPad and iPhone alike. The code reads the property and writes
nothing when it is nil, which is the right behaviour and also why this was
invisible until it was measured.

`maximizable` has no counterpart even where behaviours exist: the object carries
no third bit. It is dropped rather than mapped onto `closable`, because "may
this window be zoomed" and "may this window be closed" are different questions
and answering one with the other is a lie a reader cannot see.

`width` / `height` cannot be requested. `UIWindowSceneGeometryPreferencesIOS` —
the object `requestGeometryUpdate` takes — carries `interfaceOrientations` and
nothing else on iOS. An app that wants a fixed size sets `min_*` and `max_*` to
the same value.

### `nav::push` / `nav::pop` are refused

A pushed screen is a modal presentation on iOS — the same tier `alert`,
`prompt`, `choose` and `present_window` wait on. They answer `false` rather than
doing nothing silently, which is what their callers check.

---

## 7. `window_buttons`, and writing one toolbar for two platforms

The kind that moved out of §1. An iPad in iPadOS 26 windowed mode draws a real
close / minimise / zoom pill, so the platform this backend said had no window
buttons has window buttons.

### What the kind does here

It **reserves the corner**, and hosts nothing.

Hosting is what facet_appkit does: `CplusExtWindowButtonsView` re-parents the
window's real traffic lights into itself, so the group lands wherever the
application puts the node. That is not available here and not merely unbuilt —
the pill is drawn by the system outside the app's view hierarchy. A windowed
app's tree was walked to check, and it is four full-bleed views deep with no
control in it. No API vends the pill's frame either.

So the node occupies the space instead, and the application's own bar lays out
beside the controls rather than underneath them.

| condition | reserved |
|---|---|
| no windowing (every iPhone) | **0 × 0** |
| iPad **full screen** | **0 × 0** |
| `minimal` or `automatic` style | **0 × 0** |
| `unified`, and actually in a window | **60 × 40 pt** |

The zeroes are the part worth checking, and are checked. A phone has no window
controls; neither does a full-screen iPad, where they move up into the menu bar
and the app's top-left is its own again. Under `minimal` the controls sit
*above* the safe area — measured — so the app is already clear of them and
reserving would open the same gap twice.

**The gap appears and disappears with the window**, which was not the first
design. This reserved on "the platform can window" alone, and the cost showed up
the moment it ran: a full-screen iPad drew 60pt of empty toolbar with nothing
behind it. The reflow was the thing being avoided, but a reflow here is correct
— the controls really do appear at that moment. `UIWindowScene.isFullScreen` is
macCatalyst-only, so the test is the window's bounds against the screen's, and
`windowScene:didUpdateEffectiveGeometry:` re-marks the trees when it changes.

**60 × 40pt is the one hardcoded metric in this package**, and it is hardcoded
because nothing can be read: photographed on an iPad running iPadOS 26.6, the
pill occupied x 19.5–60pt and y 20.5–40pt from the window's top-left corner.
WINDOWING.md §9 has the picture.

Two consequences worth stating outright:

- **The node must be the leading item in the application's bar.** The pill is
  placed relative to the WINDOW, not to this node, so a reservation pushed
  inboard by the bar's own padding does not cover it.
- **Full screen is not detected.** `UIWindowScene.isFullScreen` is
  macCatalyst-only, so the only available test is comparing the window's bounds
  against the screen's — and a toolbar that reflows the moment the user drags
  the window off full screen is a worse artefact than a constant 60pt of
  leading space.

### The same source on both platforms

This is the point of answering the kind at all rather than telling applications
to branch. **Both backends now size the node themselves**, so the toolbar an
application writes is the same text on macOS and iPadOS:

```cplus
b.add(ui::window_buttons(key: "app:window-buttons").height(BAR_H));
```

No width, and no `#platform()`. The width comes from the backend:

| | width | from |
|---|---|---|
| facet_appkit | 74pt | `fittingSize` on the hosting view |
| facet_uikit, in a window + `unified` | 60pt | the measured reservation |
| facet_uikit, anywhere else | 0pt | there are no controls to avoid |

The height stays the application's, because it is a statement about the
application's bar rather than about the buttons — facet_appkit centres the real
traffic lights on the host's midline, and a host left at its natural height has
no midline to centre against.

A `.width(74.0)` written at the call site still works and still wins, but it is
now a macOS number hardcoded into portable source: it makes the iPad reserve
74pt where it needs 60, and an iPhone reserve 74pt where it needs none. iris
carries one from before either backend measured this kind, and dropping it is
the whole migration.
