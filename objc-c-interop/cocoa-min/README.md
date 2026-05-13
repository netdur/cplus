# `cocoa-min` — C+ Cocoa hello-world

The first real macOS desktop app written in pure C+ source. Reproduces
[../hello_appkit_c.c](../hello_appkit_c.c) — opens an 800×500 window titled
"Hello from C+" with a centered bold "Hello world" label, quits the app
when the window closes.

## What this proves

All four Phase 11 ObjC-interop slices working together, end-to-end:

| Primitive | Used for |
|---|---|
| `extern fn` + raw pointers + `unsafe` (Phase 10) | The ObjC runtime C API: `objc_getClass`, `sel_registerName`, `class_addMethod`, etc. |
| `#[repr(C)] struct` (Phase 10) | `NSPoint`, `NSSize`, `NSRect` passed by value across the FFI boundary. |
| `#[link_name = "objc_msgSend"]` (11.LINKNAME) | The 10 typed aliases of `objc_msgSend` — each call site picks its own ABI shape. |
| `0 as *u8` in `unsafe` (11.INTPTR) | NULL sender args, e.g. `makeKeyAndOrderFront:nil`. |
| `fn(*u8, *u8, *u8) -> i8` (11.FN_PTR) | The `IMP` function pointer for `class_addMethod`. The C+ delegate method coerces to fn-pointer at the type-directed call site. |
| `size_of[T]()` (11.LAYOUT) | Not used in this hello-world, but available for any struct allocation needs. |

No compiler-blessed `cocoa` type. No special-case codegen. Everything is
user-level C+ source built on the Phase 10/11 primitives.

## Build

```sh
./build.sh
```

Emits `hello_appkit.ll` (LLVM IR via `cpc --emit-ll`) and links against
`-framework Cocoa -lobjc` via `clang`. The compiler doesn't know about
Apple frameworks; we delegate to `clang`'s linker.

## Run

```sh
./hello_appkit
```

A window appears with "Hello world" in bold. Close the window → the
app quits (via the runtime-built `CAppDelegate` we registered).

## What this is NOT

- **Not a Cocoa SDK.** No NSString helpers, no Auto Layout bindings, no
  NSColor / NSImage / IB integration. Just enough to draw a labeled window.
- **Not a UI framework.** The next layer (a real `cocoa` package) would
  wrap each ObjC class in a typed C+ struct so user code looks like:
  ```
  let window = NSWindow::new(frame, WindowStyle::default());
  window.set_title("Hello");
  window.center();
  ```
  Inside each method, the `objc_msgSend` dance lives unchanged. That's
  ordinary library work — no further compiler changes needed.
- **Not yet a sample auto-test.** Running a GUI app from `cargo test`
  needs special sandbox handling on macOS. The build is verified
  by the build script; the binary is invoked manually.

## Files

- `hello_appkit.cplus` — the C+ source (~250 lines)
- `build.sh` — compile via `cpc --emit-ll` + link via `clang`
- `.gitignore` — exclude build artifacts
