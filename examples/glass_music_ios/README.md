# glass_music_ios

A music app's "New" tab, built to show **Liquid Glass on iOS 26** with facet.

The bottom bar is facet's `tabs`, which on iOS is the system's `UITabBar`. On
iOS 26 that bar is Liquid Glass. The "New" screen is a `scroll`, so the page runs
under the floating bar down to the screen edge, and its content is inset so the
last row can scroll clear of it. The album art is seen through the glass.
Headings use `named_size` (the platform's text styles), so they follow Dynamic
Type.

## How it is laid out

```
src/
  main.cplus          the iOS entry point; hands over to app::run
  app.cplus           registers the window and one route per screen
  shell.cplus         the tab bar: each tab is a slot showing a route
  helpers.cplus       colours, album covers, the album card, the shelf
  catalog.cplus       the albums the screens show
  screens/
    home.cplus  new_music.cplus  radio.cplus  library.cplus  search.cplus
    album.cplus       pushed into the New tab when an album is tapped
```

The shell never builds a screen itself. Each tab is a
`slot(name:, route:) { }`, and the window's routes say which screen fills it.
Tapping an album pushes the `album` route into the New tab's slot, and the
album's Back pops it.

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
