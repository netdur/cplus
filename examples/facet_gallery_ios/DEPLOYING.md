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

### 0a. If the app needs an entitlement

Keychain, biometrics, app groups — anything with a capability. **On a simulator
the entitlement goes in the BINARY, not in the signature**, and the ad-hoc
signature has to stay plain. That is how Xcode builds for a simulator;
`securityd` reads the `__TEXT,__entitlements` section the linker embeds.

```
cat > /tmp/ent.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>keychain-access-groups</key>
	<array><string>dev.cplus.yourapp</string></array>
</dict>
</plist>
PLIST
xcrun derq query -f xml -i /tmp/ent.plist -o /tmp/ent.der --raw

# ...at the end of the clang line that links the app:
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __entitlements -Xlinker /tmp/ent.plist \
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __ents_der    -Xlinker /tmp/ent.der

codesign --force --sign - $S/Gallery.app      # PLAIN. no --entitlements.
```

Both sections, because Xcode embeds both: on the iOS 26.4 runtime the XML one
alone is what the keychain honours, and the DER twin alone answers `-34018`.

**`codesign --entitlements` is the trap.** A signature carrying entitlements
makes SpringBoard refuse the launch outright, and none of the errors mention
signing: `denied by service delegate (SBMainWorkspace)`, `Security policy
issue` from `simctl spawn`, `had no entitlements` in the install log. It reads
as "this needs a provisioning profile" and it is not that — **no profile, no
device and no Xcode project are involved.** Profiles are a device concern
(§1 onward).

Two more that cost the same day:

- **`simctl install` over an app whose entitlements changed keeps the OLD
  ones**, and the launch is then refused with no process and nothing in the
  log. `simctl uninstall` first.
- **`simctl launch` calls a fast-exiting process a failed launch** and discards
  its stdout. A test runner whose `main` returns rather than entering a run
  loop always gets "denied by service delegate" even when it ran to completion
  — the system log shows its work. Write the result into the app's container
  and read it back with `simctl get_app_container <dev> <id> data`.

`vendor/securestore/tools/run_ios_tests.sh` is the worked example. It asserts
both halves, the second inverted, because either silently reintroduces the bug:

```
otool -s __TEXT __entitlements "$app/Bin" | grep -q "Contents of"   # section IS there
codesign -d --entitlements - "$app" | grep -q keychain-access-groups \
  && { echo "the SIGNATURE carries entitlements — launch will be refused"; exit 2; }
```

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
can reach it — and to a device there is no shared filesystem at all.

**THE PORT IS DERIVED FROM THE PID: `9000 + pid % 1000`.** It is not a number
the app chooses and not one you configure. `agent_mcp("<name>")` takes an ID —
the app's name, which is what `serverInfo.name` reports and what the descriptor
is filed under — and the platform decides the address from it. A launcher that
started the app has the pid, so it can work the port out; anything else reads
`/tmp/mcp-<id>-<pid>.json`, which records what was actually bound.

The wire is **Streamable HTTP**: one JSON-RPC object POSTed, one JSON object
back, which is what an MCP client reaches with no bridge written for it. (The
desktop backend serves that door too, alongside a Unix socket that still speaks
the older line-delimited framing.)

**On the simulator there is nothing to forward.** A simulator shares the Mac's
network stack, so `127.0.0.1:<port>` on the Mac IS the app's loopback:

```
xcrun simctl launch $DEV dev.cplus.facetgalleryios      # prints the pid
tools/mcp_check.py $(( 9000 + PID % 1000 ))             # 28 checks
```

and to build the app for the simulator at all, the library the Xcode project
links has to be the simulator one:

```
cpc build --target ios-arm64-simulator
xcodebuild -project ios/Gallery.xcodeproj -scheme Gallery \
           -sdk iphonesimulator -configuration Debug \
           -destination "id=$DEV" build
```

The project picks the right archives per SDK — `CPLUS_TRIPLE` and
`CPLUS_TARGETDIR` are conditioned on `[sdk=iphonesimulator*]`, so the device
build is untouched. Before 2026-08-30 it named the device paths unconditionally
and a simulator build failed at the link with *"building for iOS-simulator, but
linking in object file built for iOS"*.

**On a device the port is reached over usbmuxd**, the same mechanism Flutter's
Dart VM Service and Chrome's remote debugging use:

```
tools/mcp_check_device.sh                    # launch + forward + check, in one
```

which is the three steps below, and says which of them your setup is missing
rather than leaving you with a socket that times out:

