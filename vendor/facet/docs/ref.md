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

Two rules the shape does not show, both in `guide.md`: the transform animates as
a UNIT (one matrix, so a `set_*` on any of its numbers cancels a pending
transform animation), and the start value must reach the view in an EARLIER
apply — `set_opacity(0)` and `animate_opacity(1)` in one tick are not a fade-in.

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
| `animate_opacity` `animate_scale` `animate_scale_x` `animate_scale_y` | animate to end state |
| `animate_rotation` `animate_rotation_x` `animate_rotation_y` | animate to end state |
| `animate_translation(x:, y:)` `cancel_animations` | animate / abort |

`set_*` on opacity and the transform band **snaps**. `animate_*` writes the
same end values and asks the backend to interpolate over `duration` (default
`Duration::animation()` = 250 ms) with `easing` (default `SinInOut`). Full list
in `contract.md`.

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

### The three buttons

`button(title:)` draws the platform's own bezel. `icon_button(icon:)` is an
image and no bezel. `text_button(title:)` is text and no bezel — the flat
posture, for a Cancel beside a Save or a link-shaped action.

```cplus
text_button("Cancel", key: "cancel", on_click: this.on_cancel,
            text_color: theme::ink(0.6f64))

// the link posture
text_button("Learn more", key: "more", on_click: this.on_more,
            text_color: theme::accent(),
            text_decoration: vocab::TextDecoration::Underline)

// a chip: toggles on click, and shows its border when selected
text_button("Filter", key: "filter", on_click: this.on_filter,
            toggles: true,
            border_color: theme::accent(), border_width: 1.5f64,
            corner_radius: vocab::Corners::all(6.0f64))
```

`toggles: true` makes a click FLIP rather than fire-and-forget — AppKit's own
PushOnPushOff holds the state, so `is_on()` cannot drift from what the user
sees, and the handler still runs and reads where it landed:

```cplus
fn on_filter(ref this, sender: *u8) {
    match text_button::find("filter") {
        option::Option[text_button::TextButton]::Some(c) => {
            let _b: text_button::TextButton = c.set_bordered(c.is_on());
        }
        _ => { }
    }
    return;
}
```

The border is DESCRIBED and SWITCHED separately. `border_color`, `border_width`
and `corner_radius` say what it looks like; `set_bordered(bool)` says whether it
is drawn, and switching it off does not forget the description. A width of 0
would have meant restating the border every time it came back.

Everything in the shared band applies too — `set_background_color`,
`set_background(Brush)` for a gradient, `set_clip`, `set_shadow` — so a filled
or shadowed variant needs no new control.

Separate controls rather than one with a style flag, for the reason Flutter
separates TextButton from ElevatedButton: `border_color`, `border_width` and
`corner_radius` mean nothing on a control whose point is having none of them,
and a flag would leave them present, settable and silently ignored.

A `label(...).gesture(on_click:)` renders the same pixels and is not the same
thing: a text_button is a real button, so it activates from the keyboard when
focused and from VoiceOver, and it reports `role=button` to an agent. The label
is reachable by a pointer and nothing else.

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

## facet_runtime/runtime

(Moved out of facet, 2026-08-17: the boot facade is its own package —
see `vendor/facet_runtime/README.md`. The surface below is unchanged;
only the import path moved.)

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

## facet/resource

REST verbs over a shared store, with a change channel. A resource is an app
struct over any backing (sqlite, files, an API): implement `state` / `run` /
`apply`, keep the instance as a module static, and the verbs are the only
doors to the backing — there is no synchronous one.

| | |
|---|---|
| `get(r, then:)` | refresh the collection; broadcasts `Loaded` |
| `get(r, id:, then:)` | refresh one row; broadcasts `Loaded` with the id |
| `get(r, q:, then:)` | query — result in the resource's own hits slot, `then` only, **no broadcast** |
| `post(r, then:)` | create from the resource's draft; broadcasts `Created(new_id)` |
| `put(r, id:, then:)` | update from the draft; broadcasts `Updated(id)` |
| `delete(r, id:, then:)` | broadcasts `Deleted(id)` |
| `watch(r, f, ctx:) -> SignalSubscription[Change]` | the channel; handler `fn(Change, *u8)`, bound methods bind |

`run` executes OFF the main thread — backing calls and staging fields only.
`apply` executes ON it — install staged into live. `prepare` (optional; the
default does nothing) runs on the main thread right before each flight:
snapshot there anything `run` must not read from a worker, such as a path
held by main-only statics. One flight per resource;
verbs called while one is up queue in call order, so a later query never
overtakes an earlier write. A write whose `run` left `ok` false broadcasts
nothing — failure is the caller's, through `then`. Reads of the installed
rows are plain synchronous methods on the resource module. With no backend a
verb is the portable no-op.

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

## facet_agent/agent

(Moved out of facet, 2026-08-17: the agent surface is its own package —
see `vendor/facet_agent/README.md`. The surface below is unchanged; only
the import path moved.)

`enable()` registers the serving hooks; `disable()` returns to the no-op. An
application also declares `agent_core`, `agent_inapp`, `agent_mcp` and the
platform's agent package in its `Cplus.toml`, then names a socket with
`App::agent_mcp`.

The surface answers `describe_ui` (the live tree, addressed by key) and drives
controls through the same path a mouse takes.

`in_app(policy)` opens the attached surface directly for an embedded assistant.
It returns an `agent_inapp::Session`; no Unix socket or MCP transport is used.
Each `describe_ui`, `click`, `set_text`, `scroll_to`, and `hit_test` call checks
the policy with `auth::Channel::InApp` before touching the backend.
