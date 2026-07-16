# facet

A platform-free UI description layer for C+. You describe a screen as a tree of
`Node` values with the `@facet { }` builder DSL; a small per-platform backend
(`facet_appkit`, ...) mounts that tree into native views and lays it out with
`flex_layout`. There is no HTML, no platform type, and no virtual DOM in the
description — it is plain data.

## The model in one paragraph

A component's `build` runs **once** and returns a `Node` tree. The backend
mounts it into native views and **retains** the tree; from then on the tree is
live and you edit it in place. facet is **not reactive**: there is no re-render
and no diff. When state changes, a handler addresses the one element that shows
it — by its `key` — and mutates that element directly
(`facet::find(key).set_text(...)`). You already hold the key, so there is no
delta to rediscover.

Two ways to update, never a third:

1. **by key** — `facet::find(key)` resolves a keyed element by its id and returns
   a `Handle` you mutate. That id is the element's `agent_id` / accessibility
   identifier, so **in-app code and an external/MCP agent address the UI the same
   way** — this is the ACI surface, not a separate API.
2. **by method** — call a method on the component; internally it updates its own
   UI by key. Same mechanism, wrapped behind a named action.

## Quick start — a counter

```cplus
import "facet/facet" as facet;
import "facet/runtime" as runtime;
import "stdlib/text" as text;

struct Counter { n: i32 }

impl Counter {
    // A handler is a METHOD: `ref this` is the receiver, `sender` is the control
    // that fired. `find(key)` addresses the element by its id — global, exactly
    // how an agent addresses it. No cp, no static.
    fn inc(ref this, sender: *u8) {
        this.n = this.n + 1;
        let msg: text::Text = "count ${this.n}";
        let _u: facet::Handle = facet::find("count").set_text(msg.view());
        return;
    }
}

impl Counter: facet::Component {
    // `build` takes `ref this`, so it binds its own handler directly —
    // `.on_click(this.inc)`, the receiver being this component. `build` runs ONCE.
    fn build(ref this) -> facet::Node {
        let t: text::Text = "count ${this.n}";
        return @facet {
            vstack {
                label(t.view()).key("count")     // keyed so a handler can find it
                button("+1").on_click(this.inc)  // bind the method; receiver is the component
            }
        };
    }
}

fn main() -> i32 {
    // run_component OWNS the instance for the window's life — no module static.
    runtime::run_component(Counter { n: 0 }, title: "Counter", width: 300.0f64, height: 160.0f64);
    return 0;
}
```

`build` runs once. Clicking `+1` runs `Counter::inc`, which finds `"count"` and
sets its text on the same label view — the label is never rebuilt. Binding
`this.inc` spends the handler's `ctx` on the receiver, so `inc` takes only
`sender`. No module static, no `component_ctx`, no cp threading: `run_component`
holds the instance, and `find("count")` addresses the element by its id — the
same handle an agent uses.

## Layout

```
vendor/facet/
├── Cplus.toml                 ← package manifest (deps: stdlib, flex_layout)
├── README.md                  ← you are here
├── docs/                      ← this documentation
│   ├── component-model.md     ← components, build-once, where state lives
│   ├── updates.md             ← the keyed-direct update path (find + verbs)
│   ├── lifecycle.md           ← stage / attach / detach — the router pattern
│   ├── widgets.md             ← the @facet DSL: leaves, containers, modifiers
│   └── backends.md            ← the Backend vtable + run host; adding a backend
└── src/
    ├── facet.cplus            ← facet/facet — the core: Node, DSL, Component,
    │                            find/Handle, stage, Color/Style
    ├── runtime.cplus          ← facet/runtime — the host facade: run, alert,
    │                            present_window, menus (selects a backend)
    └── runtime_linux.cplus    ← Linux shadow of runtime.cplus (resolver override)
```

## Modules

| import | what it gives you |
|---|---|
| `facet/facet` | `Node`, the `@facet` DSL, every widget/container constructor, `Component`, `find` + `Handle` + mutators/verbs, `Lifecycle` + `stage`/`is_attached`, `Color`/`Style` |
| `facet/runtime` | the host facade: `run(window)`, `present_window`, `alert`, menus, the `Window` interface. Pulls in the platform backend (AppKit on macOS; `runtime_linux.cplus` shadows on Linux) |

A backend package (`facet_appkit`) supplies the mount + native ops and the
window host. Apps import `facet/facet` for the description and `facet/runtime`
to run it.

## Where to read next

- New to facet: [docs/component-model.md](docs/component-model.md), then
  [docs/updates.md](docs/updates.md).
- Building screens: [docs/widgets.md](docs/widgets.md).
- Multi-screen / navigation: [docs/lifecycle.md](docs/lifecycle.md).
- Porting to a new toolkit: [docs/backends.md](docs/backends.md).

## Status

The description model, the AppKit backend (`facet_appkit`), the keyed-direct
update path, and the component lifecycle are implemented and tested. A GTK
backend (`facet_gtk`) is a stub. The widget/feature coverage against SwiftUI is
tracked separately in the design notes.
