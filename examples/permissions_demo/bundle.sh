#!/bin/sh
# Build the demo and wrap it in a .app, because a bare binary cannot ask.
#
# THIS SCRIPT IS THE DEMONSTRATION as much as the app is. macOS gates every
# privacy prompt on TCC, and TCC reads the usage-description key out of the
# app's Info.plist. A plain `cpc build` binary has no plist and no bundle
# identifier, so it can never prompt. Everything below exists to make the
# prompt possible.
#
#   ./bundle.sh && open out/Permissions.app
#
# Rebuilding after a denial: TCC remembers per bundle identifier, so
#   tccutil reset Camera dev.cplus.permissionsdemo
# puts a row back to Unknown. `tccutil reset All dev.cplus.permissionsdemo`
# resets every domain at once.
set -e
cd "$(dirname "$0")"

CPC="../../target/release/cpc"
APP="out/Permissions.app"

"$CPC" build

rm -rf out && mkdir -p "$APP/Contents/MacOS"

# THE KEYS ARE THE POINT. One per domain this demo reads, each taken from the
# `plist_key` field of the row that needs it in
# `vendor/permissions/src/permissions_backend.cplus`.
#
# DELETE ONE AND PRESS ITS ASK BUTTON: the app vanishes. Measured on macOS 26.6,
# and the shape of it is worth knowing before you meet it —
#
#   Termination Reason: Namespace TCC, Code 0
#   Thread 1 Crashed:: Dispatch queue: com.apple.root.default-qos
#     __TCC_CRASHING_DUE_TO_PRIVACY_VIOLATION__
#     __TCCAccessRequest_block_invoke
#
# — the READ is fine, only the REQUEST is fatal, and it is not fatal at the call
# site: `request` returns normally and TCC kills the process a moment later from
# a background queue. Nothing fails where you are looking.
#
# Notifications has no key — Apple gates it on the prompt alone — and reads
# `Unsupported` here anyway, because UNUserNotificationCenter wants a SIGNED
# bundle and this one is only assembled.
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>Permissions</string>
	<key>CFBundleIdentifier</key><string>dev.cplus.permissionsdemo</string>
	<key>CFBundleName</key><string>Permissions</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>LSMinimumSystemVersion</key><string>12.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSCameraUsageDescription</key><string>Demo: read and request camera access.</string>
	<key>NSMicrophoneUsageDescription</key><string>Demo: read and request microphone access.</string>
	<key>NSContactsUsageDescription</key><string>Demo: read and request contacts access.</string>
	<key>NSCalendarsFullAccessUsageDescription</key><string>Demo: read and request calendar access.</string>
	<key>NSPhotoLibraryUsageDescription</key><string>Demo: read and request photo library access.</string>
</dict>
</plist>
PLIST

cp target/debug/permissions_demo "$APP/Contents/MacOS/Permissions"

# AD-HOC SIGNED, and it matters more than it looks. TCC keys its decisions on
# code identity; an unsigned binary gets a fresh identity on every rebuild, so
# every run starts from Unknown and a `Denied` you were trying to look at is
# gone. `-` is the ad-hoc identity, which is stable enough for that.
codesign --force --sign - "$APP" >/dev/null 2>&1 || \
  echo "codesign unavailable — TCC decisions will not persist across rebuilds" >&2

echo "built $APP"
