#!/usr/bin/env bash
# Why did a notification land in Notification Centre with no banner? Ask macOS.
#
# The package can prove its half — `schedule` said Ok, the permission is
# Granted, the delegate is installed — and still nothing shows, because the
# decision to interrupt is made inside usernoted AFTER the request is accepted,
# and usernoted writes that decision to the unified log. This reads it back.
#
#     vendor/notifications/tools/why_quiet.sh [bundle-id] [window]
#     vendor/notifications/tools/why_quiet.sh dev.cplus.notificationsdemo 2h
#
# One line per notification the bundle posted in the window. The two that
# matter:
#
#     suppression=none            reason=disabled
#         no Focus mode; the banner was macOS's to show — look at the app's
#         alert style in System Settings > Notifications next
#     suppression=delay delivery  reason=mode configuration type
#         a Focus mode silenced it, and `mode=` names which. A mode silences
#         every app not on its allowed list, so "Slack shows banners" is a
#         memory from before the mode was on, not a comparison made under it
#
# Then the app's own half — authorization asked and granted, requests added —
# so a `hasError: 0` beside a `delay delivery` says plainly whose decision it was.
#
# TWO TRAPS this exists to step around. `log` can be a zsh builtin (`type log`),
# and then `log show` fails with "too many arguments" — so this calls
# /usr/bin/log. And `defaults read com.apple.ncprefs` is where every older
# write-up looks for the per-app alert style; on macOS 26 that plist is not
# maintained (a year stale on a Mac that posts notifications daily), so an
# app's absence from it means nothing.
#
# Measured 2026-08-31: every notification the demo posted resolved to
# `delay delivery` under a Focus mode whose allowed-application list was empty,
# and so did every Slack notification in the same hours. The package was not
# involved, and its Outcome was `Ok` — correctly: the notification was delivered.
set -euo pipefail

bundle="${1:-dev.cplus.notificationsdemo}"
window="${2:-2h}"
LOG=/usr/bin/log

echo "== Focus decisions for $bundle, last $window (a long window is a slow query) =="
"$LOG" show --last "$window" --info --style compact \
  --predicate "process == \"usernoted\" AND eventMessage CONTAINS \"Resolved event behavior\" AND eventMessage CONTAINS \"$bundle\"" 2>/dev/null \
  | grep 'Resolved event behavior' \
  | sed -E "s/^([0-9-]+ [0-9:.]+).*interruptionSuppression: ([^;]*); intelligentBehavior: [^;]*; resolutionReason: ([^;]*); activeModeUUID: ([^;]*);.*identifier: '([^']*)'; bundleIdentifier.*/\1  id=\5  suppression=\2  reason=\3  mode=\4/" \
  || echo "(no decisions: nothing from $bundle reached usernoted in this window)"

echo
echo "== the app's half: authorization and adds =="
"$LOG" show --last "$window" --info --style compact \
  --predicate "subsystem == \"com.apple.UserNotifications\" AND eventMessage CONTAINS \"$bundle\" AND (eventMessage CONTAINS \"authorization\" OR eventMessage CONTAINS \"notification request\")" 2>/dev/null \
  | grep -v '^Timestamp' | sed -E 's/^([0-9-]+ [0-9:.]+) Df [^ ]+ \[[^]]*\] /\1 /' | cut -c1-160 \
  || echo "(nothing: the app never reached the framework in this window)"

echo
echo "== the most recent decision for ANY app (is a mode active right now?) =="
"$LOG" show --last 3h --info --style compact \
  --predicate 'process == "usernoted" AND eventMessage CONTAINS "Resolved event behavior"' 2>/dev/null \
  | grep 'Resolved event behavior' | tail -1 \
  | sed -E "s/^([0-9-]+ [0-9:.]+).*interruptionSuppression: ([^;]*); intelligentBehavior: [^;]*; resolutionReason: ([^;]*); activeModeUUID: ([^;]*);.*bundleIdentifier: ([^;]*);.*/\1  \5  suppression=\2  reason=\3  mode=\4/" \
  || echo "(no notification from any app in the last 3 hours)"
