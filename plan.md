# The agent surface has two front doors

Working notes, 2026-08-29. Scope: `agent_mcp`, `inspector`, `facet_agent`, and
the `cpc init` scaffold. Everything below was checked against the tree at
`f4f0a29`; where a claim is reasoned rather than run, it says so.

Both suites are green today — `agent_mcp` 317 passed, `inspector` 656 passed,
and a fresh `cpc init --kind gui --platform macos` builds and links. Nothing
here is a crash. It is a seam that grew a second copy of itself, and a set of
silences around that seam that turn every misconfiguration into "the app runs
fine and the tooling shows nothing".

## Where this stands (end of 2026-08-29)

Branch `agent-surface-one-door`, nine commits, every suite green.

### The model, as the user explained it — do not re-derive or argue with it

**Two surfaces, two jobs.** The agent surface (`agent_mcp`, armed by
`runtime::agent_mcp(id)`) is what a USER sees and does: an app has layout, a
user does not see layout, a user sees elements placed — so the agent sees
user-visible elements, and the actions are the small set a person has (point
and click, type, gestures). That is the RELEASE surface, 9 verbs.

The inspector (`inspect::arm()`, 12 verbs) is the whole tree, every attribute
including layout, plus structural edits. It is the DEBUG surface and it exists
**instead of hot reload** — hot reload was deliberately not built because the
retained tree plus MCP already does the job. The workflow: a button looks
wrong, so rather than edit → rebuild → look → repeat, nudge its attributes on
the live app until it sits right, then write the numbers into the source.

