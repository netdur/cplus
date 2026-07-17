# agent_mcp

**MCP-style bridge** for the agent surface: JSON-RPC over a backend-neutral
`agent_core::Backend` vtable, gated by `AuthGate` on the **External** channel.
Optional Unix-domain socket transport (newline-delimited JSON).

```toml
[dependencies]
agent_mcp = "*"
# pulls agent_core, json, stdlib
```

```cplus
import "agent_mcp/agent_mcp" as mcp;
import "agent_core/auth" as auth;
import "agent_core/events" as events;
import "agent_core/backend" as backend;

// vt = backend.mcp_backend() from agent_appkit / agent_win32 / agent_gtk
// surf = #addr_of(surface) as *u8
let line: str = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"describe_ui\",\"params\":{}}";
let resp: text::Text = mcp::handle_request(surf, vt, sub, gate, line);
```

## Docs

- [docs/tutorial.md](docs/tutorial.md) — dispatch + UDS sketch
- [docs/guide.md](docs/guide.md) — protocol, consent, transport
- [docs/ref.md](docs/ref.md) — API + methods

## Tests

Unit tests live with the package source when present:

```
cd vendor/agent_mcp && cpc test
```
