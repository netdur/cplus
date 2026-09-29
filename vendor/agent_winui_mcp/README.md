# agent_winui_mcp

Optional HTTP transport for a standalone `agent_winui::Surface`. No Facet or
Win32 control backend is used. Add `agent_winui_mcp`, `agent_mcp` and `json` to
the app's dependency closure alongside the native agent dependencies.

```cplus
import "agent_winui_mcp/agent_winui_mcp" as http;
import "agent_core/auth" as auth;

fn policy(request: auth::Request) -> auth::Grant { return auth::operator(); }

// On the Window's UI thread, with its own DispatcherQueue:
let port: i32 = http::start(surface_ptr, queue, "my-app", policy);
// A positive port means http://127.0.0.1:<port>/ is listening.
// Before closing the Surface or shutting down XAML, on that same UI thread:
http::stop();
```

`start` accepts an optional `port: u16`; zero selects the shared PID-derived
port. It binds before returning and writes the shared discovery descriptor.
Errors are -1/-2/-3 for socket/bind/listen, -10 for an active server or a
reentrant start, -11 for an invalid id, -12 for invalid UI thread/queue/input,
and -13 for failure to publish discovery. `running()` reports lifecycle state.

One server can run per process, matching shared MCP protocol state. The caller
owns the Surface and must keep it alive through `stop()`. Start, stop and status
calls belong to the owning UI thread. The worker handles sockets and framing;
the DispatcherQueue handles protocol dispatch, the authorization policy and all
native verbs. Do not run another MCP transport or call `handle_request` directly
while this server is serving; embedded `agent_inapp` calls on the UI remain valid.

Stop cancels and joins the worker, closes all connections, and removes discovery.
It works with an incomplete HTTP request and with a request queued behind a busy
UI callback. Queued callbacks carry a server generation, not borrowed Surface or
worker pointers; callbacks left behind by a stopped server cannot act after a
restart. If an action or policy calls stop reentrantly, that current dispatch
still unwinds on the UI thread: keep the Surface allocation alive until it
returns, and restart from a later UI callback. The Facet connector owns a stable
Surface allocation and performs this teardown automatically.

The transport inherits the shared MCP HTTP behavior: POST requests, JSON replies,
202 for notifications, and connection-scoped client identity over keep-alive.
Client names are self-reported policy inputs, not authenticated identities.
There is no bearer-token layer. The standalone API requires an explicit policy;
the Facet connector uses `facet_agent::effective_policy()`. The server binds only
to loopback. A partially received request occupies the single transport worker
until it completes, disconnects or the server stops; it does not block the UI.

Verify real native and Facet apps with:

```powershell
./tools/test_agent_winui.ps1
python tools/test_agent_winui_http.py
```

The HTTP harness checks MCP initialization/tools, notifications, Unicode edits,
version conflicts, interleaved client grants, Facet replacement, pending-request
cancellation/restart, interrupted body reads, and listener/discovery cleanup.
