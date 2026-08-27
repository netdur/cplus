# Platforms and targets — one source tree, several operating systems

The decision this page settles: **where a platform difference belongs.** C+
has no `#if`, no `cfg` attribute, and no conditional-compilation expression.
It has three mechanisms instead, and they are not interchangeable:

| The thing that differs | Mechanism | Chosen at |
|---|---|---|
| which **imports** a module needs (kqueue vs epoll, AppKit vs UIKit) | a `_<platform>.cplus` sibling file | import resolution |
| which **dependency** the project pulls in | `[<platform>.dependencies]` in the manifest | dependency resolution |
| a **value** inside otherwise portable code (a padding, a path, a port) | `#platform()` / `#arch()` / `#target()` | type checking, as a `str` constant |

The rule that decides between them: **the OS decides files; the form factor
decides values.** A backend, a syscall, a framework binding is a file. A
phone-shaped layout is portable code choosing a number.

Manifest shapes are in [packages.md](packages.md); exact signatures in
[ref.md](ref.md); the normative rules in [spec.md](spec.md) §3.1 and §19.

## 1. The platform vocabulary

Seven names, used identically by the file suffix, the manifest section, and
`#platform()`:

```
macos   linux   windows   ios   android   esp32   wasm
```

These are **OS families, not targets**. Several targets can share one
platform: `ios-arm64` and `ios-arm64-simulator` are both `ios`.

The targets `--target` accepts:

| `--target` | Platform | Arch | Handoff | Default OS floor |
|---|---|---|---|---|
| `host` (default) | the compiler host's | the host's | cpc links | — |
| `ios-arm64` | `ios` | aarch64 | external builder | 13.0 |
| `ios-arm64-simulator` | `ios` | aarch64 | external builder | 13.0 |
| `android-arm64` | `android` | aarch64 | external builder | API 24 |
| `esp32-xtensa` | `esp32` | xtensa | external builder | — |
| `esp32c3-riscv32` | `esp32` | riscv32 | external builder | — |

`--min-os VERSION` (written *after* `--target`) replaces the version token
in the target's triple. A target whose triple carries no version rejects it.

`wasm` is a platform name the manifest and `#platform()` accept; it is
reached through `cpc-wasm`, not through `--target`.

## 2. File overrides: `<module>_<platform>.cplus`

A sibling file whose stem is `<module>_<platform>` **shadows**
`<module>.cplus` when the active target's platform is `<platform>`:

```
src/
  reactor.cplus            # the base — Darwin kqueue
  reactor_linux.cplus      # chosen when building for linux
  reactor_windows.cplus    # chosen when building for windows
```

Every importer writes the base name and never thinks about it again:

```cplus
import "./reactor" as reactor;      // resolves to reactor_linux.cplus on Linux
```

Five facts that decide how you use this:

- **The variants must present the same public surface.** Nothing checks
  this. The importing file is compiled against exactly one of them, so a
  name that exists in only one variant is a clean error on that platform
  and silence everywhere else. The suite that catches it is a build per
  platform, not a check on one.
- **The base file is optional.** `import "./backend"` resolves with only
  `backend_macos.cplus` and `backend_linux.cplus` on disk — there need be
  no `backend.cplus` at all. The base, when present, is the fallback for
  every platform that has no variant.
- **A platform with neither a variant nor a base is E0401**, and the
  message names the *base* path (`.../backend.cplus`), because that is what
  the import resolved to before the override was tried. Read "file not
  found" on a cross build as "this module has no body for this platform".
- **The suffix comes from the target, not the host.** `--target ios-arm64`
  on a Mac looks for `_ios`, never `_macos`.
- **A file already carrying the active suffix is never re-suffixed** —
  `reactor_linux.cplus` does not look for `reactor_linux_linux.cplus`.

### Android falls back to `_linux`

Android's kernel is Linux: the same `/proc`, the same `epoll`, the same
`environ`. So an `android` build tries **two** suffixes, in order:

