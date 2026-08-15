# facet_gallery

Living **widget reference** for [facet](../../vendor/facet) on AppKit.

Browse from a **tree** catalog (sidebar), interact in the **main pane**. Each
demo shows the control live and a short constructor snippet you can copy.

## Run

```bash
cargo build --release
cd examples/facet_gallery
../../target/release/cpc build
./target/debug/facet_gallery
```

## Catalog

| Group | Demos |
|---|---|
| *(root)* | Overview |
| Views | Button, Label, Controls, Values, Inputs, Pickers, Icons, Image, Spans, Graphics, Web |
| Layout | Basics, Scroll, Split, ZStack, Responsive (`src/demos/layout/`) |
| Collections | List, Collection, Tree, Page dots, Table, Carousel |
| Interaction | Refresh, Swipe |

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

Still thin: `hybrid_web`, app-menu / `toolbar_item` chrome (window-level rather
than pane demos). Carousel shows host + dots pairing; full item recycle fills
the same way as collection when you wire count/row.
