# facet_uikit

facet's **UIKit** backend — the iOS counterpart of `facet_appkit`.

> **Status: compiles for iOS, never run.** `cpc build --target ios-arm64`
> produces a real arm64 iOS static library (`platform 2`, minos 13.0) and
> `--target ios-arm64-simulator` produces the simulator one. Nothing in this
> package has been on a device or in a simulator. Every runtime behaviour —
> layout, measurement, view ownership, the run-loop tick, target/action
> delivery — is **unvalidated**. `facet_appkit` is the reference implementation
> and the only tested one.

That is a stronger position than `vendor/facet_gtk`, which type-checks but has
never been through codegen; the whole package here goes through LLVM and out to
an object for the real target. It is a much weaker position than "works".

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

## Not wired into facet yet

facet selects a backend through a platform-suffixed facade —
`facet/src/runtime_macos.cplus` on macOS, chosen by the resolver from
`target::active_platform()`. There is no `runtime_ios.cplus`, so importing
`facet/runtime` on an iOS target still lands on the neutral no-backend base.
Writing that facade (and `[ios.dependencies]` in facet's manifest, which the
manifest parser already supports) is the next step, and it is deliberately
separate: it puts this package into facet's own build on an iOS target, and a
backend that does not build turns facet red.

Until then an application installs the backend directly:

```cplus
import "facet_uikit/facet_uikit" as backend;
backend::install();
```
