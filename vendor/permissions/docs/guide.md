# Guide

How the package is meant to be used, why the pieces exist, and the gotchas that
bite. For a fast start see [tutorial.md](tutorial.md); for signatures see
[ref.md](ref.md).

## Why a name and not an enum

A permission is a `str`. The constants exist so a typo is a compile error in the
common case; a bare literal is the escape hatch.

The reason is an asymmetry between the platforms, and it does not generalise to
this repo's other closed vocabularies:

- **Android's permission space is open by construction.**
  `checkSelfPermission` takes a `String`. There are ~180 platform permissions,
  OEMs add their own, and Google adds several per API level. A closed enum would
  make an application wait for a release of *this* package to ask for something
  the platform has supported for years.
- **Apple's is closed by construction.** Each domain is a different class with a
  different selector pair in a different framework, plus its own plist key.
  Adding one is code — but it is a *row* of code, and
  [`register_apple`](ref.md#register_apple) takes one.

So an unrecognised name is passed through verbatim on Android and answers
`Unsupported` on Apple unless a row was registered for it.

It is also not a `struct Permission { name: str }`, which was the second draft.
A `str` is a view and the pending-request table outlives the call that fills it.
Verbs take `str`; anything this package keeps, it keeps as `Text`.

## The six states

| State | Meaning | Your move |
|---|---|---|
| `Unknown` | Never asked, or the platform will not say | `request` — a dialog will appear |
| `Granted` | Full access | Proceed |
| `Limited` | Partial access | Proceed, and offer a widening path if it matters |
| `Denied` | Refused; asking again shows a dialog | Show a rationale, then `request` again |
| `Blocked` | Refused; asking again does **nothing** | `open_settings` |
| `Unsupported` | This build cannot ask | Hide the feature |

### Gotcha: `Denied` vs `Blocked` is the split that earns its keep

It is the only thing that tells you whether to show a rationale or a Settings
button. Collapse them and you ship a button that does nothing, or a prompt that
never appears.

On Android the two are genuinely hard to tell apart —
`denied + shouldShowRationale == false` means *either* "never asked" *or*
"don't ask again". The platform cannot distinguish them, so this package
remembers: a bit per permission, written when an answer arrives, read to decide
between `Unknown` and `Blocked`. Without it, "Open Settings" gets offered to
people who were never asked.

### Gotcha: `Unsupported` is never a refusal

"The person said no" and "this build has no code for that" call for opposite
responses. They are never collapsed — including in the failure paths, where it
would be tempting.

### Where `Limited` comes from

| Platform | Source |
|---|---|
| iOS photos | `PHAuthorizationStatusLimited` — the "selected photos" grant |
| iOS/macOS calendar | `EKAuthorizationStatusWriteOnly` — add events, read none |
| iOS notifications | Provisional and Ephemeral — quiet delivery, no prompt |
| Android location | Coarse granted, fine refused — approximate location |

Android's is the clearest: a person who grants Approximate has location, just
less of it. Reporting `Denied` would send an app to a settings button while it
could already show a map.

## When the callback runs

`request` and `request_many` always call back, exactly once per name — a caller
that got `Ok` and no callback has no way to finish what it was doing.

**When** it runs depends on whether a dialog was needed:

| Situation | Timing | Thread |
|---|---|---|
| Already granted / blocked / unknown name | **Before `request` returns** | the calling thread |
| A real prompt | Later | the main thread |

The synchronous case is deliberate. Always deferring would mean a callback that
never fires in a program with no run loop — which is every `cpc test` binary and
every CLI. Write handlers that tolerate being called re-entrantly, or defer your
own work.

Platform completions arrive on arbitrary queues (`UNUserNotificationCenter`'s
does; `AVCaptureDevice`'s does). Hopping to the main thread is this package's
job, not each caller's.

## The manifest is yours

**`cpc` does not check this.** `cpc new` generates `ios/Info.plist` and
`AndroidManifest.xml`, and that is the end of its involvement — the
usage-description *string* is human-written copy no package can invent. The
failure modes are severe and silent:

- **Apple:** a missing key does **not** affect `state` — a status read answers
  normally. It is `request` that is fatal, and not at the call: `request`
  returns, and a moment later TCC kills the process from a background queue.
  Verified on macOS 26.6:

      Termination Reason: Namespace TCC, Code 0
      This app has crashed because it attempted to access privacy-sensitive
      data without a usage description.

      Thread 1 Crashed:: Dispatch queue: com.apple.root.default-qos
      3  TCC  __TCC_CRASHING_DUE_TO_PRIVACY_VIOLATION__
      4  TCC  __TCCAccessRequest_block_invoke.229

  The asynchrony is what makes it expensive to debug: nothing fails at the call
  site, the callback simply never arrives, and the process is gone. A startup
  that only reads states looks perfectly healthy right up until someone taps a
  button.
- **Android:** a missing `<uses-permission>` makes `checkSelfPermission` answer
  denied forever, with no dialog and no error. It at least surfaces as a `State`
  you can see.

The pairs:

| Name | `Info.plist` key | `<uses-permission>` |
|---|---|---|
| `CAMERA` | `NSCameraUsageDescription` | `android.permission.CAMERA` |
| `MICROPHONE` | `NSMicrophoneUsageDescription` | `android.permission.RECORD_AUDIO` |
| `PHOTOS_READ` | `NSPhotoLibraryUsageDescription` | `android.permission.READ_MEDIA_IMAGES` |
| `PHOTOS_ADD` | `NSPhotoLibraryAddUsageDescription` | — (MediaStore needs none) |
| `CONTACTS` | `NSContactsUsageDescription` | `android.permission.READ_CONTACTS` |
| `CALENDAR` | `NSCalendarsFullAccessUsageDescription` | `android.permission.READ_CALENDAR` |
| `NOTIFICATIONS` | — (Apple gates on the prompt alone) | `android.permission.POST_NOTIFICATIONS` |
| `LOCATION_WHEN_IN_USE` | `NSLocationWhenInUseUsageDescription` | `ACCESS_FINE_LOCATION` **and** `ACCESS_COARSE_LOCATION` |
| `LOCATION_ALWAYS` | `NSLocationAlwaysAndWhenInUseUsageDescription` | `ACCESS_BACKGROUND_LOCATION` |

A name you invented at runtime cannot appear in any list this package ships. Its
failure mode is the same visible `Denied`.

## Location has two traps

**Fine must be requested with coarse.** Asking for `ACCESS_FINE_LOCATION` alone
gives a dialog with no Precise option: the person can only grant approximate,
the fine permission comes back denied, and an app that asked for one thing is
told no while the person believes they said yes. `request(LOCATION_WHEN_IN_USE)`
sends the pair; you only need both lines in your manifest.

**Background is a separate second ask, gated on foreground.** From API 30
Android denies `ACCESS_BACKGROUND_LOCATION` outright, with no dialog, when
foreground is not held. The damage is not the refusal but the *record* — it
would set the "have asked" bit and make every later read `Blocked` for a
permission nobody was ever offered. So `request(LOCATION_ALWAYS)` is refused
before anything is asked or written when foreground is missing, and
`state(of: LOCATION_ALWAYS)` reports `Denied` for the same case, so the two
agree by construction.

## Notifications are the odd domain

Three ways, all on Apple's side:

1. **The status enum is different.** `UNAuthorizationStatus` is NotDetermined 0,
   **Denied 1**, Authorized 2, Provisional 3, Ephemeral 4 — where the other four
   frameworks are NotDetermined 0, Restricted 1, Denied 2, Authorized 3. Running
   it through the shared map would report a refusal as `Blocked` and an
   authorisation as `Denied`.
2. **There is no synchronous read.** `getNotificationSettingsWithCompletionHandler:`
   answers through a block, so `state(of: NOTIFICATIONS)` is a **cache**: the
   first call of a process answers `Unknown` and starts a refresh, later calls
   answer the last refresh, and a completed `request` writes the answer in
   directly. `Unknown` is the right first answer — `can_prompt` is true for it,
   so your first move is to ask, which is what you would have done anyway.
3. **macOS needs a signed bundle.** `UNUserNotificationCenter` errors out for a
   bare binary, so macOS answers `Unsupported`.

On Android, `POST_NOTIFICATIONS` is API 33+. Below that the permission does not
exist and the question is `NotificationManager.areNotificationsEnabled()` — a
setting rather than a grant, so a refusal there is `Blocked`, not `Denied`.

## Adding a domain Apple ships and this package does not

The Apple half is a table of rows, and `register_apple` adds one. Two shapes
cover most domains: a class-method status read plus a class- or instance-method
request with a completion block.

```cplus
let _s: status::Status = backend::register_apple("speech",
    class_name: "SFSpeechRecognizer",
    status_sel: "authorizationStatus", shape: backend::S_ENTITY,
    request_sel: "requestAuthorization:", request_shape: backend::R_CLASS_LEVEL,
    plist_key: "NSSpeechRecognitionUsageDescription",
    framework: "/System/Library/Frameworks/Speech.framework/Speech");
```

A row without a `plist_key` is refused: it would be a process kill waiting for
its first user.

**Location cannot be expressed as a row** — its answer arrives on a
`CLLocationManagerDelegate` rather than from a class method, so a row pointed at
`CLLocationManager` would register cleanly and never call back.

Registering over a built-in name shadows it, which is how you correct a row this
package got wrong on an OS it predates.

## Why no `[link] frameworks`

The Apple half touches AVFoundation, Photos, Contacts, EventKit and
UserNotifications. Naming all five in the manifest would put every one of them
into the launch-time dyld work of any app that wanted one. Every class is
reached by name, so the framework is `dlopen`ed on first use and a domain nobody
asks about costs nothing. These are public system frameworks — permitted and
ship-legal.

If a `-framework` line for one of these ever appears in a build, the launch-cost
claim has stopped being true.

## Threading

Everything is main-thread-shaped. `state` is cheap enough to call while building
a screen. Platform completions are hopped to the main thread before your
callback sees them, so a handler can write the UI tree directly.

## Testing your integration

`state` is drivable from outside the process, which makes most of a permission
flow automatable:

```sh
xcrun simctl privacy <device> grant|revoke|reset <service> <bundle-id>
adb shell pm grant|revoke <pkg> android.permission.CAMERA
```

simctl covers calendar, contacts, location, photos, microphone, motion and
others — **not camera**, so the camera path on the simulator stays hand-verified.

What no harness can do is tap Deny twice, which is the only way to reach
`Blocked`. `playground/permprobe` exists for that.
