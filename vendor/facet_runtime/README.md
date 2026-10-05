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

## Windows: Win32 or WinUI

Windows has two backends, and the platform file variant only picks the OS.
`runtime_windows` runs on `facet_win32` unless the app selects WinUI before
`run`:

```cplus
import "facet_runtime/winui" as winui;   // Windows only: there is no base file

fn main() -> i32 {
    let app = runtime::App::new("Notes");
    app.window("main", home_factory, chrome: screen::Chrome::new(title: "Notes"));
    winui::select();
    if app.run("main").is_ok() { return 0; }
    return 1;
}
```

The app that does this lists `facet_winui`, `winui` and `winrt` under
`[windows.dependencies]` beside `facet_win32` and `win32`. An app that never
imports `facet_runtime/winui` lists none of them and never compiles the WinUI
bindings: `runtime_windows` names no WinUI package and reaches it only through
a seam (`runtime::Backend`, filled by `select()`). For that reason this
package is source mode on Windows (`[windows.build] prebuild = false`): a
prebuilt archive is compiled from every active module, `winui_windows`
included, and would put WinUI into every Windows build.

Selected, WinUI serves the whole facade surface `App` uses: one native window
per `open_window` (sessions opened before launch are queued until the XAML
Application starts), `screen::Chrome` (size, minimum/maximum, bar, buttons),
`runtime::alert` / `choose` / `prompt` / `alert_blocking` as keyed facet
sheets on the active window (`alert:title`, `alert:primary`, ...), the window
verbs, frame/density/activity reads and the active/inactive/size observers.
Closing the last window ends `run`. Not on WinUI yet: the app menu
(`App::menu`; menus inside a screen's tree do render), `on_should_quit` on a
window close, density-change observation, and the agent surface beyond the
first window - see [facet_winui's MANIFEST](../facet_winui/MANIFEST.md).
`examples/facet_runtime_winui` is a two-window app on this path. The neutral runtime reports no
backend. Shared window ownership lives in `windows.cplus`; retained content
navigation lives in `facet/navigation`.

See the [navigation guide and migration table](../facet/docs/navigation.md)
and the [API reference](../facet/docs/ref.md#facet_runtimeruntime).
Run `cpc test --filter facet_runtime` from this package to test the active
platform facade and native window ownership.
