# C+ → Cocoa / ObjC interop notes

Status: exploration, not committed work. Parked for later. Starting point: [hello_appkit_c.c](hello_appkit_c.c) — a self-contained AppKit "hello world" that drives the ObjC runtime entirely through its C API (no `.m`, no ARC, no `@` syntax). The premise here is "could C+ wrap this in a package and become a serious desktop app language on macOS?" Answer: yes, with ~2.5 days of compiler work.

## What the C file proves

You don't need ObjC syntax in the language. The ObjC runtime exposes a pure-C ABI:

- `objc_getClass(name) -> Class` — string → class lookup
- `sel_registerName(name) -> SEL` — string → selector lookup
- `objc_msgSend(receiver, selector, ...)` — the universal dispatch, type-erased at the symbol level
- `objc_allocateClassPair / class_addMethod / objc_registerClassPair` — build subclasses at runtime (used to make a `CAppDelegate` so window-close quits the app)

Every method call in [hello_appkit_c.c](hello_appkit_c.c) is one cast of `objc_msgSend` to the specific signature for that call site, then an immediate call. The C file is "ugly" because every selector repeats the full `((retty (*)(id, SEL, ...))objc_msgSend)(...)` cast dance — but that ugliness is mechanical and lives entirely inside a wrapper layer.

## Coverage from Phase 10 (already shipped)

- `extern fn` declarations — covers `objc_getClass`, `sel_registerName`, `class_addMethod`, etc.
- `*u8` raw pointer — collapses `id`, `SEL`, `Class`, `IMP` into one type at the boundary
- `unsafe { ... }` — gates every msgSend call site
- `#[repr(C)] struct` — covers `NSPoint`, `NSSize`, `NSRect` and any other by-value aggregates the ObjC runtime expects
- Varargs (`...`) — present but **the wrong tool here**; see below

## Compiler gaps blocking a clean wrap

### Gap 1 — `#[link_name = "..."]` attribute on extern fn

The whole trick of the C file is that **one symbol (`objc_msgSend`) is declared under many different typed signatures**, one per call shape. C does this via function-pointer casts. Rust does it via `#[link_name]`:

```cplus
#[link_name = "objc_msgSend"]
extern fn msg_void_id(receiver: *u8, sel: *u8, arg: *u8);

#[link_name = "objc_msgSend"]
extern fn msg_id_id(receiver: *u8, sel: *u8) -> *u8;

#[link_name = "objc_msgSend"]
extern fn msg_init_window(
    receiver: *u8, sel: *u8,
    frame: NSRect, mask: usize, backing: usize, defer: i8,
) -> *u8;
```

All three resolve to the same `_objc_msgSend` linker symbol; each lets the compiler emit a correctly-ABI'd call for that shape.

**Work:** new attribute in `attrs.rs`, plumb a `link_name: Option<String>` through `FnSig`, codegen emits `declare ... @<link_name>` and the call uses `@<link_name>` instead of `@<fn_name>`. ~half day.

### Gap 2 — function pointer values

`class_addMethod` takes an `IMP` (a function pointer) so the ObjC runtime can call your C+ function as a method body. To pass `app_should_terminate_after_last_window_closed` as `IMP`, C+ needs:

1. A function-pointer **type** — e.g. `fn(*u8, *u8, *u8) -> i8`
2. A function-pointer **value** — coerce a named C+ `fn` to that type (effectively "address-of function")
3. Stable C ABI for any function used this way — the ObjC runtime calls it directly; layout/calling convention must match `extern "C"`

**Work:** new `TypeKind::FnPtr(params, ret)`, new `ExprKind::FnRef(name)` or implicit coercion at call boundaries, sema typing, codegen as just the LLVM function pointer (which is already `ptr` post-opaque-pointers). ~1 day.

### Non-gap: do not use varargs for `objc_msgSend`

Tempting shortcut: `extern fn objc_msgSend(recv: *u8, sel: *u8, ...) -> *u8;` — one declaration, varargs handles the rest. **Don't.** On Apple platforms (and most ABIs) the variadic and fixed-arity calling conventions differ in:

- Which registers floats go in (sometimes spilled to integer regs for variadic)
- Struct-by-value rules (NSRect arguments would be passed differently)
- Whether `objc_msgSend_stret` is needed for struct returns

Apple's own headers declare `objc_msgSend` with **no prototype** so each call site picks its own ABI based on the cast. We mirror that via `#[link_name]` + per-shape typed declarations.

## Target surface — what user-facing C+ looks like

The cast dance lives inside the wrapper. User code looks like normal C+:

