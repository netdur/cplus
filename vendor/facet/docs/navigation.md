# Apps, windows, and navigation

Facet has one application startup path: register windows with `app.window(...)`,
then call `app.run(...)`. Windows own content routes and history. Opening or
closing a window is an explicit application operation.

## Ownership

A shipped product can contain several independent apps: desktop and mobile,
main and failsafe, or licensing and main. At most one runs at a time. The caller
chooses which app to run; navigation neither switches apps nor connects their
histories.

An app owns window definitions and their live instances. A window can display
one screen without any routes, or register routes for content navigation.
Screens and ordinary components can be shared between app compositions.

| Level | Responsibility |
|---|---|
| App | Startup, window registration, opening/finding windows, menus, app lifecycle |
| Window definition | Screen factory, native window settings, available content routes |
| Window instance | Name/key identity, activation, closing, its navigator |
| Navigator | Routes, Back/Forward history, whole-content and slot transitions |
| Screen | Content, instance arguments, navigation context and state notifications |
| Flex layout | Wide/narrow layout and visibility rules |

## Register and run

Given factories returning `screen::ScreenBox`:

```cplus
let app = runtime::App::new("Notes");
let main = app.window("main", home::Home::boxed,
    chrome: screen::Chrome::new(title: "Notes", width: 960.0f64, height: 700.0f64));
main.route("overview", overview::Overview::boxed);
main.route("editor", editor::Editor::boxed);
app.window("note", note::Note::boxed,
    chrome: screen::Chrome::new(title: "Note"));
let result = app.run("main");
```

Registration does not open a window. `window` returns a definition handle;
`is_valid()` checks registration, and `route` returns `bool`. Empty names,
duplicate names within the same registry, and null factories are refused.
Different window definitions can register different factories under the same
route name.

`App::run(initial, key: "") -> Status` installs the target backend and opens the
initial window. A second app cannot run while one is active. On desktop the
call returns when the native loop exits; Android returns to its Activity-owned
loop, while iOS enters UIKit's loop. Configure an app's theme in its `on_launch`
hook, after that app becomes current. Use `on_quit` for desktop shutdown;
mobile persistence must not depend on a process-exit callback.

Window title, dimensions, bar style, and zoom settings belong to the
`chrome:` argument on registration. `Screen` has no `chrome()` method.
The `Chrome` and `Bar` types remain in `facet/screen`.

## Access the running app from a screen

`runtime::app() -> runtime::App` returns a handle to the running app. Use it
inside a screen or component action to open or find another window:

```cplus
import "facet_runtime/runtime" as runtime;

fn open_model_library(sender: *u8) {
    let opened = runtime::app().open_window("model_library");
}
```

Register `"model_library"` with `app.window(...)` during app setup. The action
creates its default instance or activates the existing one. No app-specific
global `APP`, `windows::install(app)`, or second `App::new(...)` is needed.
`App::new` creates an independent app; it does not retrieve the running one.

Only one app runs at a time, so this accessor has one application destination.
For navigation within the screen's own window, keep using its
`context.navigator()`. To navigate another window, obtain that instance through
`runtime::app().find_window(...)`, then use its `nav()` as shown below.

Outside a running app, `runtime::app()` returns the process default handle;
it does not start an app, and `open_window` returns `None`. Use
`runtime::app_running()` when a callback can also run outside app lifetime.
Fetch the handle when the action runs instead of caching the current app globally.

The accessor currently exists in the macOS, Linux, iOS, Android, and neutral
facades. The Windows facade does not yet expose `runtime::app()`.

## Name, key, and instance lifetime

The pair `(name, key)` identifies a window within its app. These are separate
values; do not encode the pair as a colon-separated route string.

```cplus
match app.open_window("note", key: "/notes/shopping") {
    option::Option[window::Window]::Some(w) => {
        // Newly created, or the existing instance activated with its state intact.
        let name = w.name();
        let path = w.key();
    }
    option::Option[window::Window]::None => { /* opening was refused */ }
}
```

