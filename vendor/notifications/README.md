# notifications

Build a notification, schedule it, cancel it.

```toml
[dependencies]
notifications = "*"
permissions   = "*"   # this package's own dependency, restated — see below
stdlib        = "*"

[macos.dependencies]
objc = "*"
[ios.dependencies]
objc = "*"

[android.dependencies]
jni          = "*"
android_view = "*"
facet        = "*"
flex_layout  = "*"
events       = "*"
```

The resolver validates every import against ONE flat set taken from **your**
manifest — it does not read a dependency's own — so a package's transitive deps
are named again here. Miss one and the link says which symbol.

```cplus
import "notifications/notifications" as notifications;
```

## Common case

```cplus
let o: notifications::Outcome = notifications::schedule(
    notifications::Notification::new("reminder:1", "Stand up",
                                     body: "You have been sitting for an hour.",
                                     when: notifications::When::After(3600.0f64)));

if o == notifications::Outcome::NotPermitted {
    // Ask, then schedule again. This package refuses rather than handing the
    // platform a notification it will accept and never show.
}
```

## Three things that will bite you

- **Ask for the permission first.** Both platforms accept a notification from an
  app without permission and silently never show it. This package returns
  `NotPermitted` instead — which is why it depends on `permissions`.
- **On Apple, nothing appears while your app is in front** unless a delegate
  says to present it. This package installs one. Without it the code is correct
  and nothing happens, which is the most common "notifications don't work"
  report there is.
- **Android needs one manifest line for action buttons:**
  `<receiver android:name="cplus.facet.FacetNotificationReceiver"
  android:exported="false" />`. Without it buttons quietly do nothing.
- **Register `on_tap` whenever you like.** A tap that launched the app fired
  before anything subscribed; the latch replays it at registration.
- **On Android a missing channel drops the notification silently.** This package
  creates one before every post, so you never meet it — but it is why `channel`
  is never passed through unchecked.

## Docs

| Need | File |
|---|---|
| Use it in minutes | [docs/tutorial.md](docs/tutorial.md) |
| How / why / gotchas | [docs/guide.md](docs/guide.md) |
| Exact signatures | [docs/ref.md](docs/ref.md) |

## Platforms

| Platform | Local scheduling | Notes |
|---|---|---|
| iOS | yes | `UNUserNotificationCenter`; needs a bundle |
| macOS | yes | same file as iOS; needs a bundle, and the demo shows why |
| Android | yes | channels, `NotificationManager`; deferred does **not** survive process death |
| Linux | `Unsupported` | parked — the D-Bus spec has no scheduling concept |

**Not here:** push tokens (an entitlement and a provisioning profile on Apple,
the Firebase pipeline on Android — a remote notification that *arrives* is
handled exactly like a local one), and media/transport-control notifications
(sticky + actions covers the ask; the rest comes from a `MediaSession`).
See `plans/notifications.md` §6 and §9.

**Taps work, cold and warm.** `on_tap` hands back the payload the notification
carried. A tap on a *dead* app launches it and still reaches your handler —
`facet/app_events` latches the payload, so registering late is safe.

## Tests

```sh
cd vendor/notifications && cpc test                 # 174 checks, macOS host
vendor/notifications/tools/run_ios_tests.sh         # 9 checks, iOS simulator
```

The host suite covers the arithmetic, the guards and the record. It cannot
reach the framework: `cpc test` builds a bare binary, and
`UNUserNotificationCenter` refuses a process with no bundle identifier — so the
iOS runner is a bundled app, which is the only configuration where the centre
exists. `examples/notifications_demo` is the macOS one, and it is where a person
sees a notification actually arrive.
