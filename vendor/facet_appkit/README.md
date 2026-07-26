# facet_appkit

facet's **AppKit** backend — two modules:

| Module | Import | What it is |
|---|---|---|
| `ui` | `import "facet_appkit/ui" as ui;` | The `@ui` contextual-builder layer: widgets + layout containers over flex_layout and the typed appkit binding. Usable standalone (no facet). |
| `facet_appkit` | `import "facet_appkit/facet_appkit" as fak;` | Full per-kind `Renderer` ops, `set_identity`, the keyed-direct/lifecycle/async hook impls, and the window host (`open_window` + `run_loop`, composed by `run_window`). |

## Architecture (post multi-backend plan)

1. **Portable description** lives in `vendor/facet`: pure-data `Node` + closed kind set + flat `Renderer` vtable + `mount`.
2. **This package** supplies one op per kind (thin adapters over `ui::*`) and `set_identity`.
3. **`mount` calls `set_identity` after every widget-producing op** — pins `accessibilityIdentifier` + packed `(role, drive)` affordance metadata. Agents read the declaration; they do not re-guess from `isKindOfClass:`.
4. **Layout is write-once** in facet + flex_layout. Only leaf/widget ops are per-backend.

### Renderer kinds

The full portable kind set is implemented — leaves, value controls, pickers,
containers, wrappers, `list`, `native` (catalog: `vendor/facet/docs/widgets.md`)
— plus `set_identity` after every widget op.

`tree` / `list` stay AppKit-only `ui::` escape hatches (structured payload, not in `Node`).

### Handler conventions

| Field | Sentinel | Why |
|---|---|---|
| `Node.click` (buttons) | `noop` (never null) | Always safe to call |
| Optional handlers (composer, gestures, text_area `on_change`) | `0 as fn(*u8,*u8)` | Backend wires only when non-null |

### Agent tagging paths

| Path | Id | Affordance |
|---|---|---|
| `@facet` + mount | `set_identity` → accessibilityIdentifier | packed associated object |
| `@ui` with `agent_id:` | `tag_agent` → accessibilityIdentifier | packed associated object (same key) |
| Hand-built | `agent::set_agent_id` and/or accessibilityIdentifier | `agent::set_affordance` if needed |

## The `@ui` layer (platform-native)

```
import "flex_layout/flex_layout" as flex;
import "facet_appkit/ui" as ui;

var tree: flex::Node = @ui {
    screen {
        label("Iris", size: 28.0f64, bold: true)
        hstack {
            button("New Project", on: cb, primary: true)
            button("Open Project", on: cb)
        }
    }
};
tree.calculate_layout(800.0f64, flex::undefined(), flex::Direction::LTR);
ui::apply(#addr_of(tree), content.raw(), false);
```

Widgets own their NSView in the flex payload; measurement is per-kind inside the op. Escape hatches: `view` / `wrap_view` / `fixed_view`.

`@ui` components: a handler wired as `ui::button(on: component.method)` re-renders that component through `component_after` — an `@ui`-layer mechanism only. The `@facet` layer never re-renders; it updates by key.

## The `@facet` layer (portable description)

Apps write against `vendor/facet` (component struct + `build`, keyed-direct
updates, runtime-fired lifecycle, services, `spawn_ui`) and run through
`facet/runtime`, which selects this backend. See `vendor/facet/docs/` — this
package is the implementation side:

- **Keyed-direct verbs** (`find`, leaf mutators, structural verbs) resolve
  against the retained mounted trees; a miss no-ops.
- **Lifecycle**: the container→detach registry `present` writes is fired and
  cleared by the structural verbs before they remove content; `unmount` /
  `unmount_all` drain it while views are alive.
- **Window host**: `open_window` (creates the shell, mounts, returns) +
  `run_loop` (blocks); `run_window` is the composition. The seam is what lets
  `runtime::run_component` fire `on_attach` / `on_detach` from typed code.
- **Screen windows**: `present_screen_window` (retained slot keyed by the
  screen instance, per-window delegate records, `on_closed` fires on any
  close path; not counted against the shell-window total, so closing one
  never stops the loop) + `close_window(handle)` — the host pair under
  `nav::push` / `nav::pop`. `Application.stop` is nudged with a no-op
  app-defined event so a close from a callout (a `run_on_main` step, an
  agent's click) stops the loop immediately.
- **Async**: `run_on_main` dispatches onto the main queue; a dispatch READ
  source on the stdlib reactor's kqueue pumps `spawn_ui` tasks — awaits
  resume on the main thread.

There is no re-render loop: state changes are pushed to keyed elements in
place (`vendor/facet/docs/updates.md`).

**Wrapper + named params:** use fluent modifiers after the block (`.on_click`, `.agent_id`). Primary click for `clickable` is `Node.click`.

**Multi-child wrappers:** `bordered` / `clickable` with 2+ children mount them as a portable column (nothing is dropped).

## Tests

`cd vendor/facet_appkit && cpc test` — `@ui` suite, facet pipeline, full-kind mount, agent e2e (button / clickable / composer / text_area), re-render, components.

## Porting another backend

Clone this shape: per-kind ops (~15–40 LOC each) + `set_identity` + a small window host (mount + a component-invalidate primitive; no global runtime). Layout and mount stay in `vendor/facet` + `flex_layout`. See `plans/facet-multibackend-proposal.md`. A GTK backend is not built yet — implement `facet_gtk` on a Linux host using this package as the reference.
