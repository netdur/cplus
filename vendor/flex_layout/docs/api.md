# API reference

All items live in the `flex_layout` package (imported here as `flex`).

## Value types

| Type | Fields / constructors |
|---|---|
| `Layout` | `left`, `top`, `width`, `height` (all `f64`) — a computed frame |
| `Size` | `width`, `height` (`f64`) — a measure result |
| `StyleLength` | `::points(f64)`, `::percent(f64)`, `::auto()`, `::undef()` |
| `GridTrack` | `::points(f64)`, `::fr(f64)`, `::auto()` |

`flex::undefined() -> f64` yields the NaN sentinel used for an unconstrained
available size; `flex::is_undef(x)` / `flex::is_def(x)` test it.

## Enums

| Enum | Variants |
|---|---|
| `FlexDirection` | `Column`, `ColumnReverse`, `Row`, `RowReverse` |
| `Justify` | `FlexStart`, `Center`, `FlexEnd`, `SpaceBetween`, `SpaceAround`, `SpaceEvenly`, `Stretch`, `Start`, `End` |
| `Align` | `Auto`, `FlexStart`, `Center`, `FlexEnd`, `Stretch`, `Baseline`, `SpaceBetween`, `SpaceAround`, `SpaceEvenly`, `Start`, `End` |
| `Wrap` | `NoWrap`, `Wrap`, `WrapReverse` |
| `Edge` | `Left`, `Top`, `Right`, `Bottom`, `Start`, `End`, `Horizontal`, `Vertical`, `All` |
| `PositionType` | `Static`, `Relative`, `Absolute` |
| `Overflow` | `Visible`, `Hidden`, `Scroll` |
| `Display` | `Flex`, `None`, `Grid` |
| `BoxSizing` | `BorderBox`, `ContentBox` |
| `Gutter` | `Column`, `Row`, `All` |
| `TrackUnit` | `Points`, `Fr`, `Auto` (inside `GridTrack`) |
| `MeasureMode` | `Undefined`, `Exactly`, `AtMost` (passed to a measure callback) |
| `Direction` | `Inherit`, `LTR`, `RTL` |

## `Node`

### Construction & tree

| Method | Signature |
|---|---|
| `Node::new` | `-> Node` — a default column node |
| `add_child` | `(ref this, take Node)` |
| `child_count` | `(this) -> usize` |

### Layout & read-back

| Method | Signature |
|---|---|
| `calculate_layout` | `(ref this, avail_w: f64, avail_h: f64, owner_dir: Direction)` |
| `round_to_pixel` | `(ref this, scale: f64)` — post-pass; snap frames to a pixel grid |
| `layout_left` / `layout_top` / `layout_width` / `layout_height` | `(this) -> f64` |
| `layout_frame` | `(this) -> Layout` |
| `layout_direction` | `(this) -> Direction` — resolved LTR/RTL |
| `child_layout` | `(this, index: usize) -> Option[Layout]` |

Every `layout` is **absolute in the root's coordinate space** (it includes all
ancestor offsets). To place a node into a per-superview view tree, subtract the
parent node's `layout` origin: `child.left - parent.left`, `child.top - parent.top`.
| `overflow` / `display` / `position_type` | `(this) -> Overflow / Display / PositionType` |

### Flex style setters

| Method | Signature |
|---|---|
| `set_direction` | `(ref this, Direction)` |
| `set_flex_direction` | `(ref this, FlexDirection)` |
| `set_justify_content` | `(ref this, Justify)` |
| `set_align_items` / `set_align_self` / `set_align_content` | `(ref this, Align)` |
| `set_flex_wrap` | `(ref this, Wrap)` |
| `set_flex_grow` / `set_flex_shrink` | `(ref this, f64)` |
| `set_flex_basis` | `(ref this, StyleLength)` |
| `set_flex` | `(ref this, f64)` — shorthand: grow n, shrink 1, basis 0 |
| `set_width` / `set_height` | `(ref this, StyleLength)` |
| `set_min_width` / `set_min_height` / `set_max_width` / `set_max_height` | `(ref this, StyleLength)` |
| `set_aspect_ratio` | `(ref this, f64)` — width / height |
| `set_position_type` | `(ref this, PositionType)` |
| `set_display` | `(ref this, Display)` |
| `set_box_sizing` | `(ref this, BoxSizing)` |
| `set_overflow` | `(ref this, Overflow)` |

### Edges & gaps

| Method | Signature |
|---|---|
| `set_margin` / `set_padding` / `set_border` / `set_position` | `(ref this, Edge, StyleLength)` |
| `set_gap` | `(ref this, Gutter, f64)` — points |
| `set_gap_length` | `(ref this, Gutter, StyleLength)` — e.g. percent |

`Edge::Start` / `End` are logical (resolve to left/right per direction); a
physical `Left`/`Right` wins if also set. An `auto` margin absorbs free space.

### Content callbacks & context

| Method | Signature |
|---|---|
| `set_context` | `(ref this, ctx: *u8)` — opaque, borrowed; handed to the callbacks |
| `context` | `(this) -> *u8` |
| `set_measure` | `(ref this, fn(*u8, f64, MeasureMode, f64, MeasureMode) -> Size)` |
| `set_baseline` | `(ref this, fn(*u8, f64, f64) -> f64)` — `(ctx, width, height) -> ascent` |

The callback's first argument is the node's context. An adapter stores the
backing view there so one `fn` can size any view (cast `ctx` back, ask for its
fitting size). The engine never frees the context — the adapter owns it.

### Grid (container / item)

| Method | Signature |
|---|---|
| `add_grid_column` / `add_grid_row` | `(ref this, GridTrack)` — container template |
| `set_grid_dense` | `(ref this, bool)` — container auto-flow: dense |
| `set_grid_column_span` / `set_grid_row_span` | `(ref this, i32)` — item span (>=1) |
| `set_grid_column_start` / `set_grid_row_start` | `(ref this, i32)` — item start line (0-based, -1 = auto) |
| `name_grid_column_line` / `name_grid_row_line` | `(ref this, str)` — names the line at the current track count |
| `column_line` / `row_line` | `(this, str) -> i32` — line index, or -1 |
| `set_grid_columns_subgrid` / `set_grid_rows_subgrid` | `(ref this, bool)` — item adopts parent tracks |

## `@flex` DSL

See [dsl.md](dsl.md). Surface: `Builder` (`new` / `add` / `finish`), containers
`row` / `column` / `box`, fluent modifiers (`width`, `height`, `width_pct`,
`height_pct`, `grow`, `shrink`, `padding`, `margin`, `gap`, `justify`, `align`,
`wrap`), and the HIG presets `vstack` / `hstack` / `screen` / `card` +
`.tappable()` / `.card_padding()` / `.screen_margins()` / `.std_gap()`.

## Notes on move semantics

- `add_child(take child)` and the DSL `Builder::add(take item)` **consume** the
  node (move it in).
- The fluent DSL modifiers (`.width(v)` etc.) also consume and return the node, so
  they chain: `box().width(200.0).grow(1.0)`. Assigning a fluent chain back to the
  same `var` (`n = n.width(...)`) trips the move checker — build inline, or use
  the engine's `set_*` setters (which mutate in place) when you hold a `var`.
