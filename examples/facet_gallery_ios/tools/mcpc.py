#!/usr/bin/env python3
"""Line-delimited JSON-RPC client for facet's agent surface.

The wire is one request object per line and one response object per line, over a
stream socket — a Unix socket on macOS, a TCP port on loopback for iOS. See
vendor/agent_mcp/src/agent_mcp.cplus; the dispatch table there is the whole
protocol.
"""
import json, socket, sys

class MCP:
    def __init__(self, host="127.0.0.1", port=8787, timeout=10):
        self.s = socket.create_connection((host, port), timeout=timeout)
        self.f = self.s.makefile("rwb")
        self.id = 0
    def call(self, method, **params):
        self.id += 1
        req = {"method": method, "params": params, "id": self.id}
        self.f.write((json.dumps(req) + "\n").encode())
        self.f.flush()
        line = self.f.readline()
        if not line:
            raise RuntimeError("connection closed by peer (no response to %s)" % method)
        return json.loads(line)
    def close(self):
        self.f.close(); self.s.close()

def nodes(resp):
    r = resp.get("result") or {}
    return r.get("nodes", r if isinstance(r, list) else [])

if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8787
    m = MCP(port=port)
    r = m.call("describe_ui")
    print(json.dumps(r, indent=1)[:4000])
    m.close()
