# Tutorial

Quick path: store a secret, read it back. Rationale and gotchas live in
[guide.md](guide.md); signatures in [ref.md](ref.md).

## Depend

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

Android names `facet` because the Context comes from the running Activity. No
permission is needed on either platform, and nothing is added to the manifest.

```cplus
import "securestore/securestore" as securestore;
import "stdlib/text" as text;
```

## Store and read

```cplus
let stored: securestore::Outcome = securestore::set("token", to: session_token);

var token: text::Text = text::new();
match securestore::get("token", into: token) {
    securestore::Outcome::Ok       => { /* token.view() is the secret */ }
    securestore::Outcome::NotFound => { /* first run — sign in */ }
    securestore::Outcome::Denied   => { /* the platform refused */ }
    securestore::Outcome::Unsupported => { /* no secure storage here */ }
    securestore::Outcome::Failed   => { /* something else broke */ }
}
```

`get` leaves your `Text` untouched on anything but `Ok`, so a caller that
ignores the outcome reads what it put there rather than an empty string that
looks like a stored value.

## The rest of the surface

```cplus
securestore::remove("token")            // Ok, or NotFound — not an error
securestore::contains("token")          // bool
securestore::clear()                    // every key in this namespace
```

## Namespaces

Every verb takes `service:`, defaulted to your app's own. Use it when an app has
two independent secret sets and one should be clearable without the other:

```cplus
securestore::set("token", to: t, service: "user");
securestore::set("device-id", to: d, service: "device");
securestore::clear(service: "user");     // the device id survives
```

## Day-one rules

- **Kilobytes, not megabytes.** 4096 bytes per value, enforced; a larger write
  is `Failed`. Anything bigger is a cache and belongs in a file.
- **`NotFound` is not an error.** It is what a first run sees.
- **Binary goes through base64** at the call site — values are `str` in and
  `Text` out. `stdlib/base64` is already there.
- **No biometric gate in v1.** "Require Face ID to read this" is a different
  feature; see the guide.

## Tests

```
cd vendor/securestore && cpc test
```

Unit tests are `#[test]` blocks in `src/securestore.cplus`, and they
deliberately never touch a real keychain — see the guide's testing section for
why, and for how the platform halves are verified instead.
