# appkit_flex

The view-bearing AppKit layer over the `flex_layout` engine, and a `@`-DSL
context in which layout containers and widgets are peers. One import gives a
SwiftUI-shaped surface:

```
import "flex_layout/flex_layout" as flex;
import "appkit_flex/appkit_flex" as ui;

var tree: flex::Node = @ui {
    screen {
        label("Iris", size: 28.0f64, bold: true)
        label("Version 0.0.1", size: 12.0f64, secondary: true)
        wrap_label("Create a new project or open an existing one.", secondary: true)
        hstack {
            button("New Project", on: on_new, primary: true)
            button("Open Project", on: on_open)
        }
        for p in projects {
            hstack {
                symbol("folder", 14.0f64, secondary: true)
                label(p)
            }
        }
    }
};
tree.calculate_layout(800.0f64, flex::undefined(), flex::Direction::LTR);
tree.round_to_pixel(2.0f64);
ui::apply(#addr_of(tree), content.raw(), false);
```

Everything is a `flex::Node`. A widget constructor creates the NSView, hands
ownership to the node (the engine's payload slot releases it exactly once),
and wires content sizing through the engine's measure callback. `if`/`for`
collection flow control, fluent styling chains (`hstack { ... }.gap(4.0)`),
and `.set_*` modifier lines all come from the compiler's builder-block DSL —
see `flex_layout/docs/dsl.md` for the block grammar.

## Containers

| | |
|---|---|
| `column { }` / `row { }` | plain stacks, no preset spacing |
| `vstack { }` / `hstack { }` | 8pt between items (HIG) |
| `screen { }` | column, 20pt window margins + 8pt spacing |
| `card { }` | column, 16pt padding + 8pt spacing |

The `@ui { }` block itself is a column; containers nest arbitrarily. Style a
container with a same-line chain after its `}` or `.set_*` lines under it.

## Widgets

| Constructor | Notes |
|---|---|
| `label(text, size:, bold:, secondary:)` | single-line, self-measuring |
| `wrap_label(text, size:, secondary:)` | wraps to its flexed width |
| `button(title, on:, primary:)` | `on: fn(*u8)` gets the sender |
| `button_ctx(title, on:, ctx:, primary:)` | `on: fn(*u8, *u8)` gets sender + your pointer |
| `icon_button(symbol_name, on:)` / `icon_button_ctx(...)` | borderless SF Symbol button |
| `symbol(name, side, secondary:)` | SF Symbol at a fixed size |
| `image(path)` | proportional scaling; size it with `.width()`/`.height()` |
| `divider()` | NSBox separator; give it `.width(1.0)` in a row |
| `spacer()` | viewless flexible space |
| `box()` | viewless layout box |

Escape hatches wrap any hand-built view: `view(v)` (measured), `wrap_view(v)`
(wrapping text), `fixed_view(v)` (you size it). All named-parameter defaults
are optional: `label("hi")` is valid.

`button_ctx` carries per-item identity without `setTag:`: pass an index as
the pointer (`i as *u8`) or a pointer you own. The wiring lives in
`appkit/appkit_ext` (`set_control_action_ctx`), usable directly on any
control.

## Applying a layout

`apply(root, parent_view, parent_flipped)` walks the computed frames and
builds a real nested NSView tree: a widget node contributes its own view, a
view-less container gets a fresh transparent NSView (so per-row operations
have a real superview), and coordinates convert from flex's top-down absolute
frames to AppKit's bottom-up superview-relative ones. Views survive the node
tree: the superview's retain keeps them alive after the nodes (and their
payload references) drop — rebuild the tree freely.

## Scroll views

Scrolling is two-phase by nature: the document can only be laid out once the
outer pass has sized the viewport.

```
let scroll = ak::ScrollView::new(...);
// outer tree: place the viewport
//   card { fixed_view(scroll.as_view()).grow(1.0) }
// ... calculate_layout + apply ...
let doc_height: f64 = ui::fill_scroll(scroll, @ui {
    vstack {
        for row in rows { hstack { label(row) } }
    }
});
```

`fill_scroll` lays the rows out at the viewport width, applies them into a
fresh flipped document view (rows read top-down, the list opens at the top),
and installs it as the scroll's document. Call it again to rebuild after a
model change.

## Tests

`cpc test` from this directory runs the package suite: builder protocol,
widget wiring (including firing actions through the wired target), apply
geometry (flip math, synthesized containers), scroll fill, and the literal
`@ui` block surface (`src/test_main.cplus`). The test binary's `main` is a
leak harness: `leaks --atExit -- ./target/debug/appkit_flex_tests` shows no
per-cycle growth over repeated build/apply/drop cycles.
