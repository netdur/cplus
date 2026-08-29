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
| `rect_for_offset(n, i)` `has_caret_probe(n)` | where a character is, asked of the text system |
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

### Where a character is

`text_area`, `text_field` and `search_field` carry two reads the generator does
not describe, because they are not props — nothing is stored and nothing is
pushed. They ASK the platform's text system at the moment you call them.

| | |
|---|---|
| `rect_for_offset(i) -> Option[Rect]` | the box of the character at offset `i` |
| `caret_rect() -> Option[Rect]` | the same at `cursor_position()`, zero width |

The rect is in the control's OWN coordinate space, top-left origin — the space
`frame()` reports — so a popup goes beside it with `set_absolute` plus
`set_left`/`set_top` and nothing further to convert.

Asked rather than stored, because the answer moves on every edit, scroll,
resize, font change and re-wrap: a pushed value that missed one of those would
be a few points off with nothing to show why.

`None` means the platform cannot answer — the control is not mounted, or its
backend has no text layout right now. It is never a guess: an AppKit
`text_field` borrows the window's field editor and only has one while focused,
so an unfocused field answers `None` and a `text_area` always answers.
Application-side arithmetic over a font's advance is not a substitute — it is
wrong for the first proportional face, the first ligature and the first astral
character, and wrong silently.

```cplus
match text_area::find("editor") {
    option::Option[text_area::TextArea]::Some(ed) => {
        match ed.caret_rect() {
            option::Option[vocab::Rect]::Some(r) => {
                facet::set_left(popup, { r.x });
                facet::set_top(popup, { r.y } + { r.height });   // just below the caret
            }
            option::Option[vocab::Rect]::None => { }
        }
    }
    option::Option[text_area::TextArea]::None => { }
}
```

Backends: AppKit and UIKit answer. GTK has no probe installed and answers
`None` everywhere rather than reporting a number nobody measured.

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

Dialogs. `alert(title, message, primary, secondary:, on_answer:)` is
non-blocking and reports the chosen index through `on_answer` — 0 for the
primary, 1 for the secondary. `prompt(title, message, placeholder:, primary:,
secondary:, initial:, on_typed:, on_answer:)` adds a text field and reports
what was typed through `on_typed`. `choose(title, message, options,
on_answer:)` reports which option by index.

The callbacks are function POINTERS with an explicit context argument — C+ has
no closures, so anything the callback needs is passed alongside it rather than
captured:

```cplus
on_answer: fn(i32, *u8)     paired with  on_answer_ctx: *u8   // all three dialogs
on_typed:  fn(str, *u8)     paired with  on_typed_ctx:  *u8   // prompt only
```

Both context parameters default to `0 as *u8`, so a callback that reads only
statics can ignore them.

`initial` is what the field STARTS with and `placeholder` is the hint shown
while it is empty — a rename passes the old name as `initial`, and the caret
lands in the field with it selected, so the first keystroke replaces it.

**The keyboard answers them.** Return fires the primary, Escape the secondary,
in both `alert` and `prompt`. A `choose` binds neither: its buttons are N
options and none is the obvious one, and it carries no cancel. A one-button
alert ignores Escape, because a single button is an acknowledgement.

In a `prompt` that runs through the FIELD rather than through a default button:
a platform does not offer a default button the key while a field is being
edited, and in a prompt the caret is always in the field. So the same thing is
true of a text field YOU build — a Return in it reaches nothing unless you wire
`on_submit`. Giving a form an OK button does not give it Enter.

`alert_blocking` returns the chosen index instead, for a decision that must be
made before the process can go on with no window to attach to. An agent cannot
reach it, which is why it is not the default.

`choose_directory` and `choose_file` are the file pickers. They are the
platform's own panel, which an agent cannot drive and which cannot be
reimplemented — the panel is the sandbox door. **They also BLOCK**, so an app
that must stay answerable while one is open should not use them; an application
that needs an agent to choose a file has to offer a path some other way.

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

## facet/bands

The app's size-band vocabulary. A **band** is a named box constraint; a node
carrying a rule is hidden or shown while the box it sits in matches, decided
by the layout pass itself.

```cplus
sidebar.hide_in("compact");                         // on a cursor, after build
cards.add(pane(...).hide("tiny").hide("compact"));  // fluent, at build time
```

Six bands ship pre-registered, so an app that configures nothing still has a
shared vocabulary: `tiny` (<300pt wide), `compact` (300–599), `medium`
(600–839), `expanded` (840–1199), `large` (1200–1599), `xlarge` (≥1600).

