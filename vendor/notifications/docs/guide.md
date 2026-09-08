# Guide

How the package is meant to be used, why the pieces exist, and the gotchas that
bite. For a fast start see [tutorial.md](tutorial.md); for signatures see
[ref.md](ref.md).

## What this package is, and what it is not

It owns everything from *"a notification exists"* onward: building one,
scheduling it, cancelling it, and routing its tap back into the app.

It does **not** own how a remote one got there. A push token is a device
credential and a round trip to a server you own: an `aps-environment`
entitlement and a provisioning profile on Apple, the whole Firebase AAR pipeline
on Android. None of that is needed to build, show or handle a notification, and
a remote one that arrives travels this package's code path exactly. That is the
promise; `plans/notifications.md` §6 has the reasoning.

It also does not own **media/transport-control** notifications. Stripped of the
`MediaSession`, one of those is a sticky notification with actions — both of
which are features this package is adding. What it cannot give you is the media
area of the shade, the seek bar, album-art colouring or hardware media keys, and
none of those come from the notification. Apple has no media notification at
all; its equivalent is `MPNowPlayingInfoCenter`, a different framework.

## The permission is a hard prerequisite

Neither platform reports an error for a notification posted without permission.
It is accepted and never shown. So `schedule` reads the gate first and answers
`NotPermitted`, and this package depends on `permissions` to have somewhere to
read it from.

**It refuses only on a definite no** — `Denied`, `Blocked`, `Unsupported`.
`Granted`, `Limited` and `Unknown` all proceed.

`Limited` proceeds because a provisional authorisation on iOS delivers quietly
to the notification centre: that is showing, just not loudly, and refusing there
would be this package overruling a choice the person made.

### Gotcha: why `Unknown` proceeds, and please leave it that way

`Unknown` means *not known*, and refusing on ignorance is a bug this package
invented once already.

Apple has **no synchronous read** of the notification permission —
`getNotificationSettingsWithCompletionHandler:` answers through a block — so
`permissions` serves it from a cache that is **cold on the first call of a
process** and refreshes asynchronously behind it. An earlier version of this
gate treated `Unknown` as a refusal, which meant *the first `schedule` of every
run failed for an app whose permission was granted*. Measured with a probe:
`schedule` answered `NotPermitted` and posted nothing, on an authorised app.

The two failure modes are not equal:

| | |
|---|---|
| Refuse when actually allowed | **breaks** something the platform would have done — a bug this package invented |
| Attempt when not allowed | reproduces exactly what the platform does anyway: accepted, never shown |

So `Unknown` attempts. The gate keeps its value for the states that are
definitely refusals, which is where the silent-failure risk actually lives.

**Scheduling still never prompts.** A permission dialog appearing because a
background job set a reminder is a dialog with no context. The application asks
with `permissions::request`; this package only reports.

## When the callback… there isn't one

`schedule` is synchronous and returns an `Outcome`. There is no completion
handler, because there is nothing to wait for: the platform either accepted the
request or did not. Delivery happens later and is the operating system's
business.

## Ids, and why they are yours

The id is caller-chosen rather than returned, because both platforms key
**replacement** on it. Scheduling twice with one id updates in place instead of
stacking a duplicate — which is what a "3 unread messages" notification wants,
and what makes `cancel(id)` possible without this package handing out tokens.

An empty id is refused: a notification nothing can cancel is a leak.

## Gotcha: nothing appears while your app is in front (Apple)

A notification scheduled five seconds out, with the app foregrounded, delivers
to nothing visible — unless `userNotificationCenter:willPresentNotification:`
returns a presentation option. With no delegate the answer is "do not present".

This package installs a `UNUserNotificationCenterDelegate` and answers banner +
sound + list, so you never meet it. It is documented here because it is the
single most common "notifications don't work" report on the platform, and
because if you ever see that symptom in your own code, this is why.

**Badge is deliberately not among the options.** A package that bumped the badge
on every notification would be making your counting decision.

## Gotcha: on macOS it is in Notification Centre, but never on screen

The notification arrives — open Notification Centre and it is there — but no
banner and no sound. Your half of the exchange is green: `schedule` answered
`Ok`, the permission reads `Granted`, the delegate is installed. The decision
not to interrupt was made afterwards, inside macOS, and macOS keeps a record of
it.

