# appkit_hello — GUI app via vendor/appkit

A window with a centered label and a "Quit" button. Click the button to
exit the application. Demonstrates two things at once:

1. The typed `vendor/appkit` API surface — `Application`, `Window`,
   `View`, `TextField`, `Button` — instead of hand-coded `extern fn
   objc_msgSend` boilerplate.
2. The Phase 2B `appkit/convert` bridge module — used to write a
   dynamically-built `string` value into the label's text field, and
   to read it back via `string_value()` to verify round-trip.

## Build + run

```bash
cpc build
./target/debug/appkit_hello
```

A window labelled "C+ AppKit Demo" appears. Click **Quit** to exit.

The recipe relies on `vendor/appkit` and `vendor/stdlib` being
symlinked into the project's `vendor/` directory — the same model as
every other v0.0.6 recipe.

## File map

```
appkit_hello/
├── Cplus.toml          package metadata
├── README.md           this file
└── src/
    └── main.cplus      ~60 lines of typed AppKit + bridge usage
```

## What you won't find here

- **No raw `extern fn objc_msgSend`** — the binding package handles
  all of that. The recipe is pure C+, no FFI shims declared.
- **No manual `NSString.stringWithUTF8String:`** — the bridge module
  exposes `cplus_string_to_nsstring(s)` and `nsstring_to_cplus_string(ns)`.
- **No counter state** — C+ has no closures and no top-level mutable.
  Adding a click counter would require the `objc_setAssociatedObject`
  trick from `vendor/appkit/src/runtime.cplus`. The recipe stays
  minimal and reaches for that escape hatch only if a follow-on slice
  needs it.

## Architectural notes

- `Application::run()` enters the AppKit event loop. The Quit button's
  callback calls `Application::terminate(0 as *u8)`, which unwinds the
  loop cleanly. The recipe never reaches `pool.drain()` in the happy
  path — AppKit terminates the process before that line executes.
  This matches `proves/benchmark/programs/03-hello-appkit`'s shape.
- The label's text is set via the bridge to demonstrate the dynamic
  string case. For purely literal labels, `set_string_value(str_ptr("hi\0"))`
  is the simpler path — the binding package wraps the literal via
  `rt::ns_string()` internally.
