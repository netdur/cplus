# flex_layout — documentation

A pure-C+ layout engine with full **CSS Flexbox** and **CSS Grid** parity, a
declarative **`@flex` DSL**, and **HIG defaults**. UI-kit-agnostic: it computes
frames; a platform adapter applies them to real views.

- **[guide.md](guide.md)** — concepts, the node model, flexbox, content sizing,
  positioning, measure callbacks, RTL, pixel rounding, incremental relayout.
- **[grid.md](grid.md)** — CSS Grid: tracks, `fr`/`auto`, spanning, dense flow,
  explicit + named line placement, subgrid.
- **[dsl.md](dsl.md)** — the `@flex { }` builder DSL and the HIG presets.
- **[api.md](api.md)** — full API reference (types, enums, every method).

## 30-second tour

```
import "flex_layout/flex_layout" as flex;

var root: flex::Node = flex::Node::new();          // a column (Yoga default)
root.set_flex_direction(flex::FlexDirection::Row);

var sidebar: flex::Node = flex::Node::new();
sidebar.set_width(flex::StyleLength::points(200.0f64));
root.add_child(sidebar);

var content: flex::Node = flex::Node::new();
content.set_flex_grow(1.0f64);                     // fills the remaining width
root.add_child(content);

root.calculate_layout(1024.0f64, 768.0f64, flex::Direction::LTR);

// frames are absolute in the root's coordinate space
match root.child_layout(1 as usize) {
    option::Option[flex::Layout]::Some(f) => { /* f.left / f.top / f.width / f.height */ }
    option::Option[flex::Layout]::None => {}
}
```

The same layout with the DSL:

```
var root: flex::Node = @flex {
    row {
        box().width(200.0)
        box().grow(1.0)
    }
};
root.calculate_layout(1024.0f64, 768.0f64, flex::Direction::LTR);
```

## Design in one paragraph

Nodes are **persistent and mutable** (like Yoga): build a tree, set style
imperatively, call `calculate_layout`, read each node's frame back off the node.
Content-driven sizing (text, images) enters through a **measure function
pointer**, so the engine never depends on a UI kit. The engine is exhaustively
tested (170+ tests) and memory-safe (ASan-clean). Its `Style::new` defaults match
Yoga; HIG spacing is an opt-in layer (see [dsl.md](dsl.md)).
