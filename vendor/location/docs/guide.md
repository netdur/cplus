# Guide

The non-obvious parts, and the traps. Everything here was measured on a
machine, a simulator or an emulator — not read from a header.

## The return value is not the answer

`once` and `updates` tell you whether the request was **accepted**. Whether a
position arrives is decided later.

```cplus
let o = loc::once(on_fix: got);     // Ok = accepted, not "you have a fix"
```

`Denied`, `Disabled` and `Unsupported` are known immediately and come back
here. A request that is accepted and then **fails or times out** reaches the
handler with a negative `accuracy_m`:

```cplus
fn got(f: loc::Fix, ctx: *u8) {
    if !f.is_valid() { /* no position */ return; }
}
```

That is CoreLocation's own convention — it returns an object with a negative
accuracy when it has nothing — and this package keeps it rather than inventing
a second one. **The handler always runs exactly once for a `once`.** Silence
would be indistinguishable from success, which is the failure this shape
exists to prevent.

`Request.timeout_ms` is honoured on Android. Apple ignores it:
`requestLocation` runs its own timer of about ten seconds and offers no way to
set it.

## Unknowns are negative, not zero

Zero is a real reading for all three: a stationary device, at sea level, facing
due north. So absent is spelled `-1`, and there are predicates for it:

```cplus
if f.has_speed()    { … }   // speed_mps    >= 0
if f.has_course()   { … }   // course_deg   >= 0
if f.has_altitude() { … }   // altitude_accuracy_m >= 0
```

`is_valid()` is the one that matters most — an invalid fix carries `(0, 0)`,
which is in the Gulf of Guinea and looks entirely plausible on a map.

## Accuracy is a wish, and the answer can differ

`Accuracy` selects a **power budget**; the accuracy is a consequence.

| tier | what it costs | typical |
|---|---|---|
| `Coarse` | network/cell only, no GPS radio | ~1–3 km |
| `Balanced` | Wi-Fi and cell | ~100 m |
| `Fine` | GPS on | metres outdoors |
| `Navigation` | GPS plus course and speed, fastest rate | metres, visible battery cost |

**A grant can downgrade you silently.** Android 12+ and iOS 14+ let a person
allow *approximate* location; a `Fine` request then succeeds and every fix
arrives coarse, with no error. Read what you got:

```cplus
if STREAM.accuracy().to_code() == loc::Accuracy::Coarse.to_code() { … }
```

On Android that reads the granted permission; on Apple it reads
`accuracyAuthorization`.

## Permission: the models are inverted

| | who asks |
|---|---|
| macOS, iOS | **this package**. `CLLocationManager` is the only door — authorization is a method on the manager and the answer arrives on its delegate — so `once`/`updates` prompt when needed. |
| Android | **the app**, through `permissions`, before touching this package. `requestLocationUpdates` throws `SecurityException` when the grant is missing and never prompts. |

Writing the `permissions` gate on every platform is harmless and keeps one path.

`ACCESS_FINE_LOCATION` must be requested **together with**
`ACCESS_COARSE_LOCATION` from API 31 — asking for fine alone shows a dialog
with the precise option greyed out. The `permissions` package encodes that as a
companion, so `perm::request(perm::LOCATION_WHEN_IN_USE, …)` already does it.

## `Unknown` is not `Denied`

Both Apple platforms resolve authorization **asynchronously**. Reading it at
start-up answers `Unknown` whatever the truth is:

```cplus
loc::permission()          // Unknown at launch, even when granted
STREAM.permission()        // what the delegate actually saw — poll this
```

Folding `Unknown` into `Denied` means never prompting: the app decides it has
already been refused and stops. This is why the two are separate states.

**`+[CLLocationManager authorizationStatus]` is a lie on macOS 26.** The
deprecated *class* method answers `notDetermined` forever — measured against an
app that was authorized and receiving fixes. The instance property is correct,
and even that is only true once `locationManagerDidChangeAuthorization:` has
fired. This package uses the instance property and tracks the delegate.

## Platform traps

### macOS: a bare binary is silent

macOS keys location to a `CFBundleIdentifier`. A binary run from a terminal has
none, so it is not *denied* — it is never asked and never told. `once` returns
`Ok` and no fix ever arrives. It needs a `.app` with
`NSLocationWhenInUseUsageDescription`.

**And ad-hoc signing re-prompts on every build.** `codesign --sign -` mints a
new identity, so TCC treats each build as a new app. Sign with a real
Development identity and the grant persists.
`playground/locationprobe_mac/bundle.sh` does both.

### Android: GPS, not FUSED

This package asks the framework `LocationManager` for `GPS_PROVIDER` on the
fine tiers and `NETWORK_PROVIDER` on `Coarse`. It deliberately does **not** use
`FUSED_PROVIDER`, though API 31+ has it: fused is a *blend*, so it does not map
onto the tiers above, and the emulator never feeds it — `adb emu geo fix`
reaches the GPS provider only. A build preferring fused reports every provider
healthy, the stream running, and delivers nothing.

Play Services' own `FusedLocationProvider` is a separate thing and is also not
used: it arrives as an AAR, and the AAR measurement priced it in megabytes of
dex for a latitude and a longitude.

`getLastKnownLocation` is **not implemented** on Android. `last_known()`
answers `None` there — the seam has nowhere to put a synchronous answer, and
saying so beats faking one.

### Both mobile platforms: the dex is a build artifact

`location.dex` is committed and `#include_bytes`'d with a hard-coded length.
Editing `java/cplus/location/CplusLocation.java` changes **nothing** until
`tools/build_dex.sh` runs. The symptom is a Java method that is plainly there
and never called.

## Lifecycle

A stream is a radio. `Updates` stops on drop, and the reasons say when to stop
it deliberately:

- `Detach::Inactive` — focus only. A dialog, or the other pane of a split
  screen. **Still visible, still working.** Do not stop.
- `Detach::Background` — not visible. Stop.

Unlike a camera, nothing revokes location from a backgrounded app — so this is
a policy choice about battery, not a requirement.
