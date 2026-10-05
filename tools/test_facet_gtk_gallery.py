"""Live GTK gallery regression check. Build examples/facet_gallery first.

Run on Linux with a display (or under xvfb-run). Exercises the real agent
worker/UI-thread boundary, repeated screen replacement, recycled rows,
table/carousel geometry, labels and pickers. Optional X11 input checks menus
and resizing.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import socket
import subprocess
import re
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
GALLERY = ROOT / "examples/facet_gallery"
DEMOS = {
    "overview": "Facet Gallery", "button": "Button", "label": "Label",
    "controls": "Controls", "values": "Values", "inputs": "Inputs",
    "pickers": "Pickers", "icons": "Icons", "images": "Image",
    "spans": "Spans", "graphics": "Graphics", "web": "Web",
    "layout": "Layout basics", "scroll": "Scroll", "split": "Split",
    "zstack": "ZStack", "responsive": "Responsive", "grid": "Grid",
    "placement": "Placement", "list": "List", "relist": "relist",
    "collection": "Collection", "tree": "Tree", "page_dots": "Page dots",
    "tabs": "Tabs", "menus": "Menus", "window_chrome": "Window chrome",
    "table": "Table", "carousel": "Carousel", "shadow": "Shadow",
    "brush": "Brush", "clip": "Clip, order and hit-testing",
    "anim_basics": "Basics", "anim_easing": "Easing",
    "anim_entrance": "Entrance", "anim_rules": "Rules",
    "refresh": "Refresh", "swipe": "Swipe",
    "a11y": "Accessibility and agent tiers",
}


class NativeInput:
    """Optional X11 checks; xwininfo/xprop and libXtst must be installed."""
    def __init__(self, pid, rpc):
        self.rpc = rpc
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        for candidate in re.findall(r'(0x[0-9a-f]+) "Facet Gallery"', tree):
            prop = subprocess.check_output(["xprop", "-id", candidate, "_NET_WM_PID"], text=True)
            if re.search(r'=\s*' + str(pid) + r'\b', prop):
                self.window = int(candidate, 16)
                break
        else:
            raise AssertionError(f"No X11 window for gallery PID {pid}")
        self.x = ctypes.CDLL("libX11.so.6")
        self.xt = ctypes.CDLL("libXtst.so.6")
        signatures = {
            "XOpenDisplay": ([ctypes.c_char_p], ctypes.c_void_p),
            "XCloseDisplay": ([ctypes.c_void_p], ctypes.c_int),
            "XFlush": ([ctypes.c_void_p], ctypes.c_int),
            "XRaiseWindow": ([ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
            "XSetInputFocus": ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong], ctypes.c_int),
            "XResizeWindow": ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint], ctypes.c_int),
            "XStringToKeysym": ([ctypes.c_char_p], ctypes.c_ulong),
            "XKeysymToKeycode": ([ctypes.c_void_p, ctypes.c_ulong], ctypes.c_uint),
        }
        for name, (args, result) in signatures.items():
            fn = getattr(self.x, name)
            fn.argtypes, fn.restype = args, result
        self.xt.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
        self.xt.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        self.xt.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        self.display = self.x.XOpenDisplay(None)
        assert self.display, "No X11 display"

    def bounds(self):
        info = subprocess.check_output(["xwininfo", "-id", hex(self.window)], text=True)
        return [int(re.search(name + r':\s+(-?\d+)', info)[1]) for name in
                ("Absolute upper-left X", "Absolute upper-left Y", "Width", "Height")]

    def activate(self):
        self.x.XRaiseWindow(self.display, self.window)
        self.x.XSetInputFocus(self.display, self.window, 2, 0)

    def point(self, x, y, button=1):
        self.activate()
        bx, by, width, height = self.bounds()
        prop = subprocess.check_output(["xprop", "-id", hex(self.window), "_GTK_FRAME_EXTENTS"], text=True)
        left, right, top, bottom = map(int, re.findall(r'\d+', prop.split('=')[-1]))
        root = self.rpc("describe_ui", {"mode": "full"})[0]["frame"]
        self.xt.XTestFakeMotionEvent(self.display, -1,
            bx + left + int(x * (width - left - right) / root["w"]),
            by + top + int(y * (height - top - bottom) / root["h"]), 0)
        self.xt.XTestFakeButtonEvent(self.display, button, 1, 0)
        self.xt.XTestFakeButtonEvent(self.display, button, 0, 0)
        self.x.XFlush(self.display)
        time.sleep(.15)

    def key(self, *names):
        codes = [self.x.XKeysymToKeycode(self.display, self.x.XStringToKeysym(n.encode())) for n in names]
        for code in codes:
            self.xt.XTestFakeKeyEvent(self.display, code, 1, 0)
        for code in reversed(codes):
            self.xt.XTestFakeKeyEvent(self.display, code, 0, 0)
        self.x.XFlush(self.display)
        time.sleep(.15)

    def resize(self, width, height):
        self.x.XResizeWindow(self.display, self.window, width, height)
        self.x.XFlush(self.display)
        time.sleep(.3)

    def close(self):
        self.x.XCloseDisplay(self.display)


def run(binary, rounds, native_input=False):
    env = {**os.environ, "FACET_GALLERY_AGENT": "1", "G_DEBUG": "fatal-criticals"}
    env.pop("FACET_GALLERY_WALK", None)
    for name in ("GTK_PATH", "GIO_MODULE_DIR", "GDK_PIXBUF_MODULE_FILE"):
        env.pop(name, None)
    with tempfile.TemporaryDirectory(prefix="facet-gallery-check-") as temp:
        log = Path(temp) / "gallery.log"
        with log.open("w") as output:
            process = subprocess.Popen([str(binary)], cwd=GALLERY, env=env,
                                       stdout=output, stderr=subprocess.STDOUT)
        endpoint = Path(f"/tmp/mcp-facet_gallery-{process.pid}.socket")
        conn = socket.socket(socket.AF_UNIX)
        conn.settimeout(15)
        try:
            deadline = time.monotonic() + 20
            while not endpoint.exists():
                assert process.poll() is None, log.read_text()
                assert time.monotonic() < deadline, log.read_text()
                time.sleep(.05)
            conn.connect(str(endpoint))
            stream = conn.makefile("rwb")
            serial = 0

            def rpc(method, params=None):
                nonlocal serial
                serial += 1
                stream.write((json.dumps({"jsonrpc": "2.0", "id": serial,
                    "method": method, "params": params or {}}) + "\n").encode())
                stream.flush()
                line = stream.readline()
                assert line, f"Gallery exited during {method} {params}: {log.read_text()}"
                reply = json.loads(line)
                assert "error" not in reply, reply
                return reply["result"]

            def click(key):
                result = rpc("click", {"id": key})
                assert result["outcome"] == "allowed", (key, result)

            def text(key):
                return rpc("read_text", {"id": key})["text"]

            def wait_text(key, expected):
                deadline = time.monotonic() + 3
                while True:
                    actual = text(key)
                    matches = expected(actual) if callable(expected) else actual == expected
                    if matches:
                        return actual
                    assert time.monotonic() < deadline, (key, expected, actual)
                    time.sleep(.02)

            def nodes():
                return {n["id"]: n for n in rpc("describe_ui", {"mode": "full"})}

            def frame(key):
                return nodes()[key]["frame"]

            def pane(key):
                click(key)
                deadline = time.monotonic() + 5
                while True:
                    rendered = rpc("describe_ui", {"mode": "full"})
                    if any(n["text"] == DEMOS[key] and n["frame"]["x"] >= 250
                           for n in rendered):
                        return
                    assert time.monotonic() < deadline, (key, DEMOS[key])
                    time.sleep(.03)

            rpc("initialize", {"clientInfo": {"name": "operator", "version": "1"}})
            deadline = time.monotonic() + 10
            while not any(n["id"] == "overview" for n in rpc("describe_ui", {"mode": "full"})):
                assert time.monotonic() < deadline, "Sidebar rows were never realised"
                time.sleep(.05)
            for turn in range(rounds):
                for demo, title in DEMOS.items():
                    pane(demo)
                print(f"Round {turn + 1}: all {len(DEMOS)} demo panes rendered", flush=True)

            pane("relist")
            assert frame("row:0")["h"] >= 40
            click("row:0:star")
            wait_text("rel:echo", "row 0 starred: true")
            click("rel:add")
            wait_text("rel:echo", "appended entry 200 — the list was told nothing")
            click("rel:grid")
            wait_text("rel:echo", "columns: 3")
            time.sleep(.1)
            grid = [frame(f"row:{i}") for i in range(3)]
            assert grid[0]["y"] == grid[1]["y"] == grid[2]["y"], grid
            assert grid[0]["x"] < grid[1]["x"] < grid[2]["x"], grid
            click("row:1:star")
            wait_text("rel:echo", "row 1 starred: true")
            click("rel:grid")
            click("rel:scroll")
            time.sleep(.15)
            click("rel:far")
            wait_text("rel:echo", lambda actual: actual.startswith("row:120: FOUND"))
            assert frame("row:120")["h"] >= 40

            pane("table")
            assert frame("tb:document")["h"] == 180
            assert frame("tb:host")["h"] >= 180
            click("tb:resize")
            wait_text("tb:height", "Rows: 52")
            time.sleep(.1)
            assert frame("tb:document")["h"] == 260
            click("tb:vary")
            wait_text("tb:uneven", "Original row heights")
            time.sleep(.1)
            assert frame("tb:alan")["y"] - frame("tb:grace")["y"] == 68
            click("tb:vary")
            time.sleep(.1)
            assert frame("tb:alan")["y"] - frame("tb:grace")["y"] == 52

            pane("carousel")
            host = frame("cr:host")
            for i, key in enumerate(("cr:first", "cr:second", "cr:third", "cr:last")):
                page = frame(key)
                assert page["w"] == host["w"] and page["h"] == host["h"], (key, host, page)
                assert page["x"] == host["x"] + i * host["w"], (key, host, page)
            click("cr:keep")
            click("cr:go_last")
            wait_text("cr:status", "Page 4 of 4")
            time.sleep(.3)
            assert frame("cr:last")["x"] == frame("cr:host")["x"]
            click("cr:go_first")
            wait_text("cr:status", "Page 1 of 4")
            click("cr:next")
            wait_text("cr:status", "Page 2 of 4")
            time.sleep(.3)
            assert frame("cr:second")["x"] == frame("cr:host")["x"]

            native = NativeInput(process.pid, rpc) if native_input else None
            if native:
                try:
                    _, _, width, height = native.bounds()
                    native.resize(width - 300, height)
                    smaller = frame("cr:host")
                    assert smaller["w"] < host["w"], (host, smaller)
                    assert frame("cr:second")["w"] == smaller["w"]
                    assert frame("cr:second")["x"] == smaller["x"]
                    assert text("cr:status") == "Page 2 of 4"
                    native.resize(width, height)
                    pane("menus")
                    context = frame("mn:context")
                    x, y = context["x"] + 90, context["y"] + 22
                    native.point(x, y, button=3)
                    assert any(n["text"] == "Inspect" for n in nodes().values())
                    native.point(x, y + 33)
                    wait_text("mn:status", "Action mn:inspect: 1 click(s)")
                    entry = frame("mn:scope")
                    native.point(entry["x"] + 30, entry["y"] + entry["h"] / 2)
                    native.key("Control_L", "i")
                    wait_text("mn:status", "Action mn:inspect: 2 click(s)")
                    click("mn:priority")
                    click("mn:placement")
                    native.key("Control_L", "i")
                    wait_text("mn:status", "Action mn:inspect: 3 click(s)")
                    click("mn:style")
                    native.key("Control_L", "i")
                    wait_text("mn:status", "Action mn:inspect: 4 click(s)")
                    print("Native right-click, scoped shortcut, command rebuild and carousel resize: passed", flush=True)
                finally:
                    native.close()
            else:
                pane("menus")
            assert frame("mn:context")["h"] == 140 and frame("mn:scope")["w"] > 100
            print("Relist rows, handlers, columns and scroll; table sizing; full-width carousel: passed", flush=True)

            click("label")
            expected = "Bold and italic · underlined & 🚀\nSecond line with strikethrough and code."
            wait_text("t:html", expected)
            runs = rpc("read_runs", {"id": "t:html"})
            assert runs["supported"], runs
            click("t:html-toggle")
            wait_text("t:html-status", "Literal text")
            assert "<br>" in text("t:html") and "<code>" in text("t:html")
            click("t:html-toggle")
            wait_text("t:html", expected)

            click("pickers")
            click("pk:time-open")
            wait_text("pk:time-status", "Opened 1, closed 0, selected 0")
            click("pk:time-open")
            assert text("pk:time-status") == "Opened 1, closed 0, selected 0"

            click("inputs")
            result = rpc("set_text", {"id": "in:name", "value": "GTK café 世界"})
            assert result["outcome"] == "allowed", result
            wait_text("in:name_echo", "Hello, GTK café 世界")
            result = rpc("set_caret", {"id": "in:name", "at": 9, "length": 0})
            assert result["outcome"] == "allowed", result
            result = rpc("hit_test", {"id": "in:name"})
            assert result["supported"] and result["reachable"], result
            assert process.poll() is None, log.read_text()
            print("HTML/literal transitions, picker callback, Unicode input, caret and hit test: passed")
        finally:
            conn.close()
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            contents = log.read_text(errors="replace")
            if contents:
                print(contents, end="")
        assert "Failed to set text" not in contents, contents


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=GALLERY / "target/debug/facet_gallery")
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--native-input", action="store_true", help="Also test X11 right-click, keyboard shortcuts and window resizing")
    args = parser.parse_args()
    assert args.rounds > 0
    run(args.binary.resolve(), args.rounds, args.native_input)
