# Reference

The API by module. Concepts: [guide.md](guide.md). Fast start:
[tutorial.md](tutorial.md).

Every per-control verb is generated and listed in [contract.md](contract.md)
with its type and provenance. This page covers what the generator does not:
the tree, the runtime, the seams, and how to reach a control once it is built.

## facet/facet — the tree

`Node` is `flex_layout`'s node. These are free functions over `*Node`.

| | |
|---|---|
| `column(b, key:)` `row(b, key:)` | containers |
| `vstack` `hstack` `zstack` `screen` `card` `spacer` | named containers |
| `find_in(n, key) -> Option[*Node]` | address a node within a subtree |
| `child_count_of(n)` `child_of(n, i)` | walk children |
| `kind_of(n)` `view_of(n)` `key()` | what a node is and what backs it |
| `is_attached(n)` `is_focused(n)` | state the platform wrote back |
| `touch(n, bits)` `clear_dirty(n)` `dirty_of(n)` | the dirty word |
| `relayout(n)` | force a re-measure and a layout pass |

### The shared band

Declared on every node. Full list with types in `contract.md`.

| | |
|---|---|
| `set_opacity` `set_background_color` `set_background` | paint |
| `set_background_image` `set_shadow` `set_clip` | paint |
| `set_enabled` `set_visible` `set_input_transparent` | state |
| `set_flow_direction` `set_safe_area` | direction and insets |
| `set_rotation` `set_scale` `set_translation` `set_anchor` | transform |
| `set_z_index` | paint order among siblings |
| `set_on_focus` `set_on_blur` `set_on_attach` `set_on_detach` | lifecycle |
| `focus(n)` `blur(n)` | commands |

Layout modifiers (`set_grow`, `set_width`, `set_padding`, `set_gap`,
`set_justify`, `set_align`, `set_wrap`, and the percent forms) forward to
flex. Anything flex supports and facet does not wrap is reached with `node()`.

## facet/elements — the `@ui` namespace

One module forwarding every element name, so a bare name inside an `@` block
resolves in one context:

```cplus
import "facet/elements" as ui;

var root: core::Node = @ui {
    vstack(key: "body") {
        label("hello", key: "h")
        button("go", key: "b")
    }
};
```

The block's value is a container holding the block's items. A block may hold
more than one, so it always holds them: to address the item, give it a key and
`find` it.

`if` and `for` compose inside a block.

## Cursors — reaching a built control

Each control module offers the same two entry points:

`from(n) -> Option[C]` wraps a node you already have.

`find(key, within: *Node = 0) -> Option[C]` searches by address. Bare `find`
searches every open window in DOM order, first match wins; pass `within:` to
scope a subtree or disambiguate.

A cursor's setters return the cursor, so writes chain. A cursor whose node is
gone answers as missing rather than trapping.

```cplus
match text_field::find("email") {
    option::Option[text_field::TextField]::Some(f) => {
        let _f: text_field::TextField = f.set_text("a@b.c").set_secure(false);
    }
    option::Option[text_field::TextField]::None => { }
}
```

## facet/component

```cplus
interface Component { fn build(ref this) -> Node; }
interface Lifecycle { fn on_attach(ref this); fn on_detach(ref this); }
```

`item_of(sender)` answers what a node stands for, when the application set one
with `set_item`. Borrowed: facet never frees it.

## facet/screen

`Chrome::new(...)` describes a window. Fields: `title`, `subtitle`, `width`,
`height`, `min_width`, `min_height`, `max_width`, `max_height`, `bar`,
`maximizable`, `minimizable`, `zoomable`, `min_zoom`, `max_zoom`.

Zero means unconstrained for the size fields, on each axis independently.

`Bar` is `Native`, `Blended`, `Hidden` or `Custom`. `Custom` hides the standard
buttons so `window_buttons()` can supply its own; pair it with `.window_drag()`
on the surface that should move the window.

`AppMenu` and `MenuItem` describe the menu bar. A `MenuItem` carries a title, a
key equivalent, and either an `on_click` or a named `action` from the
`menu_action_*` set. `is_enabled` and `title_of` let an item grey itself out or
rename itself when the menu opens.

```cplus
interface Screen {
    fn chrome(this) -> Chrome;
    fn menu_items(this) -> vec::Vec[MenuItem];
}
```

`screen_box(s)` type-erases a screen for the runtime.

## facet/runtime

