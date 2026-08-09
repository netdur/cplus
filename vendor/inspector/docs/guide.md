# Guide

How the package is meant to be used, why the pieces exist, and the gotchas that
bite. For a fast start see [tutorial.md](tutorial.md); for signatures see
[ref.md](ref.md). The socket namespace is [wire.md](wire.md), and the decisions
behind all of it are [design.md](design.md).

## The three tiers

Writable properties are not one flat set, and only the third needs to know what
control it is talking to.

| Tier | What | Needs a key? |
|---|---|---|
| 1 — the common band | `props::CommonProps` is inline on **every** node: opacity, background, corners, visibility, transform, tooltip | no |
| 2 — flex style | facet's tree **is** flex's tree: width, height, padding, margin, gap, grow, shrink | no |
| 3 — control props | `text`, `title`, `on` — behind the generated typed handles | **yes** |

That shape is why this package is small. Tiers 1 and 2 are uniform across all
38 control kinds and every bare container, with no kind dispatch and no
generated metadata. Tier 3 resolves by key, so an unkeyed node answers
`Unsupported` — a reported limit rather than a hidden one.

## Handles and staleness

A `Handle` is a node pointer plus flex's global removal counter, exactly like
facet's own typed handles and for the same reason: a child lives in its own heap
slot, so its address is stable from insertion until it is itself removed.
Appends, inserts, sibling removals and reorders never move it.

**Any removal anywhere in any tree invalidates every outstanding handle.** That
is deliberately conservative — a removal elsewhere cannot have moved this node —
but it is the check facet already uses, it costs one comparison, and the
alternative is a second identity system to keep in step.

| Operation | Handles after |
|---|---|
| property write | live |
| `insert` | live |
| `remove`, `reparent`, `undo_remove` | **all dead** |

A dead handle answers `Stale`, which means *describe again*. Describing is
cheap, and structural change is rare next to property editing.

### Identity is the address

If you build a UI over the listing, key selection and expansion by the node's
address, never by its row number. A positional id renames every row after an
insert, so the selection follows the slot instead of the node — which reads as
"the selection jumped".

## Choosing a backend

`widget` is written against `insp::Backend`, not against the local walker, so
where the panel runs is a binding at the call site rather than an architecture.

| Want | Use |
|---|---|
| a panel in the app under test | `panel::embedded()` |
| a panel over some other backend | `panel::new(backend)` |
| drive the tree from code | `tree::describe` / `set` / `insert` directly |
| drive it from another process | `mcp::arm(tree::local_backend())` — see [wire.md](wire.md) |
| a build with no walker at all | `insp::Backend::none()` — refuses everything, crashes nothing |

## Reading: three categories, never flattened

`inspect` answers `declared`, `computed` and `native` separately.

- **declared** — facet props and flex style: what the application asked for.
  Writable.
- **computed** — the laid-out frame, attachment, focus: what layout and mount
  decided. Read-only.
- **native** — the platform's own, and empty when no platform module is
  installed.

Setting `width` writes a style; reading the frame reads what came out the other
side. The browser calls these `style.width` and `offsetWidth` and keeps them in
separate panels. A single `width` row would eventually show a number the
developer cannot write back and cannot explain.

## The refusals are the product

An inspector that answers a silent no-op to a misspelled property teaches the
developer that the property does not do anything, which is the opposite of true.
So the distinctions are load-bearing:

| Pair | The difference |
|---|---|
| `UnknownProperty` / `Unsupported` | "you misspelled it" vs "the name was right and this build cannot reach it here" |
| `ReadOnly` / `UnknownProperty` | "layout decided that" vs "no such thing" |
| `NotAContainer` / `UnknownKind` | "that parent cannot take children" vs "no element of that name" |
| `Stale` / `NotFound` | "the tree changed, re-describe" vs "there is no such node" |

The reader is symmetric with the writer: a tier-3 property the inspector cannot
*aim* — a duplicate key, where `find` resolves the first match — is one it
declines to **report**, rather than quietly showing another node's value as
though it belonged to this one.

