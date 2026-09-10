# location

Where the device is — once, or continuously while the app is in use.

```toml
[dependencies]
location = "*"
```

```cplus
import "location/location" as loc;

fn got(f: loc::Fix, ctx: *u8) {
    // A failed or timed-out request arrives here too, with a negative
    // accuracy. Check before you read the coordinate.
    if !f.is_valid() { return; }
    say("${f.latitude}, ${f.longitude}  ±${f.accuracy_m}m");
}

let outcome = loc::once(on_fix: got);
```

A stream is the same call with a handle you keep:

```cplus
match loc::updates(on_fix: got, request: loc::Request::new(accuracy: loc::Accuracy::Fine)) {
    result::Result::Ok(u) => { STREAM = u; }   // stops on drop
    result::Result::Err(e) => { }
}
```

## Scope

Foreground only. One fix, or a stream while the person is using the app. It
does **not** do background tracking or a service that outlives the app — those
are Android-shaped, have no iOS equivalent, and would need a typed foreground
service and an "always" grant that Android 10+ will not even prompt for.

- [tutorial](docs/tutorial.md) — a working screen in ten minutes
- [guide](docs/guide.md) — permissions, accuracy, and the traps per platform
- [ref](docs/ref.md) — every type and verb

## Tests

Unit tests live in `src/test_main.cplus`; both halves build together so the
seam is linked rather than assumed.

    cd vendor/location && cpc test

The live paths need a device or simulator — see the probes under
`playground/locationprobe_{mac,ios,android}`, each with a script that sets up
the one thing that is not guessable on its platform.
