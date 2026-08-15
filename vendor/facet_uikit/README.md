# facet_uikit

facet's **UIKit** backend — the iOS counterpart of `facet_appkit`.

> **Status: compiles for iOS, never run.** `cpc build --target ios-arm64`
> produces a real arm64 iOS static library (`platform 2`, minos 13.0) and
> `--target ios-arm64-simulator` produces the simulator one. Nothing in this
> package has been on a device or in a simulator. Every runtime behaviour —
> layout, measurement, view ownership, the run-loop tick, target/action
> delivery — is **unvalidated**. `facet_appkit` is the reference implementation
> and the only tested one.

The whole package goes through LLVM and out to an object for the real target,
which is more than a type-check proves. It is a much weaker position than
"works".

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
| `input.cplus` | target/action and tap recognisers → facet handlers |
| `scheduler.cplus` | the CFRunLoop tick, `run_on_main`, `after` |

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
