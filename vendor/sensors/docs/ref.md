# Reference

`import "sensors/sensors" as sens;`

## Kind

```cplus
enum Kind { Accelerometer, Gyroscope, Magnetometer, Barometer }

fn Kind::to_code(this) -> i32          // 0..3
fn Kind::from_code(c: i32) -> Kind     // unknown -> Accelerometer
```

## Outcome

```cplus
enum Outcome { Ok, Unsupported, Unavailable, Denied, Failed }

fn Outcome::to_code(this) -> i32
fn Outcome::from_code(c: i32) -> Outcome
```

| | meaning |
|---|---|
| `Unsupported` | no backend for this platform |
| `Unavailable` | backend present, device has no such sensor |
| `Denied` | Android 12+ high-rate gating |
| `Failed` | anything else |

## Sample

```cplus
#[repr(C)]
struct Sample { x: f64, y: f64, z: f64, timestamp_ms: i64 }

fn Sample::magnitude(this) -> f64      // euclidean norm of the triple
```

| kind | x | y | z |
|---|---|---|---|
| `Accelerometer` | m/s² | m/s² | m/s² — includes gravity |
| `Gyroscope` | rad/s | rad/s | rad/s |
| `Magnetometer` | µT | µT | µT |
| `Barometer` | hPa | 0 | 0 |

`timestamp_ms` is Unix ms **at the source**, not at delivery.

## Request

```cplus
struct Request { interval_ms: i64 }

fn Request::new(interval_ms: i64 = 0 as i64) -> Request
fn Request::defaults() -> Request
```

A hint on both platforms. 0 takes the platform default, never "as fast as
possible".

## Verbs

```cplus
fn has(kind: Kind) -> bool
fn updates(kind: Kind, on_sample: fn(Sample, *u8), ctx: *u8 = 0 as *u8,
           request: Request = Request::defaults())
    -> result::Result[Readings, Outcome]
```

## Readings

```cplus
struct Readings { opaque _h: *u8 }

fn Readings::kind(this) -> Kind
fn Readings::is_running(this) -> bool
fn Readings::stop(ref this)        // idempotent
fn Readings::drop(ref this)        // stops
```

**Stops on drop.** Park it in a `static` or a field.
