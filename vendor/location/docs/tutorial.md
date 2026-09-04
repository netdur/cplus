# Tutorial

A screen that shows where you are.

## 1. Depend on it

```toml
[dependencies]
location    = "*"
permissions = "*"     # Android only needs this; see step 3
```

## 2. Ask for one fix

```cplus
import "stdlib/option" as option;
import "location/location" as loc;

fn got(f: loc::Fix, ctx: *u8) {
    if !f.is_valid() {
        // No position. Indoors, or it gave up. NOT an error to panic about.
        return;
    }
    show("${f.latitude}, ${f.longitude}  ±${f.accuracy_m}m");
}

let outcome = loc::once(on_fix: got);
```

`outcome` says whether the request was **accepted** — `Denied`, `Disabled` and
`Unsupported` are known immediately. Whether a position ever arrives is decided
later and told to `got`.

## 3. Ask for permission first — on Android

Apple prompts on first use, so on macOS and iOS step 2 is enough.

Android **throws** instead of prompting, so the grant has to exist first:

```cplus
import "permissions/permissions" as perm;

match perm::state(perm::LOCATION_WHEN_IN_USE) {
    perm::State::Granted => { start(); }
    // Approximate-only. Still usable — the stream runs at the tier the
    // person allowed, and `accuracy()` reports it honestly.
    perm::State::Limited => { start(); }
    perm::State::Unknown => { let _ = perm::request(perm::LOCATION_WHEN_IN_USE, answered); }
    perm::State::Denied  => { let _ = perm::request(perm::LOCATION_WHEN_IN_USE, answered); }
    perm::State::Blocked => { show("enable location in Settings"); }
    perm::State::Unsupported => { show("this build cannot ask"); }
}
```

Writing the gate on every platform is harmless and keeps one code path.

## 4. Follow the person

```cplus
static STREAM: loc::Updates = #zero::[loc::Updates]();

match loc::updates(on_fix: got,
                   request: loc::Request::new(accuracy: loc::Accuracy::Fine,
                                              distance_filter_m: 25.0f64)) {
    result::Result::Ok(u) => { STREAM = u; }
    result::Result::Err(e) => { show("no stream: ${e.to_code()}"); }
}
```

`Updates` **stops when it drops**. Park it in a `static` or a field — a local
stops at the end of the block and the fixes never come.

## 5. Stop when you are not on screen

```cplus
impl Screen: component::Lifecycle {
    fn on_attach(ref this, why: component::Attach) {
        match why {
            component::Attach::Mount => { return; }   // views only
            _ => { }
        }
        start();
    }
    fn on_detach(ref this, why: component::Detach) {
        match why {
            // Focus only — a dialog, or the other half of a split screen.
            // Still visible, still working. Do NOT stop here.
            component::Detach::Inactive => { return; }
            _ => { }
        }
        STREAM.stop();
    }
}
```

## 6. Run it

Each platform has exactly one thing that is not guessable. The probes under
`playground/` encode it:

| | script | the thing |
|---|---|---|
| macOS | `locationprobe_mac/bundle.sh` | must be a signed `.app`, or it is silent |
| iOS | `locationprobe_ios/sim.sh` | `xcrun simctl location <udid> set <lat>,<lon>` |
| Android | `locationprobe_android/build.sh` | `adb emu geo fix <lon> <lat>` — **longitude first** |
