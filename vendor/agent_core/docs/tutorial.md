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

## Declare what an agent may have

A node's existence and its content are separate questions. Both tiers stay
VISIBLE — named, navigable, reported as present — and hand over no value
without the matching capability. Inherited downward, strictest wins, so mark
the container.

```cplus
reg.set_tier(payment_box, identity::tier_protected());  // the card number
reg.set_tier(cvc, identity::tier_private());            // the three digits
reg.set_excluded(assistant_panel, true);                // not app content at all
```

## Auth gate (a capability set)

```cplus
fn my_policy(req: auth::Request) -> auth::Grant {
    // req.token is opaque — verify a JWT here in your own code, then say
    // what its claims amount to.
    return match req.channel {
        auth::Channel::InApp => auth::operator(),
        auth::Channel::External => {
            if req.token == "" { auth::nothing() } else { auth::reader() }
        }
    };
}

let closed: auth::AuthGate = auth::deny_all();       // grants nothing
let served: auth::AuthGate = auth::serve(my_policy);
let g: auth::Grant = served.check(
    auth::request_with_token(auth::Channel::External, tok));
```

`operator()` reads and drives the app and opens no tier. Once the user approves
autofill, mint `auth::protected_operator()`. **Nothing bundled reaches Private**
— that takes `.with(auth::cap_read_private())`, typed on purpose, which is how
"the CVC is never readable" stays true as an app grows.

## Authorize

```cplus
let o: surface::Outcome = surface::authorize_action(reg, grant, btn);
// Allowed | NotFound | NotExposed | NotActionable | NeedsGrant | Forbidden

// reading a node's CONTENT is its own door
let r: surface::Outcome = surface::authorize_value_read(reg, grant, card);
// NeedsGrant  -> ask the user for permission, then retry
// Forbidden   -> stop; this app does not mint the bit

let versions: surface::TextVersions = surface::new_text_versions();
let tw: surface::Outcome =
    surface::authorize_text_write(reg, grant, versions, field_id, base_version);
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
