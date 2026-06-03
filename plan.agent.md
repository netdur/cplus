# C+ Agent Surface Plan (in-app UI introspection + control)

Status: **design discussion, implementation not started.** One file for now; it
will split (core / AppKit backend / MCP bridge) when work begins (§9). This doc
records the shape and the load-bearing decisions so the reasoning survives.

## 1. Motivation: two use cases, one capability

Both of these need the same thing — a semantic, curated, permissioned way for an
AI agent to *see* a live app's UI and *act* on it:

- **In-app assistant.** An app ships a help icon that opens an embedded AI
  assistant (backed by Claude / ChatGPT / Gemini). To actually help the user it
  must see what is on screen and drive the app to resolve the problem. This runs
  **in-process** and uses no socket and no MCP.
- **External agent.** An outside agent attaches to a running app (discovered by
  PID) to introspect and control it, over MCP.

The bet: build the capability **once** as a framework-agnostic core, and expose
it through both consumers. This mirrors the core-vs-consumers split in
[plan.graph.md](plan.graph.md) and the language-vs-library split in the GPU/SIMD
plans.

## 2. Core principle: a decoupled agent surface

A core library — **framework-agnostic, MCP-agnostic** — provides:

- `describe_ui` — a curated UI tree (§4),
- text operations including diff (§4),
- actions: click / scroll / input (§4),
- events: app → agent notifications (§4),
- the `agent-id` registry (§3),
- the **auth gate** every call passes through (§5).

The core **never imports MCP.** MCP is one thin adapter on top; the in-app
assistant is another. That is the decoupling.

```
   set_agent_id tags  +  AppKit view hierarchy
            │
            ▼
   ┌──────────────────────────────────────────┐
   │  Agent surface (core, framework-agnostic) │
   │  describe_ui · text ops · actions ·        │
   │  events · agent-id registry · AUTH gate    │
   └───────────────┬────────────────┬───────────┘
                   │                │
        ┌──────────▼──────┐    ┌────▼──────────────────────┐
        │ In-app assistant│    │ External MCP server        │
        │ (same process)  │    │ (UDS by PID)               │
        │ → Claude/GPT/   │    │ → outside agent attaches   │
        │   Gemini API    │    │                            │
        └─────────────────┘    └────────────────────────────┘
```

## 3. `set_agent_id`: one hook, three jobs

A builder hook on widgets, living in the UI layer (consumed by the core, not by
MCP):

```cplus
let btn = controls::Button::new("Log In")
    .set_agent_id("btn_login")        // the hook
    .set_on_click(handle_login);
```

`set_agent_id` does three things at once:

1. **Stable identity** — a durable handle that actions and text-ops reference,
   surviving UI re-renders (the alternative, raw node indices, races against
   relayout). The developer names the nodes that matter; untagged nodes get
   auto-generated IDs so the tree is still complete, just less addressable.
2. **Exposure** — tagging opts a widget into the agent view; this is the
   "less noisy than a11y" curation (the app decides what an agent sees).
3. **Auth unit** — rules reference it: "allow `input` on `btn_login`".

One annotation, three jobs, and **nothing to do with MCP**.

## 4. The surface (four capabilities)

