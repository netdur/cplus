# facet_gtk

facet's GTK 4 backend — the Linux/BSD counterpart of
[`facet_appkit`](../facet_appkit) and [`facet_uikit`](../facet_uikit).

facet owns the description tree and **all** layout (the shared `flex_layout`
engine). This package answers the contract's verbs with GTK and records in
[MANIFEST.md](MANIFEST.md) what it has not answered yet.

```
facet_gtk   the registration surface: install(), and nothing else
views       the Renderer's five verbs + intrinsic size
controls    one GTK body per node kind
paint       the shared band — background / radius / opacity / enabled / tooltip
geometry    computed frames -> GtkFixed placements
scheduler   schedule + the Scheduler service, over the GLib main loop
window      the GtkWindow host a root is laid into
```

## Status is a measurement

```
$ python3 tools/parity.py
  appkit    318 / 360    88%
  uikit     311 / 360    86%
  gtk        29 / 360     8%   <-- this package
```

**8%, and it is early.** Read [MANIFEST.md](MANIFEST.md) before trusting any
adjective. The biggest single gap is that **no handler is wired**: props are
writes and handlers are reads, and this backend currently has only the write
half — a button does not yet call `on_click`.

`tools/parity.py --check` fails if the number drops, so a refactor that
quietly loses a verb is caught by the gate rather than by someone noticing a
control stopped working.

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
