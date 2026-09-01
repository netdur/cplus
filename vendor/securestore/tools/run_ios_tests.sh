#!/usr/bin/env bash
# Run securestore's checks on the iOS simulator.
#
# `cpc test` builds a HOST binary, so the Apple backend it exercises is the
# macOS keychain — a different keychain with different rules. macOS has two and
# a code-identity ACL; iOS has one, always data-protection, scoped to an access
# group from the app's entitlements.
#
#     vendor/securestore/tools/run_ios_tests.sh [device-udid]
#
# ---- three things that each cost an hour, written down -----------------------
#
# 1. THE APP MUST BE SIGNED WITH `keychain-access-groups`. Without it every verb
#    answers -34018 errSecMissingEntitlement. No provisioning profile is needed
#    for this on a simulator — an ad-hoc signature carrying the entitlement is
#    enough, and the group is just the bundle id.
#
# 2. UNINSTALL BEFORE INSTALLING. Installing over an app whose ENTITLEMENTS have
#    changed keeps the old ones, and the launch is then refused with no process
#    and nothing in the log — indistinguishable from every other failure here.
#
# 3. `simctl launch` ALWAYS REPORTS FAILURE for this runner, and it is lying.
#    `main` returns instead of entering a run loop, and SpringBoard calls a
#    process that exits immediately a failed launch: "denied by service
#    delegate", stdout discarded. The process ran — the system log shows its
#    `SecItem*` calls. So the runner writes its report into its own container
#    and this reads it back from there.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
runner="$(cd "$(dirname "${BASH_SOURCE[0]}")/../tests" && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64-simulator"
bundle_id="dev.cplus.securestoretests"

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

out="$(mktemp -d)"
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
xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=14.0 \
  -I "$runner/target/$triple/debug" \
  "$runner/ios/main.m" \
  "$runner/target/$triple/debug/libsecurestore_tests.a" \
  $(cd "$runner" && "$cpc" build --target "$triple" --print-link-args) \
  -framework Foundation -lobjc \
  -o "$app/SecureStoreTests"

cat > "$out/entitlements.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>keychain-access-groups</key>
	<array><string>$bundle_id</string></array>
</dict>
</plist>
PLIST
codesign --force --sign - --entitlements "$out/entitlements.plist" "$app"
# `codesign` reports success for a signature carrying no entitlements at all,
# and the install then logs "had no entitlements" three layers from the -34018
# it causes.
codesign -d --entitlements - "$app" 2>&1 | grep -q "keychain-access-groups" \
  || { echo "entitlements did not embed" >&2; exit 2; }

xcrun simctl terminate "$dev" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl uninstall "$dev" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl install "$dev" "$app"

# Failure here is expected — see note 3 at the top. The report is the truth.
set +e
xcrun simctl launch --console "$dev" "$bundle_id" >/dev/null 2>&1
set -e

log="$out/log.txt"
container="$(xcrun simctl get_app_container "$dev" "$bundle_id" data 2>/dev/null || true)"
report="$container/Documents/securestore_result.txt"
[ -f "$report" ] || { echo "the runner produced no report — it did not run" >&2; exit 1; }
cat "$report"

summary="$(grep -E 'securestore result:' "$report" || true)"
[ -n "$summary" ] || { echo "no result line — the runner did not finish" >&2; exit 1; }
if grep -q "keychain unreachable" "$report"; then
  echo
  echo "PARTIAL: the keychain was out of reach, so STORAGE IS UNVERIFIED." >&2
  exit 3
fi
failed="$(echo "$summary" | sed -E 's/.*: ([0-9]+) failed.*/\1/')"
[ "$failed" = "0" ] || { echo "$failed check(s) failed" >&2; exit 1; }
echo "all checks passed"
