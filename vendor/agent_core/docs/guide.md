# Guide

How the agent surface core is layered and what each module owns. Tutorial:
[tutorial.md](tutorial.md). API: [ref.md](ref.md).

## Architecture

```
identity   — agent-id tree, roles, exposure, affordance ceiling, describe
auth       — AuthGate: allow/reject External vs InApp (no prompts)
surface    — Outcome authorization + TextVersions for concurrent text
events     — curated verbs, bubbling subscriptions, bounded queues
backend    — Backend vtable + UiNode/Rect for MCP (fn-pointer polymorphism)
agent_core — root re-exports convenience constructors
```

GUI packages (`agent_appkit`, `agent_win32`, `agent_gtk`) build a live
`Surface`, fill a `Backend` vtable, and enforce `Outcome` before calling
native APIs. `agent_mcp` serializes JSON-RPC over that vtable.

## Identity tree

- Root is always **NodeId 0**, id **`"app"`**, role `Group`.
- `add_child(parent, dev_id, role)` — if `dev_id` is `Some`, that string is
  the stable id; else auto-id **`parent/role#index`**.
- List items: `add_item(..., key)` → auto-id **`parent/key`** (data identity,
  not position).
- Ids are **deterministic** (no clocks/random) so rebuilds match.

### Exposure (layer 1)

- Dev-tagged nodes start **exposed**; auto-id nodes start **hidden** until
  `set_exposed(node, true)` (or walk rules in backends).
- `describe()` lists the **exposed** tree (unexposed subtrees pruned /
  reparented).

### Affordance ceiling (layer 2)

- `enabled` mirrors the widget; `set_observe_only` clears actionable.
- `is_agent_actionable` = enabled and not observe-only.
- `authorize_action` requires valid + exposed + actionable.
- `authorize_read` requires valid + exposed only (reads/scroll allowed on
  observe-only).

### Roles

`Button`, `Text`, `Input`, `List`, `Group`, `Window` — small curated set.
Backends may pack role+drive on native handles via `pack_affordance` /
`affordance_key` (platform-neutral encoding).

## Auth gate

- Policy is a **function pointer** `fn(Request) -> Decision`.
- `deny_all()` = reject everything; `serve(policy)` arms the surface.
- `Channel::InApp` vs `External` (MCP). Grant is **all-or-none** for that
  channel — not per-op adjudication.
- Gate never blocks or prompts; consent UI lives in the consumer.

## Surface outcomes

| Outcome | Meaning |
|---|---|
| `Allowed` | backend may execute |
| `NotFound` | bad / unknown node |
| `NotExposed` | exists but outside agent tree |
| `NotActionable` | observe-only or disabled |
| `VersionConflict` | text write base_version stale |
| `Stale` | backing widget gone (re-open surface) |

Text: `get_text` style flows use `version_of`; after a successful write,
`TextVersions.bump(node)`.

## Events

- **Verbs** are curated (Clicked, Changed, Submitted, …, UiChanged) — not a
  raw input firehose.
- Subscription = optional filters AND-ed: **node** (subtree bubble),
  **verb**, **role**.
- `everything()` matches all; narrow with `on_node` / `on_verb` / `on_role`.
- Subscriber queue is **bounded**; `UiChanged` **coalesces**; overflow drops
  **oldest** and increments `dropped_count`. Emitter never blocks.

## Backend vtable

```cplus
struct Backend {
    describe: fn(*u8) -> Vec[UiNode],
    click: fn(*u8, str) -> Outcome,
    set_text: fn(*u8, str, str, u64) -> Outcome,  // id, value, base_version
    navigate: fn(*u8, str) -> Outcome,
}
```

Receiver is type-erased `*u8` (cast to the backend’s `Surface`). Used by
`agent_mcp` so the protocol names no platform.

## Gotchas

- **No interface-bound generics** — vtable is intentional.
- Auto-id stability depends on **sibling order** for path form; use **keys**
  for lists that reorder.
- Registry `lookup` keys are `str` views into owned id storage — don’t free
  the registry while holding external aliases incorrectly.
- Auth does not replace exposure/actionable checks; MCP still returns
  outcomes from the backend after consent.

## Typical app wiring

1. Build UI; register nodes with stable ids / roles.  
2. `serve(policy)` for in-app and/or external.  
3. Backend walks native tree → registry + events.  
4. Assistant or MCP calls authorize → execute.  
