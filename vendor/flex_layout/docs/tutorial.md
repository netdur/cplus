# Tutorial

Quick path: build a tree, run layout, read frames. Deeper concepts in
[guide.md](guide.md); signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
flex_layout = "*"
```

```cplus
import "flex_layout/flex_layout" as flex;
import "stdlib/option" as option;
```

## Imperative tree

```cplus
var root: flex::Node = flex::Node::new();   // default column
root.set_flex_direction(flex::FlexDirection::Row);
root.set_padding(flex::Edge::All, flex::StyleLength::points(8.0f64));

var sidebar: flex::Node = flex::Node::new();
sidebar.set_width(flex::StyleLength::points(200.0f64));
root.add_child(sidebar);                    // consumes sidebar

var content: flex::Node = flex::Node::new();
content.set_flex_grow(1.0f64);
root.add_child(content);

root.calculate_layout(width: 1024.0f64, height: 768.0f64);

match root.child_frame(1 as usize) {
    option::Option[flex::Frame]::Some(f) => {
        // f.left / f.top / f.width / f.height — absolute in root space
    }
    option::Option[flex::Frame]::None => {}
}
```

Optional pixel snap (e.g. 2× retina):

```cplus
root.round_to_pixel(2.0f64);
```

## `@flex` DSL

```cplus
var root: flex::Node = @flex {
    row {
        box().width(200.0)
        box().grow(1.0)
    }
};
root.calculate_layout(width: 1024.0f64, height: 768.0f64);
```

HIG shell (margins + card padding):

```cplus
var ui: flex::Node = @flex {
    screen {
        hstack {
            box().width(200.0)
            card { box().grow(1.0) }
        }
    }
};
```

## Grid (sketch)

```cplus
var g: flex::Node = flex::Node::new();
g.set_display(flex::Display::Grid);
g.add_grid_column(flex::GridTrack::points(200.0f64));
g.add_grid_column(flex::GridTrack::fr(1.0f64));
g.set_gap(flex::Gutter::All, flex::StyleLength::points(12.0f64));
// add_child cells… then calculate_layout
```

## Content sizing

```cplus
fn measure_label(ctx: *u8, w: f64, wm: flex::MeasureMode,
                 h: f64, hm: flex::MeasureMode) -> flex::Size {
    return flex::Size { width: 80.0f64, height: 24.0f64 };
}
var label: flex::Node = flex::Node::new();
label.set_measure(measure_label);
```

## Day-one rules

- Frames are **absolute in the root’s coordinate space** — subtract parent
  origin for superview-relative placement.
- `add_child` / fluent DSL **move** the child; use `set_*` on a held `var`.
- Engine is UI-kit-agnostic: numbers only; you apply them to views.
- Unconstrained axis: omit the width/height argument (defaults are unconstrained).
