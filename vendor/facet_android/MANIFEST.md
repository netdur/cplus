# facet_android MANIFEST

What Android **cannot** do, and — kept strictly apart from it — what this pass
**did not build yet**. Blurring the two is the failure this file exists to
prevent: a reader who cannot tell them apart cannot tell a finished backend
from an abandoned one.

Status 2026-08-23: **first light.** A label, a button and a container render on
a Pixel 9 Pro XL emulator (API 36); taps route through facet's handler; state
survives an Activity recreation. Everything else is section 2.

---

## 1. Decided absent — Android has no such thing

### `accessibilityIdentifier`

The other two backends put a node's KEY on the platform's a11y id — one token,
three jobs (facet's address, the agent surface's id, the platform's test id).
Android has no such property. `setContentDescription` is the near neighbour and
is **the wrong slot**: it is what a screen reader reads aloud, and an address is
not a description. So the key rides `setTag`, which is the identity slot.

Consequence, stated plainly: a key is **not** visible to `uiautomator dump`,
where the other two backends' keys are visible to their platforms' inspectors.
An agent surface on Android will have to read the tag through the tree rather
than through the accessibility service.

### The layout containers

`LinearLayout`, `RelativeLayout`, `ConstraintLayout`, `GridLayout` and their
`LayoutParams` are not bound and never will be. facet owns geometry: flex
computes every frame and this backend pushes it with `View.layout()` onto a
`FacetHost` whose `onLayout` is empty. Binding Android's layout system would be
binding a second, contradictory answer to the same question.

This is why `vendor/android_view`'s generated class list is seven leaf widgets
rather than all of `android.widget`.

---

## 2. Not yet built — Android has an answer, this pass did not write it

Everything here is a debt, not a decision. Kinds with no body **warn once**
through liblog (`adb logcat -s facet`) and render an empty container.

- **The controls still without a body.** `popup`, `tabs`, `list`, `collection`,
  `table`, `tree`, `canvas`, `web`, `span`, `bordered`, `page_dots`, `carousel`,
  `refreshable`, `swipeable`, `split`, the pickers, the menu tier. `views.cplus`
  dispatches on kind; each needs a `create_` / `apply_` pair in `controls.cplus`.

  Built: `label`, `button`, `box` and the plain container, plus `checkbox`,
  `toggle`, `radio`, `slider`, `progress`, `spinner`, `text_field`,
  `text_button`, `text_area`, `search_field`, `image`, `icon_button`, `scroll`
  and `stepper` — each with BOTH halves, the props write and the event read.
  Half a control is worse than none: it looks finished and reports nothing,
  which is the shape of the bug facet_uikit carried in its checkbox until
  2026-08-25.

- **An image can only come from the FILESYSTEM.** `BitmapFactory.decodeFile`
  reads a path; an image packed into the APK lives in `assets/` and needs an
  AssetManager. A source that does not decode leaves the view empty and warns
  once — a placeholder would read as a layout bug.

- **`ScrollAxis::Both` scrolls vertically only.** ScrollView and
  HorizontalScrollView are separate widgets and neither becomes the other;
  nesting one in the other is the usual trick and it fights the gesture
  arbiter. Vertical is the phone-shaped default.
- **The gesture band.** `gestures::install_key_reader` and
  `component::install_sender_readers` are not filled, so only a button's
  `on_click` fires. `wants_view` already answers true for a node with gestures,
  so the tree shape is right and only the arming is missing.
- **`observe_size`.** Android's answer is `addOnLayoutChangeListener`, which
  needs a fifth DEX adapter. Returns 0 (no handle) today.
- **The recycler.** `RecyclerView.Adapter` is an abstract class, so it cannot be
  implemented from native code — the DEX must carry a `CplusAdapter` calling
  back into a bind hook, the same trick as `FacetClick` but much larger. See
  plan.android.md rung 5.
- **`theme::set_theme_changed_fn`.** `is_dark` is filled; the repaint-on-flip
  hook is not.
- **Prop parity, measured.** `vendor/facet_gtk/tools/parity.py` scores every
  backend: android reads **47/360 props (13%) and 11/68 handlers (16%)**, against
  gtk 98%/100%, appkit 92%/100%, uikit 89%/95%. Both numbers are quoted because
  either alone misleads — a control can honour every prop bit and still call
  nothing on tap, which is exactly what the tool was taught to catch.

---

## 3. Platform obligations the other two backends do not have

Not gaps — facts about Android that shape the code, recorded because each one
cost a debugging session.

### A view must have LayoutParams before anything is written to it

facet's mount walk creates a view FULLY CONFIGURED and inserts it afterwards
(mount.cplus M3). `TextView.setText` calls `checkForRelayout()`, which
dereferences `getLayoutParams()` — null until a parent supplies one. The first
`set_text` on a fresh label therefore threw NPE. `controls::attach_layout_params`
runs at create time; the values are never read, because this backend measures
and lays out explicitly.

### An Activity can be destroyed and recreated under a live tree

Rotate, change locale, resize a split screen: Android tears down the Activity
and every View **in the same process** and calls the entry point again. C+
statics survive; every jobject does not. `window::attach_root` unmounts, marks
the whole tree dirty (`core::touch_all` per node) and re-mounts — which is
facet's `create` == `apply`-with-all-bits rule, and here it is the only thing
that makes recreation survivable. Verified: a tap count survives a rotation.

### A stretched compound button draws at the far end

A `Switch` in a column looks right-aligned and a `CheckBox` looks left-aligned.
Neither is a layout bug: flex stretches a column's children across the cross
axis, so both views are the FULL row width — and Android draws a CheckBox's box
at the leading edge and a Switch's track at the trailing one, with the (absent)
label filling the gap. An app that wants either hugged gives it a width.

### An app writes no Java, and the dex has two ways in

`FacetActivity` ships in this package's dex. The system instantiates the launch
Activity before any C+ runs, so that one class cannot come from the in-memory
loader like the rest — it is merged into the app's own `classes.dex` at build
time (`d8` takes a `.dex` as an input, so the merge IS the build step that would
otherwise compile the app's Activity). The manifest names it and a `meta-data`
line says which `.so` to load, the way `NativeActivity` takes
`android.app.lib_name`.

So `dex::ensure_loaded` tries `FindClass` FIRST and only falls back to
`InMemoryDexClassLoader`. Loading the in-memory copy when the classes are
already merged would give the process TWO sets: the Activity would hold a
FacetHost of one class while this code registered natives on the other, and
`nativeSizeChanged` would never arrive. A missing class on that first attempt is
the ordinary case, not an error — the pending NoClassDefFoundError is cleared
rather than left for the next JNI call to trip over.

Both paths stay live and both are exercised: `examples/facet_gallery_android`
merges and has no `.java` at all; `playground/facet_android_demo` supplies its
own Activity and loads the dex at runtime.

What an app still writes is the `Java_cplus_facet_FacetActivity_nativeCreateView`
export, five lines calling `entry::start`. It lives in the APP because cpc emits
one object per package: a package that names a symbol obligates everything that
links it.

### A radio group has no widget

`RadioButton` is a `CompoundButton` like a checkbox: a tap TOGGLES it, and
nothing turns its siblings off. Exclusivity on Android belongs to `RadioGroup`
— which is a `LinearLayout`, one of the layout containers this backend never
binds because facet owns geometry. So the group is the backend's own business,
as it is on uikit and appkit, and `controls::radio_changed` is a port of
`facet_uikit::input::radio_pressed` with one root instead of a window list.

Two rules, and the second is the one a checkbox does not have: turning one ON
turns every other in its group OFF, and a radio cannot be turned off BY TAPPING
IT — Android will have toggled the control already, so the answer is to put it
back and tell no handler about a change that did not happen. A radio with an
empty group name stands alone, having nothing to be exclusive with.

### Two writers, one visibility

`paint::visibility_of` is the ONLY function that decides whether a view shows,
and it has to be, because the frame walk rewrites visibility on every layout
pass. Three rules feed it: flex's `Display::None` (which `mount::switch_to`
sets on every parked pane), a spinner that is not running, and the
application's own `is_visible`. Each was written in its own place first and
each was undone by the walk within a pass — a parked screen sat on top of the
one that replaced it, and a stopped spinner came straight back.

GONE, not INVISIBLE, for the first two: the node is out of layout and facet has
already given its space away.

### A FacetHost answers a loose measure with what facet told it

Android asks rather than tells in one place: a ScrollView measures its child
with an UNSPECIFIED height so the child may be taller than the viewport, and
`MeasureSpec.getSize` of UNSPECIFIED is ZERO. `FacetHost.setWanted` is how the
document answers; EXACTLY still wins when Android pins a size.

### Only the root host reports the window size

Every box and every scroll document is a FacetHost, and facet resizes them
itself. A host that reported its own size would hand it back as THE WINDOW'S:
a document sized to its content told facet the window was that tall, the
viewport grew to match, and the scroll had nothing left to scroll.

### stderr goes nowhere

An app's stdout and stderr are discarded unless someone sets
`log.redirect-stdio`. facet_uikit's "warns once on stderr" would be a warning
nobody sees, so diagnostics go through liblog (`log.cplus`).

### The app owns the link, and must name the whole dependency closure

cpc emits one archive per prebuilt package and compiles source-mode ones into
the app's archive. A missing archive fails at **dlopen, not at link**. Build
with `-Wl,--no-undefined` — it turns a one-symbol-per-launch hunt into a single
list at build time. `playground/facet_android_demo/build.sh` is the worked
example.
