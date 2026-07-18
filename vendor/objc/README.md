# objc

Shared **Objective-C runtime** for C+ on Apple: `objc_msgSend` shims, retain/
release, geometry value types, string bridge, and runtime class synthesis for
delegates/targets.

```toml
[dependencies]
objc = "*"
```

```cplus
import "objc/runtime" as rt;
import "objc/bridge" as bridge;

let cls: *u8 = rt::get_class(#str_ptr("NSString\0"));
let ns: *u8 = bridge::nsstring("hello");
let t: text::Text = bridge::to_text(ns);
```

Framework packages (`appkit`, `facet_appkit`, Metal bindings, …) depend on this
instead of re-declaring the runtime.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — common calls
- [docs/guide.md](docs/guide.md) — ownership, ABI, synthesis
- [docs/ref.md](docs/ref.md) — modules and API map

## Tests

```
cd vendor/objc && cpc test
```
