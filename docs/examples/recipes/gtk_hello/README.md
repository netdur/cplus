# gtk_hello — GUI app via vendor/gtk

A GTK 4 window with a header bar, a greeting label, a text entry, a **Greet**
button that copies the entry text into the label, an **Enabled** switch, and a
**Volume** slider whose value is mirrored into a second label. The Linux
counterpart to `appkit_hello`. Demonstrates:

1. The typed `vendor/gtk` API surface — `Application`, `Window`, `HeaderBar`,
   `Box`, `Label`, `Entry`, `Button`, `Switch`, `Scale` — instead of hand-coded
   `extern fn gtk_*` boilerplate.
2. GTK's closure-free signal model: handlers are plain C+ functions that reach
   the widgets they need through the signal's emitter pointer, its `user_data`
   argument, and associated data (`glib::set_data` / `get_data`).

## Build + run

Install GTK 4 first (Debian/Ubuntu: `sudo apt install libgtk-4-dev`), then:

```bash
cpc build
./target/debug/gtk_hello
```

A 420×320 window titled "C+ GTK Demo" appears. Type a name and click **Greet**
to update the label; drag the slider to update the volume readout.

The recipe relies on `vendor/gtk` and `vendor/stdlib` being resolvable as
dependencies — the same model as the other recipes.

## File map

```
gtk_hello/
├── Cplus.toml          package metadata + gtk/stdlib deps
├── README.md           this file
└── src/
    └── main.cplus      Application → activate → window + widgets + signals
```

## How it works

- `main` creates a `gtk::Application`, connects its `"activate"` signal, and
  calls `run()` (the GTK main loop). `unref()` after `run()` returns.
- `on_activate` builds the UI: a `Window::for_application(app)` (owned by the
  app), a `HeaderBar` titlebar, and a vertical `Box` of widgets set as the
  window's single child.
- The **Greet** button passes the `Entry` as its handler's `user_data`, and the
  greeting `Label` is stashed on the entry via `glib::set_data`, so `on_greet`
  can read the entry text and write it into the label without any globals.
- The **Volume** `Scale` passes its readout `Label` as `user_data`;
  `on_scale_changed` formats the value with `snprintf` into a stack buffer and
  sets the label text.

## Ownership

GTK widgets use floating references that their parent container sinks on
`append` / `set_child`, so the wrappers are borrowed handles with no `drop` —
add a widget to its parent and move on. Only `Application` owns a full
reference here; it is released with `app.unref()` after the loop ends. See
`vendor/gtk/README.md` for the full model.
