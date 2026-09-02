# Guide

## What a pass actually proves

That **the person holding the device is its owner**. Nothing else.

It does not identify a user to your server, it does not survive a device
handover, and it cannot be verified remotely — the check happens entirely on
the device and reports a boolean. An app that treats it as a login has an
authentication system a stolen unlocked phone defeats.

The shape that works: keep a real credential in `securestore`, and use this to
decide whether to read it.

## The failures are different sentences

`available()` answers false for three unrelated reasons, and `authenticate`
reports which. Collapsing any two sends somebody to the wrong place:

- **`Unavailable`** — no sensor, or disabled by policy. Nothing the person can do.
- **`NotEnrolled`** — there is a sensor and no finger or face registered. The
  fix is Settings, and telling them "your device doesn't support this" is
  wrong.
- **`LockedOut`** — too many attempts. The platform will not ask again until a
  passcode is entered, so retrying immediately does nothing.
- **`Rejected`** — the check ran and did not pass.
- **`Cancelled`** — they dismissed it. **Not a failure.** An app that retries
  on this nags somebody who already answered.

## `Cancelled` on Android arrives twice, and does not

`onAuthenticationFailed` fires for *one wrong finger* while the prompt stays up
and the person tries again. The backend ignores it: reporting it would fire the
handler several times for one ask. Only `onAuthenticationError` ends the
attempt.

## Android will not say which

Apple's `LAContext.biometryType` answers Touch ID, Face ID or Optic ID exactly.
**Android has no equivalent** — `BiometricManager.canAuthenticate()` answers
whether *something strong* is enrolled and never what it is.

So `kind()` answers `Fingerprint` on Android as "something", which is wrong on
every face-unlock phone. Naming the sensor in your UI is safe on Apple and a
guess on Android; "Biometrics" or "Unlock" is the honest label there.

## The Apple ordering rule

`biometryType` is **only populated after `canEvaluatePolicy` has run**. On a
fresh `LAContext` it answers `None`, which reads as "no hardware" and is not.
The backend always evaluates first — it is the one ordering rule in the API.

## A context is single-use

`LAContext` caches its result: evaluating the same policy twice on one context
can succeed the second time **without asking**. That is what
`touchIDAuthenticationAllowableReuseDuration` is for, and a security hole when
it is not what you meant. The backend builds a fresh context per call, releases
it in the reply, and does not offer reuse.

## Threading differs, and the facade does not hide it

**Apple** calls the reply on a private queue — *not* the main thread. A handler
that touches the UI has to hop.

**Android** is built with `getMainExecutor()`, so the callback is already on
the main thread.

This is not smoothed over, because hopping on the caller's behalf would add a
turn of latency to the platform that does not need it.

## API levels

`BiometricPrompt` is **API 28**, `BiometricManager` is **29**. This project's
floor is 26, so below those every verb answers `Unsupported` rather than
falling back to the deprecated `FingerprintManager` — which draws no dialog and
would mean this package shipping a UI.

`androidx.biometric` covers API 23+ and draws its own dialog. It is an AAR with
a transitive closure the AAR measurement priced in megabytes of dex, which is a
large thing to ship to check a fingerprint.

## `allow_passcode`

True by default. It lets the device passcode stand in when biometrics fail or
are unavailable — the alternative locks a person out of their own data over a
wet finger.

Turn it off only when the biometric *itself* is the point. On Android this maps
to `DEVICE_CREDENTIAL`, which is **API 30+**; below that the backend falls back
to a negative button, because the Builder throws if given neither.

## What was measured

Nothing yet — **a finger cannot be asserted from a test**. The suite covers the
vocabulary, that the four failures are distinct, and that asking on a machine
which cannot answers rather than prompting.
