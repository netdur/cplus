# The iPad is a window manager, and facet_uikit is not talking to it

Written against a physical iPad running iPadOS 26.6 and the iOS 26.1 simulator.
Every symptom below was photographed on the device before it was explained, and
every API quoted is from the iPhoneOS26.5 SDK headers on this machine rather
than from documentation.

**The short version.** An iPhone app is windowless and has nothing to negotiate.
An iPad app lives in a `UIWindowScene` the system manages, and since iPadOS 26
the app is expected to say how it wants that window presented. `facet_uikit`
runs a legacy `UIApplicationDelegate` with no scene manifest and therefore no
`UIWindowSceneDelegate`, so it cannot answer any of those questions — it is not
that it answers them badly. Every visible symptom follows from that one fact.

---

## 0. What it looks like when it goes wrong

Side by side on the same iPad, in windowed mode:

- **Contacts** places its sidebar button *beside* the window controls, on the
  same row. It has a visible window edge and a resize grabber at the bottom
  right.
- **facet gallery** draws its own content starting at x = 0, so the system's
  controls land on top of it. The title reads `fa●●●uikit`. One screen deeper it
  is worse: the controls sit on `‹ Back`, which is a **tappable control
  underneath the system's own hit area**, not merely an obscured label.

Full screen, both are fine — the controls move up into the menu bar and the
app's top-left is its own again. So the problem is specific to the windowed
state, where the system puts something inside the app's coordinate space and the
app lays out as though it had not.

**What is NOT wrong**, checked before blaming it:

- *Scale.* Fixed separately, and for an unrelated reason — a missing
  `UIDeviceFamily` in `Info.plist` made iOS run the app as an iPhone app in
  compatibility mode. See §5.
- *Relayout.* Works. The gallery's Responsive demo tracks the window live —
  785pt → 556pt → 1194pt, with the third card and the side rail appearing and
  disappearing at their thresholds. `layoutSubviews` already drives this, as the
  demo's own "HOW IT KNOWS" note explains. **No new resize plumbing is needed.**
- *Flex.* Nothing here is a layout-engine bug. A screen that does not fill a
  wide window is that screen's own layout not growing.

---

## 1. The API surface, and where all of it lives

From the SDK headers:

| API | What it decides | Since |
|---|---|---|
| `-preferredWindowingControlStyleForScene:` | how the controls are presented | 26.0 |
| `UISceneWindowingControlStyle` | `automatic` / `unified` / `minimal` | 26.0 |
| `-windowScene:didUpdateEffectiveGeometry:` | told when the window's geometry changes | 26.0 |
| `UISceneWindowingBehaviors` | `closable`, `miniaturizable` | 16.0 |
| `UISceneSizeRestrictions` | `minimumSize`, `maximumSize` | 13.0 |

Every single one hangs off the **scene** or the **scene delegate**. There is no
application-level route to any of them. An app without a scene does not get a
worse answer; it gets no question.

The three styles, quoted:

- `automaticStyle` — "Windowing controls will use the default system style"
- `unifiedStyle` — "Windowing controls will appear as part of the scene's content"
- `minimalStyle` — "Windowing controls will occupy as little of the scene's space as possible"

And the sentence that explains the overlap exactly:

> Called by the system to determine the windowing control style for the provided
> scene. **`automaticStyle` will be used if this method is not implemented.**

`facet_uikit` cannot implement it, because it has no scene delegate to implement
it on. So it gets `automatic` by default, which overlays the controls on the
app's own top-left. Contacts is almost certainly `unified`, which is why its
button sits beside them instead of under them.

---

## 2. Why facet_uikit is outside all of it

`window.cplus` builds its `UIWindow` inside
`application:didFinishLaunchingWithOptions:`, and `Info.plist` names no
`UIApplicationSceneManifest`. That is a deliberate, documented choice — but the
note recording it conflates two different things:

> the Info.plist must NOT name a storyboard or a scene manifest

