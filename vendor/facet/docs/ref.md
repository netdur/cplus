# Reference

Compact API map for `facet`. Behavior and recipes live in the topical docs
and [guide.md](guide.md). Tutorial: [tutorial.md](tutorial.md).

```cplus
import "facet/facet" as facet;
import "facet/runtime" as runtime;
```

---

## Modules

| Path | Contents |
|---|---|
| `facet/facet` | Node, DSL, widgets, Component, find/Handle, lifecycle, Chrome/Screen, Color/Style |
| `facet/runtime` | run, run_component, run_screen, App, Window, alert, dialogs, menus (host + backend select) |
| `facet/nav` | go, push, pop, quit, arg (screen navigation) |
| `facet/agent` | opt-in MCP serving (`agent::enable`) |

---

## `Component` and `Lifecycle`

```cplus
interface Component {
    fn build(ref this) -> Node;
}

interface Lifecycle {
    fn on_attach(ref this);
    fn on_detach(ref this);
}
```

`build` once; bind handlers with `.on_click(this.method)`. Lifecycle hooks
are fired FOR the component (by `run_component` and the structural verbs);
`run_component` requires both conformances. Empty hooks are fine.

---

## Addressing

```cplus
fn find(key: str, cp: *u8 = 0) -> Handle
```

Global by default; `cp` scopes to a component subtree. Miss → empty Handle
(no-op mutators). `found()`, `view() -> *u8`.

### Handle leaf mutators (chainable)

| method | effect |
|---|---|
| `set_text(s)` | label / button / field |
| `set_value(v: f64)` | slider / stepper / progress |
| `set_on(on: bool)` | toggle |
| `set_hidden` / `show` / `hide` | visibility |

### Handle structural verbs (keyed containers)

| method | effect |
|---|---|
| `add_child(take Node)` | append |
| `insert_child(take Node, at: usize)` | insert |
| `replace_child(key, take Node) -> bool` | swap |
| `remove_child(key) -> bool` | remove |
| `set_child(take Node)` | replace sole child |
| `present[C: Component + Lifecycle](ref c)` | show a component; hooks fired for it |

Removing / replacing a presented component's content fires its `on_detach`
first, whichever verb does it. Full narrative: [updates.md](updates.md).

---

## Lifecycle

```cplus
interface Lifecycle { fn on_attach(ref this); fn on_detach(ref this); }
fn attached[C](ref c: C) -> bool     // self-liveness: facet::attached(this)
fn is_attached(cp: *u8) -> bool
// present (Handle verb, above); stage / Staged / attach / detach = view parking
// see lifecycle.md for the router pattern and parking signatures
```

---

## Services

```cplus
interface Service { fn produce(ref this); fn apply(ref this); }
fn load_service[S: Service](ref svc: S, on_ready: fn(*u8) = noop, ctx: *u8 = 0)
fn run_on_main(work: fn(*u8), ctx: *u8)
fn spawn_ui[T: Send](take f: future::Future[T])   // async task on the UI thread
async fn on_worker[I: Send, O: Send](take input: I, f: fn(take I) -> O) -> O   // awaitable blocking work
```

`produce` on a worker, `apply` + `on_ready(ctx)` on the main thread; the
service must outlive the flight. See [services.md](services.md).

---

## Leaves (constructors)

| constructor | widget |
|---|---|
| `label` / `wrap_label` | text |
| `button` | push button |
| `text_field` / `secure_field` | single-line input |
| `text_area` / `composer` | multi-line / chat input |
| `toggle` / `slider` / `stepper` / `progress` / `gauge` | value controls |
| `segmented` / `popup` | choice |
| `color_picker` / `date_picker` | platform pickers |
| `image` / `symbol` | media (symbol often Apple-specific) |
| `divider` / `spacer` / `box` | chrome / layout |
| `path` | vector path |
| `list` | recycling list (`row` builder) |
| `native` | adopt app-owned view |

Details and options: [widgets.md](widgets.md).

---

## Containers

