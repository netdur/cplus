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
`describe_ui`, `click`, `hit_test`, `set_caret`, `read_text`, `read_runs`,
`invoke_menu`, `scroll_to`, `set_text`, `poll_event`, `activity`.

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

## The address convention

The app names itself; these turn that into somewhere to listen. Both are keyed
on the pid, which is the join key between the socket namespace and the process
table — so a launcher holding only the pid it spawned computes the address, and
a client globbing `/tmp` enumerates every live facet app. There is no
environment variable.

```cplus
fn valid_id(id: str) -> bool
fn uds_path(id: str, pid: i32) -> text::Text      // /tmp/mcp-<id>-<pid>.socket
fn loopback_port(pid: i32) -> u16                 // 9000 + pid % 1000
fn current_pid() -> i32
fn pid_is_live(pid: i32) -> bool                  // kill(pid, 0)
```

`valid_id` refuses `""`, anything over 60 bytes, and anything containing `/` or
NUL. A caller passing an old-style path means to name an address, and this
parameter is a NAME.

### Descriptor

```cplus
fn descriptor_path(id: str, pid: i32) -> text::Text   // /tmp/mcp-<id>-<pid>.json
fn write_descriptor(id: str, pid: i32, transport: str, address: str) -> bool
fn unlink_descriptor()
fn sweep_stale() -> usize
```

The descriptor records what was actually BOUND — the address is derivable, so
this is confirmation rather than discovery:

```json
{"id":"myapp","pid":8161,"transport":"http",
 "address":"http://127.0.0.1:9161/","protocol":"2024-11-05"}
```

Written 0600. `sweep_stale` removes sockets and descriptors whose pid is no
longer live; a path carries the pid so a name is never reused, and the `atexit`
hook covers a normal quit but not a kill.

---

## Identity and refusals

```cplus
fn set_server_name(id: str)
fn server_name() -> str
fn current_client() -> str
fn forget_client()
fn set_deny_hint(f: fn() -> str)
```

`server_name` is what `initialize` answers as `serverInfo.name` — the app's id,
so a client that reached a stale address finds out at the handshake instead of
driving the wrong app. Defaults to `"facet-agent-surface"`.

`current_client()` is the caller's self-reported `clientInfo.name`, `""` before
`initialize`. **Not a credential**: a client picks its own name. It is identity
in the sense a From: header is — what makes a consent prompt legible and a
remembered answer specific. Cleared when a connection opens, so a second agent
cannot inherit the first's consent.

`set_deny_hint` supplies the sentence an empty grant is refused with. Only the
policy knows whether a refusal is "not yet" (a prompt is up, retry) or "no", and
a client cannot tell those apart from a bitset. With none installed the message
is the flat `"consent denied"`.

---

## HTTP transport

MCP defines stdio and Streamable HTTP; line-delimited JSON-RPC is neither, which
is why a client otherwise needs a `nc` bridge written for it. This is the other
half: POST a JSON-RPC message, get one back as `application/json`.

```cplus
fn serve_http(
    surf: *u8,
    vt: backend::Backend,
    ref sub: events::Subscriber,
    policy: fn(auth::Request) -> auth::Grant,
    port: u16,
) -> i32

fn serve_http_fd(surf, vt, ref sub, policy, fd: i32) -> i32
```

Same `handle_request` as the socket transports, so a verb cannot exist on one
door and not the other.

**Keep-alive, not `Connection: close`** — the connection is the session. A
client's name arrives in `initialize` and is cleared per connection, so closing
after each request would hand every later one an empty `client`.

| request | answer |
|---|---|
| `POST` with a JSON-RPC body | `200`, `application/json` |
| a notification (no `id`) | `202`, empty body — silence would hang a waiting client |
| anything but `POST` | `405`, in words |
| `Content-Length` over 64 KiB | `413`, and the connection ends (an unread body would desynchronise the next request) |
| empty body | `400` |

SSE is not implemented. Nothing here pushes — `poll_event` stands in for that —
so plain JSON responses are conformant.

---

## UDS transport

```cplus
fn current_pid() -> i32

// The connected client's self-reported `clientInfo.name` from `initialize`,
// or "" before one arrives. NOT a credential: a client picks its own name and
// can pick any name, so this is identity in the sense a From: header is —
// what makes a consent prompt legible and a remembered answer specific, not
// what makes it safe. Use `token` and a policy that checks it for that.
// Cleared when a connection opens, so a second agent cannot inherit the
// first's name and, with it, the first's consent.
fn current_client() -> str

fn read_line(fd: i32, buf_ptr: *u8, cap: usize) -> i64
fn write_all(fd: i32, ptr: *u8, len: usize)

fn serve_fd(
    surf: *u8,
    vt: backend::Backend,
    ref sub: events::Subscriber,
    policy: fn(auth::Request) -> auth::Grant,
    fd: i32,
) -> i32

fn serve_uds(
    surf: *u8,
    vt: backend::Backend,
    ref sub: events::Subscriber,
    policy: fn(auth::Request) -> auth::Grant,
    path: str,
) -> i32
```

The bound socket is **0600** — `bind_uds` sets the umask around the bind so the
mode is atomic, and chmods after.

### Bind, listen and accept, separately

`serve_uds` and `serve_tcp` block in `accept` forever, so a caller can only run
them on a worker whose return value nobody is alive to read — which made every
setup failure silent. Split so an application can bind on its OWN thread, find
out, and say so:

```cplus
fn bind_uds(path: str) -> i32          // bound fd, or a negative code
fn bind_tcp(port: u16) -> i32          // binds AND listens
fn listen_on(fd: i32) -> i32           // 0, or -3
fn bind_reason(code: i32) -> str       // the code as a sentence
fn accept_loop(surf, vt, ref sub, policy, fd: i32,
               unlink_on_exit: bool, http: bool = false) -> i32
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
| `describe_ui` | `mode`, `prefix` | `describe_exposed`, or `describe` for `mode: "full"` |
| `click` | `id` | `click` |
| `hit_test` | `id` | `hit_test` |
| `set_caret` | `id`, `start`, `end` | `set_caret` |
| `read_text` | `id` | `describe` (the node's own text, under its tier) |
| `read_runs` | `id` | `read_runs` |
| `invoke_menu` | `id` | `invoke_menu` |
| `scroll_to` | `id` | `navigate` |
| `set_text` | `id`, `value`, `base_version` | `set_text` |
| `poll_event` | — | `sub.poll()` |
| `activity` | — | none — this module's own record |

`activity` answers what the surface has been asked to DO: every `click`,
`set_text`, `set_caret`, `invoke_menu`, `scroll_to` and `navigate`, in order,
with the client that asked, the node it targeted and the outcome. Reads are not
recorded. Neither is content: a text write is logged as its LENGTH, because an
agent may only write a field it was granted and the value behind a privacy tier
is readable only through `read_text` under that tier's rules — a log that quoted
it would be a way around them.

Bounded at 256 entries, oldest first. `issued` counts every entry ever made and
`held` how many remain, so a reader can tell a log that wrapped from a session
that was quiet. Reading it costs `cap_read`: it says what OTHER clients did.

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
