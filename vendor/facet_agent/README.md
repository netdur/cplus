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

fn run() -> i32 {
    agent::enable();                 // register the serving hooks
    runtime::agent_mcp("myapp");     // serve under this NAME, on any tier
    runtime::run_screen(Home::new());
    return 0 as i32;
}
```

An **id, not an address**: the platform derives where it listens from the id and
this process's pid — a 0600 Unix socket plus an HTTP port on desktop, an HTTP
port on a phone. A launcher that spawned the app knows the pid, so it can work
the address out without being told. The app also prints it and writes it to
`/tmp/mcp-<id>-<pid>.json`.

Without a policy, anything that connects is admitted with `operator()` — read
the tree and drive it, nothing behind a tier. `facet_agent/consent` is a
ready-made policy that asks the user once per client.

`import "facet_agent/agent"` resolves by filename override: `agent.cplus`
on desktop (drives agent_appkit), `agent_ios.cplus` on iOS (agent_uikit,
port transport). It installs into facet's application seam
(`application::install_agent`) — facet itself knows nothing about the
agent stack.

This is facet's OPTIONAL tier as a package boundary (2026-08-17): an app
that never imports facet_agent links none of the agent machinery, by
construction rather than by promise.

| Need | File |
|---|---|
| Use it in minutes | `docs/tutorial.md` |
| Why it is shaped this way, and the traps | `docs/guide.md` |
| Exact signatures | `docs/ref.md` |

Tests: `cd vendor/facet_agent && cpc test`. The suite compiles the serving
surface on the active platform — the `vocab::Agent` → policy translation
is live code no other build type-checks, and a wrong arm there is a card
number readable by every agent that connects.
