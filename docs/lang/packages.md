# Packages and platforms — how a project grows

The decision this page settles: **how code is organized past one file, and
how one project targets more than one OS.** Manifest keys in table form are
in [ref.md](ref.md); this page is the model.

## 1. Modules are files; imports are paths

Every `.cplus` file is a module. There is no module declaration — the file's
place on disk is its identity, and an import names it by path:

```cplus
import "stdlib/io" as io;        // module `io` in the dependency `stdlib`
import "./catalog" as catalog;   // file catalog.cplus next to this one
import "stdlib/str" as _;        // discard alias: extension methods only
```

The alias is mandatory and is the only name the import introduces —
`io::println`, `catalog::model()`. `as _` pulls in a module for its
extension methods (the blessed `impl str` block) without binding a name.

**Privacy is the underscore.** Items, fields, and methods are public by
default; a leading `_` makes them module-private (E0403 across modules).
There is no `pub` keyword and no visibility tree — one character, one rule.
Extensions of another package's types apply only where that module is
imported.

A file compiles only if some import reaches it from the entry — an orphan
`.cplus` under `src/` is a warning naming exactly that.

## 2. The manifest

`Cplus.toml`, and only `[package]` is required:

```toml
[package]
name    = "myapp"
version = "0.0.1"
edition = "2026"

[dependencies]        # the portable tier — resolved at vendor/<name>/
stdlib = "*"
facet  = "*"

[macos.dependencies]  # platform-scoped: exists only on this platform
facet_appkit = "*"
```

Dependency resolution is **flat and deliberate**: the resolver validates
every import in the build against this one manifest — it does not read a
dependency's own manifest. A backend's transitive closure is therefore named
here too. Clear and noisy beats magic: the manifest is the complete bill of
materials.

Dependencies are directories under `vendor/`. `cpc pm` fetches tree-URL
specs (`https://…/tree/main/vendor/stdlib@0.0.27`) into `vendor/<name>`;
local work symlinks them.

## 3. Apps: the entry names you, the target shapes you

A package with an **entry** is an application. Three ways to have one:

```toml
[package]
entry = "src/main.cplus"       # explicit — or omit it: src/main.cplus is the default

[ios]
entry = "src/main_ios.cplus"   # platform override
[ios.dependencies]
facet_uikit = "*"
```

**What a build produces is the target's fact, not the manifest's:**

| Platform class | Platforms | `cpc build` produces | Entry shape |
|---|---|---|---|
| self-linked | macos, linux, windows | an executable | `fn main() -> i32` (E0414 if missing) |
| external-builder | ios, android, esp32 | `lib<name>.a` + C header | `export extern fn` the platform shell calls (`fn main` is E0409) |

One source tree, each platform its own path to an artifact:
`cpc build` on the Mac links the binary; `cpc build --target
ios-arm64-simulator` stops at the archive and Xcode owns the link.
Cross-target artifacts land in `target/<target-name>/<mode>/`.

**Declared platform entries scope the app.** The moment any `[<platform>]
entry` exists, the `src/main.cplus` default stops applying elsewhere —
building for a platform you didn't name is E0413, never a silent guess. An
iOS-only app is exactly two manifest lines and a clean error everywhere
else.

## 4. Libraries: no entry, no section

A package with no entry is a library. Its consumers compile it from source;
`cpc build` inside it archives the whole `src/` tree (every module, so the
archive and the generated headers can never disagree). `stdlib` is just
`[package]` and nothing else.

Two special cases earn keys:

- **`[library]`** — a C-ABI *product*: `kind = "staticlib" | "cdylib" |
  "both"`, and an optional `entry` whose top-level names become the bare C
  symbols (that entry's import tree *is* the library). This is for shipping
  to C consumers; a C+-consumed library never needs it.
- **`[build] prebuild = true`** — compile once, reuse the archive. The
  first consumer build produces `lib/<triple>/<name>.a` + `lib/include/`
  headers and later builds link instead of recompiling. `dev = true` is the
  escape hatch back to always-from-source.

## 5. The link surface

An app's own linker inputs live in `[link]`:

```toml
[link]
frameworks    = ["Metal", "Foundation"]   # -framework X (Apple platforms)
libs          = ["objc", "z"]             # -lX
search-paths  = ["/usr/local/cuda/lib64"] # -L + rpath; ${VAR} expansion allowed
extra-objects = ["shaders.o"]             # prebuilt .o appended to the link
```

A *dependency's* `[link]` travels automatically: depending on `metal` is
enough to get `-framework Metal`. The dep walk validates
manifest-versus-filesystem for bundled binaries (a declared file that is
missing, or an undeclared one present, is an error — the manifest is truth).

## 6. Platform-variant code

Two mechanisms, two axes — they compose and don't compete:

- **File override** (compile-time, import-level): a sibling
  `<module>_<platform>.cplus` shadows `<module>.cplus` when building for
  that platform — same public surface, different implementation. This is
  the only way to vary *imports* per platform (kqueue vs epoll, AppKit vs
  UIKit), because C+ has no in-source `#if`: the unit of platform variation
  is the whole file.
- **`#platform()`** (value-level): a `str` that is the active target's
  platform name — `"macos"`, `"ios"`, `"linux"`, `"android"`, `"windows"`,
  `"esp32"`, `"wasm"` — the same vocabulary the manifest sections use. Both
  branches of an `if` on it still compile, so it can pick a padding or a
  port, never an import.

Off-platform imports fail honestly: importing a `[macos.dependencies]`
package in an iOS build is E0866 naming the platform it was declared for.

The rule of thumb from the framework work: OS decides *files* (backends,
syscalls), form factor decides *values* (a phone-shaped shell is portable
facet code — pick it at runtime, not per-OS).

## 7. Tests

`cpc test` discovers `#[test]` functions across the resolved import tree
and runs them; the entry is resolved by a ladder, first match wins:

1. `src/test_main.cplus` — a dedicated test root (imports the surface the
   suite should cover); by convention, no manifest key needed
2. the app entry for the current platform
3. the `[library]` target
4. `src/<package-name>.cplus` — the root module, for plain library packages

House discipline: every module ships unit, e2e, and negative tests; a
package is testable from its own directory with `cd <pkg> && cpc test`.

## 8. The commands that read all this

```bash
cpc build                        # this platform's artifact
cpc build --target ios-arm64     # that platform's artifact
cpc check                        # whole-project front-end, no codegen
cpc test                         # the ladder above
cpc graph / query / mcp          # the resolved code graph (use it over grep)
cpc explain E0413                # any code, offline
```
