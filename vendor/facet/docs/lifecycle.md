# Component lifecycle

> Entry path: [tutorial.md](tutorial.md) · [guide.md](guide.md) · [ref.md](ref.md)

A component that comes on and off screen implements `Lifecycle`:

```cplus
interface Lifecycle {
    fn on_attach(ref this);
    fn on_detach(ref this);
}
```

The hooks are notifications. They do nothing to your logic; what to do on
detach — cancel an in-flight request, let it run but drop the result, keep
streaming — is the app's policy, never facet's. The component itself never
calls a hook, on itself or on anything it hosts: the runtime and the
structural verbs fire them.

## Fired by the runtime

`run_component` requires `Component + Lifecycle` and owns both ends:

- `on_attach` fires after the mount, before the event loop. The tree is live
  and `find` resolves, so initial routing and subscriptions belong here, not
  in `main`.
- `on_detach` fires when the loop stops (the window is closing), before
  teardown — the component's views are still alive. Teardown then drains the
  detach of every component still presented inside the tree.

`run_screen` and screens shown by an `App` ride the same two seams; a screen
pushed with `nav::push` mirrors them around its own window's life. See
[app-screens.md](app-screens.md).

Delivery is best-effort by nature: an app terminated outright (⌘Q) never
returns from the loop, so nothing after it runs.

```cplus
impl ScreenY: facet::Lifecycle {
    fn on_attach(ref this) {
        this.display_current_screen();   // initial routing: the tree is live
        return;
    }
    fn on_detach(ref this) { return; }   // the shown child's detach is facet's job
}
```

The hooks are plain fns (an `async fn` does not conform to the interface).
For asynchronous initial work — a load, a poll loop — `on_attach` hands a
task off in one line: `facet::spawn_ui(this.refresh())`. See
[services.md](services.md).

## `present` — show a component, lifecycle included

On a keyed container `Handle`:

```cplus
fn present[C: Component + Lifecycle](this, ref c: C) -> Handle
```

`present(c)` is the component-aware `set_child`: it evicts whatever the
container currently shows, mounts `c.build()`, and fires `c.on_attach()` once
the tree is live. It also leaves behind an erased detach callback for `c`, so
that whichever verb later removes or replaces that content — another
`present`, a plain `set_child`, `remove_child`, `replace_child`, or teardown —
first notifies the outgoing component. The notification is fire-and-forget:
the verb tells the component it is about to be removed, does not wait on
anything, and proceeds.

The order on every navigation is therefore fixed, with no app code involved:

```
outgoing.on_detach()      (its views are still in the tree)
→ old subtree removed, new subtree mounted
→ incoming.on_attach()    (its views are live)
```

Nested outlets are covered: removing a subtree also notifies any component
presented inside it, before the views die.

Two rules make this sound:

- A presented component must **outlive its presentation** — facet holds its
  address, not a copy. Screens owned by a router that lives for the window
  satisfy this by construction.
- A presented container shows **one tree at a time** (the single-slot
  contract `set_child` already implies).

`present` on a missing handle does nothing and fires nothing.

`present` REBUILDS: `build()` runs on every show, so it is the verb for
showing something genuinely new, or a screen whose appearance derives
entirely from surviving data (fields, services) — there a rebuild is
lossless. For siblings the user switches back and forth between, use
`switch_to` (next section): it builds once and preserves the views.

## A router: `switch_to`

This is the IN-WINDOW pattern: one parent owns several long-lived child
components as fields and shows one at a time through a keyed outlet.
Navigation between top-level screens (whole windows) is the App tier's job —
[app-screens.md](app-screens.md).

For siblings the user RETURNS to — tabs, inspector panes, pagers — the outlet
verb is `switch_to`:

```cplus
fn switch_to[C: Component + Lifecycle](this, ref c: C) -> Handle
```

Each sibling is built ONCE, on its first visit. After that, switching parks
the outgoing sibling's view tree (kept, off-canvas) and re-attaches the
incoming one's — the views survive, so a sibling comes back exactly as it was
left: scroll position, half-typed input, selection. The child model stays
build-once-mutate-later across switches, the same as everywhere else in
facet.

The router owns its screens, `current` is its only state, and one projection
function maps state to the outlet:

```cplus
struct ScreenY {
    screens: vec::Vec[ScreenX],
    current: i64,
}

impl ScreenY {
    fn display_current_screen(ref this) {
        if this.current < 0 { return; }
        match this.get_screen(this.current) {
            option::Option[*ScreenX]::Some(sp) => {
                facet::find("screen_y:outlet").switch_to((*sp));
            }
            option::Option[*ScreenX]::None => { }
        };
        return;
    }

    fn go_next(ref this, sender: *u8) {
        if this.screens.count() < 2 { return; }
        this.current = this.current + 1;
        if this.current >= (this.screens.count() as i64) { this.current = 0; }
        this.display_current_screen();
        return;
    }
}
```

Lifecycle matches `present`: the incoming sibling's `on_attach` fires on
every switch-in (guarded loads and keyed-slot populates keep data fresh), the
outgoing sibling's `on_detach` fires as it parks. Switching to the
already-shown sibling is a no-op. Like `present`, a switched sibling must
outlive its outlet, and its address is its identity — components in a `Vec`
must all be in place before the first switch.

The outlet manages two things the app never has to:

- **Eviction.** When the outlet itself leaves the tree (a body swap, a window
  teardown), the attached sibling detaches (its `on_detach` fires) and every
  parked tree of that outlet is dropped; the next `switch_to` re-stages.
- **Theme changes.** Parked trees bake build-time colors, so `set_theme`
  drops them (what is on screen is the app's own theme path's concern); the
  next `switch_to` rebuilds against the new palette. A theme change loses
  parked view state; a switch never does.

One nuance for services: while a sibling is parked its views are NOT in the
live tree, so keyed writes (`find(key).set_text(...)`) miss — harmlessly, the
verbs no-op. Land the data in the service and repaint from `on_attach` on the
way back in, or guard with `is_attached`.

**The rule of thumb:** siblings the user switches back and forth between →
`switch_to`. Showing a genuinely new screen → `present`. Don't mix the two
verbs on one outlet.

## Liveness: `attached` and `is_attached`

```cplus
fn attached[C](ref c: C) -> bool     // self-liveness: facet::attached(this)
fn is_attached(cp: *u8) -> bool      // by component address
```

A mounted component reads attached; unmounted, unknown, or parked reads
false. The guard an async completion checks before touching UI — or sugar it
into a method:

```cplus
impl ScreenY {
    fn is_attached(ref this) -> bool { return facet::attached(this); }
}
```

For UI written through namespaced keys, a stale completion is already safe
without the guard: the keys of a torn-down screen miss, and every verb on a
missing handle no-ops. Use `found()` on the handle when you want to know.

## Parking: stage / attach / detach

The manual tier UNDER `switch_to` — the outlet verb stages, parks, and
re-attaches through exactly these calls, and most apps never use them
directly. Reach for them when you want explicit control over the parking
lifetime (a custom nav stack that pre-builds screens, an outlet with policy
`switch_to` doesn't have). To build a screen off-canvas once and move its
live views in and out yourself:

```cplus
fn stage(build: fn(*u8) -> Node,
         on_attach: fn(*u8) = lifecycle_noop,
         on_detach: fn(*u8) = lifecycle_noop) -> Staged
```

On `Staged`:

```cplus
fn cp(this) -> *u8               // the component's address (identity)
fn valid(this) -> bool           // was staging successful?
fn is_attached(this) -> bool
fn detach(this) -> bool          // unplug from the tree, KEEP it parked; fires on_detach
fn unstage(this) -> bool         // tear it down for good
```

On a keyed container `Handle`:

```cplus
fn attach(this, s: Staged) -> bool   // move a staged component in; fires on_attach
```

`stage`, `on_attach`, and `on_detach` are bound method references
(`Screen.build`, `Screen.on_attach`): each auto-fills its context slot with
the component's address.

`detach` parks the component — views and native state kept — so re-attaching
restores it exactly. This is the imperative, non-reactive equivalent of Web
Components' `connectedCallback` / `disconnectedCallback` / `isConnected`.

## Which verb, when

| you have | put it in with | remove with | on removal |
|---|---|---|---|
| siblings the user returns to (tabs, panes, pagers) | `switch_to` | the next `switch_to` | parked (views kept); dropped when the outlet dies or the theme changes |
| a new screen (normal case) | `present` | the next `present` / `set_child`, or teardown | destroyed; its `on_detach` fired first |
| a fresh `Node` description | `add_child` / `insert_child` / `set_child` | `remove_child` | destroyed |
| a staged component (manual view parking) | `attach` | `detach` | parked (kept), or `unstage` to destroy |