1. `_android` — bionic genuinely differs here, and a real file wins.
2. `_linux` — a module that already has a correct Linux body needs nothing
   more.

This fallback is load-bearing, not a convenience. Without it a module with
only a `_linux` variant resolves on Android to its **Darwin base**, which
compiles, links, and then fails at load naming `kqueue` and `sysctlbyname`
in an app that never called either. Every other platform tries exactly one
suffix.

### Orphan detection exempts them (W0005)

A `.cplus` file under `src/` that no import reaches is W0005: it never
compiles, so nothing it claims is checked — and an agent reading it takes
it for the live API. Platform-suffixed files are exempt, because being
unreachable on this target is the whole point of them. The trade-off is
real: a genuinely dead `helper_linux.cplus` never warns. Delete variants
when you delete a platform.

### Library builds pick exactly one variant

A package with no entry archives its whole `src/` tree, and the archive
must contain the same set of modules an app build resolves — one file per
module, never two. So the synthesized package entry imports **base names
only**, letting the resolver pick the active variant, and keeps a
suffixed module *only* when it has no base file and it is the variant this
target would select. (`src/test_main.cplus` is excluded outright: a
package's tests are not part of what it ships.)

"The variant this target would select" is the resolver's own answer, asked
through one function — not a rule the archive re-derives. It used to be:
the sweep compared against a single active suffix while the resolver walks
an ordered list, so a module existing only as `foo_linux.cplus` was reached
by an app build on `android` (through the fallback) and silently missing
from a library archive built for the same target. Two answers to one
question, differing on exactly one platform.

## 3. `#platform()`, `#arch()`, `#target()`

Three `str` constants, resolved at check time from the selected target:

```cplus
let p: str = #platform();   // "macos" "linux" "windows" "ios" "android" "esp32" "wasm"
let a: str = #arch();       // "aarch64" "x86_64" "xtensa" "riscv32" "wasm32"
let t: str = #target();     // "host" "ios-arm64" "ios-arm64-simulator" "android-arm64" …
```

They are **value-level only**. Both arms of an `if #platform() == "ios"`
are compiled on every platform, which is exactly why they cannot hide an
import a platform does not have — that is the file override's job. Use them
for a number, a string, a branch over portable code:

```cplus
let inset: i32 = if #platform() == "ios" { 44 } else { 0 };
```

Which of the three to reach for:

- `#platform()` — almost always. It is the axis the manifest and the file
  suffix already use.
- `#arch()` **crosses** `#platform()` rather than refining it: `macos` and
  `ios` are both `aarch64` on Apple silicon. Use it for lane widths,
  alignment, and instruction-set choices.
- `#target()` is the only axis that separates the **iOS simulator from an
  iOS device** — both are platform `ios` and arch `aarch64`. Reach for it
  when, and only when, that distinction is the question. On a plain host
  build it is the literal `"host"`, not the host's triple.

## 4. Platform sections in the manifest

```toml
[dependencies]          # portable tier — every platform
stdlib = "*"
facet  = "*"

[macos.dependencies]    # exists only on macos
facet_appkit = "*"

[ios]                   # per-platform entry override
entry = "src/main_ios.cplus"
[ios.dependencies]
facet_uikit = "*"
```

- Importing a package declared for another platform is **E0866**, and the
  message names the platforms it *was* declared for. That is the failure
  you want: it points at the manifest line, not at a missing symbol at link
  time.
- Platform section names are validated. A misspelled `[macoss]` is
  **E0406** — a manifest parse error listing the seven legal names, not a
  silently-ignored table. The same holds inside a platform section: `entry`
  and `dependencies` are the whole namespace, and anything else is E0406.
- **Declaring any `[<platform>] entry` scopes the app.** The
  `src/main.cplus` default stops applying, and building for a platform you
  did not name is E0413. An iOS-only app is two manifest lines and a clean
  error everywhere else.

## 5. What a build produces, per platform

Not a manifest choice — a fact about the target:

| Class | Platforms | `cpc build` produces | Entry shape |
|---|---|---|---|
| self-linked | macos, linux, windows | an executable | `fn main() -> i32` — E0414 if missing |
| external builder | ios, android, esp32 | `lib<name>.a` + a C header | `export extern fn` — `fn main` is E0409 |

On an external-builder target cpc stops at the archive and the platform's
own build system (Xcode, the Android NDK build, ESP-IDF) owns the final
link. Two commands make that handoff workable:

```bash
cpc build --target ios-arm64            # target/ios-arm64/debug/lib<name>.a + <name>.h
cpc build --print-link-args             # what the DEPENDENCIES add to the link line
```

`--print-link-args` prints one argument per line and builds nothing. It is
what you paste into Xcode's "Other Linker Flags" or a Gradle rule: the app's
own archive is not enough, because every dependency's `[link]` frameworks
and libs travel with it.

Cross-target artifacts land under `target/<target-name>/<mode>/`, so a host
build and an iOS build coexist without a clean.

## 6. Where the toolchain comes from

- **iOS** reuses the host clang family on macOS, with `-isysroot` from
  `xcrun --sdk`.
- **Android** uses the NDK's clang: `$ANDROID_NDK_HOME`, else the newest
  `ndk/` under the SDK. r28.2 or newer.
- **ESP32** uses esp-clang: `$CPC_ESP_CLANG`, else `~/.espressif`
  (`idf_tools.py install esp-clang`).

Build an Android shared library with `-Wl,--no-undefined`. Without it a
missing symbol is not a link error — it is a `dlopen` failure at app
start, naming a function the app never called.

## 7. Targets that lack part of the stdlib

The ESP32 targets are 32-bit and have no OS underneath. `usize`, `isize`,
and pointers are 4 bytes there, the heap types (`Text`, `Vec`) are not
supported yet, and importing any of

```
thread  mutex  channel  env  net  netsys  reactor  executor  time  fs
```

is **E0866** at resolve time rather than a failure deep in the IR. `async
fn` on a 32-bit target is **E0867** — coroutine frames are a 64-bit
feature today.

The iOS and Android targets exclude nothing: the whole stdlib is available
there.

## 8. The checklist for adding a platform to an existing project

1. `cpc pm add . <backend-package> --platform <p>` — or write the
   `[<p>.dependencies]` table yourself, naming the **whole** closure: the
   resolver validates imports against this one manifest and never reads a
   dependency's own.
2. Add `[<p>] entry = "src/main_<p>.cplus"` if the platform needs its own
   entry — and remember this scopes every *other* platform out (E0413)
   until they are named too.
3. Split the modules that cannot share a body into `_<p>.cplus` siblings.
   Keep the base file as the portable fallback where one exists.
4. `cpc build --target <t>` and read the first error literally: E0401 means
   "no body for this platform", E0866 means "the manifest did not declare
   this dependency here", E0413 means "this app is scoped and you are not
   in the list".
5. Build every platform in CI. A file override is checked only by
   compiling for the platform it belongs to.

## 9. Gotchas

- **A `_<platform>.cplus` file is invisible on every other platform.** It
  is not type-checked, not borrow-checked, and does not show up in the
  code graph for that build. Treat "it compiles on my Mac" as saying
  nothing about the `_linux` sibling.
- **`#platform()` cannot gate an import**, and no amount of `if` around it
  changes that — imports are file-leading and resolved before any
  expression is checked.
- **The iOS simulator is not a platform.** `[ios.dependencies]` covers
  both; only `#target()` tells them apart.
- **`#platform()` is the target's, not the host's.** A `--target
  ios-arm64` build from a Mac sees `"ios"`. Code that logs "running on
  macos" from a cross build is reading the wrong axis.
- **Adding a platform-scoped dependency does not add it to the portable
  tier.** Code in a shared module cannot import it at all; the import
  belongs in that platform's own file.