**Curation by key is correct — it is the DOM model.** A DOM has thousands of
elements and the interesting ones have ids. `mode:"full"` is a deliberate
escape hatch and not the default, because serving the whole tree to an agent
reproduces what makes DOM and a11y trees miserable: context blown, signal
buried. The developer's opt-out is `vocab::Agent { Open, Protected, Private,
Hidden }` on the shared band — inherits down, strictest wins, defaults `Open`.

**Any facet app is a native ACI.** The surface is product, not instrumentation:
it ships ON, and the control is runtime consent rather than a build flag. Only
the auth gate matters; the developer builds what they need with `set_policy`.
Today it serves iris's inspector and agent-run tests; it is heading for a
supervised call-centre workstation and voice-only operation.

### Still open

1. **`bugs/int-literal-in-an-i64-slot-is-stored-as-i32.md`** — an integer
   literal type-checks against `i64` and is emitted as `i32`. Silently wrong in
   an enum payload, invalid IR elsewhere. `stdlib`, `facet` and `facet_runtime`
   still carry their redundant literal casts because of it.
2. **iris needs three changes**, none made (separate repo): it sets
   `FACET_INSPECT`, which nothing reads any more; `remote.cplus` speaks the
   line-delimited framing that only the desktop socket still serves; and
   `agents_md.cplus:224` writes that stale advice into every project it opens,
   with a test at `:518` asserting the string is present.
3. **No audit of agent actions.** `inspector.journal` logs inspector
   mutations; clicks and `set_text` leave no trace. The supervised-operator
   case needs one.
4. **Startup could report what is armed** — `— 21 verbs (core, inspector)`
   vs `9 verbs (core)`. Discussed, not built; it replaces the canary that
   splitting `serve_if_asked` took away.
5. **Trailing `return;` is done**; the concise-literal pass is done except for
   the three packages blocked on (1).

## Status

| Item | State |
|---|---|
| 1 — one door | **DONE.** `serve_if_asked` is deleted from every inspector file and from the scaffold; the three per-platform entries are agent-free and `src/app.cplus` — the one file every platform builds — carries `agent::enable(); inspect::arm(); runtime::agent_mcp("<name>")`. A freshly scaffolded project builds, serves 21 verbs at its derived address, and answers click → describe → the change. Four parts: (a) `runtime::agent_mcp(path)` is a free function on every facade, called by `run`, `run_component`, `run_screen` and `App::run` alike; `App::agent_mcp` forwards to it and the per-`App` `agent_path` field is gone, so there is ONE storage. (b) `inspector/serve::arm()` arms the namespace portably without binding, with a no-op fallback on a target that has no inspector backend. (c) `arm_extension` is a TABLE — two namespaces coexist, `disarm_extension(prefix)` takes a name, re-arming replaces rather than stacks. Verified live: a `run_screen` app with `agent::enable(); inspect::arm(); runtime::agent_mcp(path)` serves all **21 verbs** on one socket with no `serve_if_asked` and no `FACET_INSPECT`. (d) `serve_if_asked` deleted, scaffold moved. |
| 2 — `agent_mcp(id)` | **DONE except the token, which was dropped on purpose — see below.** `agent_mcp` takes an ID on every facade (`App::agent_mcp()` with no argument defaults to the app's name). `agent_mcp::uds_path(id, pid)` and `loopback_port(pid)` are the conventions; `valid_id` refuses a `/` rather than sanitising it. `port_of` and `DEFAULT_PORT` are DELETED — the phone no longer guesses a port out of a path. Verified live: a launcher holding only the spawned pid derived the address and connected. The descriptor is written (`/tmp/mcp-<id>-<pid>.json`, 0600, saying what was actually bound), `sweep_stale()` clears what dead processes left, and `serverInfo.name` is the app's id. |
| 3 — consent hook | **DONE for the non-blocking shape.** `auth::Request` now carries `client` and `method`, so a policy can say WHO wants WHAT — per-verb consent was not expressible before. `facet_agent/consent` is the forty lines every app was writing, once: decision-per-client-name, one prompt at a time, refuse-then-admit-the-retry, and it falls closed when no prompt is installed. Seven tests. The app supplies the asking (`on_ask`), so the module names no window. The BLOCKING shape still needs the serve loop to go concurrent — see below |
| 4 — `run_screen` attach | **DONE.** Centralised in `facet_appkit::window::open_window`; three redundant runtime call sites removed; two regression tests, both proven to fail on the old code; verified against a live `run_screen` app over a real socket |
| 5 — may a policy block | **DONE.** Answered descriptive in `agent_core/docs/guide.md`, with the four costs a blocking policy actually pays today |
| 6 — release exposure | **DONE, by a different route than planned.** The env variable is gone, so serving is not reachable from outside the program at all: an app that does not call `agent_mcp` cannot be switched on by its launcher or by anything that can set a variable on it. The socket is 0600. The descriptor token turned out to add nothing here — see below |
| 7 — `set_policy` ordering | **DONE by construction.** Nothing arms a surface before `run()` any more — the scaffold's three lines sit together in `src/app.cplus`, so `set_policy` goes beside them and cannot be stranded after the serve thread has read the policy |
| 8 — silent failures | **DONE.** `bind_uds`/`bind_tcp` now run on the CALLER's thread, so the result is something a caller is still alive to hear; `listen_on`, `bind_reason` and `accept_loop` are split out, and `serve_uds`/`serve_tcp` are those three in sequence. Every facade reports — stderr on desktop and iOS, logcat on Android — on failure AND on success, since the address is derived now and that line is how a developer finds it. Also bigger than filed: **`set_policy` did not exist on iOS or Android** — both hardcoded `admit`, so an app's consent policy was absent on mobile and either failed to compile or silently served `operator()` to anything on loopback. Both facades now carry the policy statics and honour `effective_policy()` |
| 9 — duplicated envelopes | **DONE.** The eight byte-identical helpers in `inspector/mcp` are forwards to `agent_mcp`'s now — kept as local names rather than rewritten at 140 call sites, since the point is one definition of the frame and the descriptors stay readable. A test asserts the two namespaces emit the SAME envelope, and fails if a body is re-inlined |
| 10 — extension slot | **DONE.** Folded into item 1: `arm_extension` is a table, `disarm_extension` takes a prefix |
| 11 — doc defects | **DONE.** All five, plus a stale `serve.cplus` reference in `agent_mcp`'s header and the deny-all sentence in item 6 |

Also done, out of the numbered list: **the bound socket is `0600`**. `bind_uds`
chmodded nothing, so `bind` created it `0777 & ~umask` — measured `srwxr-xr-x`
against VS Code's `srw-------` on the same machine. Now `umask` around the bind
(atomic, no window) plus an `fchmod`-equivalent after it, with a test that
`stat`s the mode and fails on the old code. Confirmed on a live app.

### The token was dropped, and the reasoning was wrong

The plan had a per-run token in the descriptor as the thing that would make a
shipped app safe to serve. It would not.

The socket is 0600 and the descriptor is 0600, both owned by the same user in
the same directory. Anything that can read the token in order to present it can
equally just connect to the socket — a second lock on the same door. The file
mode is the boundary on a desktop; the token is not.

A credential earns its keep only on a transport with no filesystem permissions
behind it: loopback TCP, which is what a phone already uses, and HTTP if item 2
takes that road. And on a real device — the one place that transport is the only
option — there is no shared filesystem for a launcher to read the token from
either, so it cannot even be delivered.

So it is deferred to the transport that needs it, together with a way to carry
it, rather than added where it would look like protection and be none. What
actually closed the release exposure was removing the environment variable: an
app that never calls `agent_mcp` cannot be switched on from outside.

### What is left, and what it forks on

The remaining work is one piece, and it is bigger than anything above:
**concurrent connections**. It is what a blocking consent policy needs, and it
is what stops a long-lived MCP client locking out iris. Three things have to
move before threads can:

- **`CLIENT_NAME` is a process static.** Per-connection state in a global; two
  connections would inherit each other's identity, which is the consent bug in
  a new place.
- **`events::Subscriber` is a Vec-backed queue passed by `ref`** into the accept
  loop. It is not thread-safe and there is one per process.
- **The backend walk is not obviously re-entrant.** `agent_appkit`'s describe
  reads the live AppKit hierarchy from the serve thread; two of those at once is
  the hazard the main-thread marshal exists to prevent.

The shape that works: thread per connection, one lock around the POLICY call
(so app policies stay single-threaded and two dialogs cannot stack), a second
around verb execution, and the policy lock released while a dialog is up.

**It forks on the HTTP decision.** Streamable HTTP is request/response with no
long-lived connection to hold, so most of the concurrency requirement comes
free with the transport rather than being built underneath the current one.
Deciding that first is worth more than starting here.

## The core finding

There are two ways to start the same server, and a one-shot latch decides which
one wins.

```
app.agent_mcp(path)          ->  App::run  ->  app::agent_serve_once(path)
inspect::serve_if_asked()    ->  install + arm + enable + agent_serve_once(path)
```

`facet_agent::serve_once` opens with `if AGENT_SERVING { return; }`, so whichever
runs first takes the socket and the second is a silent no-op. In the scaffolded
shape `serve_if_asked()` is `main`'s first statement, so it always wins, and an
app that also called `app.agent_mcp(...)` loses its own socket with no message.

The two doors do not arm the same thing. `serve_if_asked` does four things and
`App::run` does one of them:

| | `serve_if_asked` | `App::run` |
|---|---|---|
| `install()` — overlay, native props, `mcp::set_marshal` | yes | no |
| `mcp::arm(tree::local_backend())` — the `inspector.*` namespace | yes | no |
| `agent::enable()` — AgentHooks | yes | no |
| `agent_serve_once(path)` | yes | yes |

So the choice of door silently decides whether eleven of the socket's
twenty-one verbs exist. That is the drift.

### The history, because it changes the fix

The layering itself was done right, and only the entry point drifted:

| date | commit | |
|---|---|---|
| 2026-06-09 | `6e269e4` | `agent_mcp` — the JSON-RPC core |
| 2026-08-09 | `4a78632` | `inspector` — the tree walker |
| 2026-08-09 | `729cb23` | **the inspector on the agent's socket** — `arm_extension` |
| 2026-08-15 | `c24a96c` | `serve_if_asked` appears |
| 2026-08-21 | `46583e1` | handshake, `tools/list`, `set_policy` |

`inspector` has imported `agent_mcp` and registered itself as a namespace
extension since the day it went on a socket. It does not duplicate the
protocol, the transport, the consent gate or the teardown hook, and
`inspector/mcp.cplus` already calls `bridge::prop`, `bridge::tool`,
`bridge::no_props` and `bridge::required1/2` across that import. That part is
correct and should not be rebuilt.

What arrived six days later was a *second entry point* that bundles the
inspector's own setup with starting the shared server. The scaffold adopted it,
and from that point an app had two unrelated-looking ways to become
inspectable. The fix is therefore not "make inspector use agent_mcp" — it
already does — but "collapse the two doors into one".

### Evidence that this is live, not theoretical

`~/Projects/demo_1` has walked into it. `src/main.cplus` now reads:

```cplus
    // inspect::serve_if_asked();
    return app::run();
