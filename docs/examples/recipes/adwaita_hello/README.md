# adwaita_hello — GNOME-styled app via vendor/adwaita

A libadwaita app built on `vendor/gtk`: an `AdwApplicationWindow` with an
`AdwToolbarView` (header bar + content), a clamped `AdwPreferencesGroup`
containing an `AdwActionRow` (with an `AdwAvatar`) and an `AdwSwitchRow` that
toggles dark mode via `AdwStyleManager`, and a plain `gtk::Button` that raises
an `AdwToast`. Shows how Adwaita and GTK widgets compose through raw pointers.

## Build + run

Install libadwaita (Debian/Ubuntu: `sudo apt install libadwaita-1-dev`), then:

```bash
cpc build
./target/debug/adwaita_hello
```

A 440×400 GNOME-styled window appears. Toggle **Dark mode** to recolor the app;
click **Notify** to pop a toast.

## File map

```
adwaita_hello/
├── Cplus.toml          package metadata + adwaita/gtk/stdlib deps
├── README.md           this file
└── src/
    └── main.cplus      AdwApplication → activate → ToolbarView + rows + toast
```

## Notes

- `adwaita::Application` (AdwApplication) initializes libadwaita; everything
  else (signals, the main loop) is the GtkApplication base.
- Adwaita windows use `set_content` (one child, typically an `AdwToolbarView`),
  not `set_child`.
- The toast overlay is threaded to the button handler as `user_data`; the style
  manager is the app-wide singleton fetched on demand. Both keep the handlers
  closure-free, the same pattern as `gtk_hello`.
- `ToastOverlay::add_toast` takes ownership of the toast.
