# C+ AppKit Bindings Plan (`vendor/appkit`)

Status: **backlog / triage.** `vendor/appkit` already ships broad Cocoa coverage
(16 modules, ~2,700 lines). This document records what exists, the one
load-bearing gap (no ownership/release model), the verified coverage holes, and
candidate directions for the next round. It is shaped like [plan.md](plan.md):
pick a theme, not all of it lands at once.

## 1. Current state (what's shipped)

A check of `vendor/appkit/` (`[link]` = `Cocoa` + `objc`):

- **Facade**: `appkit/appkit` re-exports the per-area modules; narrower imports
  (`appkit/window`, `appkit/controls`, …) are also supported.
- **runtime** (`runtime.cplus`, ~350 lines): geometry types (`Point`/`Size`/`Rect`),
  the `rt::get_class` / `rt::sel` / `rt::msg_*` ObjC message helpers, NSString
  helper. Every higher module sits on this.
- **Widget coverage is wide**: `application` (`AutoreleasePool`, `Application`,
  app-delegate helper), `window`, `view` (StackView/ScrollView/Box/…),
  `controls` (TextField/Button/Slider/PopUpButton/Stepper/Switch/Segmented/
  DatePicker/ColorWell/…), `text` (TextView/SecureTextField/SearchField/
  TokenField/ComboBox/Form), `containers` (SplitView/TabView/GridView/Browser/
  Popover/…), `data` (TableView/OutlineView/CollectionView + columns/cells/
  layouts), `graphics` (ImageView/Image/Font/Color), `menu`, `dialogs` (Alert),
  `panels` (Save/Open/Print), `toolbar` (Toolbar/StatusItem/TouchBar),
  `controllers` (ViewController/WindowController/Split/Tab/Array/Object).
