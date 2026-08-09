# Reference

Manual for `agent_mcp`.

```cplus
import "agent_mcp/agent_mcp" as mcp;
```

Depends on `agent_core` (`backend`, `auth`, `events`, `surface`, `identity`)
and `json`.

---

## JSON-RPC core

### `dispatch`

```cplus
fn dispatch(
    surf: *u8,
    vt: backend::Backend,
    ref sub: events::Subscriber,
    gate: auth::AuthGate,
    method: str,
    params: json::Value,
    id: f64,
) -> json::Value
```

One already-parsed request. External consent first. Methods:
`describe_ui`, `click`, `scroll_to`, `set_text`, `poll_event`.

### `handle_request`

```cplus
fn handle_request(
    surf: *u8,
    vt: backend::Backend,
    ref sub: events::Subscriber,
    gate: auth::AuthGate,
    line: str,
) -> text::Text
```

Parse JSON-RPC object from `line` → `dispatch` → response `Text`.  
Parse failure → error envelope (-32700).

---

## Serialization helpers (internal style, public)

```cplus
fn role_to_str / outcome_to_str / verb_to_str
fn node_to_json / describe_to_json
fn outcome_result
fn ok_response / err_response
```

Used by dispatch; available if you extend the protocol.

---

## UDS transport

```cplus
fn current_pid() -> i32

fn read_line(fd: i32, buf_ptr: *u8, cap: usize) -> i64
fn write_all(fd: i32, ptr: *u8, len: usize)

fn serve_fd(
    surf: *u8,
    vt: backend::Backend,
    ref sub: events::Subscriber,
    policy: fn(auth::Request) -> auth::Decision,
    fd: i32,
) -> i32

fn serve_uds(
    surf: *u8,
    vt: backend::Backend,
    ref sub: events::Subscriber,
    policy: fn(auth::Request) -> auth::Decision,
    path: str,
) -> i32
```

| `serve_uds` return | meaning |
|---|---|
| `0` | accept loop ended |
| `-1` | socket failed |
| `-2` | bind failed |
| `-3` | listen failed |
| `-4` | path too long (>100) |

`serve_fd` returns `0` after peer EOF.

---

## Wire methods (summary)

| method | params keys | Backend / events |
|---|---|---|
| `describe_ui` | — | `describe` |
| `click` | `id` | `click` |
| `scroll_to` | `id` | `navigate` |
| `set_text` | `id`, `value`, `base_version` | `set_text` |
| `poll_event` | — | `sub.poll()` |

---

## Extension namespace

One server, more than one capability model. An extension registers a method
prefix and a handler; every request whose method starts with that prefix is
routed there instead of to the surface vtable. This module knows nothing about
what it is carrying, and the dependency runs extension → `agent_mcp`, never
back — so nothing here gains a dependency and `agent_core::Backend` never grows
a debug mode to serve a development tool.

| fn | Signature | Notes |
|---|---|---|
| `arm_extension` | `(prefix: str, h: fn(str, json::Value, f64) -> json::Value)` | Opens the namespace. The prefix is copied, so a composed one is safe. |
| `disarm_extension` | `()` | Closes it and clears the handler. |
| `extension_armed` | `() -> bool` | |
| `is_extension_method` | `(method: str) -> bool` | |

**Arming is the gate.** An extension is typically more powerful than the agent
surface, so its presence in the binary must not be enough to expose it: a
process that never arms answers `-32601` — naming the method, so a missing
`arm_extension` does not read as a protocol mismatch. The consent gate runs
**first** and covers the namespace too; arming grants no path around it.

The live inspector is the extension this exists for — see
`vendor/inspector/docs/wire.md`.

---

## Package

| | |
|---|---|
| Name | `agent_mcp` |
| Module | `agent_mcp/agent_mcp` |
| Dependencies | `stdlib`, `json`, `agent_core` |
| Platform notes | UDS helpers target Darwin sockaddr layout |
