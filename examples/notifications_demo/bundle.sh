#!/bin/sh
# Build the demo and wrap it in a .app, because a bare binary cannot ask.
#
# THIS SCRIPT IS WHAT MAKES THE PACKAGE WORK AT ALL, not packaging trivia.
#
# `UNUserNotificationCenter` refuses a process with no bundle identifier, and it
# refuses it by RAISING: `+currentNotificationCenter` throws rather than
# returning nil, and an unhandled ObjC exception aborts. That killed this
# package's own test runner with SIGABRT until the backend grew a
# `bundleIdentifier` guard — so a bare `cpc build` binary now answers
# `Unsupported` instead of dying, and this bundle is the only way to get past it.
#
# The plist below also carries no usage-description key for notifications,
# deliberately: Apple gates notifications on the prompt alone, unlike camera or
# contacts. The `permissions` package answers that domain, and this app asks it.
#
#   ./bundle.sh && open out/Notifications.app
#
# Resetting the permission between runs:
#   tccutil reset All dev.cplus.notificationsdemo
#
set -e
cd "$(dirname "$0")"

CPC="../../target/release/cpc"
APP="out/Notifications.app"

"$CPC" build

rm -rf out && mkdir -p "$APP/Contents/MacOS"

# NO PRIVACY KEYS HERE. Notifications is the one Apple domain gated on the
# prompt alone — there is no NSNotificationsUsageDescription and nothing to
# forget. What IS load-bearing is `CFBundleIdentifier`: without it there is no
# centre, and without a signature TCC cannot remember the answer across rebuilds.
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>Notifications</string>
	<key>CFBundleIdentifier</key><string>dev.cplus.notificationsdemo</string>
	<key>CFBundleName</key><string>Notifications</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>LSMinimumSystemVersion</key><string>12.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

cp target/debug/notifications_demo "$APP/Contents/MacOS/Notifications"

# AD-HOC SIGNED, and it matters more than it looks. TCC keys its decisions on
# code identity; an unsigned binary gets a fresh identity on every rebuild, so
# every run starts from Unknown and a `Denied` you were trying to look at is
# gone. `-` is the ad-hoc identity, which is stable enough for that.
codesign --force --sign - "$APP" >/dev/null 2>&1 || \
  echo "codesign unavailable — TCC decisions will not persist across rebuilds" >&2

echo "built $APP"
