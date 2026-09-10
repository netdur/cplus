#!/usr/bin/env bash
# Run permissions' iOS checks on the simulator, then drive TCC from outside and
# check the reads followed.
#
# `cpc test` builds a HOST binary and covers the table, both status maps and the
# whole request state machine on macOS. This covers what a host cannot:
# UserNotifications with a real bundle, UIApplication as the settings road, and
# dlopen paths answered by the dyld shared cache rather than by files.
#
#     vendor/permissions/tools/run_ios_tests.sh [device-udid]
#
# THE GRANT/REVOKE PASS is the half a test inside the process cannot do. `xcrun
# simctl privacy` writes the simulator's TCC database, so the app is launched
# three times — once as-is, once with contacts granted, once revoked — and the
# state this package reports has to change with it. That is the only evidence
# available here that `state` reads the real thing rather than a constant, and
# it needs no hands.
#
# `camera` is deliberately NOT part of that pass: simctl's service list has no
# camera entry, so the camera path stays hand-verified.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
runner="$(cd "$(dirname "${BASH_SOURCE[0]}")/../tests" && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64-simulator"
bundle_id="dev.cplus.permissionstests"

[ -x "$cpc" ] || { echo "build the compiler first: cargo build --release" >&2; exit 2; }

# ---- the device --------------------------------------------------------------
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

# ---- build + link ------------------------------------------------------------
( cd "$runner" && "$cpc" build --target "$triple" )

out="$(mktemp -d)"
app="$out/PermissionsTests.app"
mkdir -p "$app"

# THE USAGE-DESCRIPTION KEYS ARE LOAD-BEARING, and this bundle is where that
# stops being a documentation claim.
#
# `plans/permissions.md` §7 records the decision that `cpc` does not check them:
# the scaffold writes Info.plist and that is the end of it. So this plist is the
# first real test of "documented, not enforced" — every domain this runner reads
# has its key here, taken from the `plist_key` field of the row that needs it.
# Take one out and the process is killed by the OS on that read, which is
# exactly the failure the decision accepts.
#
# Notifications has no key: Apple gates it on the prompt alone.
cat > "$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>PermissionsTests</string>
	<key>CFBundleIdentifier</key><string>dev.cplus.permissionstests</string>
	<key>CFBundleName</key><string>PermissionsTests</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>LSRequiresIPhoneOS</key><true/>
	<key>MinimumOSVersion</key><string>15.0</string>
	<key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
	<key>UILaunchScreen</key><dict/>
	<key>NSCameraUsageDescription</key><string>Test harness: camera status read.</string>
	<key>NSMicrophoneUsageDescription</key><string>Test harness: microphone status read.</string>
	<key>NSPhotoLibraryUsageDescription</key><string>Test harness: photo library status read.</string>
	<key>NSPhotoLibraryAddUsageDescription</key><string>Test harness: photo add status read.</string>
	<key>NSContactsUsageDescription</key><string>Test harness: contacts status read.</string>
	<key>NSCalendarsFullAccessUsageDescription</key><string>Test harness: calendar status read.</string>
</dict>
</plist>
PLIST

# Every prebuilt dependency slice, not just this package's own archive — the
# same reason facet_uikit's runner globs them: `prebuild` is the default, so a
# dependency's object code lives in its own slice rather than in the app's
# archive, and an external-builder target hands the final link to us.
#
# NO -framework LINES FOR THE FOUR PERMISSION FRAMEWORKS, and that is the point
# rather than an omission. AVFoundation, Photos, Contacts, EventKit and
# UserNotifications are all reached by dlopen at runtime. If a link line ever
# appears here, the thing this package claims about launch cost has stopped
# being true and the check that asserts it has stopped meaning anything.
artifact_triple="arm64-apple-ios-simulator"
slices=$(find "$root/vendor" -maxdepth 4 -path "*/lib/$artifact_triple/*.a" | sort)
[ -n "$slices" ] || echo "warning: no prebuilt slices found for $artifact_triple" >&2

# shellcheck disable=SC2086 # word splitting is deliberate: one arg per slice
xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=15.0 \
  -I "$runner/target/$triple/debug" \
  "$runner/ios/main.m" "$runner/target/$triple/debug/libpermissions_tests.a" \
  $slices \
  -framework Foundation -framework UIKit -framework CoreFoundation -lobjc \
  -o "$app/PermissionsTests"

xcrun simctl terminate "$dev" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl install "$dev" "$app"

# One launch, captured. Prints the log and returns the failure count.
launch() {
  local log="$1"
  set +e
  xcrun simctl launch --console-pty "$dev" "$bundle_id" > "$log" 2>&1
  set -e
  cat "$log"
  local summary
  summary="$(grep -E '^selftest result:' "$log" || true)"
  if [ -z "$summary" ]; then
    echo "no selftest summary — the runner did not finish" >&2
    return 1
  fi
  local failed
  failed="$(echo "$summary" | sed -E 's/.*; ([0-9]+) failed.*/\1/')"
  [ "$failed" = "0" ] || { echo "$failed check(s) failed" >&2; return 1; }
  return 0
}

# The number this package reported for one domain, out of a run's log.
#
# `tr -d '\r'` is not cosmetic: `simctl launch --console-pty` gives a PTY, so
# every line ends CRLF and a `$`-anchored match finds nothing. It cost a run.
state_of() { tr -d '\r' < "$2" | sed -nE "s/^  state $1 = ([0-9]+)\$/\1/p" | head -1; }

echo "--- pass 1: as installed ---"
launch "$out/run1.txt"

echo "--- pass 2: contacts granted ---"
xcrun simctl privacy "$dev" grant contacts "$bundle_id"
launch "$out/run2.txt"

echo "--- pass 3: contacts revoked ---"
xcrun simctl privacy "$dev" revoke contacts "$bundle_id"
launch "$out/run3.txt"

granted="$(state_of contacts "$out/run2.txt")"
revoked="$(state_of contacts "$out/run3.txt")"
echo "contacts: granted=$granted revoked=$revoked"

# State::Granted is 1 and State::Denied is 3 — the codes in permissions.cplus.
# Asserted as VALUES rather than as "they differ", because two wrong numbers
# that happen to differ would pass the weaker check.
[ "$granted" = "1" ] || { echo "expected contacts=1 (Granted) after simctl grant, got '$granted'" >&2; exit 1; }
[ "$revoked" = "3" ] || { echo "expected contacts=3 (Denied) after simctl revoke, got '$revoked'" >&2; exit 1; }

echo "all checks passed, and TCC round-tripped"