```

commented out, with `app.agent_mcp(sock)` in `app.cplus` taking over. Per that
project's own bug report — written before the line was commented out — this
costs it the entire `inspector.*` namespace and the main-thread marshal, with
no diagnostic. The app still answers `describe_ui`, still lists tools, still
clicks. It just quietly has half a surface.

## Work items

### 1. One door: `app.agent_mcp` should arm everything

The target rule is the simple one: **no `agent_mcp`, no inspector. If the app
has `agent_mcp` and an auth policy, the inspector rides on both.**

#### Why it did not do that — investigated, 2026-08-29

`App::agent_mcp` was **not** missing. It landed 2026-07-20 (`5bb9c06`), almost a
month before `serve_if_asked` (2026-08-15). So the answer is not "there was
nothing to use".

**It was never designed.** `serve_if_asked` entered the tree in `c24a96c` — a
*compiler bugfix commit about view lifetimes* — under that commit's own heading:

> CARRIED IN THIS COMMIT, NOT AUTHORED HERE
> Work already in the working tree when this started, committed alongside at the
> repo owner's direction: […] inspector appkit/widget/remote […]

There is no design commit for it anywhere in the history. Five days later
`0a01d04` scaffolded it into every generated app, and that commit's reasoning is
sound for the problem it names — a launcher sets `FACET_INSPECT`, and without a
reader "the inspect pane sits empty against an app that is running perfectly" —
but it never asks whether `app.agent_mcp` could carry the same job.

That said, four things really were missing, and **(a) still is**. They are what
the fix has to supply:

**(a) The scaffolded tier has no agent entry at all.** `agent_mcp(path)` is a
method on `runtime::App` (`runtime_macos.cplus:535`) and only `App::run` serves
it (`:628`). `run_screen` is a free function:

```cplus
fn run_screen[S: component::Component + component::Lifecycle + screen::Screen](
    take screen_v: S,
    take menu: AppMenu = screen::empty_app_menu(),
) -> S
```

No path, no `App`, nothing. And `cpc init --kind gui` scaffolds
`runtime::run_screen(Home::new())`, because AGENTS.md prescribes it (facet's
Android facade does not implement `App::run`). **A scaffolded app cannot call
`app.agent_mcp` — there is no `app`.** This is the load-bearing reason, and it
is unchanged today.

**(b) Nothing outside `inspector/` reads `FACET_INSPECT`.** Grepped across
`facet`, `facet_runtime`, `facet_agent` and `agent_mcp`: zero hits. The launcher
channel lives entirely inside the inspector package, and `app.agent_mcp`
requires the *app* to name a path — so at the time, being launcher-driven and
being on `app.agent_mcp` genuinely were exclusive.

This is the one of the four that dissolves rather than needing to be built. A
launcher-driven flow does not need anyone to read the environment: the launcher
knows the pid it spawned, and that is enough to derive the address. See item 2,
*Why no environment variable*.

**(c) The extension namespace had no grant.** At `729cb23` (Aug 9) the handler
was `fn(str, json::Value, f64) -> json::Value` — no `auth::Grant`. It gained one
at `46583e1` (Aug 21). So on the day `serve_if_asked` was written the inspector
namespace **could not participate in the auth model**; there was nothing to
inherit.

**(d) There was no app-supplied policy to inherit either.**
`facet_agent::set_policy` also landed at `46583e1` — six days *after*
`serve_if_asked`, one day *after* the scaffold adopted it. "Use the app's auth
setup" describes a world that did not exist when this code was written, and
nobody went back once it did.

One thing is still missing from `agent_mcp` proper: **`arm_extension` has to be
called.** There is no registry a linked package can join. The natural caller is
`facet_agent` — it already imports `agent_mcp` — but it correctly does not
import `inspector`, since that would put the inspector in every agent app. So
there is no place today for "arm the inspector when the app serves", which is
exactly why it ended up in a second entry point.

And `inspector/serve` — the only inspector module with a true no-backend
fallback — exposes only `serve_if_asked()`. `install()` and `mcp::arm()` *are*
portably reachable through `inspector/appkit`, which is platform-shadowed three
ways (`appkit.cplus`, `appkit_ios.cplus`, `appkit_android.cplus`), but that
module has no neutral fallback, so importing it breaks a build for any platform
without an inspector backend. There is no portable "arm the namespace, do not
bind a socket".

#### What the fix therefore has to include

1. **An agent entry on the `run_screen` tier**, or the scaffold moves to `App`.
   The second is blocked on Android's `App::run`, so the first is the near-term
   answer. Without this nothing else matters — the scaffolded shape still cannot
   ask for a socket.
2. **Nothing** — reading `FACET_INSPECT` looked like the one genuinely useful
   thing `serve_if_asked` does, and it turns out not to be needed at all. See
   item 2's *Why no environment variable*: the launcher already knows the pid,
   so the address is derivable and the variable was never carrying information.
   That removes `serve_if_asked`'s last remaining job.
3. **A portable `arm()` in `inspector/serve`** with a no-op fallback, separate
   from anything that binds.
4. **Registration in `agent_mcp` that does not require a caller who knows both
   packages** — a small table (item 10) that a linked package joins, so the
   serve path arms whatever is present.

Then the target rule falls out for free: one serve path, one gate, and the
inspector is visible exactly when the app serves and exactly under the policy
the app installed.

Acceptance test: an app that calls `app.agent_mcp(...)` and links `inspector`
gets all twenty-one verbs under its own policy; an app that does not call
`app.agent_mcp` serves nothing; and there is no second function that starts a
server.

Removing the scaffolded `inspect::serve_if_asked()` line then also removes the
seven-package dependency block from every generated manifest — see item 6.

### 2. `agent_mcp(id)` — the app names itself, the platform picks the address

`app.agent_mcp(path)` takes a `str` that means two different things by platform
and coerces between them by guessing.

On iOS and Android, `facet_agent`'s `port_of` walks the string and returns
`DEFAULT_PORT` (8787) the moment it hits a non-digit. So

```cplus
app.agent_mcp("/tmp/myapp.sock");   // the README's own example
```

binds a Unix socket on macOS and TCP port 8787 on a phone. There is no error and
no way for the app to state which it wanted. `facet_agent/README.md:19`
documents the reinterpretation in a parenthetical — `// the MCP socket (a PORT
on iOS)` — but not the fallback, and a caller who passes a path on a phone has
no way to learn what port they got.

The reason it cannot be fixed by validating harder is that an address is the one
thing the application does not know. It does not know whether it is on a phone,
whether the path is free, or whether a launcher already picked something. It
knows who it is.

#### The shape

```cplus
app.agent_mcp();            // serve under App::new's name
app.agent_mcp("demo_1");    // or an explicit id
```

`App::new(name: str = "facet")` already carries a name, and demo_1 already calls
`app.name()` to build its own path — so for most apps the id is not even a new
parameter.

The runtime appends the pid and derives the address:

| platform | address |
|---|---|
| macOS, Linux | `/tmp/mcp-<id>-<pid>.socket` |
| iOS, Android | TCP on loopback, port derived from the pid |

Derived rather than ephemeral, so that a launcher holding only the pid can
compute it — see *Why no environment variable* below.

That is exactly the path demo_1 hand-built —
`"/tmp/mcp-${app_name}-${pid}.socket"` — which is the strongest evidence the
shape is right: the app was working around the absence of it.

#### The pid is the join key, not just a discriminator

**It belongs to the runtime, not the app**, but the reason it has to be a pid
rather than a uuid is discoverability: the pid is what joins the socket
namespace to the process table, and that join works in both directions.

- Forward: a socket → `kill(pid, 0)` → is this live; `ps -p <pid>` → which
  binary is actually behind it. The id in the filename is *self-reported*; the
  pid resolves to the real executable through the OS.
- Reverse, and this is the direction a coding agent needs: "the app I just
  built" → its pid → its socket. No trust in a name, no registry to consult.

A uuid is unique for the same reason and supports neither. This is also a shape
with precedent on the same machine — Claude Code's own sockets are
`/tmp/cc-socks/<pid>.sock`.

**Observed in `/tmp` right now**, and it settles three things at once:

```
srwxr-xr-x  /tmp/mcp-demo_1-1507.socket
srwxr-xr-x  /tmp/mcp-demo_1-6877.socket
srwxr-xr-x  /tmp/mcp-demo_1-15073.socket
srwxr-xr-x  /tmp/mcp-demo_1-16143.socket
srwxr-xr-x  /tmp/iris-inspect-94542.sock
srw-------  /tmp/Visual Studio Code-8ffd088d-….sock
```

1. **The convention works.** Four demo_1 runs, four distinct sockets, nothing
   overwritten.
2. **The liveness sweep is not optional.** All five pids are dead and
   `lsof -U` shows no process holding any of them — every one is orphaned. The
   `atexit` unlink in `agent_mcp` does not cover however these died, and macOS
   does not sweep `/tmp` for days. So a listing is only meaningful with the
   `kill(pid, 0)` check, which is exactly what the pid in the name makes free
   — no `lsof`, no `ps` parsing, no permissions.
3. **The chmod gap, on real files.** Ours are `srwxr-xr-x`; VS Code's is
   `srw-------`. That is the missing `fchmod`, visible on disk.

**iris currently defeats this, and it is the same bug as its known limitation.**
`run_cmd.cplus:71` names the socket after **iris's own pid**:

```cplus
fn inspect_sock(pid: i32) -> text::Text { return "/tmp/iris-inspect-${pid}.sock"; }
```

`94542` above is iris, not the app. So the pid resolves to the IDE, the join
gives you nothing about what is being debugged, and — for the same reason — "two
apps run at once would contend, and the second one's serve loses", which that
file documents two lines above. Lost discoverability and the contention limit
are one defect: the name is keyed on the wrong process. Naming it after the
app's pid fixes both. That is an iris change, noted here because this is where
the convention is decided.

**The id must not be the port.** A port is a 16-bit number and an id is a name;
there is no collision-free map between them. And any scheme where the app picks
a *fixed* port hits item 8 directly — two apps, the second one's bind fails,
silence. `http://localhost:<port>/<id>` — one shared port, path-routed — is
nicer but needs a broker process, and then its lifecycle, its crashes and the
question of who starts it are all yours. An ephemeral port plus a written-down
address avoids both, and is what every debugger does for the same reason
(DevTools, JDWP, DAP).

