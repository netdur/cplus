#!/usr/bin/env bash
# Adding a FILE must not change what a program does. Build one project twice,
# once with `macos/Info.plist` and once without, and check both halves.
#
#     vendor/permissions/tools/run_embedded_plist_probe.sh
#
# WHY THIS CANNOT BE A `cpc test` CASE, which is the whole reason it exists as a
# script. `cpc test` links a bare binary and `run_test` does not embed
# `macos/Info.plist` — only `build_project` does — so the process running the
# suite is permanently the "no section" row and can never be made into the row
# that crashes. The bug is a property of a LINK, so it takes a build to see.
#
# WHAT WENT WRONG, so a future reader knows what is being defended. Both
# packages guarded `+[UNUserNotificationCenter currentNotificationCenter]` —
# which RAISES rather than returning nil for a process with no bundle proxy — by
# asking whether `[[NSBundle mainBundle] bundleIdentifier]` was non-nil. That
# was the same question right up until `cpc build` learned to embed
# `macos/Info.plist` into `__TEXT,__info_plist`: an embedded plist gives a BARE
# binary an identifier and does NOT give it a bundle proxy. The guard then said
# yes on exactly the process it was written to stop, and the program died with
# SIGABRT inside `permissions::state` — before `notifications` was ever reached.
# `objc/bundle` carries the four measured rows and the predicate that replaced
# it. iris/gaps/an-embedded-plist-turns-a-guarded-refusal-into-sigabrt.txt.
#
# BOTH HALVES ARE ASSERTED, and the second one is the point:
#
#     survives      neither build may abort — this is the regression
#     is honest     both must still answer `Unsupported`, because an embedded
#                   plist does NOT give a bare binary a notification centre.
#                   A "fix" that made the crash go away by claiming success
#                   would pass the first half and fail this one.
#
# The unit tests in `objc/bundle` do NOT catch this on their own — measured by
# ablation: replacing the predicate with the old `bundleIdentifier` question
# leaves all six of them green and brings the abort straight back. This script
# is what fails.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cpc="$root/target/release/cpc"
[ -x "$cpc" ] || { echo "no local cpc at $cpc — run: cargo build --release" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src" "$work/macos"

cat > "$work/Cplus.toml" <<'TOML'
[package]
name    = "plistprobe"
version = "0.0.1"
edition = "2026"
entry   = "src/main.cplus"

[dependencies]
stdlib        = "*"
facet         = "*"
flex_layout   = "*"
events        = "*"
notifications = "*"
permissions   = "*"

[macos.dependencies]
objc = "*"
TOML

# The two calls in the order an application makes them: `state` is what a
# notification-aware app reads first, and it is where the abort landed.
cat > "$work/src/main.cplus" <<'CPLUS'
import "stdlib/io" as io;
import "permissions/permissions" as permissions;
import "notifications/notifications" as notify;

fn _name(o: notify::Outcome) -> str {
    return match o {
        notify::Outcome::Ok => "Ok",
        notify::Outcome::InvalidInput => "InvalidInput",
        notify::Outcome::NotPermitted => "NotPermitted",
        notify::Outcome::Unsupported => "Unsupported",
        notify::Outcome::Failed => "Failed",
    };
}

fn main() -> i32 {
    let s: permissions::State = permissions::state(of: permissions::NOTIFICATIONS);
    io::println("state-read-ok");
    let n: notify::Notification = notify::Notification::new(id: "t1", title: "Title");
    io::print("schedule=");
    io::println(_name(notify::schedule(n)));
    return 0 as i32;
}
CPLUS

plist="$work/macos/Info.plist"
cat > "$work/plist.keep" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>dev.cplus.plistprobe</string>
</dict></plist>
PLIST

fail=0

run_row() {
    local label="$1"
    ( cd "$work" && "$cpc" build ) > "$work/build.log" 2>&1 \
        || { echo "FAIL  $label: build failed"; sed -n '1,20p' "$work/build.log"; fail=1; return; }

    local out rc
    set +e
    out="$( "$work/target/debug/plistprobe" 2>&1 )"
    rc=$?
    set -e

    if [ "$rc" -ne 0 ]; then
        echo "FAIL  $label: exit $rc — the process did not survive"
        echo "$out" | sed -n '1,3p'
        fail=1
        return
    fi
    if ! grep -q 'state-read-ok' <<<"$out"; then
        echo "FAIL  $label: never reached the end of permissions::state"
        fail=1
        return
    fi
    # THE INVERTED HALF. An embedded plist is not a bundle, so the honest answer
    # is still `Unsupported` — `Ok` here would mean the guard had been widened
    # into a lie rather than corrected.
    if ! grep -q 'schedule=Unsupported' <<<"$out"; then
        echo "FAIL  $label: expected schedule=Unsupported, got: $(grep '^schedule=' <<<"$out")"
        fail=1
        return
    fi
    echo "ok    $label: survived, schedule=Unsupported"
}

rm -f "$plist"
run_row "without macos/Info.plist"

cp "$work/plist.keep" "$plist"
run_row "with    macos/Info.plist"

if [ "$fail" -ne 0 ]; then
    echo
    echo "An embedded Info.plist changed what this program does. See objc/bundle."
    exit 1
fi
echo
echo "Both rows agree: adding macos/Info.plist changed nothing."
