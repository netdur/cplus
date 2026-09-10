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
- ~~The UI-thread hop.~~ **BUILT.** Every verb packs its arguments into a job,
  hands it to `mainthread::run_on_ui` and waits; the work happens on the UI
  thread, where facet's tree and this backend's JNIEnv both live. On the UI
  thread already — the in-app assistant's path — it is a direct call with no
  queue and no wait, which is what makes it affordable on every verb rather than
  only the ones that obviously need it.

  The serve worker attaches itself through `AttachCurrentThreadAsDaemon`, so it
  has an env of its own for the one call it makes: posting the job. As a DAEMON
  deliberately — a non-daemon attachment keeps the VM alive, and the accept loop
  lives for the life of the app.

  ONE JOB SLOT, because there is one serve worker and it waits for its own job
  before posting another. A second poster is refused rather than raced; the
  answer to two would be a queue.

- ~~Rows.~~ **BUILT, and live.** A realised row is not a child of the list node,
  so `walk_rows` reaches across into the recycler for the rows currently ON
  SCREEN (an agent cannot click a row that has no view, and reporting one would
  be reporting something it cannot act on). A tree names its rows through
  `row_id`; a list has no name for one, so agent_core gives it a positional
  auto-id. A row is clicked through its LIST — the row's own node is a bare
  container with no handler — which the registry's `parent_of` makes a lookup
  rather than a search.

## 3. What a click is

A click is the CONTROL'S OWN EVENT PATH — `controls::agent_click` calls the same
function the Java listener adapter calls, one layer below the adapter. An
agent's click and a finger's click are the same code from there down.

Nothing here can drag, pinch or swipe. That is a property of the surface, not a
gap in it: an agent has no hands, and the answer to "the agent needs to drag" is
a click path the UI owes its users anyway.
