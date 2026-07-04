# The `@flex` DSL + HIG defaults

`@flex { }` is a contextual builder block (C+'s compiler-owned DSL). It desugars
to ordinary `flex_layout` calls — no macros, no runtime — and produces a `Node`
tree you then lay out.

Import the package as `flex` (the alias is the DSL context name):

```
import "flex_layout/flex_layout" as flex;

var ui: flex::Node = @flex {
    row {
        box().width(200.0)
        box().grow(1.0).padding(8.0)
    }
};
ui.calculate_layout(1024.0f64, 768.0f64, flex::Direction::LTR);
```

## The three pieces

- **The block** `@flex { ... }` builds a **column** whose children are the items.
- **Containers** — a bare `name { ... }` nests a container of the same context:
  - `column { }` — a vertical stack (same as the block itself)
  - `row { }` — a horizontal stack
  - plus the HIG containers below
- **Leaves** — `box()` is an empty node; give it size / flex / content via
  modifiers.

Containers nest arbitrarily deep:

```
@flex {
    column {
        row { box().width(40.0)  box().width(40.0) }
        row { box().grow(1.0) }
    }
}
```

> Nesting here is **same-context containers**, not a "sub-DSL". A *different*
> `@`-block inside an `@flex` block is not allowed (and isn't needed for layout).

## Modifiers

Modifiers are fluent — they chain on one line and each returns the node:

```
box().width(200.0).height(44.0).padding(8.0)
box().grow(1.0).align(flex::Align::Center)
```

| Modifier | Effect |
|---|---|
| `.width(v)` / `.height(v)` | fixed size in points |
| `.width_pct(v)` / `.height_pct(v)` | size as a percent of the parent |
| `.grow(v)` / `.shrink(v)` | flex grow / shrink factor |
| `.padding(v)` / `.margin(v)` | uniform padding / margin (points) |
| `.gap(v)` | gap between this container's children |
| `.justify(j)` / `.align(a)` / `.wrap(w)` | justify-content / align-items / flex-wrap |

Any of the engine's `set_*` setters also work as modifiers (they mutate in place;
use them on their own leading-dot line).

## Flow control

`if` and `for` work as Flutter-style collection flow control — they add items
into the enclosing container:

```
@flex {
    column {
        header()
        for item in items {
            row { label(item) }
        }
        if show_footer {
            footer()
        }
    }
}
```

`while` / `return` / `break` / etc. are not allowed inside a block.

---

# HIG defaults

Apple's Human Interface Guidelines layout system is an **8-point grid**: related
items sit 8pt apart, content gets 16pt padding, macOS windows keep ~20pt edge
margins, and any tappable control needs a **44×44pt** hit area. The engine ships
these as **opt-in** presets — the engine's own `Style::new` defaults stay
unchanged, so you choose HIG spacing by using the presets.

## Preset containers

| Container | HIG rule |
|---|---|
| `vstack { }` | vertical stack, **8pt** between items |
| `hstack { }` | horizontal stack, **8pt** between items |
| `screen { }` | column with **20pt** window/screen edge margins + 8pt spacing |
| `card { }` | column with **16pt** content padding + 8pt spacing |

## Preset modifiers

| Modifier | Effect |
|---|---|
| `.tappable()` | enforce the **44×44pt** minimum tap target |
| `.card_padding()` | 16pt content padding |
| `.screen_margins()` | 20pt edge margins |
| `.std_gap()` | 8pt gap between children |

## Constants

```
HIG_SPACE_XS = 4    HIG_SPACE_S = 8    HIG_SPACE_M = 16
HIG_SPACE_L  = 20   HIG_SPACE_XL = 32  HIG_TAP_MIN = 44
```

## Example — a HIG-correct app shell

No magic numbers; every value comes from the guidelines:

```
var ui: flex::Node = @flex {
    screen {                          // 20pt window margins
        hstack {                      // sidebar | content, 8pt apart
            box().width(200.0)
            card {                    // 16pt padding
                box().grow(1.0)
            }
        }
    }
};
ui.calculate_layout(1024.0f64, 768.0f64, flex::Direction::LTR);
```

See the HIG **[Layout](https://developer.apple.com/design/human-interface-guidelines/layout)**
foundation for the source guidance.
