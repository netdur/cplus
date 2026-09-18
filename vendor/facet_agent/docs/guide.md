# facet_agent — guide

Fast start: [tutorial.md](tutorial.md). Signatures: [ref.md](ref.md).

Why this package exists, and the choices in it that are not obvious.

## Why it is a package and not part of facet

An app that never imports `facet_agent` links none of the agent stack — not
`agent_core`, not `agent_mcp`, not the platform overlay. That is guaranteed by
the package boundary rather than promised by a comment. `facet_agent` installs
into facet's application seam (`application::install_agent`), so facet itself
knows nothing about agents.

## enable, and what happens without it

`enable()` fills three hooks: `attach_window`, `serve_once`, `pin_policy`. It
does not start anything. `runtime::agent_mcp(id)` is what asks to serve, and the
host starts the worker.

Call `agent_mcp` without `enable()` and the runtime finds a null hook and says
so on stderr rather than serving quietly. Call `enable()` without `agent_mcp`
and nothing binds — which is the rule the whole surface hangs off: **no
`agent_mcp`, no server.** There is no environment variable that can turn one on,
so a shipped binary cannot be switched on by its launcher or by anything that
can set a variable on the process.

## Every window, in one answer

`attach_window` fires when a window comes up or a screen mounts in one, and it
re-walks **every window the app has open**, in open order, into one surface.
The hook's argument is the signal that something changed, not the thing to walk
— facet's own window list says what to walk.

Each window is a node of role `window` under `app`, addressed by the name the
app opened it under (`App::window("settings", …)` ⇒ the id `settings`). A
window nobody named keeps its positional auto-id, `app/window#0`, and stays out
of the curated view exactly as before. So a node's window is its parent chain,
and there is no "current window" for a caller to set: ids resolve in one
registry across every window, and a verb names a node.

**This is what the surface did NOT do until 2026-09-17**, and it is worth
knowing because the failure is silent. The surface held ONE window and
re-opened from it on every verb, so an app with two registered windows — the
shape facet's window tier asks for — hid the first the moment the second
opened, and answered a single node named `app` for the rest of the process once
that second window CLOSED. The user saw a working app throughout. A driver
cannot tell one node named `app` from an app that has not finished launching,
and waits forever.

On GTK and Win32 the backend walks the platform's widgets and never imports
facet, so it asks for the window list through a seam (`set_windows_fn`) that
`facet_agent` fills from facet's registry. A backend with nothing installed
keeps the old one-window behaviour, which is what an app driving a window facet
does not know about still gets.

## An id, not an address

`agent_mcp` used to take a path, and on a phone `port_of` scanned it for digits
and silently substituted 8787 at the first `/`. The README's own
`app.agent_mcp("/tmp/myapp.sock")` therefore bound a socket on a Mac and port
8787 on a phone, with nothing said either way.

An address is the one thing the application cannot know: not whether it is on a
phone, not whether the path is free, not what a launcher would prefer. It knows
its own name. So it supplies a name and `agent_mcp::uds_path(id, pid)` /
`loopback_port(pid)` derive the rest.

### Why the pid, and not a uuid

The pid is the **join key between the socket namespace and the process table**,
and it works both ways:

- forward: a socket → `kill(pid, 0)` for liveness, `ps` for which binary is
  really behind it. The id in the filename is self-reported; the pid resolves to
  the truth through the OS.
- backward, which is the direction a coding agent needs: "the app I just built"
  → its pid → its address, with no registry and no trust in a name.

A uuid is unique for the same reason and supports neither. It is also what
removes the need for a channel: a launcher spawned the process, so it has the
pid, so it can compute the address without being told.

### What the descriptor adds

`/tmp/mcp-<id>-<pid>.json` is not what makes an app findable — the filename and
the process table already do that. It is the app saying what it **actually
bound**, which a derived address cannot: derived is a prediction. That is the
difference between "not running yet", "running and bound elsewhere", and "not
bound at all" — three states that otherwise look identical.

`sweep_stale()` runs before each bind. A path carries the pid, so a name is
never reused and nothing else would ever remove a dead one; the `atexit` hook
covers a normal quit but not a kill.

## Two doors on Unix desktops, HTTP everywhere

