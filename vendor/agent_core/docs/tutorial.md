# Tutorial

Build a small agent-id tree, check auth and action outcomes. Details:
[guide.md](guide.md). Signatures: [ref.md](ref.md).

## Setup

```toml
[dependencies]
agent_core = "*"
```

```cplus
import "agent_core/identity" as identity;
import "agent_core/auth" as auth;
import "agent_core/surface" as surface;
import "agent_core/events" as events;
import "stdlib/option" as option;
```

## Identity registry

```cplus
var reg: identity::Registry = identity::new();  // NodeId 0, id "app"
let root: usize = reg.root();
let btn: usize = reg.add_child(
    root,
    option::Option[str]::Some("ok"),
    identity::Role::Button
);
// resolved id is the dev id "ok" when provided
// auto id would be "app/button#0" without a dev id

reg.set_name(btn, "OK");
// observe-only: reg.set_observe_only(btn);
// hide from agent: reg.set_exposed(btn, false);
```

## Auth gate (all-or-none per channel)

```cplus
fn allow_inapp(req: auth::Request) -> auth::Decision {
    return match req.channel {
        auth::Channel::InApp => auth::Decision::Allow,
        auth::Channel::External => auth::Decision::Reject,
    };
}

let closed: auth::AuthGate = auth::deny_all();
let open: auth::AuthGate = auth::serve(allow_inapp);
let d: auth::Decision = open.check(auth::request(auth::Channel::External));
```

## Authorize actions

```cplus
let o: surface::Outcome = surface::authorize_action(reg, btn);
// Allowed | NotFound | NotExposed | NotActionable | …

let versions: surface::TextVersions = surface::new_text_versions();
let tw: surface::Outcome =
    surface::authorize_text_write(reg, versions, field_id, base_version);
// on Allowed: backend applies edit, then versions.bump(field_id)
```

## Events

```cplus
var sub: events::Subscriber =
    events::subscriber(events::everything().on_verb(events::Verb::Clicked), 64);
// backend: sub.offer(#addr_of(reg), events::event(node, verb));
match sub.poll() {
    option::Option[events::Event]::Some(ev) => { /* ev.source, ev.verb */ }
    option::Option[events::Event]::None => {}
}
```

## Day-one rules

- Core **does not touch widgets** — backends execute only after `Allowed`.
- Exposure (layer 1) and observe-only (layer 2) are on the **registry**.
- Auth gate is **per agent/channel**, not per node.
- Text edits need a **base_version** (optimistic concurrency).
