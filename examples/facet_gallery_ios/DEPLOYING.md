# Getting a C+ app onto an iPhone or iPad

Written for iris, and written the hard way: every command below was run against
a real iPad in one sitting, and every error quoted is one that actually came
back. The order matters — each step tells you whether the next one can work.

**The short version.** Simulator needs nothing. A real device needs four things
that are all invisible until one of them is missing: a paired device with
Developer Mode on, a valid signing certificate, a provisioning profile that
names *this* device and *this* bundle id, and — on a free account — a free slot
and a certificate you have trusted on the device by hand.

---

## 0. The simulator, which needs none of this

Do this first. If the app is broken, find out here.

```
cd examples/facet_gallery_ios
cpc build --target ios-arm64-simulator

S=/tmp/GalleryApp && mkdir -p $S/Gallery.app && cp ios/Info.plist $S/Gallery.app/

# Every prebuilt dependency slice. NOT optional since `prebuild` became the
# default on 2026-08-16: a dependency's object code lives in its own
# `vendor/<pkg>/lib/<triple>/` archive rather than inside this app's, so
# leaving these out fails the link on symbols nothing defines. Globbing
# over-links, which costs nothing — an archive nothing references contributes
# no bytes. README.md step 4 says the same for the Xcode route.
slices=$(find ../../vendor -maxdepth 4 -path '*/lib/arm64-apple-ios-simulator/*.a')

xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=14.0 \
  -I target/ios-arm64-simulator/debug \
  ios/main.m target/ios-arm64-simulator/debug/libfacet_gallery_ios.a \
  $slices \
  -framework UIKit -framework QuartzCore -framework Foundation \
  -framework CoreGraphics -framework WebKit -lobjc \
  -o $S/Gallery.app/Gallery

DEV=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys
for rs in json.load(sys.stdin)["devices"].values():
    for x in rs:
        if x.get("state")=="Booted": print(x["udid"]); break')
xcrun simctl install $DEV $S/Gallery.app
xcrun simctl launch --console-pty $DEV dev.cplus.facetgalleryios
```

No signing, no account, no Xcode project. `--console-pty` gives you the app's
stderr, which is where facet's diagnostics go.

**What the simulator cannot tell you:** whether
`UIApplicationMain(0, NULL, …)` works (it does — verified on device), how touch
actually feels, or anything reached over USB. Those need the real thing.

---

## 1. Is a device there, and is it usable?

```
xcrun devicectl list devices
```

```
Name       Identifier                             State       Model                    Reality
iPad (4)   DC85D387-689E-5F8B-B0A4-FF3F733ABD8B   connected   iPad Pro (11-inch) …     physical
```

Take the **Identifier** column — that is what `devicectl` wants. `xcrun xctrace
list devices` prints the *other* identifier (the 25-character UDID, e.g.
`00008103-000D0988229A001E`), which is what **xcodebuild** wants. They are
different strings for the same device and mixing them up is a confusing hour.

Then:

```
xcrun devicectl device info details --device <identifier> | grep -iE "Pairing|Developer Mode"
```

```
• Pairing State: paired
• Developer Mode Status: Enabled (1)
```

Both must say that. Developer Mode is on the device:
**Settings → Privacy & Security → Developer Mode** (it needs a reboot).

**Prove the tunnel actually works** before trusting any of it:

```
xcrun devicectl device info apps --device <identifier>
```

If that lists apps, the connection is real. If it hangs or errors, nothing
below will work and the problem is the cable, the trust prompt on the device,
or a locked screen.

---

## 2. Is there a certificate?

```
security find-identity -v -p codesigning
```

```
1) 834A06CC…  "Apple Development: you@example.com (YJ252A5FQ8)" (CSSMERR_TP_CERT_REVOKED)
5) 20F9FD0F…  "Apple Development: you@example.com (YJ252A5FQ8)"
   5 valid identities found
```

Ignore the count — it lies. Read the annotations: anything marked
`CSSMERR_TP_CERT_REVOKED` is dead. You need **at least one without a marker**.
Revoked ones pile up over years and are harmless as long as one is live.

If every one is revoked, sign out and back in to Xcode
(**Xcode → Settings → Accounts**), which mints a fresh one.

---

## 3. Which team, and is it free?

This is the step with a trap in it.

```
defaults read com.apple.dt.Xcode IDEProvisioningTeams
```

**Do not trust this while Xcode is running.** `defaults` reads a cached domain
and will report `does not exist` for an account that is signed in perfectly
well. It cost me a wrong diagnosis. Read the file instead:

```
plutil -p ~/Library/Preferences/com.apple.dt.Xcode.plist | grep -A 8 IDEProvisioningTeamByIdentifier
```

```
"isFreeProvisioningTeam" => true
"teamID" => "YW2A442B88"
"teamName" => "adel b (Personal Team)"
"teamType" => "Personal Team"
```

`isFreeProvisioningTeam => true` is the one that shapes everything after it:

| | free / Personal Team | paid Developer Program |
|---|---|---|
| apps installed at once | **3** | unlimited |
| profile lifetime | **7 days**, then the app stops launching | 1 year |
| app ids per week | 10 | unlimited |
| trust prompt on device | **required, by hand** | required once per certificate |

A free team is entirely fine for testing. It just expires under you, and the
first symptom is an app that was working yesterday refusing to launch.

