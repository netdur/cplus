# Guide

How the layout engine is meant to be used: node model, flex, grid, measure,
DSL/HIG, and adapter gotchas. Fast start: [tutorial.md](tutorial.md).
Signatures: [ref.md](ref.md).

## The node model

A layout tree is a tree of `Node`s. A node owns its children (the parent owns
them by value), carries a `Style`, and — after layout — a computed `Frame`.

```cplus
var n: flex::Node = flex::Node::new();   // default: Column, align-items stretch
```

A fresh node is a **column** container with `align-items: stretch`,
`flex-shrink: 0`, and all sizes `auto`.

```cplus
var parent: flex::Node = flex::Node::new();
parent.add_child(child_a);   // child_a is moved in
parent.add_child(child_b);
```

The tree is DOM-like: retained and mutable in place. Beyond `add_child` there
are `insert_child(node, at:)`, `remove_child(at:)` (returns the detached
subtree — drop it or reinsert it elsewhere to reparent), and
`move_child(from:, to:)` for reordering. Each child occupies its own heap
slot, so a `*Node` cursor from `child_ptr(at:)` stays valid until that node is
removed — no re-searching after appends, and reorders never move a node.
Structural verbs invalidate the incremental cache themselves; just run
`calculate_layout` again.

## Running layout

```cplus
root.calculate_layout(width: available_width, height: available_height);
```

- Every parameter has a default: an omitted axis is **unconstrained**
  (content-sized) layout, and the default direction resolves to LTR —
  `root.calculate_layout()` lays out fully content-sized.
- Root is at `(0, 0)`; every frame is **absolute in the root’s coordinate
  space**. For a superview-relative frame: `child.left - parent.left`,
  `child.top - parent.top`.

```cplus
root.frame()                        // -> Frame (left/top/width/height)
root.child_frame(i)                 // -> Option[Frame]
```

Deeper than direct children: walk down with `child_ptr` and call `child_frame`
on that node.

## Sizing a box

Precedence for main/cross size:

1. explicit `width` / `height` (point or percent),
2. flex `grow` / `shrink`,
3. `measure` callback (content leaves),
4. content-fit (container with `auto` hugs children).

```cplus
n.set_width(flex::StyleLength::points(120.0f64));
n.set_height(flex::StyleLength::percent(50.0f64));
n.set_min_width(flex::StyleLength::points(80.0f64));
n.set_max_width(flex::StyleLength::points(320.0f64));
```

Style reads back through the public `style` field (`n.style.width`,
`n.style.flex_grow`) — requested values, as distinct from the computed
`frame()`. Direct writes to `style` are as cache-safe as the setters.

## Flex distribution

```cplus
a.set_flex_grow(1.0f64);   b.set_flex_grow(2.0f64);   // 1/3 and 2/3
n.set_flex(1.0f64);        // grow 1, shrink 1, basis 0
```

Min/max freeze + redistribute (full CSS algorithm).
`justify-content` is main axis; `align-items` / `align-self` are cross.
`auto` margins absorb free space (push / center idioms).

```cplus
row.set_justify_content(flex::Justify::SpaceBetween);
row.set_align_items(flex::Align::Center);
child.set_margin(flex::Edge::Left, flex::StyleLength::auto());
```

## Spacing: padding, border, margin, gap

```cplus
n.set_padding(flex::Edge::All, flex::StyleLength::points(8.0f64));
n.set_gap(flex::Gutter::All, flex::StyleLength::points(8.0f64));
n.set_gap(flex::Gutter::Column, flex::StyleLength::percent(2.0f64));
n.set_box_sizing(flex::BoxSizing::ContentBox);   // default is border-box
```

Logical `Start` / `End` edges resolve with direction; physical `Left`/`Right`
win if also set.

## Wrapping

```cplus
row.set_flex_wrap(flex::Wrap::Wrap);
row.set_align_content(flex::Align::Center);
```

## Positioning

