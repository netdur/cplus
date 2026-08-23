# facet_gtk — handoff

Written 2026-08-23 for whoever picks this up next, which is probably me.
`git log --oneline vendor/facet_gtk/` for what landed; this file is the part
that is not in the diff.

**Where it stands: 8%. That is a floor, not a backend.** The structure is
right, the number is real and gated, and a portable facet tree renders on
Linux. Everything else is ahead of you.

---

## 1. Read these first, in this order

1. `vendor/facet_appkit/MANIFEST.md` — **the doctrine**, and the only thing
   here that is not negotiable. For each verb facet declares, a backend either
   implements it or **states plainly that the platform cannot**. A verb that is
   neither is a gap, and the gap is a bug. AppKit's cannot-ledger is EMPTY at
   324/324.
2. `vendor/facet_uikit/MANIFEST.md` §"TWO NUMBERS" — props are WRITES,
   handlers are READS, and a backend at 100% on props can still be a control
   that is tapped and calls nothing. That is not hypothetical; it shipped.
3. `vendor/facet_appkit/src/facet_appkit.cplus` — this package's facade
   has the same job in 140 lines, "the whole registration surface … nothing else a backend could
   smuggle a hook through".
4. This package's `MANIFEST.md`, then `python3 tools/parity.py`.

The bar that matters most, from appkit's manifest: **a control does not have
to be one native widget.** Sixty-odd rows left its cannot-ledger once the
answer was allowed to be built from two classes instead of one. Do not write a
"GTK cannot" row until you have looked that hard.

## 2. The thing I did not do, and it is the next thing

**No handler is wired. At all.** `gestures::install_key_reader` and
`component::install_sender_readers` are unset in `install()`, deliberately —
facet's portable no-op is honest where a half-filled struct looks installed
and behaves randomly.

So: a button does not call `on_click`, a switch does not report a change, a
text field does not report an edit. Everything in this package is the write
half. **Start here.** It needs an `input.cplus` (the module both mature
backends have and this one does not) that connects GTK signals to facet's
handler dispatch. `vendor/gobject/src/signal.cplus` is the wiring — its
`connect` / `connect_bool` are tested and work; the extra-argument shapes are
generated per-package as `__sigc_*` beside the `connect_*` wrapper that uses
them (see the note in signal.cplus; do NOT add shapes to gobject).

Read `vendor/facet_appkit/src/input.cplus` for the shape. Note it also owns
`sender_readers` and the gesture band, and that `views::apply` must re-arm on
`C_GESTURES` — my `apply` does not, because there is nothing to arm yet.

## 3. The conformance target, which I did not reach

`examples/facet_gallery` — a Flutter-Gallery-style catalog of every widget.
Its manifest already has `[macos.dependencies] facet_appkit`, so it takes a
`[linux.dependencies] facet_gtk` the same way.

But it goes through **`facet_runtime`**, not the backend directly. And
`vendor/facet_runtime/src/runtime.cplus` — the neutral base — says this in its
own header:

> macOS resolves to runtime_macos.cplus over facet_appkit, **Linux to
> runtime_linux.cplus over facet_gtk**. A target with no override lands HERE,
> which means one thing: no facet backend exists for that platform yet.

**`runtime_linux.cplus` does not exist.** The architecture already has the
socket cut for this package and nothing is plugged into it. That file is the
integration seam, and writing it is what turns "a demo I wrote" into "the
gallery runs on Linux".

`runtime_macos.cplus` is ~78 fns and imports `facet_appkit/window`,
`facet_appkit/dialogs`. So the runtime facade expects a `dialogs` module this
package does not have. Its header also says: *"Porting facet to a new platform
starts by copying a real facade, not this file."*

Order I would take it: handlers → `runtime_linux.cplus` → gallery builds →
then let the gallery tell you which kinds matter, instead of guessing.

## 4. What is weird, and what is not

**Not weird, though it looks it.** In the demo the switch renders as a
520px-wide oval and the button spans the window. That is `align_items:
Stretch`, which is flex's default (`flex_layout.cplus:227`) — facet's own
layout doing exactly what it was told. AppKit stretches an `NSSwitch` the same
way. Do not "fix" it in the backend; a real tree constrains its children.