## Structure is addressing, not a tree editor

`facet/mount` owns every structural operation against the live tree: it creates
the views for an inserted subtree and computes the native slot through
passthrough containers, and on removal it notifies detach while the subtree is
still whole, pulls the views out of the host, and hands the subtree back.

This package adds the addressing and the refusals — the cases mount would
perform faithfully and a developer would not have wanted:

- a **leaf control** as the parent. It hosts no views, so `nearest_host` would
  put the new child's view in the *leaf's* parent while the node sat under the
  leaf: visibly in the wrong place, and wrong in the leaf's measure too.
- a **list, table, tabs or collection**, which own their children through
  recycling or a pane registry that a raw insert fights rather than joins.
- a **window root**, which is closed with `app::close_window`.
- the **panel's own subtree**, because a tool that can delete its own controls
  can end a debugging session by accident — and unlike a property edit, that one
  has no field to type the old value back into.
- a **move into a node's own subtree**, checked *before* the removal, because
  afterwards the answer is still no and the tree is already cut.

`can_host_children` is the predicate: kind 0 (every container facet did not
generate — row, column, stack, card), plus `box` and `scroll`.

### The maker vocabulary is a name, not a kind

Kind 0 covers row, column *and* spacer, so a kind code cannot say which to
build. `Spec` carries the `facet/elements` function name instead, spelled as
source spells it — which is also what lets the journal emit the call that was
made rather than a translation of it. `maker_names()` is the list; adding one is
a line in `make` and a line in `maker_names`.

### Delete keeps the subtree

`mount::remove_child` hands the subtree back and `mount::remove_node` drops it.
This package takes the first and holds it, which is what makes one level of undo
cost nothing but a vector. The stored parent pointer is re-checked against the
mounted windows before a restore: the application may have deleted the parent in
the meantime, and then the honest answer is `NothingToUndo` and to keep holding
the subtree rather than insert it somewhere invisible.

### Where structural editing stops

A button the inspector makes has **no** `on_click` and cannot be given one: a
function pointer is not a value any inspector can carry, and typing one into a
text box is a crash with extra steps. Everything visual and structural is
reachable from here; new *behaviour* is what would need a reloadable module.

## Persistence: a ledger and a journal, both volatile

| | Records | Undone by |
|---|---|---|
| override ledger | the value a property had before the first inspector write | `reset` |
| journal | each structural edit, as the C+ line that would make it | nothing — `undo_remove` restores the tree, not the record |

A structural edit has no "value it used to be", so the ledger cannot hold it and
`reset` cannot undo it. What it has instead is what the property tier already
had: the edit as source. `snippet_for` and `journal_at` are the whole
persistence story, on purpose — an inspector that replayed its own edits at
startup would have become the part of the application that builds the UI.

An edit under an **unkeyed** node journals a comment saying it cannot be
addressed. Inventing a key would produce a line that compiles and edits a
different node.

## Threading

The tree is main-thread-owned: `mount::install` records the UI thread and
`core::touch` asserts on it, because a worker writing the tree is a data race
that otherwise surfaces as a distant, unattributable crash.

An **embedded** consumer is already on that thread and needs nothing. A
**socket** consumer is not, and every request hops through a synchronous
`dispatch_sync_f` that `iplatform::install()` installs. That cost belongs to the
transport, which is where it sits.

### Gotcha: install the platform module before serving

`mcp` calls straight through when no marshal is installed — right for headless
tests and for an in-process caller, and wrong for a socket server. If you serve
over `agent_mcp` without `iplatform::install()`, the first write aborts the
process.

### Gotcha: a caller-supplied property name is not a literal

The override ledger records the property name it was given as a **borrowed
`str`**, sound because every name reaching it came from the `declared_names`
table. A name parsed off a socket into a request-scoped `Text` does not live
that long, and recording a view of it leaves the ledger holding freed bytes.

Nothing crashes, which is what makes it worth stating: `reset` compares the
dangling name against the one it was asked for, matches nothing, and answers
`Ok` having restored nothing at all.

