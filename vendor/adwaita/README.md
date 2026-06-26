# adwaita

C+ bindings for **libadwaita 1** — GNOME's companion library to GTK 4 (adaptive
widgets, the GNOME HIG look, boxed-list preferences, in-app toasts, and
light/dark recoloring).

This is a thin layer **on top of `vendor/gtk`**, not a replacement. Every
Adwaita object is a GObject, and the key types subclass their GTK counterparts
(`AdwApplication ⊂ GtkApplication`, `AdwApplicationWindow ⊂ GtkApplicationWindow`),
so these bindings **reuse all of `vendor/gtk`** through raw pointers and only add
the `Adw*` widgets. You mix the two freely: put a `gtk::Button` in an
`adwaita::HeaderBar`, a `gtk::Box` inside an `adwaita::Clamp`, etc.

## Why a separate package

libadwaita is its own shared library and an *optional, opinionated* layer (the
GNOME look; extra dependencies). A pure-GTK app — or one targeting a non-GNOME
desktop — should not be forced to link it. Since `Cplus.toml`'s `[link]` is
package-wide, Adwaita lives in its own package that **depends on `gtk`** and adds
only `libadwaita-1` to the link line (the GTK stack comes transitively).

## Building

Install libadwaita (Debian/Ubuntu: `sudo apt install libadwaita-1-dev`), then:

```
cpc build
```

## Usage

```toml
[dependencies]
adwaita = "*"
gtk     = "*"
stdlib  = "*"
```

```cplus
import "adwaita/adwaita" as adw;   // Adw widgets
```

Or import narrower modules directly (`import "adwaita/rows" as rows;`).

The base GTK widget modules (`gtk/gtk`, `gtk/glib`, …) that these containers
hold are not part of the current trimmed `vendor/gtk` (only `gtk/convert`
ships there yet); the small set of GObject / GtkApplication / GtkWindow symbols
Adwaita itself delegates to is bound internally in `src/glue.cplus`. Window
geometry/lifecycle conveniences (`set_default_size`, `present`, `close`) are
exposed directly on the `Window` wrapper, so an Adwaita-only app does not need
the GTK base modules.

## Modules

- `application`: `Application` (AdwApplication) — initializes libadwaita and runs
  the GTK main loop (delegated to the GtkApplication base). `new`,
  `connect_activate`, `run`, `quit`, `add_action`, `set_menubar`, `unref`.
- `window`: `Window` (AdwApplicationWindow) and `PlainWindow` (AdwWindow) — use
  `set_content` (not `set_child`); host a `ToolbarView`. The common GtkWindow
  geometry/lifecycle calls (`set_title`, `set_default_size`, `present`, `close`)
  are surfaced directly on the wrapper.
- `header`: `HeaderBar` (AdwHeaderBar), `WindowTitle` (title + subtitle), and
  `ToolbarView` (the modern top-bars + content + bottom-bars scaffold).
- `view`: `Clamp` (constrain content to a readable width), `Bin` (single-child
  host), `StatusPage` (full-page empty/placeholder state).
- `toast`: `Toast` + `ToastOverlay` — transient in-app notifications
  ("snackbars") with an optional action button.
- `rows`: the boxed-list settings widgets — `ActionRow`, `EntryRow`,
  `SwitchRow`, `ComboRow`, `ExpanderRow`, and the `PreferencesGroup` →
  `PreferencesPage` → `PreferencesWindow` hierarchy.
- `viewstack`: `ViewStack` + `ViewSwitcher` — paged content with a GNOME-style
  switcher.
- `style`: `StyleManager` — runtime color-scheme control (light/dark) and the
  `color_scheme_*` constants; `is_dark` + "notify::dark".
- `widgets`: `Avatar`, `ButtonContent` (icon+label for buttons), `SplitButton`.

## Example

```cplus
import "adwaita/adwaita" as adw;

fn on_activate(app: *u8, _d: *u8) {
    let win = adw::Window::for_application(app);
    win.set_default_size(420 as i32, 360 as i32);

    let tv = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(
        adw::WindowTitle::new(#str_ptr("Demo\0"), #str_ptr("\0")).raw());
    tv.add_top_bar(header.raw());

    let group = adw::PreferencesGroup::new();
    let row = adw::SwitchRow::new();
    row.set_title(#str_ptr("Enabled\0"));
    row.set_active(1 as i32);
    group.add(row.raw());

    let clamp = adw::Clamp::new();
    clamp.set_child(group.raw());
    tv.set_content(clamp.raw());
    win.set_content(tv.raw());
    win.present();
}

fn main() -> i32 {
    let app = adw::Application::new(#str_ptr("org.example.Demo\0"));
    app.connect_activate(on_activate);
    let status = app.run();
    app.unref();
    return status;
}
```

## Ownership

Same model as `vendor/gtk`: widget wrappers are `opaque` borrowed handles (the
parent container sinks the floating reference on add), so no `drop`. Only
`Application` owns a full reference — `unref()` after `run()`. Note
`ToastOverlay::add_toast` takes ownership of the toast, and the AdwApplication
owns its windows. See `vendor/gtk/README.md` for the full discussion.
