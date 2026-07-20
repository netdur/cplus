# App, Screen, and nav

The tiers above one window: `Screen` names the unit of presentation, `App`
owns the process, `nav` moves between screens. All three sit on the existing
component model; nothing about `build`, `find`, lifecycle, or services
changes.

| import | gives |
|---|---|
| `facet/facet` | `Chrome`, `Screen`, `ScreenBox`, `screen_box` |
| `facet/nav` | `go`, `push`, `pop`, `quit`, `arg` |
| `facet/runtime` | `run_screen`, `App` |
| `facet/agent` | MCP serving for `app.agent_mcp` (opt-in) |

## Screen

A screen is an ordinary component plus one conformance: `chrome()`, the
presentation metadata its host reads when showing it. On desktop a screen
shown at the top level is a window; presented into a keyed container it is a
pane; on a mobile backend it is a page.

```cplus
struct Settings { n: i32 }

impl Settings: facet::Component {
    fn build(ref this) -> facet::Node { /* as any component */ }
}
impl Settings: facet::Lifecycle {
    fn on_attach(ref this) { return; }
    fn on_detach(ref this) { return; }
}
impl Settings: facet::Screen {
    fn chrome(this) -> facet::Chrome {
        return facet::Chrome::new(title: "Settings", width: 420.0f64);
    }
}
```

`Chrome` is a plain record built with named parameters; name only what
differs from the defaults:

```cplus
Chrome::new(
    title: str = "facet",
    width: f64 = 480.0, height: f64 = 360.0,
    min_width: f64 = 0.0, min_height: f64 = 0.0,
    clean_titlebar: bool = false, close_button_only: bool = false,
    custom_chrome: bool = false, native_buttons: bool = false,
    unified_toolbar: bool = false, hide_title: bool = false,
    zoomable: bool = false, min_zoom: f64 = 1.0, max_zoom: f64 = 4.0,
) -> Chrome
```

Run one directly:

```cplus
let s: Settings = runtime::run_screen(Settings { n: 0 });
```

`run_screen` is `run_component` with the window read from the screen itself:
it blocks until the window closes and returns the instance with its final
field state (the launcher read-back pattern).

## App

`App` formalizes the sequential-window main loop: one screen at a time as a
blocking window, plus the once-per-process concerns.

```cplus
fn make_welcome() -> facet::ScreenBox { return facet::screen_box(Welcome::new()); }
fn make_workspace() -> facet::ScreenBox { return facet::screen_box(Workspace::new()); }

fn main() -> i32 {
    var app: runtime::App = runtime::App::new("Iris");
    app.screen("welcome", make_welcome);
    app.screen("workspace", make_workspace);
    let _s: status::Status = app.run("welcome");
    return 0;
}
```

| member | role |
|---|---|
| `App::new(name)` | `name` titles the default app menu |
| `app.screen(name, factory)` | register a route; `factory: fn() -> ScreenBox` |
| `app.run(initial, arg?)` | blocks for the app's life; `InvalidInput` on an unknown route |
| `app.menu(build)` | app-global menu bar, `build: fn() -> AppMenu`; without it the App installs `name` + Quit |
| `app.on_launch(f, ctx?)` | once, before the first screen (no window yet) |
| `app.on_quit(f)` | once, after the loop ends |
| `app.agent_mcp(path)` | serve the agent surface at a Unix socket (below) |

The factory constructs a fresh boxed screen each time its route shows. State
that must survive navigation lives in services or module statics, not in the
screen instance. `screen_box` moves the screen to the heap; the box owns it,
and the runtime drops it when the screen leaves.

Closing the current window with no nav intent pending quits the app, as does
the menu's Quit item. Deliberately not part of `App`: services, scene graphs,
diffing. It replaces a hand-written while-loop, nothing else.

## nav

Handlers navigate by route name. The verbs are explicit requests; nothing
re-renders behind them.

| verb | effect |
|---|---|
| `nav::go(route, arg?)` | replace: the current window unwinds (its screen detaches and drops), the target's opens |
| `nav::push(route, arg?)` | overlay: the target opens in a secondary window alongside; `false` if the route is unknown |
| `nav::pop()` | dismiss the most recently pushed screen; `false` when none is up |
| `nav::quit()` | end the app |
| `nav::arg()` | the argument the verb that showed the CURRENT screen carried |

`go` and `quit` also unwind a `run_screen` / `run_component` window; the
caller reads `nav::pending()` / `nav::target()` after the return if it wants
to route by hand. `push` and `pop` need a running `App` (they resolve the
route registry) and report `false` otherwise.

A pushed screen's lifecycle is symmetric with a primary's: `on_attach` after
its window mounts, `on_detach` before its views are torn down, whichever way
it closes (`pop`, its close button, or the primary unwinding — replacing the
root dismisses its pushed screens). `go` dismisses pushed screens before the
next route shows.

Arguments ride the verb: `nav::go("workspace", arg: path)`; the workspace
factory or its `on_attach` reads `nav::arg()`. `app.run(initial, arg:)`
seeds the first screen's the same way.

## The agent surface

```cplus
import "facet/agent" as agent;

agent::enable();                       // once, before app.run
app.agent_mcp("/tmp/iris.sock");
```

After each screen mounts, the App walks the new window into the agent
surface; a worker thread serves `describe_ui` / `click` / `set_text` over
the socket (`agent_mcp` line protocol). Writes marshal to the main thread.
An agent should re-describe after the app navigates; a request racing a
navigation reads a stale tree and gets `Stale` outcomes, never a dangling
view.

`facet/agent` is a separate module so the agent packages are only in the
build when an app actually serves. With `agent_mcp(path)` set but the module
not enabled, `run` prints a pointer instead of silently serving nothing.

## What maps where

| | desktop (facet_appkit) | mobile backend (future) |
|---|---|---|
| screen at top level | window | page |
| `go` | close window, open next | set root |
| `push` / `pop` | secondary window | nav stack push/pop |
| `Chrome` | window chrome | page metadata (title) |

The gtk facade carries the same `run_screen` / `App` shape; members the
backend cannot honor yet are marked in `runtime_linux.cplus`.

## In-window navigation still exists

`App`/`nav` route between top-level screens. Inside one screen, the
established patterns are unchanged: `present` into a keyed outlet for
sub-screens, `stage`/`attach` for view parking. A pager like the tutorial's
ScreenY needs no App involvement at all.
