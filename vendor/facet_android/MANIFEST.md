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

This is why `vendor/android_view`'s generated class list is a curated list
rather than all of `android.widget`.

### AN SF SYMBOL NAME

`image("square.and.arrow.up")` and `icon_button("checkmark.circle")` name SF
Symbols, and Android ships no such glyph set. A source resolves through four
doors here — a FILE on disk, an APK ASSET, a DRAWABLE RESOURCE in the app, then
one in the `android` package — and an SF Symbol name is none of them, so it
warns once and draws nothing.

That fourth door IS the system tier `symbol.cplus` describes: "the platform's
own set. macOS reads SF Symbols, Linux the freedesktop theme, Windows Segoe
Fluent." Android's own set is `android.R.drawable`, reached by name, and asking
for it is what an app writing `icon_button("ic_menu_share")` gets. The names
differ per platform BY DESIGN — the contract calls naming one "a deliberate,
visible choice to write platform-specific code" — so a shared source picks the
name with `#platform()`, which is what examples/facet_gallery_ios does.

An `icon_button` in that state is an EMPTY BORDERLESS button, not a slab: the
kind's posture is glyph-only, so it takes `borderlessButtonStyle` the way
`text_button` does. The default `imageButtonStyle` puts an opaque background
under the glyph, and with no glyph to put there it WAS the control — which
reads as a broken button rather than an empty one.

Mapping the common names onto `android.R.drawable` was considered and rejected:
the legacy set is a different vocabulary drawn in a different decade, and
picking `ic_menu_share` for `square.and.arrow.up` would put a picture on the
screen the application never asked for. An Android app that wants an icon ships
one, and all three doors are open to it.

---

## 2. Not yet built — Android has an answer, this pass did not write it

Everything here is a debt, not a decision. Kinds with no body **warn once**
through liblog (`adb logcat -s facet`) and render an empty container.

- **The controls still without a body.** `popup`, `tabs`, `collection`,
  `table`, `web`, `bordered`, `carousel`, `refreshable`, `time_picker`, the menu
  tier. `views.cplus` dispatches on kind; each needs a `create_` / `apply_` pair
  in `controls.cplus`.

  Built: `label`, `button`, `box` and the plain container, plus `checkbox`,
  `toggle`, `radio`, `slider`, `progress`, `spinner`, `text_field`,
  `text_button`, `text_area`, `search_field`, `image`, `icon_button`, `scroll`,
  `stepper`, `list`, `tree`, `split`, `canvas`, `swipeable`, `page_dots`,
  `date_picker` and `symbol` — each with BOTH
  halves, the props write and the event read. Half a control is worse than none:
  it looks finished and reports nothing, which is the shape of the bug
  facet_uikit carried in its checkbox until 2026-08-25.

- **A `canvas` replays everything except FOUR blend modes and a wrapped span
  list.** The display list is read whole — every state command, the state stack,
  all four transforms, three clips, ten shapes, text, spans and images. What is
  absent is named rather than approximated: `Hue`, `Saturation`, `Color` and
  `Luminosity` are the non-separable blends, which arrived with `BlendMode` on
  API 29 and this backend's floor is 26; and `draw_spans` puts its runs on ONE
  baseline, because wrapping a run list means a SpannableString where a
  `draw_text_block` already goes through StaticLayout for the case that asks for
  a box. A font WEIGHT rounds to bold or not: `Typeface.create(String, int)` has
  four styles, and the variable-weight door is API 28.

- **A `swipeable` does not ANIMATE its reveal open under the finger** — it
  animates the LANDING, which is the part that has a duration. Everything else
  the kind names is here: the drag, the threshold, the two minimums of travel,
  the actions, the destructive colouring and all five events.

  The one structural difference from facet_uikit: there the strip sits UNDER the
  content at subview zero, and here the children's native indices are facet's —
  `insert` puts a child at ITS slot — so the strip is parked just past the
  trailing edge and content and strip translate TOGETHER. Nothing overlaps, so
  nothing depends on who draws first.

- **A `split` does not DRAG.** The kind is built and every verb it has works —
  the axis, the position, both minimums, the collapse and the drawn divider are
  all geometry, and geometry is flex's, so the division is written into facet's
  own layout exactly as facet_uikit writes it. What is absent is the grab: there
  is no touch gesture that means "take hold of this hairline", so `on_move`
  never fires and the position is the application's to set. The divider is one
  extra `View` this package owns, tagged onto the split's host and placed by the
  frame walk — facet's children keep their own indices.

