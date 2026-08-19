#!/usr/bin/env bash
# Run facet_uikit's checks on a PAIRED iPad, not the simulator.
#
# The sibling `run_ios_tests.sh` runs the same checks under `simctl`. This one
# exists because some of what this backend gets wrong is only wrong on real
# hardware — a compatibility-mode canvas, a touch that a simulator synthesises
# differently, a signed binary that will not launch — and because a device is
# the only place `userInterfaceIdiom` answers Pad for real.
#
#     vendor/facet_uikit/tools/run_ipad_tests.sh <devicectl-identifier> [bundle-id]
#
# THE BUNDLE ID IS AN ARGUMENT, and that is the awkward part rather than an
# oversight. A free provisioning team installs THREE apps at once and mints a
# profile only as a side effect of an Xcode build, so a runner with a bundle id
# of its own needs a project and a free slot. Passing an id that is ALREADY
# provisioned and installed reuses both: the runner ships in place of that app,
# and reinstalling the real one afterwards puts it back.
set -euo pipefail

dev="${1:-}"
bundle_id="${2:-dev.cplus.facetgalleryios}"
[ -n "$dev" ] || { echo "usage: run_ipad_tests.sh <devicectl-identifier> [bundle-id]" >&2; exit 2; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
runner="$(cd "$(dirname "${BASH_SOURCE[0]}")/../tests" && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64"
artifact_triple="arm64-apple-ios"
[ -x "$cpc" ] || { echo "build the compiler first: cargo build --release" >&2; exit 2; }

( cd "$runner" && "$cpc" build --target "$triple" )

out="$(mktemp -d)"
app="$out/FacetUIKitTests.app"
mkdir -p "$app"
# UIDeviceFamily [1,2] is load-bearing: without it an iPad runs the bundle in
# COMPATIBILITY MODE and the device checks pass while testing the phone path.
cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>FacetUIKitTests</string>
	<key>CFBundleIdentifier</key><string>${bundle_id}</string>
	<key>CFBundleName</key><string>FacetUIKitTests</string>
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

slices=$(find "$root/vendor" -maxdepth 4 -path "*/lib/$artifact_triple/*.a" | sort)
[ -n "$slices" ] || echo "warning: no prebuilt slices found for $artifact_triple" >&2

# shellcheck disable=SC2086
xcrun -sdk iphoneos clang -arch arm64 -mios-version-min=14.0 \
  -I "$runner/target/$triple/debug" \
  "$runner/ios/main.m" "$runner/target/$triple/debug/libfacet_uikit_tests.a" \
  $slices \
  -framework UIKit -framework QuartzCore -framework Foundation \
  -framework CoreGraphics -framework WebKit -lobjc \
  -o "$app/FacetUIKitTests"

# The profile that already covers this bundle id, and the entitlements out of
# it — a device build is refused without both, and a free profile carries the
# device list, so borrowing the id borrows its registration too.
prof=""
for f in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles/"*.mobileprovision; do
  [ -f "$f" ] || continue
  appid=$(security cms -D -i "$f" 2>/dev/null | plutil -extract Entitlements.application-identifier raw - 2>/dev/null || true)
  case "$appid" in *".$bundle_id") prof="$f"; break;; esac
done
[ -n "$prof" ] || { echo "no provisioning profile for $bundle_id — build that app in Xcode once" >&2; exit 1; }
cp "$prof" "$app/embedded.mobileprovision"
security cms -D -i "$prof" | plutil -extract Entitlements xml1 -o "$out/ent.plist" -

ident="$(security find-identity -v -p codesigning | grep -v REVOKED | grep "Apple Development" | head -1 | sed -E 's/.*\) ([0-9A-F]{40}) .*/\1/')"
[ -n "$ident" ] || { echo "no unrevoked Apple Development identity" >&2; exit 1; }
codesign --force --sign "$ident" --entitlements "$out/ent.plist" --timestamp=none "$app"

xcrun devicectl device install app --device "$dev" "$app" >/dev/null
log="$out/log.txt"
set +e
xcrun devicectl device process launch --device "$dev" --console "$bundle_id" > "$log" 2>&1
set -e
cat "$log"

summary="$(grep -E '^selftest result:' "$log" || true)"
if [ -z "$summary" ]; then
  echo "no selftest summary — the runner did not finish" >&2
  exit 1
fi
failed="$(echo "$summary" | sed -E 's/.*; ([0-9]+) failed.*/\1/')"
[ "$failed" = "0" ] || { echo "$failed check(s) failed" >&2; exit 1; }
echo "all checks passed on device"
