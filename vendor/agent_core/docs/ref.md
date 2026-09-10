# Reference

Manual for `agent_core`. Import per module:

```cplus
import "agent_core/identity" as identity;
import "agent_core/auth" as auth;
import "agent_core/surface" as surface;
import "agent_core/events" as events;
import "agent_core/backend" as backend;
import "agent_core/agent_core" as agent_core;  // convenience only
```

---

## `identity`

### `Role`

`Button`, `Text`, `Input`, `List`, `Group`, `Window`

```cplus
fn role_tag(r: Role) -> str
fn role_index / role_eq / role_from_index
fn drive_none / drive_click / drive_text / drive_for_role
fn affordance_key() -> *u8
fn affordance_gobject_key() -> *u8
fn pack_affordance / unpack_role / unpack_drive / is_packed_affordance
```

### `Registry`

```cplus
fn new() -> Registry                    // root "app"
fn root(this) -> usize                  // 0
fn count / is_valid
fn add_child(ref this, parent, dev_id: Option[str], role) -> usize
fn add_item(ref this, parent, dev_id, role, key: u64) -> usize
fn resolved_id / role_of / parent_of
fn set_name / name_of
fn set_enabled / is_enabled
fn set_observe_only / is_agent_actionable
fn set_exposed / is_exposed
fn describe(this) -> Vec[NodeView]
fn lookup(this, id: str) -> Option[usize]
fn contains / nearest_exposed_ancestor
```

### tiers

```cplus
fn tier_open() / tier_protected() / tier_private() -> u32
fn stricter_tier(a: u32, b: u32) -> u32
fn read_caps_for_tier(t: u32) -> u32
fn act_caps_for_tier(t: u32) -> u32

// pinned-policy vocabulary, as a backend reads it off a native widget
fn policy_none() / policy_protected() / policy_private() / policy_exclude() -> u32
fn policy_key() -> *u8
fn pack_policy(p: u32) -> *u8
fn unpack_policy(packed: *u8) -> u32
```

On `Registry`:

```cplus
fn set_tier(ref this, node: usize, tier: u32)
fn declared_tier(this, node: usize) -> u32     // this node's own
fn tier_of(this, node: usize) -> u32           // strictest along the ancestor chain
fn is_sensitive(this, node: usize) -> bool
fn set_policy(ref this, node: usize, policy: u32)
fn can_read_value(this, grant: Grant, node: usize) -> bool
fn can_act(this, grant: Grant, node: usize) -> bool
fn describe(this, grant: Grant) -> Vec[NodeView]
fn set_content_name(ref this, node: usize, name: str)   // refused at a non-Open tier
```

### `NodeView`

`id`, `role`, `name`, `parent`, `enabled`, `actionable` (describe output).

---

## `auth`

```cplus
enum Channel { InApp, External }

// capability bits
fn cap_read() / cap_act() -> u32
fn cap_read_protected() / cap_act_protected() -> u32
fn cap_read_private() / cap_act_private() -> u32
fn cap_edit_tree() -> u32

struct Grant { bits: u32 }
fn has(this, caps: u32) -> bool        // conjunction over the WHOLE mask
fn is_empty(this) -> bool
fn with(this, cap: u32) -> Grant
fn without(this, cap: u32) -> Grant

// convenience grants — none of these reaches Private
fn nothing() -> Grant
fn reader() -> Grant                   // read
fn operator() -> Grant                 // read | act
fn protected_reader() -> Grant         // + read_protected
fn protected_operator() -> Grant       // + act_protected

struct Request { channel: Channel, token: str, client: str, method: str }
fn request(channel: Channel) -> Request
fn request_with_token(channel: Channel, token: str,
                      client: str = "", method: str = "") -> Request

struct AuthGate { policy: fn(Request) -> Grant }
fn deny_all() -> AuthGate
fn serve(policy: fn(Request) -> Grant) -> AuthGate
fn check(this, req: Request) -> Grant

fn grant_eq / channel_eq
```

`token` is opaque to the core — a JWT, a session id, whatever the transport
received. Verify it in your own policy and return the Grant its claims describe.

---

## `surface`

```cplus
enum Outcome {
    Allowed, NotFound, NotExposed, NotActionable,
    VersionConflict, Stale,
    NeedsGrant,    // a wider grant opens this — ask the user, then retry
    Forbidden,     // Private tier, or no edit_tree — stop
}
fn outcome_eq / outcome_index
fn refusal_for_tier(t: u32) -> Outcome

fn authorize_read(reg: Registry, grant: Grant, node: usize) -> Outcome
fn authorize_value_read(reg: Registry, grant: Grant, node: usize) -> Outcome
fn authorize_action(reg: Registry, grant: Grant, node: usize) -> Outcome
fn authorize_tree_edit(grant: Grant) -> Outcome

struct TextVersions { /* HashMap[usize, u64] */ }
fn new_text_versions() -> TextVersions
fn version_of(this, node: usize) -> u64
fn bump(ref this, node: usize)

fn authorize_text_write(reg, grant, versions, node, base_version: u64) -> Outcome
```

`authorize_read` answers "may this agent know the node exists" — a gated node
still says yes. `authorize_value_read` is the one door for reading content.

---

## `events`

```cplus
enum Verb {
    Clicked, Changed, Submitted, Selected, Toggled,
    Opened, Closed, Enabled, Disabled, Added, Removed,
    Appeared, Disappeared, WindowOpened, WindowClosed, UiChanged,
}
fn verb_eq / verb_index / is_ui_changed

struct Event { source: usize, verb: Verb }
fn event(source, verb) -> Event

struct Subscription { node?, verb?, role? }  // optional filters, AND
fn everything() -> Subscription
fn on_node / on_verb / on_role (this) -> Subscription

struct Subscriber { /* bounded queue */ }
fn subscriber(filter: Subscription, cap: usize) -> Subscriber
fn offer(ref this, reg: *Registry, ev: Event)
fn poll(ref this) -> Option[Event]
fn pending / dropped_count / has_pending_ui_changed / compact
```

---

## `backend`

```cplus
struct Rect { x, y, w, h: f64 }
struct UiNode {
    id, role, class_name, frame, is_hidden, text, actionable, parent?
}
struct Backend {
    describe: fn(*u8) -> Vec[UiNode],
    click: fn(*u8, str) -> Outcome,
    set_text: fn(*u8, str, str, u64) -> Outcome,
    navigate: fn(*u8, str) -> Outcome,
}
```

---

## `agent_core` root

```cplus
fn new_registry() -> identity::Registry
fn closed_gate() -> auth::AuthGate
fn subscribe_all() -> events::Subscriber
fn new_versions() -> surface::TextVersions
```

---

## Package

| | |
|---|---|
| Name | `agent_core` |
| Dependencies | `stdlib` |
| Tests | `cpc test` |
| Consumers | `agent_mcp`, `agent_*` backends |