- **A gradient `background` SNAPS to eight directions.** Both stops are drawn
  now — `FacetDraw.gradientBackground` builds the `int[]` and picks the
  Orientation, because `vendor/jni` types no array slots and C+ cannot name the
  ctor's enum parameter. What Android has is eight directions and nothing
  between them, so facet's angle rounds to the nearest 45 degrees. A gradient on
  a `canvas` does NOT round: a Shader takes two endpoints, so the angle is
  honoured exactly there.

- **A `date_picker` is a FIELD that opens a dialog**, and a `time_picker` is not
  built at all. Android's `DatePicker` widget is a full calendar whose mode is
  fixed by an XML attribute with no setter; UIDatePicker's compact posture — a
  chip that opens a picker, which is what an app asking for a 44-point picker
  means — is a Button and a `DatePickerDialog` here. Every verb the kind names
  lands: the date, the format (`SimpleDateFormat` reads the same LDML patterns),
  both bounds, `open`, and all three events.

  AND THE FONT WORKS, which is the one place this backend does something the
  iOS one cannot: a Button is a TextView, so the twelve font verbs facet_uikit
  records as unreachable all reach it.

- **A `symbol` has BOTH tiers, and one of them needs an asset.** The portable
  tier — `symbol(icons::home)`, a codepoint in facet's own
  MaterialSymbolsOutlined — is a TextView carrying that font, loaded through
  `Typeface.Builder` so the FILL axis can be asked for; the system tier is an
  ImageView against `android.R.drawable`. A node whose SET changes after mount
  keeps the view it was created with.

  The font is an APK ASSET, which is the standing it has in a macOS app's
  bundle: `build_android.sh` copies it out of `vendor/facet/assets` and
  `aapt2 -A` ships it. An app that ships no symbols can drop that line — the
  backend warns once and draws nothing rather than failing.

- **An image is resolved but never CACHED.** All three doors are open — a file,
  an APK asset through the AssetManager, a drawable resource by name — and each
  decode happens on the apply that asked for it. A list of thumbnails would
  decode the same bitmap once per row; a cache keyed by source is the answer and
  is not written.

- **`ScrollAxis::Both` scrolls vertically only.** ScrollView and
  HorizontalScrollView are separate widgets and neither becomes the other;
  nesting one in the other is the usual trick and it fights the gesture
  arbiter. Vertical is the phone-shaped default.
- **A pinch MAGNIFIES; nobody has felt it yet.** `Chrome.zoomable` is built —
  the tree is wrapped in one `FacetZoomHost`, which is a ScaleGestureDetector, a
  scale and a translation written onto the child, the same shape the iOS backend
  gets from a UIScrollView's zoom. Scale only, never a relayout, and touches
  belong to the content until there are two of them.

  What is NOT verified is the FEEL. `adb shell input` has no pinch, so nothing
  in this repo can drive a two-finger gesture — the arithmetic is pinned by
  reading, and the rest is a person's hands on a device.

- **A BOUNCE only bounces OUT.** The animate band is built —
  `ViewPropertyAnimator` drives both channels, and every easing maps to an
  interpolator: the sine and cubic curves to `PathInterpolator`, which takes the
  same four numbers a CSS `cubic-bezier` does, and the two springs to
  Anticipate and Overshoot, which are what a backswing and an overshoot are.
  What Android has no twin for is `BounceIn`: `BounceInterpolator` is an OUT
  bounce, and an in-bounce would have to be invented. It uses the out one and
  this line is why.

- **A TEXT TRANSFORM only goes UP.** `setAllCaps` is Android's and there is no
  lowercase twin — and rewriting the string instead would leave the control
  disagreeing with the props an application reads back. `Uppercase` works,
  `Lowercase` does nothing, and the other two are the identity.

- **A font WEIGHT rounds to bold or not.** `Typeface.create(String, int)` has
  four styles — plain, bold, italic, bold-italic — and the variable-weight door
  (`Typeface.create(Typeface, int weight, boolean italic)`) is API 28 against a
  floor of 26. So Semibold, Bold, Heavy and Black are all bold, and the six
  lighter weights are all plain. Same rounding in the canvas replay, for the
  same reason.

