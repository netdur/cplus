# Guide

How the shared ObjC runtime is organized and how to stay out of ABI trouble.
Tutorial: [tutorial.md](tutorial.md). Catalog: [ref.md](ref.md).

## Role

`objc` is infrastructure for **Apple-platform C+**, not an app UI kit.

| Module | Role |
|---|---|
| `objc/runtime` | msgSend zoo, get_class/sel, retain/release, Range/Rect/Point/Size, blocks isa |
| `objc/bridge` | `str`/`Text` ↔ NSString, Text arrays, nil → Option |
| `objc/synthesis` | allocateClassPair, addMethod IMPs, associated objects |
| `objc/objc` | package facade (`Range` re-export); bindings usually import submodules |

Generated wrappers (AppKit, …) call `runtime` + `bridge` so each framework
does not re-declare `objc_msgSend`.

## Messaging model

Every method send is:

```text
objc_msgSend(receiver, selector, args...)
```

C+ needs a **typed prototype per ABI shape** (return class + argument classes).
`runtime` exposes thin wrappers:

```cplus
msg_id(recv, sel)           // id return, no args beyond self/_cmd
msg_void_id(recv, sel, a)   // void, one id arg
msg_rect(recv, sel)         // NSRect-by-value return
// … dozens more
```

If a selector’s signature is missing, add a new `objc_msg_*` extern + `msg_*`
wrapper (same pattern as existing ones). Arm64 HFAs: `Rect`/`Point`/`Size` pass
by value in fp registers, matching `NSRect`/`NSPoint`/`NSSize`.

## Ownership

Hand-rolled ARC:

- **`retain` / `release` / `autorelease`** map to libobjc.
- Generated bindings typically own +1 on create and `release` in Drop.
- Bridge `nsstring` produces autoreleased Foundation objects (pool-friendly).
- `to_text` **copies** UTF-8 out of NSString so the `Text` outlives the pool.

Nil:

- `bridge::to_text_option(nil)` → `None`
- `bridge::obj_option(nil)` → `None`

## String bridge details

- Encoding: **UTF-8** (`NSUTF8StringEncoding = 4`).
- Empty `str` / empty `Text` / `(NULL, 0)` views: routed through a valid empty
  pointer so `stringWithBytes:` never sees NULL.
- `nsarray_of_text`: builds autoreleased `NSMutableArray` of NSStrings.

## Synthesis (delegates / targets)

No ObjC source required for simple delegates:

1. `allocate_class_pair(superclass, name, 0)`
2. `add_method_*` for each IMP signature (void / id / i64 / … × arity)
3. `register_class_pair`
4. `alloc_init_class` instance
5. Stash C+ context with `set_associated` or `retain_associated`

IMPs are C functions `(self, _cmd, …)`. Read context via `get_associated`.
`class_responds` / `object_class` support `respondsToSelector:` overrides so
optional protocol methods can fall through to framework defaults.

Used heavily by `appkit` / `facet_appkit` target-action and data sources.

## Blocks

`stack_block_isa()` resolves `_NSConcreteStackBlock` via `dlsym`. Generated
code builds stack block structs for `NS_NOESCAPE` callbacks; this package
supplies the shared isa/descriptor helpers, not a full Block_copy API.

## Geometry

| Type | Fields | ABI |
|---|---|---|
| `Range` | location, length (`u64`) | NSRange |
| `Rect` | x, y, w, h (`f64`) | NSRect HFA |
| `Point` | x, y | NSPoint |
| `Size` | w, h | NSSize |

Documented for **arm64 macOS**; x86_64 is not a support target for this stack.

## Gotchas

- **Wrong `msg_*` shape** → register corruption / crash. Match the ObjC
  signature exactly (including f32 vs f64).
- **Selector strings must be NUL-terminated** for `sel_registerName`.
- **Do not use bridge empty-string bugs** — empty is fixed; still avoid dangling
  pointers into freed `Text` after mutation/realloc.
- **Synthesis names** must be unique process-wide (`objc_allocateClassPair`).
- **Associated ASSIGN** does not retain the context — you keep it alive.
- App code should prefer **typed framework packages** over raw `msg_*` soup.

## Consumers

- Generated / hand bindings: AppKit, UIKit-ish, Metal, QuartzCore, …
- Facet AppKit backend and agent AppKit surface
- Any package that must invent a one-off ObjC class in pure C+
