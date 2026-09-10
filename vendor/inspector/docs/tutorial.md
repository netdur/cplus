# Tutorial

Quick path: put the panel beside your UI, edit a property, add a node. Deeper
rationale and gotchas live in [guide.md](guide.md); signatures in
[ref.md](ref.md); the socket namespace in [wire.md](wire.md).

## Setup

```toml
[dependencies]
inspector = "*"
```

```cplus
import "inspector/widget" as panel;
import "facet_agent/inspect_tree" as itree;
```

## A panel beside your app

The panel is a `Component`. Hold it as a **field**, so it lives exactly as long
as the component the runtime owns.

```cplus
struct Probe {
    panel: panel::Inspector,
}

impl Probe: component::Component {
    fn build(ref this) -> core::Node {
        var row: core::Builder = core::Builder::new();
        row.add(app_side());
        row.add(this.panel.build().width(620.0f64));
        return ui::row(row, key: "window").grow(1.0f64);
    }
}
```

facet fires `Lifecycle` for the component the runtime owns, not for one nested
inside another's tree — so the host runs the panel's two hooks:

```cplus
impl Probe: component::Lifecycle {
    fn on_attach(ref this) { panel::attach(#addr_of(this.panel)); return; }
    fn on_detach(ref this) { panel::detach(#addr_of(this.panel)); return; }
}
```

```cplus
fn main() -> i32 {
    iplatform::install();
    let probe: Probe = Probe { panel: panel::embedded() };
    let _final: Probe = runtime::run_component(probe, title: "probe",
                                               width: 1320.0f64, height: 680.0f64);
    return 0;
}
```

Run it, click a row in the tree pane, and edit a field. Each write is a real
facet setter: prop, dirty bit, scheduled sync, backend apply.

## Driving it from code

The panel is one consumer of a vtable; here is the other. Every verb takes a
`Handle` from the listing.

```cplus
let rows: vec::Vec[insp::Node] = itree::describe();   // flat, parent-indexed
let h: insp::Handle = /* the row whose key is "card" */;

let _a: insp::Outcome = itree::set(h, "padding", insp::Value::Num(40.0f64));
let _b: insp::Outcome = itree::set(h, "text", insp::Value::Str(text::from_str("hi")));
let _c: insp::Outcome = itree::reset(h, "padding");   // back to what the APP declared
```

Reading a node gives three separate lists — `declared`, `computed`, `native`:

```cplus
match itree::inspect(h) {
    option::Option[insp::Detail]::Some(d) => { /* d.declared, d.computed, d.native */ }
    option::Option[insp::Detail]::None => { /* stale — describe again */ }
}
```

## Adding and removing nodes

```cplus
// Append a label under `h`. Past-the-end clamps, so this is "add child".
let _i: insp::Outcome = itree::insert(h, 99 as usize,
                                      insp::Spec::of("label", "made", "hello"));

let _r: insp::Outcome = itree::remove(other);   // held, not freed
let _u: insp::Outcome = itree::undo_remove();   // back in the same slot
```

`insp::Spec::of(element, key, text)` — `element` is one of
`itree::maker_names()`, spelled the way `facet/elements` spells the function.

## Over the socket

If your app serves an agent, it is already inspectable — the fourteen verbs
come with the surface, and there is no second call:

```cplus
agent::enable();
runtime::agent_mcp("app");        // an ID; the platform derives the address
```

Then `describe_tree`, `set`, `insert`, … See
[wire.md](wire.md).

## Day-one rules

- **Install the platform module** before serving over a socket. Without it a
  write from the server's thread trips facet's main-thread assertion.
- **Any removal invalidates every `Handle`** — describe again. Inserts do not.
- **Tier 3 needs a key.** `text`, `title` and `on` go through generated typed
  handles, which resolve by key; an unkeyed node answers `Unsupported`.
- **`reset` restores what the app declared**, not the type default.
- Edits are **volatile**. `snippet_for` and the journal give you the C+ to
  paste; nothing is replayed at startup.
