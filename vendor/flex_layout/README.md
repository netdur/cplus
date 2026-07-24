# flex_layout

Pure-C+ layout engine: **CSS Flexbox** + **CSS Grid**, optional **`@flex` DSL**
and HIG presets. UI-kit-agnostic — computes frames; an adapter applies them.
An optional platform-neutral responsive module classifies caller-supplied
viewport sizes using application-defined named breakpoints.

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

## Responsive configuration

Responsive classification is kept outside the flex algorithm. The host supplies
the viewport size and chooses every class name and threshold:

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
resolve again. If `next.same_class(previous)` is true, recalculate the existing
fluid layout; otherwise reapply class-specific styles or rebuild it.
Thresholds use the same logical unit as the supplied viewport (points, CSS
pixels, or another host-selected unit), never physical-screen detection.

## Docs

| File | Role |
|---|---|
| [docs/tutorial.md](docs/tutorial.md) | Fast path |
| [docs/guide.md](docs/guide.md) | Flex, grid, responsive config, measure, DSL/HIG, adapters |
| [docs/ref.md](docs/ref.md) | Types, enums, methods |

## Tests

```
cd vendor/flex_layout && cpc test
```
