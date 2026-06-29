# cpc-bindgen

Generates C+ bindings from a C or Objective-C header, or a Swift module. For
C/ObjC it shells out to `clang` to dump the header's AST as JSON; for Swift
(`--swift`) it reads a `swift symbolgraph-extract` graph. Either way it emits C+
source to stdout, and constructs it cannot model are written as `// SKIPPED`
comments, never as wrong code, each naming its reason.

Requires `clang` on `PATH`. Objective-C and Swift modes also need the Xcode
command-line tools (`xcrun` for the SDK path / `swift symbolgraph-extract`).

## Build

```
cargo build --release -p cpc-bindgen
# binary: target/release/cpc-bindgen
```

## Usage

```
cpc-bindgen [--objc] [--prefix P] [--overrides FILE] <header.h> [-- <clang args>...]
```

Output goes to stdout: redirect it into a `.cplus` file. Everything after `--`
is forwarded to `clang` verbatim (`-I`, `-D`, `-F`, `-isysroot`, framework
paths, and so on).

### C headers (default)

```
cpc-bindgen path/to/header.h -- -I/usr/local/include > bindings.cplus
```

Emits `extern fn` declarations, `#[repr(C)]` structs, typedefs, and enum
constants for the C API.

### Objective-C headers (`--objc`)

```
cpc-bindgen --objc --prefix NS \
  "$(xcrun --show-sdk-path)/System/Library/Frameworks/Foundation.framework/Headers/NSTimeZone.h" \
  > timezone.cplus
```

Classes become wrapper structs over an ObjC handle, with ARC-correct
construction and a `drop` that releases. The SDK sysroot is resolved
automatically via `xcrun --show-sdk-path`; pass your own `-isysroot ...` after
`--` to override it.

### Whole frameworks (`--framework`)

```
cpc-bindgen --framework NaturalLanguage --prefix NL --overrides overrides.json
```

Generates an entire package from an Apple system framework in one step, instead
of one header at a time. It reads the framework's umbrella header to discover the
public headers, emits one binding module per header, and writes the package
skeleton the single-header mode leaves to you:

- `src/<module>.cplus` per header (mechanical snake_case names),
- `src/<pkg>.cplus`, the umbrella that imports the modules and re-exports their types,
- `Cplus.toml`, populated from the framework metadata (name, `[link]` frameworks,
  `stdlib`/`objc` deps) with a provenance header recording the framework, SDK
  version, generator version, header count, and the exact reproduce command,
- a starter `overrides.json` if you did not pass one.

`overrides.json` stays a hand-authored input (the curated names); everything
mechanical is regenerated. Output goes to `--out DIR` (default: the lowercased
framework name).

#### `--merge` — one module per framework

By default a framework emits one module per header. C+ forbids cyclic module
imports, and ObjC type graphs are cyclic (device → queue → buffer → device), so
across the per-header layout an object return resolves only to a *method-less
stub* of the sibling type — you get a named handle but cannot chain a call on it
without a manual `.raw()` / `from_raw` bridge.

`--merge` parses the framework umbrella once and emits the whole framework as a
single `src/<pkg>.cplus`. Every wrapper type is then co-resident, so object
returns/args resolve to the *full* type and method chaining works with no
bridging — the hand-written ergonomics, generated:

```
cpc-bindgen --framework Metal --prefix MTL --merge --out vendor/metal
# device.new_command_queue().command_buffer()  — full types, no .raw() dance
```

Regeneration is byte-stable (deterministic). The merged module is typically
*smaller* than the sum of the per-header modules (no duplicated stubs / enum
converters). Single-header and non-merge output is unchanged.

### Swift modules (`--swift`)

```
# bind a pre-extracted symbol graph
cpc-bindgen --swift CoreAIRuntime.symbols.json

# or extract it first (args after -- go to symbolgraph-extract)
cpc-bindgen --swift-module CoreAIRuntime -- \
  -target arm64-apple-macos27.0 -sdk "$SDK" -F "$SDK/System/Library/SubFrameworks"
```

Reads the JSON from `swift symbolgraph-extract` — the documented, stable
description of a Swift module's public API (the Swift analog of clang's
`-ast-dump=json`).

