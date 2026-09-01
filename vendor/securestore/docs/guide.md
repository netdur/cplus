# Guide

What "secure" means here, what each platform actually does, and the several ways
this can look like it is working when it is not. Signatures are in
[ref.md](ref.md); the five-minute path is [tutorial.md](tutorial.md).

## What this protects you from, and what it does not

The bytes at rest are protected by key material the app cannot extract. On Apple
that is the Keychain; on Android an AES key generated inside `AndroidKeyStore`,
which on a device with a TEE or StrongBox is not extractable even with root.

**It is not protection from a compromised running process.** Anything this
package can read, code running as your app can read. The threat it answers is a
stolen device, a copied backup, and a sandbox someone walked out with — not an
attacker already inside the process.

That is worth stating plainly because the Android file is *readable by design*:

```
adb shell run-as your.package cat shared_prefs/<service>.xml
```

It should be readable — it is your app's own file, under your app's uid. What it
must not contain is the plaintext, and that is the claim the package makes and
the test worth running.

## What each platform does

| | Apple | Android |
|---|---|---|
| store | Keychain, `kSecClassGenericPassword` | AES-GCM, key in `AndroidKeyStore` |
| where the ciphertext lives | the keychain itself | `SharedPreferences`, base64 |
| key material | never in your process | never in your process |
| namespace | `kSecAttrService` | the preferences file name |
| needs a permission | no | no |
| needs a manifest entry | no | no |

**macOS uses the LEGACY keychain**, not the data-protection one, and that was
measured rather than chosen. `kSecUseDataProtectionKeychain` answers `-34018`
`errSecMissingEntitlement` in every configuration this repo can build —
unsigned, bundled, ad-hoc signed — and asking for the entitlement with an ad-hoc
signature is *SIGKILLed by AMFI before `main`*. The modern keychain needs a
Developer ID and a provisioning profile, which is a shipping concern; it would
be an opt-in and is not one yet.

## Gotcha: a rebuilt dev binary can be denied its own data on macOS

The legacy keychain ties each item's edit rights to the **code identity** that
created it. An ad-hoc signature changes whenever the binary changes, so a
rebuild can meet `errSecInvalidOwnerEdit` against an item its predecessor wrote,
and a *different* binary reading it raises a modal "enter your login keychain
password" dialog.

`set` handles the first case — it treats that error as delete-and-re-add,
because from your app's side it is its own item and the intent is unambiguous. A
Developer-ID-signed app has a stable identity and never reaches that path.

The password prompt is not something the package can prevent. It is the same
shape as the TCC problem `examples/notifications_demo` documents, and it is a
development-time artifact rather than a property of the design.

## Gotcha: `clear` used to delete one item

`SecItemDelete` with a query matching several items removes **one** on the
legacy keychain. The first version of `clear` therefore deleted one key of two
and reported success. It now loops until `errSecItemNotFound`.

Recorded because it is invisible from inside: the call answers `Ok` either way,
and only a test with two keys in one namespace catches it.

## Why not `EncryptedSharedPreferences`

It is `androidx.security:security-crypto`, an `.aar`, and it depends on Tink — a
large general-purpose crypto library — for what is about sixty lines over
classes already in `android.jar`.

**Linking an AAR without Gradle is possible**, and that is worth being accurate
about: an `.aar` is a zip, `d8` takes jars directly, and this repo's Android
build already runs `d8`. What Gradle provides is Maven dependency resolution,
`aapt2` resource compilation, and manifest merging — only the first is
unavoidable, and only for a library with transitive deps.

So this is a judgement about the trade, not a wall: pulling a crypto library
plus a resolution step to avoid sixty lines is the wrong deal for the first
package in the tree that would do it.

## Sizes, and why the limit is enforced at both ends

4096 bytes per value. A write over it is `Failed`; a read of an item that
somehow exceeds it is `Failed` rather than truncated.

Truncation is the failure worth spending a constant to prevent: half a token
fails authentication somewhere far away, and nobody traces that back to storage.
Refusing at the write is the only place it can be reported to somebody who can
act on it.

The Keychain is a daemon round-trip per item and the Android side encrypts each
value on its own, so this is the shape both platforms are built for. A megabyte
here is a cache, and a cache belongs in a file.

## Namespaces bound `clear`

`service` is a namespace, defaulted to your app's own. It is not decoration: it
is what makes `clear` safe to have at all.

Every query carries the service, on both platforms — on Apple it is always in
the `SecItem` dictionary, and on Android it *is* the preferences file. So there
is no shape of call that reaches items outside one namespace, and on Apple none
that reaches another application's items at all.

## No biometric gate in v1

"Require Face ID / fingerprint to read this" is `SecAccessControl` on Apple and
`setUserAuthenticationRequired` on Android. It is a real feature and a different
one: it makes a read able to put a prompt on screen, so every `get` would have
to become asynchronous for the sake of the few items that want it.

It belongs as a second verb later, not as a tax on the first.

## Testing, and why the suite avoids your keychain

`cd vendor/securestore && cpc test` checks the guards, the outcome mapping and
the size limit. It **never stores anything**.

That is deliberate. `cpc test` builds an unsigned binary and runs it on your
machine, where the legacy keychain is your *login* keychain — a suite that
stored items would leave real entries on every run, and a rebuilt test binary
reading its predecessor's item can raise a password dialog. A test suite that
can interrupt you with a password prompt is one you stop running.

The platform round-trips live in probes, run on purpose:

| platform | how | state |
|---|---|---|
| macOS | `playground/securestoreprobe` | 19/19, cleans up after itself |
| Android | `playground/ssprobe` on a device or emulator | 13/13, plus the `run-as` check below |
| iOS | `vendor/securestore/tools/run_ios_tests.sh` | **blocked** — see below |

The Android probe makes the assertion that matters from outside the process:
the plaintext appears zero times in `shared_prefs/probe.xml`, and two keys
holding the *same* secret have entirely different ciphertexts — a fresh IV per
encryption, which is the one property GCM cannot survive losing.

**iOS storage is unverified.** The runner builds and the package answers a clean
`Denied` on every verb, but the simulator will not launch the entitled bundle
storage needs. `bugs/ios-securestore-runner-will-not-launch-on-the-simulator.md`
has the four facts the harness depends on and the lead worth trying first.
