"""External HTTP verification against actual standalone and Facet WinUI apps.

Build both smoke examples first (tools/test_agent_winui.ps1). Windows only.
"""
import http.client
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def rpc(conn, method, params=None, notification=False):
    request = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if not notification:
        request["id"] = 1
    conn.request("POST", "/", json.dumps(request, ensure_ascii=False).encode(),
                 {"Content-Type": "application/json"})
    response = conn.getresponse()
    body = response.read()
    assert response.status == (202 if notification else 200), (response.status, body)
    if notification:
        assert body == b""
        return None
    value = json.loads(body)
    assert "error" not in value, value
    return value["result"]


def connect(port, name="operator"):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    rpc(conn, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                            "clientInfo": {"name": name, "version": "1"}})
    return conn


def run(name):
    example = ROOT / "examples" / name
    log = example / "out" / "http.log"
    runtime = example / "out" / "runtime"
    with log.open("w", encoding="utf-8") as output:
        process = subprocess.Popen([str(runtime / (name + ".exe"))], cwd=runtime,
            env={**os.environ, "CPLUS_AGENT_HTTP_TEST": "1"}, stdout=output,
            stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
        connections = []
        try:
            deadline = time.monotonic() + 20
            while True:
                contents = log.read_text(encoding="utf-8", errors="replace")
                ready = re.search(r"HTTP READY (\d+)", contents)
                if ready:
                    break
                assert process.poll() is None, contents
                assert time.monotonic() < deadline, contents
                time.sleep(.05)
            port = int(ready[1])
            surface_id = "agent-winui-http-test" if name == "agent_winui_smoke" else "facet-agent-winui-http-test"
            descriptor = Path(tempfile.gettempdir()) / f"mcp-{surface_id}-{process.pid}.json"
            assert descriptor.exists(), descriptor
            primary = connect(port)
            connections.append(primary)
            rpc(primary, "notifications/initialized", notification=True)
            tools = rpc(primary, "tools/list")
            assert "base_version" in json.dumps(tools)
            tree = rpc(primary, "describe_ui")
            assert "never-read-this" not in json.dumps(tree)
            assert "sensitive label" not in json.dumps(tree)
            edit = rpc(primary, "read_text", {"id": "edit"})
            version = edit["version"]
            text = "HTTP héllo 世界"
            write = rpc(primary, "tools/call", {"name": "set_text", "arguments": {
                "id": "edit", "value": text, "base_version": version}})
            assert "allowed" in json.dumps(write), write
            edited = rpc(primary, "read_text", {"id": "edit"})
            assert edited["text"] == text and edited["version"] > version, edited
            stale = rpc(primary, "set_text", {"id": "edit", "value": "stale", "base_version": version})
            assert stale["outcome"] == "version_conflict", stale
            if name == "agent_winui_smoke":
                reader = connect(port, "reader")
                connections.append(reader)
                assert rpc(reader, "click", {"id": "button"})["outcome"] == "needs_grant"
                assert rpc(primary, "click", {"id": "button"})["outcome"] == "allowed"
                # Revisit the first client after interleaving another session.
                assert rpc(reader, "click", {"id": "button"})["outcome"] == "needs_grant"
                assert rpc(primary, "click", {"id": "restart"})["outcome"] == "allowed"
                deadline = time.monotonic() + 5
                while "HTTP: blocking UI" not in log.read_text(encoding="utf-8"):
                    assert time.monotonic() < deadline
                    time.sleep(.01)
                # This request reaches the worker but cannot execute on the UI.
                try:
                    rpc(primary, "set_text", {"id": "edit", "value": "must not execute",
                                             "base_version": edited["version"]})
                    raise AssertionError("pending request survived server shutdown")
                except (ConnectionError, http.client.RemoteDisconnected):
                    pass
                deadline = time.monotonic() + 5
                while "PASS: HTTP restart" not in log.read_text(encoding="utf-8"):
                    assert time.monotonic() < deadline, log.read_text(encoding="utf-8")
                    time.sleep(.02)
                primary = connect(port)
                connections.append(primary)
                assert rpc(primary, "read_text", {"id": "edit"})["text"] == text
            else:
                for _ in range(2):
                    assert rpc(primary, "click", {"id": "increment"})["outcome"] == "allowed"
                    time.sleep(.1)  # native event replaces the keyed button
            try:
                rpc(primary, "click", {"id": "close"})
            except (ConnectionError, http.client.RemoteDisconnected):
                pass  # closing the window may cancel its own response
            partial = None
            if name == "facet_agent_winui_smoke":
                # The UI timer closes the host while the worker reads an
                # incomplete body. Cancellation must also interrupt socket I/O.
                partial = socket.create_connection(("127.0.0.1", port), timeout=3)
                partial.sendall(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000\r\n\r\n{")
            assert process.wait(timeout=5) == 0, log.read_text(encoding="utf-8")
            if partial is not None:
                partial.close()
            assert not descriptor.exists(), descriptor
            # Listener and discovery record must disappear at shutdown.
            with socket.socket() as probe:
                assert probe.connect_ex(("127.0.0.1", port)) != 0
            print(f"PASS: {name}: external MCP, Unicode, versions, grants and clean HTTP shutdown")
        finally:
            for conn in connections:
                conn.close()
            if process.poll() is None:
                process.kill()
                process.wait()
            print(log.read_text(encoding="utf-8", errors="replace"))


if __name__ == "__main__":
    for app in ("agent_winui_smoke", "facet_agent_winui_smoke"):
        run(app)