Unlike Objective-C, Swift has **no universal dynamic entry point** like
`objc_msgSend`: methods use the Swift calling convention with mangled names, and
value types, generics, `async`, `throws`, and move-only (`~Copyable`) types have
no C ABI. `--swift` therefore binds only the subset that already has a guaranteed
C ABI — raw-value enums and functions marked `@_cdecl` / `@convention(c)` — and
writes `// SKIPPED <path>: <reason>` for everything else. For a pure-Swift
framework that is an all-SKIP manifest; use `--swift-bridge` to actually bind it.

### Swift bridges (`--swift-bridge`)

```
cpc-bindgen --swift-bridge --swift-module CoreAIRuntime --out coreai --link CoreAI \
  --bridge-spec coreai.json -- -target arm64-apple-macos27.0 -sdk "$SDK" -F "$SDK/.../SubFrameworks"
```

Instead of skipping, *generate* the C ABI. For each bindable symbol this emits
two artifacts in lockstep — a `@_cdecl` Swift thunk (into
`bridge/<Module>Bridge.swift`) and the matching C+ `extern fn` + ergonomic
wrapper (into `src/<module>.cplus`) — plus a `<module>_bridge.h`, a `build.sh`
that compiles the Swift into a dylib, and a `Cplus.toml` that links it. The C+
side owns opaque handles (`+1`/`drop` via `Unmanaged`); the Swift side owns the
real values.

What it binds: reference/value types and enums as opaque handles (boxed in a
Swift class); scalars; `String` params/returns (`Text`); `throws` (→ error
channel + `Option`/`Result`); `async` (a blocking bridge); `T?` and failable
`init?` (→ `Option`); `[scalar]` params (slices) and `[scalar]`/`[String]`
property getters (→ `Vec`); scalar/String property get/set. Constructs the graph
can't decide (copyability, raw-enum-ness, generic instantiations) are supplied by
`--bridge-spec`; everything still undecidable is `// SKIPPED` with its reason.

`--bridge-spec FILE` is a JSON object of human facts the symbol graph can't
provide:

- `copyable`: value types that are `Copyable` (unlocks handle property getters).
- `raw_enums`: integer-`RawValue` enums (bind as `i64` + per-case constants).
- `enum_cases`: raw-value-less enums (handle + a constructor per case).
- `noncopyable_owners`: `~Copyable` types (each member read becomes a consuming
  `take_<member>()`).
- `view_copy`: `{ "NDArray": ["Float", ...] }` — bulk-copy a `~Escapable` element
  view into a caller buffer (`<Type>_copy_<elem>`).
- `instantiate`: `{ "NDArray.init(scalars:shape:)": ["Float", ...] }` — emit one
  binding per concrete element type for a generic method.

Requires an SDK shipping the framework (e.g. CoreAI needs Xcode 27+); select it
with `DEVELOPER_DIR=/Applications/Xcode-beta.app/Contents/Developer`.

## Flags

- `--objc`: Objective-C mode. Without it the input is treated as C.
- `--swift <Module.symbols.json>`: bind the C-ABI subset of a Swift graph (the
  all-SKIP classifier).
- `--swift-bridge`: generate a compiled `@_cdecl` Swift bridge + C+ bindings for
  the whole bindable surface. Pairs with `--swift-module`/`--swift` + `--out DIR`,
  `--link <Framework>`, and `--bridge-spec FILE`.
- `--bridge-spec FILE`: JSON of human-supplied facts (see above).
- `--swift-module <Name>`: run `swift symbolgraph-extract` for `Name` first;
  pass `-target`/`-sdk`/`-F` after `--`.
- `--prefix P`: strip a class-name prefix from emitted type names. `--prefix NS`
  turns `NSTimeZone` into `TimeZone`.
- `--overrides FILE`: a JSON file of naming overrides (see below).
- `--framework <Name>`: generate a whole package from an Apple system framework
  (see above); implies Objective-C, no single header needed.
- `--merge`: with `--framework`, emit the whole framework as one module so object
  returns/args are full types and method chaining works (see above).
