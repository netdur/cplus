# CSS Grid

Set `display: grid` on a container and define its tracks. Grid covers the common
CSS Grid Level 1/2 path: `points` / `fr` / `auto` tracks, row-major auto-flow,
implicit rows, spanning, dense packing, explicit + named line placement, and
subgrid.

## Tracks

Columns and rows are lists of `GridTrack`s:

```
var g: flex::Node = flex::Node::new();
g.set_display(flex::Display::Grid);
g.add_grid_column(flex::GridTrack::points(200.0f64));   // fixed 200pt
g.add_grid_column(flex::GridTrack::fr(1.0f64));         // shares remaining space
g.add_grid_column(flex::GridTrack::auto());             // sized to its content
```

- **`points(v)`** — a fixed size.
- **`fr(v)`** — a flexible share of the space left after fixed + auto tracks and
  gaps (`1fr 2fr` splits the remainder 1 : 2).
- **`auto()`** — sized to the max content of the items in that track.

Rows work the same via `add_grid_row`. Rows beyond the explicit template are
**implicit** and sized `auto` (to their content).

## Auto-flow

Items fill the columns left-to-right, top-to-bottom, one cell each, and stretch
to their cell unless they set an explicit size. Gaps use the normal `gap` API:

```
g.set_gap(flex::Gutter::All, 12.0f64);
g.add_child(cell_a);   // (row 0, col 0)
g.add_child(cell_b);   // (row 0, col 1)
```

`grid-auto-flow: dense` backfills earlier holes left by spanning items:

```
g.set_grid_dense(true);
```

## Spanning

An item can span multiple columns and/or rows:

```
header.set_grid_column_span(2);   // occupies two columns (+ the gap between them)
photo.set_grid_row_span(2);
```

## Explicit + named line placement

Place an item at a specific 0-based line instead of auto-flowing it:

```
item.set_grid_column_start(2);    // start at column line 2
item.set_grid_row_start(1);       // start at row line 1
```

Name lines while building the template, then place by name:

```
g.name_grid_column_line("sidebar");   // names the line before the next column
g.add_grid_column(flex::GridTrack::points(200.0f64));
g.name_grid_column_line("main");
g.add_grid_column(flex::GridTrack::fr(1.0f64));

content.set_grid_column_start(g.column_line("main"));   // -> the "main" line index
// g.column_line(name) returns the 0-based index, or -1 if unknown
```

## Subgrid

A grid item that is itself a grid can **adopt the parent's tracks** over its
span, so its children align to the parent's grid lines and gaps:

```
var sub: flex::Node = flex::Node::new();
sub.set_display(flex::Display::Grid);
sub.set_grid_columns_subgrid(true);     // (and/or set_grid_rows_subgrid)
sub.set_grid_column_span(3);            // occupy 3 parent columns
sub.add_child(a);  sub.add_child(b);  sub.add_child(c);   // land on parent col lines
g.add_child(sub);
```

The subgrid re-lays-out whenever the parent does (its tracks are derived from the
parent), so the alignment always stays correct.

## Example — a dashboard

```
var dash: flex::Node = flex::Node::new();
dash.set_display(flex::Display::Grid);
dash.set_gap(flex::Gutter::All, 16.0f64);
dash.add_grid_column(flex::GridTrack::fr(1.0f64));
dash.add_grid_column(flex::GridTrack::fr(1.0f64));
dash.add_grid_column(flex::GridTrack::fr(1.0f64));

var wide: flex::Node = flex::Node::new();
wide.set_grid_column_span(2);            // a 2-wide hero tile
dash.add_child(wide);
dash.add_child(tile_a);
dash.add_child(tile_b);
dash.add_child(tile_c);

dash.calculate_layout(1200.0f64, 800.0f64, flex::Direction::LTR);
```
