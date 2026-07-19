# Backends

> Entry path: [tutorial.md](tutorial.md) · [guide.md](guide.md) · [ref.md](ref.md)

`facet/facet` is platform-free: it produces a `Node` tree and knows nothing
about native views. A **backend** package turns that description into a native
UI. `facet_appkit` is the reference backend (macOS / AppKit); `facet_gtk` is a
stub.

An app never talks to a backend directly for the description — it imports
`facet/facet` to build the tree and `facet/runtime` to run it. `runtime.cplus`
selects the backend for the host (AppKit on macOS; `runtime_linux.cplus` shadows
it on Linux).

## Running a window

```cplus
import "facet/runtime" as facet;

fn main() -> i32 {
    var win = window::Window::new(content, width: 1000.0f64, height: 700.0f64);
    facet::run(win);          // installs the backend, mounts, enters the event loop
    return 0;
}
```

`facet/runtime` also provides:

- `present_window(root, title, w, h)` — open a secondary window from a `Node`.
- `alert(title, message, primary, secondary?)` — a modal alert; returns which
  button was pressed.
- `app_menu(title)` and the menu-action helpers.

## The `Renderer` vtable

`mount(np, renderer) -> flex::Node` walks the `Node` tree and dispatches each
kind to a flat per-kind vtable, the `Renderer`. Containers, spacing, and every
layout prop are applied here write-once; only leaf widgets and wrappers go
through the vtable. After every widget-producing op, `mount` calls
`set_identity` so the agent id + role are pinned once.

A backend fills the `Renderer`:

```cplus
struct Renderer {
    opaque ctx: *u8,
    // leaves: build the native widget, return a flex node whose payload owns it
    label:  fn(*u8, *Node) -> flex::Node,
    button: fn(*u8, *Node) -> flex::Node,
    // ... one per widget kind ...
    // wrappers: mount recurses the child first, then hands it over
    bordered: fn(*u8, *Node, take flex::Node) -> flex::Node,
    scroll:   fn(*u8, *Node, take flex::Node) -> flex::Node,
    split:    fn(*u8, *Node, take flex::Node, take flex::Node) -> flex::Node,
    // give a KEYED container a backing view at mount so find(key) can address it
    container: fn(*u8, *Node, *flex::Node),
    // called after every widget op: pin (id, role)
    set_identity: fn(*u8, *u8, str, u32),
}
```

## Installed hooks

The keyed-direct verbs, the lifecycle verbs, and `list`/`raise` are
backend-provided: the core declares a hook per verb, and the backend registers
its implementation once, at startup, in `install()`. Until a backend registers a
hook, that verb no-ops (the portable-by-default posture). `facet/runtime`'s
`run` calls `install()` before any render.

| setter | registers |
|---|---|
| `set_find_fn` | `find(cp, key) -> Handle` |
| `set_set_text_fn` / `set_set_value_fn` / `set_set_on_fn` / `set_set_hidden_fn` | the leaf mutators |
| `set_add_child_fn` / `set_insert_child_fn` / `set_replace_child_fn` / `set_remove_child_fn` / `set_set_child_fn` | the structural verbs |
| `set_lc_register_fn` | the container→detach registry `present` writes; the backend's structural verbs fire+clear entries before removing content |
| `set_is_attached_fn` | liveness (mounted = attached; staged answers by attach state) |
| `set_run_on_main_fn` | `run_on_main(work, ctx)` (main-thread dispatch; `load_service` relies on it) |
| `set_stage_fn` / `set_attach_fn` / `set_detach_fn` / `set_unstage_fn` | view parking (stage / attach / detach) |
| `set_list_builder` | the recycling `list` |
| `set_raise_fn` | `raise(sender, key)` (bring a keyed element to front) |

## Adding a backend

To port facet to a new toolkit, a package supplies:

1. The `Renderer` vtable — one op per widget kind, plus `apply_style`,
   `set_identity`, the wrapper ops, `container`, and the `native` hatch op.
2. The keyed-direct + lifecycle hook implementations, registered in `install()`.
3. A window / run host and a **relayout** primitive (used on window resize and
   after a keyed-direct edit to re-lay the retained tree — geometry only, no
   re-render). The host splits into `open_window` (create + mount, returns)
   and `run_loop` (blocks): the seam lets `run_component` fire `on_attach`
   after the mount and `on_detach` after the loop stops, from typed code.
4. A main-thread dispatch for `run_on_main` (AppKit: `dispatch_async_f` onto
   the main queue), and a reactor watch for `spawn_ui`: observe the stdlib
   reactor's kqueue fd from the UI loop and call `facet::pump_async` when it
   reads ready (AppKit: a dispatch READ source on the main queue).

The `native(...)` op is the universal fallback: anything a backend does not have
a portable op for, an app can drop in as an app-owned native view. Layout stays
the shared `flex_layout` engine on every backend, so only the leaf widgets and
the small window/relayout host are per-platform.

The reference implementation is `facet_appkit`. The portable conformance test is
a `fake_renderer` that fills the vtable with test doubles and asserts `mount`
walks and tags the tree correctly, independent of any real toolkit.
