# facet_gallery

Living **widget reference** for [facet](../../vendor/facet), on AppKit and on
GTK 4. The source names no platform: `facet_runtime` picks the backend by
target, so this is the same application both times and the DIFFERENCE between
the two windows is a measurement of the younger backend.

Browse from a **tree** catalog (sidebar), interact in the **main pane**. Each
demo shows the control live and a short constructor snippet you can copy.

## Run

```bash
cargo build --release
cd examples/facet_gallery
ln -sfn ../../vendor vendor      # resolve deps from the repo, not the registry
../../target/release/cpc build
./target/debug/facet_gallery
```

For a live GTK regression pass after building, run from the repository root:

```bash
python3 tools/test_facet_gtk_gallery.py
```

This requires a display (or `xvfb-run`). It visits all 39 demo panes twice,
then checks relist row handlers, columns and scrolling, table row heights,
carousel page geometry, HTML/literal label transitions, the time picker's
opened callback, Unicode input, caret placement and native hit testing through
the app's agent socket. GTK critical warnings abort the check, so navigating away from
a control with a pending animation also tests its teardown.

On X11, add `--native-input` to test right-click context menus, scoped Ctrl+I
shortcuts, command rebuilds and carousel resizing. This needs `xwininfo`,
`xprop` and `libXtst`.

When launching from a Snap-packaged editor, its GTK/GIO environment can make
WebKit's system helper load incompatible Snap libraries. Use the system GTK
environment for this app:

```bash
env -u GTK_PATH -u GIO_MODULE_DIR -u GDK_PIXBUF_MODULE_FILE ./target/debug/facet_gallery
```

The regression script clears these variables automatically.

On Linux the backend is [`facet_gtk`](../../vendor/facet_gtk), which is younger
than the AppKit one: its `MANIFEST.md` says what is answered and what is not,
and a demo whose control looks like a plain box is a kind that has no body yet
rather than a bug in the demo.

## Catalog

| Group | Demos |
|---|---|
| *(root)* | Overview |
| Views | Button, Label, Controls, Values, Inputs, Pickers, Icons, Image, Spans, Graphics, Web |
| Layout | Basics, Scroll, Split, ZStack, Responsive, Grid, Placement (`src/demos/layout/`) |
| Collections | List, Collection, Tree, Page dots, Table, Carousel |
| Decoration | Shadow, Brush, Clip & order (`src/demos/decoration/`) |
| Animation | Basics, Easing, Entrance, Rules (`src/demos/animation/`) |
| Interaction | Refresh, Swipe |
| *(root)* | Accessibility |

## Implementation pattern

```cplus
import "facet/elements" as ui;
import "facet/label" as label;   // for find + setters

// build once
ui::button("Save", key: "save", on_click: this.on_save)

// update in place — no re-render
match label::find("count") {
    option::Option[label::Label]::Some(l) => { l.set_text("…"); }
    option::Option[label::Label]::None => { }
}
```

## Animation

`src/demos/animation/` is both a showcase and the reference for the two rules
that are not visible in the `animate_*` signatures:

- **the transform animates as a unit** — scale, rotation and translation are one
  matrix, so two verbs in a tick compose into one animation and a `set_*` on any
  of them cancels a pending one;
- **the start value needs its own apply** — `set_opacity(0)` and
  `animate_opacity(1)` in one handler is not a fade-in. Build in the hidden
  state (the mount walk applies it) and animate from `on_attach`, or hop through
  a timer (`services::after`) for a replay — a main-queue hop is drained in the
  same run-loop turn and does not buy you a later apply.

Both are demonstrated live, next to the spelling that works, in **Rules**.
**Easing** races all eleven presets over the same distance and names what each
one actually plays — AppKit has four timing functions, so several rows are the
same motion.

Still thin: `hybrid_web`, app-menu / `toolbar_item` chrome (window-level rather
than pane demos). Carousel shows host + dots pairing; full item recycle fills
the same way as collection when you wire count/row.
