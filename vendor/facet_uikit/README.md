# facet_uikit

facet's **UIKit** backend — the iOS counterpart of `facet_appkit`.

> **Status: runs in the simulator; not on a device.** `cpc build --target
> ios-arm64` produces a real arm64 iOS static library (`platform 2`, minos 13.0)
> and `--target ios-arm64-simulator` produces the simulator one.
> `examples/facet_gallery_ios` links against it and runs: layout, measurement,
> view ownership, the run-loop tick and target/action delivery have all been
> observed working. What is **unvalidated** is FEEL — a drag, a fling, momentum,
> the exact moment a swipe opens — because those cannot be asserted from a
> screenshot and nobody has held the phone.

**Prop parity with `facet_appkit`: 305 of 318 (95%).** The thirteen-prop
difference is MANIFEST.md §1 in full — a date or time picker's font band (twelve
verbs UIDatePicker has no property for) and a window's chrome style. Nothing is
missing for want of writing.

That number is a measurement, not a claim — `tools/parity.py` walks facet's
contract modules and asks which bits each backend names:

```
python3 vendor/facet_uikit/tools/parity.py      # from the repo root
```

Anything it prints under UIKIT MISSING must appear in MANIFEST.md §1. It also
reports the other direction, which is how the four props `facet_uikit` honours
and `facet_appkit` does not were found.

## Build

```
cd vendor/facet_uikit
cpc build --target ios-arm64             # device: target/ios-arm64/debug/libfacet_uikit.a
cpc build --target ios-arm64-simulator   # simulator
cpc check --target ios-arm64             # sema + borrowck only, faster
```

Xcode owns the final link — an iOS target stops at object emission by design
(`plans/plan.backends.md`, rung 1). Add the `.a` and a bridging call to
`facet_uikit::install()` from the app's `main`.

## What it is

facet owns the description tree (`facet::Node`) and all layout (the shared
`flex_layout` engine). This package fills the same five-verb `Renderer` seam
`facet_appkit` fills, and no wider surface:

| File | Role |
|---|---|
| `facet_uikit.cplus` | `install()` — the whole registration surface |
| `views.cplus` | `create` / `apply` / `insert` / `remove` / `view_release`, the backing rule, measurement, and this package's tests |
| `controls.cplus` | one body per kind |
| `paint.cplus` | the shared band — colour, brush, shadow, clip, radius, transform, `animate_*` |
| `geometry.cplus` | the layout pass and the frame walk |
| `input.cplus` | target/action and tap recognisers → facet handlers, and the radio group |
| `scheduler.cplus` | the CFRunLoop tick, `run_on_main`, `after` |
| `text_input.cplus` | the field / editor / search band, style runs, the length limit |
| `recycler.cplus` | `list`, `table`, `tree` on UITableView; `collection`, `carousel` on UICollectionView |
| `drawing.cplus` | the canvas replay — facet's recorded display list into a `CGContext` |
| `swipe.cplus` | swipe-to-reveal, driven by a pan so `reveal_threshold` means something |
| `web.cplus` | `web` and `hybrid_web` over WKWebView |
| `dates.cplus` | facet's `Date` / `Time` ↔ `NSDate`, through `NSDateComponents` |
| `window.cplus` | the app delegate, the window, and the first tick |

## Where UIKit is SHORTER than AppKit

Worth reading before porting anything else from `facet_appkit`, because these
are the places where copying that code would add machinery for no reason:

- **No flip.** A UIView is top-left origin natively. `facet_appkit` creates a
  flipped subclass for every node and says so in three files.
- **Always layer-backed.** No `setWantsLayer:`, so every paint verb is one call
  shorter and every clear is unconditional.
- **No appearance dance.** A `UIColor` resolves against the current trait
  collection when asked; `NSColor` flattens against whatever appearance is
  current, which costs the AppKit backend a re-entrant saved-static wrapper
  around every configure.
- **No document view.** A `UIScrollView` scrolls its own subviews.
- **`insertSubview:atIndex:` exists**, so slot ordering is one call.
- **`sizeThatFits:` takes the bound**, so a wrapping label needs no
  `preferredMaxLayoutWidth` pre-step.
- **`isSecureTextEntry` is a property**, so there is no live reclass path at all
  — `views::reclass` exists in the AppKit backend for that single prop.
- **One synthesized class, not one per kind.** Target/action takes a separate
  target object, so no view has its class moved.

Three things are genuinely richer here: SF Symbols (`systemImageNamed:` is the
whole implementation), Dynamic Type, and the `keyboard` band — which facet
declares on every input and macOS can do nothing with.

## What is not here

`MANIFEST.md` is the authority, and it keeps two lists strictly apart:

- **decided absent** — iOS has no such thing (window buttons, the menu bar, a
  draggable split divider). Finished; silent at runtime.
- **not yet built** — iOS has an answer and this pass did not write it (the
  list/table/collection/tree recycling tier, web, canvas, swipe actions). Each
  renders its children through a plain backing view and **warns once on stderr**
  when mounted, so the debt is audible.

The largest single piece of work left is the recycling tier: `facet_appkit`'s
`recycler.cplus` is 2,900 lines and has no counterpart here.

## Wired into facet

`import "facet/runtime"` resolves to `facet/src/runtime_ios.cplus` on an iOS
target — the same filename override that picks `runtime_macos.cplus` on a Mac,
keyed off `target::active_platform()`. facet's manifest carries
`[ios.dependencies]`, so **facet's own build on iOS compiles this package**,
exactly as it compiles `facet_appkit` on macOS. The consequence is the one
facet's manifest already states for the Mac: a backend that does not build turns
facet red.

`facet/src/agent_ios.cplus` shadows `agent.cplus` the same way, because that
module is written against `agent_appkit` and there is no `agent_uikit`. It keeps
the surface compiling and refuses loudly.

```
cd vendor/facet && cpc check --target ios-arm64     # facet + this backend
```

### One structural difference an app author must know

`UIApplicationMain` **does not return**. `[NSApp run]` does, and the macOS
facade is built on that — `run_component` blocks and then hands the component's
final state back, `App::run` loops (open a window, block, close, read the nav
intent, open the next).

So on iOS:

- `run_component` / `run_screen` keep their signatures and never return, so the
  value they promise never arrives. The component lives in the frame that
  entered the loop, which is the process's bottom frame forever.
- `App::run` shows the initial screen and enters the loop. **Navigation is a
  swap**: `nav::go` builds the next screen into the same window, because a phone
  has one window and closing it is not something an app may do.
- `on_quit` never fires and nothing is torn down. That is what iOS termination
  is — apps are killed, they do not wind down. `observe_backgrounding` is the
  hook the platform actually gives for saving state.
