# Tutorial

A first screen, from an empty file to a window you can click. Combine the
steps in `src/main.cplus`, keeping all imports at the top of the file. Concepts:
[guide.md](guide.md). API: [ref.md](ref.md). Every declared verb:
[contract.md](contract.md).

facet describes user interfaces. It does not draw them: a backend does that.
On macOS the backend is `facet_appkit`, and this tutorial assumes it.

## 1. The package

`Cplus.toml`:

```toml
[package]
name    = "hello"
version = "0.0.1"
edition = "2026"

[dependencies]
stdlib      = "*"
facet       = "*"
facet_runtime = "*"
events      = "*"
flex_layout = "*"

[macos.dependencies]
facet_appkit = "*"
objc         = "*"
appkit       = "*"
quartzcore   = "*"
webkit       = "*"
```

The `[macos.dependencies]` block is the backend and what it needs. A target
with no backend still compiles: facet's verbs become no-ops that say so once
on stderr.

## 2. A tree

A screen is a tree of nodes. Write it with the `@ui` block, which resolves
bare element names through one module:

```cplus
import "facet/facet" as core;
import "facet/elements" as ui;

fn body() -> core::Node {
    return @ui {
        vstack(key: "body") {
            label("hello", key: "greeting")
            button("click me", key: "go")
        }
    };
}
```

Two rules to know now:

`key` is an address. `find("greeting")` resolves it, the agent surface uses it,
and the platform uses it as the accessibility identifier. One token, three
jobs.

The block's value is a container holding the block's items, not the single
item inside it. A block may hold two, so it always holds them.

## 3. State and handlers

State lives in a struct. A bound handler such as `this.bump` receives that
instance automatically. Retain the screen context to scope lookups to its root.

```cplus
import "facet/component" as component;
import "facet/nav" as nav;
import "facet/label" as label;
import "stdlib/option" as option;
import "stdlib/status" as status;
import "stdlib/text" as text;

struct Counter { clicks: i64, context: nav::Context }
impl Counter {
    fn bump(ref this, sender: *u8) {
        this.clicks = this.clicks + (1 as i64);
        let count = this.clicks;
        match label::find("count", within: this.context.root()) {
            option::Option[label::Label]::Some(l) => { let _l = l.set_text("clicked ${count}"); }
            option::Option[label::Label]::None => { }
        }
    }
}
```

The handler finds the one label that changed and writes it. Nothing else in
the tree is visited, compared, or rebuilt. That is the whole update model.

## 4. A component

`Component` supplies the tree. Binding `this.bump` connects the handler to
this retained instance, so state needs no module static.

```cplus
impl Counter: component::Component {
    fn build(ref this) -> core::Node {
        return @ui {
            vstack(key: "body") {
                label("clicked 0", key: "count")
                button("click me", key: "go", on_click: this.bump)
            }
        };
    }
}

impl Counter: component::Lifecycle {
    fn on_attach(ref this, why: component::Attach) { }
    fn on_detach(ref this, why: component::Detach) { }
}
```

## 5. A screen and a window

`Screen` receives navigation context and contributes menu items. Window chrome
belongs to the app's window registration.

```cplus
import "facet/screen" as screen;
import "stdlib/vec" as vec;

impl Counter: screen::Screen {
    fn on_context(ref this, context: nav::Context) { this.context = context; }
    fn menu_items(this) -> vec::Vec[screen::MenuItem] {
        return vec::new::[screen::MenuItem]();
    }
}

fn counter_screen() -> screen::ScreenBox {
    return screen::screen_box(Counter { clicks: 0 as i64, context: nav::Context::none() });
}
```

## 6. Run it

```cplus
import "facet_runtime/runtime" as runtime;
import "stdlib/status" as status;

fn main() -> i32 {
    var app: runtime::App = runtime::App::new("hello");
    app.window("counter", counter_screen, chrome: screen::Chrome::new(
        title: "hello", width: 360.0f64, height: 200.0f64));
    match app.run("counter") {
        status::Status::Ok => { return 0 as i32; }
        _other => { return 1 as i32; }
    }
}
```

`cpc build` then run the binary. Clicking the button renames the label.

## 7. Theme

Colours resolve through roles. Configure the theme after this app becomes
current: add `app.on_launch(configure_theme)` before `app.run(...)`, with:

```cplus
import "facet/theme" as theme;
import "facet/vocabulary" as vocab;

fn configure_theme(ctx: *u8) {
    theme::set_theme(theme::Theme::new(
        primary: vocab::Color::rgba(0.30f64, 0.42f64, 0.85f64, 1.0f64),
        surface: vocab::Color::adaptive(
            light: vocab::Color::rgba(0.97f64, 0.97f64, 0.98f64, 1.0f64),
            dark: vocab::Color::rgba(0.11f64, 0.11f64, 0.13f64, 1.0f64),
        ),
    ));
}
```

`adaptive` carries both sides. The backend picks by the current appearance and
repaints when the system flips.

## 8. Layout

Layout is `flex_layout`'s. Nodes carry the modifiers:

```cplus
core::set_grow(n, 1.0f64);
core::set_padding(n, 12.0f64);
core::set_gap(n, 8.0f64);
```

Controls size themselves from their content, so a label is as wide as its
text without being told. A control with no intrinsic width takes the space it
is offered.

## 9. Where to go next

`guide.md` explains the model: what a key is, why there is no re-render, how
the dirty word reaches the backend, and what a backend has to fill.

`contract.md` lists every declared verb with the ledger row it came from. A verb
absent there does not exist: calling it is a compile error, never a silent
no-op.

For what the macOS backend does with each verb, including where it deviates,
read `vendor/facet_appkit/MANIFEST.md`.

## Next: multiple windows and navigation

`App::run` installs the backend. To add another native window, register another
window definition and call `app.open_window`. To change content inside a window,
register routes on its definition and use its `nav()` handle. There is no
history across windows.

The [navigation guide](navigation.md) covers keyed instances, default slots,
Back/Forward state, screen arguments, and explicit desktop/mobile composition.