#### `/tmp`, and the chmod that makes it safe

`/tmp` rather than `~/.cplus/run/`, because the OS sweeps it and a store
directory would accumulate dead sockets forever.

That is safe, but not currently. **`bind_uds` never chmods** — grep for
`chmod`/`fchmod`/`umask` in `agent_mcp.cplus` returns nothing — so the socket
takes whatever umask gives, typically `0755`, and any local user can connect.
The directory is not what governs this: `connect()` on a Unix socket checks the
*socket's own* mode, so `0600` closes it even in a world-readable `/tmp`. One
`fchmod` after the bind, and the same for the descriptor.

Plain `/tmp` rather than `$TMPDIR` on purpose. On macOS `$TMPDIR` is already
per-user and swept, which sounds better, but it makes the location depend on how
the process was launched — and the whole point is that a client can derive or
glob it without being told. Predictability wins.

#### The descriptor

Beside the socket, `/tmp/mcp-<id>-<pid>.json`, mode `0600`:

```json
{ "id": "demo_1", "pid": 8161, "transport": "uds",
  "address": "/tmp/mcp-demo_1-8161.socket", "token": "…" }
```

On desktop the descriptor is **not** what makes an app discoverable — the
filename and the process table already do that, per the section above, and a
client that has globbed `/tmp` has everything it needs to connect. The
descriptor earns its keep for the two things the filename cannot carry:

