# gobject_smoke — vendor/gobject against real GTK

A headless link-and-runtime check for [`vendor/gobject`](../../../../vendor/gobject),
the shared GObject runtime for the GTK stack. It constructs a real
`GtkApplication` (forcing a link against `libgtk-4`) and exercises the whole
runtime surface without running the main loop, so it completes with no display —
usable as a smoke test in CI or on a headless box.

What it verifies, each printed as `PASS`/`FAIL`:

- **`signal`** — connecting `activate` through `gobject/signal::connect`, i.e. a
  C+ `fn(*u8, *u8)` marshalled as a C-ABI `GCallback` and accepted by
  `g_signal_connect_data`.
- **`runtime` type identity** — `is_a` safe downcasts (`GtkApplication`,
  `GObject`, and a negative `GtkButton`).
- **`runtime` lifetime** — `object_is_floating` (a `GApplication` is ref-counted,
  not floating) and `object_unref`.
- **`runtime` per-instance data** — `set_data`/`get_data` round-trip.
- **`bridge`** — a transfer-full `g_strdup` result copied into a `Text` and
  `g_free`d, via `bridge::cstr_to_text_full`.

## Build + run

```bash
cpc build
./target/debug/gobject_smoke
```

Requires GTK 4 development files (`libgtk-4-dev` on Debian/Ubuntu); the
`gobject-2.0` / `glib-2.0` link libs arrive transitively from the `gobject`
dependency, so this package only adds `gtk-4` under `[link] libs`.
