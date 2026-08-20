# Guide

What facet is, the model it commits to, and what a backend has to fill. Fast
start: [tutorial.md](tutorial.md). API: [ref.md](ref.md). Every declared verb:
[contract.md](contract.md).

## What it is

facet is a portable description of a user interface. It owns the vocabulary
and the tree; it draws nothing. A backend reads the tree and produces native
views: `facet_appkit` on macOS.

The vocabulary is not invented. It is bootstrapped from the ledger's portable
surface, curated row by row, and generated. That is why `contract.md` names a
ledger row for almost every verb, and why a verb that is not there is not a
missing feature but a decision recorded elsewhere.

## The tree is flex_layout's tree

A facet node IS a `flex_layout` node. Geometry, style and children are flex's;
facet's own per-node data rides in the attachment slot flex already releases.
Nothing is stored twice, and layout is not a second system to keep in step.

Consequences worth knowing:

Layout verbs (`grow`, `padding`, `gap`, `justify`, `align`) are flex's and
behave as CSS flexbox does.

Anything flex supports but facet does not wrap is reached with `node()`.

`display: none` is how a node leaves the layout without leaving the tree.

A node can also decide that for itself, by naming a size band instead of a
number — `sidebar.hide_in("compact")`. The layout pass re-decides on every
pass, against the box the node actually sits in rather than the window, so
there is no size observer to arm and nothing to re-run on resize. See
`facet/bands` in [ref.md](ref.md).

## Keys

A key is an ADDRESS. `find("save")` resolves it, the agent surface uses it as
its id, and the backend sets it as the platform's accessibility identifier.

Identity is a different question and has a different name. A list row's
identity is `id`, never `key`, because identity is derived from the data while
an address is chosen by the author.

An unkeyed pure-layout container costs no native view: it passes its children
to the nearest host. Giving it a key gives it a view, because an address has to
resolve to something.

## Keyed-direct updates

There is no virtual DOM, no re-render, and no diff.

A handler finds the control that changed and writes it:

```cplus
match label::find("count") {
    option::Option[label::Label]::Some(l) => { let _l: label::Label = l.set_text("2"); }
    option::Option[label::Label]::None => { }
}
```

The write marks that node's dirty word. The backend applies only the bits the
word names, on the next tick. Nothing else in the tree is visited.

This is a deliberate choice, not a missing feature. A description that is
rebuilt to be compared is a description built many times; facet builds it once
and addresses it afterwards.

## The dirty word

Every node carries a `u64`. A write sets the bit for the verb it changed, and
the backend reads the bits to decide what to re-apply.

The low bits are the control's own (`P_*`, per module). The high bits are the
shared band (`C_*`): opacity, background, shadow, clip, transform, focus,
layout, and the handler set.

Command bits are acted on and cleared: `C_FOCUS`, `C_BLUR`, `C_ANIMATE`,
`C_CANCEL_ANIMATIONS`, and the batch flush. `C_LAYOUT` means re-run the layout
pass rather than re-read props.

### Animation

`set_opacity` / `set_scale` / `set_rotation` / `set_translation_*` **snap**.
To interpolate, use the matching command:

```cplus
match label::find("banner") {
    option::Option[label::Label]::Some(l) => {
        let _l: label::Label = l.animate_opacity(to: 0.0f64);
    }
    option::Option[label::Label]::None => { }
}
```

End values land in the real props (so `opacity()` / `scale()` stay true for
reads and the agent). Timing defaults to 250 ms and `Easing::SinInOut`; pass
`duration:` and `easing:` to override. `cancel_animations()` aborts in-flight
motion and snaps presentation to the model. Progress has the same shape as
`animate_progress(to:, duration:, easing:)`.

**The transform animates as a unit.** Scale, rotation and translation are one
matrix — the backend rebuilds all nine numbers together, so it cannot
interpolate the rotation while snapping the scale. Two transform verbs in one
tick are one animation over the composed result, and a `set_*` on any transform
number cancels a pending transform animation outright.

