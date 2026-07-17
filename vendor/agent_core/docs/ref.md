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

### `NodeView`

`id`, `role`, `name`, `parent`, `enabled`, `actionable` (describe output).

---

## `auth`

```cplus
enum Channel { InApp, External }
enum Decision { Allow, Reject }
struct Request { channel: Channel }
fn request(channel: Channel) -> Request

struct AuthGate { policy: fn(Request) -> Decision }
fn deny_all() -> AuthGate
fn serve(policy: fn(Request) -> Decision) -> AuthGate
fn check(this, req: Request) -> Decision

fn decision_eq / channel_eq
```

---

## `surface`

```cplus
enum Outcome {
    Allowed, NotFound, NotExposed, NotActionable,
    VersionConflict, Stale,
}
fn outcome_eq / outcome_index

fn authorize_action(reg: Registry, node: usize) -> Outcome
fn authorize_read(reg: Registry, node: usize) -> Outcome

struct TextVersions { /* HashMap[usize, u64] */ }
fn new_text_versions() -> TextVersions
fn version_of(this, node: usize) -> u64
fn bump(ref this, node: usize)

fn authorize_text_write(reg, versions, node, base_version: u64) -> Outcome
```

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
