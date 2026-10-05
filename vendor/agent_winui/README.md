# agent_winui

Native WinUI 3 backend for `agent_core`, written in C+ over generated WinRT
bindings. It has **no Facet dependency and no C++ application bridge**.

```toml
[dependencies]
stdlib = "*"
winrt = "*"
winui = "*"
agent_core = "*"
agent_winui = "*"
agent_inapp = "*"
```

On the window's UI thread, after constructing a native WinUI Window:

```cplus
import "agent_winui/agent_winui" as agent;
import "agent_inapp/agent_inapp" as inapp;
import "agent_core/identity" as identity;

// Match Result[Surface, winrt::Error] in production (must is the example helper).
var surface = must::[agent::Surface](agent::open(window.object()));
check(surface.pin(button.object(), "save"));
check(surface.pin(editor.object(), "title"));
check(surface.pin(payment_panel.object(), "payment", identity::policy_protected()));
let session = inapp::open(#addr_of(surface) as *u8, agent::mcp_backend());
let nodes = session.describe_ui();
let clicked = session.click("save");
```

Keep the surface at a stable address while any session borrows it. Pinning owns
the id and retains the control; `unpin(object)` releases the registration.
`update` supports renderer replacement under the same key. `set_policy` changes
policy without replacing a registration. `close()` detaches text listeners and
releases all pins; call it on the UI thread before XAML shuts down. Do not reuse
a session after its Surface owner is destroyed.

Only explicit pins are exposed. `AutomationProperties.AutomationId` alone never
grants access. Every request rebuilds the live window-content tree, so detached
controls cannot be invoked even though a pin retains their COM object. Both
snapshots rebase parent indexes into the returned array. Excluded subtrees are
absent from both snapshots; protected/private content is gated throughout native
template descendants. Password content and descendant text are never read;
password filling follows the declared policy, like `agent_core`'s autofill contract.

Button clicks use native Invoke, Toggle or SelectionItem automation patterns.
TextBox/PasswordBox writes, focus, native values and peer bounds are supported.
TextBox versions track synchronous `TextChanging`, including edits outside the
agent. `Session.text_version(id)` returns the version for a permitted read; pass
it to `set_text` as `base_version`. Shared MCP `read_text` includes `version`
when the backend supports it and the content is readable. No version is reported
by older backends until they implement the optional vtable slot.

## Threading and MCP

The backend vtable is synchronous and **UI-thread-only**. `open` checks thread
access and every operation asserts ownership. A worker must queue the whole
request with `agent::enqueue(window_dispatcher_queue, callback, context)` and
send the response after the callback runs. Context must survive delivery; an
enqueue refusal means the queue is shutting down. Never block the UI thread
waiting for its own queued callback.

`agent_mcp::handle_request(surface_ptr, agent::mcp_backend(), subscriber, gate,
line)` works on that UI thread. No transport or permissive authentication policy
is installed by this package. Do not pass this vtable directly to a worker-thread
`accept_loop`.

For external HTTP clients, use the optional
[agent_winui_mcp](../agent_winui_mcp/README.md) adapter. It owns the worker and
UI dispatch lifecycle; this native backend retains no MCP dependency.

## Current boundaries

- One native Window per Surface; no popup enumeration or multiwindow aggregator.
- Visual-tree controls only: unrealized list items and WebView DOM are not walked.
- Range values are readable; range editing and rich editor operations are not implemented.
- Hit testing and styled runs report unsupported; caret/menu verbs refuse.
- `wired` is unknown: invoking a native pattern cannot prove an app attached a handler.
- Controls without automation peers have no reported peer bounds/class/name.
- Exposure and versions belong to this registration's lifetime; after replacing
  or re-pinning an editor, discard prior read/edit requests and read again.

The independent app in [examples/agent_winui_smoke](../../examples/agent_winui_smoke)
checks real controls, grants, privacy, Unicode, version conflicts, shared MCP and
in-app calls, removal and cleanup. Run from the repository root:

```powershell
./tools/test_agent_winui.ps1
./tools/test_agent_winui.ps1 -Release
```

For a Facet app, use the optional
[facet_agent_winui](../facet_agent_winui/README.md) connector.
