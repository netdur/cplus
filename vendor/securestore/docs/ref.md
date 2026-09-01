# Reference

Every entry point. How and why live in [guide.md](guide.md); the quick path is
[tutorial.md](tutorial.md).

```cplus
import "securestore/securestore" as securestore;
```

## `Outcome`

```cplus
enum Outcome { Ok, NotFound, Denied, Unsupported, Failed }
```

| variant | meaning |
|---|---|
| `Ok` | Done. |
| `NotFound` | No item under this key in this service. **Not an error** — it is what a first run sees. |
| `Denied` | The platform refused. On Apple this is its access control; see the guide's note on code identity. |
| `Unsupported` | No secure storage reachable here — no keychain, or no Activity and VM on Android. |
| `Failed` | Anything else. The platform's own code is deliberately not surfaced: a caller cannot act on `errSecDecode` differently from `errSecIO`, and leaking `OSStatus` would make every consumer platform-specific to read. |

## `MAX_VALUE_BYTES`

```cplus
const MAX_VALUE_BYTES: usize = 4096 as usize;
```

Enforced at both ends. A write over it is `Failed`; a read of an item that
exceeds it is `Failed` rather than truncated.

## `set`

```cplus
fn set(key: str, to: str, service: str = "") -> Outcome
```

Store `to` under `key`, replacing whatever was there.

`Failed` for an empty key or a value over `MAX_VALUE_BYTES`, both refused before
the platform is touched.

Update-or-insert is two calls on Apple and the backend does both — `SecItemAdd`
answers `errSecDuplicateItem` for an existing key, so a `set` that only added
would work once and silently never change the value again.

## `get`

```cplus
fn get(key: str, into: *text::Text, service: str = "") -> Outcome
```

Read the item under `key` into `into`, replacing its contents.

**`into` is left untouched on anything but `Ok`**, so a caller that ignores the
outcome reads what it put there rather than an empty string that looks like a
stored value.

An out-parameter rather than `Option[Text]` is a deliberate deviation from the
naming guideline, recorded in the source: a caller must tell "absent" from
"refused" from "broken", and `Option` collapses the last two into the first.

`Failed` for an empty key or a null `into`.

## `remove`

```cplus
fn remove(key: str, service: str = "") -> Outcome
```

`NotFound` when there was none — a caller clearing a token it may never have
written should not have to check first.

## `contains`

```cplus
fn contains(key: str, service: str = "") -> bool
```

A `bool` rather than an `Outcome`, because the failures collapse honestly here:
refused, absent and broken all mean "you cannot read a value under this key",
and a caller that needs them apart is calling `get`.

Does **not** decrypt, and on Apple does not request the data — asking for the
bytes is what makes the legacy keychain prompt for a password when the code
identity has changed, and `contains` must never put a dialog up.

## `clear`

```cplus
fn clear(service: str = "") -> Outcome
```

Remove every item in one service. `Ok` when there was nothing to remove.

**Bounded by construction**: every query carries the service, so this cannot
reach items outside the namespace it was given, and on Apple cannot reach
another application's items at all.

On Android the AES key is deliberately left in the keystore. Deleting it would
make every value written before the call permanently unreadable — which `clear`
just did anyway — but would also break a concurrent reader holding a decrypt
cipher. The alias is reused on the next `set`.

## `service`

Every verb takes it, defaulted to `""` — the app's own: the bundle identifier on
Apple, a per-package preferences file on Android. Use it when an app has two
independent secret sets and one should be clearable without the other.

## Platform support

| platform | mechanism | state |
|---|---|---|
| macOS | Keychain, legacy (file) keychain | verified, 19/19 |
| Android | AES-GCM under an `AndroidKeyStore` key, ciphertext in `SharedPreferences` | verified, 13/13 |
| iOS | Keychain, data-protection | builds; storage unverified — see the guide |
| Linux, Windows | none | every verb answers `Unsupported` |
