# Facet in-app agent spike

This spike demonstrates an assistant embedded inside a Facet application
seeing and controlling the live app without connecting to its external MCP
socket.

```text
Facet keyed UI
      │ attach live window
      ▼
agent_appkit::Surface ── agent_core::Backend
      │                         │
      ├── external ─ agent_mcp ─┤ checks Channel::External
      │                         │
      └── embedded ─ agent_inapp┘ checks Channel::InApp
                              │
                    provider/model tool loop
```

The executable uses a deterministic stand-in for the model loop. Once the
screen mounts it:

1. proves a denied in-app policy cannot click;
2. calls `describe_ui()` and finds the keyed `profile:name` field;
3. calls `set_text("profile:name", "Ada")`;
4. re-describes the live UI and observes `Ada`;
5. calls `click("profile:save")` and proves the real handler fired once.

## Product integration

At startup, call `agent::enable()` before `App::run`. In the assistant
controller, create a session with a policy that explicitly admits the in-app
channel:

```cplus
let session: inapp::Session = agent::in_app(inapp_only);
```

Expose business controls with stable `key: "..."` values and clear visible
labels. Leave the chat panel's own controls unkeyed unless the assistant has a
reason to operate them.

The provider loop is conventional tool use:

1. send the user's message and a compact serialization of
   `session.describe_ui()`;
2. advertise five tools: `describe_ui`, `click(id)`,
   `set_text(id,value,base_version)`, `scroll_to(id)`, and `hit_test(id)`;
3. execute returned calls through the session and return the exact `Outcome`;
4. on navigation or `stale`, describe again; stop when the model returns its
   user-facing answer.

Do provider networking/model inference off the main thread. The AppKit backend
already marshals UI reads and writes to the main thread. Do not put API keys in
the app binary; use a user-supplied credential, a backend-issued short-lived
token, or an on-device model.

## Build and run

Like the other standalone recipes, symlink the checkout's packages into a
local `vendor/` directory, including `agent_inapp`, then run:

```bash
cpc build
./target/debug/facet_inapp_agent
```

Expected output:

```text
facet_inapp_agent: PASS (describe -> set_text -> describe -> click)
```
