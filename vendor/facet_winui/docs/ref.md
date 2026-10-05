# Reference

Import `"facet_winui/facet_winui" as host`.
[Tutorial](tutorial.md) · [Guide](guide.md) · [Coverage](../MANIFEST.md)

An application written against `facet_runtime` does not call this module: it
selects WinUI with `facet_runtime/winui`'s `select()` and keeps using
`runtime::App` (see the [guide](guide.md#through-facet_runtime)). The functions
below are the explicit host that driver is built on.

## One window

```cplus
fn run(take tree: core::Node, title: str = "Facet / WinUI",
       on_ready: fn(*u8) = 0 as fn(*u8), on_ready_ctx: *u8 = 0 as *u8) -> rt::Status;
fn run_with(take tree: core::Node, take chrome: screen::Chrome,
            on_ready: fn(*u8) = 0 as fn(*u8), on_ready_ctx: *u8 = 0 as *u8) -> rt::Status;
```

Consume a retained Facet tree, initialize the runtime/apartment, open one
window for it and run until every window has closed. `run` opens at the
system's default size with a native caption; `run_with` applies a
`screen::Chrome` (see *Chrome* below). `on_ready` runs once, when the first
window's content has loaded. Callback owners must outlive the call. Releases
the tree's native records and handlers before returning. Initial unsupported
kinds/adopted views/leaf children return E_NOTIMPL; a second run attempt
returns E_UNEXPECTED. Deployment prerequisites are described in the sample
README. Internal renderer HRESULT failures are currently fatal.

## Many windows

```cplus
fn start(on_launch: fn(*u8), on_launch_ctx: *u8 = 0 as *u8,
         on_ready: fn(*u8) = 0 as fn(*u8), on_ready_ctx: *u8 = 0 as *u8,
         on_stopped: fn(*u8) = 0 as fn(*u8), on_stopped_ctx: *u8 = 0 as *u8) -> rt::Status;
fn open_window(root: *core::Node, take chrome: screen::Chrome,
               will_close: fn(*u8) = 0 as fn(*u8), will_close_ctx: *u8 = 0 as *u8,
               owned: bool = false) -> *u8;
fn launched() -> bool;
fn window_count() -> usize;
fn close_window_of(root: *core::Node) -> bool;
fn close() -> rt::Status;
```

`start` runs the one WinUI Application of the process. A XAML window cannot
exist before Application.Start launches, so windows are opened from
`on_launch` (and afterwards from any UI-thread callback) with `open_window`,
which answers null before launch, for an unsupported tree, or on failure.
The answer is an opaque window handle, also recorded as the root's
`app::window_native`.

`open_window` borrows `root` unless `owned` is true, in which case the root
is a `box::Box[core::Node]` raw pointer the host frees after unmounting.
`will_close` runs on Window.Closed while the views still exist; the tree is
unmounted after it returns. Closing any window unmounts only its tree; closing
the last one retires the process-wide services and returns from `start`, after
`on_stopped`, which is where a borrower drops the roots it lent (their native
records are then released while the XAML thread is still up). `close` closes
every window; `close_window_of` the one showing `root`.

## Chrome

`screen::Chrome` maps onto AppWindow and its OverlappedPresenter:

| Chrome | WinUI |
|---|---|
| `width`, `height` | `AppWindow.ResizeClient`, in points scaled by the window's DPI; 0 leaves the system default |
| `min_width/height`, `max_width/height` | `OverlappedPresenter.PreferredMinimum/MaximumWidth/Height`; 0 is unbounded |
| `minimizable`, `maximizable` | `IsMinimizable`, `IsMaximizable`; `IsResizable` follows `maximizable` (facet_gtk's rule) |
| `Bar::Native` | the system caption, dark through `DWMWA_USE_IMMERSIVE_DARK_MODE` |
| `Bar::Blended` + the tree has `window_buttons()` | `SetBorderAndTitleBar(true, false)`: no system caption, the app's buttons are the only set, border and resize edges stay |
| `Bar::Blended`, no `window_buttons()` | `ExtendsContentIntoTitleBar`: content runs under the caption, the system buttons float over it |
| `Bar::Hidden`, `Bar::Custom` | `SetBorderAndTitleBar(false, false)` |
| `.window_drag()` | on a captionless window, a press no control took inside the region starts the system move loop (`WM_NCLBUTTONDOWN`/`HTCAPTION`) |

`title_text` is the window title. `subtitle_text`, `zoomable` and the zoom
range are not mapped.

## Modal layer and dialogs

```cplus
fn present_layer(window: *u8, root: *core::Node, width: f64, height: f64,
                 on_key: fn(i32, *u8) -> bool = ..., on_key_ctx: *u8 = 0 as *u8) -> bool;
fn dismiss_layer(window: *u8);
fn set_layer_gone(f: fn(*u8));
fn focus_when_loaded(n: *core::Node);
fn active_host() -> *u8;
```

One facet tree shown over a window's content, centred on a card above a scrim
that takes every press. The tree is mounted like a window, so `find` and the
agent reach its keys. `on_key` sees virtual keys pressed in the card that no
control handled. `set_layer_gone` is told when a window closes under its
layer. `active_host` is the most recently activated window.

`facet_winui/dialogs` builds the facade's dialogs on it, with the trees every
other backend uses:

```cplus
fn alert_sheet(title: str, message: str, primary: str, secondary: str = "",
               on_answer: fn(i32, *u8) = ..., on_answer_ctx: *u8 = 0 as *u8);
fn choose_sheet(title: str, message: str, take options: vec::Vec[text::Text],
                on_answer: fn(i32, *u8) = ..., on_answer_ctx: *u8 = 0 as *u8);
fn prompt_sheet(title: str, message: str, placeholder: str, primary: str, secondary: str = "",
                on_typed: fn(str, *u8) = no_typed, on_typed_ctx: *u8 = 0 as *u8,
                on_answer: fn(i32, *u8) = ..., on_answer_ctx: *u8 = 0 as *u8);
fn alert(title: str, message: str, primary: str, secondary: str = "") -> i32;
```

Keys: `alert:title`, `alert:message`, `alert:primary`, `alert:secondary`,
`choose:opt:N`, `prompt:value`, `prompt:ok`, `prompt:cancel`. Answers arrive
on a later turn (0 primary, 1 secondary, or the option index). Return answers
the primary and Escape the secondary in alerts and prompts. `alert` blocks in
a nested message loop and answers -1 when it could not be shown or its window
closed under it. One sheet at a time; a second is refused.

## Window reads, verbs and observers

```cplus
fn window_close(sender: *u8);    fn window_minimize(sender: *u8);    fn window_zoom(sender: *u8);
fn window_frame_of(root: *core::Node) -> vocab::Rect;
fn set_window_frame_of(root: *core::Node, frame: vocab::Rect);
fn density_of(root: *core::Node) -> f64;
fn is_window_active(root: *core::Node) -> bool;
fn observe(kind: i32, call: fn(*u8), ctx: *u8 = 0 as *u8) -> u64;   // 1 activated, 2 deactivated, 3 resized
fn cancel_observe(id: u64);
fn presenter() -> ui::IOverlappedPresenter;                          // the first window's
```

Verbs take a sender record inside the window. Frames are the outer window in
screen pixels (`GetWindowRect`); density is the window's DPI over 96. The host
also installs `facet/window`'s seam (frame, title, density, activity, close)
and `app::set_activate_fn`, so `window::Window` cursors work on WinUI.

## Diagnostics

```cplus
fn live_views() -> i64;
fn live_subscriptions() -> i64;
```

UI-thread diagnostic counts for owned native view records and control event
subscriptions. Both must be zero after `run` returns. These are not counts of
all internal allocations or COM references in the Windows App SDK.

The sibling `views` and `native` modules are implementation support, not a
second application API. Native pointers exposed to handlers are borrowed View
records; use `component::key_of` / `item_of`, never cast them to HWND.