| | speaks | reachable by |
|---|---|---|
| Unix socket (macOS, Linux) | line-delimited JSON-RPC | the owning user only — 0600 |
| HTTP (everywhere) | Streamable HTTP, `application/json` | any MCP client, no bridge; any local process |

Both funnel into one `handle_request`, so a verb cannot exist on one and not the
other, and one consent gate covers both.

The socket keeps filesystem permissions HTTP cannot have. HTTP is what an MCP
client can actually reach — without it, every tool needs its own `nc` bridge
written for it, and Windows and mobile have no Unix socket in this stack to
bridge to at all. Neither subsumes the other, which is why Unix desktops keep
both.

Keep-alive, not `Connection: close`: **the connection is the session.** A
client's self-reported name arrives in `initialize` and is cleared when a
connection opens, so closing after every request would hand every later request
an empty `client` — which is exactly what a consent policy keys on.

## The policy

`set_policy(f)` where `f` is `fn(auth::Request) -> auth::Grant`. Without one,
`admit` serves `auth::operator()`: read the tree, click, set text, and nothing
behind a tier.

**Call it before `enable()`.** The serve thread reads the policy once when it
starts; installed afterwards leaves a window in which the default served, and
nothing reports that it happened.

### What the policy is told

`Request` carries `channel`, `token`, `client` and `method`. The last two exist
because a policy could not otherwise answer the question it is asked: a prompt
says "X wants to do Y", and with a channel and a token alone it could say
neither, nor do per-verb consent ("read freely, ask before a write").

`client` is **not a credential**. A caller picks its own name and can pick any
name, so it is identity in the sense a From: header is: it makes a prompt
legible and a remembered answer specific, not safe. Anything unforgeable goes in
`token` and is checked by the policy.

### The policy runs on the serve thread

This is the trap that aborts a process. A dialog built there is an `NSWindow` off
the main thread, and AppKit raises rather than warning. Hop with
`services::run_on_main`, and guard with `services::has_main_hop()` — without a
scheduler `run_on_main` is a silent no-op, so nobody is ever asked and the client
is refused forever by a prompt that never appeared.

### Blocking

A policy MAY block; `agent_core`'s "the gate never blocks" is a statement about
agent_core's dispatch, not a rule for yours. What it costs today: each listener
serves one connection at a time, so a parked policy stalls that door — the other
door keeps answering, but a second client on the same one waits. Nothing enforces
a deadline. `stdlib/channel` has no timed receive.

`facet_agent/consent` therefore does not block.

## consent

The machinery every app was writing by hand: a decision per client, a
have-I-asked guard, and a way for a dialog callback to get its answer back.

**Refuse, then admit the retry.** The first request from an unknown client is
refused and scheduling the prompt is a side effect of refusing it; the client's
next request is admitted or refused for good.

That makes the refusal load-bearing, so it says which kind it is:

```
consent pending: a prompt is on screen — retry in a few seconds (retry_after_seconds: 3)
consent denied: the user refused this client; it will not be asked again
```

Without that distinction both came back as `-32001 consent denied`, which reads
as final — a client that believes it never retries, and the Allow the user just
clicked does nothing.

Other decisions worth knowing:

- **One prompt at a time.** A second client arriving mid-decision is refused and
  does **not** take the pending slot, or the first client's answer would be
  recorded against the second's name.
- **Keyed on the client name, not the connection.** A one-shot client
  reconnects constantly; keying on the connection would re-ask every time, and
  keying globally would hand the second agent what the user granted the first.
- **Falls closed.** No `on_ask` installed means nothing external is admitted.
- **`cancel_pending()` matters.** A prompt nobody answered leaves the slot full
  and every later request refused with no prompt — an app that looks permanently
  closed after one dismissed dialog.

## in_app

`in_app()` opens the attached surface for an assistant compiled into this
process — no socket, same backend vtable, same grant model. The default is
`operator()`, which is not distrust of your own code: it bounds what MODEL
OUTPUT can reach. `in_app_with_grant(grant)` is the wider session an app opens
after the user approves something, as a NEW session rather than a widened one,
so a permission approved for one task does not outlive it.

## What an agent may not do

There is no drag verb, no pinch, no swipe, and there will not be one. A gesture
is a thing a person does with a pointer. An affordance only a gesture can reach
is the bug — and for someone operating by voice it is not an inconvenience, it
is inaccessible. The fix is a click path in the UI, not a verb here.
