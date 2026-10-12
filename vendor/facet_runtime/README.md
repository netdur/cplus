# facet_runtime

Facet's application entry point. `App::run` installs the target backend;
apps register windows and each window definition can register content routes.

```cplus
import "facet_runtime/runtime" as runtime;
import "facet/screen" as screen;

fn main() -> i32 {
    let app = runtime::App::new("Notes");
    let main = app.window("main", home_factory,
        chrome: screen::Chrome::new(title: "Notes"));
    main.route("editor", editor_factory);
    let result = app.run("main");
    if result.is_ok() { return 0; }
    return 1;
}
```

Factories return `screen::ScreenBox`. Window title, size, and controls come
from registration; screens receive `nav::Context` and `nav::State` hooks.

From a screen or component action, use `runtime::app()` to retrieve the running
app: `runtime::app().open_window("model_library")`. Register that window during
setup. No global app storage or `windows::install(app)` helper is needed.
`App::new(...)` creates another app; it does not retrieve the current one.
See [app access and platform availability](../facet/docs/navigation.md#access-the-running-app-from-a-screen).

`open_window(name, key:)` creates or activates a window instance;
`find_window(name, key:)` retrieves it without activation. Both return
`Option[window::Window]`. `w.nav()` owns that instance's routes and history.
Closing a window disposes its content and history. Opening another window
never becomes content navigation. Desktop and mobile apps explicitly compose
shared screens for their platform.

The alternate `run`, `run_component`, `run_screen`, and `present_window`
functions and the `runtime::Window` interface have been removed. Use
`app.window(...)` and `app.run(...)` for demos as well as applications.

`runtime_macos`, `runtime_linux`, `runtime_windows`, `runtime_ios`, and
`runtime_android` install their backends.

## Windows: WinUI, and the Win32 opt-out

`runtime_windows` runs the app on **WinUI** (`facet_winui`), the Windows
backend (netdu/cplus#20). An app names no backend; its manifest lists
facet_runtime's Windows closure:

```toml
[windows.dependencies]
facet_winui = "*"
winui       = "*"
winrt       = "*"
win32       = "*"   # the shell services: clipboard and file pickers
```

A plain `cpc build` output runs wherever the Windows App Runtime 2 is
installed; see [facet_winui's Deployment](../facet_winui/README.md#deployment).

WinUI serves the whole facade surface `App` uses: one native window per
`open_window` (sessions opened before launch are queued until the XAML
Application starts, and timers scheduled before launch are armed then),
`screen::Chrome` (size, minimum/maximum, bar, buttons), `runtime::alert` /
`choose` / `prompt` / `alert_blocking` as keyed facet sheets on the window that
asked (`alert:title`, `alert:primary`, ...), the window verbs, frame/density/
activity reads and the active/inactive/size/density observers. Closing the last
window ends `run`. A user close (caption button, Alt+F4) asks the window's
`on_should_close`, and closing the last window also asks `on_should_quit`, as
`app.quit()` does. `App::menu` is a MenuBar in every window, its `key`s Ctrl
shortcuts, and `facet_agent` serves every window through one connection - see
[facet_winui's MANIFEST](../facet_winui/MANIFEST.md). The clipboard and the
file and folder pickers are `src/shell_windows.cplus`, over `win32`, whichever
backend draws. When WinUI cannot start (no runtime), `run` answers
`InvalidInput` with a message naming the installer.

The deprecated Win32 backend stays reachable as an explicit opt-out until it
is removed:

```cplus
import "facet_runtime/win32" as win32;   // Windows only: there is no base file
win32::select();                          // before app.run(...)
```

The app that does this lists `facet_win32` beside the closure above. An app
that never imports `facet_runtime/win32` never compiles facet_win32:
`runtime_windows` reaches it only through a seam (`runtime::Backend`, filled by
`select()`). For that reason this package is source mode on Windows
(`[windows.build] prebuild = false`): a prebuilt archive is compiled from every
active module, `win32_windows` included. `facet_runtime/winui`'s `select()`, the
old WinUI opt-in, still exists and does nothing.

`examples/facet_runtime_winui` is a two-window app on WinUI; `examples/facet_runtime_win32` takes the opt-out. The neutral runtime reports no
backend. Shared window ownership lives in `windows.cplus`; retained content
navigation lives in `facet/navigation`.

See the [navigation guide and migration table](../facet/docs/navigation.md)
and the [API reference](../facet/docs/ref.md#facet_runtimeruntime).
Run `cpc test --filter facet_runtime` from this package to test the active
platform facade and native window ownership.
