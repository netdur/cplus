# flex_layout

Pure-C+ layout engine: **CSS Flexbox** + **CSS Grid**, optional **`@flex` DSL**
and HIG presets. UI-kit-agnostic — computes frames; an adapter applies them.

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

## Docs

| File | Role |
|---|---|
| [docs/tutorial.md](docs/tutorial.md) | Fast path |
| [docs/guide.md](docs/guide.md) | Flex, grid, measure, DSL/HIG, adapters |
| [docs/ref.md](docs/ref.md) | Types, enums, methods |

## Tests

```
cd vendor/flex_layout && cpc test
```