**The start value has to reach the view a tick earlier.** There is one field per
property and one apply per tick, so this is *not* a fade-in:

```cplus
n.set_opacity(0.0f64);            // dead: overwritten below, never applied
n.animate_opacity(to: 1.0f64);    // animates from what the view already shows
```

Both writes land in `opacity`, the second wins, and the animation travels from
1.0 to 1.0 — nothing moves. facet prints a line to stderr when it sees the pair,
because the failure is otherwise silent. Set the start where it gets its own
apply — at build time, or from `on_attach` — and animate after. To re-run it
later, separate the two with a TIMER (`services::after`): a main-queue hop
(`services::run_on_main`) is drained in the same run-loop turn as the click that
scheduled it, ahead of the sync that applies dirty nodes, so the snap and the
animate can still collapse into one apply.

```cplus
label("hi", key: "banner", opacity: 0.0f64)     // applied at mount
    .on_attach(fade_in)                          // animates on the next tick

fn fade_in(n: *core::Node) {
    core::animate_opacity(n, 1.0f64, vocab::Duration::of_seconds(0.4f64),
                          vocab::Easing::SinOut);
    return;
}
```

## Components, screens, and the app

`Component` supplies a tree from `build(ref this)`. State is the struct's own
fields; handlers reach it through the ctx pointer the tree bound. There are no
statics in the model.

`Lifecycle` adds `on_attach` and `on_detach`, fired by the mount walk after the
tree is in place and before it comes down.

`Screen` adds `chrome()` (the window) and `menu_items()` (this screen's
contribution to the app menu, merged each time the screen is shown).

`App` holds named routes and runs the loop. `nav::go`, `nav::push`, `nav::pop`
and `nav::quit` are the intents it acts on.

## The mount seam

A backend fills install structs and nothing else. There is no other registrar;
a hook family outside these is drift.

| Struct | What it answers |
|---|---|
| `Renderer` | create, apply, insert, remove, view_release, schedule |
| `Scheduler` | run_on_main, after, cancel_after, observe_size, cancel |
| `KeyReader` | reading a key event |
| `SenderReaders` | resolving a handler's sender back to a node |
| `AgentHooks` | serving the agent surface, attaching a window, pinning a node's agent tier |

Plus two slots: `theme::set_is_dark_fn` and `theme::set_theme_changed_fn`.

A zero field keeps the portable no-op, so a partially implemented backend is
degraded rather than broken.

An embedded assistant uses `facet_agent/agent::in_app()`. This opens a typed
in-process session over the same attached surface; it does not connect to the
application's MCP socket. The provider loop feeds `describe_ui()` to the model,
maps model tool calls to session methods, returns each `Outcome`, and repeats
until the model answers the user.

The session carries a capability grant — `auth::operator()` by default, which
reads and drives the app and opens no tier. `in_app_with_grant(g)` opens a wider
one, and it is a NEW session rather than a widened one so a permission the user
approved for a task does not outlive it.

Mark the assistant's own panel `.set_agent(Agent::Hidden)` rather than leaving
it unkeyed: keys are load-bearing for your own `find()`, and `Hidden` takes the
whole subtree out of the agent's world by marking its root.

## What an agent may have: `.set_agent(...)`

The shared band carries one more fact about a node — what an agent may do with
its **content**. It is separate from exposure, which only answers whether the
agent knows the node exists.

```cplus
var box_n: core::Node = ui::column(fields, key: "payment");
core::set_agent(#addr_of(box_n), vocab::Agent::Protected);
```

| Value | In the agent's tree | Its value |
|---|---|---|
| `Agent::Open` | yes (default) | readable |
| `Agent::Protected` | **yes, named** | needs a grant carrying `read_protected` / `act_protected` |
| `Agent::Private` | **yes, named** | needs `read_private` / `act_private` — bits an app declares and never mints |
| `Agent::Hidden` | no | — |

Protected and Private are both **visible**, and that is the point: an assistant
can say "the card number is still blank" because it knows the field is there and
what it is called. It just has no digits. A field left unkeyed says nothing at
all, and costs you the address your own code needs.

