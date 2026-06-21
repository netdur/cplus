# gtk

C+ bindings for **GTK 4** — the cross-platform (Linux/BSD-first, also
Windows/macOS) GUI toolkit. The Linux counterpart to `vendor/appkit` (macOS)
and `vendor/uikit` (iOS), organized the same way.

Unlike Cocoa, GTK is a plain **C / GObject** library, so there is no
`objc_msgSend` dispatch: every binding is a direct `extern fn` to a `gtk_*` /
`g_*` symbol, called inside an FFI block. That makes this package simpler than
the AppKit one — thin C+ structs over GObject pointers.

## Building

GTK is a real native dependency, so — unlike the iOS/macOS bindings, which stop
at object emission — a GTK app links and runs on the host:

```
cpc build            # build the consuming app
```

Install GTK 4 first. On Debian/Ubuntu:

```
sudo apt install libgtk-4-dev
```

That provides the `gtk4` pkg-config file and the unversioned `.so` link
symlinks the linker needs. The `[link]` libs declared here (`gtk-4`,
`gobject-2.0`, `gio-2.0`, `glib-2.0`, `cairo`) land on the consumer's link
line; the loader resolves the rest of the stack (pango, gdk-pixbuf, graphene,
harfbuzz) transitively.

## Usage

Add the dependency in your `Cplus.toml`:

```toml
[dependencies]
gtk = "*"
```

Import the umbrella facade:

```cplus
import "gtk/gtk" as gtk;
```

Or import narrower modules directly:

```cplus
import "gtk/glib" as glib;
import "gtk/application" as application;
import "gtk/window" as window;
import "gtk/widget" as widget;
import "gtk/controls" as controls;
import "gtk/containers" as containers;
import "gtk/text" as text;
import "gtk/dialogs" as dialogs;
import "gtk/graphics" as graphics;
import "gtk/cairo" as cairo;
import "gtk/menu" as menu;
import "gtk/actions" as actions;
import "gtk/data" as data;
// import-directly modules (no umbrella re-export):
import "gtk/convert" as bridge;
import "gtk/events" as events;
import "gtk/pasteboard" as pasteboard;
import "gtk/drag" as drag;
import "gtk/notifications" as notifications;
```

## Modules

- `glib`: the runtime core — GObject reference counting (`ref_obj` / `unref` /
  `ref_sink`), associated data, the `g_signal_connect` family (typed by
  callback shape: `signal_connect`, `signal_connect3`, `signal_connect_state`),
  the main loop helpers (`idle_add` / `timeout_add` / `source_remove`), GTK
  version, and the enum constants (`orientation_*`, `align_*`, `policy_*`,
  `application_default_flags`).
- `application`: `Application` (GtkApplication) — `new`, `connect_activate`,
  `run`, `quit`, `set_menubar`, `add_action`, `unref`.
- `window`: `Window` (GtkApplicationWindow / GtkWindow) — `for_application`,
  `set_title`, `set_default_size`, `set_child`, `set_titlebar`, `present`,
  `close`, minimize/maximize/fullscreen.
- `widget`: `Widget` — the GtkWidget base knobs (the `view` + `layout` analog):
  show/hide, `set_sensitive`, `set_size_request`, focus, `queue_draw`, margins,
  `set_halign`/`set_valign`, `set_hexpand`/`set_vexpand`, tooltip, CSS classes,
  opacity. Wrap any widget via `Widget::from_raw(thing.raw())`.
- `controls`: `Button`, `Label`, `Entry`, `CheckButton`, `Switch`, `Scale`
  (slider), `SpinButton`, `ProgressBar`, `ToggleButton`, `DropDown`.
- `containers`: `Box`, `Grid`, `Stack` (+ `StackSwitcher`), `Notebook`,
  `Paned`, `ScrolledWindow`, `Frame`, `CenterBox`, `ListBox`, `HeaderBar`.
- `text`: `TextView` (+ `TextBuffer`), `SearchEntry`, `PasswordEntry`.
- `dialogs`: `AlertDialog` and `FileDialog` (the modern GTK 4.10+ objects).
- `graphics`: `Image` (themed/icon), `Picture` (scaled file content),
  `DrawingArea` (a canvas painted via `cairo`), and a `Color` (GdkRGBA) value.
- `cairo`: the 2D paint primitives used inside a `DrawingArea` draw function —
  move/line/rect/arc, source color, fill/stroke/paint. The `BezierPath` analog.
