# gobject

Shared GObject runtime for the GTK stack on Linux/BSD — the structural analog of
[`vendor/objc`](../objc) for Cocoa. Generated *and* hand-written GTK / Adwaita
bindings depend on this for object lifetime, signal wiring, and string
conversion, instead of each re-declaring `g_object_ref` / `g_signal_connect` /
`g_type_*` inline.

## Why it looks different from `vendor/objc`

Objective-C funnels **every** method call through one dynamic entry point,
`objc_msgSend`, so `vendor/objc` is mostly a large zoo of typed `objc_msgSend`
shims (one per ABI shape). **GObject has no such entry point** — each method is
its own exported C symbol (`gtk_button_set_label`, `gtk_window_present`, …),
which a plain `extern fn` already models and the C binding path already emits. So
this package has **no message-shim zoo**. What it centralizes is the part raw
method calls *don't* give you:

| `vendor/objc` piece | here |
| --- | --- |
| `objc_msgSend` shim zoo | — (not needed; methods are direct symbols) |
| ARC `objc_retain`/`objc_release` | `runtime` — `object_ref` / `object_unref` / `object_ref_sink` (floating refs) |
| `objc_getClass` / `sel_registerName` | `runtime` — `type_from_name` / `is_a` (safe downcasts) |
| `synthesis` (delegate callbacks) | `signal` — `connect` / `connect_bool` (`g_signal_connect_data`) |
| `bridge` (NSString ↔ Text) | `bridge` — `gchar*` ↔ `Text`, transfer-none and transfer-full |

## Modules

- **`runtime`** — GObject lifetime and GType casts. Lifetime accounts for the one
  state ObjC lacks: a **floating reference**. Fresh `GtkWidget`s start floating;
  a container *sinks* the float when you add the widget. So:
  - handing a widget to a container: do nothing (the container sinks it);
  - keeping a widget yourself: `object_ref_sink` to claim it, `object_unref` when done;
  - a plain (non-floating) GObject: `object_ref` / `object_unref` as usual.

  Downcasts are checked: `is_a(instance, "GtkButton\0")` resolves the GType by
  name and tests membership before you treat the handle as that type. Also
  `set_data` / `get_data` (per-instance slots) and `free` (`g_free`, for
  transfer-full buffers).

- **`signal`** — connect a C handler to a named signal. A handler is a plain
  C-ABI function pointer: `(instance, …signal args…, user_data)`. Two shapes are
  provided — void-returning (`connect`) and gboolean-returning (`connect_bool`,
  return TRUE to halt propagation) — plus `disconnect`. Add a shape by aliasing
  `g_signal_connect_data` with the exact handler `fn(...)` type, exactly as
  `vendor/objc` adds `objc_msgSend` shapes.

- **`bridge`** — `const gchar*` ↔ `Text`/`str`, with GObject transfer semantics
  spelled out: `cstr_to_text` (transfer-none, borrow), `cstr_to_text_full`
  (transfer-full, copy then `g_free`), `cstr_to_str_unsafe` (borrowed view),
  `str_to_cstring` / `free_cstring` (allocate a NUL-terminated arg for a callee).

## Usage

```
import "gobject/runtime" as g;
import "gobject/signal" as sig;
import "gobject/bridge" as bridge;

fn on_clicked(widget: *u8, user: *u8) { /* ... */ return; }

// wire a button, keeping ownership of a widget we hold ourselves
sig::connect(button, #str_ptr("clicked\0"), on_clicked, { 0 as *u8 });
let owned: *u8 = g::object_ref_sink(my_widget);
// ... later ...
g::object_unref(owned);
```

Declare `gobject = "*"` in your `[dependencies]`. `libgobject-2.0` / `libglib-2.0`
land on the link line via this package's `[link]`.

## Relationship to `vendor/gtk`

`vendor/gtk/src/convert.cplus` is the seed of `bridge` (str ↔ C string) and is
superseded by it; GTK/Adwaita bindings and `agent_gtk` can migrate their
`convert::` uses to `gobject/bridge` and drop the inline `g_object_*` / `g_type_*`
externs in favor of `gobject/runtime`. Left non-breaking for now — this package
is purely additive.

## Status

Hand-written, `cpc check`-clean. This is the runtime foundation a future
`cpc-bindgen --gobject` (GIR-driven) generator would target — GTK/Adwaita
wrapper structs calling the direct method symbols while delegating lifetime,
signals, and strings here.

## Testing

    cd vendor/gobject && cpc test

19 tests, and they run against the **real** libgobject/libglib — a GObject is
constructed, reffed, sunk and unreffed; a signal is registered with
`g_signal_new`, connected through this module's `connect`, and emitted; a
`g_strdup` allocation goes through the transfer-full bridge and is freed with
`g_free`. Nothing here is mocked, because the failure this package is exposed to
is an `extern fn` whose declared shape disagrees with the symbol's, and a mock
agrees with the declaration by construction.

That means the suite needs a host with the GObject libraries — it will not run
on macOS. Until 2026-08-23 this package had no tests and had never been
executed at all: everything that depends on it (the generated GTK stack,
`facet_gtk`, `agent_gtk`) type-checks anywhere but links `libgtk-4`, so nothing
ran this code until a Linux host was available.