- **The token.** Below.
- **Confirmation of what was actually bound.** The address is derivable, so a
  client can find the app without reading this — but derivable is a prediction,
  and the descriptor is the app saying what it really got. It is how a
  collision, an override, or a bind that failed becomes visible rather than
  being read as "not running yet".

It is also the right place for anything later clients need that a path cannot
encode — protocol version, which namespaces are armed, whether the app is still
mounting.

**The token in it is what makes item 6 go away.** Default policy: the `token` on
params must match the descriptor's. Then iris reads the file and connects with
no friction, while a shipped release binary is useless to anything that cannot
read the file — which is the user themself. That is ship-safe by default with no
developer effort, and it does not depend on anyone remembering to build with the
right profile, which a debug gate would. `set_policy` stays the escape hatch for
anything richer; demo_1's consent dialog still sits on top.

Identity and authorization stay split, deliberately: the id is a name, the token
is the credential. Making the id itself unguessable merges them again and costs
the stable name.

#### Why no environment variable

The first draft of this item kept an injected address for the device case. It is
not needed, and the reason is one fact: **the launcher spawns the app, so it
already knows the pid.** Everything the variable was carrying is derivable from
that.

**Desktop.** iris forks the child and has its pid. It globs
`/tmp/mcp-*-<pid>.socket`, gets exactly one match, and connects. It does not
even need the id. Fully covered by the naming convention, with no channel at
all.

