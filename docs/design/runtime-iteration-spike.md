# Runtime iteration spike: hot reload and a live Facet inspector

Status: approach 2 is BUILT — `vendor/inspector`, with `examples/inspector_probe`
as the probe. It reads and writes properties, adds and removes nodes, and is
reachable both in-app and over the agent's MCP socket. Approach 1 remains
research: nothing about behaviour — a handler, a function body — is reachable
from any of it, which is the boundary the two approaches actually divide on.

This document records two related approaches for changing a running C+
application during development:

1. loading newly compiled C+ code into a stable host process;
2. inspecting and manipulating an already-mounted Facet UI.

The second approach is much narrower, but it fits Facet's current architecture
better and can deliver a large part of the useful UI-development experience
without requiring general code hot reload.

## What shipped, and where this document was wrong

The inspector's own design notes are [vendor/inspector/docs/design.md](../../vendor/inspector/docs/design.md).
Three things below did not survive contact with the code:

**The narrow initial scope was drawn in the wrong place.** This document
proposed AppKit-only *and* keyed-node-only. Keys turned out to be needed for
almost nothing: `props::CommonProps` is inline on every node and Facet's tree
*is* Flex's tree, so opacity, background, corners, visibility, transform, width,
padding, margin, gap, grow and shrink are all uniform across every control kind
and every bare container, with no key and no kind dispatch. Only `text`, `title`
and `on` need a key, because those go through the generated typed handles. The
generated property metadata this document treats as a prerequisite is not needed
for the useful part at all.

**Embedded versus external was a false choice.** This document weighs an in-app
window against an external tool. But `agent_mcp` and `agent_inapp` are both
consumers of one `agent_core` vtable, and the inspector copies that: the panel
is written against `inspector::Backend`, so where it runs is a binding at the
call site rather than an architecture. The transport limits discussed under
"Transport considerations" are real, and they are a socket adapter's problem,
not the inspector's.

**Handles did not need a revision scheme.** Facet's own typed handles are a node
pointer plus Flex's global removal counter, and the same shape works here for
one comparison. `describe` answers a flat, parent-indexed vector, so an index
into that listing is the wire address a remote client uses.

**Structural editing was blocked on policies that mount then grew.** This
document, and the inspector's own notes, listed insert/delete/reparent as
needing identity, focus, selection, scroll and component-lifecycle policies that
did not exist. `facet/mount` has since grown `insert_child`, `add_child`,
`remove_child`, `remove_node` and `remove(key)` against the live tree, each
handling exactly those concerns. The inspector's structural verbs are therefore
an addresser and a set of refusals over mount, not a tree editor — see the
package's design notes. What they do *not* reach is behaviour: a button the
inspector makes has no handler and cannot be given one, which is the real
boundary between approach 2 and approach 1.

One thing this document flagged that proved exactly right: keep it away from the
agent surface, and let `pick_at` exist only here.

One thing it got half right, and this is now built: the inspector's *capability*
model must stay separate from the agent's, but the *transport* need not be a
separate server. `agent_mcp` grew one erased namespace hook and `inspector/mcp`
registers `inspector.` into it, so there is no second server, socket, teardown
hook or consent gate. `agent_core::Backend` did not gain a debug mode, and the
dependency runs `inspector` → `agent_mcp` and never back. The "Transport
considerations" section below is therefore answered rather than open: the
listing is flat and parent-indexed so an index is the wire address, `inspect`
is a per-node read rather than part of the tree dump, and the 8 KiB request
buffer bounds requests, not answers.

The cost this document did predict correctly is the thread. `mount::install`
records the UI thread and `core::touch` asserts on it, so every socket-borne
write hops through a synchronous `dispatch_sync_f` installed by the platform
module — the embedded panel needs none of it, which is exactly where the cost
was said to belong.

## Executive conclusion

A browser-like live inspector is feasible without changing the C+ language. It
can inspect mounted Facet nodes and apply typed runtime overrides through the
setters and dirty-bit machinery that already updates native views.

