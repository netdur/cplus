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
native APIs. `agent_mcp` serializes JSON-RPC over that vtable for external
agents; `agent_inapp` exposes typed calls over it for embedded assistants.

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

## Auth gate — capabilities, not yes/no

Policy is a **function pointer** `fn(Request) -> Grant`. `deny_all()` grants
nothing; `serve(policy)` arms the surface. The gate never blocks, prompts, or
does I/O.

A `Grant` is a bitset of seven capabilities:

| Capability | Opens |
|---|---|
| `cap_read` | see structure, read ordinary values |
| `cap_act` | click, type, invoke — on ordinary nodes |
| `cap_read_protected` / `cap_act_protected` | a node declared **Protected** |
| `cap_read_private` / `cap_act_private` | a node declared **Private** |
| `cap_edit_tree` | change the UI's **structure** (the inspector's verbs) |

Reads and actions are independent of each other and **monotone within a tier**:
acting on a Protected node costs `act` **and** `act_protected`, so no grant can
touch a protected field while unable to touch an ordinary one. `cap_edit_tree`
is implied by nothing — `act` does what a person could do through the interface
as built; `edit_tree` rebuilds the interface.

Convenience grants: `nothing()`, `reader()`, `operator()`, `protected_reader()`,
`protected_operator()`. **None of them reaches Private.** Getting there takes
`.with(cap_read_private())` on purpose — see the tiers below.

`Request` carries `channel` (`InApp` / `External`) and an opaque `token: str` —
whatever credential the transport received. The core never reads the token; a
JWT middleware verifies the signature in its own code and returns the Grant the
claims describe. An empty grant means "not an agent of this application", and a
transport refuses the connection rather than answering every verb.

## Sensitivity tiers

What an application declares about a node's **content**, checked against what
the caller's grant carries. Declared per node and **inherited downward**,
strictest wins — so marking a payment box covers every field, label and summary
line inside it. (The leak is never the field you remembered; it is the "Card
ending 4242" label beside it.)

| Tier | In the tree | Value costs | Refusal |
|---|---|---|---|
| `tier_open` | yes | `read` / `act` | — |
| `tier_protected` | **yes, named** | `+ read_protected` / `act_protected` | `NeedsGrant` |
| `tier_private` | **yes, named** | `+ read_private` / `act_private` | `Forbidden` |
| excluded (`set_excluded`) | no | — | `NotExposed` |

Both tiers stay **visible**: an assistant can say "the card number is still
blank" because it knows the field is there and what it is called. That is the
whole difference between a tier and simply not exposing the node.

**Protected vs Private is about which grants you mint.** Protected is the card
number: a grant exists, and an app that wants autofill mints one after the user
approves. Private is the CVC: you declare the tier and never mint the bit.

`is_agent_actionable` is the widget's ceiling (enabled, not observe-only) and is
grant-free; `can_act(grant, node)` and `can_read_value(grant, node)` are the
per-caller answers, and `describe(grant)` fills `NodeView.actionable` /
`readable` from them. A registry is built once and answers many grants, so
nothing per-caller is cached in it.

`set_content_name` is the door a backend walk uses for the widget's-own-content
fallback (a button's title, a field's string) and it is **refused at any
non-Open tier**. `set_name` — the developer's accessibility label — stays open;
"Card number" is not a card number.

## Surface outcomes

| Outcome | Meaning |
|---|---|
| `Allowed` | backend may execute |
| `NotFound` | bad / unknown node |
| `NotExposed` | exists but outside agent tree |
| `NotActionable` | observe-only or disabled |
| `VersionConflict` | text write base_version stale |
| `Stale` | backing widget gone (re-open surface) |
| `NeedsGrant` | a wider grant would open this — **ask the user, then retry** |
| `Forbidden` | Private tier, or no `edit_tree` — **stop, do not retry** |

`NeedsGrant` and `Forbidden` are distinct on purpose. An agent told the wrong
one either nags for a permission that is never coming, or abandons work one
approval would have unblocked. Neither is `NotActionable` ("not right now" — an
agent retries that).

The authorize family:

| Function | For |
|---|---|
| `authorize_read(reg, grant, node)` | structure: describe, scroll_to |
| `authorize_value_read(reg, grant, node)` | **content**: get_text, read_runs |
| `authorize_action(reg, grant, node)` | click, set_text, set_caret |
| `authorize_text_write(reg, grant, versions, node, base)` | the above plus optimistic concurrency |
| `authorize_tree_edit(grant)` | structural mutation |

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
- **Any new verb that reads a node's content goes through
  `authorize_value_read`**, not `authorize_read`. `read_runs` shipped on the
  wrong one: attribute runs report a value's length, which on a card field is a
  leak. A verb returning pixels would need to black out non-Open frames.
- A secure text field (`NSSecureTextField`, or a plain field with a secure cell)
  defaults to **Private** with no opt-in. An explicit pin overrides in both
  directions.

## Typical app wiring

1. Build UI; register nodes with stable ids / roles.  
2. Supply a policy to `agent_inapp::open` and/or the external server.
3. Backend walks native tree → registry + events.  
4. Assistant or MCP calls authorize → execute.  