**Only the storyboard is actually forbidden.** A storyboard makes UIKit wait for
a nib that facet does not have, and the screen stays black. A scene manifest is
a different key, and the normal shape for a programmatic app is a manifest that
names a `UISceneDelegateClassName` and **no** `UISceneStoryboardFile`. The two
were correctly avoided together and only one of them had to be.

The cost of that conflation is everything in §1.

---

## 3. facet already has the vocabulary

This is the part that makes the fix worth doing rather than merely possible.
`screen::Chrome` already carries the fields, and they map one to one:

| `Chrome` field | iPad scene API |
|---|---|
| `min_width` / `min_height` | `sizeRestrictions.minimumSize` |
| `max_width` / `max_height` | `sizeRestrictions.maximumSize` |
| `minimizable` | ~~`windowingBehaviors.miniaturizable`~~ — **wrong, see §9** |
| `maximizable` | ~~`windowingBehaviors.closable` / fullscreen~~ — **wrong, see §9** |
| `title_text` | the window's title in the menu bar |
| `bar` | the windowing control style — **added in §9** |

Nothing new has to be invented at the facet tier. `MANIFEST.md` currently says
"`Chrome` is almost entirely inert … the screen is the window here", and that
sentence is true of a phone and false of an iPad.

**Two rows of that table were guesses and both were wrong**, which is recorded
here rather than quietly corrected because the shape of the mistake is the
lesson: they were read off a header's *names* without checking whether the
object exists at runtime. It does not — `windowingBehaviors` is nil on iPadOS.
§9 has the measurement. The `bar` row is the one that replaced them, and it was
not in the original table at all.

`width` / `height` stay inert on iOS either way, and that is not an omission:
`UIWindowSceneGeometryPreferencesIOS` — the object `requestGeometryUpdate` takes
— carries `interfaceOrientations` and nothing else on iOS. **There is no
"open at this size" call.** An app that wants a fixed size says so by setting
min and max to the same value.

---

## 4. What is not yet known

One question the headers do not settle, and it decides how much layout work
follows adoption:

**Do the window controls contribute to `safeAreaInsets`, and does that differ
per style?** No API exposes the controls' frame. That absence suggests the
intended answer is not "read an inset and dodge it" but "declare a style and lay
out normally" — but that is an inference, not a measurement.

It is cheap to settle: log `view.safeAreaInsets` under each of the three styles,
windowed and full screen, on the iOS 26 iPad simulator. Until that is run, do
not write inset arithmetic — the odds are good that `unified` or `minimal` makes
it unnecessary, and inset arithmetic that turns out to be redundant is the kind
of code nobody deletes afterwards.

**ANSWERED — and the answer is YES, for exactly one of the three styles.**
Measured on a physical iPad running iPadOS 26.6, windowed at 400x505, with
nothing changed between runs but `Chrome.bar`:

| style | safe insets | the control pill sits at | verdict |
|---|---|---|---|
| `automatic` | `10,0,10,0` | 20 – 40pt from the window top | **inside** the safe area — overlaps content |
| `unified` | `10,0,10,0` | 20 – 40pt | **inside** — overlaps content |
| `minimal` | `32,0,10,0` | 5 – 26pt | **above** the safe area — content is clear |

So the inference in the paragraph above was half right. "Declare a style and lay
out normally" IS the intended answer, but only `minimal` makes it true; under
the other two the system draws its pill over the app's own top-left corner and
the safe area does not move to accommodate it.

