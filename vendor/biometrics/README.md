# biometrics

Ask the person to prove they own the device.

```toml
[dependencies]
biometrics = "*"
```

```cplus
import "biometrics/biometrics" as bio;

fn answered(o: bio::Outcome, ctx: *u8) {
    if o == bio::Outcome::Ok { unlock(); }
}

let _o: bio::Outcome = bio::authenticate("Unlock your notes", answered);
```

## Not a permission, and not authentication

**Not a permission** — a permission is asked once and remembered; this is asked
every time and answered by a finger. There is nothing to grant in Settings,
which is why it is not in `permissions`.

**Not authentication.** A pass means *"the person holding this device is its
owner"*, never *"this user is who they claim to be to your server"*. Anything
needing the second wants a token — keep it in `securestore` and gate it behind
this, rather than replacing it with this.

## Four ways of failing, and they are different sentences

| | what to say |
|---|---|
| `Unavailable` | no sensor — nothing the person can do |
| `NotEnrolled` | sensor, no finger registered — send them to Settings |
| `Rejected` | that was not you — try again |
| `LockedOut` | too many tries — a passcode is needed first |
| `Cancelled` | they said no. **Not a failure** — do not nag |

## Coverage

| | macOS | iOS | Android |
|---|---|---|---|
| prompt | ✅ Touch ID / Watch | ✅ Touch ID / Face ID | ✅ API 28+ |
| which sensor | ✅ exactly | ✅ exactly | ⚠ "something" only |
| passcode fallback | ✅ | ✅ | ✅ API 30+ |

- [tutorial](docs/tutorial.md) · [guide](docs/guide.md) · [ref](docs/ref.md)

## Tests

    cd vendor/biometrics && cpc test

A finger cannot be asserted.