- **Relative** (default) — flow, then offset by position insets.
- **Static** — flow, ignore insets.
- **Absolute** — out of flow; insets against padding box.
- **`display: none`** — out of flow entirely.

```cplus
overlay.set_position_type(flex::PositionType::Absolute);
overlay.set_position(flex::Edge::Left, flex::StyleLength::points(10.0f64));
hidden.set_display(flex::Display::None);
```

## Aspect ratio

```cplus
thumb.set_aspect_ratio(16.0f64 / 9.0f64);
thumb.set_width(flex::StyleLength::points(320.0f64));   // height 180
```

## CSS Grid

Set `display: grid` and define tracks. Supports points / `fr` / `auto` tracks,
row-major auto-flow, implicit rows, spanning, dense packing, explicit + named
lines, subgrid, and gaps.

### Tracks

```cplus
var g: flex::Node = flex::Node::new();
g.set_display(flex::Display::Grid);
g.add_grid_column(flex::GridTrack::points(200.0f64));
g.add_grid_column(flex::GridTrack::fr(1.0f64));
g.add_grid_column(flex::GridTrack::auto());
g.set_gap(flex::Gutter::All, flex::StyleLength::points(12.0f64));
```

- **`points(v)`** — fixed.
- **`fr(v)`** — share of space after fixed + auto + gaps.
- **`auto()`** — max content of items in the track.

Rows via `add_grid_row`. Extra rows are implicit `auto`.

### Flow, span, dense

```cplus
g.set_grid_dense(true);           // backfill holes
header.set_grid_column_span(2);
photo.set_grid_row_span(2);
```

### Named lines

A track's `line:` argument names the grid line before it, matching CSS
`[sidebar] 200px [main] 1fr`:

```cplus
g.add_grid_column(flex::GridTrack::points(200.0f64), line: "sidebar");
g.add_grid_column(flex::GridTrack::fr(1.0f64), line: "main");
match g.column_line(named: "main") {
    option::Option[i32]::Some(l) => { content.set_grid_column_start(l); }
    option::Option[i32]::None => { }
}
```

Explicit starts: `set_grid_column_start` / `set_grid_row_start` (0-based; -1
resets to auto).

### Subgrid

```cplus
sub.set_display(flex::Display::Grid);
sub.set_grid_columns_subgrid(true);
sub.set_grid_column_span(3);
```

Children align to the parent’s lines over the span.

## Content sizing — measure callback

Content leaves report size through a measure `fn` + borrowed `ctx` (adapter
stores the view). Engine never owns or frees `ctx`.

```cplus
fn measure_label(ctx: *u8, w: f64, wm: flex::MeasureMode,
                 h: f64, hm: flex::MeasureMode) -> flex::Size {
    // cast ctx → view; return fitting size under (w, wm) / (h, hm)
    return flex::Size { width: 80.0f64, height: 24.0f64 };
}
label.set_context(view_ptr as *u8);
label.set_measure(measure_label);
```

`MeasureMode`: `Exactly` / `AtMost` / `Undefined`. Optional baseline callback
for `align-items: baseline`.

Owned attachment (retain/release a view with the node): `attach` / `detach` —
see [ref.md](ref.md).

## `@flex` DSL and HIG

Import as `flex` (alias is the DSL context name).

```cplus
var ui: flex::Node = @flex {
    row {
        box().width(200.0)
        box().grow(1.0).padding(8.0)
    }
};
```

- `@flex { }` builds a **column** of its items.
- Containers: bare `column { }` / `row { }` (and HIG presets below).
- Leaf: `box()` plus fluent modifiers.
- Nesting is same-context containers only (no nested different `@` blocks).

### Modifiers

Same-line fluent chain returns `Node` (consumes):

```cplus
box().width(200.0).height(44.0).grow(1.0)
column { box() }.gap(8.0).grow(1.0)
```

Own leading-dot line: mutating `set_*` (in-place). Putting a consuming fluent
on its own leading-dot line is a use-after-move.

