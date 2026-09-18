# Reference

Fast start: [tutorial.md](tutorial.md). Platform behavior and security model:
[guide.md](guide.md).

`import "biometrics/biometrics" as bio;`

## Kind

```cplus
enum Kind { None, Fingerprint, Face, Iris }

fn Kind::to_code(this) -> i32
fn Kind::from_code(c: i32) -> Kind
```

Exact on Apple. On Android `Fingerprint` means "something strong is enrolled" —
the framework will not say which sensor.

## Outcome

```cplus
enum Outcome { Ok, Unsupported, Unavailable, NotEnrolled,
               Rejected, Cancelled, LockedOut, Failed }
```

`Cancelled` is **not** a failure. `NotEnrolled` is **not** `Unavailable`.

## Verbs

```cplus
fn kind() -> Kind
fn available() -> bool
fn authenticate(reason: str,
                on_result: fn(Outcome, *u8),
                ctx: *u8 = 0 as *u8,
                allow_passcode: bool = true) -> Outcome
```

**`reason`** is shown to the person and must be non-empty — Apple *throws* on
an empty one, so the facade refuses it first.

**The return value** is whether the prompt was raised. The answer arrives on
the handler.

**The handler runs on the main thread on Android and on a private queue on
Apple.** The current Linux and Windows stubs call it synchronously on the
calling thread with `Unavailable`.

## Coverage

| | macOS | iOS | Android | Linux | Windows |
|---|---|---|---|---|---|
| prompt | ✅ | ✅ | ✅ API 28+ | ❌ `Unavailable` | ❌ `Unavailable` |
| `kind()` exact | ✅ | ✅ | ❌ "something" | `None` | `None` |
| `allow_passcode` | ✅ | ✅ | API 30+, else a Cancel button | — | — |
