# Guide

MCP bridge: consent, JSON-RPC methods, serialization, UDS transport. Tutorial:
[tutorial.md](tutorial.md). API: [ref.md](ref.md).

## Role in the stack

```
agent (external)  --JSON-RPC-->  agent_mcp  --Backend vtable-->  agent_* surface
                                      ^
                                      AuthGate (External)
                                      events::Subscriber (poll_event)
```

- **Does:** parse/serialize JSON-RPC, enforce External consent, call
  `describe` / `click` / `set_text` / `navigate`, poll semantic events.
- **Does not:** own the widget tree, implement platform UI, or prompt the user
  for consent (policy is a `fn` you supply).

## Consent

Every `dispatch` / `handle_request` path:

1. `gate.check(request(External))`
2. if `Reject` → error code **-32001**, message `"consent denied"`
3. else run the method

`serve_fd` / `serve_uds` re-arm with `auth::serve(policy)` **per request**, so
policy changes can take effect without reconnect (depending on how you write
the policy).

## Protocol

JSON-RPC **2.0** envelopes:

- Success: `{ "jsonrpc":"2.0", "id", "result" }`
- Error: `{ "jsonrpc":"2.0", "id", "error": { "code", "message" } }`

| code | meaning |
|---|---|
| -32700 | parse error |
| -32601 | method not found |
| -32001 | consent denied (app-specific) |

### `describe_ui`

Calls `Backend.describe` → JSON array of nodes:

`id`, `role`, `class`, `frame` `{x,y,w,h}`, `hidden`, `text`, `actionable`,
`parent` (index or null).

### `click` / `scroll_to`

`params.id` (string) → `click` / `navigate` → `{ "outcome": "<wire>" }`.

Outcomes: `allowed`, `not_found`, `not_exposed`, `not_actionable`, `stale`,
`version_conflict`.

### `set_text`

`params.id`, `params.value`, `params.base_version` (number → u64).  
Optimistic concurrency: mismatch → `version_conflict` from the backend.

### `poll_event`

Non-blocking: next `events::Event` as `{ "source", "verb" }` or JSON `null`.
Verbs are snake_case wire strings (`clicked`, `ui_changed`, …).

## Transport (UDS)

- Stream Unix socket, **one JSON object per line** (request and response).
- `current_pid()` for path conventions like `/tmp/cplus-agent-<pid>.sock`.
- `serve_uds` binds, listens, accepts **sequentially**, serves each with
  `serve_fd`, unlinks path on exit. Path length ≤ **100** bytes (`sun_path`).
- Darwin-oriented sockaddr layout in the implementation; verify on Linux if
  you port paths.

`handle_request` stays pure (string in/out) for tests or non-UDS transports.

## Backend requirements

The `Backend` must return core `Outcome` values after applying
`authorize_*` (or equivalent) against the live registry. MCP does not
re-check exposure — it trusts the backend’s outcome after consent.

## Gotchas

- **No method** → -32601; bad JSON → -32700 with id `0`.
- **Missing params** may become empty strings / 0.0 — backends should treat
  empty id as not found.
- **Line buffer** is 8 KiB in `serve_fd`; longer lines truncate.
- **Blocking serve** — run off the UI thread if the UI must stay responsive.
- Comment in source mentions a separate `serve.cplus`; transport lives in
  **this** module today.

## Typical product flow

1. App builds UI + registry; installs event subscriber.  
2. User grants external access → policy returns Allow for External.  
3. Background task: `serve_uds(..., policy, path)`.  
4. Agent connects, `describe_ui`, acts, `poll_event`.  