```cplus
fn configure(name: str, min_width: f64 = …, max_width: f64 = …,
                        min_height: f64 = …, max_height: f64 = …) -> Status
fn remove(name: str) -> bool
fn is_registered(name: str) -> bool
fn count() -> usize
fn matches(name: str, width: f64, height: f64) -> bool
fn bands() -> *BandSet          // what the backends pass to calculate_layout
```

Edges are independently optional and an omitted one is UNBOUNDED, not zero.
Re-using a name updates it, so a settings reload is idempotent.

**The band is measured against the node's nearest CONTAINED ancestor** — the
closest box up the tree whose size does not depend on its own contents —
never the window. An app in Split View or on half a foldable was handed a box,
and the screen's width answers a question nobody asked. A node never queries
itself: a sidebar pinned to 400pt is 400pt wide in every window, so that would
make the rule a constant.

Nothing re-runs a rule by hand. `relayout_root` passes the set on every pass,
so visibility tracks the box with no size observer and no callback to keep
alive. Band names are `str`, so a typo is not a compile error — it makes the
rule inert. `is_registered` is there for a startup check.

Node verbs: `hide_in(band)` / `show_in(band)` (mutating, raise the layout bit)
and `clear_rules()` / `rule_count()`. Generated onto every control cursor, so
they chain. **The last matching rule wins.** Depth: `flex_layout`'s
[guide](../../flex_layout/docs/guide.md), "Conditional visibility — bands".

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

### `on_key` on a text control

Keys go to the FIRST RESPONDER, and for a text control that is not the view the
rest of the band is armed on — on AppKit a `text_area`'s node is backed by the
enclosing NSScrollView, and a `text_field` borrows the window's shared field
editor. So the backend arms `keyDown:` on the view that actually receives it,
and only that selector: the text system's own selection, dragging and
insertion-point machinery is left alone.

Declining is the common case and it is unchanged — `false`, or no handler, and
the character is inserted exactly as before. Only `true` swallows. That is what
lets Down/Up/Return/Escape drive a completion list while every other key still
reaches the text, and `caret_rect()` is what says where to put the list.

Read the key with `gestures::key_named(event)` against `key_return()`,
`key_escape()`, `key_arrow_up()`, `key_arrow_down()` and the rest, so a handler
never touches a platform key code.

## facet_agent/agent

(Moved out of facet, 2026-08-17: the agent surface is its own package —
see `vendor/facet_agent/README.md`. The surface below is unchanged; only
the import path moved.)

`enable()` registers the serving hooks; `disable()` returns to the no-op. An
application also declares `agent_core`, `agent_inapp`, `agent_mcp` and the
platform's agent package in its `Cplus.toml`, then names ITSELF with
`runtime::agent_mcp(id)`.

`agent_mcp` takes an **id, not an address** — a name like `"myapp"`, from which
the platform derives where to listen: `/tmp/mcp-<id>-<pid>.socket` at mode 0600
plus `http://127.0.0.1:<9000+pid%1000>/` on a desktop, the HTTP port alone on a
phone. Both are keyed on this process's pid, so a launcher that spawned the app
can compute the address without being told; the app also reports it on stderr
and writes it to `/tmp/mcp-<id>-<pid>.json`. An id containing `/` is refused.

It is a free function on every host tier — `run`, `run_component`, `run_screen`
and `App::run` alike — and `App::agent_mcp(id)` forwards to it, defaulting to
the app's own name when called with no argument. There is ONE address per
process. **No `agent_mcp` call, no server**: nothing outside the program can
turn one on.

`facet_agent` is documented in its own `docs/` — tutorial, guide and reference.

The surface answers `describe_ui` (the live tree, addressed by key) and drives
controls through the same path a mouse takes.

`in_app()` opens the attached surface directly for an embedded assistant. It
takes nothing and returns an `agent_inapp::Session`; no Unix socket or MCP
transport is used. Each `describe_ui`, `click`, `set_text`, `scroll_to`, and
`hit_test` call checks the policy with `auth::Channel::InApp` before touching
the backend. `in_app_with_grant(grant)` is the same door opened with a wider
authority the user has already approved — a new session rather than a widened
one, so a permission granted for one task does not outlive it.

`set_policy(f)` installs the application's own authorization policy,
`fn(auth::Request) -> auth::Grant`. It is the ONLY way an app can be narrower
(or wider) than the default: without it every connection is served the same
built-in grant, which is `auth::operator()` — read the UI and drive it, nothing
from behind a tier. An app that wants to ask its user first, check a token, or
refuse outright puts that here.

**Call it before `enable()`.** The serve thread reads the policy once, when it
starts; a policy installed afterwards leaves a window in which the default
served, and nothing reports that it happened.
