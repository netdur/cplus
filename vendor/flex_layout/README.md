# flex_layout

A pure-C+ layout engine implementing the **CSS Flexbox** and **CSS Grid**
specifications. It is
**UI-kit-agnostic**: it computes frames from a flexbox style tree, and a platform
adapter (e.g. AppKit) applies those frames to real views. No FFI, `stdlib` only.

## Model — persistent, mutable nodes

Create a `Node`, set style imperatively, attach children (the parent owns them),
call `calculate_layout`, then read each node's computed frame back off the node.

```
import "flex_layout/flex_layout" as flex;

var root: flex::Node = flex::Node::new();
root.set_flex_direction(flex::FlexDirection::Row);
root.set_padding(flex::Edge::All, flex::StyleLength::points(8.0f64));

var sidebar: flex::Node = flex::Node::new();
sidebar.set_width(flex::StyleLength::points(200.0f64));
root.add_child(sidebar);

var content: flex::Node = flex::Node::new();
content.set_flex_grow(1.0f64);                       // fills remaining width
root.add_child(content);

root.calculate_layout(1024.0f64, 768.0f64, flex::Direction::LTR);

// absolute frames in the root's coordinate space
let f: flex::Layout = root.child_layout(1 as usize);   // Option[Layout]
// f.left / f.top / f.width / f.height
```

## Content sizing — measure callback

Content nodes (text, images) size themselves through a **measure function
pointer**, so the engine never depends on a UI kit. The adapter supplies one
that asks the real view for its fitting size:

```
fn measure_label(w: f64, wm: flex::MeasureMode, h: f64, hm: flex::MeasureMode) -> flex::Size {
    return flex::Size { width: 80.0f64, height: 24.0f64 };   // e.g. NSTextField fitting size
}
var label: flex::Node = flex::Node::new();
label.set_measure(measure_label);
```

## API surface

- **Enums**: `FlexDirection`, `Justify`, `Align`, `Wrap`, `Edge`,
  `PositionType`, `Overflow`, `Display`, `BoxSizing`, `Unit`, `MeasureMode`,
  `Direction`.
- **Values**: `StyleLength::points(v)` / `percent(v)` / `auto()` / `undef()`; `Size`.
- **Node**: `new()`, `add_child(take)`, `child_count()`, `set_measure(fn)`.
- **Style setters**: `set_flex_direction`, `set_justify_content`,
  `set_align_items`, `set_align_self`, `set_flex_grow/shrink/basis`,
  `set_width/height`, `set_min_/max_width/height`, `set_margin/padding/border/position(edge, len)`,
  `set_position_type`, `set_display`, `set_box_sizing`.
- **Read**: `layout_left/top/width/height()`, `layout_frame() -> Layout`,
  `child_layout(i) -> Option[Layout]`.

## Status

**Implemented (full CSS Flexbox + Grid)** — flex-direction (+reverse),
justify-content & align-items/self (all values, incl. **baseline** and
Start/End/Stretch), grow/shrink/basis + the **`flex` shorthand**, **iterative
min/max flex resolution** (freeze + redistribute), width/height/min/max
(point + percent), margin/padding/border with edge shorthands and **`margin: auto`**
(main-axis distribution + cross centering), **absolute / relative / static**
positioning, **measure** and **baseline** callbacks, `display:none`, **content-fit
sizing**, **flex-wrap** + **align-content**, **gap/gutters** (incl. **percent**),
**aspect-ratio**, **RTL/direction**, **box-sizing: content-box**, **pixel-grid
rounding** (`round_to_pixel`), a sound within-pass **measurement memo**, and
**CSS Grid** (`display: grid` — points/fr/auto tracks, auto-flow, implicit rows,
gaps). **108 tests, ASan-clean.**

```
// Flexbox
root.calculate_layout(1024.0f64, 768.0f64, flex::Direction::LTR);
root.round_to_pixel(2.0f64);           // optional: snap to a retina pixel grid

// CSS Grid
grid.set_display(flex::Display::Grid);
grid.add_grid_column(flex::GridTrack::points(200.0f64));
grid.add_grid_column(flex::GridTrack::fr(1.0f64));       // shares remaining width
grid.set_gap(flex::Gutter::All, 12.0f64);
```

Also: **RTL logical `Start`/`End` edges**, **CSS Grid spanning + dense flow +
explicit/named line placement**, and a **sound cross-call incremental cache**
(unchanged subtrees keep their frames across re-layouts; deep mutations through
content-fit ancestors correctly invalidate). **165 tests, ASan-clean.**

```
// Grid with a named line + a spanning item
grid.name_grid_column_line("main");
grid.add_grid_column(flex::GridTrack::fr(1.0f64));
header.set_grid_column_span(2);
sidebar.set_grid_column_start(grid.column_line("main"));
grid.set_grid_dense(true);
```

Also **subgrid** — `set_grid_columns_subgrid(true)` on a grid item makes its
children align to the parent grid's lines over its span.

**Full CSS Flexbox + Grid parity.** `overflow` is stored + readable
(`node.overflow()`) for the adapter to apply clipping; it does not alter frames.

## HIG defaults (`@flex`)

Apple's layout system is an **8-point grid**; the engine ships opt-in presets that
encode it, so `@flex` layouts are HIG-correct without magic numbers:

| Preset | HIG rule |
|---|---|
| `vstack { }` / `hstack { }` | related items **8pt** apart |
| `screen { }` | **20pt** window/screen edge margins (macOS) + 8pt spacing |
| `card { }` | **16pt** content padding + 8pt spacing |
| `.tappable()` | enforces the **44×44pt** minimum tap target (iOS/touch) |
| `.card_padding()` / `.screen_margins()` / `.std_gap()` | 16 / 20 / 8 pt |

Constants: `HIG_SPACE_XS/S/M/L/XL` = 4/8/16/20/32, `HIG_TAP_MIN` = 44. The engine's
own `Style::new` defaults are unchanged — HIG is a layer on top.

```
@flex {
    screen {                 // 20pt margins
        hstack {             // sidebar | content, 8pt apart
            box().width(200.0)
            card { box().grow(1.0) }   // 16pt padding
        }
    }
}
```