**The team on the certificate and the team on an old profile can differ** — this
machine had a cert under one team and a stale profile under another. Match the
profile to whatever `teamID` above says.

---

## 4. Mint a profile

There is no CLI that asks for a provisioning profile directly. Xcode makes one
as a side effect of building a project, so a project is what you need — even
though `cpc` has already produced the whole binary.

`ios/Gallery.xcodeproj` in this directory is that project, deliberately minimal:
one target, `main.m`, the static library from `target/ios-arm64/debug`, five
frameworks, and automatic signing. **Change `DEVELOPMENT_TEAM` to your own team
id** (search the pbxproj — it appears twice) and the bundle id if you want your
own.

```
cd examples/facet_gallery_ios
cpc build --target ios-arm64                       # the device slice, NOT the simulator one

cd ios
xcodebuild -project Gallery.xcodeproj -target Gallery -configuration Debug \
  -destination 'platform=iOS,id=<25-char-UDID>' \
  -allowProvisioningUpdates build
```

`-allowProvisioningUpdates` is what lets Xcode create the profile and register
the device without opening the UI. Success looks like:

```
Signing Identity:     "Apple Development: you@example.com (…)"
Provisioning Profile: "iOS Team Provisioning Profile: dev.cplus.facetgalleryios"
** BUILD SUCCEEDED **
```

If it cannot sign, it says so here and not later.

---

## 5. Install

```
xcrun devicectl device install app --device <identifier> \
  ios/build/Debug-iphoneos/Gallery.app
```

Success prints a `bundleID` and an `installationURL`.

**The free-slot failure**, which is the one you will actually hit:

```
ApplicationVerificationFailed
-[MIFreeProfileValidatedAppTracker _onQueue_addReferenceForApplicationIdentifier:bundle:error:]
   ( "TEAM.some.other.app", "TEAM.another.app", "TEAM.a.third.app" )
```

That list is the three apps already using your free slots. Delete one on the
device and install again. Nothing is wrong with your build.

---

## 6. Trust the certificate, on the device

```
ERROR: The application failed to launch.
  … it has an invalid code signature, inadequate entitlements or its profile
    has not been explicitly trusted by the user.
```

This is not a build problem. On the device:

**Settings → General → VPN & Device Management → Developer App →
"Apple Development: you@…" → Trust**

Once per certificate — and signing out and back in to Xcode makes a *new*
certificate, so a previously trusted machine needs trusting again. If the entry
is missing, tap the app icon on the home screen once so iOS registers it, then
look again.

---

## 7. Launch, and get the console

```
xcrun devicectl device process launch --device <identifier> \
  --console dev.cplus.facetgalleryios
```

`--console` attaches stderr, which is where facet's diagnostics come out. Keep
it running and reproduce whatever you are chasing; the output arrives live.

Is it still alive?

```
xcrun devicectl device info processes --device <identifier> | grep -i Gallery
```

**On crashes.** A segfault gives you `App terminated due to signal 11.` and
nothing else, and `devicectl device copy from` **cannot** reach the crash logs:

```
Access restricted: '/private/var/mobile/Containers/Data/Application/…' is
outside the allowed container directories (Library, Documents, tmp).
```

The full report is in **Xcode → Window → Devices and Simulators → View Device
Logs**. But the faster route, and the one that found the real bug here, is to
put `io::eprintln` at each step of the suspect path, deploy, and read the last
line before the signal. A crash log gives you a stack; a print gives you the
state, and the state is usually what you were wrong about.

---

## 8. Redeploy loop

Once it is set up, an edit-to-device cycle is:

```
cd examples/facet_gallery_ios && cpc build --target ios-arm64 \
 && cd ios && xcodebuild -project Gallery.xcodeproj -target Gallery \
      -configuration Debug -destination 'platform=iOS,id=<UDID>' \
      -allowProvisioningUpdates build \
 && xcrun devicectl device install app --device <identifier> \
      build/Debug-iphoneos/Gallery.app \
 && xcrun devicectl device process launch --device <identifier> \
      --console dev.cplus.facetgalleryios
```

---

## What an iPad tests that a phone cannot

Worth knowing before you pick a device:

- **The wide width class.** An iPad is ≥720pt in both orientations, so it
  exercises multi-column layout that a phone never reaches.
- **The hardware key band** (`pressesBegan:`) with a Magic Keyboard or any
  Bluetooth keyboard. A phone with no keyboard fires it for nobody.
- **Hover**, with a trackpad or an Apple Pencil —
  `UIHoverGestureRecognizer` is real on iPadOS.

---

## The agent surface over USB — UNVERIFIED

`facet_uikit` serves its agent surface over **TCP on loopback**, not a Unix
socket, because an app's socket lives inside its sandbox where nothing on the
development machine can reach it — and to a device there is no shared
filesystem at all. The port is what an app passes to `agent_mcp(...)`, default
**8787**.

Reaching it from the Mac means forwarding over usbmuxd, the same mechanism
Flutter's Dart VM Service and Chrome's remote debugging use:

```
iproxy 8787 8787 <UDID>          # or: pymobiledevice3 usbmux forward 8787 8787
```

**This has not been run against a device.** The transport is tested and the
reader is tested; the USB forward is the one link in the chain nobody has
exercised. Treat it as a starting point, not a recipe.
