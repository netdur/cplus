# Facet Gallery — Android

The same catalog idea as `facet_gallery` (macOS) and `facet_gallery_ios`, on
facet's Android backend. Five screens, one per group of controls, described in
facet's vocabulary and laid out by flex.

```sh
export ANDROID_SDK_ROOT=~/Library/Android/sdk
~/Library/Android/sdk/emulator/emulator -avd Pixel_9_Pro_XL &   # or a real device
./build.sh                                                     # builds, installs, launches
```

`build.sh` is the whole pipeline in fifty lines — cpc, the NDK linker, `d8`,
`aapt2`, `zipalign`, `apksigner`, `adb install`. **No Gradle.** Every step is
visible, which is the point; adopt Gradle when packaging demands it, not for
comfort.

## What to look at

| screen | what it is showing |
|---|---|
| Buttons | `button`, `text_button`, `icon_button`, each writing to one echo line |
| Text | `label` at three sizes, and the three input kinds that share facet's `input_view` block |
| Toggles | `checkbox`, `toggle`, `radio` — all three are `CompoundButton` on Android, so one adapter carries every change |
| Values | `slider` driving the `progress` bar beside it, the composite `stepper`, and a `spinner` that collapses when stopped |
| Layout | rows, `grow` at 1:2:1, nesting, and a page long enough to scroll |

**Every screen writes to a line at the bottom from its handlers.** A control
that visibly moves while that line stays still has a dead read half, which is
invisible from the write side — the failure this app exists to make obvious.

Navigation is `mount::switch_to`: each screen is built ONCE and parked under
`Display::None`, so a screen you return to keeps its scroll position, its typed
text and its selections.

## What is deliberately absent

`vendor/facet_android/MANIFEST.md` §2 is the ledger. The short version: fifteen
kinds have bodies. `list`, `tree`, `table` and `collection` wait on the
recycler; `canvas`, `web`, the pickers and the menu tier are unwritten; an
image can only come from the filesystem, not from inside the APK.

A gallery screen whose controls do not exist would be a screen of empty boxes,
so this app grows as the backend does rather than showing the gaps.

## The whole JVM side

`java/cplus/gallery/MainActivity.java`, thirteen lines. facet_android ships its
own Java — the layout host, the Choreographer tick, and one listener adapter per
event shape — inside the package as a DEX.
