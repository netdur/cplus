# Component lifecycle — stage / attach / detach

> Entry path: [tutorial.md](tutorial.md) · [guide.md](guide.md) · [ref.md](ref.md)

You can build a component **off-canvas**, hold it, then attach it into the live
tree at the right moment and detach it later — without rebuilding it. Its views
and native state survive being parked, so re-attaching restores it exactly:
scroll position, half-typed input, selection. This is the router / navigation
stack: one nav owns every screen and attaches one at a time; navigating back
shows the screen as you left it, because the view object never died.

This is the imperative, non-reactive equivalent of Web Components'
`connectedCallback` / `disconnectedCallback` / `isConnected`.

## The verbs

```cplus
struct Staged { }                    // an opaque handle to a staged component

fn stage(build: fn(*u8) -> Node,
         on_attach: fn(*u8) = lifecycle_noop,
         on_detach: fn(*u8) = lifecycle_noop) -> Staged
fn is_attached(cp: *u8) -> bool
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

`stage`, `on_attach`, and `on_detach` are **bound method references**
(`Screen.build`, `Screen.on_attach`). Each auto-fills its own context slot with
the component's address, so you pass the methods and never the ctx — the same
mechanism `mount_component` uses for `build`.

## The lifecycle callbacks

`attach` and `detach` do nothing to your logic; they only **notify** the
component that it happened. Opt in by implementing `Lifecycle`:

```cplus
interface Lifecycle {
    fn on_attach(ref this);
    fn on_detach(ref this);
}
```

What to do on detach — cancel an in-flight request, let it run but drop the
result, keep streaming — is the app's policy, never facet's.

`is_attached(cp)` is the guard a handler checks at an async completion before it
touches the UI: the imperative version of "don't update a detached view", as a
plain boolean.

```cplus
impl Feed: facet::Component { fn build(ref this) -> facet::Node { ... } }

impl Feed: facet::Lifecycle {
    fn on_attach(ref this) { this.reload(); }        // came on screen
    fn on_detach(ref this) { /* your policy */ }
}

// an async completion, possibly long after navigating away:
fn feed_loaded(cp: *u8) {
    if !facet::is_attached(cp) { return; }            // parked -> don't touch a dead view
    facet::find(cp, "list").set_text(...);            // live -> normal keyed-direct update
}
```

## A router

A nav owns a keyed `"outlet"` container, stages each screen once, and tracks the
screen currently shown. Routing detaches the current screen and attaches the
target. Because detach keeps the screen parked, navigating back restores its
exact state for free.

```cplus
// the nav tracks the current screen (Staged is Copy)
struct Nav { current: facet::Staged }
static NAV: Nav = #zero::[Nav]();

// stage the screens off-canvas (eagerly, or lazily on first visit)
static HOME: facet::Staged = #zero::[facet::Staged]();
static SETTINGS: facet::Staged = #zero::[facet::Staged]();
// HOME = facet::stage(Home.build, on_attach: Home.on_attach, on_detach: Home.on_detach);
// SETTINGS = facet::stage(Settings.build, on_attach: Settings.on_attach, on_detach: Settings.on_detach);

fn route_to(nav_cp: *u8, target: facet::Staged) {
    if NAV.current.valid() { let _d: bool = NAV.current.detach(); }   // park the current screen (kept)
    let _a: bool = facet::find(nav_cp, "outlet").attach(target);       // target.on_attach fires
    NAV.current = target;
}
```

## stage / attach vs the structural verbs

`add_child` / `set_child` (see [updates.md](updates.md)) mount a **fresh** `Node`
and **destroy** it on removal. `attach` / `detach` move a **live, retained**
component in and out; `detach` parks it, it is not torn down. Different verbs for
different lifetimes:

| you have | put it in with | remove with | on removal |
|---|---|---|---|
| a fresh `Node` description | `add_child` / `insert_child` / `set_child` | `remove_child` | destroyed |
| a live staged component | `attach` | `detach` | parked (kept), or `unstage` to destroy |
