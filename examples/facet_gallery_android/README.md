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

## The JVM side: none of it is this app's

**There is no `.java` file in this directory.** The manifest names
`cplus.facet.FacetActivity`, which lives in facet_android and ships as a
precompiled DEX; `build.sh` feeds that DEX to `d8` alongside nothing else, so it
lands in this APK's `classes.dex`. The meta-data line tells the generic Activity
which `.so` to load, the way `NativeActivity` takes `android.app.lib_name`.

What this app writes instead is five lines of C+:

```cplus
export extern fn Java_cplus_facet_FacetActivity_nativeCreateView(
    envp: *jni::JNIEnv, cls: jni::jobject, activity: jni::jobject,
) -> jni::jobject {
    return entry::start(envp, activity, build);
}
```

The export lives in the app rather than in the package on purpose: cpc emits one
object per package, so a package that names a symbol obligates everything that
links it — see plan.android.md finding 3.

facet_android still has plenty of Java of its OWN — the layout host, the
Choreographer tick, the Activity, and one listener adapter per event shape,
eight files in `vendor/facet_android/java/`. The point is not that the Java is
gone. It is that a person writing an Android app in C+ never opens one.