**Focus is the first suspect, not the alert style.** A Focus mode delivers
every application that is not on its allowed list straight to Notification
Centre, silently, which is indistinguishable from a broken notification. It is
easy to have one on without knowing: the sign is a small crescent moon in the
menu bar, and a Focus turned on from an iPhone with *Share Across Devices* is on
the Mac too. System Settings > Focus shows Do Not Disturb as "On" but has no
switch for it; it is turned off from Control Center or by clicking that moon. It also
silences Slack and Mail alongside your app, so "other apps show banners" is
usually a memory from before the mode was turned on rather than a comparison
made under it. Measured 2026-08-31 with `examples/notifications_demo`: every
notification it posted resolved to `interruptionSuppression: delay delivery`
under an active mode whose allowed-application list was empty, and so did
every Slack notification in the same hours.

Ask the system rather than guessing:

    vendor/notifications/tools/why_quiet.sh <your-bundle-id> 2h

It reads the unified log and prints the decision usernoted made for each
notification your bundle posted in the window, then your app's own
authorization and add calls beside it.

| Line | Meaning |
|---|---|
| `suppression=none  reason=disabled` | no Focus; if there was still no banner, look at the alert style |
| `suppression=delay delivery  reason=mode configuration type` | a Focus mode silenced it; `mode=` names the mode |
| `Requested authorization [ didGrant: 1 ]`, `Added notification request: [ hasError: 0 ]` | your half worked |

The script is one `log show` predicate on `process == "usernoted"` and
`eventMessage CONTAINS "Resolved event behavior"`; run it by hand for a
different window. Two traps if you do: `log` may be a shell builtin in zsh, so
call `/usr/bin/log`; and `com.apple.ncprefs`, where older write-ups look for
the per-app settings, is not maintained on macOS 26 — a year stale on a Mac
that posts notifications daily — so an app's absence from it means nothing.

**Then the alert style.** An app whose style is `None` in System Settings >
Notifications is also delivered straight to Notification Centre.
`NSUserNotificationAlertStyle` in Info.plist sets the default (`banner` or
`alert`); the person's choice in System Settings wins. *What `sticky` buys*
below says what `alert` does.

Nothing in this package can override either, and it should not: the one
notification that breaks through Focus is Time Sensitive, which on Apple needs
an entitlement the package cannot claim on your behalf.

## Gotcha: a missing channel drops the notification silently (Android)

From API 26 every notification names a channel, and posting to one that was
never created does not throw and does not log — it is simply dropped.

This package creates the channel before every post rather than once at init,
because a caller can invent a channel id at any time and the only safe moment to
create it is just before use. `createNotificationChannel` on an existing id is a
no-op that does not reset the person's own settings, so doing it every time is
safe as well as convenient.

`channel` is **Android-only**. An earlier version of this file called it
"Android channel id / Apple category id", which was wrong and would have shaped
the actions tier badly: an Android channel is importance, sound and vibration —
the per-app list a person toggles in Settings — while an Apple *category* is the
**action set**, registered up front. They are orthogonal, and `category` becomes
its own field when actions land.

## Gotcha: you need a bundle on Apple

`UNUserNotificationCenter` refuses a process with no bundle identifier, and it
refuses by **raising** — `+currentNotificationCenter` throws rather than
returning nil, and an unhandled ObjC exception aborts. That killed this
package's own test runner with SIGABRT until the backend grew a guard.

A bare `cpc build` binary now answers `Unsupported` honestly. To actually post
one you need a `.app`: `examples/notifications_demo/bundle.sh` is the smallest
version, about twenty lines.

Notifications is the one Apple domain gated on the prompt alone — there is no
`NSNotificationsUsageDescription` to forget, unlike camera or contacts.

**Windows needs no bundle, and that is the reason this backend uses balloons.**
`Shell_NotifyIconW` with `NIF_INFO` posts from any process, and on Windows 10
and 11 the shell renders those as real toasts in the notification centre — the
same surface a WinRT toast lands on. The WinRT road would give buttons and
pictures, but it requires an AppUserModelID registered in the registry, which is
an install-time artifact a `cpc build` executable does not have. That is the
same shape as the Apple bundle problem, and balloons are the road with no
install step.

The permission state is read from
`HKCU\...\PushNotifications\ToastEnabled` — `Granted` when it is absent or 1,
`Blocked` when it is 0. It is never `Unsupported`, because there is always a
notification centre here.

## Buttons, and where they are declared

Apple and Android both have action buttons and neither declares them the same
way, which is why this is a verb rather than a field:

| | Apple | Android | Windows |
|---|---|---|---|
| Where declared | a `UNNotificationCategory` set on the centre **up front** | `addAction` per notification | — |
| A notification | names the category | carries the actions | — |
| Registered late | shows **no buttons, no error** | works fine | — |

So register at startup and one call order works on both. That is the single
ordering rule this package cannot paper over.

