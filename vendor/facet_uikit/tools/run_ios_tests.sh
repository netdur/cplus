#!/usr/bin/env bash
# Run facet_uikit's checks on the iOS simulator.
#
# `cpc test` builds a HOST binary and macOS has no UIKit, so this package's
# checks cannot run the ordinary way. This builds the runner in
# ../tests, links it against the simulator SDK, installs it
# and launches it — and exits non-zero when anything failed.
#
#     vendor/facet_uikit/tools/run_ios_tests.sh [device-udid]
#
# With no argument it picks the first booted simulator, and boots one if none
# is running.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
runner="$(cd "$(dirname "${BASH_SOURCE[0]}")/../tests" && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64-simulator"
bundle_id="dev.cplus.facetuikittests"

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
app="$out/FacetUIKitTests.app"
mkdir -p "$app"
cat > "$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>FacetUIKitTests</string>
	<key>CFBundleIdentifier</key><string>dev.cplus.facetuikittests</string>
	<key>CFBundleName</key><string>FacetUIKitTests</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>LSRequiresIPhoneOS</key><true/>
	<key>MinimumOSVersion</key><string>14.0</string>
	<key>UILaunchScreen</key><dict/>
</dict>
</plist>
PLIST

xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=14.0 \
  -I "$runner/target/$triple/debug" \
  "$runner/ios/main.m" "$runner/target/$triple/debug/libfacet_uikit_tests.a" \
  -framework UIKit -framework QuartzCore -framework Foundation \
  -framework CoreGraphics -framework WebKit -lobjc \
  -o "$app/FacetUIKitTests"

# ---- run ---------------------------------------------------------------------
xcrun simctl terminate "$dev" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl install "$dev" "$app"

log="$out/log.txt"
set +e
xcrun simctl launch --console-pty "$dev" "$bundle_id" > "$log" 2>&1
set -e
cat "$log"

# The runner returns the failure count, but `simctl launch` reports its own
# status — so the SUMMARY LINE is what decides, and a missing one is a failure
# too (the process died before it could print).
summary="$(grep -E '^selftest result:' "$log" || true)"
if [ -z "$summary" ]; then
  echo "no selftest summary — the runner did not finish" >&2
  exit 1
fi
failed="$(echo "$summary" | sed -E 's/.*; ([0-9]+) failed.*/\1/')"
[ "$failed" = "0" ] || { echo "$failed check(s) failed" >&2; exit 1; }
echo "all checks passed"