```
iproxy -u <25-char-UDID> <port>:<port>        # brew install libimobiledevice
# The positional form `iproxy <port> <port> <udid>` is the OLD syntax and now
# fails with "Invalid listen port specified in argument" — current
# libimobiledevice takes LOCAL:DEVICE pairs and the udid through -u.
pymobiledevice3 usbmux forward <port> <port> # or: pip install pymobiledevice3
tools/mcp_check.py <port>                    # the same script, unchanged
```

Three things have to be true, none of them announce themselves, and all three
are written up where they belong: **plugged in** (§1 — `devicectl` is happy over
the network and usbmuxd is not), **unlocked** (§1 — install works while locked,
launch does not), and **in the foreground** (§7 — a suspended app stops
accepting, and the symptom is a hang rather than a refusal).

A fourth belongs only here, and it is the one that will actually mislead you.

### The trap that gives you a green run against the wrong app

A simulator shares the Mac's network stack, so a gallery left running in one is
**also** listening on loopback. Start the forwarder against that port and it
cannot bind, exits, and the check connects to the SIMULATOR — every assertion
green, and not one byte of it went near the device.

Deriving the port from the pid narrows this a great deal — two instances collide
only when their pids agree mod 1000 — but it does not close it, and the failure
still looks exactly like success.

That happened here on the first device run (2026-08-19). The only tell was a
402pt-wide window on an 834pt iPad, and it was nearly missed.

`tools/mcp_check_device.sh` now refuses to start when the port is already bound,
and checks the forwarder is still alive before running anything. If you build
your own loop, do both — a forwarder that failed is silent, and the app that
answers instead is the one you were trying to compare against.

The decisive question, if you are ever unsure which app answered: terminate it
**on the device** and ask again. A socket that keeps answering was never the
device's.

**And derive the port from a pid you have just read, not one you read earlier.**
A relaunch — including the one `--console` does — gives a new pid and therefore
a new port, and the old one is refused rather than answered. usbmuxd says
`Error connecting to device: Connection refused` in the forwarder's own log,
while curl reports only `Recv failure: Connection reset by peer`, so the useful
message is in the log and not in the client. That cost a wrong diagnosis here on
2026-09-06 (the app looked suspended; it was serving on a different port).

```
xcrun devicectl device process terminate --device <identifier> --pid <pid>
```

### The whole device sequence, in one block

Verified on a physical iPad, **2026-09-06** — iPad Pro 11-inch (3rd gen),
iPadOS 26.6.1. Copy this; every line of it is load-bearing and three of them are
things that were re-derived the hard way twice.

```sh
cd examples/facet_gallery_ios
# The CoreDevice id, by PATTERN and not by column: the Name field contains a
# space ("iPad (4)"), so $3 is the hostname and not the id.
DEV=$(xcrun devicectl list devices | grep physical \
      | grep -oE '[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}' | head -1)
UDID=$(xcrun xctrace list devices | sed -n 's/.*(\([0-9]\{8\}-[0-9A-F]\{16\}\)).*/\1/p' | head -1)

cpc build --target ios-arm64            # the DEVICE slice, not the simulator one
cd ios && xcodebuild -project Gallery.xcodeproj -target Gallery \
     -configuration Debug -destination "platform=iOS,id=$UDID" \
     -allowProvisioningUpdates build && cd ..
xcrun devicectl device install app --device $DEV ios/build/Debug-iphoneos/Gallery.app

# LAUNCH FIRST, THEN DERIVE THE PORT. Never the other way round.
xcrun devicectl device process launch --device $DEV --terminate-existing \
     dev.cplus.facetgalleryios
PID=$(xcrun devicectl device info processes --device $DEV \
      | grep "Gallery.app/Gallery" | awk '{print $1}' | head -1)
PORT=$(( 9000 + PID % 1000 ))

pkill -f iproxy
iproxy -u $UDID $PORT:$PORT &          # note: -u, and LOCAL:DEVICE
curl -s -X POST http://127.0.0.1:$PORT/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}'
```

A good answer names the app, and that is how you know it is the DEVICE talking:

```json
{"result":{"serverInfo":{"name":"facet_gallery_ios","version":"0.0.27"}}}
```

**Three failures and what each one actually is.** They are easy to confuse
because two of them produce the same message at the client.

