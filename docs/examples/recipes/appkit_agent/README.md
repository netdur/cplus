# appkit_agent — an AppKit UI an agent can see and drive

The basics of **agent-aware code**: an ordinary AppKit window that an external
agent can inspect (`describe_ui`) and operate (`click` / `set_text`) over the MCP
bridge's JSON-RPC, with every request passing through a consent gate.

It builds a window with a **Save** button, a **name** field, and a decorative
label, then prints — without a GUI event loop — what an agent sees and the result
of each action.

## The five ideas

1. **Tagging = exposure.** `agent::set_agent_id(view, "save-btn")` marks a widget
   as part of the agent's surface. The developer curates this: untagged views
   still appear in the tree but are **not actionable** (the label below is
   untagged on purpose). Declarative frameworks tag through the view's
   `accessibilityIdentifier` instead — facet's `.agent_id("...")` modifier does
   exactly that — and the walk honors both channels; an explicit pin wins.
2. **The Surface.** `agent::open(window)` walks the live view hierarchy into the
   agent-core identity tree + a NodeId→NSView map + text-version state. The
   surface retains its views, so a UI update can never leave it dangling.
3. **Authorized actions.** `click` / `set_text` / `scroll_to` route through the
   agent-core authorization brain and only touch the real widget when it returns
   `allowed`. `set_text` carries `base_version` for optimistic concurrency.
4. **Consent.** Every request is gated by an `auth::AuthGate` first. An un-served
   (`deny_all`) gate refuses everything; you arm a real policy with `auth::serve`.
5. **Staleness.** When a widget leaves the window after the surface was opened
   (a re-render, a removal), acting on it reports `stale` instead of touching
   dead UI. The agent re-opens the surface to address the fresh tree.

## Build + run

The recipe relies on its dependencies being symlinked into `vendor/` (the same
model as every other recipe):

```bash
mkdir -p vendor
for p in stdlib json objc quartzcore appkit agent_core agent_appkit agent_mcp; do
  ln -s "$(git rev-parse --show-toplevel)/vendor/$p" "vendor/$p"
done
cpc build
./target/debug/appkit_agent
```

## What it prints

```text
--- describe_ui (what the agent sees) ---
{"jsonrpc":"2.0","id":1,"result":[
  {"id":"app/window#0", "role":"window", ...},
  {"id":"save-btn",     "role":"button", "actionable":true,  ...},
  {"id":"name-field",   "role":"input",  "actionable":true,  ...},
  {"id":".../text#2",   "role":"text",   "actionable":false, ...}   // untagged label
]}
--- click save-btn ---
{"jsonrpc":"2.0","id":2,"result":{"outcome":"allowed"}}
(the real Save handler fired: SAVES == 1)
--- set_text name-field (base_version 0) ---
{"jsonrpc":"2.0","id":3,"result":{"outcome":"allowed"}}
--- set_text again with the SAME stale base_version 0 ---
{"jsonrpc":"2.0","id":4,"result":{"outcome":"version_conflict"}}
--- describe_ui through a deny-all gate (consent refused) ---
{"jsonrpc":"2.0","id":5,"error":{"code":-32001,"message":"consent denied"}}
--- click save-btn after the button left the window ---
{"jsonrpc":"2.0","id":6,"result":{"outcome":"stale"}}
(the handler did NOT fire again: SAVES still 1)
```

(The `result` array is emitted as one compact line; it's expanded here for
reading.) Note: the click's `allowed` is matched by `SAVES == 1` — the action
actually actuated the AppKit button, not just the tree.

## From here to a real app

This program focuses on the surface, so it skips the blocking `app.run()` loop. A
real app builds the same tagged UI, calls `app.run()`, and serves the surface to
agents on a background connection:

```cplus
// after building the window and opening the surface:
mcp::serve_uds(surf, sub, allow_external, "/tmp/cplus-agent.sock");
```

`serve_uds` accepts connections on a Unix-domain socket and runs the same
`describe_ui` / actions / events JSON-RPC, one request per line.

For the same surface over a *declarative* facet UI — tags that survive MVU
re-renders — see the `facet_agent` recipe.
