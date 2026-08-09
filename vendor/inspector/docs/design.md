# Design

Why this package is shaped the way it is. The research that led here is
[docs/design/runtime-iteration-spike.md](../../../docs/design/runtime-iteration-spike.md);
this records the decisions that survived contact with the code.

## The vtable is the foundation, transport is an adapter

`agent_core::backend::Backend` is a `Copy` struct of fn-pointers over a
type-erased receiver, and both `agent_mcp` (JSON-RPC over a socket) and
`agent_inapp` (transport-free, `Channel::InApp`) are consumers of it. The MCP
package names no backend.

The inspector copies that layering exactly. `inspector::Backend` is the
contract; `tree::local_backend()` is the in-process implementation. Whether the
panel runs embedded or in a separate tool process is therefore a **binding at
the call site**, not an architecture. `widget` is written against the vtable,
so the same file serves both.

This is why the first version needs no marshaling. `core::touch` asserts it is
on the UI thread, and an embedded consumer is already there. A socket adapter is
the thing that will have to marshal, and that cost belongs there rather than on
the local case that does not need it.

## Walking facet, not the platform

`agent_appkit` exists because the agent surface walks the `NSView` hierarchy.
The inspector walks facet's own mounted tree, which is already
platform-neutral. The consequence is not just fidelity, it is cost:

- the agent needed a backend per platform;
- the inspector needs one walker plus a small platform shim.

`tree.cplus` is portable. `appkit.cplus` answers only the two questions facet
genuinely cannot: where the pointer is, and how to draw over a window.

