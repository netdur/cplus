# notifications_demo

`vendor/notifications` on macOS, in a real `.app` bundle.

```sh
cd examples/notifications_demo && ./bundle.sh && open out/Notifications.app
```

A permission row with an **Ask** button, three schedule buttons (Now, In 5
seconds, In 30 seconds), **Cancel**, **Clear all**, and two readouts: the last
outcome and how many notifications the package believes are pending.

## Why it is a bundle

`UNUserNotificationCenter` refuses a process with no bundle identifier — and it
refuses by **raising**, not by returning nil. `+currentNotificationCenter`
throws, and an unhandled ObjC exception aborts.

That killed this package's own test runner with SIGABRT until the backend grew a
`bundleIdentifier` guard. A bare `cpc build` binary now answers `Unsupported`
honestly instead of dying, and this bundle is the only way past it.

Note what is **not** in the plist: there is no usage-description key.
Notifications is the one Apple domain gated on the prompt alone — unlike camera
or contacts, there is no `NS…UsageDescription` to forget. What is load-bearing
is `CFBundleIdentifier`, plus the ad-hoc signature so TCC remembers the answer
across rebuilds.

## What to watch, in order

1. **Press Ask first.** Until the permission row says Granted, every schedule
   button answers `NotPermitted` and posts nothing. That is this package
   refusing to do what both platforms do silently — accept a notification they
   will never show — and it is why it depends on `permissions` at all.

2. **Press "In 5 seconds" and leave the app in front.** The banner still
   appears. Without the `UNUserNotificationCenterDelegate` this package installs,
   nothing would show while the app is foregrounded and the code would be
   perfectly correct. That is the single most common "notifications don't work"
   report there is, and it is why the delegate exists before the tap tier needs
   it.

3. **Press a schedule button twice.** One notification, updated in place. Both
   platforms key replacement on the id, so the demo reuses one on purpose —
   unique ids would hide it.

4. **Press "In 30 seconds", then Cancel.** Nothing arrives, and the pending
   count returns to zero.

5. **Read the pending count.** It is this package's own record, not the
   platform's. Apple can be asked and Android cannot — `NotificationManager` has
   no listing API — so an answer that works on both has to be the one kept here.
   It can go stale in one direction only: an entry may outlive a notification
   the system already delivered.

## Resetting

```sh
tccutil reset All dev.cplus.notificationsdemo
```

## What this demo does not show

**Taps.** Pressing a delivered notification does nothing yet — routing its
payload back into the app is the next tier, and it needs
`E_NOTIFICATION_TAP` plus the cold-start latch in `facet/app_events`. The
payload is already being attached (`"demo:tapped"`, under one `userInfo` key),
so the tap tier is wiring rather than redesign.

**Push.** Deliberately not this package's — see `plans/notifications.md` §6. A
remote notification that arrives is presented and routed exactly like a local
one; obtaining a token is an entitlement, a provisioning profile and, on
Android, the whole Firebase pipeline.