**If you pass a property name you did not get from `declared_names`, exchange it
first**:

```cplus
match itree::canonical_prop(whatever_the_caller_said) {
    option::Option[str]::Some(name) => { /* safe to hand to set / reset */ }
    option::Option[str]::None => { /* UnknownProperty — refuse it here */ }
}
```

### Gotcha: the panel must not restructure itself

The panel addresses its own controls by key and writes them. It never changes
its own structure after `build`, because a rebuild would churn
`flex::removal_count()` and invalidate the handles it had just issued.

## Hiding the panel from itself

`panel::attach` calls `tree::mark_self(mount::node("insp:root"))`. The walk
still descends into it — one descent rule — and the listing carries
`is_inspector` so a consumer can filter. The embedded panel filters; a remote
client is a different consumer and may legitimately want to see it. Editing it
is refused independently of either choice, by `Protected`.

A host that calls `attach` before its tree is mounted gets a panel listing its
own controls, and the panel says so rather than looking like a walker bug.

## Two invariants the package will not break

**It never raises a command bit.** `C_FOCUS`, `C_BLUR` and `C_FLUSH` are
*commands* the backend performs and clears, not state it re-reads. A write that
raised them would re-issue them — and the symptom is first responder leaving
whatever the developer was typing into, on an unrelated node's property edit.
Every setter here names the one bit it changed; the only wholesale re-read is
`core::touch_all`, which is `C_ALL_STATE` and excludes the commands by
construction. There is a test.

**It never writes the platform view directly.** A native-only write leaves
facet's declared state stale and is overwritten by the next sync walk. Every
edit goes through the same setters an application calls, so it takes the same
path: prop, dirty bit, scheduled sync, backend apply. That is also why an
inspector edit is not sacred — the application can write over it, exactly as it
could write over any other value.

## Limits in this version

- **Tier 3 needs a key.** Closes when `gen_contract.py` grows an inspector
  dispatch layer.
- **The maker vocabulary is nine hand-written elements.** Same generator, same
  eventual fix; adding one by hand is two lines.
- **No point picking.** Selecting an element by clicking it in the app was built
  and removed — see [design.md](design.md#point-picking-was-built-and-removed).
  Select from the tree pane, which is strictly more complete: it reaches unkeyed
  and viewless nodes a click could never hit.
- **No handler replacement, no expression evaluation, no memory access.**
- **No source navigation.** facet's runtime `Data` carries no source origin;
  that needs a debug-only origin ID injected during `@ui` lowering. Until then
  the key is the handle and an editor can search for it.
- **macOS only for the overlay and the thread hop.** `inspector/tree` is
  portable and needs no platform at all.
- **Handles are process-local pointers.** A transport sends indices into the
  flat `describe` listing instead — which is why that listing is flat and
  parent-indexed.

## Values

`Value` is a small tagged vocabulary: `Nothing`, `Bool`, `Int`, `Num`, `Str`,
`Color`, `Enum`. Handlers, raw pointers, child nodes and owned collections are
deliberately absent — a value the inspector cannot round-trip is a value it must
not claim to edit.

Three reading rules that are easy to get wrong, and are pinned by tests:

- **A theme token is a name, not a colour.** `vocabulary` reserves 255 for a
  literal `rgba` and 254 for a light/dark pair; every other non-zero token is a
  theme role whose channels are all zero. Treating "non-zero token" as themed
  reports every literal as `token:255`; treating it as "has channels" reports
  every themed colour as transparent black.
- **An unset length reads as `Nothing`.** `auto` has no number to hand back
  unchanged, and `0.0` is a value a UI would write back as a real zero.
- **A mixed padding reads as nothing.** Showing one edge in a single `padding`
  box is how a developer sets all four to the value that happened to be on one
  of them.

`parse_as` reads what arrived as the tag the property already holds, so the
current value is the type declaration. The panel's text fields and the socket
share it, which is what keeps the two transports from drifting.