`App::new(name)` then:

| | |
|---|---|
| `screen(name, factory)` | register a named route |
| `menu(build)` | the app menu |
| `on_launch(f, ctx:)` `on_quit(f)` | process hooks |
| `agent_mcp(path)` | serve the agent surface on a Unix socket |
| `run(initial, arg:) -> Status` | run the loop |

`run_component(c)` and `run_screen(s, menu:)` are smaller hosts for one
component or one screen.

Window and app readers: `display_density`, `observe_display_density`,
`is_window_active`, `observe_window_size`, `observe_backgrounding`,
`observe_resumed`, `observe_stopped`, `set_app_appearance`, `app_appearance`,
`window_frame`, `set_window_frame`.

Dialogs: `alert(...)` is non-blocking and reports through `on_answer`;
`alert_blocking` returns the chosen index. `choose_directory` and
`choose_file` are the file pickers.

## facet/nav

`go(route, arg:)` replaces the current screen. `push(route, arg:)` opens one
alongside. `pop()` closes the newest. `quit()` ends the loop. `arg()` reads
what the caller passed.

Outside a running `App` these answer false or no-op: that is the portable
posture, not a failure.

## facet/services

| | |
|---|---|
| `after(seconds, cb, ctx:) -> Cancellable` | one-shot timer |
| `observe_size(n, cb, ctx:) -> Cancellable` | fire after every resize |
| `run_on_main(work, ctx)` | hop to the UI thread |
| `spawn_ui(future)` | a task that resumes on the UI thread |
| `worker_start(input, f)` | work off the UI thread |
| `run_job(job, then:, ctx:)` | a cancellable unit with a completion |

A `Cancellable` cancels on drop, so holding it is how you keep an observer
alive.

## facet/theme

Two tiers. Tier 1 is a platform token (`Color::text()`, `Color::accent()`,
`Color::window_background()`). Tier 2 is a ROLE (`Color::primary()`,
`Color::surface()`) that an application fills with `set_theme`; an unfilled
role falls through to a platform token.

`Color::adaptive(light:, dark:)` carries both sides; the backend picks by the
current appearance.

`set_theme(t)` repaints live. `is_dark()` reads the current appearance, and
`on_appearance_change(cb, ctx:)` observes it.

## facet/mount

The backend seam.

`install(renderer)` arms the whole pipeline. `mount(root)` opens a window;
`unmount(root)` takes it down.

`realise(n, host:)` mounts a subtree the platform owns (a recycled row);
`unrealise(n)` is its teardown; `sync_from(n)` applies dirty bits over it.

`sync()` applies every dirty node in every open window. `request_sync()` asks
the backend to schedule one.

`find(key, within:)` resolves a key to the node itself, for the places with no
control to narrow to — an outlet is a keyed column, and `set_content` wants the
node, not a cursor. Same rule as a typed `find`: the running app's windows in
open order, first match, `within:` to scope a subtree. `find_node(key)` is the
resolver underneath, the one the 42 typed finds share.

`mounted_root()`, `mounted_count()`.

## facet/gestures

`.gesture(...)` attaches a set to a node: `on_click`, `on_double_click`,
`on_right_click`, `on_long_press`, `on_press`, `on_release`, `on_hover`,
`on_unhover`, `on_pointer_move`, `on_pan`, `on_pinch`, `on_swipe`, `on_key`,
plus `can_drag`, `allow_drop` and the drag family (`on_drag_start`,
`on_drag_over`, `on_drag_leave`, `on_drop`, `on_drop_completed`).

A handler answers whether it TOOK the event. Declining lets the platform have
it, which is how a control keeps its own behaviour under a gesture.

A handler may be a component's own method — `.gesture(on_click: this.on_open)`
— because each one carries its own context slot, the same shape a generated
control's `on_clicked:` has. `ctx:` is the set-wide context, used by every
handler that did not bring one; that is what a free function wants.

Standing rule: a gesture-only affordance must also have a click path. An agent
has no hands, and a feature it cannot reach is a feature half the users of this
framework cannot reach.

## facet/agent

`enable()` registers the serving hooks; `disable()` returns to the no-op. An
application also declares `agent_core`, `agent_mcp` and the platform's agent
package in its `Cplus.toml`, then names a socket with `App::agent_mcp`.

The surface answers `describe_ui` (the live tree, addressed by key) and drives
controls through the same path a mouse takes.
