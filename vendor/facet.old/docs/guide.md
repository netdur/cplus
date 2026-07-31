# Guide

How facet is meant to be used: build-once trees, keyed updates, lifecycle, and
backends. Fast start: [tutorial.md](tutorial.md). API tables: [ref.md](ref.md).

**Deep dives (kept as topical docs — do not skip them for real apps):**

| Topic | Doc |
|---|---|
| Components, state, composition | [component-model.md](component-model.md) |
| `find` / Handle / structural verbs | [updates.md](updates.md) |
| Lifecycle, `present`, router, parking | [lifecycle.md](lifecycle.md) |
| App, Screen, nav (the process tier) | [app-screens.md](app-screens.md) |
| Theme, color tiers, light/dark | [theme.md](theme.md) |
| Services, `load_service`, `run_on_main` | [services.md](services.md) |
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
| `facet/facet` | `Node`, DSL, widgets, `Component`, `find`/`Handle`, lifecycle, `Chrome`/`Screen`, Color/Style |
| `facet/runtime` | host: `run`, `run_component`, `run_screen`, `App`, `alert`, menus; selects backend |
| `facet/nav` | screen navigation verbs (`go`/`push`/`pop`/`quit`/`arg`) |
| `facet/agent` | opt-in MCP serving for `app.agent_mcp` |

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

Components implement `Lifecycle` (`on_attach` / `on_detach`); the hooks are
fired for them. `run_component` fires the root's around the run loop;
`present` shows a component in a keyed container and arranges the outgoing
one's `on_detach` before its tree is removed — by any verb, including a plain
`set_child` and teardown. Routers therefore hold state and project it;
they never call hooks. For view parking across navigation (scroll,
half-typed input) there is `stage` / `attach` / `detach`. See
[lifecycle.md](lifecycle.md).

## App and screens (the process tier)

A component that also implements `Screen` (one method, `chrome()`) can be run
as a window with `run_screen`, or registered under a route name on an `App`.
`App::run` shows one screen at a time as a blocking window; handlers move
between them with `nav::go` (replace), `nav::push`/`nav::pop` (overlay), and
`nav::quit`. The App also owns the app menu, launch/quit hooks, and the
agent-surface socket (`app.agent_mcp`). In-window navigation (`present` into
an outlet) is unchanged and needs no App. See
[app-screens.md](app-screens.md).

## Theme (color)

Two tiers of color names: the platform's semantic colors (pass-through,
"look native") and app-retintable theme roles (`primary`, `ink(a)`,
`surface`/`raised`/`sunken`, status). `facet::set_theme(Theme::new(...))`
once; unset roles fall back to platform colors. Light/dark and runtime
re-theming repaint the mounted chrome in place — no rebuild, no
`on_appearance_change` for chrome. See [theme.md](theme.md).

## Services (slow data)

A service owns data and sources it off the main thread: implement
`Service` (`produce` on a worker / `apply` on main) and call
`facet::load_service` — or the service's own `load_async` wrapper — from
`on_attach`. See [services.md](services.md).

## Backends

Description never imports AppKit/GTK. A backend implements mount + leaf ops;
`runtime` installs it for the host. Porting guide: [backends.md](backends.md).

## Agent surface

`.key(id)` and `.agent_id(id)` participate in the agent identity story. Prefer
stable, namespaced ids so external agents and `find` stay aligned
(`agent_core` / `agent_mcp`). The exposed describe is the agent's TOOL LIST:
a node's name auto-derives from its title/label, `.accessibility_label`
overrides it (essential for icon-only controls), and `.accessibility_hint`
supplies the intent description ("opens the New Project wizard") — the same
channel VoiceOver reads, one annotation serving both.

## Gotchas

- **Do not re-call `build` each frame** — that is the wrong model.
- **Do not assume diffing** — you already know the key.
- **Move semantics** in the DSL: fluent chains consume; use in-place `set_*`
  on held vars when needed (same as flex_layout).
- **`find` after teardown** no-ops; check `found()` if you must know.
- **Identical keys in two components** — pass `cp: #addr_of(this)` to scope, or
  namespace keys globally.
- **Async completions** — with namespaced keys a stale delivery just misses;
  guard with `found()` or `facet::attached(this)` when you must know.
- **Never `block_on` in a handler** — it blocks the main thread. Async work
  goes through `facet::spawn_ui`; CPU-blocking reads through a service
  ([services.md](services.md)).

## Status (high level)

Description model, AppKit backend, keyed updates, lifecycle, and the
App/Screen/nav tier are real and tested. GTK is a stub relative to AppKit. Widget coverage continues to grow
toward portable kit parity; platform-only escapes use `native(...)`.
