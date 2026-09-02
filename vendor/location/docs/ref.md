# Reference

`import "location/location" as loc;`

## Accuracy

```cplus
enum Accuracy { Coarse, Balanced, Fine, Navigation }

fn Accuracy::to_code(this) -> i32          // 0, 1, 2, 3
fn Accuracy::from_code(c: i32) -> Accuracy // unknown codes -> Fine
```

Selects a power budget. See the [guide](guide.md#accuracy-is-a-wish-and-the-answer-can-differ)
— a grant can silently downgrade what you get.

## Outcome

```cplus
enum Outcome { Ok, Denied, Unsupported, Disabled, Timeout, Failed }

fn Outcome::to_code(this) -> i32           // 0..5
fn Outcome::from_code(c: i32) -> Outcome
```

| | meaning |
|---|---|
| `Ok` | the request was **accepted**. Not "you have a fix". |
| `Denied` | the person said no, or has not been asked (Android). |
| `Unsupported` | no backend, or no hardware. |
| `Disabled` | location services are off system-wide. Different from `Denied`, and needs a different thing said to a person. |
| `Timeout` | reserved; a give-up currently reaches the handler as an invalid fix. |
| `Failed` | anything else. |

## Fix

```cplus
#[repr(C)]
struct Fix {
    latitude: f64,
    longitude: f64,
    accuracy_m: f64,             // horizontal, ~68% confidence. NEGATIVE = invalid
    altitude_m: f64,
    altitude_accuracy_m: f64,    // negative = altitude unavailable
    speed_mps: f64,              // negative = unknown
    course_deg: f64,             // clockwise from true north. negative = unknown
    timestamp_ms: i64,           // unix ms AT THE DEVICE; a cached fix can be old
}

fn Fix::is_valid(this) -> bool       // accuracy_m >= 0 — CHECK THIS FIRST
fn Fix::has_speed(this) -> bool
fn Fix::has_course(this) -> bool
fn Fix::has_altitude(this) -> bool
```

`#[repr(C)]` because it crosses the seam in a handler's parameter list.

## Request

```cplus
struct Request { accuracy: Accuracy, distance_filter_m: f64, timeout_ms: i64 }

fn Request::new(accuracy: Accuracy = Accuracy::Balanced,
                distance_filter_m: f64 = 0.0f64,
                timeout_ms: i64 = 0 as i64) -> Request
fn Request::defaults() -> Request
```

`distance_filter_m` — metres of movement before another fix. 0 = every fix.
`timeout_ms` — honoured on Android; Apple runs its own ~10 s timer. 0 = the
platform default, never "forever".

`defaults()` exists only because `Request::new()` cannot be a default-argument
expression (E0308 counts parameters before filling them). It is a deliberate
deviation from the naming guideline, recorded at the definition.

## Module verbs

```cplus
fn available() -> bool                     // a backend and the hardware exist
fn services_enabled() -> bool              // the DEVICE's switch, which no app can change
fn permission() -> perm::State             // Unknown at launch on Apple — not Denied
fn last_known() -> option::Option[Fix]     // free, possibly stale. None on Android
fn once(on_fix: fn(Fix, *u8), ctx: *u8 = 0 as *u8,
        request: Request = Request::defaults()) -> Outcome
fn updates(on_fix: fn(Fix, *u8), ctx: *u8 = 0 as *u8,
           request: Request = Request::defaults())
    -> result::Result[Updates, Outcome]
```

The handler and its `ctx` are adjacent with `ctx` defaulted, which is what lets
a caller pass a bound method (`this.got`) and have the receiver filled in.

## Updates

```cplus
struct Updates { opaque _h: *u8 }

fn Updates::accuracy(this) -> Accuracy      // what was GRANTED, read live
fn Updates::permission(this) -> perm::State // what the delegate actually saw
fn Updates::is_running(this) -> bool
fn Updates::stop(ref this)                  // idempotent
fn Updates::drop(ref this)                  // stops
```

**Stops on drop.** A local `Updates` stops at the end of its block; park it in
a `static` or a field.

## Platform coverage

| | macOS | iOS | Android |
|---|---|---|---|
| `available` / `services_enabled` | ✅ | ✅ | ✅ |
| `permission` | ✅ | ✅ | ✅ |
| `last_known` | ✅ | ✅ | ❌ `None` |
| `once` / `updates` | ✅ | ✅ | ✅ |
| `accuracy()` read-back | ✅ | ✅ | ✅ |
| prompts on first use | ✅ | ✅ | ❌ ask via `permissions` |
| `timeout_ms` honoured | ❌ own ~10 s | ❌ own ~10 s | ✅ |

Verified live on macOS, the iOS simulator and the Android emulator, 2026-09-02.