Protected is the card number — there *is* a grant that opens it, and an app that
wants autofill mints `auth::protected_operator()` after the user approves.
Private is the CVC: you declare the tier and never mint the bit.

**It inherits, strictest wins.** Mark the box, not each field: the leak is never
the field you remembered, it is the "Card ending 4242" label beside it. A
`Private` field inside a `Protected` box stays Private.

Two things worth knowing:

- A `text_field(secure: true)` becomes an `NSSecureTextField`, which the agent
  surface treats as **Private with no declaration at all**. Say
  `.set_agent(Agent::Open)` if you actually want an assistant to read one.
- Give a gated field an `.accessibility_label`. The agent surface names a node
  from that label, falling back to the widget's own content — and that fallback
  is blocked at any non-Open tier, so an unlabelled card field tells a model
  only its key and role.

`Renderer.schedule` is asked to coalesce: a hundred writes in a loop should
cost one sync, not a hundred. The backend decides how, on its own loop.

## Two mount paths

`mount(root)` opens a window: it walks the tree, creates views, inserts each
into the nearest ancestor view, and marks every node attached. The root's own
view is inserted nowhere, because it has no host above it; the facade puts it
in the window.

`realise(n, host)` is the same walk without the window, for a subtree the
platform owns: a recycled list row is not in facet's tree at all, so it can
never be reached by a walk from a window root. `unrealise` is its teardown, and
`sync_from` applies dirty bits over such a subtree.

## Handlers

Every handler is `fn(*u8, *u8)`: sender first, ctx last. The ctx is bound where
the tree is built.

Handlers are read from the props AT FIRE TIME rather than captured, so an
application that swaps a handler after mount is answered by the new one with
nothing re-installed.

## Data — resources

A screen's data lives in a resource: an app struct over any backing
(sqlite, files, a network API) whose only doors are REST verbs —
`get` / `post` / `put` / `delete` from `facet/resource`. Each verb runs the
backing work off the main thread and installs the result back on it, then
broadcasts one typed `Change {verb, id}` to every watcher. Nobody calls the
backing directly, so nothing can block the UI; nobody hand-wires "I changed
this, tell that screen", because the write itself is the notification.

The component discipline, whole:

- `on_attach`: `watch(...)` the resources you show, then `get(...)` them.
- `build`: render immediately from whatever the store holds (usually empty).
- Mutating handlers: fill the draft, call `post`/`put`/`delete` — no UI
  code at the call site.
- ALL screen updating lives in the watch handler: reconcile the one change,
  by key, reading the store's synchronous accessors.
- Subscriptions are owning handles; dropping the component cancels them.

A move is a `put` with a new owner — there is no "moved" verb. The watch
handler diffs the store against what it has mounted (`mount::find` scoped
`within:` a lane answers "where is this card showing now"). A query
(`get(r, q: ...)`) is caller-scoped: it lands in the resource's hits slot
and only its `then` hears about it, because a `?q=` response is not the
collection.

The one-shot cousin: a `services::Job` (`run`/`apply` + `run_job`) is still
the right shape for a load that is not a store — a file-tree walk, a
scaffold step. Resources ride the same flight and the same teardown
guarantees (`jobs_settled`).

## Errors

facet does not panic. A mutator answers `Status`, a read answers `Option`, and
a value with a reason answers `Result`. A verb applied to a node that cannot
carry it does nothing and says so through its return, not by trapping.

## What a backend may not do

Implement a verb facet did not declare. The contract is the whole surface, and
a backend that adds vocabulary makes an application non-portable without
telling it.

Refuse silently. Where a platform cannot answer a verb, the backend records it
in its own manifest. `facet_appkit`'s ledger is machine-checked.

Grow a verb table in facet. Per-kind knowledge belongs in the backend.

## Reading the contract

`contract.md` is generated. It lists every verb, its type, and the ledger row it
came from, in one table per control plus the shared band.

It is the answer to "does facet have X". A verb absent there does not exist,
and calling it is a compile error rather than a silent no-op.
