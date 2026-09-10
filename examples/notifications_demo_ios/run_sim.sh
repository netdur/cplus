#!/usr/bin/env bash
# Build, bundle, install and launch the demo on an iOS simulator.
#
# An iOS target stops at object emission — Xcode normally owns the final link —
# so this does that link itself, the way facet_uikit's own probes do. For a
# device, see examples/facet_gallery_ios/DEPLOYING.md: same archive, plus an
# Xcode project for signing.
#
#     ./run_sim.sh [device-udid]
#
# The notification permission is NOT grantable from outside: `xcrun simctl
# privacy` covers contacts, photos and location, but notification authorisation
# is not TCC. So press Ask in the app and answer the system prompt — that part
# needs a person, which is the whole reason this demo exists.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64-simulator"
artifact_triple="arm64-apple-ios-simulator"
bundle_id="dev.cplus.notificationsdemo.ios"

[ -x "$cpc" ] || { echo "build the compiler first: cargo build --release" >&2; exit 2; }

dev="${1:-}"
if [ -z "$dev" ]; then
  dev="$(xcrun simctl list devices booted -j \
        | python3 -c 'import json,sys
d=json.load(sys.stdin)["devices"]
for rs in d.values():
    for x in rs:
        if x.get("state")=="Booted": print(x["udid"]); raise SystemExit')" || true
fi
if [ -z "$dev" ]; then
  dev="$(xcrun simctl list devices available -j \
        | python3 -c 'import json,sys
d=json.load(sys.stdin)["devices"]
for k,rs in d.items():
    if "iOS" not in k: continue
    for x in rs:
        if x.get("isAvailable"): print(x["udid"]); raise SystemExit')"
  echo "booting $dev"
  xcrun simctl boot "$dev"
  xcrun simctl bootstatus "$dev" -b >/dev/null
fi
echo "device $dev"

( cd "$here" && "$cpc" build --target "$triple" )

out="$here/build"
app="$out/NotificationsDemo.app"
rm -rf "$out"; mkdir -p "$app"
cp "$here/ios/Info.plist" "$app/Info.plist"

# Every prebuilt slice, not a hand-kept list: `prebuild` is the default, so a
# dependency's object code lives in vendor/<pkg>/lib/<triple>/ rather than in
# this package's archive. Over-linking is deliberate — an archive nothing
# references contributes nothing — and a glob cannot drift out of step with the
# manifest. bash 3.2 on macOS has no mapfile.
slices=()
while IFS= read -r line; do slices+=("$line"); done < <(
  find "$root/vendor" -maxdepth 4 -path "*/lib/$artifact_triple/*.a" | sort)

# NO -framework FOR UserNotifications, and that is the point rather than an
# omission: the notifications package dlopen's it on first use, so an app that
# never posts one never loads it. If a link line for it ever appears here, that
# claim has stopped being true.
xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=15.0 \
  -I "$here/target/$triple/debug" \
  "$here/ios/main.m" "$here/target/$triple/debug/libnotifications_demo_ios.a" \
  "${slices[@]}" \
  -framework UIKit -framework QuartzCore -framework Foundation \
  -framework CoreGraphics -framework WebKit -lobjc \
  -o "$app/NotificationsDemo"

xcrun simctl terminate "$dev" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl install "$dev" "$app"
xcrun simctl launch "$dev" "$bundle_id"
echo
echo "Press Ask first — until the permission is Granted every button answers"
echo "NotPermitted. Then try 'In 5s' and leave the app in FRONT: the banner"
echo "still appears, which is the presentation delegate doing its job."