`open_window(name, key: "", params:)` returns `Option[window::Window]`.
The omitted key selects that definition's default instance. Reopening a live
name/key requests activation and preserves its screen state, arguments, and
history; new `params` do not overwrite an existing instance's arguments.
Opening requires that app to be running.

`find_window(name, key: "")` retrieves an existing instance without creating
or activating it. The handle offers `activate()`, `close()`, `is_live()`,
`root()`, and `nav()`, as well as native window readers and setters.

Closing disposes that instance and its history. Reopening its name/key creates
a fresh instance. A stale handle stays stale; it cannot target the replacement.
Closing one registered window does not invalidate another window's handle.
There is no Back history across windows.

## Content commands

Obtain a navigator from a window's `nav()` or a screen's
`context.navigator()`. Every command applies to that particular window.

| Operation | Result and meaning |
|---|---|
| `push(route, arg: "", params:, into: "", key: "")` | `bool`; display a fresh screen and add a Back entry |
| `replace(route, arg: "", params:, into: "", key: "")` | `bool`; replace content in the target, preserving its history position |
| `pop()` | `bool`; undo the latest push in this window |
| `forward()` | `bool`; restore the latest undone entry |
| `can_pop()` / `can_forward()` | `bool`; whether the corresponding history exists |
| `depth()` | Number of Back entries, excluding base content |
| `current(into: "")` | `Option[nav::Context]` for the active screen in the target |
| `state()` | Current `nav::State` snapshot |

An omitted `into:` targets the entire content area. The native window remains
the same instance. A navigation `key:` identifies a screen entry for inspection;
it is separate from the window's instance key and does not deduplicate pushes.

Unknown routes, missing or ambiguous slots, stale handles, and commands during
an unfinished transition return `false`. A pending native window has a handle,
but navigation is refused until it mounts. `can_pop()` describes history; it
does not override the transition guard.

## Slots and history

A slot can supply authored base content or name a default route:

```cplus
return @ui {
    column {
        label("Notes")
        slot("details", route: "overview")
    }
};
```

Register the default route on the owning window definition. If the default
cannot be resolved, the current implementation keeps the slot's authored
content. An empty slot without a route is also valid.

```cplus
w.nav().push("editor", arg: note_path, into: "details");
```

Only `details` changes. The surrounding screen remains mounted and receives
navigation-state notifications. Slot lookup stays inside the addressed window's
active navigation content, excluding parked screens. It does not inspect width
or redirect a flex-hidden slot elsewhere.

Each window has one chronological history across its targets. For example:

1. Push A into `details`.
2. Push B into `preview`.
3. Back restores `preview`'s base; Back again restores `details`'s base.

Back and Forward retain screen instances, preserving edits and local state.
A successful new push or replacement disposes the abandoned Forward branch;
a failed command preserves it. Replacing A while B is newer leaves A's position
in the history unchanged. Replacing a registered base screen adds no Back entry. A slot with only
authored content has no registered screen entry yet; its first `replace`
currently creates a routed entry with a Back path to that content. Replacing
a parent disposes its nested screens and their associated history.

## Screen arguments and navigation events

A screen implements `Component`, `Lifecycle`, and `Screen`. `on_context` runs
before build. `on_navigation` runs after mounting and after committed
transitions; it also reaches the surrounding screen when a slot changes.
Both hooks have default empty implementations. `menu_items()` is required.

This complete screen uses its own root to update its Back button:

```cplus
import "facet/facet" as core;
import "facet/elements" as ui;
import "facet/component" as component;
import "facet/screen" as screen;
import "facet/nav" as nav;
import "facet/button" as button;
import "stdlib/option" as option;
import "stdlib/vec" as vec;

struct Page { context: nav::Context }
impl Page {
    fn boxed() -> screen::ScreenBox {
        return screen::screen_box(Page { context: nav::Context::none() });
    }
    fn back(ref this, sender: *u8) { let _popped = this.context.navigator().pop(); }
}
impl Page: component::Component {
    fn build(ref this) -> core::Node {
        return @ui {
            column {
                button("Back", key: "back", on_click: this.back)
                label(this.context.arg(), key: "document")
            }
        };
    }
}
impl Page: component::Lifecycle {
    fn on_attach(ref this, why: component::Attach) { }
    fn on_detach(ref this, why: component::Detach) { }
}
impl Page: screen::Screen {
    fn on_context(ref this, context: nav::Context) { this.context = context; }
    fn on_navigation(ref this, state: nav::State) {
        match button::find("back", within: this.context.root()) {
            option::Option[button::Button]::Some(b) => { let _b = b.set_enabled(state.can_pop()); }
            option::Option[button::Button]::None => { }
        }
    }
    fn menu_items(this) -> vec::Vec[screen::MenuItem] { return vec::new::[screen::MenuItem](); }
}
```

`Context` exposes `arg()`, `param(name)`, `window_name()`, `window_key()`,
`root()`, `navigator()`, and `is_live()`. Arguments belong to the screen instance,
including when it is parked. Another window's navigation cannot change them.
Before mounting, context identity and arguments are available; the root is not
yet available for finding mounted controls.

Create named arguments with `nav::params_new()`, insert owned `text::Text`
keys and values, and move the map into `params:`. `arg:` is the convenience
spelling for the `"@arg"` entry in that map.

`State` contains the navigator, `back_count`, `forward_count`, and `revision`.
Use `can_pop()` and `can_forward()` to update controls. Notifications describe
committed state; synchronous navigation from a notification is refused while
the current transition finishes.

## Navigate another window

From a screen or component action, retrieve the running app first:

```cplus
let app = runtime::app();
match app.find_window("settings") {
    option::Option[window::Window]::Some(w) => {
        let changed = w.nav().push("model_settings", into: "details");
        if changed { let _activated = w.activate(); }
    }
    option::Option[window::Window]::None => { }
}
```

The target's routes and history apply. Activation is a separate choice.
`screen::find(w.nav(), key)`, `screen::top(w.nav(), into:)`, and
`screen::stack(w.nav())` inspect retained screens in that window; the latter
includes base and Forward entries, so it is not the same as `nav().depth()`.

## Desktop and mobile

A product may explicitly choose `desktop_app` or `mobile_app` and share their
screen implementations. For example, desktop can open a note window while
mobile pushes the same editor into its single window. A shared component can
report an action through a callback so its app composition chooses the result.

Android uses one app window. Its native Back adapter calls that window's
navigator, then falls through to platform behavior at the root. iPhone window
opening does not become a content push; additional iPad windows require an app
that opts into multiple scenes. OS window activation and scene creation can be
asynchronous, and native capabilities can refuse operations.

The shared content engine supports screen-driven Back on iOS. Native interactive
Back gestures and iPad restoration were not validated with this redesign; the
older UIKit controller-stack implementation is not evidence of those working
through the new navigator. Mobile archives were built during the migration,
not exercised on devices.

## Migration from the previous API

| Removed API | Replacement |
|---|---|
| `app.screen(name, factory)` | `app.window(name, factory, chrome:)` |
| `Screen::chrome()` | `chrome:` on window registration |
| Free `runtime::run`, `run_component`, `run_screen` | Register a window, then `App::run` |
| `runtime::Window` interface | Window definition plus a `screen::Screen` factory |
| `runtime::present_window` | Explicit window registration/opening; dialogs use their own APIs |
| `nav::go` | Choose explicitly: open/activate a window, or replace its content |
| Global `nav::push`, `replace`, `forward` | The destination window's navigator |
| Global `nav::arg`, `param` | The screen's retained `nav::Context` |
| `nav::Show::Window` / platform fallback | Explicit app composition; opening never becomes a push |
| `nav::quit` | `app.quit()` where the platform supports application quitting |

`nav::pop` and `set_pop_fn` remain as the native Android Back adapter seam.
Application content commands use `w.nav().pop()` or the screen's context.

The [legacy app shell](../../../examples/app_shell/src/main.cplus) and
[Liquid Glass app shell](../../../examples/app_shell_glass/src/main.cplus) show
route registration, a default slot, and navigation-state-driven controls.