**Windows shows no buttons at all**, and the degradation is deliberately the one
Apple already has for a category registered late: the notification posts, the
text is right, and the buttons are simply absent. A `Shell_NotifyIconW` balloon
has nowhere to put them. The road to buttons there is the WinRT toast stack —
`ToastNotificationManager` and an AppUserModelID registered in the registry —
which is a different pipeline with a real install-time cost, and it is not
walked for a feature nothing has asked for yet.

```cplus
notifications::register_action("mail", "reply", "Reply");
notifications::register_action("mail", "archive", "Archive");
// ...then
Notification::new(id, title, category: "mail")
```

The tap tells you which: `on_tap(f)` hands `f` the payload **and** the action id,
with `""` for a tap on the body.

### Gotcha: a button must not open the app

Pressing Play on a player, or Archive on a mail notification, should do the
thing and leave the shade where it is. An Activity `PendingIntent` cannot: it
always brings the app forward and collapses the shade.

So buttons route through a broadcast, and **Android apps need one manifest
line**:

```xml
<receiver android:name="cplus.facet.FacetNotificationReceiver"
          android:exported="false" />
```

Omit it and the buttons quietly do nothing while everything else works — a
better failure than a crash. The receiver also survives a cold process: the
`.so` is loaded by `FacetActivity.onCreate`, so a button pressed while the app
was dead parks its payload and the Activity delivers it on next start.

## Icon buttons and photos

Two Android styles, and a notification has exactly **one** style — so `compact`
and `picture` are mutually exclusive and asking for both is `InvalidInput`
rather than a silent winner.

**`compact`** renders the actions as an icon row instead of text labels.
Android's ordinary template shows action *titles* and ignores their icons;
`Notification.MediaStyle` shows the icons. **Measured: MediaStyle with no
`MediaSession` token renders exactly that** — so icon buttons do not need a
session, which an earlier version of this guide got wrong. What the session
would add is the media area of the shade, the seek bar, album-art colouring and
hardware media keys, none of which come from the notification.

`sticky: true` plus `compact: true` plus three actions is a media player's
notification, minus the system integration.

**`picture`** takes an absolute path and shows the image full-width inside the
notification — `BigPictureStyle` on Android, `UNNotificationAttachment` on
Apple. A path that will not load is dropped and the notification still posts: a
photo that cannot load is a reason to show the text, not to show nothing.

Both are ignored on Apple, which has one action presentation and no choice in
it — except `picture`, which Apple does support as an attachment.

## What `sticky` buys, and what it does not

**The field is Android only**, but the capability is not — and an earlier
version of this guide said macOS had nothing, which was wrong.

| | Persistent notification? | How |
|---|---|---|
| Android | yes | `sticky: true` — per notification |
| **macOS** | **yes** | `NSUserNotificationAlertStyle` = `alert` in your Info.plist — **app-wide** |
| iOS | no | always dismissible; interruption levels and Live Activities answer different questions |
| Windows | no | a balloon fades on its own; the shell owns the timing |

macOS's is a plist key rather than an API, and it applies to every notification
your app posts. That asymmetry is why `sticky` stays a one-platform field
instead of becoming portable: a per-notification bool cannot express an app-wide
setting, and pretending otherwise would have `sticky: false` silently doing
nothing on macOS.

**If your macOS notifications only appear in Notification Centre and never on
screen**, an alert style of `None` is one of two causes, and the less common
one — *Gotcha: on macOS it is in Notification Centre, but never on screen*
above starts with Focus and ends with the command that tells you which.
`examples/notifications_demo/bundle.sh` sets the key; System Settings >
Notifications is where a person overrides it, and their choice wins.

Code signing is not on that list. The demo is ad-hoc signed (`codesign --sign
-`), and the system log shows its notifications authorised, accepted and
delivered under exactly that signature.

**Before blaming the package**, the chain up to macOS is checkable from inside
the app. `permissions::state(of: NOTIFICATIONS)` after a run-loop turn says
whether you are authorised; `schedule`'s `Outcome` says whether the request was
accepted; and the backend's `delegate_installed()` and `presentation_requests()`
say whether macOS asked the presentation delegate and got an answer. All of it
green with nothing on screen means the decision was macOS's — Focus, or the
app's alert style — and `tools/why_quiet.sh` reads that decision back out of
the system log.

On Android it sets `ONGOING_EVENT` and turns auto-cancel off, which are one
decision: a notification meant to stay should not vanish because somebody tapped
it. Measured on an emulator rather than assumed:

| | |
|---|---|
| The shade's **Clear all** | gone entirely while an ongoing notification is present |
| Swipe **right** | resists |
| Swipe **left** | dismisses it |
| The app's own `cancel` / `cancel_all` | removes it, as it should — the app owns its notifications |

