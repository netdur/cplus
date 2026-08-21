# Getting a C+ app onto an iPhone or iPad

Written for iris, and written the hard way: every command below was run against
a real iPad in one sitting, and every error quoted is one that actually came
back. The order matters — each step tells you whether the next one can work.

**The short version.** Simulator needs nothing. A real device needs four things
that are all invisible until one of them is missing: a paired device with
Developer Mode on, a valid signing certificate, a provisioning profile that
names *this* device and *this* bundle id, and — on a free account — a free slot
and a certificate you have trusted on the device by hand.

**And a fifth, if you want the agent surface**: an actual CABLE. Everything
above works over the network; a port forward does not. §1 is where that is
written down, because it is the state that looks most like success.

*Second sitting, 2026-08-19: the app ran on a device for the first time and its
MCP surface was driven over USB. Everything that cost time then — a locked
device, a cable that was not there, a port a simulator had already taken — is in
here now.*

---

## 0. The simulator, which needs none of this

Do this first. If the app is broken, find out here.

```
cd examples/facet_gallery_ios
cpc build --target ios-arm64-simulator

S=/tmp/GalleryApp && mkdir -p $S/Gallery.app && cp ios/Info.plist $S/Gallery.app/

# ASK CPC WHAT TO LINK. NOT optional since `prebuild` became the default on
# 2026-08-16: a dependency's object code lives in its own
# `vendor/<pkg>/lib/<triple>/` archive rather than inside this app's, so
# leaving these out fails the link on symbols nothing defines — and the error
# names whichever package resolves first, which reads as a bug in that package
# and is not one.
#
# `--print-link-args` prints exactly that list, one argument per line, from the
# same walk the compiler links a host build with: project `vendor/`, then a
# sibling, then `~/.cplus/<tier>/vendor`, then `lib/<triple>`. It brings the
# slices up to date first, so every path it prints is a file that exists and is
# current. It replaces a `find` over `vendor/`, which over-linked, hard-coded
# the layout, and silently missed a slice living in the store rather than the
# project. README.md step 4 says the same for the Xcode route.
#
# INLINE, not through a variable, and that is a zsh fact rather than a style.
# zsh does not word-split an unquoted `$var`, so `slices=$(cpc ...)` followed
# by `$slices` hands clang ONE argument with newlines in it — "no such file or
# directory" naming every archive at once. Unquoted `$(...)` does split, in
# both shells.
xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=14.0 \
  -I target/ios-arm64-simulator/debug \
  ios/main.m target/ios-arm64-simulator/debug/libfacet_gallery_ios.a \
  $(cpc build --target ios-arm64-simulator --print-link-args) \
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
`00008103-000D0988229A001E`), which is what **xcodebuild**, `iproxy` and
`pymobiledevice3` want. They are different strings for the same device and
mixing them up is a confusing hour.

You do not need two commands for them. `-j` carries both, plus the two facts the
human-readable table leaves out:

```
xcrun devicectl list devices -j /tmp/d.json      # a PATH; /dev/stdout does not work
python3 - /tmp/d.json <<'EOF'
import json, sys
for x in json.load(open(sys.argv[1]))["result"]["devices"]:
    hw = x["hardwareProperties"]
    if hw.get("reality") != "physical":       # simulators are in this list too
        continue
    print(x["identifier"], hw["udid"],
          x["connectionProperties"]["transportType"])
EOF
```

```
DC85D387-689E-5F8B-B0A4-FF3F733ABD8B 00008103-000D0988229A001E wired
```

`reality` separates a real device from a simulator. **`transportType` is the one
to read**, and it is `wired` or `localNetwork` — see the cable section below,
because it decides whether half of this document can work at all.

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

### `devicectl` reaching it does NOT mean the cable is in

This is the most misleading state in the whole document, because everything
looks fine.

`devicectl` talks to a paired device **over the network** (`iPad-4.coredevice.local`).
So with no cable at all it lists the device, reports `available (paired)`,
installs apps and launches them — and `transportType` says `localNetwork`.
Anything built on **usbmuxd** sees nothing, because usbmuxd is a different
mechanism and it needs the wire:

```
pymobiledevice3 usbmux list       # [] with no cable; the device with one
```

That matters for exactly one thing here, and it is the agent surface: a port
forward goes through usbmuxd. Install and launch do not.

**Do not use `system_profiler SPUSBDataType` to answer this.** Measured on this
machine with the cable in and usbmuxd reporting `"ConnectionType": "USB"`:
`system_profiler SPUSBDataType` printed **nothing at all** — a confident false
negative. usbmuxd and `transportType` are the authorities; the USB tree is not.

### A locked device refuses the launch, and says which

```
xcrun devicectl device process launch --device <identifier> dev.cplus.facetgalleryios
```

```
The request was denied by service delegate (SBMainWorkspace) for reason: Locked
  ("Unable to launch … because the device was not, or could not be, unlocked")