- **The gesture band.** `gestures::install_key_reader` and
  `component::install_sender_readers` are not filled, so only a button's
  `on_click` fires. `wants_view` already answers true for a node with gestures,
  so the tree shape is right and only the arming is missing.
- **`observe_size`.** Android's answer is `addOnLayoutChangeListener`, which
  needs a fifth DEX adapter. Returns 0 (no handle) today.
- **`table` and `collection`.** `tree` is BUILT, on the same adapter as `list`
  with a flattened visible-row index in place of a count. The recycler is — `list` runs on
  `ListView` + a `FacetRows` adapter in the dex — and these three ride the same
  machinery with a different model on top. `RecyclerView` was not used: it is
  AndroidX, an .aar with its own dex, and this project ships no Gradle;
  `ListView` recycles through `convertView`, which is the whole mechanism, and
  facet owns layout so RecyclerView's LayoutManagers would go unused.
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

### A COLOUR IS A TOKEN, not three numbers

`vocab::Color` carries a `token`, and only 255 means "the rgba fields are the
answer". Reading `r`/`g`/`b` off any other kind gives 0,0,0,0 — transparent
black — which is why every themed surface on this backend painted NOTHING while
literal colours looked perfect. Four kinds resolve first:

| token | is | resolved by |
|---|---|---|
| 100..117 | a theme ROLE | the application's palette, then facet's fallback |
| 200..217 | a derived INK | the contrast of the role it reads against |
| 254 | an adaptive PAIR | the side for this appearance |
| 1..24 | a PLATFORM colour | a table, light and dark |

Two reductions, both deliberate:

**A derived ink is the CONTRAST of its base, not the base.** AppKit reads the
effective appearance; Android has nothing equivalent, so the base's Rec. 601
luma decides — light base, dark ink. Resolving it to the base instead paints
text the colour of the thing behind it, which is a whole screen of invisible
words with every individual value correct.

**Platform tokens are a table, not a Resources read.** Android's system colours
are attributes on a theme, and reading them means a round trip per colour per
paint; the table holds what those attributes resolve to on a stock theme, in
both appearances. An UNKNOWN token paints the label colour rather than nothing:
visible and obviously wrong beats invisible and silent.

### A background, a radius and a border are ONE drawable

facet declares them as separate bits; Android expresses them as a single
`GradientDrawable` set as the view's background. `setBackgroundColor` can say
only the first of them, and calling it after building a drawable throws the
drawable away — so any of the four rebuilds the background whole, which is the
only version that cannot half-apply. A radius also turns on `clipToOutline`, or
a rounded background is a rounded rectangle with square content sitting on it.

### A shadow is an ELEVATION

Android's shadow is derived from a view's height above the surface, not authored
as an offset, a radius and a colour. facet's four fields collapse to one number
and the colour is the theme's. Recorded here rather than approximated with a
drawn shadow, which would be a different thing that looked similar.

### facet's units are density-independent; Android's are pixels

Every other backend gets this free — a point is a point on AppKit and UIKit, and
GTK scales the surface — so `geometry`'s `px` / `dp` are the only crossing here,
and everything above them is in facet's units.

IT HID FOR AS LONG AS EVERYTHING WAS MEASURED. A TextView's default text is in
`sp`, so a label asked for its natural size answers in already-scaled pixels and
the layout looks right; the probe that started this backend used no density
constant at all and was correct to. It breaks the moment an application STATES a
number. The iOS gallery's catalog asks for `row_height: 44` — 44 points, and 44
raw pixels on a 3x phone is a third of a row, so every row showed the top eighth
of its text.

`density` is cached in `env` (not `window`) because `px` runs for every frame in
every layout pass and the read is three JNI calls — and because
window -> scheduler -> geometry is already a chain.

### An undefined extent is not zero

`flex`'s `undefined()` is NaN, and `NaN as i32` is 0 — so `measure_node`
translated "no constraint on this axis" into `AT_MOST(0)` and every child
measured to nothing. It never showed while the root had a definite height,
because then every extent handed down is a real number. It appeared the moment
something was laid out UNBOUNDED, which is exactly what a recycled row is:
`calculate_layout` with a width and no height, so the row can be as tall as its
content.

Two hundred rows of nothing, and the measure looked like it was working because
the WIDTHS were right. `is_number` is the guard, and a NaN is the only value not
equal to itself.

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
