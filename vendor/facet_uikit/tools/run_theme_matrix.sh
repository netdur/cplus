#!/usr/bin/env bash
# Drive the device's theme settings and assert what the app actually painted.
#
#     vendor/facet_uikit/tools/run_theme_matrix.sh [device-udid]
#
# The appearance flip was shipped on 2026-08-18 and verified by a probe and a
# screenshot — which is to say it was verified by someone looking, twice, and
# the first look was wrong. This is that verification as a check that can fail.
#
# ---- why it is an external driver and not a selftest --------------------------
#
# `tests/` runs in-process and could set `overrideUserInterfaceStyle` on a window
# to simulate a flip. That would test the OVERRIDE path — the one an app takes
# when it forces its own appearance — and the question here is about the SYSTEM
# path, which arrives as a trait change from outside the process. The two do not
# share the code that broke. So the setting is changed the way a user changes
# it, with `simctl ui`, and the app is a live one that reports what it sees.
#
# ---- what it can and cannot conclude -------------------------------------------
#
# UIKit's semantic colours re-resolve themselves on a flip with no help from
# facet, so a probe painted from semantic roles passes against a completely
# unwired implementation. Everything asserted below is therefore read off a
# FLATTENED value — an `adaptive` literal pair, or a gradient stop that reached
# a CGColor. The semantic swatch is asserted too, but as a POSITIVE CONTROL: it
# proves the harness is wired, and it is never evidence about facet.
#
# Phases B and C drive the contrast and Dynamic Type axes. Facet does not react
# to either yet — `font_scales` is a per-node opt-in rather than a theme-level
# default — so those phases assert only that THE TRAIT ARRIVED, and print the
# swatch readings as a baseline. That is the door requirement 3 walks through,
# and a check that failed today would just be a check someone disables.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
probe="$(cd "$(dirname "${BASH_SOURCE[0]}")/../themeprobe" && pwd)"
cpc="$root/target/release/cpc"
triple="ios-arm64-simulator"
artifact_triple="arm64-apple-ios-simulator"
bundle_id="dev.cplus.facetthemeprobe"

[ -x "$cpc" ] || { echo "build the compiler first: cargo build --release" >&2; exit 2; }

# ---- the device ----------------------------------------------------------------
dev="${1:-}"
if [ -z "$dev" ]; then
  dev="$(xcrun simctl list devices booted -j \
        | python3 -c 'import json,sys
d=json.load(sys.stdin)["devices"]
for rs in d.values():
    for x in rs:
        if x.get("state")=="Booted": print(x["udid"]); raise SystemExit')" || true
fi
[ -n "$dev" ] || { echo "no booted simulator — boot one, or pass a udid" >&2; exit 2; }
echo "device $dev"

# ---- restore the device on the way out -----------------------------------------
#
# These are DEVICE settings, not the app's: leaving a simulator pinned to
# accessibility-extra-extra-extra-large and Increase Contrast after a failed run
# would silently change what every later run and every manual look is testing.
# Captured before anything is touched, restored however this exits.
was_appearance="$(xcrun simctl ui "$dev" appearance 2>/dev/null || echo light)"
was_contrast="$(xcrun simctl ui "$dev" increase_contrast 2>/dev/null || echo disabled)"
was_content="$(xcrun simctl ui "$dev" content_size 2>/dev/null || echo large)"
app_pid_file="$(mktemp)"

cleanup() {
  [ -s "$app_pid_file" ] && kill "$(cat "$app_pid_file")" 2>/dev/null || true
  xcrun simctl terminate "$dev" "$bundle_id" >/dev/null 2>&1 || true
  case "$was_appearance" in light|dark) xcrun simctl ui "$dev" appearance "$was_appearance" >/dev/null 2>&1 || true;; esac
  case "$was_contrast" in enabled|disabled) xcrun simctl ui "$dev" increase_contrast "$was_contrast" >/dev/null 2>&1 || true;; esac
  case "$was_content" in unknown|unsupported|"") ;; *) xcrun simctl ui "$dev" content_size "$was_content" >/dev/null 2>&1 || true;; esac
  rm -f "$app_pid_file"
}
trap cleanup EXIT

# ---- build + link --------------------------------------------------------------
( cd "$probe" && "$cpc" build --target "$triple" )

out="$probe/build"
app="$out/FacetThemeProbe.app"
rm -rf "$out"; mkdir -p "$app"
cp "$probe/ios/Info.plist" "$app/Info.plist"

# Every prebuilt slice, not a hand-kept list: `prebuild` is the default, so a
# dependency's object code lives in vendor/<pkg>/lib/<triple>/ and not in this
# package's archive. Over-linking is deliberate — an archive nothing references
# contributes nothing, and a glob cannot drift out of step with the manifest.
# bash 3.2 on macOS has no mapfile.
slices=()
while IFS= read -r line; do slices+=("$line"); done < <(
  find "$root/vendor" -maxdepth 4 -path "*/lib/$artifact_triple/*.a" | sort)

