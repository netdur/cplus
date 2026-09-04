#!/usr/bin/env bash
# Run notifications' iOS checks on the simulator.
#
# `cpc test` builds a HOST binary with no bundle, and UNUserNotificationCenter
# refuses a process with no bundle identifier — it RAISES rather than returning
# nil, which killed this package's own test runner with SIGABRT before the
# backend grew a guard. So on the host every seam verb answers `Unsupported`
# and the framework half is untestable. This is a bundled app, which is the
# only configuration where the centre exists.
#
#     vendor/notifications/tools/run_ios_tests.sh [device-udid]
#
# NO GRANT/REVOKE PASS, unlike the permissions runner. `xcrun simctl privacy`
# has no notifications service — it covers contacts, photos, location and the
# rest, but notification authorisation is not TCC and cannot be written from
# outside. So the granted path needs a person, and it is
# `examples/notifications_demo` on a Mac. What runs here is everything up to it.
#
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
runner="$(cd "$(dirname "${BASH_SOURCE[0]}")/../tests" && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64-simulator"
bundle_id="dev.cplus.notificationstests"

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
app="$out/NotificationsTests.app"
mkdir -p "$app"

# NO PRIVACY KEYS. Notifications is the one Apple domain gated on the prompt
# alone — there is no NSNotificationsUsageDescription to forget. What IS
# load-bearing here is `CFBundleIdentifier`: without it there is no centre, and
# the checks below would all report `Unsupported` correctly and prove nothing.
cat > "$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>NotificationsTests</string>
	<key>CFBundleIdentifier</key><string>dev.cplus.notificationstests</string>
	<key>CFBundleName</key><string>NotificationsTests</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>LSRequiresIPhoneOS</key><true/>
	<key>MinimumOSVersion</key><string>15.0</string>
	<key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
	<key>UILaunchScreen</key><dict/>
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
  "$runner/ios/main.m" "$runner/target/$triple/debug/libnotifications_tests.a" \
  $slices \
  -framework Foundation -framework UIKit -framework CoreFoundation -lobjc \
  -o "$app/NotificationsTests"

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

launch "$out/run.txt"
echo "all checks passed"