Walking facet is also the only way to see pure-layout nodes (no view at all),
spans and menu items (runs and rows inside someone else's view), the difference
between what was declared and what layout computed, and the dirty word — which
is the whole of facet's update model.

## The three tiers

The scope estimate changed once the code was read.

`props::CommonProps` is inline on **every** node, not per control. Opacity,
background, corners, shadow, visibility, transform, tooltip — each with its own
dirty bit. That is nearly the entire useful property list, uniform across all 38
control kinds, with no kind dispatch.

Flex style is the same story: facet's tree *is* flex's tree, so width, padding,
margin, gap, grow and shrink are style writes plus `C_LAYOUT`.

Only `text`, `title` and `on` need to know the control, and those live behind
the generated typed handles, which resolve by key. So `gen_contract.py` work
moved out of v1 entirely — and tier 3's key requirement became a reported
limit rather than a hidden one.

## Handles are pointer plus generation

facet's own typed handles (`label::Label`, `button::Button`) are a node pointer
plus flex's global removal counter. A child lives in its own heap slot, so its
address is stable from insertion until it is itself removed; appends, inserts,
sibling removals and reorders never move it.

`insp::Handle` is the same shape, for the same reasons, and gets the same
staleness check for one comparison. It is conservative — any removal anywhere
ends every outstanding generation — but that is what facet already does, and the
alternative is a second identity system to keep in step.

A remote client cannot hold one: a pointer does not survive a socket. That is
why `describe` answers a **flat, parent-indexed** vector rather than a nested
structure — the index is the wire address, and an adapter resolves it to a
`Handle` on the app's side.

## The tree pane is `facet/tree`

This was first built as a fixed pool of 160 rows with a hand-rolled collapse
set, a visible-row list, and disclosure triangles made of text buttons. All of
it already existed. `TreeNode` carries a stable identity and an arbitrary
payload, the backend round-trips expansion in both directions, the outline view
indents and recycles, and `restore(expanded, selected)` exists for exactly the
rebuild-then-put-the-state-back case.

The pool also had a bound it could not exceed, and had to *report* the overflow
to stay honest. An outline view has no such bound.

One property of the replacement is worth stating because it is not obvious:

**Identity is the node's address.** Unique, stable while the node lives, and
derived from the item rather than from where it sits. `TreeNode`'s own
documentation is emphatic about this, and it is right: a positional id renames
every row after an insert, so expansion and selection follow the slot instead
of the node.

The model carries the facet node pointer in `TreeNode.data`, so selection needs
no lookup by id — the callback already has the node.

The row builder, the bind and the row height are all supplied rather than
defaulted. The bind is the one that is not optional: cell reuse is gated on it,
so a tree without one rebuilds every row.

Building this surfaced a real backend bug, since fixed in `facet_appkit`: both
places that sized a tree row laid it out BEFORE realising it, and a label's
intrinsic height arrives with the measure callback the backend installs when it
creates the native view. Every row came out one point tall — present,
selectable, invisible.

## The overlay is a layer

A highlight must not participate in layout, must not intercept a hit test, and
must not appear in the tree it is drawn over. A `CALayer` satisfies all three by
construction — layers are not views, do not lay out, are not hit-tested, and
nothing that walks `subviews` or facet's tree will ever see one. A transparent
overlay *view* would have needed excluding from three separate walks and would
still have been in the responder chain.

It attaches to the window's **content** view, not the inspected node's view, so
the inspected view is never modified — not its layer-backing, not its sublayers,
not its rendering path. The rect is converted into the content view's space
instead.

A viewless node still gets a highlight: its flex frame is relative to its
parent, so the offsets accumulate up the chain until a backed ancestor is
reached. Whether that ancestor's view is flipped is asked at runtime rather than
assumed — facet's hosts are flipped, but a host that was not would put every
highlight on the wrong side of the window and look like a layout bug.

## Separate from the agent surface

The agent surface hides unexposed nodes, limits actions to declared affordances,
and prohibits point-addressed actions. A developer inspector needs the opposite
of all three. Growing `agent_core::Backend` a debug mode would have weakened a
user-facing permission model to serve a development tool.

So: separate vtable, separate package, nothing reachable from the agent surface.

## Point picking was built and removed

Clicking an element in the running app to select it worked in outline and was
wrong in practice, so it is gone rather than left half-right.

It needs a global event monitor that swallows mouse-down. That makes the app
feel broken in every case where the pick does not resolve — and it resolves
against the NSView under the pointer, which for a facet tree is often a
control's internal part that no facet node owns. A monitor that eats clicks
and sometimes does nothing is worse than no picker.

The performance shape was also against it: resolving a hit means walking up
from the hit view looking for a node facet owns, and each step needs the tree
snapshot. Doing that on mouse-moved, continuously, is a lot of work per frame
for a hover highlight.

The tree pane is the way in. It is complete — it shows unkeyed and viewless
nodes a click could never reach — and selecting a row highlights the node in
the window, which was most of what the picker was for.

If it comes back it should be a mouse-moved-only hover that never swallows a
click, with the commit coming from a keystroke rather than from a click.

## Decisions worth not relitigating

**Refusals are typed and distinct.** `UnknownProperty` and `Unsupported` are
different answers — "you misspelled it" versus "the name was right and this
build cannot reach it here". `ReadOnly` and `UnknownProperty` likewise: "layout
decided that" is not "no such thing".

**The reader is symmetric with the writer.** A tier-3 property the inspector
cannot aim (a duplicate key, where `find` resolves the first match) is one it
declines to *report*, rather than quietly showing another node's value as though
it belonged to this one.

**A theme token is a name, not a colour.** `vocabulary` reserves 255 for a
literal `rgba` and 254 for a light/dark pair; every other non-zero token is a
theme role whose channels are all zero. Treating "non-zero token" as "themed"
reports every literal as `token:255`; treating it as "has channels" reports
every themed colour as transparent black. Both were live bugs; there is a test.

**Unset lengths read as `Nothing`.** `auto` has no number an inspector could
hand back unchanged, and answering `0.0` would be a value the UI then writes
back as a real zero.

**A mixed padding reads as nothing.** Showing one edge in a single `padding`
box is how a developer sets all four to the value that happened to be on one of
them.

## What is deliberately not here

Structural editing (insert, delete, reparent) needs identity, focus, selection,
scroll and component-lifecycle policies that do not exist yet. Handler
replacement is code hot reload wearing a property editor's clothes. Expression
evaluation would mean embedding an interpreter.

Source navigation — "open the code that made this node" — is the one item that
needs a compiler change: facet's runtime `Data` carries no source origin. The
lexer already has file-aware spans, so a debug-only origin ID could be injected
during `@ui` lowering with a side table mapping it to file, line and column.
Until then the key is the handle, and an editor can search for it.
