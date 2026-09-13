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
`runtime_android` install their backends. The neutral runtime reports no
backend. Shared window ownership lives in `windows.cplus`; retained content
navigation lives in `facet/navigation`.

See the [navigation guide and migration table](../facet/docs/navigation.md)
and the [API reference](../facet/docs/ref.md#facet_runtimeruntime).
Run `cpc test --filter facet_runtime` from this package to test the active
platform facade and native window ownership.
