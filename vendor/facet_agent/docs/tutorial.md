# facet_agent — tutorial

Make a facet app driveable by an agent, then decide who may drive it.

## Depend

```toml
[dependencies]
facet_agent = "*"
# the stack it serves; the resolver checks every import against ONE flat set
# taken from THIS manifest, so the closure is named here
agent_core  = "*"
agent_mcp   = "*"
agent_inapp = "*"
json        = "*"

[macos.dependencies]
agent_appkit = "*"      # agent_uikit on ios, agent_android on android
```

`cpc init --kind gui` writes all of this for you.

## Serve

Three lines, before the host:

```cplus
import "facet_runtime/runtime" as runtime;
import "facet_agent/agent" as agent;

fn run() -> i32 {
    agent::enable();                    // fill the serving seam
    runtime::agent_mcp("myapp");        // serve under this NAME
    runtime::run_screen(Home::new());
    return 0 as i32;
}
```

`agent_mcp` takes an **id, not an address**. Where the app listens is derived
from the id and this process's pid:

| platform | address |
|---|---|
| macOS, Linux | `/tmp/mcp-myapp-<pid>.socket` (0600) **and** `http://127.0.0.1:<9000+pid%1000>/` |
| iOS, Android | `http://127.0.0.1:<9000+pid%1000>/` |

The app prints where it landed on stderr at startup, and writes the same thing
to `/tmp/mcp-myapp-<pid>.json`.

## Connect

Any MCP client reaches the HTTP door with no bridge:

```
$ curl -s -X POST http://127.0.0.1:9161/ \
    -d '{"jsonrpc":"2.0","id":1,"method":"describe_ui"}'
```

## Add the inspector's verbs

```cplus
import "inspector/serve" as inspect;

    agent::enable();
    inspect::arm();                     // + the twelve inspector.* verbs
    runtime::agent_mcp("myapp");
```

Nine verbs without it, twenty-one with. Same socket, same gate.

## Decide who may drive

Without a policy, **anything that connects is admitted** with `operator()` —
read the tree, click, set text; nothing behind a `Protected` or `Private` tier.

To ask the user instead:

```cplus
import "facet/services" as services;
import "facet_agent/consent" as consent;

fn answered(index: i32, ctx: *u8) {
    if index == (0 as i32) { consent::allow_pending(); return; }
    consent::deny_pending();
    return;
}

fn show(ctx: *u8) {
    let msg: text::Text = "${consent::pending()} wants to drive this app.";
    runtime::alert("Allow agent access?", msg.view(), "Allow",
                   secondary: "Deny", on_answer: answered);
    return;
}

fn ask(client: str, ctx: *u8) {
    if !services::has_main_hop() { consent::cancel_pending(); return; }
    services::run_on_main(show, 0 as *u8);      // the policy runs OFF the main thread
    return;
}

    consent::on_ask(ask);
    agent::set_policy(consent::gate);
    agent::enable();
```

`cpc init` generates this as `src/agent_consent.cplus`, unwired.

## Day-one rules

- **`set_policy` before `enable()`.** The serve thread reads the policy once.
- **The policy runs on the serve thread.** Anything touching UI must hop
  (`services::run_on_main`); building an `NSWindow` there aborts the process.
- **The first request from an unknown client is refused.** `consent` prompts and
  admits the retry — it never blocks.
- **An id is a name, not a path.** A `/` in it is refused, and the app says so.

## Tests

`cd vendor/facet_agent && cpc test`
