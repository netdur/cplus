# facet_agent

The agent serving surface for facet apps: `enable()` before `app.run`, and
a connected agent can read the UI and act through it.

```toml
[dependencies]
facet_agent = "*"
# plus the agent stack it serves (agent_core/mcp/inapp + the platform
# overlay) — `cpc pm add . facet_agent` writes the closure
```

```cplus
import "facet_agent/agent" as agent;

fn main() -> i32 {
    agent::enable();                       // register the serving hooks
    var app: runtime::App = runtime::App::new("MyApp");
    app.agent_mcp("/tmp/myapp.sock");      // the MCP socket (a PORT on iOS)
    // ...
}
```

`import "facet_agent/agent"` resolves by filename override: `agent.cplus`
on desktop (drives agent_appkit), `agent_ios.cplus` on iOS (agent_uikit,
port transport). It installs into facet's application seam
(`application::install_agent`) — facet itself knows nothing about the
agent stack.

This is facet's OPTIONAL tier as a package boundary (2026-08-17): an app
that never imports facet_agent links none of the agent machinery, by
construction rather than by promise. The surface (enable/disable, the
policy translation from `vocab::Agent` tiers) is documented in
`vendor/facet/docs/ref.md` under "facet_agent/agent"; only the import path
moved.

Tests: `cd vendor/facet_agent && cpc test`. The suite compiles the serving
surface on the active platform — the `vocab::Agent` → policy translation
is live code no other build type-checks, and a wrong arm there is a card
number readable by every agent that connects.