General code hot reload is also possible, but only behind an explicit runtime
boundary. The practical design is a stable executable that owns long-lived
state and loads versioned C+ dynamic libraries through a small C-compatible
function table. This is plugin reloading, not transparent replacement of
arbitrary code in an application that was not designed for it.

The recommended order is:

1. build a developer-only Facet inspector for keyed nodes and common props;
2. add generated property metadata and optional runtime override persistence;
3. consider reloadable code modules only for applications that need live
   behavior changes as well as visual changes.

Step 1 is done, and has since grown structural editing: insert, delete,
reparent and one level of undo, over `facet/mount`'s live-tree verbs. Step 2's
generated metadata turned out not to be a prerequisite (see above); what remains
of it is control-specific properties beyond `text`, `title` and `on`, and a
maker table beyond the nine hand-written elements the structural verbs can
build. Override persistence was deliberately not built — the inspector generates
a C+ line to paste, for property and structural edits alike, so a development
tool cannot quietly become an application state store.

## Approach 1: compiled-code hot reload

### What is realistically possible

C+ is an ahead-of-time systems language. A normal executable contains direct
calls, concrete type layouts, function pointers, destructors, and native GUI
callbacks. Once that code is running, the operating system has no semantic
understanding of which instructions may safely be replaced.

Consequently, arbitrary transparent hot reload cannot be added purely as a file
watcher. A workable solution requires a boundary that was designed to be
reloadable:

```text
stable host process
    owns application state, windows, scheduler and reload coordinator
        |
        | versioned ABI/function table
        v
reloadable C+ dynamic library
    owns replaceable behavior for the current generation
```

The host watches source files, invokes `cpc` to build a new dynamic library,
loads that library under a unique filename, validates its ABI, and atomically
publishes the new function table. Existing calls finish on the old generation;
new calls use the new table.

This is the same general shape as a plugin system. The useful trick is to load
successive generations side-by-side rather than trying to rewrite machine code
inside the original executable.

### ABI boundary

The library should export one stable entry point that returns a versioned table
of functions. The boundary should use C-compatible data only:

- fixed-width integers and floats;
- pointers plus lengths rather than language-owned strings or vectors;
- opaque host-owned handles;
- explicit allocation and release functions when ownership crosses the boundary;
- an ABI version and preferably a layout/capability hash.

Conceptually:

```text
PluginApi {
    abi_version
    initialize(host_api, previous_state)
    handle_event(app_handle, event)
    build_or_update_ui(app_handle, ui_api)
    prepare_reload(app_handle)
    shutdown(app_handle)
}
```

The exact table can be much smaller for an initial experiment. The important
property is that the host and plugin agree on a deliberately small binary
contract rather than sharing arbitrary C+ objects.

### State ownership

Long-lived state should remain in the stable host or in versioned opaque blobs.
Moving ordinary C+ structs across generations is unsafe when a field is added,
removed, reordered, or changes type.

There are three defensible strategies:

1. Host-owned state with accessor functions. This is the safest initial model.
2. Serialized plugin state with an explicit migration function.
3. Versioned opaque plugin state retained by its original generation until it
   can be destroyed by code from that same generation.

The first is the best fit for a spike. It allows function bodies to change while
keeping a counter, document model, or navigation state alive in the host.

### Function replacement

All calls that may be replaced must go through indirection. The host can keep a
dispatch table indexed by a stable function or handler ID. UI callbacks then
enter a host-owned trampoline, which reads the current table and calls the
current implementation.

This can replace behavior, but only while signatures remain compatible. It does
not safely support changing:

- a function's ABI or captured state layout;
- the layout of a live struct;
- generic instantiations already compiled into the host;
- callback payload ownership;
- application state schemas without migration.

This indirection also needs to be designed into callbacks before they are
registered. A native control holding a raw function pointer into generation 1
will continue to call generation 1 even after generation 2 loads.

### Loading and unloading

