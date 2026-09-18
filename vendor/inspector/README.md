# inspector

A developer-only live inspector for a mounted facet tree: read and write any
node's properties, add and remove nodes, in a running application.

It walks **facet's own tree**, not the platform's view hierarchy, so it sees
pure-layout nodes, spans and menu items that have no native view at all — and
one walker serves every backend.

This is a **developer surface**, distinct from the curated, permissioned UI
verbs in `agent_core`. It sees unexposed nodes and writes properties that are
not user affordances. `facet_agent` can publish these inspector verbs over the
same MCP server, guarded by its `edit_tree` capability.

```toml
[dependencies]
inspector = "*"
facet     = "*"
stdlib    = "*"
```

Use `cpc pm add . inspector` to write the full platform-specific closure.

## Embed the panel

```cplus
import "inspector/widget" as panel;
import "facet/component" as component;
import "facet/facet" as core;
import "facet/elements" as ui;

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
    fn on_attach(ref this, why: component::Attach) {
        if why == component::Attach::Mount { panel::attach(#addr_of(this.panel)); }
    }
    fn on_detach(ref this, why: component::Detach) {
        if why == component::Detach::Unmount { panel::detach(#addr_of(this.panel)); }
    }
}
```

The platform half — the highlight overlay, the native rows, the UI-thread hop —
is installed by whoever serves the agent surface (`facet_agent`), so an embedded
panel has nothing to install.

## Or attached to another process

`connect` takes `inapp`, a socket path, a loopback port, or `http://host:port/`
— the scheme picks the transport — and `discover` answers where a named app is
listening.

```cplus
import "stdlib/status" as status;
import "stdlib/text" as text;

async fn attach_remote(st: *panel::Inspector) -> status::Status {
    return await panel::connect(
        st, text::from_str("http://127.0.0.1:9123/"));
}
```

`panel::discover("myapp")` returns live addresses for a named app, newest
first; each returned `Text` can be moved directly into `connect`.

The fourteen verbs are on every agent surface: an app that calls
`runtime::agent_mcp(id)` is inspectable, with no second call. See
[docs/wire.md](docs/wire.md).

## Modules

**This package is the panel.** The verbs, the walker and the platform halves
live where their layer does, which is what lets `agent_mcp` serve the verbs
without depending on a toolkit:

| Module | |
|---|---|
| `inspector/widget` | the embeddable panel, and the host API — written against the vtable |
| `inspector/remote` | the same vtable over a socket, a port, or HTTP |

| Elsewhere | |
|---|---|
| `agent_core/inspect` | the neutral surface — `Handle`, `Value`, `Spec`, `Prop`, `Outcome`, `Backend`, the property vocabulary. Names no toolkit, which is why the verbs could move |
| `agent_mcp/inspect` | the fourteen verbs, published by `agent_mcp` itself when it starts serving |
| `facet_agent/inspect_tree` | the facet-tree walker, the typed dispatch, the structural verbs |
| `facet_agent/inspect_platform` | highlight overlay, native rows, the UI-thread hop — resolved for macOS, Linux, Windows, iOS, and Android |

## Docs

- [docs/tutorial.md](docs/tutorial.md) — get a panel on screen, edit a property.
- [docs/guide.md](docs/guide.md) — the three tiers, handles and staleness, the
  refusals, and the gotchas.
- [docs/ref.md](docs/ref.md) — signatures, including the host API a
  containing application drives the panel with.
- [docs/wire.md](docs/wire.md) — the JSON-RPC verbs on the wire.
- [docs/design.md](docs/design.md) — why it is shaped this way.

## Tests

Unit, e2e and negative tests live in `src/test_main.cplus`; the facet tree is
facet's own state, so the walker, the dispatch, the ledger, the structural verbs
and every refusal are exercisable headlessly.

```
cd vendor/inspector && cpc test
```

`examples/inspector_probe` is the manual-test app for the things a test cannot
assert — that an edit *feels* live.
