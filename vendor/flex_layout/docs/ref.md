# Reference

Manual for the `flex_layout` package. Signatures and behavior only.
Walkthrough: [tutorial.md](tutorial.md). Concepts: [guide.md](guide.md).

All items live in the `flex_layout` package (imported here as `flex`).

## Value types

| Type | Fields / constructors |
|---|---|
| `Frame` | `left`, `top`, `width`, `height` (all `f64`) — a computed frame |
| `Size` | `width`, `height` (`f64`) — a measure result |
| `StyleLength` | `::points(f64)`, `::percent(f64)`, `::auto()`, `::undefined()` |
| `GridTrack` | `::points(f64)`, `::fr(f64)`, `::auto()` |

`flex::undefined() -> f64` yields the NaN sentinel used for an unconstrained
available size; `flex::is_undefined(x)` / `flex::is_defined(x)` test it. The
`calculate_layout` defaults cover the common case, so most callers never touch
the sentinel directly.

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
| `children` | public field, `Vec[Node]` — walk with `at_ptr` for deep traversal |

### Layout & read-back

| Method | Signature |
|---|---|
| `calculate_layout` | `(ref this, width: f64 = undefined(), height: f64 = undefined(), direction: Direction = Inherit)` |
| `round_to_pixel` | `(ref this, scale: f64 = 1.0)` — post-pass; snap frames to a pixel grid |
| `frame` | `(this) -> Frame` — the computed frame |
| `child_frame` | `(this, at: usize) -> Option[Frame]` |
| `resolved_direction` | `(this) -> Direction` — resolved LTR/RTL |
| `overflow` / `display` / `position_type` | `(this) -> Overflow / Display / PositionType` |

An omitted `calculate_layout` axis is unconstrained (content-sized); the
default `direction` resolves to LTR at the root. Labels pass in free order:
`root.calculate_layout(width: 1024.0f64, height: 768.0f64)`.

Every frame is **absolute in the root's coordinate space** (it includes all
ancestor offsets). To place a node into a per-superview view tree, subtract the
parent node's frame origin: `child.left - parent.left`, `child.top - parent.top`.

### Flex style setters

| Method | Signature |
|---|---|
| `set_direction` | `(ref this, direction: Direction)` |
| `set_flex_direction` | `(ref this, direction: FlexDirection)` |
| `set_justify_content` | `(ref this, justify: Justify)` |
| `set_align_items` / `set_align_self` / `set_align_content` | `(ref this, align: Align)` |
| `set_flex_wrap` | `(ref this, wrap: Wrap)` |
| `set_flex_grow` / `set_flex_shrink` | `(ref this, factor: f64)` |
| `set_flex_basis` | `(ref this, basis: StyleLength)` |
| `set_flex` | `(ref this, factor: f64)` — shorthand: grow n, shrink 1, basis 0 |
| `set_width` / `set_height` | `(ref this, length: StyleLength)` |
| `set_min_width` / `set_min_height` / `set_max_width` / `set_max_height` | `(ref this, length: StyleLength)` |
| `set_aspect_ratio` | `(ref this, ratio: f64)` — width / height |
| `set_position_type` | `(ref this, position: PositionType)` |
| `set_display` | `(ref this, display: Display)` |
| `set_box_sizing` | `(ref this, sizing: BoxSizing)` |
| `set_overflow` | `(ref this, overflow: Overflow)` |

### Edges & gaps

| Method | Signature |
|---|---|
| `set_margin` / `set_padding` / `set_border` / `set_position` | `(ref this, edge: Edge, length: StyleLength)` |
| `set_gap` | `(ref this, gutter: Gutter, length: StyleLength)` — points or percent |

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

### Owned attachment

| Method | Signature |
|---|---|
| `attach` | `(ref this, ptr: *u8, release: fn(*u8))` — the node takes ownership |
| `attachment` | `(this) -> Option[*u8]` |
| `detach` | `(ref this)` — releases now, leaves the node attachment-free |

The owning counterpart to `set_context`: the engine calls `release(ptr)`
exactly once — when the node drops, on `detach()`, or when a new `attach`
replaces the value. An adapter keeps the borrowed measure context and the
owned attachment on the same node (typically the same view pointer: retain it
into the attachment, release in the release fn). The engine never reads
through the pointer, so it stays UI-kit agnostic.

### Grid (container / item)

| Method | Signature |
|---|---|
| `add_grid_column` / `add_grid_row` | `(ref this, track: GridTrack, line: str = "")` — container template; `line` names the line before the track |
| `set_grid_dense` | `(ref this, dense: bool)` — container auto-flow: dense |
| `set_grid_column_span` / `set_grid_row_span` | `(ref this, count: i32)` — item span (>=1) |
| `set_grid_column_start` / `set_grid_row_start` | `(ref this, line: i32)` — item start line (0-based; -1 resets to auto) |
| `column_line` / `row_line` | `(this, named: str) -> Option[i32]` — a named line's index |
| `set_grid_columns_subgrid` / `set_grid_rows_subgrid` | `(ref this, enabled: bool)` — item adopts parent tracks |

Line naming matches CSS `[sidebar] 200px`:
`g.add_grid_column(GridTrack::points(200.0f64), line: "sidebar")`.

## `@flex` DSL surface

| Piece | Names |
|---|---|
| Builder | `Builder::new` / `add(take)` / `finish` |
| Containers | `row` / `column` / `box` |
| Fluent | `width`, `height`, `width_percent`, `height_percent`, `grow`, `shrink`, `padding`, `margin`, `gap`, `justify`, `align`, `wrap` |
| HIG containers | `vstack` / `hstack` / `screen` / `card` |
| HIG modifiers | `.tappable()` / `.card_padding()` / `.screen_margins()` / `.std_gap()` |
| HIG constants | `HIG_SPACE_XS/S/M/L/XL` (4/8/16/20/32), `HIG_TAP_MIN` (44) |

Usage rules and examples: [guide.md](guide.md) (`@flex` DSL and HIG).

## Responsive module

Import separately with `import "flex_layout/responsive" as responsive;`.
It has no dependency on the flex algorithm or a platform UI toolkit.

| Type | API |
|---|---|
| `ResponsiveConfig` | `::new(fallback)`, `add_breakpoint(name, up_to:)`, `remove_breakpoint(name)`, `remove_all_breakpoints()`, `set_fallback(name)`, `breakpoint_count()`, `resolve(width, height)` |
| `LayoutEnvironment` | `width()`, `height()`, `class_name()`, `is(name)`, `breakpoint_width() -> Option[f64]`, `orientation()`, `is_same_class(other)` |
| `Orientation` | `Portrait`, `Landscape`, `Square` |

Breakpoints are inclusive maximum widths
(`config.add_breakpoint("mobile", up_to: 300.0f64)`). Resolution chooses the
smallest matching maximum, so configuration order does not matter. Above every
maximum, the configured fallback class is used; `breakpoint_width()` is `None`
for a fallback class.

## Notes on move semantics

- `add_child(take child)` and the DSL `Builder::add(take item)` **consume** the
  node (move it in).
- Fluent DSL modifiers also consume and return the node, so they chain:
  `box().width(200.0).grow(1.0)`. Assigning a fluent chain back to the same
  `var` (`n = n.width(...)`) trips the move checker — build inline, or use
  the engine's `set_*` setters when you hold a `var`.