- `--out DIR`: output directory for `--framework` (default: lowercased framework name).
- `-- <clang args>`: everything after `--` is passed to clang.

## Generated output depends on the `objc` runtime

ObjC bindings call into the hand-written bridge. Each emitted file imports
`objc/runtime` (`rt::`, the typed `objc_msgSend` shims) and `objc/bridge`
(`bridge::`, the `Text` / `Vec` / `Option` converters), plus the stdlib modules
it uses (`stdlib/text`, `stdlib/option`, `stdlib/vec`, `stdlib/string_map`). A
package that consumes generated bindings must declare `objc` and `stdlib` as
dependencies.

## Overrides

Generated names are mechanical snake_case. An overrides file replaces them where
the SDK metadata cannot supply a good name. It is JSON with three optional keys:

```json
{
  "types":   { "NSTimeZone": "TimeZone" },
  "methods": {
    "NSXMLParserDelegate": {
      "parser:didStartElement:namespaceURI:qualifiedName:attributes:": { "name": "did_start_element" }
    }
  },
  "skip": [ "someUnwantedSelector:" ]
}
```

- `types`: rename an Objective-C type to a C+ name.
- `methods`: rename a method, keyed by class name, or by *protocol* name for
  delegate / data-source callbacks. The value is `{ "name": "<c+ name>" }`.
- `skip`: names to omit entirely.

Any other key (for example `_comment`) is ignored. `--framework` writes a starter
`overrides.json` (in `--out DIR`) if you don't pass one.

## What the Objective-C mode covers

- **Classes & protocols as wrapper structs.** An `@interface` and a *non-delegate*
  `@protocol` (MTLDevice, MTLBuffer — the whole callable API surface) both become a
  struct over an ObjC handle with one method per method. A `…Delegate` /
  `…DataSource` protocol instead becomes a runtime class-synthesis helper.
- **Init / factory constructors.** A nullable factory becomes `Option[Self]`; ARC
  ownership with a `drop` that releases (owning wrappers); non-owning handles are
  `opaque` with no drop.
- **Typed object wrappers.** An object return/arg (`id<MTLBuffer>`, `MTLFoo *`)
  resolves to its wrapper type, not `*u8` — gated on the wrapper being defined in
  the module. Foreign-framework types (NSURL, NSError) and `NSError**` out-params
  correctly stay `*u8`. With `--merge` these are *full* types (chaining works).
- **By-value C structs** (`typedef struct { … } MTLSize;`). Emitted as a
  `#[repr(C)]` struct plus a module-local `objc_msgSend` shim per call shape;
  passed/returned by value (the 24-byte `MTLSize` sret case is ABI-verified against
  clang and runtime-verified). Scalar / nested-struct / typedef-chained fields.
- **`NSArray` ↔ `Vec`, both directions.** `NSArray<NSString*>` ↔ `Vec[Text]`;
  `NSArray<NSValue*>` → `Vec[Range]`; `NSArray<id<P>>` ↔ `Vec[P]` (return wraps each
  element, param builds an `NSMutableArray`) for non-owning *protocol* elements.
- **Scalars & geometry.** `str`/`Text`, nullable → `Option`, `NS_ENUM` / `NS_OPTIONS`,
  `BOOL`, `NSInteger`/`NSUInteger` (i64/u64), `f64` (double/CGFloat/NSTimeInterval),
  32-bit `int`/`unsigned`/`float` (i32/u32/f32), `NSRange`, `NSRect`/`NSPoint`/`NSSize`
  (HFA), `MTLResourceID`/`MTLGPUAddress` (u64).
- **String-keyed `NSDictionary` *returns*** → `StringMap` (NSString / NSNumber values).
- Any-arity selectors, categories, blocks (`usingBlock:`), delegate/data-source
  protocols (void and value-returning callbacks, multi-method, override-named),
  C+-keyword-safe names (the full lexer keyword set is escaped, e.g. `trait`).

## Remaining skips (TODO)

The generator never emits wrong code: anything it can't model becomes a
`// SKIPPED <selector>: <reason>` comment naming the gap. As a reference, a full
`Metal --merge` regen leaves ~201 skips, in these buckets (largest first). The
skip comment names the exact shape, so `grep '// SKIPPED'` over a generated module
is the work-list.

