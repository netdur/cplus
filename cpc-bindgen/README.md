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
no C ABI. So this mode binds only the subset that has a guaranteed C ABI —
raw-value enums (as named `i64` constant accessors) and functions marked
`@_cdecl` / `@convention(c)` — and writes `// SKIPPED <path>: <reason>` for
everything else, with a summary histogram of the reasons. Each skip names what a
hand-written `@_cdecl` Swift bridge would have to cover, so the output doubles as
the bridge spec. (For a pure-Swift framework like CoreAI the result is an
all-SKIP manifest: there is nothing C-callable to bind directly.)

## Flags

- `--objc`: Objective-C mode. Without it the input is treated as C.
- `--swift <Module.symbols.json>`: bind a Swift `symbolgraph-extract` JSON graph.
- `--swift-module <Name>`: run `swift symbolgraph-extract` for `Name` first;
  pass `-target`/`-sdk`/`-F` after `--`.
- `--prefix P`: strip a class-name prefix from emitted type names. `--prefix NS`
  turns `NSTimeZone` into `TimeZone`.
- `--overrides FILE`: a JSON file of naming overrides (see below).
- `--framework <Name>`: generate a whole package from an Apple system framework
  (see above); implies Objective-C, no single header needed.
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

Any other key (for example `_comment`) is ignored. See
`vendor/delegate_proof/overrides.json` and
`vendor/naturallanguage/overrides.json` for live examples.

## What the Objective-C mode covers

Classes, init / factory constructors (a nullable factory becomes
`Option[Self]`), ARC ownership, `str` / `Text`, nullable returns to `Option`,
`NS_ENUM`, `NSRange`, `NSArray` to and from `Vec` (both directions), string-keyed
`NSDictionary` *returns* to `StringMap`, `BOOL`, `f64`
(double / CGFloat / NSTimeInterval), 32-bit scalars
(`int` / `unsigned` / `float` to i32 / u32 / f32), any-arity selectors,
categories, blocks (`usingBlock:`), and delegate / data-source protocols (void
and value-returning callbacks, multi-method, override-named).

Not yet modelled: `NSDictionary` *parameters* (returns are done). Methods that
use them are emitted as `// SKIPPED`.

## Examples in the tree

Generated bindings carry a `// Auto-generated by cpc-bindgen` header. Live ones:
`vendor/foundation_gen/` (NSTimeZone / NSDate / NSScanner), `vendor/nl_gen/`
(NaturalLanguage tokenizer), and `vendor/delegate_proof/` (NSXMLParser delegate).

## Extending the generator

The ObjC emitter is `src/objc.rs`. New `objc_msgSend` ABI shapes are added in
lockstep at two places: the typed shim + wrapper in `vendor/objc/src/runtime.cplus`,
and the matching tag in the `KNOWN` list in `send_expr`. Value/return type
mappings live in `map_ret` / `map_arg` / `param_sig_type`. New delegate callback
shapes are curated in `vendor/objc/src/synthesis.cplus` and gated by
`delegate_shape_known`.