| what you see | what it is |
|---|---|
| `Recv failure: Connection reset by peer`, and `Error connecting to device: Connection refused` in the FORWARDER's log | nothing is listening on that device port. Either a stale pid (see below) or the app is suspended |
| launch refused, "profile has not been explicitly trusted" | §6 — trust the certificate on the device. A re-minted profile needs it again |
| `Invalid listen port specified in argument` | the old `iproxy <port> <port> <udid>` syntax; use `-u UDID LOCAL:DEVICE` |

**The client tells you almost nothing.** `curl` reports a reset; the reason is
in the forwarder's own log and nowhere else. Always start `iproxy` with its
output somewhere you can read it.

**A relaunch moves the port.** The port is derived from the pid, so ANY relaunch
— including the one `--console` performs — gives a new one, and the old port is
refused. Re-read the pid after every launch. On 2026-09-06 this read as a
suspended app for several minutes; it was serving perfectly well on a port
nobody was asking.

**A backgrounded app REFUSES, it does not hang.** The section above says a
suspended app hangs. Measured here it answered `Connection refused` through
usbmuxd while `devicectl` still listed the process — the pid being alive is not
evidence the server is. Relaunching is the reliable fix and costs a port change.

### Seeing what the device is doing

```sh
xcrun devicectl device capture screenshot --device $DEV --destination /tmp/ipad.png
```

No developer disk image, no Xcode window. `idevicescreenshot` from
libimobiledevice needs the DDI mounted and fails with *"Could not start
screenshotr service: Invalid service"* until it is; `devicectl` does not.
There is a `capture screen-record` beside it for the same reason.

**Worth taking one even when MCP is answering.** The socket tells you what the
app believes; the screenshot tells you what the person sees, and the two
disagree in exactly the cases worth finding. A second window that had mounted
its tree into the FIRST window's content view read perfectly over MCP and was a
blank screen — the picture is what said so.

It also answers a question nothing else here does: whether the app is running
WINDOWED or full-screen on an iPad. The close/minimise/zoom pill in the corner
is the tell, and it decides what half the window tier even means.

### Driving it: what a click needs

```sh
P=$PORT
call(){ curl -s -X POST http://127.0.0.1:$P/ -H 'Content-Type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}"; }
call describe_ui '{}'
call hit_test '{"id":"row:button"}'
call click    '{"id":"row:button"}'
```

**Click the ROW, not the label inside it** — `row:button`, not `button`. That is
SKILL §9 trap 7 arriving through the socket: a gesture on something inside a row
is not the list's selection.

**And check `hidden` first.** A catalog row scrolled out of view reports
`hidden: true`, and `click` on one answers `{"outcome":"allowed"}` while nothing
happens — the call was legal, the row was not on screen. `hit_test` is the tool
that says so before you are confused by it:

```json
{"outcome":"allowed","supported":true,"reachable":true,"covered":false}
```

A successful pick is visible in the next `describe_ui`: `g:back` stops being
hidden, `g:title` changes, `g:catalog` hides, and the node count grows.

> **Status: verified on a DEVICE, 2026-09-06.** iPad Pro 11-inch (3rd gen),
> iPadOS 26.6.1, over usbmuxd. `initialize`, `tools/list`, `describe_ui`,
> `describe_tree`, `hit_test` and `click` all answered; a click on `row:button`
> drove a real navigation (title changed, catalog hid, node count 57 -> 63). The
> root frame read **1194 x 782** — full landscape width on this iPad, so the
> 402pt mis-size below did not recur. What this run did NOT exercise: the window
> cursor (`window::find`) and the in-place screen stack, because the gallery
> calls neither — it navigates by swapping its own outlet.
>
> **Status: verified on the SIMULATOR, 2026-08-30.** iPhone 16 Pro, iOS 18.5,
> straight to loopback. All 28 checks pass. This run is what caught three things
> a cross-build cannot: the app was serving on the pid-derived port while every
> doc said 8787, the harness still spoke the retired line framing, and list rows
> had stopped appearing in `describe_ui` when both backends moved to walking
> facet's tree. A device run has not been repeated since.
>
> **Status: verified on device, 2026-08-19.** iPad Pro 11-inch (3rd gen), iOS
> 26.6, over `pymobiledevice3 usbmux forward 8787 8787`. All 25 checks pass, and
> the run was confirmed to be the device's by the terminate test above.
>
> The window came up at **423.5 × 719** — iPadOS 26 gives an app a resizable
> window rather than the screen, so this run exercised the COMPACT width class,
> the same one a phone gets. The agent surface does not care; the LAYOUT under
> it does, and the regular-width tree an iPad uniquely reaches was not part of
> this run.