**Genuinely unresolved, and worth a look:**

- `insert` ignores `slot`. Children land in insertion order, which is right for
  a fresh mount and wrong for an insert into the middle of a live list. `GtkFixed`
  has no ordering; the fix is probably `gtk_widget_insert_after`.
- `relayout_all` re-lays every window on every sync, at the window's current
  size. That is a full layout per frame. AppKit prunes on `layout_changed()`
  and its `geometry.cplus` has a long comment about the two callers where that
  prune is WRONG — read it before copying the optimisation.
- `after()` holds ONE outstanding timer in a static pair. A second call before
  the first fires replaces it. Recorded in MANIFEST §2; fix when something
  needs two.
- `observe_size` returns 0 (facet reads it as "nothing registered"). Nothing
  in facet currently depends on it here, but a `screen` that reacts to size
  will.
- I never checked whether a GTK widget removed from its Fixed and re-inserted
  keeps its CSS provider. `paint::provider_for` hangs it on the widget with
  `g_object_set_data`, so it should — but it is untested.

## 5. Traps I hit, so you do not

- **flex sizes a LEAF from its measure callback.** Without one everything
  measures 0x0 and you get a zero-height strip: every control in the right
  place, invisible. `views::measure_view` is the fix; GTK's `gtk_widget_measure`
  fills min/natural for ONE orientation and takes the other as `for_size`,
  which is what makes a wrapped label answer the height its text needs.
- **A written-to control has a stale measurement.** `set_text` changes no
  style, so flex's incremental cache prunes the subtree and the label keeps the
  box it had for its old string. `apply` calls `mark_content_changed()` on any
  measured kind. Blunt on purpose — the alternative is a per-kind list of which
  props affect measurement, which is a second copy of the band that drifts.
- **A fresh GtkWidget carries a FLOATING reference.** `create` sinks it,
  `view_release` drops it. Without the sink an unparented widget leaks; without
  the release a removed one does. `vendor/gobject` has tests pinning exactly
  this behaviour — a plain GObject is not floating, a GInitiallyUnowned is.
- **`C_RESTYLE`, never `all_bits()`, for a theme flip.** `all_bits()` is also
  the CREATE SENTINEL — appkit has 76 sites testing `dirty == all_bits()` to
  mean "this is the create pass". Passing it on an appearance change makes
  every flip look like a create. That bug shipped on AppKit; the comment is
  copied into `facet_gtk.cplus` on purpose.
- **`cpc check FILE` inside a package DOES resolve imports.** Useful — you can
  check one module without building the package.
- **The GIR does not have everything.** `gtk_settings_get_default` is a real
  exported symbol that cpc-bindgen never emitted, because the GIR does not
  present it as a bindable namespace function. Hand-bind with `#[link_name]`,
  as `window.cplus` does; verify with `nm -D --defined-only`.

## 6. How to run it

```bash
cargo build --release                       # then ALWAYS ./target/release/cpc
cd vendor/facet_gtk && ../../target/release/cpc check
python3 tools/parity.py --check             # from the repo root
```

There is no demo in the repo — I built mine in a scratch dir. Ten minutes to
recreate: a package depending on `facet`, `facet_gtk` and the GTK closure, a
`facet::column` of a few kinds, `backend::open_window(...)`, then a
`glib::MainLoop`. **Put the next one in `examples/`** so it is not thrown away
again; `playground/` is gitignored and `examples/` is tracked.

To see what facet actually computed rather than squinting at a window, walk the
tree printing `(*n).frame()` and `facet::view_of(n)` per node. That is how the
zero-height-strip bug was found, and it is faster than any screenshot.

## 7. The one-line summary

The plumbing is done and proven end to end — contract, Renderer, measure,
layout, placement, appearance, and a window on screen. What is missing is
breadth (29 of 360 verbs) and the entire read half (0 handlers). Neither is
hard; both are long.
