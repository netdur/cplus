# Reference

`import "haptics/haptics" as hap;`

## Feel

```cplus
enum Feel { Selection, Light, Medium, Heavy, Success, Warning, Error }

fn Feel::to_code(this) -> i32        // 0..6
fn Feel::from_code(c: i32) -> Feel   // unknown -> Light
```

Codes 0–3 are the impact family, 4–6 the outcome family. The backends split on
that boundary, so the ordering is load-bearing.

## Verbs

```cplus
fn available() -> bool
fn play(feel: Feel = Feel::Light) -> bool
fn prepare(feel: Feel = Feel::Light)
```

**`play`** answers whether the platform accepted the request, never whether
anything was felt. Safe to call when unavailable, and safe to call often.

**`available`** is for hiding a setting, not for guarding a tap.

**`prepare`** warms the Taptic Engine for a tap a few milliseconds away.
Optional; a no-op on macOS, Android and Windows.

## Coverage

| | macOS | iOS | Android | Windows |
|---|---|---|---|---|
| `play` | ✅ Force Touch trackpad only | ✅ | ✅ | ✅ XInput gamepad rumble |
| `prepare` | no-op | ✅ | no-op | no-op |
| `available` | false with no Force Touch | class presence | `hasVibrator()` | a connected XInput pad |

**Windows taps a GAMEPAD, not the machine.** A desktop has nothing to buzz, so
`available()` is false unless an XInput controller is connected — the common
case on a desktop, and not an error. XInput itself is bound at runtime, so a
machine without it answers false rather than failing to start.

An **iPad has no Taptic Engine**: the classes exist, `available` answers true,
and nothing is felt. That is the case this package's "fire and forget" contract
is written for.
