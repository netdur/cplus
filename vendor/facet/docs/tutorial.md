# Tutorial

A working counter, then the two rules that define facet. Deeper reading:
[guide.md](guide.md). Signatures: [ref.md](ref.md). Topical deep dives are
linked from the guide (components, updates, lifecycle, widgets, backends).

## Setup

```toml
[dependencies]
facet = "*"
# platform: facet_appkit (macOS) is selected via facet/runtime
```

```cplus
import "facet/facet" as facet;
import "facet/runtime" as runtime;
import "stdlib/text" as text;
```

## Counter

```cplus
struct Counter { n: i32 }

impl Counter {
    fn inc(ref this, sender: *u8) {
        this.n = this.n + 1;
        let msg: text::Text = "count ${this.n}";
        let _u: facet::Handle = facet::find("count").set_text(msg.view());
        return;
    }
}

impl Counter: facet::Component {
    fn build(ref this) -> facet::Node {
        let t: text::Text = "count ${this.n}";
        return @facet {
            vstack {
                label(t.view()).key("count")
                button("+1").on_click(this.inc)
            }
        };
    }
}

impl Counter: facet::Lifecycle {
    fn on_attach(ref this) { return; }   // fired by the runtime after the mount
    fn on_detach(ref this) { return; }   // fired when the window closes, before teardown
}

fn main() -> i32 {
    runtime::run_component(
        Counter { n: 0 },
        title: "Counter",
        width: 300.0f64,
        height: 160.0f64
    );
    return 0;
}
```

What matters:

1. **`build` runs once** — the backend mounts the tree and keeps it live.
2. **Updates are keyed** — `find("count").set_text(...)` mutates one element;
   there is no re-render and no diff.
3. **Handlers are methods** — `.on_click(this.inc)`; `find` is global (same id
   an agent would use).
4. **Lifecycle is the runtime's job** — `run_component` fires `on_attach` once
   the tree is live (initial work goes there, not in `main`) and `on_detach`
   before teardown. The counter needs nothing, so its hooks are empty.

## Minimal DSL

```cplus
@facet {
    vstack {
        label("Title", size: 20.0f64, bold: true)
        hstack {
            button("New", primary: true).on_click(this.on_new)
            button("Open")
        }
    }
    .padding(12.0f64)
    .gap(8.0f64)
}
```

More widgets and modifiers: [widgets.md](widgets.md).

## Day-one rules

- State lives in **struct fields**, not in a virtual DOM.
- Prefer **window-unique keys** so plain `find(key)` is enough.
- Missed keys: empty `Handle` (mutators no-op) — safe on teardown.
- Host entry: `runtime::run_component` or `runtime::run(window)`; a
  multi-screen app runs an `App` of named screens
  ([app-screens.md](app-screens.md)).
- `run_component` needs `Component` **and** `Lifecycle`; empty hooks are fine.
- Slow reads go in a service (`load_async`), never in a handler:
  [services.md](services.md).