1. **`msgSend shape <sig> not yet modelled`** (~103, the long tail; ~63 distinct
   shapes → poor ROI per shim). A `(return, arg)` ABI signature with no typed
   `objc_msgSend` shim. Fix: add the shim + wrapper in `vendor/objc/src/runtime.cplus`
   and its tag to `KNOWN` in `send_expr` (see "Extending" below). Shapes that involve
   a by-value struct are emitted as module-local shims automatically.
2. **`param <T> — unmapped type`** (~39): mostly completion-handler **block params**
   (`MTLNew*CompletionHandler`, needs block-arg modelling — hard) and **enum-field
   value structs** (`MTLTextureSwizzleChannels`: `struct_field_type` needs the enum's
   exact backing width from clang's `fixedUnderlyingType` to keep the `#[repr(C)]`
   ABI-correct), plus odds like `dispatch_queue_t`.
3. **`return`/`param … generic collection`** (~35): **class-element arrays**
   (`NSArray<MTLFoo*>`, ~22). Only non-owning *protocol*-element arrays are bound so
   far; class elements need a `non_owning_types` set (protocols ∪ interfaces with no
   `init`) — an owning wrapper in a `Vec` would over-release the +0 elements. The rest
   are nested/element-typed collections (`NSArray<NSArray<…>>`) and `NSDictionary`.
4. **`return <T> — unmapped type`** (~6) and **NSDictionary** *params* + non
   String/Number dictionary values (~4): `NSDictionary` returns are done; params and
   exotic value types are not.
5. **`method <name> already defined`** (~8, unfixable): two ObjC selectors collapse to
   one C+ name (`-open` / `-open:`); C+ has no overloading, so the second is skipped.
6. **block methods with a non-void return / extra init variants** (a few): niche shapes.

## Examples in the tree

Generated packages carry an `# Auto-generated by cpc-bindgen` provenance header
(with the exact reproduce command) and a `// Auto-generated` header per module.
Live ones:

- `vendor/metal/` — `--framework Metal --merge` (single-module ObjC: typed wrappers,
  full-type chaining, value structs, object arrays; ~201 skips, see above).
- `vendor/appkit/` — `--framework AppKit` (per-header ObjC, 280 modules).
- `vendor/accelerate/` — `--framework Accelerate` (a pure-C framework via the C path).

## Extending the generator

The ObjC emitter is `src/objc.rs`. Value/return type mappings live in `map_ret` /
`map_arg` / `param_sig_type`; the `(return, arg)` ABI shape and its wire form are
computed by the free fns `msg_shape` / `arg_tag` / `arg_expr` and dispatched in
`send_expr`.

- **New scalar `objc_msgSend` shapes** are added in lockstep at two places: the typed
  shim + wrapper in `vendor/objc/src/runtime.cplus`, and the matching tag in the
  `KNOWN` list (`msg_shape_is_known`). These clear the `msgSend shape … not yet
  modelled` skips.
- **By-value struct shapes** need no manual shim: any shape with a PascalCase tag is
  emitted as a module-local `objc_msg_*` extern alongside the `#[repr(C)]` struct
  (`render_value_structs` / `render_struct_shims`). To bind more value structs, widen
  `struct_field_type` (it must keep the layout exact-or-absent, never wrong).
- **Typed-wrapper / array gating.** `known_types` (pre-registered in Pass 2a) decides
  whether an object return/arg is typed vs `*u8`. `protocol_types` gates which
  `NSArray<id<P>>` and object-array params/returns become `Vec[P]` (protocols are
  always non-owning). Extending arrays to class elements means adding a
  `non_owning_types` set (see Remaining skips #3).
- **Delegate callback shapes** are curated in `vendor/objc/src/synthesis.cplus` and
  gated by `delegate_shape_known`.

Every new mapping should keep the invariant: emit correct code or a `// SKIPPED`
comment, never wrong code. The C-ABI is the hard constraint — verify by-value
struct and msgSend shapes against `clang -S` / `-emit-llvm`, and ideally a runtime
round-trip, not just `cpc check`.
