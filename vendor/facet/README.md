# facet

Platform-free UI description for C+: a `@facet` `Node` tree, mounted once by a
backend, updated **in place by key** — not reactive, not a virtual DOM.

```toml
[dependencies]
facet = "*"
```

```cplus
import "facet/facet" as facet;
import "facet/runtime" as runtime;

// Component + build-once tree + find("key").set_text(...)
// See docs/tutorial.md for a full counter.
runtime::run_component(Counter { n: 0 }, title: "Counter",
                       width: 300.0f64, height: 160.0f64);
```

## Docs

| File | Role |
|---|---|
| [docs/tutorial.md](docs/tutorial.md) | Fast path (counter, day-one rules) |
| [docs/guide.md](docs/guide.md) | Model overview + index of deep dives |
| [docs/ref.md](docs/ref.md) | Compact API map |

**Topical deep dives** (authoritative detail — preserved):

| Topic | Doc |
|---|---|
| Components & state | [docs/component-model.md](docs/component-model.md) |
| Keyed updates | [docs/updates.md](docs/updates.md) |
| Bound components (spike) | [docs/bound-components.md](docs/bound-components.md) |
| Lifecycle, `present`, parking | [docs/lifecycle.md](docs/lifecycle.md) |
| App, Screen, nav | [docs/app-screens.md](docs/app-screens.md) |
| Theme & color tiers | [docs/theme.md](docs/theme.md) |
| Services & threading | [docs/services.md](docs/services.md) |
| DSL & widgets | [docs/widgets.md](docs/widgets.md) |
| Backends & host | [docs/backends.md](docs/backends.md) |

## Modules

| import | provides |
|---|---|
| `facet/facet` | Node, DSL, widgets, Component, find/Handle, lifecycle, Chrome/Screen, Color/Style |
| `facet/runtime` | run / run_component / run_screen, App, Window, alert, menus (selects platform backend) |
| `facet/nav` | go / push / pop / quit / arg — screen navigation verbs |
| `facet/agent` | opt-in MCP serving for `app.agent_mcp` |

## Tests

```
cd vendor/facet && cpc test
```
