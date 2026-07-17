# Guide

How facet is meant to be used: build-once trees, keyed updates, lifecycle, and
backends. Fast start: [tutorial.md](tutorial.md). API tables: [ref.md](ref.md).

**Deep dives (kept as topical docs — do not skip them for real apps):**

| Topic | Doc |
|---|---|
| Components, state, composition | [component-model.md](component-model.md) |
| `find` / Handle / structural verbs | [updates.md](updates.md) |
| stage / attach / detach (router) | [lifecycle.md](lifecycle.md) |
| `@facet` DSL, leaves, containers | [widgets.md](widgets.md) |
| Backend vtable, runtime host | [backends.md](backends.md) |

## What facet is

A **platform-free UI description**: a `Node` tree (pure data) plus a small
per-platform backend that mounts it into native views and lays it out with
`flex_layout`. Not HTML, not a virtual DOM, not reactive.

```
app code  →  @facet / Node tree  →  backend mount  →  native views
                ↑ find(key).set_*   (in-place, live tree)
```

## Two modules

| import | role |
|---|---|
| `facet/facet` | `Node`, DSL, widgets, `Component`, `find`/`Handle`, lifecycle, Color/Style |
| `facet/runtime` | host: `run`, `run_component`, `alert`, menus, `Window`; selects backend |

Apps describe with `facet`; they run with `runtime`. Backend packages
(`facet_appkit`, …) are pulled in by runtime, not by the description.

## The model (non-reactive)

1. Implement `Component`: `fn build(ref this) -> Node`.
2. Backend calls `build` **once**, mounts natives, **retains** the tree.
3. On events, handlers mutate fields, then **address one element by key** and
   set properties — no rebuild.

Two update paths, never a third:

1. **by key** — `facet::find(key)` (optionally scoped with `cp`)
2. **by method** — component method that uses `find` internally

Keys are also **agent / accessibility ids** — in-app code and MCP agents share
the same address space (ACI). See [updates.md](updates.md).

## Components

- Plain struct = state.
- Inherent `impl` = handlers.
- `impl T: facet::Component` = checked `build`.
- Instance ownership: `run_component` holds it for the window life — no module
  static required.

Composition: call other `build()`s or Node-returning functions inside `@facet`.
Details: [component-model.md](component-model.md).

## Layout and widgets

Containers (`vstack`, `hstack`, `scroll`, `grid`, …) and modifiers map to
`flex_layout`. Leaves (`label`, `button`, `text_field`, …) go through the
backend vtable. Full catalog: [widgets.md](widgets.md).

## Lifecycle (navigation)

`stage` / `attach` / `detach` park a fully built component off-canvas without
destroying native state (scroll, selection, half-typed input). Router pattern:
one nav owns screens, attaches one at a time. See [lifecycle.md](lifecycle.md).

## Backends

Description never imports AppKit/GTK. A backend implements mount + leaf ops;
`runtime` installs it for the host. Porting guide: [backends.md](backends.md).

## Agent surface

`.key(id)` and `.agent_id(id)` participate in the agent identity story. Prefer
stable, namespaced ids so external agents and `find` stay aligned
(`agent_core` / `agent_mcp`).

## Gotchas

- **Do not re-call `build` each frame** — that is the wrong model.
- **Do not assume diffing** — you already know the key.
- **Move semantics** in the DSL: fluent chains consume; use in-place `set_*`
  on held vars when needed (same as flex_layout).
- **`find` after teardown** no-ops; check `found()` if you must know.
- **Identical keys in two components** — pass `cp: #addr_of(this)` to scope, or
  namespace keys globally.
- **Async completions** — use `is_attached(cp)` before touching UI (lifecycle).

## Status (high level)

Description model, AppKit backend, keyed updates, and lifecycle are real and
tested. GTK is a stub relative to AppKit. Widget coverage continues to grow
toward portable kit parity; platform-only escapes use `native(...)`.