xcrun -sdk iphonesimulator clang -arch arm64 -mios-simulator-version-min=14.0 \
  -I "$probe/target/$triple/debug" \
  "$probe/ios/main.m" "$probe/target/$triple/debug/libfacet_uikit_themeprobe.a" \
  "${slices[@]}" \
  -framework UIKit -framework QuartzCore -framework Foundation \
  -framework CoreGraphics -framework WebKit -lobjc \
  -o "$app/FacetThemeProbe"

# ---- launch --------------------------------------------------------------------
xcrun simctl terminate "$dev" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl uninstall "$dev" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl install "$dev" "$app"

# Start from a known side so phase A's first reading is not whatever the last
# run left behind.
xcrun simctl ui "$dev" appearance light >/dev/null
xcrun simctl ui "$dev" increase_contrast disabled >/dev/null
xcrun simctl ui "$dev" content_size large >/dev/null

log="$out/probe.log"
: > "$log"
# `--console-pty` never returns on its own: the app does not exit.
( xcrun simctl launch --console-pty "$dev" "$bundle_id" > "$log" 2>&1 & echo $! > "$app_pid_file" ) || true

# ---- reading a report ----------------------------------------------------------
#
# The probe prints a numbered block twice a second. Taking the LAST COMPLETE one
# — matched `TM begin N` / `TM end N` — is what keeps a half-written block from
# being parsed as a reading, and requiring N to have advanced past a recorded
# baseline is what keeps a STALE block from being read as the answer to a
# setting that was changed a moment ago. A fixed sleep does neither.
parse_py="$out/parse.py"
cat > "$parse_py" <<'PY'
import re, sys
lines = open(sys.argv[1], errors="replace").read().splitlines()
end = None
for i in range(len(lines) - 1, -1, -1):
    m = re.search(r"TM end (\d+)\s*$", lines[i])
    if m:
        end = (i, m.group(1)); break
if end is None:
    print("R_SEQ=0"); sys.exit(0)
i, n = end
start = None
for j in range(i, -1, -1):
    if re.search(r"TM begin %s " % n, lines[j]):
        start = j; break
if start is None:
    print("R_SEQ=0"); sys.exit(0)
blk = lines[start:i + 1]
out = {"R_SEQ": n}
for ln in blk:
    m = re.search(r"TM begin \d+ (\S+) pid=(\d+)", ln)
    if m: out["R_TAG"], out["R_PID"] = m.group(1), m.group(2)
    m = re.search(r"TM axis style=(-?\d+) contrast=(-?\d+) bold=(-?\d+) idiom=(-?\d+)", ln)
    if m:
        out["R_STYLE"], out["R_CONTRAST"] = m.group(1), m.group(2)
        out["R_BOLD"], out["R_IDIOM"] = m.group(3), m.group(4)
    m = re.search(r"TM csize (\S+)", ln)
    if m: out["R_CSIZE"] = m.group(1)
    m = re.search(r"TM ax darker=(\d+) motion=(\d+) transparency=(\d+) nocolor=(\d+) bold=(\d+)", ln)
    if m:
        out["R_DARKER"], out["R_MOTION"] = m.group(1), m.group(2)
        out["R_TRANSP"], out["R_NOCOLOR"], out["R_AXBOLD"] = m.group(3), m.group(4), m.group(5)
    m = re.search(r"TM color (\S+) (\S+) (\S+) (\S+) (\S+) (\S+)", ln)
    if m:
        k = m.group(1).upper()
        out["R_" + k] = ",".join(m.group(i) for i in range(2, 6))
        out["R_" + k + "_OK"] = m.group(6)
for k, v in out.items():
    print("%s=%s" % (k, v))
PY

fails=0
ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fails=$((fails + 1)); }
note() { printf '  --   %s\n' "$1"; }

# Wait for the app to REPORT A GIVEN VALUE, not merely to report again.
#
# THIS DISTINCTION IS THE WHOLE RELIABILITY OF THIS FILE. A trait change takes
# longer to reach the app than one report interval, so "the next block after the
# setting changed" is usually a block describing the state BEFORE it — which is
# how the first version of this script scored a correct implementation as four
# failures and nearly had the appearance flip reopened as a bug.
#
# Waiting on the value instead is also self-timing: it returns as soon as the
# change lands rather than after a sleep long enough for the worst case.
wait_until() {   # wait_until <var> <expected> <label> ; leaves R_* set
  local var="$1" want="$2" label="$3" tries=0 have=""
  while [ "$tries" -lt 80 ]; do
    eval "$(python3 "$parse_py" "$log")"
    eval "have=\${$var:-}"
    if [ "$have" = "$want" ]; then ok "$label ($want)"; return 0; fi
    tries=$((tries + 1)); sleep 0.25
  done
  bad "$label: waited 20s, $var is ${have:-<unset>}, wanted $want"
  return 1
}

wait_until_changed() {   # wait_until_changed <var> <old> <label>
  local var="$1" old="$2" label="$3" tries=0 have=""
  while [ "$tries" -lt 80 ]; do
    eval "$(python3 "$parse_py" "$log")"
    eval "have=\${$var:-}"
    if [ -n "$have" ] && [ "$have" != "$old" ]; then ok "$label ($old -> $have)"; return 0; fi
    tries=$((tries + 1)); sleep 0.25
  done
  bad "$label: waited 20s, $var never moved off ${old}"
  return 1
}