The last three are the platform's behaviour, not this package's. **Android 14
made ongoing notifications user-dismissible** for an app that is not running a
foreground service, and the shade implements that as a directional gesture. So
`sticky` is "hard to get rid of", not "impossible". An application that needs a
notification to truly persist needs a foreground service, which is a different
feature and not this package's.

## Deferred delivery on Android and Windows

**A scheduled notification does not survive the process being killed.** This is
the one place the platforms genuinely differ in what they promise.

Apple hands the trigger to the OS: the notification fires whether or not your
app is alive. Android's equivalent is `AlarmManager` with a `PendingIntent`
aimed at a `BroadcastReceiver`, and a receiver has to be declared in the *app's*
`AndroidManifest.xml` and merged into its `classes.dex` — the same arrangement
`FacetActivity` has, one package further out.

That is not built. A deferred notification there rides facet's own scheduler, so
it fires while the app is running and is lost if the process dies first.
`schedule` reports `Ok` either way, because the schedule *was* accepted.

**Windows has the same limitation and no way out of it on this road.** A
deferred notification lives in a table in the process and a thread sleeps until
its moment, so it fires only while the process lives. Nothing on classic Win32
will take the problem: the Task Scheduler is an installed artifact, not a call.
`pending` answers from that table, `cancel` marks the entry dead and the timer's
wake finds nothing to fire, and a STAMP per entry makes replacement correct —
scheduling the same id again bumps the stamp, so the superseded timer wakes,
sees a stamp it does not recognise, and walks away.

If your app needs a reminder that survives a swipe-away, this package does not
give you one yet on either platform.

## What `pending` actually answers

Its own record, not the platform's. Apple can be asked —
`getPendingNotificationRequestsWithCompletionHandler:` — and Android has no
listing API at all, so an answer that exists on both platforms has to be the one
kept here.

It goes stale in one direction only: an entry may outlive a notification the
system already delivered. It will not claim nothing is pending while something
is, which is the direction that would hide work.

## `cancel_all` versus `clear_shown`

Different questions. `cancel_all` is about the future — notifications scheduled
and not yet delivered. `clear_shown` is about the past — what is sitting in the
shade right now. An app clearing a badge on launch wants the second.

On Android they happen to be the same call (`NotificationManager.cancelAll`,
plus this package's own timer list for the first). The facade keeps them apart
because Apple's centre distinguishes pending from delivered, and collapsing them
would lose that.

On Windows `clear_shown` exploits a coupling in the useful direction. The shell
requires balloons to come from a tray icon, and deleting that icon removes its
notifications from the notification centre — so delete-and-re-add *is* "clear
what was shown". The tray icon is the cost of the whole approach: the first
notification adds one, named after the executable and hidden in the overflow by
default, and an `atexit` hook removes it.

## Taps, and why a cold one is the hard case

`on_tap(f)` hands `f` the notification's `payload`. Both platforms deliver, by
completely different roads: a `UNUserNotificationCenterDelegate` method on
Apple, and on Android a `PendingIntent` that starts the Activity —
`onNewIntent` if it is running, the launch intent's extra if the process was
dead.

**The dead case is the one that breaks elsewhere.** A tap on a notification
while the app is not running LAUNCHES it, and the payload arrives around process
start — before any package's `init` has subscribed to anything. Deliver it only
forward and taps work on a warm app and silently do nothing on a cold one, which
only reproduces from a killed process and is among the most-reported bugs in
every mobile framework there is.

So `facet/app_events` **latches** it: the last tap is remembered and handed to a
handler at registration. `on_tap` is therefore safe to call whenever — startup,
a screen's `on_attach`, after a route change — and if a tap already happened
your handler runs before `on_tap` returns.

Edges do not latch. `E_FOREGROUND` replayed at registration would tell a
subscriber the app just came to the front when it has been there for an hour.
The test is whether the event describes a state of the world that is still true.

**One handler, not a list.** A tap is a routing decision, and two routers
disagreeing about one payload is a bug rather than a feature. Fan out on your
own side, where you can say which wins.

## Testing your integration

```sh
cd vendor/notifications && cpc test           # arithmetic, guards, the record
vendor/notifications/tools/run_ios_tests.sh   # framework, centre, gate
```

Two things no harness reaches:

- **A notification actually appearing.** On Android
  `adb shell dumpsys notification --noredact` will tell you what posted, with
  what title, on what channel — which is most of the way there. Apple has no
  equivalent; `simctl` can inject a remote notification but cannot enumerate.
- **The granted path on iOS.** `xcrun simctl privacy` has no notifications
  service — authorisation is not TCC and cannot be written from outside — so
  granting needs a person. `examples/notifications_demo` on a Mac is where that
  happens.