```cplus
import cocoa::{NSApp, NSWindow, NSTextField, NSRect, NSPoint, NSSize};

fn main() -> i32 {
    let app = NSApp::shared();
    app.set_activation_policy(ActivationPolicy::Regular);

    let frame = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size:   NSSize { width: 800.0, height: 500.0 },
    };

    let window = NSWindow::new(frame, WindowStyle::default(), BackingStore::Buffered);
    window.set_title("Hello from C+");
    window.center();

    let label = NSTextField::label(frame);
    label.set_string_value("Hello world");
    label.set_alignment(TextAlignment::Center);
    label.set_font(NSFont::bold_system(36.0));

    window.content_view().add_subview(label);
    window.make_key_and_order_front();

    app.activate();
    app.run();
    return 0;
}
```

Inside the `cocoa` package, `NSWindow::set_title` expands to something like:

```cplus
#[link_name = "objc_msgSend"]
extern fn msg_set_title(recv: *u8, sel: *u8, title: *u8);

static SEL_SET_TITLE: *u8 = sel_registerName(c"setTitle:");  // future: lazy static

impl NSWindow {
    fn set_title(self, title: str) {
        let ns_title: *u8 = ns_string(title);
        unsafe { msg_set_title(self.obj, SEL_SET_TITLE, ns_title); }
    }
}
```

## Quality-of-life items (not blockers)

- **`static` globals** for caching Class/SEL pointers (today every wrapper would re-lookup via `sel_registerName` on each call). Phase 11 polish item; today's workaround is a per-call `sel_registerName` which still works, just slower.
- **`const` at module scope** for things like `NSWindowStyleMaskTitled = 1 << 0`. Today: wrap as `fn ns_window_style_titled() -> usize { return 1 as usize; }`.
- **C string literals** (`c"setTitle:"` → null-terminated `*u8`) — today's `str_ptr` returns a pointer to a length-prefixed view, **not** null-terminated. ObjC selectors need null-terminated. Workaround: write `"setTitle:\0"` and use `str_ptr`, which works because string literals are stored in-memory contiguously but is fragile. Should add proper C string literals as a Phase 11 item.
- **Auto-derive `Class`/`SEL` caching** via a macro or `static` initializers once `static` lands.
- **`objc_msgSend_stret`** for struct returns ≥ 16 bytes on some ABIs (NSRect-returning getters). Not needed for the hello-world but real Cocoa wrappers will hit this.

## Effort estimate to first running C+ Cocoa hello-world

| Slice | Effort | Notes |
|---|---|---|
| `#[link_name]` attribute | ~0.5 day | Small attribute + codegen plumbing |
| Function pointer values | ~1 day | New type, value form, codegen |
| Hand-write `cocoa-min` package | ~1 day | NSApp, NSWindow, NSTextField, NSAutoreleasePool, just enough to reproduce [hello_appkit_c.c](hello_appkit_c.c) |
| **Total** | **~2.5 days** | |

After that, the gating items for *production-grade* desktop apps are:

- `static` globals (perf — selector caching)
- C string literals (correctness — null termination)
- `objc_msgSend_stret` (correctness — struct returns)
- Reference counting story — autorelease pool today; longer-term decide whether C+ codegen emits `objc_retain`/`objc_release` automatically for `*ObjC` types (basically ARC-in-C+) or leaves it manual
- Block/closure support (`^{ ... }`) — only matters once you hit Cocoa APIs that take blocks

## Open questions to revisit

1. **Should C+ have a dedicated `*ObjC<T>` type** that auto-emits retain/release, or stay with raw `*u8` + a `cocoa::Id<T>` wrapper struct that uses `Drop` to release? The latter is more uniform with the rest of the language.
2. **Selector caching via `static`** — do we want a `selector!("setTitle:")` macro/builtin that monomorphizes to a cached lookup, or just a manual `lazy_static` pattern?
3. **Block trampolines** — C+'s normal `fn` won't satisfy ObjC block layout. Need a separate `objc_block!` builtin or accept that block-taking APIs can't be wrapped from C+ alone (call them from a thin `.m` shim).
4. **Cross-platform story** — is `cocoa-min` the canary, or do we want a portability layer (Cocoa / win32 / GTK behind one trait)? Probably out of scope for the initial demo.
5. **AppKit vs UIKit/Catalyst** — same runtime, same ABI, so the wrapping pattern carries over. Worth noting but not blocking.

## Reference material

- [hello_appkit_c.c](hello_appkit_c.c) — the reference implementation in plain C
- Apple: `<objc/objc.h>`, `<objc/runtime.h>`, `<objc/message.h>`
- Rust's `objc` and `cocoa` crates — same trick, just with Rust's `extern` + `link_name` instead of C casts
