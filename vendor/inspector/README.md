# inspector

A developer-only live inspector for a mounted facet tree: read and write any
node's properties, add and remove nodes, in a running application.

It walks **facet's own tree**, not the platform's view hierarchy, so it sees
pure-layout nodes, spans and menu items that have no native view at all — and
one walker serves every backend.

This is **not** the agent surface. `agent_core` is curated and permissioned;
this one sees unexposed nodes and writes properties that are not user
affordances. Nothing here is reachable from the agent surface.

```toml
[dependencies]
inspector = "*"
```

## Embed the panel

```cplus
import "inspector/widget" as panel;
import "inspector/appkit" as iplatform;

struct App { panel: panel::Inspector, }

impl App: component::Component {
    fn build(ref this) -> core::Node {
        var row: core::Builder = core::Builder::new();
        row.add(my_ui());
        row.add(this.panel.build().width(620.0f64));
        return ui::row(row, key: "window").grow(1.0f64);
    }
}

impl App: component::Lifecycle {
    // A nested component's hooks are the host's to run.
    fn on_attach(ref this) { panel::attach(#addr_of(this.panel)); return; }
    fn on_detach(ref this) { panel::detach(#addr_of(this.panel)); return; }
}

fn main() -> i32 {
    iplatform::install();                       // highlight overlay + native rows
    let app: App = App { panel: panel::embedded() };
    ...
}
```

## Or over the agent's MCP socket

One `arm` call publishes an `inspector.` namespace inside the server an app is
already running. Arming is explicit and is the gate — see
[docs/wire.md](docs/wire.md).

```cplus
imcp::arm(itree::local_backend());
```

## Modules

| Module | |
|---|---|
| `inspector/inspector` | the neutral surface — `Handle`, `Value`, `Spec`, `Prop`, `Outcome`, `Backend` |
| `inspector/tree` | the facet-tree walker, the typed dispatch, the structural verbs |
| `inspector/widget` | the embeddable panel, written against the vtable |
| `inspector/appkit` | highlight overlay, native rows, the UI-thread hop — the PLATFORM half, and it resolves per platform: `appkit.cplus` on macOS, `serve_ios.cplus` on iOS, `serve_android.cplus` on Android |
| `inspector/serve` | `arm()` — one name an app calls on any platform to add the `inspector.*` verbs to the agent socket it already serves. Binds nothing: `runtime::agent_mcp(id)` decides whether this process serves at all |
| `inspector/mcp` | the `inspector.` namespace for `agent_mcp`'s server |

## Docs

- [docs/tutorial.md](docs/tutorial.md) — get a panel on screen, edit a property.
- [docs/guide.md](docs/guide.md) — the three tiers, handles and staleness, the
  refusals, and the gotchas.
- [docs/ref.md](docs/ref.md) — signatures.
- [docs/wire.md](docs/wire.md) — the `inspector.` JSON-RPC namespace.
- [docs/design.md](docs/design.md) — why it is shaped this way.

## Tests

Unit, e2e and negative tests live in `src/test_main.cplus`; the facet tree is
facet's own state, so the walker, the dispatch, the ledger, the structural verbs
and every refusal are exercisable headlessly.

```
cd vendor/inspector && ../../target/release/cpc test
```

`examples/inspector_probe` is the manual-test app for the things a test cannot
assert — that an edit *feels* live.