`vstack`/`column`, `hstack`/`row`, `zstack`, `grid`, `card`, `scroll`,
`split`, `bordered`, `clickable`, `material`, …

---

## Common modifiers

**Identity / interaction:** `.key`, `.agent_id`, `.on_click`, `.on_drop`,
`.draggable`, `.context_menu`, …

**Layout (flex_layout):** `.grow`, `.shrink`, `.width`/`.height`,
`.width_pct`/`.height_pct`, min/max, `.gap`, `.padding`/`.margin`,
`.align_items`/`.justify_content`, `.flex_direction`/`.flex_wrap`, absolute
position, grid placement, `.aspect_ratio`, `.z_index`, …

**Style:** `.font`, `.monospaced`, `.foreground_color`, `.background`, …

Full tables: [widgets.md](widgets.md).

---

## `Color` / `Style` / `Theme`

```cplus
// Tier 1 (platform): text/text_secondary/text_tertiary, placeholder, link,
//   accent, window_background/under_page_background/control_background,
//   fill/fill_secondary, selected_*_background, separator, system_*
// Tier 2 (theme roles): primary/on_primary, secondary/on_secondary,
//   ink(a?), surface/raised/sunken, content/toolbar/tabstrip/track/chip/
//   recessed, outline, success/warning/danger
// Literals: rgba(r,g,b,a) fixed; adaptive(light:, dark:) — a pair resolved
//   by appearance at paint time
fn set_theme(take t: Theme)     // Theme::new(named optional roles); calling
                                // again re-themes the live app in place
```

`Style` holds font/paint fields used by leaves and containers. Deep dive:
[theme.md](theme.md); token tables: widgets.md.

---

## Runtime host

```cplus
fn run[W: Window](take window: W)
fn run_component[C: Component + Lifecycle](take component: C, title, width, height, ...) -> C
fn run_screen[S: Component + Lifecycle + Screen](take screen: S, menu?, ...) -> S
fn present_window(take root: Node, title, width, height)
fn alert(title, message, primary, secondary?) -> i32
fn choose_file() / choose_directory() -> Option[Text]
// Window interface: root, title, size, chrome flags, menus, close hooks
```

`run_component` fires the component's `on_attach` after the mount and its
`on_detach` (then a teardown drain of presented children) when the loop
stops; it returns the component with its final field state. `run_screen` is
the same run with the window read from the screen's `chrome()`.

Backend selection and porting: [backends.md](backends.md).

---

## Screens and App

```cplus
// facet/facet
struct Chrome { title, width, height, min_*, titlebar flags, zoom }   // Chrome::new(named params)
interface Screen { fn chrome(this) -> Chrome; }
fn screen_box[S: Component + Lifecycle + Screen](take s: S) -> ScreenBox

// facet/runtime
App::new(name) ; app.screen(name, fn() -> ScreenBox) ; app.menu(fn() -> AppMenu)
app.on_launch(f, ctx?) ; app.on_quit(f) ; app.agent_mcp(path)
app.run(initial, arg?) -> Status        // blocks; InvalidInput on unknown route

// facet/nav
fn go(route, arg?)          // replace the current screen
fn push(route, arg?) -> bool // overlay in a secondary window (App only)
fn pop() -> bool             // dismiss the last pushed screen
fn quit()                    // end the app
fn arg() -> str              // the current screen's route argument

// facet/runtime — app context (live while an App runs, inert otherwise)
fn app_running() -> bool
fn app_name() -> str                 // "" when none
fn has_screen(route) -> bool
fn register_screen(route, factory) -> bool   // dynamic route; false if taken

// facet/agent (opt-in MCP serving for app.agent_mcp)
fn enable()
```

Deep dive: [app-screens.md](app-screens.md).

---

## Package

| | |
|---|---|
| Name | `facet` |
| Dependencies | `stdlib`, `flex_layout` |
| Tests | `cpc test` (`src/test_main.cplus`) |
| Backends | `facet_appkit` (primary), `facet_gtk` (stub/partial) |
