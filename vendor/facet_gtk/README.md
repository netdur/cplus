# facet_gtk

facet's GTK 4 backend — the Linux/BSD counterpart of
[`facet_appkit`](../facet_appkit) and [`facet_uikit`](../facet_uikit).

facet owns the description tree and **all** layout (the shared `flex_layout`
engine). This package answers the contract's verbs with GTK and records in
[MANIFEST.md](MANIFEST.md) what it has not answered yet.

```
facet_gtk   the registration surface: install(), and nothing else
views       the Renderer's five verbs + intrinsic size
controls    one GTK body per node kind, and the CSS each one composes
input       the READ half — gestures, keys, control actions, sender readers
paint       the shared band — background / radius / opacity / enabled / tooltip
geometry    computed frames -> GtkFixed placements, and a scroll's extent
scheduler   schedule + the Scheduler service, over the GLib main loop
window      the GtkWindow host, the run loop, and the menus
chrome      the toolbar, the window buttons, and the drag region
swipe       swipe-to-reveal, assembled — GTK has no such row
anim        the two things that move on their own: a progress tween, a GIF
recycler    list, collection and tree over GtkListView — rows built as they scroll in
dialogs     alert / choose / prompt as facet trees, and the file chooser
drawing     the canvas replay, against cairo and Pango
web         a WebKitGTK view, opened at runtime rather than linked
```

## Status is a measurement

```
$ python3 tools/parity.py
  appkit    338 / 360    93%
  uikit     335 / 360    93%
  gtk       353 / 360    98%   <-- this package

  appkit     68 / 68    100%
  uikit      67 / 68     98%
  gtk        68 / 68    100%   <-- this package
```

**Two numbers, because props are WRITES and handlers are READS** — a backend at
100% on props can still be a control that is clicked and calls nothing. Both
are measured now, and both are gated: `--check` carries a floor per axis,
because the two fail differently. A missing prop is a control that ignores you;
a missing handler is a control that never answers.

The second number reaching 68/68 does not mean every handler is EXACT — two are
narrowed by facet's own contract rather than by GTK, and `MANIFEST.md` §3 names
both. What it means is that no declared handler is silently dead.

Read [MANIFEST.md](MANIFEST.md) before trusting any adjective — including that
one. `tools/parity.py --check` fails if the number drops, so a refactor that
quietly loses a verb is caught by the gate rather than by someone noticing a
control stopped working.

## Two ways to check it

```bash
cd vendor/facet_gtk && ../../target/release/cpc test    # 212 tests, no display needed
cd examples/facet_gtk_probe && ../../target/release/cpc run
cd examples/facet_gallery   && ../../target/release/cpc run

# ...and the gallery can walk ITSELF — every demo, one a tick:
FACET_GALLERY_WALK=1 ./target/debug/facet_gallery     # 35 screens, warnings visible
FACET_GTK_SCROLL=1   ./target/debug/facet_gallery     # every scroll move, and who made it
FACET_GTK_FRAMES=1   ./target/debug/facet_gallery     # the computed frame tree
```

The suite pins everything the backend DECIDES — the mappings, the CSS it
composes, the bits it claims, the encoders — and makes no widget, because a
suite that needs a login session is a suite that stops running. The probe is
the other half: whether the switch reported its change, whether the slider
fired while it was being dragged, whether the field kept its caret. An agent
has no hands, so that half is an app you run.

The gallery is the third: it is the same application macOS runs, unchanged,
and it reaches this backend through `facet_runtime/runtime_linux.cplus`. If it
builds and comes up, the whole stack — contract, backend, facade, app — is
wired end to end.

## Seeing what was computed

```bash
FACET_GTK_FRAMES=1 ./target/debug/facet_gallery 2>&1 | head -40
```

One line per node: its key, its kind, the frame flex computed, and whether it
has a widget. This is the package's one diagnostic and it exists because a
backend under an external layout engine fails in a way a screenshot cannot
explain — every control in the right place and none of them visible. `w=0` on a
kind that has a body is that bug; `view=-` on a backed kind is the other one.
Both have already happened here, and both were found in a second by this.

## It runs

That is new. The previous generation of this package could only ever be
type-checked — it targeted a superseded 34-entry per-kind `Renderer`, imported
`facet::Renderer` from a module that no longer exports it, and the GTK stack
did not link on Linux at all. All three are fixed; this one is built and run.

```cplus
import "facet_gtk/facet_gtk" as backend;

fn main() -> i32 {
    backend::install();
    var root: facet::Node = ui();
    let _ok: bool = backend::open_window(#addr_of(root), "facet · GTK 4", 520.0f64, 340.0f64);
    var lp: glib::MainLoop = glib::MainLoop::new(glib::main_context_default(), false);
    lp.run();
    return 0;
}
```

The tree `ui()` builds is portable: it is the same description `facet_appkit`
would mount on a Mac.

## The two rules worth knowing before editing

**One code path for create and apply.** `create` configures a new widget with
`paint::all_bits()`, which is the same call `apply` makes with the node's real
dirty word. A prop honoured on update is therefore honoured on create, and the
two cannot drift. Every write is gated on its bit.

**Ownership.** A fresh GtkWidget carries a *floating* reference. `create`
sinks it (facet holds the widget in the node) and `view_release` drops it.
Without the sink an unparented widget leaks; without the release, a removed one
does.
