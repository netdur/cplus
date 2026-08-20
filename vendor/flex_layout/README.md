# flex_layout

Pure-C+ layout engine: **CSS Flexbox** + **CSS Grid**, optional **`@flex` DSL**
and HIG presets. UI-kit-agnostic — computes frames; an adapter applies them.
Nodes can hide and show themselves by named size band, resolved against the
box they actually sit in.

```toml
[dependencies]
flex_layout = "*"
```

```cplus
import "flex_layout/flex_layout" as flex;

var root: flex::Node = flex::Node::new();
root.set_flex_direction(flex::FlexDirection::Row);

var sidebar: flex::Node = flex::Node::new();
sidebar.set_width(flex::StyleLength::points(200.0f64));
root.add_child(sidebar);

var content: flex::Node = flex::Node::new();
content.set_flex_grow(1.0f64);
root.add_child(content);

root.calculate_layout(1024.0f64, 768.0f64, flex::Direction::LTR);
```

## Conditional visibility

A node can state when it should be present, and the layout pass enforces it —
no size observer, no callback, no re-evaluation by hand:

```cplus
import "flex_layout/bands" as bands;

var set: bands::BandSet = bands::BandSet::defaults();
set.set("watch", max_width: 120.0f64, max_height: 120.0f64);   // or add your own

var sidebar: flex::Node = flex::Node::new().hide("compact");

root.calculate_layout(width: w, height: h, bands: #addr_of(set));
```

A **band** is a named box constraint (`min_width` / `max_width` /
`min_height` / `max_height`, each optional). Six ship pre-registered — `tiny`,
`compact`, `medium`, `expanded`, `large`, `xlarge` — so a shared vocabulary
exists without setup; naming the threshold once is what stops two screens
disagreeing about where a phone stops being a phone.

The band is measured against the node's nearest **contained** ancestor — the
closest box up the tree whose size does not depend on its own contents — never
the window. An app in Split View or on half a foldable was handed a box, and
the screen's width would answer a question nobody asked.

Passing no `bands` skips the mechanism entirely, so rules cost nothing until
a set is supplied.

## Responsive configuration (superseded)

`flex_layout/responsive` still ships for existing callers; prefer bands above
in new code. The host supplies the viewport size and chooses every class name
and threshold, and reapplying the styles stays the host's job:

```cplus
import "flex_layout/responsive" as responsive;

var screens: responsive::ResponsiveConfig =
    responsive::ResponsiveConfig::new("desktop");
screens.add_breakpoint("mobile", 300.0f64);  // width <= 300
screens.add_breakpoint("tablet", 900.0f64);  // 300 < width <= 900

let env: responsive::LayoutEnvironment = screens.resolve(view_width, view_height);
if env.is("mobile") {
    // Configure/build the compact form.
}
```

The module knows no devices, platforms, windows, or UI toolkits. On resize,
resolve again. If `next.is_same_class(previous)` is true, recalculate the
existing fluid layout; otherwise reapply class-specific styles or rebuild it.
Thresholds use the same logical unit as the supplied viewport (points, CSS
pixels, or another host-selected unit), never physical-screen detection.

## Docs

| File | Role |
|---|---|
| [docs/tutorial.md](docs/tutorial.md) | Fast path |
| [docs/guide.md](docs/guide.md) | Flex, grid, bands, measure, DSL/HIG, adapters |
| [docs/ref.md](docs/ref.md) | Types, enums, methods |

## Tests

```
cd vendor/flex_layout && cpc test
```
