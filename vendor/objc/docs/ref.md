# Reference

Manual for the `objc` package. Messaging overloads are numerous — listed by
family, not every arity.

```cplus
import "objc/runtime" as rt;      // or import "objc/runtime" as objc in some codebases
import "objc/bridge" as bridge;
import "objc/synthesis" as synth;
import "objc/objc" as objc;       // facade: type Range = runtime::Range
```

Links: `frameworks = ["Foundation"]`, `libs = ["objc"]`.

---

## `runtime` — types

```cplus
struct Range  { location: u64, length: u64 }
struct Rect   { x: f64, y: f64, w: f64, h: f64 }
struct Point  { x: f64, y: f64 }
struct Size   { w: f64, h: f64 }
struct BlockDescriptor { reserved: u64, size: u64 }
```

```cplus
fn stack_block_isa() -> *u8
```

---

## `runtime` — lookup & lifetime

```cplus
fn get_class(name: *u8) -> *u8      // objc_getClass
fn class_name(obj: *u8) -> *u8
fn sel(name: *u8) -> *u8            // sel_registerName
fn retain(obj: *u8) -> *u8
fn release(obj: *u8)
fn autorelease(obj: *u8) -> *u8
fn alloc_init(class_name: *u8) -> *u8   // +alloc / -init by name
```

---

## `runtime` — messaging families

Naming: `msg_<return>[_<arg>…](recv, selector, …)`.

| Family | Examples |
|---|---|
| id / void | `msg_id`, `msg_id_id`, `msg_void`, `msg_void_id`, multi-id |
| integers | `msg_i8`, `msg_i32`, `msg_i64`, `msg_u32`, `msg_u64`, mixes |
| floats | `msg_f32`, `msg_f64`, void/id combinations |
| Range | `msg_range`, `msg_range_u64`, `msg_id_range`, … |
| geometry | `msg_rect`, `msg_void_rect`, `msg_id_rect`, `msg_size`, `msg_point`, … |
| mixed | `msg_void_id_u64`, `msg_void_id_id_i8`, … |

Add new shims only when a binding needs a signature not already present.
Underlying symbol is always `objc_msgSend` (`#[link_name]`).

---

## `bridge`

```cplus
fn ns_utf8() -> u64                          // 4
fn nsstring(s: str) -> *u8                   // NSString (autoreleased copy)
fn to_text(ns: *u8) -> text::Text            // owned UTF-8 Text
fn to_text_option(ns: *u8) -> option::Option[text::Text]
fn nsarray_of_text(items: vec::Vec[text::Text]) -> *u8
fn obj_option(p: *u8) -> option::Option[*u8] // nil → None
```

---

## `bundle`

```cplus
fn main_bundle() -> *u8                      // [NSBundle mainBundle], or null
fn has_bundle_id() -> bool                   // bundleIdentifier != nil
fn url_is_bundle(url: *u8) -> bool           // does this file URL name a .app?
fn has_bundle_proxy() -> bool                // both, cached — ASK THIS ONE
```

`has_bundle_proxy` is the question to ask before touching any bundle-scoped
framework. `+[UNUserNotificationCenter currentNotificationCenter]` **raises**
for a process that has no bundle proxy, `respondsToSelector:` does not save you
(the selector is there — it is the process that is wrong), and a `@try` cannot
catch it either: the raise happens inside a `dispatch_once` block and libdispatch
terminates on any exception crossing it.

It is **not** `bundleIdentifier != nil`, which is the trap this module exists to
close. Measured 2026-09-09, one probe built four ways:

| process | `bundleIdentifier` | `url_is_bundle` | centre |
|---|---|---|---|
| bare, no section | nil | false | **raise** |
| bare + `__TEXT,__info_plist` | set | false | **raise** |
| a real `.app` | set | true | ok |
| a `.app` with no `CFBundleIdentifier` | nil | true | **raise** |

Rows 2 and 4 are why the predicate is an **and**: `cpc build` embeds a project's
`macos/Info.plist` into `__TEXT,__info_plist`, which gives a bare binary an
identifier and no bundle. `bundleURL` is the value the exception itself prints,
and the `.app` extension is the public stand-in for the private
`bundleProxyForCurrentProcess`. `.appex` is deliberately refused — it has a
proxy, but C+ cannot build one, so the row is untested.

The answer is cached: a process cannot acquire a bundle while it runs, and the
callers ask on a hot path. `url_is_bundle` is separate so the discriminator is
testable — the test process is always row 1.

Callers: `permissions`, `notifications`.

---

## `synthesis`

```cplus
fn allocate_class_pair(superclass: *u8, name: *u8, extra_bytes: usize) -> *u8
fn register_class_pair(cls: *u8)
fn alloc_init_class(cls: *u8) -> *u8

// class_addMethod wrappers — void return:
fn add_method_v_0id … add_method_v_5id
// id return:
fn add_method_id_0id … add_method_id_4id
// i64 / u64 / i16 / i8 returns with various arities:
fn add_method_i64_* / add_method_u64_* / add_method_i16_0id / add_method_i8_*

fn set_associated(object, key, value)          // ASSIGN
fn retain_associated(object, key, value)       // RETAIN_NONATOMIC
fn get_associated(object, key) -> *u8

fn object_class(obj: *u8) -> *u8
fn class_responds(cls: *u8, sel: *u8) -> i8
```

Type encodings (`types: *u8`) are the ObjC runtime encoding strings for
`class_addMethod`.

---

## `objc` facade

```cplus
type Range = runtime::Range
```

Imports every submodule — `runtime`, `bridge`, `bundle`, `synthesis` — for the
package graph and for test discovery: `cpc test` walks from the package entry,
so a module nothing imports is a module whose tests never run.

---

## Package

| | |
|---|---|
| Name | `objc` |
| Platform | Apple (arm64 macOS focus) |
| Dependencies | `stdlib` |
| Link | Foundation framework, libobjc |
| Tests | `cpc test` (bridge string round-trips, bundle predicate rows, etc.) |
