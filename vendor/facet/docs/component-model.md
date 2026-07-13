# The component model

A component is a plain struct. Its fields are the state, its handlers are
methods in the inherent `impl`, and `impl T: facet::Component` is the checked
conformance block holding `build`:

```cplus
struct Sidebar {
    collapsed: bool,
    width: f64,
}

impl Sidebar {
    // handlers + helpers live in the inherent impl
    fn new() -> Sidebar { return Sidebar { collapsed: false, width: 240.0f64 }; }
}

impl Sidebar: facet::Component {
    fn build(this) -> facet::Node { /* return a Node tree */ }
}
```

## `build` runs once

`build` returns the **portable description** — a `Node` tree of pure data, no
platform types. The backend calls `build` a single time, mounts the description
into native views, and **retains** the tree. After that the tree is live: you
address elements and edit them in place. `build` is never called again.

Because `build` is one-shot, it must return already-owned children. Do not mint
a fresh child inside `build` on the assumption it will be called each frame — it
will not be.

## Where state lives

State lives in the struct's fields. The component is retained for the life of
the tree, so its fields persist. A handler writes a field, then pushes that
value to the element that displays it (see [updates.md](updates.md)). Nothing is
recomputed and the tree is not rebuilt.

```cplus
fn toggle_collapse(sender: *u8, ctx: *u8) {
    SIDEBAR.collapsed = !SIDEBAR.collapsed;                 // 1. update state
    let _u: facet::Handle =
        facet::find(ctx, "panel").set_hidden(SIDEBAR.collapsed);  // 2. push to the view
    return;
}
```

## The conformance block

`interface Component { fn build(this) -> Node; }` is closed: the `impl T:
facet::Component` block is verified against this declaration both ways, so a
typo in the method name or signature is a compile error rather than a silently
unmounted component.

`build` takes `this` by read-only receiver — it reads state to produce the
description, it does not mutate. Mutation happens in the inherent handlers
(`ref this`, or via the component's address as a free function's `ctx`).

## Composition

A component composes by calling other components' `build` inside its own `@facet`
block, or by writing plain functions that return `Node`:

```cplus
fn chip(name: str, ok: bool) -> facet::Node {
    var b: facet::Builder = facet::Builder::new();
    if ok { b.add(facet::label("[ok]", secondary: true)); }
    else  { b.add(facet::label("[--]", secondary: true)); }
    b.add(facet::label(name));
    return facet::hstack(b);
}

impl Panel: facet::Component {
    fn build(this) -> facet::Node {
        var p: OtherComponent = OtherComponent::new();
        return @facet {
            vstack {
                chip("clang", true)          // a plain Node-returning function
                p.build()                    // another component
            }
        };
    }
}
```

Both forms are just `Node`s in the tree. There is no component instance kept by
facet beyond the retained description and the struct you own.

## Why not re-render

Re-calling `build` when state changes, holding the tree to "render it top-down
each frame," or diffing a fresh tree against the old one is the pattern facet
deliberately does not use. The app already holds the element's key, so the
change is applied directly to that element. See [updates.md](updates.md) for the
mechanics and [../README.md](../README.md) for the rationale.