- `menu`: `Menu` (a `GMenu` model), `MenuButton`, `PopoverMenu`. GTK 4 menus are
  data that reference actions by name.
- `actions`: `Action` (GSimpleAction) — the named commands menu items invoke.
- `data`: model-backed list/table views (appkit's `data` analog) — `StringList`
  (a `GListModel`), `SingleSelection`, `ListItem`, `ListItemFactory`
  (`SignalListItemFactory` with `setup`/`bind` callbacks), `ListView`, and
  `ColumnView` (+ `ColumnViewColumn`). Re-exported from the umbrella.

The cross-cutting modules below are **import-directly** (no umbrella re-export),
matching appkit's `convert`/`events`/`drag` convention:

- `convert`: C+ ↔ C string/data bridges — `cstr_to_str_unsafe` / `cstr_to_text`
  / `str_to_cstring` (NUL-terminated copy for any `const char*` param),
  `string_object_text`, and `GBytes` ↔ `vec::Vec[u8]`. Requires `stdlib`.
- `events`: input event controllers (appkit's `events` analog) — `ClickGesture`
  (`pressed`/`released`), `KeyController` (`key-pressed` + GDK keysym
  constants), `MotionController` (`motion`/`enter`). Attach with
  `events::add_controller(widget, controller)`.
- `pasteboard`: `Clipboard` (GdkClipboard) — `for_widget`, `set_text`, and the
  async `read_text` / `read_text_finish`.
- `drag`: drag-and-drop — `DropTarget` (accept dropped text/values, with
  `string_from_value`) and `DragSource`, plus the `gtype_*` / `action_*`
  constants.
- `notifications`: `Notification` (GNotification) — desktop notifications sent
  through the application (`notifications::send(app, id, n)`).

## Signals & callbacks

GTK callbacks are plain C function pointers, so handlers are closure-free C+
functions. The dominant shape is `fn(emitter: *u8, user_data: *u8)`
(button "clicked", switch "toggled", range "value-changed", …); a few signals
take a different shape (`signal_connect3` for `(emitter, arg1, user_data)`,
`signal_connect_state` for the switch "state-set"). Thread app state to a
handler through the signal's `user_data` pointer, an associated object
(`glib::set_data` / `get_data`), or a global.

```cplus
import "gtk/gtk" as gtk;
import "gtk/glib" as glib;

fn on_clicked(button: *u8, _data: *u8) {
    let b = gtk::Button::from_raw(button);
    b.set_label(#str_ptr("Clicked!\0"));
}

fn on_activate(app: *u8, _data: *u8) {
    let win = gtk::Window::for_application(app);
    win.set_title(#str_ptr("C+ GTK\0"));
    win.set_default_size(360 as i32, 240 as i32);

    let vbox = gtk::Box::new(glib::orientation_vertical(), 12 as i32);
    gtk::Widget::from_raw(vbox.raw()).set_margin(16 as i32);

    let label = gtk::Label::new(#str_ptr("Hello from C+ via GTK 4\0"));
    vbox.append(label.raw());

    let btn = gtk::Button::new(#str_ptr("Press me\0"));
    btn.connect_clicked(on_clicked);
    vbox.append(btn.raw());

    win.set_child(vbox.raw());
    win.present();
}

fn main() -> i32 {
    let app = gtk::Application::new(#str_ptr("org.example.Hello\0"));
    app.connect_activate(on_activate);
    let status = app.run();
    app.unref();
    return status;
}
```

## Ownership

GObject widgets use **floating references**: a freshly-created widget owns one
floating ref, and the container it is added to (`Box::append`,
`Window::set_child`, …) *sinks* that ref and takes over the lifetime. So the
common case needs no manual reference counting — add a widget to its parent and
forget it.

Accordingly every widget wrapper here holds an `opaque obj: *u8` with **no
`drop`** (the same conservative default as appkit's child widgets) and exposes
`raw()` (the pointer, for packing into containers) plus `from_raw(*u8)` (to wrap
a pointer a signal handed you). The objects you *do* own a full reference to —
`Application`, a `Menu`/`Action` model, an `AlertDialog`/`FileDialog` — expose an
explicit `unref()`; call it once you are done (for `Application`, after `run()`).

Do not pass `Widget::new().raw()` inline into `append` and let the temporary
wrapper drop — hold the wrapper in a local across the call, exactly as in the
AppKit bindings.
