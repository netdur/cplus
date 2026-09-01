#!/usr/bin/env bash
# Run securestore's checks on a REAL iOS DEVICE, which is the only place they
# can actually store anything.
#
# WHY NOT THE SIMULATOR. iOS has one keychain, always the data-protection one,
# and it scopes every item to an access group taken from the app's
# `application-identifier` entitlement. No entitlement, no group, no storage —
# an unsigned or plainly-signed bundle answers -34018 errSecMissingEntitlement
# on every verb.
#
# And an ENTITLED bundle will not launch on a simulator. Entitlements have to be
# backed by a provisioning profile, a profile names the DEVICES it covers, and a
# simulator can never be one of them. Measured 2026-09-01 across both signing
# identities, four entitlement spellings, `simctl launch` and `simctl spawn`:
# "denied by service delegate" every time, even with a real profile's exact
# app-id. `tools/run_ios_tests.sh` runs the same checks on the simulator and
# reports PARTIAL for exactly this reason.
#
#     SECURESTORE_PROFILE=~/Library/Developer/Xcode/UserData/Provisioning\ Profiles/<uuid>.mobileprovision \
#     vendor/securestore/tools/run_device_tests.sh [device-identifier]
#
# THE BUNDLE ID IS THE PROFILE'S, not this package's: `application-identifier`
# must match exactly, so the id is read out of the profile rather than written
# down here. Xcode mints these; see examples/facet_gallery_ios/DEPLOYING.md.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
runner="$(cd "$(dirname "${BASH_SOURCE[0]}")/../tests" && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64"

[ -x "$cpc" ] || { echo "build the compiler first: cargo build --release" >&2; exit 2; }

prof="${SECURESTORE_PROFILE:-}"
if [ -z "$prof" ]; then
  # One is usually already there from any Xcode device build.
  prof="$(ls -t ~/Library/Developer/Xcode/UserData/Provisioning\ Profiles/*.mobileprovision 2>/dev/null | head -1 || true)"
fi
[ -n "$prof" ] || { echo "no provisioning profile — build any app for this device in Xcode once, then re-run" >&2; exit 2; }

out="$(mktemp -d)"
security cms -D -i "$prof" > "$out/prof.plist"
read -r appid bundle_id team expires <<EOF
$(python3 - "$out/prof.plist" <<'PY'
import plistlib, sys
d = plistlib.load(open(sys.argv[1], 'rb'))
e = d['Entitlements']
appid = e['application-identifier']
team = d['TeamIdentifier'][0]
print(appid, appid[len(team) + 1:], team, d['ExpirationDate'].isoformat())
PY
)
EOF
echo "profile   $appid  (expires $expires)"

dev="${1:-}"
if [ -z "$dev" ]; then
  dev="$(xcrun devicectl list devices -j "$out/d.json" >/dev/null 2>&1 && python3 - "$out/d.json" <<'PY'
import json, sys
# `hardwareProperties.reality` is the physical/simulated marker — every
# simulator also reports platform "iOS", so filtering on that alone picks one.
# Prefer a device whose tunnel is UP; fall back to any physical one so the
# failure is "could not reach it" rather than "no device".
best = None
for d in json.load(open(sys.argv[1]))["result"]["devices"]:
    if d.get("hardwareProperties", {}).get("reality") != "physical":
        continue
    if d.get("connectionProperties", {}).get("tunnelState") == "connected":
        print(d["identifier"]); break
    best = best or d["identifier"]
else:
    if best: print(best)
PY
)"
fi
[ -n "$dev" ] || { echo "no device — plug an iPhone/iPad in and pair it (DEPLOYING.md §1)" >&2; exit 2; }
echo "device    $dev"

ident="$(security find-identity -v -p codesigning 2>/dev/null \
        | grep -v CSSMERR | grep -oE '[0-9A-F]{40}' | head -1)"
[ -n "$ident" ] || { echo "no valid codesigning identity" >&2; exit 2; }
echo "identity  $ident"

( cd "$runner" && "$cpc" build --target "$triple" >/dev/null )

app="$out/SecureStoreTests.app"
mkdir -p "$app"
cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>SecureStoreTests</string>
	<key>CFBundleIdentifier</key><string>$bundle_id</string>
	<key>CFBundleName</key><string>SecureStoreTests</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>LSRequiresIPhoneOS</key><true/>
	<key>MinimumOSVersion</key><string>14.0</string>
	<key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
	<key>UILaunchScreen</key><dict/>
</dict>
</plist>
PLIST

# `--print-link-args` rather than a find over vendor/: it walks the same path
# the compiler links a host build with and brings the slices up to date first.
# DEPLOYING.md §0 explains why the find form silently missed store slices.
# shellcheck disable=SC2046 # word splitting is the point
xcrun -sdk iphoneos clang -arch arm64 -miphoneos-version-min=14.0 \
  -I "$runner/target/$triple/debug" \
  "$runner/ios/main.m" \
  "$runner/target/$triple/debug/libsecurestore_tests.a" \
  $(cd "$runner" && "$cpc" build --target "$triple" --print-link-args) \
  -framework Foundation -lobjc \
  -o "$app/SecureStoreTests"

cp "$prof" "$app/embedded.mobileprovision"
cat > "$out/entitlements.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>get-task-allow</key><true/>
	<key>application-identifier</key><string>$appid</string>
	<key>com.apple.developer.team-identifier</key><string>$team</string>
	<key>keychain-access-groups</key>
	<array><string>$appid</string></array>
</dict>
</plist>
PLIST
codesign --force --sign "$ident" --entitlements "$out/entitlements.plist" "$app"
codesign -d --entitlements - "$app" 2>&1 | grep -q "application-identifier" \
  || { echo "entitlements did not embed" >&2; exit 2; }

xcrun devicectl device install app --device "$dev" "$app" >/dev/null
log="$out/log.txt"
set +e
xcrun devicectl device process launch --device "$dev" --console "$bundle_id" > "$log" 2>&1
set -e
cat "$log"

summary="$(grep -E 'securestore result:' "$log" || true)"
[ -n "$summary" ] || { echo "no result line — the runner did not finish" >&2; exit 1; }
if grep -q "keychain unreachable" "$log"; then
  echo "PARTIAL: the keychain was out of reach even on device — check the profile" >&2
  exit 3
fi
failed="$(echo "$summary" | sed -E 's/.*: ([0-9]+) failed.*/\1/')"
[ "$failed" = "0" ] || { echo "$failed check(s) failed" >&2; exit 1; }
echo "all checks passed"
