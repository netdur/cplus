# facet_agent — reference

`import "facet_agent/agent" as agent;` resolves per platform: `agent.cplus`
(macOS), `agent_linux.cplus`, `agent_ios.cplus`, `agent_android.cplus`. The
surface below is identical on all four.

## facet_agent/agent

```cplus
fn enable()
fn disable()
```

`enable` fills facet's application seam (`attach_window`, `serve_once`,
`pin_policy`). It starts nothing — `runtime::agent_mcp(id)` is what asks to
serve. `disable` restores the no-op hooks.

```cplus
fn set_policy(f: fn(auth::Request) -> auth::Grant)
```

The application's own authorization policy, and the only way to be narrower (or
wider) than the default. **Call before `enable()`**: the serve thread reads the
policy once when it starts.

With none installed, every connection is served `auth::operator()` —
`cap_read | cap_act`. Content behind a `Protected` or `Private` tier is still
refused; that needs `cap_read_protected`.

```cplus
fn in_app() -> inapp::Session
fn in_app_with_grant(grant: auth::Grant) -> inapp::Session
```

The attached surface for an assistant compiled into this process. No socket.
`in_app()` takes nothing and grants `operator()`; `in_app_with_grant` opens a
NEW session with a wider authority the user has approved.

```cplus
fn policy_for(a: vocab::Agent) -> u32
```

facet's declared tier → agent_core's policy word. `Open`→`policy_none`,
`Protected`→`policy_protected`, `Private`→`policy_private`,
`Hidden`→`policy_exclude` (exclusion, not a tier).

### Where it listens

Not called directly — `runtime::agent_mcp(id)` drives it — but the addresses are
derivable by anything that knows the id and the pid:

| platform | address |
|---|---|
| macOS, Linux | `agent_mcp::uds_path(id, pid)` → `/tmp/mcp-<id>-<pid>.socket`, mode 0600 |
| macOS, Linux | plus `agent_mcp::loopback_port(pid)` → `http://127.0.0.1:<port>/` |
| iOS, Android | `agent_mcp::loopback_port(pid)` → `http://127.0.0.1:<port>/` |

`loopback_port(pid)` is `9000 + pid % 1000`. Both are reported on stderr at
startup (logcat on Android) and written to `/tmp/mcp-<id>-<pid>.json`.

An id containing `/` is refused — see `agent_mcp::valid_id`.

## facet_agent/consent

A ready-made interactive policy. Portable: it names no window.

```cplus
fn gate(req: auth::Request) -> auth::Grant
```

Hand this to `set_policy`. `InApp` is admitted without asking. An external
caller with a recorded decision gets it; an undecided one is **refused** and the
prompt is scheduled, so the retry carries the answer. Never blocks.

```cplus
fn on_ask(f: fn(str, *u8), ctx: *u8 = 0 as *u8)
```

Install the function that puts the question on screen; `f` receives the client
name. Until this is called, `gate` refuses everything external and never
prompts.

**`f` runs on the serve thread.** Hop with `services::run_on_main` before
touching UI, and guard with `services::has_main_hop()`.

`on_ask` also installs the deny hint (below).

```cplus
fn pending() -> str
fn allow_pending()
fn deny_pending()
fn cancel_pending()
```

The client a prompt is up for, and the three ways it ends. A dialog callback
receives an index and a context, not a name, so it calls `allow_pending` /
`deny_pending`. `cancel_pending` clears an unanswered prompt — without it the
slot stays full and every later request is refused with no prompt.

```cplus
fn allow(client: str)
fn deny(client: str)
fn forget(client: str)
fn forget_all()
fn decision(client: str) -> option::Option[bool]
```

Record or read a decision directly, for an app that decides some other way.
`decision` answers `None` for never-asked and for forgotten.

```cplus
fn deny_hint() -> str
fn anonymous_key() -> str
```

The sentence a refused caller gets — `"consent pending: … retry …"` while a
prompt is up, `"consent denied: …"` once refused. Installed into `agent_mcp` by
`on_ask`. `anonymous_key()` is the stable key for a caller that never sent
`initialize`.

## Related

| | |
|---|---|
| `agent_core/auth` | `Request { channel, token, client, method }`, `Grant`, the capability table |
| `agent_mcp` | the JSON-RPC core, both transports, the address convention |
| `agent_inapp` | the in-process session `in_app()` returns |
| `inspector/serve` | `arm()` — the twelve `inspector.*` verbs on the same socket |
| `facet_runtime/runtime` | `agent_mcp(id)`, `agent_id()` |