| Capability | Shape | The load-bearing choice |
| :--- | :--- | :--- |
| a11y-like UI tree | `describe_ui` → pruned, structured tree | **curated, not raw** — only exposed nodes, with role/label/agent-id |
| text ops + diff | `get_text(id)` / `diff` / `replace_range(id, …)` | long text is **not** in the tree; the agent pulls it on demand, keeping the tree (and the agent's context) small |
| actions | `click(id)` / `scroll(…)` / `input(id, …)` | reference nodes by `agent-id`, never by screen coordinates |
| events | app → agent notifications ("download finished") | push, not poll; the agent subscribes |

For the MCP consumer these map onto MCP primitives directly: tree/text → tools
or resources, actions → tools, events → server→client notifications.

## 5. Auth: programmable middleware, per-channel policy

Authorization is **mandatory and programmable** — which is why enabling the
surface is an API call that takes a policy, not a flag (§8). The runtime calls
the developer's policy before executing any tool; the policy returns one of:

- **allow**,
- **reject**,
- **ask the user** — via an in-app consent dialog, optionally backed by a
  persistent rules list in settings ("what an agent may access / act on").

Defaults and granularity:

- **Default-deny.** This is a high-trust, outward-facing surface; nothing is
  exposed or actionable until the policy (and/or the user) says so.
- **User-facing rules** are coarse (categories: read-UI / read-text / act).
  **Developer middleware** can refine per-node (per `agent-id`).
- **Policy varies by channel** — the decisive reason auth is supplied code, not
  a fixed switch:
  - **In-app assistant**: the user clicked the help icon and is present, asking
    in real time → consent is implicit and light.
  - **External MCP**: an agent attaches by PID with the user possibly not
    watching → default-deny, explicit consent.
  - Same gate, two policies; the developer wires "in-app = trusted, external =
    ask".

## 6. Consumers

- **In-app assistant** (no MCP): same process, calls the core directly, then the
  app forwards the curated tree/text to whatever LLM API it uses. No socket, no
  PID discovery.
- **External MCP server**: a Unix-domain socket named by PID (e.g.
  `$TMPDIR/cplus-mcp/<pid>.sock`, optionally a small registry file listing live
  PIDs so a tool can enumerate). JSON-RPC 2.0 over the socket — **not stdio**,
  because the running app owns its own stdin/stdout (the stdio default only fits
  a subprocess like the `cpc` graph server, not a live app). Stricter auth than
  the in-app path.

## 7. Load-bearing design points

1. **Stable node identity** — `set_agent_id` plus auto-IDs; must survive
   re-render. The technical core, analogous to the graph's stable symbol IDs.
2. **One mechanism for noise *and* auth** — "what's exposed" and "what's
   allowed" are the same question; `set_agent_id` answers both. Do not build two
   systems.
3. **Main-thread marshaling** — the socket / assistant runs on a background
   thread, but AppKit actions must run on the main run loop. Every
   `click`/`input` hops to the main thread; a naive design races or crashes.
4. **Safety is the spine, not an add-on** — this is the most outward-facing
   feature in the project (an agent driving a user's app). Default-deny,
   local-only UDS with owner-only permissions, dev/debug-build gated, user
   consent, optionally a token.

## 8. Enablement

Not a pure flag, because auth is mandatory:

- a **build flag** *compiles it in* (zero cost when off; not shipped by accident
  in release),
- an **API call** *arms it* with a policy: `agent::serve(policy)` (in-app) /
  the MCP bridge started with the same policy. You cannot turn it on without
  supplying the policy.

## 9. Relationship to other plans, and the future split

- [plan.appkit.md](plan.appkit.md): the AppKit view hierarchy plus `set_agent_id`
  tags are the **UI backend** that feeds the core. The agent surface is a strong
  reason to finish AppKit's ownership/eventing model.
- [plan.graph.md](plan.graph.md): same core-vs-consumers architecture; the
  stable-symbol-ID lesson applies directly to stable node IDs.

When implementation starts, split this file into:

- **`plan.agent-core.md`** — the framework-agnostic surface + auth gate +
  `agent-id` registry.
- **`plan.agent-appkit.md`** — the AppKit backend (tree from NSView hierarchy,
  action routing, main-thread marshaling).
- **`plan.agent-mcp.md`** — the external MCP bridge (UDS by PID, discovery,
  JSON-RPC).

## 10. Open questions

- **Naming** — "agent surface"? `set_agent_id` vs an attribute vs a method.
- **Auth granularity** — exact category set; how rules persist; the consent
  dialog UX.
- **Event subscription** — how an agent subscribes/filters; backpressure.
- **Auto-ID scheme** — stable IDs for untagged nodes across re-renders.
- **Security token** — needed for the external channel? rotation?
- **Non-AppKit backends** — the core is framework-agnostic by design; which
  backend (if any) comes after AppKit.
