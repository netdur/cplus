# facet_gallery_ios

The phone-shaped gallery — `examples/facet_gallery`'s counterpart on iOS, and
the app to point at a simulator when `facet_uikit` is run for the first time.

> **Nothing here has run.** The package compiles to a real iOS static library
> for both the device and the simulator. It has never been launched, so
> everything below the build step is a recipe, not a report. When it does run,
> the things most likely to be wrong are listed at the bottom.

## Build

```
cd examples/facet_gallery_ios
cpc build --target ios-arm64-simulator     # target/ios-arm64-simulator/debug/
cpc build --target ios-arm64               # target/ios-arm64/debug/
```

Each produces `libfacet_gallery_ios.a` plus `facet_gallery_ios.h`, which
declares the one exported entry:

```c
int32_t facet_gallery_ios_main(void);
```

## Run it in Xcode

An iOS target stops at object emission — Xcode owns the final link — so the app
bundle is Xcode's and the code is cpc's.

1. **New project → iOS → App.** Language Objective-C. Delete `AppDelegate.h/m`,
   `SceneDelegate.h/m`, `ViewController.h/m`, `Main.storyboard`.
2. Add `ios/main.m` from this directory to the target (it replaces Xcode's).
3. **Info.plist**: remove `UIApplicationSceneManifest` and
   `UIMainStoryboardFile`. facet_uikit synthesizes its own app delegate and
   builds the UIWindow itself; a scene manifest would put UIKit in charge of the
   window instead and nothing would appear.
4. **Build Phases → Link Binary With Libraries**: add
   `libfacet_gallery_ios.a`, and the frameworks it needs — `UIKit`,
   `QuartzCore`, `Foundation`, `CoreGraphics`, plus `libobjc.tbd`.
5. **Build Settings → Header Search Paths**: the `target/<triple>/debug/`
   directory, so `#import "facet_gallery_ios.h"` resolves.
6. **Library Search Paths**: the same directory. Point the simulator
   configuration at `ios-arm64-simulator` and the device one at `ios-arm64` —
   they are different archives, and linking the wrong one fails with an
   architecture mismatch rather than anything more helpful.
7. Run.

Rebuild the archive with `cpc` after any C+ change; Xcode does not know how.

## What is on screen

Five pages behind a segmented picker. The picker is a `popup`, which on this
backend is itself a `UISegmentedControl` — so the shell is one of the
approximations the last page names.

| Page | What it is for |
|---|---|
| **Controls** | every kind with a real UIKit body, wired to handlers. Move a slider and the label beside it reads the value back **out of facet's props** — that is the write-back path, which is the thing most likely to be subtly wrong |
| **Text** | type, alignment, fields — and the `keyboard:` band, which exists because a phone has one and which AppKit can do nothing with at all |
| **Paint** | radius, brush, shadow, clip, opacity, transform. Core Animation is the same framework on both platforms, so **this page should look identical to the desktop gallery's** |
| **Motion** | `animate_*`, plus the entrance pattern and one button that is deliberately wrong (the dead snap) so its stderr line can be seen |
| **Gaps** | what is NOT built, shown rather than described: a `list` that warns once, a `split` that is silent because it is answered, and the approximations |

## The one layout trap, written down because it cost an evening

**flex defaults `flex_shrink` to 0** — Yoga's deviation from CSS, inherited by
the engine — so an item never lays out smaller than its content. A scroll's
whole job is to be smaller than its content, so a `scroll` **and every container
between it and the window** needs `.shrink(1.0f64)`. Miss one and that container
lays out at its content's height, pins everything below it, and the scroll view
ends up exactly as tall as what is inside it: `contentSize == frame`, and
nothing scrolls.

Measured here, before the fix: `frame 402x1137` on an 874pt screen, with
`contentSize 402x1137`. After: `frame 402x730, content 402x1137`.

It is not an iOS bug — the same rule applies on macOS. A phone just runs out of
screen sooner.

## What to watch for on the first run

In rough order of how likely each is to be the thing that breaks:

1. **The app does not start at all.** `run_loop` calls `UIApplicationMain` with
   `argc 0` / `argv NULL`, which is untested. Everything else depends on this.
2. **A blank window.** The mount happens in the launch hook and the first
   layout is forced there (`scheduler::tick_now`); if the tree is mounted but
   unsized, that call or the frame walk is where to look.
3. **Content under the notch.** The safe-area inset is read from the root view
   controller's view and applied by the layout pass.
4. **Controls at zero size.** `sizeThatFits:` is asked with the available width;
   a control answering zero is floored, and a control answering something absurd
   is not.
5. **A tap that does nothing.** One shared target object serves every control;
   if one kind is dead, check that its `create_*` armed the right event.
6. **The dark-mode flip does nothing.** Known and recorded: UIKit has no
   application-level appearance hook, so nothing fires the repaint walk yet.
   Semantic colours still follow on their own; a gradient or a border will not.

`vendor/facet_uikit/MANIFEST.md` is the authority on what is unfinished, and it
keeps "iOS cannot" and "not built yet" strictly apart.