- **Callback model**: closure-free `set_on_click(fn(*u8))`; the runtime stashes
  the fn on the sender via `objc_setAssociatedObject` (no environment capture,
  per locked principle #2).
- **Data bridge** (`convert.cplus`): `cplus_{str,string}_to_nsstring` /
  `nsstring_to_cplus_*`, `nsarray_*`, `nsdata_*`. Documents that returned `str`
  views borrow the **autorelease pool's** lifetime, not the caller's.

So coverage is not the headline problem. The headline is correctness.

## 2. The load-bearing gap: no ownership / release model

Every wrapper is `struct X { pub obj: *u8 }`, and a grep across `src/` finds
**zero** `objc_release` / `objc_retain` / `CFRelease` / `fn drop` calls. Objects
are `alloc`/`init`-ed and never released. Nothing has a destructor.

Today this survives for three accidental reasons, none of them a model:

1. The `AutoreleasePool` mops up convenience-constructor objects.
2. Objects added to the view hierarchy are retained by their parent (the window
   retains its content view, a stack view retains its arranged subviews, …), so
   the leaf wrapper going out of scope does not free anything the UI still uses.
3. A GUI app runs until `exit`, and process exit reclaims everything.

The leaks that *do* happen are the off-pattern ones: an `alloc`/`init` object
that is never added to a parent, a transient `Alert` or `Image` created in a
loop, a wrapper dropped before it is installed. There is no RAII to catch them,
and no way for the type system to know an `obj` is owned.

This is exactly the **foreign-handle** case in [plan.opaque.md](plan.opaque.md):
`obj: *u8` is a pointer the C+ side holds but a foreign runtime (the ObjC
retain/release machinery) manages. Two coupled decisions:

- **Ownership / `Drop`.** A blanket "every wrapper releases its `obj` in `drop`"
  is *wrong* — it would over-release. ObjC ownership is construction-path
  dependent: `alloc`/`new`/`copy`/`mutableCopy` return +1 (the caller owns a
  release); convenience constructors return autoreleased (+0); an object added
  to a parent is retained by the parent. A correct model must (a) `retain` on
  capture or track the +1, and (b) `release` exactly once in `drop`, matching
  the construction path. Getting this right is the real work; it is the same
  retain/release discipline ARC automates in Swift/ObjC, done by hand here.
- **`opaque` marker.** When [plan.opaque.md](plan.opaque.md) lands, every
  `obj: *u8` must be *accounted for*: either released by a `drop` (the owned
  case) or marked `opaque` (managed elsewhere — e.g. a child view whose parent
  owns it, or a borrowed handle). This is the ~70-field migration that doc's §9
  references; `vendor/appkit` is the bulk of it. The per-field choice (drop vs
  `opaque`) *is* the ownership audit above, written into the types.

This is the strongest standalone headline: it is correctness, it is bounded
(one runtime + N wrapper structs), it composes with the `opaque` work, and it
turns "AppKit objects might leak" into a compiler-checkable fact.

## 3. Eventing and delegates

Current eventing is the single `set_on_click(fn(*u8))` + associated-object
pattern, plus some delegate wiring (7 files reference `delegate`). Gaps:

- **No `NSEvent`** surface (0 files): raw key/mouse events, modifier flags,
  event monitors.
- **No `NSNotification` / `addObserver`** (0 files): the notification center,
  the standard cross-object signal path.
- The target-action + delegate model exists ad hoc per widget; it could be a
  single documented pattern (one `(fn_ptr, user_data)` convention, one
  delegate-class-synthesis helper) instead of per-control bespoke wiring.

## 4. Verified coverage holes

Probed against the source; absent today:

| Capability | Status | Notes |
| :--- | :--- | :--- |
| Auto Layout / `NSLayoutConstraint` | **absent** | only autoresizing-mask hints appear; no constraint API. The biggest real-app gap. |
| `NSEvent` (key/mouse) | **absent** | see §3 |
| Custom drawing (`drawRect:`, `NSBezierPath`) | **absent** | no path/drawing surface; blocks custom views |
| Pasteboard / clipboard | **absent** | no copy/paste |
| Drag and drop | **absent** | depends on pasteboard + events |
| `NSNotification` center | **absent** | see §3 |

Widget *breadth* is already strong (§1); these are *depth* gaps that a real app
hits. Triage them against a concrete target app (§5) rather than binding
exhaustively up front.

## 5. A real desktop-app milestone

`appkit_hello` (`docs/examples/recipes/appkit_hello/`) proves a window + button.
The next driver is a complete small app — a list+detail (master/detail with a
`TableView`, a menu, an `Alert`, a save dialog) — chosen because it exercises
data sources, selection events, menus, and dialogs together, and will surface
the §3/§4 gaps in priority order instead of guessing. Let the app lead and file
gaps as it hits them, the way the llama.cplus port drives the compiler.

## 6. Recommendation

Three coherent shapes:

- **"ObjC ownership model"** (§2): the strongest standalone headline. Correctness,
  bounded surface, composes with [plan.opaque.md](plan.opaque.md) (the appkit
  `obj` fields are the bulk of that migration). Best landed *with or just after*
  `opaque`, so the per-field drop-vs-`opaque` decision is written into the types
  once.
- **"App-driven depth"** (§5 + §3/§4): build the list+detail milestone and close
  the Auto Layout / events / drawing gaps it surfaces, in hit order. Highest
  user-visible payoff; coverage-led.
- **"Eventing model"** (§3): a single documented target-action + delegate +
  notification pattern, replacing the per-widget ad hoc wiring. Smaller, enables
  the app milestone.

Suggested shape: **§2 as the headline, sequenced with `opaque`** (do the
ownership audit and the `opaque`/`drop` annotation in one pass), with **§5
driving** which §3/§4 gaps to close next. §2 is correctness and doesn't expand
surface; §5 expands surface and is best paced by a real app.

## 7. Relationship to other plans

- [plan.opaque.md](plan.opaque.md): the `obj: *u8` handles are the canonical
  foreign-handle case; §2 here is the AppKit-side instance of that rule, and the
  bulk of its §9 migration.
- [plan.graph.md](plan.graph.md): once method-dispatch call edges land, the
  graph can enumerate which wrappers have a `drop` and which `obj` fields are
  released vs `opaque` — a mechanical aid to the §2 audit.