**iOS simulator.** `simctl launch` prints the pid. The container is on the host
filesystem (`~/Library/Developer/CoreSimulator/Devices/<UDID>/…`), so the
descriptor is readable, and the simulator shares the host's loopback, so a port
bound inside is reachable at `127.0.0.1` directly. Also covered.

**Real device.** The only case with no shared filesystem, and the one the
variable was really for. But `devicectl` returns the pid too, so a pid → port
derivation that both sides compute removes the need here as well. iris already
has that function — it is just keyed on the wrong process:

```cplus
fn inspect_port(pid: i32) -> text::Text {
    let n: i32 = (9000 as i32) + (pid % (1000 as i32));
```

That is iris's own pid, the same defect as `inspect_sock` two lines above it.

So the address is a pure function of the pid on every platform, and the
variable stops being the mechanism. It survives only as an **escape hatch** for
the collision case — rare on a device, essentially never on desktop.

**A fixed default port is the one option ruled out by experience.** A simulator
holding `:8787` once made a device run look green when it was not, and
`DEFAULT_PORT = 8787` is what hid it. Deriving from the app's own pid gives a
different port per instance and would have caught it.

Consequences beyond this item: `port_of` disappears outright, because nothing
passes an address any more; and reading the environment was `serve_if_asked`'s
entire remaining purpose, so item 1 gets smaller rather than larger.

#### What is left to write down

1. Derive the address from the app's pid, per the table above.
2. Write the descriptor, and **report what was bound** (item 8) — the one thing
   nothing does today.
3. Accept an override only where a collision is plausible.

#### One free win

`initialize` answers `serverInfo.name` as the hardcoded string
`"facet-agent-surface"` (`agent_mcp.cplus:829`). It should be the app's id. A
client that reached the wrong process — a recycled port, a stale descriptor —
then finds out during the handshake instead of driving the wrong app.

#### The open question: HTTP

Today's transport is raw newline-delimited JSON-RPC over TCP, which is neither
of MCP's two transports. That is why `.iris/mcp.json` has to bridge with `nc -U`
and why nothing off the shelf can talk to a running facet app. Streamable HTTP
on loopback would make an app directly connectable by any MCP client on every
platform with no shim, collapse the socket/port duality into one descriptor
field, and make iris *simpler* — a URL instead of a subprocess.

Cost is a real HTTP server in `agent_mcp`, though request/response needs only
POST; SSE can wait until something needs server→client messages.

**The call to make: does HTTP replace UDS on desktop, or sit beside it?**
Alongside means two servers to keep correct. Replacing means iris's current path
changes and filesystem permissions stop being the access control — which is
precisely what the descriptor token is there to replace. Leaning toward
replacing.

### 3. There is no consent hook, so every app rebuilds one

demo_1 wrote roughly forty lines — a `channel`, a `services::after` timeout, a
modal alert, a `run_on_main` hop, and a park-and-wake gate — to answer the
question "may this agent connect?". That is a gap in `agent_mcp`, not an
application concern, and the pieces it had to work around are specific:

- **`auth::Request` carries `{channel, token}` and nothing else.** Not the
  client's self-reported name (that lives in `agent_mcp`'s `current_client()`,
  a separate global), not the method being requested, not a connection
  identity. So a policy cannot render "Claude Code wants to press buttons", and
  cannot do per-verb consent at all. Widening `Request` is the smallest useful
  change in this whole file.
- **`serve_fd` handles one connection at a time.** A policy that parks stalls
  the entire surface; a second agent cannot even be refused, it hangs. Nothing
  documents this, and it is load-bearing for anyone who takes the blocking
  route.
- **`stdlib/channel` has no timed receive**, so a timeout has to be assembled
  from `close()` plus a `services::after` one-shot.
- **Parking with no main-thread hop installed hangs forever.**
  `services::run_on_main` is a silent no-op without a scheduler;
  `services::has_main_hop()` is the guard and appears in no documentation.

The design question underneath is item 5 — whether a policy is allowed to block
at all. Answer that first, then decide whether the hook is "ask and park" or
"refuse and admit the retry". Either way an app should not be writing a channel
and a timer to get a consent dialog.

### 4. `run_screen` never attaches its window (confirmed, macOS)

Verified in the tree. `runtime_macos.cplus:221`:

```cplus
    let _w: *u8 = host::open_window(#addr_of(root), c);
```

The window is bound to a discard name and never attached. Every other site in
the same file attaches — `:370` `present_window`, `:657` `App::run`, `:797`
`push_screen`. So an app on the `run_screen` tier serves `describe_ui`, `click`
and `hit_test` against an empty registry for the life of the process, while
`inspector.*` answers the full tree over the same socket. It cannot self-heal:
`agent_appkit` re-walks from the surface's own node 1, which never exists.

