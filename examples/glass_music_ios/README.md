# glass_music_ios

A music app's "New" tab, built to show **Liquid Glass on iOS 26** with facet.

The bottom bar is facet's `tabs`, which on iOS is the system's `UITabBar`. On
iOS 26 that bar is Liquid Glass. The "New" pane is a `scroll`, so the page runs
under the floating bar down to the screen edge, and its content is inset so the
last row can scroll clear of it. The album art is seen through the glass.
Headings use `named_size` (the platform's text styles), so they follow Dynamic
Type.

## Run

Boot an iOS 26 simulator yourself, then:

```
./run_sim.sh <udid>        # xcrun simctl list devices booted
```

The script links `vendor` to the repo's packages, so the example builds
against the working tree and not the released store. It uses the repo's own
`target/release/cpc`.

## Not yet

The mini player that sits above the bar in Apple Music is UIKit's
`UITabBarController.bottomAccessory`. facet's `tabs` is a bare `UITabBar` and
has no word for an accessory yet.
