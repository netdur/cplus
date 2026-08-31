# Tutorial

Quick path: check the permission, schedule something, cancel it. Deeper
rationale and gotchas live in [guide.md](guide.md); signatures in
[ref.md](ref.md).

## Setup

See the dependency block in the [README](../README.md) — it is longer than one
line because the resolver validates against one flat set from your own manifest.

```cplus
import "notifications/notifications" as notifications;
import "permissions/permissions" as permissions;
```

## Ask before you post

Both platforms accept a notification from an app with no permission and never
show it. This package refuses instead, so the permission comes first.

```cplus
fn answered(name: str, s: permissions::State, ctx: *u8) {
    if s == permissions::State::Granted { schedule_the_thing(); }
    return;
}

if permissions::can_prompt(permissions::NOTIFICATIONS) {
    let _s: status::Status = permissions::request(permissions::NOTIFICATIONS,
                                                  on_answer: answered);
}
```

## Post one now

```cplus
let o: notifications::Outcome = notifications::schedule(
    notifications::Notification::new("hello", "Hello", body: "from C+"));
```

## Schedule one for later

```cplus
notifications::When::Now              // as soon as the platform will take it
notifications::When::After(300.0f64)  // seconds from now
notifications::When::At(unix_seconds) // an absolute instant
```

```cplus
let _o: notifications::Outcome = notifications::schedule(
    notifications::Notification::new("tea", "Tea is ready",
                                     when: notifications::When::After(180.0f64)));
```

## Cancel

```cplus
let _c: notifications::Outcome = notifications::cancel("tea");
let _a: notifications::Outcome = notifications::cancel_all();   // pending
let _s: notifications::Outcome = notifications::clear_shown();  // already shown
```

## Ask what is pending

```cplus
var ids: vec::Vec[str] = vec::new::[str]();
let _p: notifications::Outcome = notifications::pending({ #addr_of(ids) as *vec::Vec[str] });
```

It appends to a Vec you own, so you can accumulate across calls.

## Read the outcome

```cplus
notifications::Outcome::Ok            // accepted
notifications::Outcome::InvalidInput  // empty id or empty title — your bug
notifications::Outcome::NotPermitted  // ask for the permission
notifications::Outcome::Unsupported   // no backend here, or not a bundle
notifications::Outcome::Failed        // the platform refused
```

`NotPermitted` and `Unsupported` are deliberately different: one is answered by
asking, the other by hiding the feature.

## Day-one rules

- **The id is yours and it is the handle.** Scheduling twice with one id
  *replaces* rather than stacking — which is what an "N unread" notification
  wants. It is also what `cancel` takes.
- **A title is required.** An empty one shows as a blank row on both platforms,
  so it is refused.
- **On Apple you need a bundle.** A bare `cpc build` binary has no bundle
  identifier and `UNUserNotificationCenter` refuses it; every verb answers
  `Unsupported`. `examples/notifications_demo/bundle.sh` is the smallest thing
  that works.
- **On Android a scheduled notification does not survive the app being
  killed.** See [guide.md](guide.md#deferred-delivery-on-android).
- **Tapping does nothing yet.** The payload is attached and routing is the next
  tier.