FBSOpenApplicationErrorDomain error 7
```

Install works while locked; launch does not. Nothing is wrong with the build,
and the message is precise — read it rather than rebuilding.

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

That prints `<pid> <path>`, and the pid is what terminates it — which is worth
knowing because it is the only way to prove which app a socket belongs to:

```
xcrun devicectl device process terminate --device <identifier> --pid <pid>
```

**Keep the app in the FOREGROUND while you are talking to it.** iOS suspends a
backgrounded app, and a suspended app stops accepting on its socket. The symptom
is a connect that HANGS rather than one that is refused, which reads like a bug
in whatever you are testing. Launching anything else on the device — another
test runner, say — is enough to cause it.

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

`xcodebuild` does NOT build the C+ code — `cpc` does, and Xcode only links the
archive it finds. So the `cpc build` at the front is the load-bearing step, and
skipping it re-signs yesterday's binary without a word.

To check the change rather than look at it, drop the `--console` launch and end
with `tools/mcp_check_device.sh`, which launches, forwards and asserts.

---

## What an iPad tests that a phone cannot

Worth knowing before you pick a device:

- **The wide width class** — *if you give it one*. An iPad screen is ≥720pt in
  both orientations, but **iPadOS 26 hands an app a resizable WINDOW rather than
  the screen**, and the window it opens with is not the screen's size. Measured
  here on an iPad Pro 11-inch (iOS 26.6): the gallery came up at **423.5 × 719**
  and reported the COMPACT width class — the same one a phone gets. If you are
  testing multi-column layout, check the frame you actually got before believing
  you tested it. `describe_ui` with `{"mode":"full"}` answers it directly.
- **The hardware key band** (`pressesBegan:`) with a Magic Keyboard or any
  Bluetooth keyboard. A phone with no keyboard fires it for nobody.
- **Hover**, with a trackpad or an Apple Pencil —
  `UIHoverGestureRecognizer` is real on iPadOS.

---

## The agent surface over USB

The gallery serves MCP — see [README.md](README.md#the-mcp-surface--read-and-drive-this-app-over-a-socket)
for the two lines that turn it on and the protocol it speaks.

`facet_uikit` serves it over **TCP on loopback**, not a Unix socket, because an
app's socket lives inside its sandbox where nothing on the development machine
can reach it — and to a device there is no shared filesystem at all. The port is
what the app passes to `agent_mcp(...)`, default **8787**.

**On the simulator there is nothing to forward.** A simulator shares the Mac's
network stack, so `127.0.0.1:8787` on the Mac IS the app's loopback:

```
xcrun simctl launch $DEV dev.cplus.facetgalleryios
tools/mcp_check.py                    # 25 checks; verified 2026-08-19
```

**On a device the port is reached over usbmuxd**, the same mechanism Flutter's
Dart VM Service and Chrome's remote debugging use:

```
tools/mcp_check_device.sh                    # launch + forward + check, in one
```

which is the three steps below, and says which of them your setup is missing
rather than leaving you with a socket that times out:

```
iproxy 8787 8787 <25-char-UDID>              # brew install libimobiledevice
pymobiledevice3 usbmux forward 8787 8787     # or: pip install pymobiledevice3
tools/mcp_check.py                           # the same script, unchanged
```

Three things have to be true, none of them announce themselves, and all three
are written up where they belong: **plugged in** (§1 — `devicectl` is happy over
the network and usbmuxd is not), **unlocked** (§1 — install works while locked,
launch does not), and **in the foreground** (§7 — a suspended app stops
accepting, and the symptom is a hang rather than a refusal).

A fourth belongs only here, and it is the one that will actually mislead you.

### The trap that gives you a green run against the wrong app

A simulator shares the Mac's network stack, so a gallery left running in one is
**also** listening on `127.0.0.1:8787`. Start the forwarder against that and it
cannot bind, exits, and the check connects to the SIMULATOR — twenty-five
assertions, all green, and not one byte of it went near the device.

That happened here on the first device run (2026-08-19). The only tell was a
402pt-wide window on an 834pt iPad, and it was nearly missed.

`tools/mcp_check_device.sh` now refuses to start when the port is already bound,
and checks the forwarder is still alive before running anything. If you build
your own loop, do both — a forwarder that failed is silent, and the app that
answers instead is the one you were trying to compare against.

The decisive question, if you are ever unsure which app answered: terminate it
**on the device** and ask again. A socket that keeps answering was never the
device's.

```
xcrun devicectl device process terminate --device <identifier> --pid <pid>
```

> **Status: verified on device, 2026-08-19.** iPad Pro 11-inch (3rd gen), iOS
> 26.6, over `pymobiledevice3 usbmux forward 8787 8787`. All 25 checks pass, and
> the run was confirmed to be the device's by the terminate test above.
>
> The window came up at **423.5 × 719** — iPadOS 26 gives an app a resizable
> window rather than the screen, so this run exercised the COMPACT width class,
> the same one a phone gets. The agent surface does not care; the LAYOUT under
> it does, and the regular-width tree an iPad uniquely reaches was not part of
> this run.
