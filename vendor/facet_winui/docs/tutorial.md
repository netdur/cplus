# Tutorial

See [guide.md](guide.md) for lifetime rules and [ref.md](ref.md) for signatures.

For the full gallery, use [facet_gallery_winui](../../../examples/facet_gallery_winui/README.md).
For a smaller example, use [facet_winui_smoke](../../../examples/facet_winui_smoke/README.md).
Its build script stages the SDK runtime, embeds activation and DPI declarations,
and creates a local dependency junction. A plain executable without those
deployment steps cannot activate WinUI reliably.

Build a normal retained tree with `facet/elements`, then transfer it to the host:

```cplus
let status: rt::Status = host::run(tree, "My application");
```

Keep any component whose bound methods are in that tree alive through `run`.
The sample uses a Box for a stable component address. Handlers use normal Facet
cursors, such as `label::find("count")` followed by `set_text`, to update live
controls. The host dispatches those dirty writes on the next WinUI queue turn.

Containers, labels, buttons, text fields, and scroll views are supported.
Give nodes keys. Leaf intrinsic size comes from WinUI; explicit Facet dimensions also work. Layout
and child ordering remain Facet's. Read [coverage](../MANIFEST.md) before adding
other controls or services.

Text handlers can read `text()` after resolving a `text_field::find` cursor:
native text is already reflected in the retained properties. Application
`set_text` updates the TextBox without raising a user edit event.
For scroll content, create `ui::scroll`, find its cursor within the unmounted
node, and use `set_content`. The sample shows a document taller than its viewport
with `shrink(0)` so flex layout preserves its scrollable height.

## A runtime::App on WinUI

An application with several windows, routes and dialogs is better written
against `facet_runtime` and pointed at this backend with one call:

```cplus
let app = runtime::App::new("Notes");
app.window("main", home_factory, chrome: screen::Chrome::new(title: "Notes",
    width: 640.0f64, height: 420.0f64, bar: screen::Bar::Blended));
app.window("settings", settings_factory);
winui::select();                      // import "facet_runtime/winui" as winui
app.run("main");
```

`runtime::app().open_window("settings")` then opens a second WinUI window and
`runtime::alert(...)` shows a keyed sheet in the active one.
[facet_runtime_winui](../../../examples/facet_runtime_winui/README.md) is that
app, with a build script that stages the runtime the same way.
