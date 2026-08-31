# Reference

Manual for the `notifications` package. Signatures and behavior only — no
tutorials. Import:

```cplus
import "notifications/notifications" as notifications;
```

---

## Conventions

| Item | Definition |
|---|---|
| Id | caller-chosen, stable, non-empty. Both platforms key **replacement** on it |
| Title | required; an empty one is refused |
| Permission | read before every `schedule`; `Granted` and `Limited` allow, everything else answers `NotPermitted` |
| Return | every verb answers `Outcome`; there are no callbacks |
| Bundle | on Apple, a process with no bundle identifier has no notification centre and every verb answers `Unsupported` |

---

## `Outcome`

```cplus
enum Outcome { Ok, InvalidInput, NotPermitted, Unsupported, Failed }
```

| Arm | Meaning |
|---|---|
| `Ok` | Accepted. For a deferred notification this means scheduled, not delivered |
| `InvalidInput` | Empty id or empty title. The caller's bug |
| `NotPermitted` | The notification permission is not held. Ask for it |
| `Unsupported` | No backend on this platform, or no bundle on Apple |
| `Failed` | The platform accepted the call and refused the notification |

`NotPermitted` and `Unsupported` never collapse into each other: one is answered
by asking for the permission, the other by hiding the feature.

### `to_code` / `from_code`

```cplus
fn to_code(this) -> i32
fn Outcome::from_code(c: i32) -> Outcome
```

The wire form the seam speaks: `Ok` 0, `InvalidInput` 1, `NotPermitted` 2,
`Unsupported` 3, `Failed` 4. An unrecognised code is `Failed` — a newer backend
answering something this build has never heard of has plainly not succeeded.

---

## `When`

```cplus
enum When { Now, After(f64), At(f64) }
```

| Arm | Meaning |
|---|---|
| `Now` | As soon as the platform will take it. Not a promise of instantaneous — both platforms may coalesce |
| `After(seconds)` | Seconds from now. Below 1 collapses to `Now` |
| `At(unix_seconds)` | An absolute instant. A past one collapses to `Now` rather than being refused |

Repeating triggers are not in this pass.

---

## `Notification`

```cplus
struct Notification {
    id: str,
    title: str,
    body: str,
    when: When,
    payload: str,
    channel: str,
}
```

| Field | Meaning |
|---|---|
| `id` | The handle: `cancel` takes it, and scheduling twice with it replaces |
| `title` | Required |
| `body` | Optional second line |
| `when` | See `When` |
| `payload` | Opaque; handed back to `on_tap`. A `str` rather than a URL, so `"order:1234"` needs no invented scheme |
| `channel` | **Android only.** Importance, sound, vibration. `""` takes the package default, created lazily. Ignored on Apple — an Apple *category* is the action set and becomes its own field when actions land |

### `new`

```cplus
fn Notification::new(id: str, title: str, body: str = "",
                     when: When = When::Now, payload: str = "",
                     channel: str = "") -> Notification
```

Content-taking constructor with the rest defaulted.

---

## Verbs

### `schedule`

```cplus
fn schedule(n: Notification) -> Outcome
```

Post or schedule. Scheduling an id that is already pending **replaces** it.

Reads the notification permission first and answers `NotPermitted` rather than
handing the platform a notification it will accept and never show.

On Android a deferred notification does **not** survive the process being
killed; `Ok` means the schedule was accepted.

### `cancel`

```cplus
fn cancel(id: str) -> Outcome
```

Cancel a pending notification and remove a delivered one with the same id.
`InvalidInput` for an empty id; `Ok` whether or not anything was pending —
cancelling something already delivered or never scheduled is not an error.

### `cancel_all`

```cplus
fn cancel_all() -> Outcome
```

Cancel everything this application scheduled. Does not clear what is already
showing — see `clear_shown`.

### `clear_shown`

```cplus
fn clear_shown() -> Outcome
```

Remove this application's already-delivered notifications from the shade.
Separate from `cancel_all` because they are different questions: one is about
the future, one about the past.

### `pending`

```cplus
fn pending(out: *vec::Vec[str]) -> Outcome
```

Append the ids this package believes are still pending. `InvalidInput` for a
null pointer.

**Appends** to a Vec the caller owns. The answer is this package's own record,
not the platform's — Apple can be asked and Android cannot, so the record is the
only answer available on both. It may outlive a notification the system already
delivered; it will not under-report.

### `on_tap`

```cplus
fn on_tap(f: fn(str, *u8), ctx: *u8 = 0 as *u8) -> Outcome
```

Route notification taps to `f`, which receives the notification's `payload`.
`InvalidInput` for a null handler. A second call **replaces** the first — one
slot, not a list, because a tap is a routing decision and two routers
disagreeing about one payload is a bug.

**Safe to call at any time.** If a tap already happened — including the one that
launched the app from a dead process — `f` is called before this returns.
`facet/app_events` latches the payload, so there is no ordering to get right.

### `off_tap`

```cplus
fn off_tap() -> Outcome
```

Stop routing. The underlying subscription stays: it costs nothing idle, and
removing it would drop the latch replay for a handler installed later.

---

## Package metadata

| Item | Value |
|---|---|
| Import | `notifications/notifications` |
| Dependencies | `stdlib`, `permissions`; `objc` on macOS and iOS; `jni`, `android_view`, `facet`, `flex_layout`, `events` on Android |
| `[link]` frameworks | none — the Apple half `dlopen`s UserNotifications |
| Unit tests | `cd vendor/notifications && cpc test` |
| iOS checks | `tools/run_ios_tests.sh` |
| macOS demo | `examples/notifications_demo` |

## Not in this package

| | Where instead |
|---|---|
| Push token registration | An entitlement + provisioning profile on Apple; Firebase on Android. `plans/notifications.md` §6 |
| Media / transport controls | Sticky + actions covers the shade; the rest is a `MediaSession`. §9 |
| Actions, images, sticky, badges, grouping | Later tiers. §9 has the list |
