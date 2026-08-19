#!/usr/bin/env python3
"""End-to-end MCP check against a RUNNING facet_gallery_ios.

    tools/mcp_check.py [port]        # 8787 by default

The one script for both targets, and that is the point of it. On the SIMULATOR
the app's loopback is the Mac's, so it connects straight to 127.0.0.1:8787. On a
DEVICE the same port arrives over usbmuxd — `iproxy 8787 8787 <UDID>` or
`pymobiledevice3 usbmux forward 8787 8787` — and nothing here changes, which is
how "the device answers the same as the simulator" becomes a thing you can check
rather than a thing you believe.

It drives the app: it navigates, types, and puts it back where it found it. Run
it against a gallery you are not also using by hand.
"""
import json, sys, time
from mcpc import MCP

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8787
fails = []
def check(name, ok, detail=""):
    print(("  PASS  " if ok else "  FAIL  ") + name + (("   " + str(detail)) if detail else ""))
    if not ok: fails.append(name)

m = MCP(port=PORT)
def ex():   return {n["id"]: n for n in m.call("describe_ui")["result"]}
def full(): return {n["id"]: n for n in m.call("describe_ui", mode="full")["result"]}

print("== transport + describe ==")
e = ex()
check("describe_ui answers the shell", {"app","g:title","g:catalog","g:outlet"} <= set(e))
check("the title reads back", e["g:title"]["text"] == "facet_uikit", e["g:title"]["text"])
rows = [k for k in e if k.startswith("row:")]
check("tree rows are named", len(rows) > 10, f"{len(rows)} rows")
check("rows are clickable", all(e[r]["clickable"] for r in rows))
f = full()
check("mode=full carries frames", f["g:catalog"]["frame"]["w"] > 0, f["g:catalog"]["frame"])
check("mode=full is the wider tree", len(f) > len(e), f"{len(f)} vs {len(e)}")

print("== click drives the app ==")
r = m.call("click", id="row:values")["result"]
check("click a catalog row", r["outcome"] == "allowed", r)
time.sleep(0.7)
e = ex()
check("the surface followed the swap", "c:slider" in e, sorted(k for k in e if k.startswith("c:")))
check("the app's own title changed", e["g:title"]["text"] == "Gallery", e["g:title"]["text"])
check("the catalog is now hidden", e["g:catalog"]["hidden"] is True)

print("== write, and the app sees it ==")
# describe -> act -> describe -> act. The surface is a snapshot that re-walks
# on describe, so an agent that navigates and then writes without looking is
# writing against the screen it left, and `t:name` is legitimately `not_found`
# there. That is the protocol, not a workaround for it.
m.call("click", id="row:fields"); time.sleep(0.7)
e = ex()
check("the fields demo is on the surface", "t:name" in e, sorted(k for k in e if k.startswith("t:")))
r = m.call("set_text", id="t:name", value="hello from mcp")["result"]
check("set_text allowed", r["outcome"] == "allowed", r)
time.sleep(0.7)
e = ex()
check("set_text reads back", e["t:name"]["text"] == "hello from mcp", repr(e["t:name"]["text"]))
check("the app's handler ran", e["t:echo"]["text"] != "(nothing typed)", repr(e["t:echo"]["text"]))
r = m.call("set_text", id="t:name", value="second write")["result"]
time.sleep(0.5)
check("a second write lands", ex()["t:name"]["text"] == "second write")

print("== the tier gate ==")
e = ex()
check("the secure field is gated", e["t:secure"]["readable"] is False, e["t:secure"])
check("its text is withheld", e["t:secure"]["text"] == "")

print("== reads that are not writes ==")
r = m.call("hit_test", id="t:name")["result"]
check("hit_test reaches a visible field", r["reachable"] is True, r)
r = m.call("read_runs", id="t:echo")["result"]
check("read_runs answers runs", r["outcome"] == "allowed" and len(r["runs"]) > 0, r)
r = m.call("scroll_to", id="t:area")["result"]
check("scroll_to (focus) allowed", r["outcome"] == "allowed", r)

print("== refusals say which ==")
check("unknown id", m.call("click", id="nope")["result"]["outcome"] == "not_found")
check("missing param", m.call("click")["error"]["code"] == -32602)
check("unknown method", m.call("frobnicate")["error"]["code"] == -32601)
r = m.call("invoke_menu", path="File/Open")["result"]
check("invoke_menu is unsupported, not a lie", r["outcome"] != "allowed", r)

print("== back to the catalog ==")
r = m.call("click", id="g:back")["result"]
check("the Back button clicks", r["outcome"] == "allowed", r)
time.sleep(0.7)
e = ex()
check("the catalog is back", e["g:catalog"]["hidden"] is False)
check("the title is back", e["g:title"]["text"] == "facet_uikit", e["g:title"]["text"])

m.close()
print()
print(("FAILED: " + ", ".join(fails)) if fails else "ALL CHECKS PASSED")
sys.exit(1 if fails else 0)