Each rebuild should use a unique library filename. Loading a new path avoids
dynamic-loader caching and permits the old and new generations to coexist.

The first implementation should not unload old libraries during the development
session. Safe unloading is difficult because old code may still be referenced
by:

- native target/action or delegate callbacks;
- worker threads and queued work;
- timers and subscriptions;
- function pointers stored in Facet props;
- live values whose destructor is in the old image;
- stack frames currently executing old code.

Keeping a few development generations mapped costs memory but avoids executing
an invalid address. Unloading can be added later only after generation tracking,
callback revocation, task quiescence, and ownership rules are proven.

### Relationship to Facet

Facet does not rebuild a virtual description on every state change. It builds a
retained tree once and performs keyed direct writes afterward
([guide](vendor/facet/docs/guide.md#keyed-direct-updates)). This means a reloaded
function cannot simply return a new description and expect a framework diff to
reconcile it.

Possible integration models are:

- Keep the existing Facet tree in the host and reload only handlers/behavior.
- Give the plugin an API of host-owned, keyed Facet mutations.
- Replace a deliberately reloadable subtree using Facet's existing live tree
  operations, accepting that local widget state and cursors in that subtree may
  be reset.

The first two are safer. Facet does have `replace`, `set_content`, `add_child`,
`insert_child`, and `remove_child` operations
([mount.cplus](vendor/facet/src/mount.cplus#L187)), but using them as a general
hot-reload reconciler would require identity, focus, selection, scroll, and
component-lifecycle policies that do not currently exist.

### Other possible approaches

#### Process restart with state restoration

Watch, rebuild, relaunch, and restore a serialized development state. This is
the simplest and most reliable general solution. It is not true hot reload, but
it works for all code changes and has clear failure behavior.

#### Embedded interpreter

Compile selected functions into bytecode and run them through an interpreter.
This provides strong replacement semantics but creates a second execution
engine, foreign-function boundary, debugger story, and performance model.

#### JIT compilation

An LLVM ORC-style JIT could replace symbols through indirection. It is a major
compiler/runtime project and still cannot automatically migrate arbitrary live
data layouts.

#### Machine-code patching

Overwriting function entry points or process memory is platform-specific,
unsafe around in-flight calls, hostile to code signing, and does not solve data
layout or callback lifetime. It is not recommended.

### Suggested compiled-code proof of concept

Use a purpose-built sample rather than trying to reload an existing application:

1. The host owns a small state object and a Facet window.
2. A plugin exports a versioned function table with one event handler and one UI
   update function.
3. The host loads generation 1 and invokes it through a trampoline.
4. Recompile generation 2 under a unique filename.
5. Load and validate generation 2, then swap the published table.
6. Confirm that the next click uses generation 2 while host-owned state and the
   window remain alive.
7. Leave generation 1 loaded.
8. Reject an incompatible ABI cleanly without disturbing the current generation.

Success would prove behavior replacement. It would not yet prove arbitrary type
migration, structural UI reconciliation, or safe unloading.

## Approach 2: browser-like live Facet inspector

### Why this is a strong fit

Facet already retains all of the state an inspector needs. A node's `Data`
contains its key, control kind, typed props, common props, native view, and dirty
word ([facet.cplus](vendor/facet/src/facet.cplus#L20)).

A Facet setter already performs the complete live-edit pipeline:

1. update the node's stored property;
2. set the corresponding dirty bit;
3. schedule a UI sync;
4. have the backend apply the changed property to the existing native view.

The write and scheduling seam is in
[`facet.cplus`](vendor/facet/src/facet.cplus#L238), and the backend sync walk is
in [`mount.cplus`](vendor/facet/src/mount.cplus#L454).

This means an inspector should mutate Facet nodes through the same setters an
application uses. It should not write directly to `NSView`: a native-only write
would leave Facet's declared state stale and could be overwritten by the next
sync.

### Browser DevTools mapping

| Browser concept | Facet equivalent |
|---|---|
| DOM tree | Mounted Facet node tree |
| DOM node ID | Inspector handle; stable key where available |
| Styles panel | Facet's declared common and control-specific props |
| Computed panel | Flex frame/layout plus platform-owned live state |
| Element picker | Platform hit test mapped back to a Facet node |
| Highlight overlay | Non-interactive platform overlay around the selected node |
| Console `$0` | The currently selected Facet node handle |
| CSS edit | A typed call to an existing Facet setter |

Chrome keeps these concerns separate in its DOM, CSS, Overlay, and Runtime
protocol domains. Facet should follow the first three parts; arbitrary runtime
evaluation is not necessary for the narrow feature.

### Existing `agent_*` foundation

The existing agent packages already provide several reusable pieces:

- a live native-tree snapshot with IDs, roles, classes, frames, text, and
  parents ([backend.cplus](vendor/agent_core/src/backend.cplus#L31));
- `describe_ui`, `click`, `hit_test`, `scroll_to`, `set_text`, and event polling
  over JSON-RPC ([agent_mcp.cplus](vendor/agent_mcp/src/agent_mcp.cplus#L229));
- main-thread marshaling for AppKit operations;
- stable developer IDs and generated IDs;
- optimistic concurrency for text and stale-operation outcomes;
- a transport-free in-process consumer in `agent_inapp`.

The full `describe_ui` view is a useful diagnostic start, but the AppKit backend
currently walks the native `NSView` hierarchy
([agent_appkit.cplus](vendor/agent_appkit/src/agent_appkit.cplus#L689)). A
developer inspector should instead walk Facet's mounted tree and join native
information when a node has a backing view.

Inspecting only the native tree would miss or misrepresent:

- pure-layout Facet nodes with no native view;
- spans, menu items, and other semantic non-view nodes;
- Facet control kinds and declared properties;
- the distinction between declared layout and computed native geometry;
- property provenance and dirty state.

### Keep the inspector separate from the agent capability

The agent surface is deliberately curated and permissioned. It hides unexposed
nodes, limits actions to declared affordances, and prohibits point-addressed
actions. Those rules protect a user-facing automation surface.

A developer inspector needs different powers:

- see internal and unexposed nodes;
- select a node by clicking a point in the application;
- modify visual and layout properties that are not user affordances;
- possibly observe implementation-specific native information.

Therefore the inspector should have a separate `InspectorBackend` or
`inspector.*` protocol namespace. It may reuse JSON-RPC framing, identity ideas,
event queues, and main-thread marshaling, but it should not weaken
`agent_core::Backend`. In particular, a `pick_at(x, y)` operation belongs only
to the trusted developer inspector, never to the agent action surface.

It should also be debug-build-only, explicitly enabled, and exposed through an
owner-only local socket and optionally a token. Inspector access is effectively
arbitrary UI mutation and is more powerful than the normal agent surface.

### Narrow initial scope

The first implementation should be AppKit-only and keyed-node-only.

Keys make the initial version unusually small:

- `mount::find(key)` already behaves like `getElementById`, with scoped lookup
  available for duplicate keys across windows
  ([mount.cplus](vendor/facet/src/mount.cplus#L128));
- a keyed Facet node gets a native view;
- on AppKit, its key becomes the view's accessibility identifier
  ([views.cplus](vendor/facet_appkit/src/views.cplus#L171));
- the agent surface already uses the same identifier.

The initial inspector should support:

- enumerate mounted keyed nodes;
- select a node from the tree;
- hover and click to select in the live application;
- highlight the selected node without affecting layout or hit testing;
- inspect key, Facet kind, ancestry, computed frame, attachment/focus state,
  native class, and a small property set;
- edit common visual/layout properties;
- reset an individual override;
- copy the current edit as a C+ setter snippet.

Suggested writable properties:

- visible, enabled, and input-transparent state;
- opacity;
- width, height, grow, and shrink;
- padding, margin, and gap;
- background color;
- label text, button title, and input text;
- a small selection of typography properties.

Do not initially expose:

- callback or function-pointer replacement;
- arbitrary memory access;
- arbitrary C+ expression evaluation;
- structural insert/delete/reparent operations;
- complex owned collections or application model pointers.

### Protocol shape

A small typed protocol is sufficient:

```text
inspector.list_tree(root?, depth?) -> { revision, nodes }
inspector.inspect(handle)          -> { declared, computed, native }
inspector.set(handle, property, value, base_revision)
inspector.reset(handle, property, base_revision)
inspector.highlight(handle?)
inspector.set_pick_mode(enabled)
```

Useful notifications are:

```text
inspector.selection_changed
inspector.tree_changed
inspector.node_changed
```

The console can present a browser-like object without embedding a C+
interpreter:

```text
$0 = inspect("save")
$0.get("opacity")
$0.set("opacity", 0.65)
$0.set("padding", 12)
$0.set("background_color", "#2277ff")
$0.reset("opacity")
```

Each command becomes a validated typed request. Invalid property names or value
types must return an error rather than becoming a silent no-op.

### Node identity and stale handles

A key alone is not a complete inspector handle:

- duplicate keys across separate windows are legal;
- unkeyed nodes will eventually need handles;
- nodes may be removed or replaced while a client is inspecting them.

The protocol should issue opaque handles scoped to a tree revision, for example
`{ window, node, revision }`. A command carrying an old revision should return
`Stale` and make the client inspect again.

For the keyed-only version, commands can resolve the key afresh inside the
selected window rather than retaining a raw node pointer. Later, a complete
Facet-tree registry can assign deterministic path IDs to unkeyed nodes. The
existing agent registry's stable path-ID rules are useful prior art, but its
small role vocabulary and exposure semantics should not define inspector
identity.

### Property metadata without reflection

Runtime reflection is not required. Facet already generates its control props,
setters, readers, dirty bits, and contract from one ledger
([gen_contract.py](tools/gen_contract.py#L2455)). The generated contract currently
contains 408 declared verbs over 38 generated controls
([contract.md](vendor/facet/docs/contract.md#L1)).

The generator can eventually emit an inspector dispatch layer:

```text
kind -> property descriptors
kind + property name -> typed getter
kind + property name + typed value -> existing setter + dirty bit
```

A small tagged `Value` vocabulary can cover the first version:

- boolean;
- integer;
- number;
- text;
- color;
- named enum;
- simple structured values such as insets or corners.

Handlers, raw pointers, callbacks, child nodes, and complex owned collections
should not be serializable inspector values.

### Declared, computed, and native values

The inspector should not flatten all state into one property list. It should
show three categories:

1. **Declared**: values stored in Facet props or Flex style.
2. **Computed**: the laid-out frame and derived geometry.
3. **Native**: platform-owned current state such as native class, focus, or
   control value.

This distinction is important for layout. Facet's own documentation notes the
same distinction as browser `style.width` versus `offsetWidth`: setting width
writes style, while reading `width()` currently reads the laid-out frame
([facet.cplus](vendor/facet/src/facet.cplus#L967)).

### Picker and highlight implementation

On AppKit, picker mode can hit-test the content view at the current pointer,
walk from the hit view toward its ancestors, and resolve the nearest Facet node.
Keyed views can be resolved by accessibility identifier. A later complete
implementation can also maintain or scan a native-view-to-Facet-node mapping.

The selected node should be highlighted by a transparent, non-interactive
overlay or drawing layer. The overlay must:

- not participate in Facet layout;
- not intercept normal hit tests outside picker mode;
- be excluded from both the agent and inspector trees;
- update when layout or window geometry changes.

Pure-layout nodes without views require their Flex frame to be translated
through ancestor coordinates. That is a later step; keyed backed nodes keep the
first version simple.

### Runtime overrides

Inspector writes should initially be volatile. They disappear when the process
restarts and may disappear when the application replaces the selected node.
That makes behavior predictable and prevents a development tool from silently
becoming an application state store.

An optional later override ledger can retain `{ window/key, property, value }`
and reapply compatible values when a matching node appears again. This would
make navigation and subtree replacement feel more like hot reload. It must still
reject property/type mismatches and ambiguous duplicate keys.

Generating a C+ setter snippet is a simpler first persistence mechanism:

```text
label::find("status").set_text("Ready").set_opacity(0.8)
```

The developer can copy the successful experiment into source explicitly.

### Source navigation

Basic inspect and edit functionality needs no compiler change. Exact “open the
source that created this node,” however, is not currently available: the runtime
Facet `Data` structure carries no source origin.

The compiler already has file-aware spans during parsing
([lexer.rs](cplus-core/src/lexer.rs#L3)), so a later debug-only enhancement could
inject a compact source-origin ID during `@ui` lowering and place it on each
node. A side table would map that ID to file, line, and column without retaining
large path strings on every node.

Until then, the inspector can expose the stable key and let an editor/tool search
the workspace for that key. This will be approximate when a key string appears
more than once.

### Transport considerations

The present `agent_mcp` server uses newline-delimited JSON-RPC over a Unix-domain
socket and reads requests into an 8 KiB buffer
([agent_mcp.cplus](vendor/agent_mcp/src/agent_mcp.cplus#L487)). A rich inspector
tree can exceed that model quickly.

The inspector transport should support at least one of:

- subtree/depth queries and property reads on demand;
- pagination;
- length-prefixed or otherwise non-truncating messages;
- a small local bridge if the frontend is an HTML/WebSocket application.

An in-app Facet inspector window is another option and can use a transport-free
session, but it must exclude its own subtree from inspection. An external
developer-tool window has a cleaner separation from the inspected application.

### Suggested inspector proof of concept

The first complete vertical slice should demonstrate:

1. The target application remains running.
2. The inspector enumerates keyed Facet nodes in one AppKit window.
3. Hovering a tree entry highlights the corresponding live view.
4. Picker mode selects an element clicked in the target application.
5. The inspector reads its type, key, frame, common props, and native class.
6. Editing text, visibility, padding, and background color uses Facet setters and
   updates the live application on its normal sync path.
7. An invalid property or value type returns a typed error without crashing.
8. Removing or replacing the node makes the old handle return `Stale`.
9. The inspector can reset the override or generate an equivalent C+ setter
   snippet.

This proves the useful development loop without requiring structural editing,
source mapping, generated metadata for every control, or code hot reload.

## Comparison

| Capability | Dynamic-library reload | Live Facet inspector |
|---|---:|---:|
| Change visual properties | Possible, but indirect | Strong fit |
| Change layout values | Possible, but indirect | Strong fit |
| Change text/control state | Possible | Strong fit |
| Change event-handler behavior | Yes, behind dispatch table | No |
| Change arbitrary function bodies | Only inside reloadable modules | No |
| Preserve process/app state | Yes, if host-owned | Yes |
| Preserve widget-local state | Depends on integration | Yes for property edits |
| Change live type layouts | Requires explicit migration | Not applicable |
| Needs language changes | No for plugin boundary | No for core inspector |
| Needs applications designed for it | Yes | Only explicit debug enablement/keys |
| Initial implementation risk | Medium/high | Low/medium |

## Recommended combined direction

Treat the two mechanisms as complementary rather than competing:

- The Facet inspector is the default rapid UI iteration tool. It edits the live
  retained tree through existing setters and covers most visual experimentation.
- Process restart with optional state restoration remains the general fallback
  for arbitrary code changes.
- A dynamic-library reload boundary can later serve applications that need to
  change behavior without restarting, provided their state and callbacks are
  intentionally routed through the stable host.

The inspector should be implemented first. It has a narrow safety boundary,
reuses Facet's strongest existing property—the retained keyed tree—and does not
force the language or every application into a hot-reload architecture.
