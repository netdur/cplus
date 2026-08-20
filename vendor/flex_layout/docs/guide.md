# Guide

How the layout engine is meant to be used: node model, flex, grid, bands,
measure, DSL/HIG, and adapter gotchas. Fast start: [tutorial.md](tutorial.md).
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

## Conditional visibility — bands

A node states when it should be present; the layout pass enforces it. There is
no size observer to arm, no callback to keep alive, and nothing to re-run on
resize.

```cplus
import "flex_layout/bands" as bands;

var set: bands::BandSet = bands::BandSet::defaults();

var sidebar: flex::Node = flex::Node::new().hide("compact");
// or, after the fact: sidebar.add_rule("compact", hide: true);

root.calculate_layout(width: w, height: h, bands: #addr_of(set));
```

### What a band is

A named box constraint. Each edge is optional and an omitted one is
**unbounded**, not zero:

```cplus
set.set("watch", max_width: 120.0f64, max_height: 120.0f64);
set.set("wide", min_width: 1400.0f64);
```

Bounds are half-open (`min <= v < max`), which is what lets a ladder tile a
range exactly with no gap and no double-match. Re-using a name updates it
rather than adding a second entry, so reloading a configuration is idempotent.

Six bands ship pre-registered, Material 3's window size classes with Compact
split at 300 so a watch face is not lumped in with a 599pt phone:

| Band | Width |
|---|---|
| `tiny` | < 300 |
| `compact` | 300–599 |
| `medium` | 600–839 |
| `expanded` | 840–1199 |
| `large` | 1200–1599 |
| `xlarge` | ≥ 1600 |

They are width-only on purpose: a one-axis ladder tiles, so every box lands in
exactly one. Adding a height bound to a default would open a gap — a 200×800
box would be too tall for a height-bounded `tiny` and too narrow for
`compact`, and match nothing. Height belongs in bands you define yourself,
which is what `watch` above is.

Names are `str`, so **the compiler cannot catch a typo**: `hide("compat")`
simply never fires. `is_registered` is there for a startup check.

### Which box a rule is measured against

The node's nearest **contained** ancestor — the closest box up the tree whose
size does not depend on its own contents. Never the window: an app in Split
View, on half a foldable, or in a resized window has been handed a box, and
the screen's size answers a question nobody asked.

**A node never queries itself.** A sidebar pinned to 400pt is 400pt wide in
every window, so a self-query would make `hide("compact")` a constant. The
useful question is "is the space I was given narrow", and only an ancestor can
answer it. (CSS has the same rule, for the same reason.)

A size is contained when the style pins it (an explicit point length, a
percent of a definite parent, or `min == max`), or the parent stretches the
node across its cross axis from a definite box. Anything else falls through to
the next ancestor up. This is deliberately conservative: a flex item growing
from a definite basis is genuinely contained too, but proving it needs
reasoning about siblings, and claiming containment wrongly costs correctness
while falling through only costs a larger box.

If no ancestor is contained on an axis — a fully content-sized tree, say — a
band constraining that axis will not match, and rules stay inert rather than
guessing.

### Why it settles in two passes

A band tests a container's resolved size, which is not known until layout has
run. So the order is: lay out, evaluate, lay out again.

That is not an iteration with a bail-out. Because a container's size cannot
depend on its own contents, hiding or showing anything inside it cannot change
it, so the second pass resolves every container to the box the first one did
and every rule re-evaluates the same way. It converges in exactly two passes,
always. This is the same reason CSS requires `container-type` to impose size
containment — the difference is that the engine works it out instead of making
you declare it.

The second pass goes through the incremental cache like any other, so on a
resize that does not cross a threshold nothing changed and it is nearly free.
A whole extra layout is paid only on the frame a band actually flips.

### Rules

`add_rule(band, hide:)` and the fluent `hide(band)` / `show(band)`. **The last
matching rule wins**, so a broad hide followed by a narrow show reads the way
it is written. When no rule matches, the node returns to its parked display —
`Grid` if it has grid tracks, otherwise `Flex` — so a hide is never permanent.

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
