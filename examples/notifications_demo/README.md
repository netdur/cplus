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

## What the buttons do

**One permission, at the top.** Nothing below it works until it says Granted:
both platforms accept a notification from an unauthorised app and silently never
show it, and this package answers `NotPermitted` instead. Press **Ask** first.

### content — the shapes a notification can take

| Button | What it shows |
|---|---|
| **With body** | The ordinary case: a title and a second line |
| **Title only** | A body is optional, a title is not. This is what a "3 new messages" summary looks like |
| **Long body** | More text than a banner shows. How much is revealed is the platform's decision, not this package's |

### timing

| Button | What it shows |
|---|---|
| **In 5s** | **Leave the app in front.** The banner still appears — because this package installs a `UNUserNotificationCenterDelegate` that says to present it. Without that, nothing would show while the app is foregrounded and the code would be perfectly correct. It is the most common "notifications don't work" report there is |
| **In 30s** | Long enough to press **Cancel 30s** before it fires |
| **At +10s** | The other road to the same place: `When::At(instant)` rather than `When::After(seconds)` |

### behaviour

| Button | What it shows |
|---|---|
| **Three at once** | Three distinct ids, so three notifications stack |
| **Replace** | One id posted repeatedly. It updates **in place** and the list does not grow — both platforms key replacement on the id, which is what an "N unread" notification wants. The body counts the presses |
| **Click me** | Posts one carrying a payload. **Click the delivered banner** and the payload appears in the `tapped:` readout. Close the app, click a banner, reopen it — the payload still arrives, because `facet/app_events` latches a tap that happened before anything subscribed |

### manage

**Cancel 30s** takes back a pending one. **Cancel all** is about the future;
**Clear shown** is about the past — what is sitting in Notification Centre right
now. They are different questions, which is why they are different verbs.

**Refresh** re-reads the permission and the pending count.

## The two readouts

`pending:` is this package's own record, not the platform's — Apple can be asked
and Android cannot, so an answer that works on both has to be the one kept here.
`tapped:` is the last payload routed back through `on_tap`.

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
