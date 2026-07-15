# facet_appkit

facet's **AppKit** backend — two modules:

| Module | Import | What it is |
|---|---|---|
| `ui` | `import "facet_appkit/ui" as ui;` | The `@ui` contextual-builder layer: widgets + layout containers over flex_layout and the typed appkit binding. Usable standalone (no facet). |
| `facet_appkit` | `import "facet_appkit/facet_appkit" as fak;` | Full per-kind `Renderer` ops, `set_identity`, `mount`/`mount_into`/`run` (window host). |

## Architecture (post multi-backend plan)

1. **Portable description** lives in `vendor/facet`: pure-data `Node` + closed kind set + flat `Renderer` vtable + `mount`.
2. **This package** supplies one op per kind (thin adapters over `ui::*`) and `set_identity`.
3. **`mount` calls `set_identity` after every widget-producing op** — pins `accessibilityIdentifier` + packed `(role, drive)` affordance metadata. Agents read the declaration; they do not re-guess from `isKindOfClass:`.
4. **Layout is write-once** in facet + flex_layout. Only leaf/widget ops are per-backend.

### Renderer kinds (all implemented)

`label`, `wrap_label`, `button`, `text_area`, `composer`, `bordered`, `clickable`, `split` + `set_identity`.

`tree` / `file_tree` stay AppKit-only `ui::` escape hatches (structured payload, not in `Node`).

### Handler conventions

| Field | Sentinel | Why |
|---|---|---|
| `Node.click` (buttons) | `noop` (never null) | Always safe to call |
| Optional handlers (composer, gestures, text_area `on_change`) | `0 as fn(*u8,*u8)` | Backend wires only when non-null |

### Agent tagging paths

| Path | Id | Affordance |
|---|---|---|
| `@facet` + mount | `set_identity` → accessibilityIdentifier | packed associated object |
| `@ui` with `agent_id:` | `tag_agent` → accessibilityIdentifier | packed associated object (same key) |
| Hand-built | `agent::set_agent_id` and/or accessibilityIdentifier | `agent::set_affordance` if needed |

## The `@ui` layer (platform-native)

```
import "flex_layout/flex_layout" as flex;
import "facet_appkit/ui" as ui;

var tree: flex::Node = @ui {
    screen {
        label("Iris", size: 28.0f64, bold: true)
        hstack {
            button("New Project", on: cb, primary: true)
            button("Open Project", on: cb)
        }
    }
};
tree.calculate_layout(800.0f64, flex::undefined(), flex::Direction::LTR);
ui::apply(#addr_of(tree), content.raw(), false);
```

Widgets own their NSView in the flex payload; measurement is per-kind inside the op. Escape hatches: `view` / `wrap_view` / `fixed_view`.

## The `@facet` layer (portable description)

```
import "facet/facet" as facet;
import "facet_appkit/facet_appkit" as fak;

static COUNT: i32 = 0;
fn on_inc(sender: *u8, ctx: *u8) { COUNT = COUNT + 1; return; }

fn view() -> facet::Node {
    let t: text::Text = "count ${COUNT}";
    return @facet {
        vstack {
            label(t.view(), size: 20.0f64).agent_id("count-label")
            button("+1").on_click(on_inc).agent_id("inc-btn")
            clickable {
                label("row")
            }.on_click(on_inc).agent_id("row")
        }
    };
}

fn main() -> i32 { fak::run(view(), title: "counter"); return 0; }
```

**Re-render (component model — there is no global runtime):** a handler mutates state, then the owner re-renders. Two paths: automatic for `@ui` components (a handler wired via `ui::button(on: component.method)` re-renders that component through `component_after`), or explicit (clear the host view and call `mount_into` again). The `run` above mounts once; drive updates by one of those two paths.

**Wrapper + named params:** use fluent modifiers after the block (`.on_click`, `.agent_id`). Primary click for `clickable` is `Node.click`.

**Multi-child wrappers:** `bordered` / `clickable` with 2+ children mount them as a portable column (nothing is dropped).

## Tests

`cd vendor/facet_appkit && cpc test` — `@ui` suite, facet pipeline, full-kind mount, agent e2e (button / clickable / composer / text_area), re-render, components.

## Porting another backend

Clone this shape: per-kind ops (~15–40 LOC each) + `set_identity` + a small window host (mount + a component-invalidate primitive; no global runtime). Layout and mount stay in `vendor/facet` + `flex_layout`. See `plans/facet-multibackend-proposal.md`. A GTK backend is not built yet — implement `facet_gtk` on a Linux host using this package as the reference.
