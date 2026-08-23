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

- **Every control except `label`, `button`, `box` and the plain container.**
  `text_field`, `image`, `scroll`, `checkbox`, `radio`, `toggle`, `slider`,
  `progress`, `spinner`, `stepper`, `popup`, `tabs`, `list`, `collection`,
  `table`, `tree`, `canvas`, `web`, the pickers, the menu tier. `views.cplus`
  dispatches on kind; each needs a `create_` / `apply_` pair in
  `controls.cplus`.
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
- **Prop parity is unmeasured.** `tools/parity.py` counts props AND handlers for
  the other two backends; this one has no numbers yet. The uikit lesson stands —
  a prop-only count is misleading, because a control can honour every prop bit
  and still call nothing on tap.

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
