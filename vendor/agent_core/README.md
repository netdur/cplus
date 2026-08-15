# agent_core

Framework-agnostic **agent UI surface**: identity tree, exposure/affordance
ceilings, **capability grants and sensitivity tiers**, authorization outcomes,
semantic events, and a backend vtable for MCP bridges.

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
let gate: auth::AuthGate = auth::deny_all();      // grants nothing until serve(policy)

// A node an agent can SEE and name but not read: the card number needs
// read_protected, the CVC needs read_private — a bit an app declares and
// never mints. Both inherit downward, so marking the box is the declaration.
reg.set_tier(payment_box, identity::tier_protected());
reg.set_tier(cvc, identity::tier_private());
```

Backends (AppKit / Win32 / GTK) and the wire protocol live in other packages
(`agent_appkit`, `agent_mcp`, …) that depend on this one. Embedded assistants
use `agent_inapp`, which calls the same backend vtable without a transport and
carries a `Grant` like every other caller.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — layers, id rules, events, outcomes
- [docs/ref.md](docs/ref.md) — API catalog

## Tests

```
cd vendor/agent_core && cpc test
```
