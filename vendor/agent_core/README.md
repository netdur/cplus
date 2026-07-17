# agent_core

Framework-agnostic **agent UI surface**: identity tree, exposure/affordance
ceilings, authorization outcomes, semantic events, and a backend vtable for
MCP bridges.

```toml
[dependencies]
agent_core = "*"
```

```cplus
import "agent_core/identity" as identity;
import "agent_core/auth" as auth;
import "agent_core/surface" as surface;
import "agent_core/events" as events;

var reg: identity::Registry = identity::new();   // root id "app"
let gate: auth::AuthGate = auth::deny_all();      // closed until serve(policy)
```

Backends (AppKit / Win32 / GTK) and the wire protocol live in other packages
(`agent_appkit`, `agent_mcp`, …) that depend on this one.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — layers, id rules, events, outcomes
- [docs/ref.md](docs/ref.md) — API catalog

## Tests

```
cd vendor/agent_core && cpc test
```
