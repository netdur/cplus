# Migration — the 2026-07-31/08-01 API pass

Everything that changed, what still needs migrating, and the sharp edges that
are easy to rediscover the hard way. Signatures: [ref.md](ref.md).

## Renames

| Old | New |
|---|---|
| `Layout` (the type) | `Frame` |
| `layout_frame()` | `frame()` |
| `child_layout(i)` | `child_frame(at:)` |
| `layout_left/top/width/height()` | removed — read `frame().left` etc. |
| `layout_direction()` | `resolved_direction()` |
| `calculate_layout(w, h, dir)` | `calculate_layout(width:, height:, direction:)`, all defaulted |
| `set_payload(p, release)` | `attach(ptr, release:)` |
| `payload()` (`0` when none) | `attachment() -> Option[*u8]` |
| `has_payload()` | removed — `attachment()` is `None` |
| `clear_payload()` | `detach()` |
| `set_gap(gutter, f64)` | removed — `set_gap(gutter, length: StyleLength)` |
| `set_gap_length(...)` | `set_gap(...)` |
| `name_grid_column_line(n)` | `add_grid_column(track, line: n)` |
| `column_line(n) -> i32` (-1) | `column_line(named:) -> Option[i32]` |
| `is_undef` / `is_def` | `is_undefined` / `is_defined` |
| `StyleLength::undef()` | `StyleLength::undefined()` |
| `.width_pct` / `.height_pct` (DSL) | `.width_percent` / `.height_percent` |
| `node.children` (public field) | private — use `child_ptr(at:)` |

Labels are optional in C+, so adding parameter names and defaults broke no
positional call site. Only the table above is breaking.

## `flex_layout/responsive` is gone (2026-08-20)

`ResponsiveConfig` / `LayoutEnvironment` are removed, superseded by
`flex_layout/bands` — see [guide.md](guide.md), "Conditional visibility".

The old module classified a caller-supplied viewport into a named class and
left the host to reapply styles; it had no consumers, because that split is
the work. A `BandSet` is the same idea with two differences that matter: a
band is a box constraint rather than one max-width, and the ENGINE evaluates
it during layout, so a node carrying `hide("compact")` needs no observer, no
key lookup and no re-run on resize.

| Old | New |
|---|---|
| `ResponsiveConfig::new(fallback)` | `BandSet::defaults()` (six bands) or `BandSet::new()` |
| `add_breakpoint(name, up_to:)` | `set(name, min_width:, max_width:, min_height:, max_height:)` |
| `remove_breakpoint(name)` | `remove(name) -> bool` |
| `resolve(w, h)` + `env.is(name)` | `matches(name, w, h)` |
| `env.is_same_class(other)` | — (the pass re-decides; nothing to compare) |
| `LayoutEnvironment::orientation()` | — removed: device pose is not a layout input |

`Orientation` is gone rather than renamed. A phone in landscape, a tablet in
Split View and a foldable's outer screen can hand you the same box, so the
pose was never the question — the box is. A band constraining `max_height`
says the useful half of what portrait/landscape was reaching for, without
asking what the hardware is doing.

## Not yet migrated

- **`vendor/facet_gtk/src/facet_gtk.cplus:483`** — `{ (*node).children.at_ptr(i) }`
  must become `{ (*node).child_ptr(i) }` (identical `Option[*Node]` return).
  **This package does not compile until that line changes.** Left deliberately:
  the owner asked for no GTK work during the facet refactor. Nothing else in
  the file needs changing; `add_child`'s new `Status` return is ignorable
  (`Status` is Copy, discarding it leaks nothing).
- Every consumer was updated textually but **not built** — the workspace was
  mid-facet-refactor, so only `vendor/flex_layout` and `vendor/stdlib` were
  compiled and tested. Build `examples/pad_flex`, `examples/pad_portable`,
  `examples/hello_facet`, and `vendor/terminal` when the workspace is stable.

## Sharp edges

- **Cursor lifetime.** A `*Node` survives every structural change except the
  removal of itself or an ancestor. Hold one across possible removals only
  with a captured `removal_count()` (see ref.md); re-derive when it changed.
- **Whole-tree teardown is invisible** to `removal_count()` — it counts
  `remove_child`, not drops. A cursor must not outlive its tree. Hooking
  `Node::drop` would bump on every discarded build-time temporary, so it does
  not; per-node generations would need an arena, which the by-value builder
  model rules out.
- **No parent pointers, by design.** Roots are values the app moves around,
  and `remove_child` returns a subtree by value; a stored `_parent` would
  dangle on exactly those two operations. Layout runs top-down and frames are
  absolute, so nothing in the engine needs to climb. A cursor holder that
  needs the parent should capture it during the downward walk, then use
  `parent.child_index(of: node)`.
- **The cache compares snapshots, not setter calls.** A direct
  `n.style.width = ...` write is therefore as safe as `set_width` — but
  anything the snapshot cannot see needs an explicit invalidation. The engine
  covers the ones it owns (structural verbs, callback setters,
  `set_context`); content that changes what `measure` returns is the caller's
  job: `mark_content_changed()`.
- **A trailing `match` cannot end a function body** (E0333). Add `return;`
  after it.

## Where a property belongs (for generated bindings)

- **LAYOUT** — the algorithm reads it: write through a `set_*` setter or the
  `style` field. No mirrored copy, no dirty bit; the layout pass is the apply.
- **PAINT** — only the native view reads it (color, font, radius, opacity):
  keep it in the adapter's own state, never in engine `Style`. Every `Style`
  field is stored twice per node and compared each pass, so widening it costs
  memory and time on every layout.
- **MEASURE** — changes what the measure callback returns (text, font size):
  adapter state **plus** `mark_content_changed()` on the node.
- A few properties are both (a "visible" flag is `Display::None` for layout
  *and* a native hide); opacity is paint-only — a transparent node still
  occupies space, as in CSS.
