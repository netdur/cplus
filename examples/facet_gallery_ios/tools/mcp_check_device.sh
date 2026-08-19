#!/usr/bin/env bash
# Run the MCP check against the gallery on a REAL DEVICE, over USB.
#
#     tools/mcp_check_device.sh                 # the one connected device
#     tools/mcp_check_device.sh <identifier>    # or name it (the devicectl one)
#
# The app has to be BUILT, SIGNED and INSTALLED first — DEPLOYING.md §4 and §5.
# This does the three things after that: launch it, forward the port over
# usbmuxd, and run the SAME `mcp_check.py` the simulator runs. One script both
# sides is the point — what a device adds is a single hop, and a check that
# passes through it is the only evidence the hop works.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bundle_id="dev.cplus.facetgalleryios"
port="${PORT:-8787}"

# TWO IDENTIFIERS, one device, and mixing them up is a confusing hour.
# `devicectl` wants the UUID-shaped `identifier`; `iproxy` and `xcodebuild` want
# the 25-character `udid`. `devicectl list devices -j` carries both, so nothing
# here has to guess or cross-reference `xctrace`.
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
xcrun devicectl list devices -j "$tmp/d.json" >/dev/null 2>&1 \
  || { echo "devicectl failed" >&2; exit 2; }

read -r ident udid transport <<EOF
$(WANT="${1:-}" python3 - "$tmp/d.json" <<'PY'
import json, os, sys
want = os.environ.get("WANT") or ""
for x in json.load(open(sys.argv[1]))["result"]["devices"]:
    hw = x["hardwareProperties"]
    # `reality` is what separates a physical device from a simulator; both are
    # listed here and only one can be forwarded to.
    if hw.get("reality") != "physical":
        continue
    if want and want not in (x["identifier"], hw.get("udid")):
        continue
    print(x["identifier"], hw.get("udid"),
          x["connectionProperties"].get("transportType", "?"))
    break
PY
)
EOF
[ -n "${ident:-}" ] || { echo "no physical device — plug one in and unlock it" >&2; exit 2; }
echo "device $ident (udid $udid, transport $transport)"

# THE CABLE IS NOT OPTIONAL and does not announce itself. `devicectl` reaches a
# device over the network, so it is perfectly happy with no cable at all and
# reports `localNetwork` — while usbmuxd, which is what forwards a port, sees
# nothing. This is the check that says so instead of leaving you with a
# connection that times out.
if [ "$transport" != "wired" ]; then
  echo "device is on $transport, not USB — usbmuxd cannot forward to it." >&2
  echo "plug the cable in (and unlock the device), then run this again." >&2
  exit 2
fi

# THE PORT MUST BE FREE, and this check is not hygiene — it is the difference
# between a device run and a lie.
#
# A simulator shares the Mac's network stack, so a gallery left running in a
# simulator is ALSO listening on 127.0.0.1:$port. Start the forwarder against
# that and it fails to bind, exits, and `mcp_check.py` connects to the
# SIMULATOR — 25 checks, all green, and not one byte of it went near the
# device. That happened here on the first device run (2026-08-19) and the only
# tell was a 402pt-wide window on an 834pt iPad.
if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "port $port is already listening on this Mac — something else has it:" >&2
  lsof -nP -iTCP:"$port" -sTCP:LISTEN >&2
  echo "a simulator running the same app is the usual culprit; terminate it first." >&2
  exit 2
fi

if command -v iproxy >/dev/null 2>&1; then
  forward=(iproxy "$port" "$port" "$udid")
elif command -v pymobiledevice3 >/dev/null 2>&1; then
  forward=(pymobiledevice3 usbmux forward "$port" "$port")
else
  echo "no forwarder: brew install libimobiledevice, or pip install pymobiledevice3" >&2
  exit 2
fi

# FOREGROUND, and it matters: iOS suspends a backgrounded app and a suspended
# app stops accepting on its socket — a connect that hangs rather than one that
# is refused, which reads like a bug in the bridge.
xcrun devicectl device process launch --device "$ident" "$bundle_id" >/dev/null
sleep 2

"${forward[@]}" >/dev/null 2>&1 &
fwd=$!
# `wait` reaps it quietly — without that the shell announces "Terminated: 15" on
# the way out of a run that succeeded.
trap 'kill "$fwd" 2>/dev/null; wait "$fwd" 2>/dev/null; rm -rf "$tmp"' EXIT
sleep 2

# The second half of the same guard: a forwarder that could not bind has
# already EXITED, and a check run after it would be talking to whatever did
# bind. Ask whether it is still there rather than assuming.
kill -0 "$fwd" 2>/dev/null || { echo "the forwarder exited — it could not bind $port" >&2; exit 2; }

python3 "$here/mcp_check.py" "$port"
