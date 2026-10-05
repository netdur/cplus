# facet_agent_winui

Optional connector between `facet_winui`, `facet_agent` and `agent_winui`.
The native agent package remains independently usable; ordinary WinUI and Facet
WinUI apps do not pull in this connector unless they import it.

```cplus
import "facet_agent_winui/facet_agent_winui" as winui_agent;
import "facet_agent/agent" as agent;
import "facet_winui/facet_winui" as host;

winui_agent::enable(); // before host::run; do not also enable the HWND backend
let status = host::run(tree, "My app");
winui_agent::disable();
```

After the host's ready callback, `agent::in_app()` and
`agent::in_app_with_grant(grant)` select the WinUI surface. Keyed Facet nodes are
exposed; unkeyed nodes carry inherited privacy boundaries without entering the
curated list. Native replacement, policy updates and release keep pins current.
The window close hook detaches the surface before XAML teardown.

Copy the full dependency closure from
[examples/facet_agent_winui_smoke/Cplus.toml](../../examples/facet_agent_winui_smoke/Cplus.toml).
The connector uses `prebuild = false`, like `facet_winui`, so its renderer routing
hooks have one owner in the host executable.

To serve external MCP clients, call `winui_agent::enable("my-app")` before
`host::run`. Alternatively call `agent::serve_once("my-app")` from the host's
ready callback. `winui_agent::serve(id)` exposes the bound port or negative setup
error directly. Set `agent::set_policy(...)` before starting the listener; the
existing default grants ordinary operator access with privacy tiers enforced.

The optional [agent_winui_mcp](../agent_winui_mcp/README.md) transport binds
loopback HTTP, publishes discovery, and dispatches protocol/policy/native calls
on the UI thread. Closing the host cancels pending requests and joins the worker
before releasing the Surface. The Win32 Facet path keeps its existing backend.
Do not also run direct `handle_request` calls or a second MCP server alongside
this listener. Embedded `agent::in_app()` calls remain available on the UI thread.

Run `./tools/test_agent_winui.ps1` to verify both the standalone native backend and
the Facet connection. The Facet test checks callback delivery, editor readback,
private unkeyed containers, self-replacing buttons, and zero remaining native
views/subscriptions on exit.
Run `python tools/test_agent_winui_http.py` after building to verify external MCP
against both applications, including interrupted-request shutdown.