**The standing instruction stands, and for a better reason than before.** Do not
write inset arithmetic — not because the controls never need dodging, but
because an app cannot dodge them even if it wanted to. They are not in the view
hierarchy (a windowed app's tree is full-bleed, checked by walking it), no API
vends their frame, and the insets are silent. The only lever is the style, and
that is now what `control_style_for` spends.

Left and right insets were `0` in every sample of every run, so the controls
never reserve horizontal space in any style.

§9 records what this changed in the code.

---

## 5. The packaging bug that hid underneath this

Worth recording because it silently invalidated every earlier iOS run.

Neither `Info.plist` in the tree declared `UIDeviceFamily`. Without it iOS reads
the app as iPhone-only and runs it on an iPad in **compatibility mode**: a
phone-sized canvas scaled up. It presents as "the text is too big" and is a
missing key.

It also quietly defeats the reason to own an iPad. `DEPLOYING.md` says the wide
width class is the thing a phone cannot exercise — and a compatibility-mode app
is handed a phone's width on any screen, so that class was never reached. Every
iOS run before this fix, device and simulator alike, was a phone-width run.

The tell that it *looked* fine: the gallery's plist carried
`UISupportedInterfaceOrientations~ipad`, which describes how an iPad may rotate
and does not claim the iPad as a device family. The app was declaring iPad
orientations for a device it had never told iOS it supported.

`UIDeviceFamily` = `[1, 2]` fixes it. `1` is iPhone, kept so one bundle runs on
both.

---

## 6. The loop for fixing it

This does not need the physical iPad. The iOS 26 iPad simulators on this machine
are windowing-capable, and the whole cycle is local and screenshotted:

```
xcrun simctl list devices available | grep -A3 "iOS 26"     # pick an iPad, iOS 26+
cpc build --target ios-arm64-simulator

# link, bundle, install, launch — see DEPLOYING.md §0, which now also names the
# prebuilt dependency slices that every iOS link needs
xcrun simctl bootstatus <UDID> -b && open -a Simulator
xcrun simctl install <UDID> <App>.app
xcrun simctl launch --console-pty <UDID> <bundle-id>

xcrun simctl io <UDID> screenshot shot.png                  # and look at it
```

`bootstatus -b` rather than `boot`: it is idempotent, and it *waits*, so the
install cannot race a device that has not finished booting.

**Look at the screenshot each time.** Every finding in this file came from
reading one, and two wrong conclusions were caught the same way — "the app does
not relayout" and "maximise is disabled" were both refuted by the next picture.

---

## 7. The order to do it in

1. **Probe first.** Answer §4 before writing layout code.
2. **Adopt a scene.** A `UIApplicationSceneManifest` with a delegate class name
   and no storyboard, plus a synthesized `UIWindowSceneDelegate`.
   `synth::allocate_class_pair` already synthesizes the app delegate and the nav
   delegate, so a third is the same move.
3. **Move window construction** from `didFinishLaunching` to
   `scene:willConnectToSession:options:`. This is the risky step: get it wrong
   and the app launches to a black screen, the same failure a storyboard causes.
4. **Answer the style question**, and let facet express it rather than hardcoding
   one — `unified` and `minimal` suit different apps.
5. **Wire the rest through `Chrome`**, which already has the fields (§3).

Steps 2 and 3 are the whole risk. Everything after them is filling in a mapping
that already exists.

---

## 8. What happened when step 2 was done

Adopted, and it runs. `FacetUIKitSceneDelegate` is synthesized beside the app
delegate, the plist names it, and the window is **adopted** rather than rebuilt:
the app delegate still constructs it exactly as before and
`scene:willConnectToSession:options:` calls `setWindowScene:` on it. An app whose
plist has no manifest never connects a scene and is untouched, which is what
made the step safe to try.

**One real bug in the objc binding surfaced.** The first attempt launched and
died immediately:

    'representation's delegateClass must conform to UISceneDelegate protocol'
    -[UIScene initWithSession:connectionOptions:]

Every method was present. UIKit checks the **protocol list**, not the method
list, and `vendor/objc` had no binding for `class_addProtocol` at all — synthesis
could build a class and give it methods but could not make it claim anything.
`synth::conform` was added for this and is generally useful: any framework that
tests `conformsToProtocol:` rather than `respondsToSelector:` needed it and
could not have been served.

**And the window filled its screen.** Before adoption the catalog drew across
roughly 57% of an iPad simulator and left the rest black; after, it reaches the
right edge. That had been written off as the catalog screen's own layout failing
to grow — wrong. The window was sized from `Screen.main_screen().bounds()` at
launch, before it belonged to a scene, and never re-sized to the geometry it
actually got. Adopting the scene fixed a layout symptom that looked nothing like
a windowing bug.

STILL UNVERIFIED: `unifiedStyle` itself. The screenshots above are full screen,
where the controls live in the menu bar and cannot collide with anything. The
overlap this file exists for only happens in the windowed state, so
`preferredWindowingControlStyleForScene:` is wired, compiled and reachable but
has not been seen to change a pixel. That is the next thing to look at, and §4's
`safeAreaInsets` probe belongs in the same sitting.

---

## 9. Steps 4 and 5, and the numbers that came back

Written 2026-08-18, against the iOS 26.1 iPad Pro 11-inch and the iOS 26.4
iPhone 17 Pro simulators. Every value below was printed by a running app.

### The rule that replaced the idiom check

The first version of `apply_size_restrictions` said it needed "no idiom check",
and that instinct is now the package's stated rule, in `device.cplus`:

> **Capability gates ask the object. Layout decisions ask the idiom. Neither
> stands in for the other.**

UIKit answers every capability the same way — the property is nil where the
platform does not support the feature — and that answer cannot be wrong about a
device this code has never run on. It is also the only kind of check that
survives what the iPad just did: gaining real windowing in iPadOS 26 **without
its idiom changing by one bit**. There is deliberately no `if is_pad()` in the
windowing path, and adding one would be a regression.

The idiom is still read, because it is a real and different question — `pad` vs
`phone` is what an app asks to choose a sidebar over a tab bar. It lives in
`device::idiom()`, cached, and it is what the trap in §5 corrupts.

### What the devices actually answered

One line per reading, from `window::windowing_report()`:

| | iPad (iPadOS 26.1) | iPhone (iOS 26.4) |
|---|---|---|
| `idiom` | `pad` | `phone` |
| `sizeRestrictions` | **non-nil** | **nil** |
| `windowingBehaviors` | **nil** | **nil** |
| `UISceneWindowingControlStyle` | present | present |
| `UIScene.title` after `setTitle:` | set | set |
| safe insets, full screen | `32,0,20,0` | `62,0,34,0` |

Four things follow, and three of them contradict something written above.

**`sizeRestrictions` non-nil on iPad and nil on iPhone is the whole design
working.** The nil check discriminates correctly with no device test anywhere
near it. This was the load-bearing assumption and it is now a measurement.

**`windowingBehaviors` is nil on BOTH.** §3's table said `minimizable` maps to
`miniaturizable`; it does not, on any iOS device. The header describes those
buttons as living on "the NSWindow associated with this scene" — Mac Catalyst's
window. `minimizable` and `maximizable` are now recorded in `MANIFEST.md` as
cannots. The code was already correct because it null-checks; only the
documentation was wrong, and it was wrong because it was read off a header's
names instead of run.

**The style is chosen by facet, not by this file** — and the first mapping was
wrong, which is worth recording because of HOW it was wrong.

It was derived by matching the header sentences: `Bar::Native` sounds like
`automaticStyle`, and `Bar::Blended`'s contract line ("the bar shares the
content's surface") is nearly word for word `unifiedStyle`'s ("windowing
controls will appear as part of the scene's content"). Both readings are
defensible and both were wrong, because the words describe intent and the
question was about pixels. Photographed windowed on the iPad, `unified` put the
system's close/minimise/zoom pill **on top of the app's own title**.

The corrected mapping, and the rule behind it:

> **`unified` is only correct for an app that draws a top bar and leaves room in
> it for the controls.** Everything else gets `minimal`.

| `Bar` | style | why |
|---|---|---|
| `Native` | `minimal` | facet HIDES the navigation bar at the root screen, so "stock bar" means no visible bar here |
| `Hidden` | `minimal` | no bar, said outright |
| `Blended` | `unified` | the app has a bar; the controls join it — the Contacts arrangement |
| `Custom` | `unified` | same, more so |

**Nothing maps to `automatic`, deliberately.** `automatic` is precisely what
UIKit uses when the delegate does not implement the method, so answering with it
is an elaborate way to say nothing — and it measured pixel-identical to
`unified` anyway. Implementing the method earns its place only by saying
something better than the default.

An app on the `unified` side must keep the first **~60 x 40pt** of its bar
clear: the pill was measured at x 19.5–60pt, y 20.5–40pt from the window's
top-left corner. **`window_buttons` now reserves exactly that**, which is what
took it out of `decided_absent` — see `MANIFEST.md` §7. The point of answering
the kind rather than telling applications to branch is that the toolbar source
is then the same on both backends:

```cplus
b.add(ui::window_buttons(key: "app:window-buttons").height(BAR_H));
```

No width and no `#platform()`. One binary, the node's measured frame and where
the next item in the bar starts:

| | window vs screen | the node measured | next item at |
|---|---|---|---|
| iPad, in a window, `unified` | `541x648` of `1210x834` | `60 x 44` | x = 68 |
| iPad, **full screen** | equal | `0 x 44` | x = 8 |
| iPhone, no windowing | equal | `0 x 44` | x = 8 |

All three measured on simulators, and the windowed row photographed: the
system's pill at the window's top-left and the toolbar's own first item clear to
the right of it.

**Getting a SIMULATOR into the windowed state is the hard part**, and worth
writing down. Nothing scripted can do it — no `simctl` verb, no self-resize,
`devicectl appResize` is Mac/Vision-only, and driving Simulator.app needs macOS
accessibility permission. A person has to drag it once. But the windowed state
belongs to the **scene session**, which is keyed by BUNDLE ID and survives
reinstalls — so once any app has been windowed by hand, another build carrying
that same `CFBundleIdentifier` inherits the window. That is how the row above
was measured: the probe was temporarily given the gallery's bundle id.

The full-screen row is a correction. The reservation first keyed on "the
platform can window", which drew 60pt of empty toolbar on a full-screen iPad
with nothing behind it — the controls are in the menu bar there. It keys on
being in a window now, and `windowScene:didUpdateEffectiveGeometry:` re-marks
the trees when that changes, so the gap arrives with the controls.

**One thing had to be added to make it real.** `run_screen` calls `build()`
before `open_window`, and the tree is mounted and laid out inside
`didFinishLaunching` — all before any scene exists. A `window_buttons` node
measured then answers 0, correctly, because at that instant the app genuinely
has no window controls. Nothing else in the pipeline knows the scene arriving
changes that: flex caches a measurement whose inputs have not changed, and from
flex's side nothing has. So `scene:willConnectToSession:` re-marks the mounted
trees for layout. Without it the reservation is computed correctly and never
reaches a frame.

**In FULL SCREEN the insets are identical under all three styles** (`32,0,20,0`),
which is the expected non-answer: the controls are in the menu bar there and
cannot collide with anything. Every style difference above is windowed-only,
which is why none of it showed up until the window came off full screen.

### The trap in §5 was still live in the test runner

`tools/run_ios_tests.sh` writes its own `Info.plist` and it had no
`UIDeviceFamily`. So the package's own suite had been running in compatibility
mode on every iPad it was ever pointed at — a phone-width run reporting itself
as a pass. Fixed in the same change as the checks that would otherwise have
been meaningless: `the idiom agrees with the trait collection` passes on an
iPad only because the runner is now allowed to be one.

The suite prints `running as: pad` or `running as: phone` before the device
checks, so a run that was secretly a phone run says so on its own.

### Reproducing it

    vendor/facet_uikit/tools/run_ios_tests.sh <ipad-udid>    # 40 checks
    vendor/facet_uikit/tools/run_ios_tests.sh <iphone-udid>  # same 40

and the live readings, from a probe under `playground/windowprobe`:

    playground/windowprobe/run.sh <udid>

whose `bar_under_test()` is one line to flip per style. **On a real iPad, drag
the window's resize grabber while that probe is running** — it reports every two
seconds, and `win=` and `safe=` moving is the §4 answer nothing on this machine
could produce.