| Modifier | Effect |
|---|---|
| `.width` / `.height` | points |
| `.width_percent` / `.height_percent` | percent of parent |
| `.grow` / `.shrink` | flex factors |
| `.padding` / `.margin` | uniform points |
| `.gap` | container gap |
| `.justify` / `.align` / `.wrap` | main/cross/wrap |

Flow control: `if` and `for` may add children; no `while` / `break` / `return`
inside the block.

### HIG presets (opt-in)

Engine `Style::new` defaults stay conservative. HIG is a layer on top (8pt
grid):

| Container / modifier | Rule |
|---|---|
| `vstack` / `hstack` | 8pt between items |
| `screen` | 20pt edge margins + 8pt spacing |
| `card` | 16pt padding + 8pt spacing |
| `.tappable()` | min 44×44 tap target |
| `.card_padding` / `.screen_margins` / `.std_gap` | 16 / 20 / 8 |

Constants: `HIG_SPACE_XS/S/M/L/XL` = 4/8/16/20/32, `HIG_TAP_MIN` = 44.

```cplus
@flex {
    screen {
        hstack {
            box().width(200.0)
            card { box().grow(1.0) }
        }
    }
}
```

## RTL / direction

```cplus
root.set_direction(flex::Direction::RTL);
child.set_margin(flex::Edge::Start, flex::StyleLength::points(16.0f64));
```

The `direction:` argument of `calculate_layout` also seeds inheritance.

## Pixel-grid rounding

```cplus
root.round_to_pixel();         // integer points (scale defaults to 1)
root.round_to_pixel(2.0f64);   // retina half-points
```

## Incremental relayout

Re-calling `calculate_layout` skips unchanged subtrees when style/grid/children
match the last pass. Deep mutations invalidate correctly — nothing to opt in.

## Responsive configuration

The flex/grid engine intentionally does not identify devices or own global
screen state. The optional `flex_layout/responsive` module converts viewport
numbers supplied by a host into application-defined layout classes:

```cplus
import "flex_layout/responsive" as responsive;

var screens: responsive::ResponsiveConfig =
    responsive::ResponsiveConfig::new("desktop");
screens.add_breakpoint("mobile", up_to: 300.0f64);
screens.add_breakpoint("tablet", up_to: 900.0f64);

let env: responsive::LayoutEnvironment =
    screens.resolve(viewport_width, viewport_height);

var root: flex::Node = if env.is("mobile") {
    compact_layout()
} else {
    regular_layout()
};
root.calculate_layout(width: env.width(), height: env.height());
```

Each breakpoint is an inclusive maximum width. The smallest matching maximum
wins, independent of registration order; the fallback applies above all
breakpoints. Names such as `mobile`, `compact`, or `sidebar` have no built-in
meaning.

Thresholds are expressed in the same logical unit as `viewport_width`: AppKit
points, CSS pixels, or another unit selected by the host. The module does not
perform DPI conversion or inspect a physical display.

On resize, resolve again. When `next.is_same_class(previous)` is true, the
existing tree can be passed straight to `calculate_layout`. When it is false,
reapply the
new class's styles or rebuild the tree before layout. This split keeps ordinary
fluid resizing cheap while leaving structural adaptation under application or
adapter control.

## Adapter sketch

Walk the node tree with your view tree; set frames from `frame()`; apply
`overflow()` (e.g. clip). Leaves measure via `fittingSize`-style callbacks.
Engine only produces numbers.

## Gotchas

- **Move semantics:** `add_child` and fluent DSL consume; mutate a `var` with
  `set_*`, not `n = n.width(...)`.
- **Cursor lifetime:** a `*Node` from `child_ptr` dies when that node (or an
  ancestor) is removed — never from sibling churn or reorders. To hold one
  across possible removals, capture `flex::removal_count()` with it and
  re-derive when the count changed (see ref.md).
- **Absolute frames:** always root-relative until the adapter subtracts.
- **Measure context:** borrowed; adapter owns the view lifetime.
- **Overflow** is stored for adapters; it does not change layout math.
