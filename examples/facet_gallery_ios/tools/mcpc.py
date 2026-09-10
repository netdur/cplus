#!/usr/bin/env python3
"""JSON-RPC client for facet's agent surface, over either door.

The MOBILE backend binds a TCP port on loopback and speaks **Streamable HTTP** —
one JSON-RPC object POSTed, one JSON object back. That is what any MCP client
reaches with no bridge written for it, which is the point of an app being an ACI
rather than something each tool needs a shim for. The desktop backend serves
that same door alongside a Unix socket that speaks the older line-delimited
framing.

This talks HTTP by default and keeps the line framing available for the socket.
See vendor/agent_mcp/src/agent_mcp.cplus; the dispatch table there is the whole
protocol, and one dispatch sits under both doors — a verb cannot exist on one
and not the other.

THE PORT IS DERIVED FROM THE PID: `9000 + pid % 1000`. It is not a number the
app chooses and not one you configure — a launcher that started the app knows
its pid, and `/tmp/mcp-<id>-<pid>.json` records what was actually bound for
anything that did not. On a device the same port arrives over usbmuxd
(`iproxy <port> <port> <UDID>`), and nothing here changes.
"""
import json, socket, sys, urllib.request

class MCP:
    def __init__(self, host="127.0.0.1", port=8787, timeout=10, http=True):
        self.http = http
        self.timeout = timeout
        self.url = "http://%s:%d/" % (host, port)
        self.id = 0
        if not http:
            self.s = socket.create_connection((host, port), timeout=timeout)
            self.f = self.s.makefile("rwb")

    def call(self, method, **params):
        self.id += 1
        req = {"jsonrpc": "2.0", "method": method, "params": params, "id": self.id}
        if self.http:
            r = urllib.request.Request(
                self.url,
                data=json.dumps(req).encode(),
                headers={"Content-Type": "application/json"},
            )
            return json.loads(urllib.request.urlopen(r, timeout=self.timeout).read())
        self.f.write((json.dumps(req) + "\n").encode())
        self.f.flush()
        line = self.f.readline()
        if not line:
            raise RuntimeError("connection closed by peer (no response to %s)" % method)
        return json.loads(line)

    def close(self):
        if not self.http:
            self.f.close(); self.s.close()

def nodes(resp):
    r = resp.get("result") or {}
    return r.get("nodes", r if isinstance(r, list) else [])

def port_for_pid(pid):
    return 9000 + pid % 1000

if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8787
    m = MCP(port=port)
    r = m.call("describe_ui")
    print(json.dumps(r, indent=1)[:4000])
    m.close()
