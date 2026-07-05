# facet_appkit

facet's AppKit backend — two modules:

| Module | Import | What it is |
|---|---|---|
| `ui` | `import "facet_appkit/ui" as ui;` | The `@ui` contextual-builder layer: widgets + layout containers as peers over flex_layout and the typed appkit binding. Usable standalone (no facet). |
| `facet_appkit` | `import "facet_appkit/facet_appkit" as fak;` | The facet `Renderer` ops, `mount`/`mount_into`/`run` hosts, and the `run_app` MVU runtime. |

## The `@ui` layer (platform-native, full power)

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

Every widget constructor returns a `flex::Node` that owns its NSView through
the engine's payload slot (released exactly once) and self-measures through
the engine's measure callback. Containers: `column`/`row` plus HIG presets
`vstack`/`hstack`/`screen`/`card`. Widgets: `label`, `wrap_label`, `button`,
`button_ctx` (per-item identity), `icon_button(_ctx)`, `symbol`, `image`,
`divider`, `spacer`, `box`. Anything the binding can build enters through the
escape hatches — `view(v)` (measured), `wrap_view(v)`, `fixed_view(v)` — so
AppKit coverage never requires per-widget wrapper code. `apply` converts
flex's top-down absolute frames into AppKit's nested bottom-up view tree;
`fill_scroll` packages the two-phase scroll layout into a flipped document.

Controls are wired exactly once, at construction (a replaced target strands
its creation reference — verified); backends that wire their own action use
`button_view` (styled, unwired).

## The facet layer (portable description + state)

```
import "facet/facet" as facet;
import "facet_appkit/facet_appkit" as fak;

static COUNT: i32 = 0;

fn on_inc(sender: *u8, ctx: *u8) { COUNT = COUNT + 1; return; }

fn view() -> facet::Node {
    let t: text::Text = "count ${COUNT}";
    return @facet {
        vstack {
            label(t.view(), size: 20.0f64)
            button("+1").on_click(on_inc)
        }
    };
}

fn main() -> i32 { fak::run_app(view, title: "counter"); return 0; }
```

State lives wherever the app puts it; handlers mutate it; after every event
handler the runtime re-runs `view()` and re-renders. One rule, no hidden
subscriptions. For a static tree use `fak::run(root)` or
`fak::mount_into(#addr_of(root), view_ptr, w, h)`.

Component idioms (see the living-doc tests in `src/test_main.cplus`):
a component is a plain fn returning `facet::Node`; scoped state is a state
struct passed by pointer, where the click ctx IS the state slice.

## Tests

`cpc test` runs both modules' suites: the `@ui` surface (containers, fluent
chains, apply geometry, scroll), the facet pipeline (vtable ops, `@facet` ->
NSView), the app runtime (counter, removable list, hook ordering), the
component idioms, and the appkit_ext companions. `main` is a leak harness —
`leaks --atExit -- ./target/debug/facet_appkit_tests` shows no per-cycle
growth and zero `Cplus*` target leaks.

Porting to another platform: clone this package's shape (gtk4_flex-style `ui`
module + three Renderer ops + apply + click wiring). The checklist is
bugs/dsl-notes-from-iris.md note B; layout, containers, and mount are already
write-once in vendor/facet + vendor/flex_layout.
