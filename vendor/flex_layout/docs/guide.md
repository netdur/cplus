# Guide

## The node model

A layout tree is a tree of `Node`s. A node owns its children (the parent owns
them by value), carries a `Style`, and — after layout — a computed `Layout`
frame.

```
var n: flex::Node = flex::Node::new();   // default: a Column, align-items stretch
```

Yoga's defaults are used, so a fresh node is a **column** container with
`align-items: stretch`, `flex-shrink: 0`, and all sizes `auto`.

Build the tree with `add_child` (which consumes the child):

```
var parent: flex::Node = flex::Node::new();
parent.add_child(child_a);   // child_a is moved in
parent.add_child(child_b);
```

## Running layout

```
root.calculate_layout(available_width, available_height, flex::Direction::LTR);
```

- `available_width` / `available_height` are the space the root is given. Either
  may be `flex::undefined()` for an **unconstrained** axis — the root then sizes
  to its content.
- The root is placed at `(0, 0)`; every descendant's frame is filled in and is
  **absolute** in the root's coordinate space.

Read frames back:

```
root.layout_left()  root.layout_top()  root.layout_width()  root.layout_height()
root.layout_frame()                 // -> Layout { left, top, width, height }
root.child_layout(i)                // -> Option[Layout]  (a direct child's frame)
```

To read deeper than direct children, navigate the node tree with
`children.at_ptr(i)` (the children `Vec`), then `child_layout` on that node.

## Sizing a box

Each node's main/cross size comes from, in order of precedence:

1. an explicit `width` / `height` (point or percent),
2. flex `grow` / `shrink` distributing free space,
3. a `measure` callback (for content leaves — see below),
4. the node's **content** (a container with an `auto` dimension hugs its
   children).

```
n.set_width(flex::StyleLength::points(120.0f64));      // 120pt
n.set_height(flex::StyleLength::percent(50.0f64));     // 50% of the parent
n.set_width(flex::StyleLength::auto());                // size to content / stretch
```

`min` / `max` clamp the result:

```
n.set_min_width(flex::StyleLength::points(80.0f64));
n.set_max_width(flex::StyleLength::points(320.0f64));
```

## Flex distribution

`grow` shares extra space; `shrink` absorbs overflow; `basis` is the starting
size. Resolution is the full CSS algorithm — items that hit a min/max are frozen
and their space is redistributed.

```
a.set_flex_grow(1.0f64);   b.set_flex_grow(2.0f64);   // a gets 1/3, b gets 2/3
n.set_flex(1.0f64);        // shorthand == grow 1, shrink 1, basis 0
```

`justify-content` positions items along the main axis; `align-items` /
`align-self` along the cross axis:

```
row.set_justify_content(flex::Justify::SpaceBetween);
row.set_align_items(flex::Align::Center);
child.set_align_self(flex::Align::FlexEnd);            // overrides align-items
```

`auto` margins absorb free space (the centering / push idiom):

```
child.set_margin(flex::Edge::Left, flex::StyleLength::auto());   // push to the right
// both left+right auto -> centered on the main axis
```

## Spacing: padding, border, margin, gap

All accept a `StyleLength` and an `Edge` (`Left`/`Top`/`Right`/`Bottom`, the
shorthands `Horizontal`/`Vertical`/`All`, or the logical `Start`/`End`):

```
n.set_padding(flex::Edge::All, flex::StyleLength::points(8.0f64));
n.set_margin(flex::Edge::Top, flex::StyleLength::points(4.0f64));
n.set_border(flex::Edge::All, flex::StyleLength::points(1.0f64));
```

`gap` sits between children (and between wrapped lines). `set_gap` takes points;
`set_gap_length` takes any length (e.g. percent):

```
n.set_gap(flex::Gutter::All, 8.0f64);
n.set_gap(flex::Gutter::Column, 12.0f64);   // horizontal gap only
```

`box-sizing` defaults to `border-box` (width includes padding+border). Switch to
`content-box` so width is the content area:

```
n.set_box_sizing(flex::BoxSizing::ContentBox);
```

## Wrapping

```
row.set_flex_wrap(flex::Wrap::Wrap);          // items wrap onto multiple lines
row.set_align_content(flex::Align::Center);   // distributes the lines on the cross axis
```

Content sizing is wrap-aware: an auto-sized wrapping container sizes to all its
lines.

## Positioning

- `Relative` (the default) — the node flows normally, then is offset by its
  `position` insets (without affecting siblings).
- `Static` — flows normally and ignores insets.
- `Absolute` — removed from the flow, positioned against the padding box by its
  insets (and `left`+`right` / `top`+`bottom` pairs define its size).

```
overlay.set_position_type(flex::PositionType::Absolute);
overlay.set_position(flex::Edge::Left, flex::StyleLength::points(10.0f64));
overlay.set_position(flex::Edge::Right, flex::StyleLength::points(10.0f64));   // width = parent - 20
```

`display: none` collapses a node out of the flow entirely:

```
hidden.set_display(flex::Display::None);
```

## aspect-ratio

Ties one dimension to the other (`width / height`). The unset dimension is
derived; on the main axis it follows the flexed size:

```
thumb.set_aspect_ratio(16.0f64 / 9.0f64);
thumb.set_width(flex::StyleLength::points(320.0f64));   // -> height 180
```

## Content sizing — the measure callback

Content leaves (text, images) report their intrinsic size through a **measure
function pointer**, so the engine stays UI-kit-agnostic. The adapter supplies one
that asks the real view for its fitting size.

```
fn measure_label(w: f64, wm: flex::MeasureMode, h: f64, hm: flex::MeasureMode) -> flex::Size {
    // e.g. call NSTextField.fittingSize with the given constraints
    return flex::Size { width: 80.0f64, height: 24.0f64 };
}

var label: flex::Node = flex::Node::new();
label.set_measure(measure_label);
```

`MeasureMode` tells the callback how the constraint is meant: `Exactly` (fixed),
`AtMost` (shrink to fit within), `Undefined` (unconstrained).

For text baselines (align-items: baseline), supply a baseline callback returning
the ascent from the top:

```
fn baseline_first_line(width: f64, height: f64) -> f64 { return 15.0f64; }
label.set_baseline(baseline_first_line);
```

## RTL / direction

`set_direction` (or the `owner_dir` passed to `calculate_layout`) flips a row's
main axis and a column's cross axis; it inherits down the tree. Logical
`Start` / `End` edges resolve to physical sides per direction (a physical
`Left`/`Right` wins if also set).

```
root.set_direction(flex::Direction::RTL);          // row flows right-to-left
child.set_margin(flex::Edge::Start, flex::StyleLength::points(16.0f64));  // leading margin
```

## Pixel-grid rounding

After layout, snap frames to a device pixel grid so adjacent edges stay seamless
(no cumulative rounding gaps):

```
root.round_to_pixel(1.0f64);   // integer points
root.round_to_pixel(2.0f64);   // retina half-points
```

## Incremental relayout

Calling `calculate_layout` again is cheap when little changed: the engine
compares each node's style/grid/children against a snapshot from the last pass
and **skips re-laying-out unchanged subtrees** whose box is unchanged. Mutations
— including a deep change through a content-fit ancestor — invalidate correctly,
automatically. There is nothing to opt into; just call `calculate_layout` again.

## The AppKit adapter (sketch)

The engine only produces frames. An adapter walks the node tree alongside its own
view tree and, for each node, sets the view's frame from `layout_frame()` and
applies `overflow()` (e.g. `NSView.clipsToBounds`). Leaf measure callbacks are
backed by the view's `fittingSize`. See [dsl.md](dsl.md) for building the tree
declaratively.
