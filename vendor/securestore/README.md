# securestore

Small secrets at rest, under the platform's own key material: the Keychain on
Apple, an `AndroidKeyStore`-held AES key on Android.

```toml
[dependencies]
stdlib      = "*"
securestore = "*"

[macos.dependencies]
objc = "*"
[ios.dependencies]
objc = "*"
[android.dependencies]
jni          = "*"
android_view = "*"
facet        = "*"
flex_layout  = "*"
events       = "*"
```

```cplus
import "securestore/securestore" as securestore;
import "stdlib/text" as text;

let _s: securestore::Outcome = securestore::set("token", to: session_token);

var token: text::Text = text::new();
match securestore::get("token", into: #addr_of(token)) {
    securestore::Outcome::Ok       => { /* token.view() */ }
    securestore::Outcome::NotFound => { /* first run */ }
    securestore::Outcome::Denied   => { }
    securestore::Outcome::Unsupported => { }
    securestore::Outcome::Failed   => { }
}
```

No permission, no manifest entry, no AAR. A token, a refresh token, an API key
— **kilobytes, not megabytes**: 4096 bytes per value, enforced.

**What it protects you from** is a stolen device, a copied backup and a sandbox
someone walked out with. Not a compromised running process: anything this can
read, code running as your app can read.

- [docs/tutorial.md](docs/tutorial.md) — depend, store, read back
- [docs/guide.md](docs/guide.md) — what each platform does, the macOS
  code-identity trap, why not `EncryptedSharedPreferences`, and how the platform
  halves are verified
- [docs/ref.md](docs/ref.md) — every signature

## Tests

```
cd vendor/securestore && cpc test
```

Unit tests are `#[test]` blocks in `src/securestore.cplus`. They **never touch a
real keychain**, deliberately — the round-trips live in `playground/
securestoreprobe` (macOS) and `playground/ssprobe` (Android), run on purpose.
The guide's testing section says why, and what each platform's
verification actually covers.
