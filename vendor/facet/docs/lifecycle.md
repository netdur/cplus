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

## A router

The router owns its screens, `current` is its only state, and one projection
function maps state to the outlet. Handlers mutate and re-project; every hook
firing is facet's:

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
                facet::find("screen_y:outlet").present((*sp));
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

Each `present` rebuilds the incoming screen's views from its fields. State
worth keeping across visits belongs in the component's fields; what does not
survive is pure view state (scroll position, an uncommitted selection). To
keep views alive across navigation instead, see parking below.

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

`present` rebuilds on every show. When a screen's **views** must survive
navigation — scroll position, half-typed input, selection — build it
off-canvas once and move the live views in and out instead:

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
| a component (normal case) | `present` | the next `present` / `set_child`, or teardown | destroyed; its `on_detach` fired first |
| a fresh `Node` description | `add_child` / `insert_child` / `set_child` | `remove_child` | destroyed |
| a staged component (view parking) | `attach` | `detach` | parked (kept), or `unstage` to destroy |
