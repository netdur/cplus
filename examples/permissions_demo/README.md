# permissions_demo

`vendor/permissions` on macOS, in a real `.app` bundle.

```sh
cd examples/permissions_demo && ./bundle.sh && open out/Permissions.app
```

Six rows — camera, microphone, contacts, calendar, photos, notifications — each
showing the state this package reports and a button that asks for it. Refresh
re-reads everything; Open Settings deep-links to the first blocked domain.

## Why it is a bundle

This is the point of the example rather than an implementation detail. macOS
gates every privacy prompt on TCC, and TCC reads the usage-description key out
of the app's `Info.plist`. A bare `cpc build` binary has no plist and no bundle
identifier, so it can never prompt — and if it asks anyway, the process dies.

`bundle.sh` assembles the `.app`, writes the plist with one key per domain, and
ad-hoc signs it so TCC's decisions survive a rebuild.

## The experiment worth running

Delete a `<key>NSCameraUsageDescription</key>` line from `bundle.sh`, rebuild,
and press Camera's **Ask**. The app vanishes. Measured on macOS 26.6:

```
Termination Reason: Namespace TCC, Code 0
This app has crashed because it attempted to access privacy-sensitive data
without a usage description.

Thread 1 Crashed:: Dispatch queue: com.apple.root.default-qos
  __TCC_CRASHING_DUE_TO_PRIVACY_VIOLATION__
  __TCCAccessRequest_block_invoke.229
```

Two things in that report are worth more than the crash itself:

- **Only the request is fatal.** The status read that ran a moment earlier was
  perfectly happy. An app that only reads states at startup looks healthy right
  up until someone taps a button.
- **It is not fatal at the call site.** Thread 0 is idle in the run loop —
  `request` had already returned normally, and TCC killed the process from a
  background queue. Nothing fails where you are looking.

That is why `plans/permissions.md` §7's decision (cpc generates the manifest and
checks nothing) is worth knowing about, and why the guide carries the full
key-per-domain table.

## Resetting

TCC remembers per bundle identifier:

```sh
tccutil reset Camera dev.cplus.permissionsdemo
tccutil reset All dev.cplus.permissionsdemo
```

## Notes

- **Notifications reads `Unsupported`**, correctly: `UNUserNotificationCenter`
  wants an app whose signature it trusts, and this one is only ad-hoc signed.
- **"Ask camera + mic"** calls `request_many`. On macOS that is two prompts in
  sequence — Apple has no batch API — where the same call on Android is one
  dialog. Same code; the facade absorbs the difference.
- The Android counterpart is `playground/permprobe`.
