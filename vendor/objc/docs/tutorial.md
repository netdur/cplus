# Tutorial

Look up a class, send a message, convert strings. Details: [guide.md](guide.md).
API map: [ref.md](ref.md).

## Setup

```toml
[dependencies]
objc = "*"
stdlib = "*"
```

```cplus
import "objc/runtime" as rt;
import "objc/bridge" as bridge;
import "stdlib/text" as text;
import "stdlib/option" as option;
```

Links `Foundation` + `libobjc` (see package `Cplus.toml`).

## Class, selector, message

```cplus
let cls: *u8 = rt::get_class(#str_ptr("NSObject\0"));
let sel: *u8 = rt::sel(#str_ptr("description\0"));
// alloc/init helper for class *names*:
let obj: *u8 = rt::alloc_init(#str_ptr("NSMutableArray\0"));
rt::msg_void_id(obj, rt::sel(#str_ptr("addObject:\0")), other);
let desc: *u8 = rt::msg_id(obj, sel);
```

Pick the `msg_*` shape that matches the ObjC method’s return and argument ABI
(`msg_id`, `msg_void_id`, `msg_rect`, `msg_i64`, …).

## Strings

```cplus
let ns: *u8 = bridge::nsstring("hola");     // copies into NSString
let t: text::Text = bridge::to_text(ns);    // owned Text (UTF-8 copy out)
match bridge::to_text_option(maybe_ns) {
    option::Option[text::Text]::Some(s) => { /* ... */ }
    option::Option[text::Text]::None => { /* nil NSString */ }
}
```

## Lifetime

```cplus
let kept: *u8 = rt::retain(obj);   // +1
rt::release(kept);                 // -1
let _a: *u8 = rt::autorelease(obj);
```

## Day-one rules

- Object handles are **`*u8`** (type-erased `id`).
- Selector/class **names** for `sel` / `get_class` need **NUL-terminated** C
  strings (`#str_ptr("foo\0")`).
- `nsstring` / `to_text` hide encoding; empty / null-backed empty `str` is safe.
- Prefer generated framework bindings for app code; use this package when
  writing or extending those bindings.
