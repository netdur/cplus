# agent_android — what it answers, and what it does not

The agent surface for facet's Android backend: the sibling of `agent_appkit`,
`agent_uikit` and `agent_gtk`. agent_core owns the identity registry, auto-ids,
grants, curation and the exposed reduction; this package walks a tree into that
registry and answers live reads and actions against it.

## 1. Decided different — the walk is over FACET'S tree

The other three walk the NATIVE view hierarchy and read an id back off an
associated object or a widget property. This one walks facet's node tree.

Two reasons, and the second is the load-bearing one:

- **Android has no `accessibilityIdentifier`.** facet_android stashes the node's
  key in a keyed `View` tag (MANIFEST §3 there), so a native walk would read
  back — lossily, one JNI call per node — the very thing the tree already holds.
- **Every app on this backend is a facet app.** The Apple and GTK surfaces were
  written for a world where the surface might serve an app facet did not build.
  That is not this world, and paying for it would buy nothing.

The contract with agent_core is identical either way. What changes is where the
`*u8` handle points: on this backend it is a `*core::Node`, which is also why
`set_agent_policy` writes facet's own agent band instead of a side table.

## 2. Not yet built

- **`navigate`, `set_caret`, `read_runs`, `invoke_menu`.** Each returns
  `NotFound`, and the two that carry a `supported` flag report `false` rather
  than a plausible answer — an omission and a false assertion are different
  things.
- **`hit_test`.** Answering it means asking Android which view a point reaches,
  which is a native walk this package deliberately does not do. It needs a
  design, not just an implementation.
- **A re-walk on tree change.** `open` snapshots the shape; live text, frames
  and hidden-ness are read per request, so a screen that changes SHAPE (a
  `switch_to`, an inserted row) needs `open` again. The Apple backends refresh
  from the window; the equivalent hook here is `application::agent_attach`.

## 3. What a click is

A click is the CONTROL'S OWN EVENT PATH — `controls::agent_click` calls the same
function the Java listener adapter calls, one layer below the adapter. An
agent's click and a finger's click are the same code from there down.

Nothing here can drag, pinch or swipe. That is a property of the surface, not a
gap in it: an agent has no hands, and the answer to "the agent needs to drag" is
a click path the UI owes its users anyway.