wait_for_first_report() {
  local tries=0
  while [ "$tries" -lt 80 ]; do
    eval "$(python3 "$parse_py" "$log")"
    if [ "${R_SEQ:-0}" -gt 0 ]; then return 0; fi
    tries=$((tries + 1)); sleep 0.25
  done
  echo "the app never reported — it did not start" >&2
  echo "---- log ----" >&2; tail -30 "$log" >&2
  return 1
}

assert_eq()  { if [ "$2" = "$3" ]; then ok "$1 ($2)"; else bad "$1: expected $3, got $2"; fi; }
assert_ne()  { if [ "$2" != "$3" ]; then ok "$1 ($3 -> $2)"; else bad "$1: did not change, both $2"; fi; }

# ---- phase A: the appearance flip ---------------------------------------------
echo
echo "phase A — the appearance flip"
wait_for_first_report || exit 1
a_pid="$R_PID"
a_bg="$R_BG"; a_text="$R_TEXT"; a_grad="$R_GRAD"; a_sem="$R_SEMANTIC"
assert_eq "starts on the light appearance" "$R_STYLE" "1"
assert_eq "the adaptive fill has a layer colour" "${R_BG_OK:-unset}" "ok"
assert_eq "the adaptive text has a colour" "${R_TEXT_OK:-unset}" "ok"
assert_eq "the gradient has a first stop" "${R_GRAD_OK:-unset}" "ok"

xcrun simctl ui "$dev" appearance dark >/dev/null
wait_until R_STYLE 2 "the dark setting reached the app as a trait" || true
# THE ASSERT THIS FILE EXISTS FOR. A relaunch would hand back a perfect-looking
# before and after taken from two different processes.
assert_eq "the app did not relaunch" "$R_PID" "$a_pid"
assert_ne "the adaptive fill followed the flip" "$R_BG" "$a_bg"
# The second bug, and the one that hid behind the first: a label's colour is a
# per-kind prop, so a repaint walk touching only the shared band left this
# frozen while the background above it correctly followed.
assert_ne "the adaptive TEXT colour followed the flip" "$R_TEXT" "$a_text"
assert_ne "the gradient stop followed the flip" "$R_GRAD" "$a_grad"
assert_ne "positive control: the semantic swatch moved" "$R_SEMANTIC" "$a_sem"

xcrun simctl ui "$dev" appearance light >/dev/null
wait_until R_STYLE 1 "the light setting reached the app as a trait" || true
assert_eq "still the same process" "$R_PID" "$a_pid"
# Back to the VALUE it started at, not merely different from dark: a walk that
# repainted with the wrong side would move on every flip and never come home.
assert_eq "the adaptive fill came back" "$R_BG" "$a_bg"
assert_eq "the adaptive text came back" "$R_TEXT" "$a_text"
assert_eq "the gradient came back" "$R_GRAD" "$a_grad"

# ---- phase B: increase contrast ------------------------------------------------
#
# The assert is on `UIAccessibilityDarkerSystemColorsEnabled()`, NOT on the
# trait, and that is a measured decision rather than a preference. On iOS 26.1
# `simctl ui <dev> increase_contrast enabled` leaves
# `UITraitCollection.accessibilityContrast` reading 1 ("normal") on a live app
# and on a cold start alike, while the accessibility predicate flips correctly.
# The trait value is still printed, so the day a runtime starts populating it
# the reading is already in the log.
echo
echo "phase B — increase contrast (the setting arrives; facet does not act on it yet)"
xcrun simctl ui "$dev" increase_contrast enabled >/dev/null
wait_until R_DARKER 1 "the contrast setting reached the app" || true
assert_eq "still the same process" "$R_PID" "$a_pid"
note "the trait meanwhile reads accessibilityContrast=$R_CONTRAST (1=normal, 2=high)"
note "swatches under high contrast: bg=$R_BG text=$R_TEXT grad=$R_GRAD semantic=$R_SEMANTIC"
note "facet has no contrast response yet — this is the baseline, not a pass"
xcrun simctl ui "$dev" increase_contrast disabled >/dev/null
wait_until R_DARKER 0 "contrast returns to normal" || true

# ---- phase C: Dynamic Type -----------------------------------------------------
echo
echo "phase C — Dynamic Type (the trait arrives; facet does not scale by default yet)"
base_csize="$R_CSIZE"
xcrun simctl ui "$dev" content_size accessibility-extra-extra-extra-large >/dev/null
wait_until_changed R_CSIZE "$base_csize" "the content size setting reached the app as a trait" || true
assert_eq "still the same process" "$R_PID" "$a_pid"
note "facet's Dynamic Type is a per-node opt-in (font_scales), not a theme default"
note "widening the filter in window.cplus is where requirement 3 starts — and a"
note "content-size change alters METRICS, so it needs a relayout, not a repaint"

# ---- summary -------------------------------------------------------------------
echo
if [ "$fails" -eq 0 ]; then
  echo "theme matrix: all checks passed"
else
  echo "theme matrix: $fails check(s) failed" >&2
  exit 1
fi
