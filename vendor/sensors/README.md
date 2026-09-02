# sensors

Accelerometer, gyroscope, magnetometer, barometer — one shape each.

```toml
[dependencies]
sensors = "*"
```

```cplus
import "sensors/sensors" as sens;

fn shook(s: sens::Sample, ctx: *u8) {
    if s.magnitude() > 15.0f64 { /* a shake */ }
}

match sens::updates(sens::Kind::Accelerometer, on_sample: shook) {
    result::Result::Ok(r) => { READINGS = r; }   // stops on drop
    result::Result::Err(e) => { }
}
```

## Units are normalised

The platforms disagree and the difference is silent, so this package picks one
set and converts:

| kind | unit | note |
|---|---|---|
| `Accelerometer` | m/s² | **includes gravity** — flat on a table reads ~9.8, not 0 |
| `Gyroscope` | rad/s | |
| `Magnetometer` | µT | uncalibrated; a compass needs more than this |
| `Barometer` | hPa | in `x`; `y` and `z` are 0 |

Apple reports acceleration in **G**; an app written against one platform and
run on the other would be wrong by 9.8× with no error anywhere.

## Coverage

| | macOS | iOS | Android |
|---|---|---|---|
| accelerometer, gyroscope, magnetometer | hardware permitting | ✅ | ✅ |
| barometer | ❌ | ❌ | ✅ |

Verified live on an iPad Pro M1 and an Android emulator, 2026-09-02.

A Mac answers `Unavailable` for everything — CoreMotion links there and the
hardware does not exist. That is deliberately **not** `Unsupported`, which
means "this build cannot ask at all".

The barometer is Android-only for now: Apple's is `CMAltimeter`, a separate
class with no polled entry point. See [docs/guide.md](docs/guide.md).

- [tutorial](docs/tutorial.md) · [guide](docs/guide.md) · [ref](docs/ref.md)

## Tests

    cd vendor/sensors && cpc test

Live paths need a device or emulator — `playground/sensorprobe_android` runs
all four against `adb emu sensor set`.
