# Guide

What facet is, the model it commits to, and what a backend has to fill. Fast
start: [tutorial.md](tutorial.md). API: [ref.md](ref.md). Every declared verb:
[contract.md](contract.md).

## What it is

facet is a portable description of a user interface. It owns the vocabulary
and the tree; it draws nothing. A backend reads the tree and produces native
views: `facet_appkit` on macOS.

The vocabulary is not invented. It is bootstrapped from MAUI's portable
surface, curated row by row, and generated. That is why `contract.md` names a
MAUI row for almost every verb, and why a verb that is not there is not a
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

Two bits are commands rather than state: `C_FOCUS` and `C_BLUR` are acted on
and cleared. `C_LAYOUT` means re-run the layout pass rather than re-read props.

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
| `AgentHooks` | serving and attaching the agent surface |

Plus two slots: `theme::set_is_dark_fn` and `theme::set_theme_changed_fn`.

A zero field keeps the portable no-op, so a partially implemented backend is
degraded rather than broken.

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

`contract.md` is generated. It lists every verb, its type, and the MAUI row it
came from, in one table per control plus the shared band.

It is the answer to "does facet have X". A verb absent there does not exist,
and calling it is a compile error rather than a silent no-op.
