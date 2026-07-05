# facet_appkit

The AppKit `Renderer` for facet, over appkit_flex. facet owns the tree walk
and all layout (the shared flex engine); this package materializes the three
leaf widgets and hosts the app.

## Static tree

```
import "facet/facet" as facet;
import "facet_appkit/facet_appkit" as fak;

var root: facet::Node = @facet {
    card {
        label("Iris", size: 28.0f64, bold: true)
        hstack {
            button("New", primary: true).on_click(on_new)
            button("Open").on_click(on_open)
        }
    }
};
fak::run(root, title: "Iris");            // window + event loop
// or: fak::mount_into(#addr_of(root), view_ptr, w, h) into your own view
```

## App runtime (MVU)

State lives wherever the app puts it; handlers mutate it; the runtime re-runs
the view after every event handler. One rule, no hidden machinery.

```
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

`on_click(cb, ctx:)` carries per-item identity (an index as a pointer), so
removable lists reflow without `setTag:` workarounds.

## Wiring rule

A control's target/action is wired exactly once, at construction (verified:
replacing a wired target strands its creation reference). The re-render hook
rides the same target (`appkit_ext::set_control_action_ctx_then`).

## Tests

`cpc test` here runs the package suite: vtable ops, the `@facet` -> NSView
pipeline, the counter (label follows state through a real click), the
removable list, and after-hook ordering. `main` is a leak harness:
`leaks --atExit -- ./target/debug/facet_appkit_tests` — no per-cycle growth,
zero `Cplus*` target leaks.
