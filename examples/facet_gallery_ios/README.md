# facet_gallery_ios

> **Putting this on a real iPhone or iPad: [DEPLOYING.md](DEPLOYING.md).**
> Device detection, certificates, teams, provisioning, the free-account limits
> and the trust step — every command, and every error they actually produce.

The phone-shaped gallery — `examples/facet_gallery`'s counterpart on iOS, and
the app to point at a simulator when `facet_uikit` is run for the first time.

> **It runs.** Launched on an iPhone 17 Pro simulator (iOS 26.4, arm64) on
> 2026-08-16. Labels, button, slider, progress, stepper, switch, the segmented
> picker, card surfaces, the safe area, scrolling and the value write-back are
> all correct on screen.
>
> Never on a device.

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
   `libfacet_gallery_ios.a`, **every `vendor/*/lib/<artifact-triple>/*.a`**,
   and the frameworks — `UIKit`, `QuartzCore`, `Foundation`, `CoreGraphics`,
   `WebKit`, plus `libobjc.tbd`.

   > The dependency archives are not optional and did not used to be needed.
   > `prebuild` became the default on 2026-08-16: a library package is
   > compiled once into `vendor/<pkg>/lib/<triple>/lib<pkg>.a` and linked
   > thereafter, so its object code is no longer inside this app's archive.
   > Miss them and the link fails on symbols nothing defines — typically
   > `Vec[T]` instantiated over a dependency's type, because a generic has no
   > object code until a consumer instantiates it and the instantiation calls
   > a concrete method that lives only in that dependency's slice.
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

## Run it from the command line, without Xcode

Faster than the GUI, and how everything above was verified:

```
cd examples/facet_gallery_ios
cpc build --target ios-arm64-simulator

# Every prebuilt dependency slice, as step 4 explains. Globbing over-links,
# which costs nothing: an archive nothing references contributes no bytes.
slices=$(find ../../vendor -maxdepth 4 -path '*/lib/arm64-apple-ios-simulator/*.a')

xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=14.0 \
  -I target/ios-arm64-simulator/debug \
  ios/main.m target/ios-arm64-simulator/debug/libfacet_gallery_ios.a \
  $slices \
  -framework UIKit -framework QuartzCore -framework Foundation \
  -framework CoreGraphics -framework WebKit -lobjc -o Gallery.app/Gallery
# Gallery.app also needs the Info.plist from the Xcode recipe above.

xcrun simctl boot "iPhone 17 Pro"
xcrun simctl install booted Gallery.app
xcrun simctl launch --console-pty booted dev.cplus.facetgalleryios
xcrun simctl io booted screenshot shot.png
```

`--console-pty` carries `io::eprintln`, which is how a live view was
interrogated from `on_attach` while the app ran. `open -a Simulator` puts the
device on screen; without it the whole loop is headless.

## What was wrong on the first run

Five bugs, all found by running and none by reading:

1. **No content at all** — `on_attach` fired before the tree was mounted. Fixed:
   the host has a launch seam the facade hangs the lifecycle on.
2. **Content under the status bar** — `safeAreaInsets` reads zero inside
   `didFinishLaunching`. Fixed: key and visible first, then a forced layout.
3. **Every label a black bar** — `isOpaque` YES with no background colour. Fixed:
   `opaque` is cleared with the background.
4. **Nothing scrolled** — the flex trap above.
5. **A `button` drew nothing** — and this one was not iOS, not UIKit, and not
   this package. `musttail` was freeing the stack frame that an
   indirectly-passed argument pointed into, so facet's 25-parameter
   `elements::button` forwarder lost both its `vocab::Color` arguments and the
   backend built a `UIColor` with alpha 0 out of the remains of a stack
   pointer. Fixed in the compiler; **not one line of `facet_uikit` changed**.
   `bugs/closed/ios-target-defaulted-struct-param-garbage.md`.

The fifth is worth reading even if you never touch iOS. It only appeared here
because an iOS target builds a **library archive**, and the library pipeline
gives every name-public function `weak_odr` linkage — which forfeits `fastcc`,
which is what had been passing the big struct in registers. The same source
built as an executable was correct on every platform, which is why it read for
three sessions as a UIKit problem.

Not yet exercised at all: the dark-mode flip (known unwired — UIKit has no
application-level appearance hook), and every kind on the Gaps page.

`vendor/facet_uikit/MANIFEST.md` is the authority on what is unfinished, and it
keeps "iOS cannot" and "not built yet" strictly apart.