That is every scaffolded project, because AGENTS.md prescribes `run_screen`
(facet's Android facade does not implement `App::run`).

Fix in the iOS shape rather than adding a fourth call site.
`runtime_ios.cplus:605` centralised the attach inside the host's window-open
path *specifically* so it would cover `run_screen`, and its comment predicts
this hole in as many words. macOS should do the same.

This one is upstream of a lot: it is why demo_1 moved to the `App` tier, which
is why demo_1's Android build is broken.

### 5. Decide whether a policy may block, then write it down

`agent_core/docs/guide.md` says "The gate never blocks, prompts, or does I/O",
and that reads two opposite ways — descriptively, about agent_core's own
dispatch, or prescriptively, about the app's policy. The tree argues both
sides: `facet_agent`'s `set_policy` comment names "ask its user first" as the
motivating use case, while the guide's `NeedsGrant` row says "ask the user,
then retry".

demo_1 built both and measured the difference: refuse-then-retry returns
`-32001` on the first request and admits the second; park-and-wake held
`describe_ui` open for eight seconds until Allow was clicked. Both work. They
are visibly different products.

This is a one-sentence fix in the guide, but it has to be the *right* sentence,
and it gates item 3.

### 6. The scaffold ships the inspector into release builds

Measured on a fresh `cpc init --kind gui --platform macos`, building with and
without the scaffolded line:

| | modules | binary |
|---|---|---|
| as scaffolded | 127 | 18.4 MB |
| line and dep block removed | 111 | 14.3 MB |

There is no debug gate anywhere — not in the scaffold, not in `serve.cplus`, no
profile. So `cpc build --release` of any generated app carries a JSON-RPC server
that arms on an environment variable — which is the part that makes it an
exposure rather than merely dead weight, and the part item 2 removes.

And the grant it serves is not what the docs describe.
`agent_mcp/agent_mcp.cplus:10` says "an un-served (deny-all) gate rejects
everything, so the surface is closed until the developer arms a policy", but
`facet_agent::effective_policy` returns `admit` — that is `auth::operator()` —
when no policy was set. `arm` is deliberate; the grant is open by default.
`bind_uds` never chmods, so the socket takes whatever umask gives.

On Android it is visible to end users: every scaffolded app requests
`android.permission.INTERNET` for the inspector and nothing else.

Four things, and none of them is the debug gate this item originally proposed:

1. Reconcile that sentence with what `facet_agent` installs, regardless of
   anything else on this list.
2. **Serving stops being reachable from outside the program.** Once the address
   is derived rather than injected (item 2), there is no environment variable
   left to arm anything: an app that does not call `app.agent_mcp` cannot be
   switched on by its launcher, its parent process, or anyone who can set a
   variable on it. That is the exposure closed at the root rather than
   mitigated, and it is a consequence of the change rather than a feature to
   build.
3. **The descriptor token covers what remains** — an app that *does* serve. A
   default policy requiring the token from a `0600` descriptor is closed to
   anyone who cannot read the file, in every build, with no developer effort,
   and it does not depend on remembering to build the right way. The socket
   also needs the `fchmod` that `bind_uds` does not currently do.
4. Item 1 removes the dep block from generated manifests as a side effect,
   which is most of the size delta.

What that leaves undecided is only whether a release build should serve *at
all*. With the token in place the answer can reasonably be yes — a shipped app
that is inspectable by its own user, and by nobody else, is a feature rather
than a hole.

### 7. `set_policy` must run before the serve, and the scaffold makes that hard

`effective_policy()` is read once, at spawn. `AGENT_POLICY`'s own comment says
"SET IT BEFORE `enable()`". The scaffold puts `inspect::serve_if_asked()` — which
calls `enable()` and spawns — as `main`'s first statement, so a developer who
later adds `agent::set_policy(mine)` inside `app::run` silently gets `admit`.
Item 1 fixes this by construction; until then it is worth a comment on the
scaffolded line.

### 8. Every failure mode is silent

All three `serve_once` implementations do this:

```cplus
    let _t: thread::JoinHandle[i32] = thread::spawn_with::[AgentServe, i32](...);
```

`serve_uds` returns −1/−2/−3/−4/−5 and `bind_tcp` likewise; nobody reads any of
it. macOS and iOS print nothing. Android prints `inspector: serving on the port
in debug.facet.inspect` **after** the spawn, so it claims success in exactly the
case its own AndroidManifest comment documents — no INTERNET permission, bind
fails with EACCES, "an IDE connects to nothing while the app runs perfectly".

Two of these failures are already known by name in the tree. iris's
`run_cmd.cplus:70` says "two apps run at once would contend, and the second
one's serve loses" — that is `bind_uds` returning −5, invisible. From outside,
all of them look identical to a working app with a blank Inspect tab.

`bind_uds` and `bind_tcp` are *already* split out of the accept loop so a test
can call them. Bind on the calling thread, report the code, then spawn the
accept loop with the bound fd. That is a small change and it lets the serve
path print one honest line.

### 9. Duplicated envelope code

`ok_response`, `err_response`, `missing_param`, `jstr`, `jnum`, `jbool`,
`member` and `obj_get` are byte-identical in `agent_mcp/src/agent_mcp.cplus` and
`inspector/src/mcp.cplus`. Not a boundary decision — the same file already
imports `bridge::prop`, `bridge::tool`, `bridge::no_props` and
`bridge::required1/2`. The envelope half was just missed, and it is the half
where a drift produces a malformed JSON-RPC frame on one namespace only.

### 10. One extension slot, overwritten without a word

`arm_extension` writes `EXT_PREFIX`/`EXT_HANDLER`/`EXT_DESCRIBER` with no check.
A second caller replaces the first silently. Only the inspector arms it today,
so this is latent — but the header sells it as a general mechanism, and item 1
may well add a second armer. Refuse a conflicting second arm, or make it a small
table.

### 11. Documentation defects, all verified

| Where | Says | Actually |
|---|---|---|
| `agent_mcp/docs/ref.md:75,83` | `policy: fn(auth::Request) -> auth::Decision` | `auth::Decision` does not exist anywhere in `agent_core/src`. The type is `auth::Grant` |
| `facet/docs/ref.md:478` | `in_app(policy)` | source is `in_app()`; the one taking an argument is `in_app_with_grant(grant)` |
| `facet/docs/ref.md` §facet_agent | documents `enable`, `disable`, `in_app` | omits `set_policy` — the only way an app supplies a policy at all — although `facet_agent/README.md:32` sends the reader to exactly this section for it |
| `agent_mcp/docs/ref.md` §UDS | lists `current_pid`, `read_line`, `write_all`, `serve_fd`, `serve_uds` | omits `current_client()`, the intended key for per-client consent |
| `facet/docs/ref.md:264` | "reports the chosen index through `on_answer`" | never gives the type: `fn(i32, *u8)` plus `on_answer_ctx: *u8` |

`facet_agent`, `agent_inapp` and `agent_jwt` — the three packages an application
calls into for authorization — are the three with no `docs/` at all. The
mechanism packages beneath them are documented well.

## Triage of the demo_1 reports

They are in `~/Projects/demo_1/bugs`. All five hold up; two need their proposed
fix changed, and one is a symptom of another.

| Report | Verdict |
|---|---|
| `run-screen-never-attaches...` | **Real, confirmed against the tree.** Item 4. The report's own preferred fix (one line at `:221`) is the weaker of the two it offers; take its second suggestion, the iOS centralisation. |
| `serve-if-asked-is-load-bearing...` | **Real, and it is the drift.** Item 1. But its fix — two comments — is too small. It explicitly considers and rejects a structural fix on the grounds that it would need three per-platform project files; that is true for a fix inside the *app*, and not true for the fix inside `facet_agent`. |
| `the-app-tier-workaround-breaks-the-android-build` | **Real but derivative.** It exists only because of item 4. Fixing that lets demo_1 revert to `run_screen` and the report closes itself. The underlying Android `App::run` gap is genuine and separate. Report is honest that it is reasoned from source, not run. |
| `the-app-facing-agent-policy-api-is-undocumented` | **Real.** Item 11; every claim in its table checks out. |
| `the-gate-never-blocks-...is-ambiguous` | **Real.** Item 5, and it is the one that unblocks item 3. |

Where the reports show their own drift: the first one describes
`inspect::serve_if_asked()` as load-bearing and worth keeping with a warning
comment, when the better answer is that it should not exist as a second door.
And demo_1 has since commented the line out anyway, so the project is now living
in exactly the half-armed state the report predicted — which is the strongest
argument for item 1 in the whole file.

## Order

Item 5 is one sentence and unblocks item 3. Item 11 is text and blocks nobody.
Item 4 is one localised change and closes two demo_1 reports.

Then item 1, which is the real work and makes items 6, 7 and 10 mostly
disappear. Item 8 should ride along with it, since both touch the serve path.

Item 2 is the largest, and it now carries the answer to item 6 as well, so it
should not be deferred as far as its size suggests. Two pieces of it are small
and independent of the transport question and can land immediately: the
`fchmod` on the bound socket, and `serverInfo.name` reporting the app's id.

## Open questions

- ~~Does Streamable HTTP replace UDS on desktop, or sit beside it?~~ **ANSWERED
  AND BUILT: beside it.** HTTP is the door every platform has and the one the
  descriptor names — any MCP client reaches it with no bridge. The 0600 socket
  stays on desktop, where it has filesystem permissions HTTP cannot, and it is
  the only consumer of the line-delimited framing left. Mobile serves HTTP on
  its loopback port instead of the private framing, since a phone has no socket
  to bridge to and the port was the only door anyway.
- **May a policy block?** Item 5. Everything about the consent hook follows from
  it.
- **Where does the inspector namespace get armed** once `serve_if_asked` is
  gone — a `facet_agent` sibling module, or something more general in
  `agent_mcp` that lets any linked package register?

Answered since the first draft: *should a release build serve at all* — with
item 2's descriptor token, yes. An app inspectable by its own user and by nobody
else does not need a build-time gate.
