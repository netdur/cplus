# Tutorial

Dispatch one JSON-RPC line and serve over UDS. Details: [guide.md](guide.md).
API: [ref.md](ref.md).

## Setup

```toml
[dependencies]
agent_mcp = "*"
agent_core = "*"
json = "*"
```

You also need a **backend** package that implements `agent_core::backend::Backend`
(e.g. `agent_appkit`).

```cplus
import "agent_mcp/agent_mcp" as mcp;
import "agent_core/auth" as auth;
import "agent_core/events" as events;
import "agent_core/backend" as backend;
import "stdlib/text" as text;
```

## Consent

External MCP traffic always hits the gate first:

```cplus
fn allow_external(req: auth::Request) -> auth::Decision {
    return match req.channel {
        auth::Channel::External => auth::Decision::Allow,
        auth::Channel::InApp => auth::Decision::Allow,
    };
}
// deny_all() → every request returns consent denied (-32001)
```

## Handle one request line

```cplus
var sub: events::Subscriber =
    events::subscriber(events::everything(), 64);
let gate: auth::AuthGate = auth::serve(allow_external);
let resp: text::Text = mcp::handle_request(
    surf,   // *u8 surface
    vt,     // Backend vtable
    sub,
    gate,
    "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"describe_ui\",\"params\":{}}"
);
// resp is a JSON-RPC response object (as Text)
```

## Methods agents call

| method | params (typical) | result |
|---|---|---|
| `describe_ui` | `{ "mode": "exposed" \| "full", "prefix": "…" }` | array of nodes |
| `click` | `{ "id": "…" }` | `{ "outcome": "…" }` |
| `hit_test` | `{ "id": "…" }` | what a pointer at its centre would reach |
| `set_caret` | `{ "id", "start", "end" }` | outcome |
| `read_text` | `{ "id": "…" }` | the node's text |
| `read_runs` | `{ "id": "…" }` | the styled runs as drawn |
| `invoke_menu` | `{ "id": "…" }` | outcome |
| `scroll_to` | `{ "id": "…" }` | outcome |
| `set_text` | `{ "id", "value", "base_version" }` | outcome |
| `poll_event` | `{}` | event object or `null` |
| `activity` | `{}` | what this surface has been asked to DO |

## Serve UDS (blocking)

```cplus
// path e.g. "/tmp/cplus-agent-<pid>.sock" — keep ≤ 100 bytes
let rc: i32 = mcp::serve_uds(surf, vt, sub, allow_external, path);
// 0 ok exit; negative = socket/bind/listen setup error
```

Or accept yourself and call `serve_fd(...)` per connection.

## Day-one rules

- Gate **Reject** → no surface touch; JSON-RPC error `consent denied`.
- Protocol is **backend-neutral** — only the vtable knows AppKit/GTK/Win32.
- Transport is **newline-delimited** JSON-RPC 2.0 over a stream socket.
- This module includes UDS helpers; pure `handle_request` needs no sockets.
