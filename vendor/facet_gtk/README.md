# facet_gtk

facet's **GTK 4** backend — the Linux/BSD counterpart of `facet_appkit`.

> **Status: type-checked, not run.** `cpc check` reports **zero errors in this
> package's files** — every method, signature, and borrow is validated against
> the real `gtk4` binding. It links **libgtk-4**, so it has not been built or
> run here. Every runtime behavior — layout, measurement, widget ownership, the
> window host, the text widgets — must be validated on a Linux/GTK host.
> `facet_appkit` is the reference implementation.
>
> **Known blocker (not this package):** a fully-green whole-project `cpc check`
> is currently blocked by **22 pre-existing E0917 errors in the stale vendored
> `gobject_gir` binding** (middle-`__` marshal names like
> `cclosure_marshal_VOID__FLAGS`, predating the bindgen `__`-collapse fix).
> Those 22 also block `gtk4` itself (`cd vendor/gtk4 && cpc check` shows the
> same 22), so they are orthogonal to facet_gtk. **Regenerate `gobject_gir`**
> with the current `cpc-bindgen` (or hand-collapse the names) to unblock
> building anything on the typed GTK stack — on any platform, not just Linux.

## Same contract as facet_appkit

facet owns the description tree (`facet::Node`) and all layout (the shared
`flex_layout` engine). This package supplies:

| Piece | Role |
|---|---|
| Per-kind `Renderer` ops | Map each `facet::Node` kind → a typed `gtk4` widget |
| `set_identity` | `gtk_widget_set_name` + packed `(role, drive)` affordance (read by `agent_gtk`) |
| `mount` / `mount_into` | Description → flex tree → `GtkFixed` frames (`put` + `set_size_request`) |
| `render_into` | Component-path re-render: clear the host + `mount_into` (no global runtime) |
| `run` | GtkApplication window host (blocks in the main loop) |

Re-render is the **component model** — a handler mutates state, then the app or
the owning component re-renders explicitly (`render_into`). There is no
`run_app`/`refresh` global runtime (removed from facet everywhere, 2026-07-09).

## Kind coverage (Phase 1)

| Kind | Status |
|---|---|
| `label`, `wrap_label`, `button` | **real** (GtkLabel / GtkButton, click wired) |
| `set_identity` | **real** (widget name + affordance data key) |
| `text_area`, `composer` | **partial** — GtkTextView in a GtkScrolledWindow, displays `value`; **no** event wiring or text read-back yet (needs hand-bound `GtkTextBuffer` iterators — the generated binding omits them) |
| `bordered`, `clickable` | **pass-through** (content shown; no border / no gesture yet) |
| `split` | **flex row** of the two panes (no GtkPaned divider yet) |
| `context_menu`, `context_menu_item` | **not built** — `GtkPopoverMenu` + `GMenuModel` is the answer, and the kinds are not mentioned in this package at all |

Each gap is localized and greppable (`TODO(gtk,linux)`), the shape the plan
sanctions — a second pass grows each into a real op.

### The menu family, stated so it is a decision and not a discovery

`ui::context_menu` and `ui::context_menu_item` live in `facet/elements`, so they
read as portable. Only AppKit builds anything: this package does not mention the
kinds, and `facet_uikit` classifies them as a debt and warns once when one is
mounted. An app that declares a context menu therefore gets one on macOS and
silence here.

That is **not** a decided-absent, the way `refreshable` and `swipeable` are
decided-absent on iOS. GTK has a real answer — `GtkPopoverMenu` driven from a
`GMenuModel`, with `gtk_popover_menu_new_from_model` and a right-click
`GtkGestureClick` — so this is unbuilt work, not a thing the platform cannot
do. It is recorded here rather than built because this package is Phase 1,
type-checked and never run, and `facet_appkit` is the reference: writing a menu
tier that no one can execute would add a second unverified surface to a package
whose whole status line is that nothing in it has been executed.

The AppKit side of this family was finished on 2026-08-24 (a `context_menu` in a
tree or list row could never open, for two unrelated reasons). Anyone porting it
here should read `facet_appkit/src/controls.cplus` `attach_context_menu` and
`recycler.cplus` `menuForEvent:` together — the second is the half that is easy
to miss, because a table widget handles its own right-click and never asks the
row.

## What must be validated on Linux (before relying on this)

1. **Ownership** — `widget_leaf` does `g_object_ref_sink` on each widget and
   `g_object_unref` on drop, mirroring AppKit's retain/release. GTK's
   floating-ref rules are subtle; confirm no leak / no double-free under
   valgrind.
2. **Layout** — the `GtkFixed` `put` + `set_size_request` mapping of flex
   frames (top-left origin, no flip).
3. **Measurement** — `measure_widget` via `gtk_widget_measure`.
4. **The window host** — `run` uses a hand-bound `g_application_run` and the
   `activate` handler; GTK builds windows only after the app registers.

## Build & run (Linux / GTK host)

```
# Debian/Ubuntu
sudo apt install libgtk-4-dev

cd vendor/facet_gtk
cpc test          # links libgtk-4; structural tests + (add) runtime tests
```

On any host (no GTK needed), type-check the whole package against the binding:

```
cd vendor/facet_gtk
cpc check
```

## Reference

`vendor/facet_appkit` is the complete, tested backend — clone its op shapes.
The design is `plans/facet-multibackend-proposal.md` (Phase 1).
