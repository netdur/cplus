# C+ Programming Language

C+ is an experimental, pre-1.0 systems programming language and toolchain. It
compiles to native machine code through LLVM, uses explicit ownership and a
borrow checker without a garbage collector, speaks the C ABI in both
directions, and keeps high-level capabilities in ordinary vendored packages
instead of growing the core language.

Version **0.0.29** supports macOS, Windows, Linux, iOS, Android, ESP32, and
WebAssembly. Facet supplies native AppKit, UIKit, Android, GTK, and Win32 UI
backends.

**Website:** <https://cplus-lang.dev> ·
**Documentation:** <https://cplus-lang.dev/docs> ·
**Releases:** <https://github.com/netdur/cplus/releases> ·
**Changelog:** [changeslog.md](changeslog.md)

## What C+ is

- **Native systems programming.** C+ emits LLVM IR and uses clang to assemble
  and link native artifacts. There is no VM and no garbage collector.
- **Explicit ownership.** A parameter is a read-only borrow by default, `ref`
  writes back to the caller, and `take` transfers ownership. Owning values drop
  deterministically at the end of their scope.
- **A checked safe subset with an explicit raw tier.** The borrow checker
  enforces aliasing-xor-mutation and rejects uninitialized reads, invalid
  escapes, and data races. Raw-pointer dereferences, pointer casts, and foreign
  calls remain available and visible at their point of use; C+ does not claim
  that those operations are automatically safe.
- **A small, unambiguous language surface.** C+ has no exceptions, closures,
  function overloading, implicit numeric conversions, or source-rewriting
  macros. Structs, tagged enums, generics, interfaces, methods, modules, async
  functions, generators, and explicit error values cover the core model.
- **Package-defined high-level syntax.** Contextual builder blocks such as
  `@view { ... }` let packages provide declarative construction syntax without
  compiler plugins or hidden control flow.
- **Two-way C interoperability.** C+ calls C through `extern fn`, exports plain
  C symbols, emits C headers, and can be introduced one object file at a time
  inside an existing C or C++ build.
- **Compiler-checked real-time code.** `#[no_alloc]`, `#[no_block]`,
  `#[bounded_recursion]`, `#[max_stack(N)]`, and `#[realtime]` are checked over
  the call graph rather than treated as advisory lints.
- **Packages instead of compiler magic.** The standard library, SIMD, GPU,
  platform bindings, UI, application capabilities, and agent surfaces are
  regular C+ packages under [`vendor/`](vendor/).

## Platforms and artifacts

C+ uses seven platform names throughout source-file overrides, manifest
sections, and `#platform()`:

| Platform | Build path | Artifact or runtime | Facet backend |
|---|---|---|---|
| macOS | native host | executable | AppKit |
| Linux | native host | executable | GTK 4 |
| Windows | native host | executable | Win32 |
| iOS | `ios-arm64`, `ios-arm64-simulator` through Xcode | static library + C header for the platform shell | UIKit |
| Android | `android-arm64` through the Android NDK | static library + C header for the platform shell | Android |
| ESP32 | `esp32-xtensa`, `esp32c3-riscv32` through esp-clang / ESP-IDF | static library + C header for firmware integration | — |
| WebAssembly | `cpc-wasm` in the browser | full front-end; runnable WebAssembly for the current supported subset | — |

The native compiler runs on macOS, Linux, and Windows. Cross-target builds use
the target platform's own toolchain: Xcode for iOS, the Android NDK for
Android, and Espressif's esp-clang for ESP32. External-builder targets stop at
an archive and generated header; Xcode, the Android build, or ESP-IDF owns the
final link.

See [Platforms and targets](docs/lang/platforms.md) for target triples,
platform-specific source files, manifest sections, artifact locations, and
toolchain discovery.

## Install

### macOS / Apple Silicon

```sh
brew install netdur/cplus/cplus
```

The Homebrew package installs prebuilt `cpc`, `cpc-lsp`, and `cpc-bindgen`
binaries. Native builds require the Xcode Command Line Tools:

```sh
xcode-select --install
```

### Linux / x86-64

Download the `.deb` from the
[latest release](https://github.com/netdur/cplus/releases/latest), then install
it with apt so the clang dependency is resolved:

```sh
sudo apt install ./cplus_*_amd64.deb
```

### Windows / x86-64

Download `cplus-x86_64-pc-windows-msvc.zip` from the
[latest release](https://github.com/netdur/cplus/releases/latest) and put
`cpc.exe`, `cpc-lsp.exe`, and `cpc-bindgen.exe` on `PATH`. Native linking uses
clang and the MSVC toolchain.

The compiler front end, formatter, code graph, MCP server, package manifest
operations, and documentation tools do not need clang. Commands that emit,
link, or run native code do.

## Quick start

Create, check, build, and run a host command-line project:

```sh
cpc init hello
cd hello
cpc pm install
cpc check
cpc build
./target/debug/hello
```

The generated project has a `Cplus.toml` manifest and `src/main.cplus`. A
minimal standalone program is:

```cplus
fn main() -> i32 {
    #println("hello, world");
    return 0;
}
```

Compile a single import-free file directly with:

```sh
cpc hello.cplus -o hello
./hello
```

Scaffold a Facet application for a supported UI platform with `--kind gui`:

```sh
cpc init --kind gui --platform macos notes
```

`cpc init --help` documents platform-scoped entries and the files generated
for host, iOS, and Android projects.

## Projects and packages

A project is described by `Cplus.toml`. Modules are `.cplus` files, and every
import names its source and binds an alias:

```cplus
import "./catalog" as catalog;
import "stdlib/io" as io;
import "stdlib/str" as _;
```

Dependencies are explicit in the manifest:

```toml
[package]
name    = "notes"
version = "0.0.1"
edition = "2026"

[dependencies]
stdlib = "*"
```

`cpc pm` materializes dependencies into the versioned per-user store, or into
the project's `vendor/` directory with `--local`. Toolchain packages use `*`
and are locked to the compiler version; third-party packages use exact pinned
tree URLs. There are no version ranges or dependency solver.

```sh
cpc pm add . facet
cpc pm install
cpc pm manifest
```

Use `cpc pm add` for a multi-package feature such as Facet: it writes the
feature's complete dependency closure, including the platform-specific part,
instead of leaving that closure to be assembled by hand.

Applications use `src/main.cplus` by default or an explicit `entry`. A package
with no entry is a C+ library. `[library]` describes a C-ABI static or dynamic
library product. The removed `[[bin]]` and `[lib]` forms are not part of the
v0.0.28 manifest model.

See [Packages and platforms](docs/lang/packages.md) and the
[`cplus-pm` reference](cplus-pm/README.md) for the complete model.

## Toolchain

| Tool | Purpose |
|---|---|
| `cpc skill` | Print the version-matched C+ language reference plus dependency skills for an LLM or agent. |
| `cpc explain E####` | Explain a diagnostic offline with its cause, fix, and example. |
| `cpc init` | Scaffold a host, platform-scoped, or Facet project. |
| `cpc pm` | Add, install, update, and inspect dependencies; remove local vendored copies; resolve Android Maven/AAR dependencies. |
| `cpc check` | Run the whole-project front end without code generation; the fast project validation path. |
| `cpc check FILE` | Check and code-generate one import-free file without invoking clang. |
| `cpc build` | Build the current project for the host or a selected target. |
| `cpc FILE -o BIN` | Compile and link one import-free source file. |
| `cpc test` | Discover and run `#[test]` functions and doctests; supports filters, JSON, release mode, and sanitizers. |
| `cpc fmt` | Format files or a project; supports check, stdout, and stdin modes. |
| `cpc doc` / `cpc headers` | Generate Markdown API documentation or C headers. |
| `cpc graph` | Emit the resolved, typed code graph as JSON. |
| `cpc query` | Ask one semantic question: definitions, references, callers, callees, hierarchy, members, types, scope, completion, or an edit context. |
| `cpc mcp` | Keep the graph resident and expose it as MCP tools, including live unsaved-buffer updates. |
| `cpc lsp` | Run the language server over the same resident graph. |
| `cpc-bindgen` | Generate C+ bindings for C, Objective-C, Swift frameworks, Java, GObject Introspection, and pkg-config packages. |
| `cpc-wasm` | Run the front end in a browser and execute the currently supported WebAssembly subset. |

`cpc check` deliberately never invokes clang. It answers whether the C+
front end accepts the program; `cpc build` answers whether the emitted IR also
assembles and links. Diagnostics support human, short, and NDJSON output.

See [Tooling](docs/lang/tooling.md) for flags, sanitizer support, code-graph
queries, MCP tools, documentation generation, and artifact inspection.

## The complete agent development loop

“Built for agents” does not mean only reducing the time or token count needed
to generate source. C+ is designed around the complete development loop:

```text
understand code
    ↓  cpc query / cpc mcp
modify code
    ↓  edit source
validate cheaply
    ↓  cpc check
produce executable
    ↓  cpc build
run application
    ↓  connect to live agent surface
exercise real behavior
    ↓  click / type / invoke actions
inspect outcome
    ↓  read exposed state / events / results
expected?
    ├─ yes → continue
    └─ no  → query → edit → check → build → run again
```

The compiler's code graph supplies resolved understanding before an edit.
Numbered, machine-readable diagnostics close the source-repair step. For
applications that enable it, the optional agent stack exposes the live native
interface through stable identities, capability grants, semantic actions,
state, and events. The running application's behavior—not only a successful
build—then becomes evidence for the next iteration.

The stack is split into ordinary packages: `agent_core`, platform backends
(`agent_appkit`, `agent_uikit`, `agent_android`, `agent_gtk`, `agent_win32`),
`agent_mcp`, `agent_inapp`, and the optional `facet_agent` integration.

## Facet and native application capabilities

[Facet](vendor/facet/README.md) is the cross-platform UI package. Shared
component and application code is paired with native backends:

- `facet_appkit` — macOS / AppKit
- `facet_uikit` — iOS / UIKit
- `facet_android` — Android
- `facet_gtk` — Linux / GTK 4
- `facet_win32` — Windows / Win32

`facet_runtime` owns application startup, windows, routes, and navigation.
Capabilities remain packages rather than language features: HTTP, filesystem
watching, notifications, application links, camera, location, sensors,
biometrics, secure storage, permissions, file picking, haptics, sharing,
terminal support, SQLite, Metal, CUDA, Core ML, and llama.cpp bindings are all
represented in [`vendor/`](vendor/), with native implementations or explicit
unsupported outcomes per platform.

## Repository layout

The Rust workspace contains:

- [`cplus-core/`](cplus-core/) — lexer, parser, AST, semantic analysis, borrow
  checking, monomorphization, LLVM IR generation, diagnostics, code graph, and
  the WebAssembly emitter.
- [`cpc/`](cpc/) — compiler CLI, build driver, project scaffolding, package
  manager integration, queries, and MCP server.
- [`cpc-lsp/`](cpc-lsp/) — language server using the resident project graph.
- [`cpc-bindgen/`](cpc-bindgen/) — binding generation for foreign APIs.
- [`cpc-wasm/`](cpc-wasm/) — browser front end and WebAssembly run path.
- [`cplus-pm/`](cplus-pm/) — package manager library and standalone
  compatibility binary; also exposed through `cpc pm`.

The C+ packages live under [`vendor/`](vendor/). Language documentation is in
[`docs/lang/`](docs/lang/), runnable recipes are in
[`docs/examples/recipes/`](docs/examples/recipes/), and compiler internals and
design notes are in [`docs/compiler/`](docs/compiler/).

## Building and testing the toolchain

Building the compiler requires a Rust toolchain and a compatible clang for the
native end-to-end tests:

```sh
git clone https://github.com/netdur/cplus.git
cd cplus
cargo build --release
cargo test --workspace
```

The release compiler is written to `target/release/cpc`.

GitHub Actions runs the Rust workspace tests on macOS / Apple Silicon, Linux /
x86-64, and Windows / x86-64 MSVC after every push. Linux and Windows also
package and smoke-test the installed toolchain; release assets are attached on
`v*` tags. End-to-end fixtures share mutable package build outputs, so tests run
serially inside each platform job while the platform jobs run independently.

For C+ packages, run `cpc test` from the package directory. The project uses
unit, end-to-end, negative, and doctest coverage; platform behavior is also
checked with simulator, device, and probe applications where applicable.

## Documentation

- [Language tour](docs/lang/tour.md)
- [Guide](docs/lang/guide.md)
- [Language reference](docs/lang/ref.md)
- [Normative specification](docs/lang/spec.md)
- [Ownership](docs/lang/ownership.md)
- [Memory model](docs/lang/memory-model.md)
- [Packages](docs/lang/packages.md)
- [Platforms](docs/lang/platforms.md)
- [Tooling](docs/lang/tooling.md)
- [Testing](docs/lang/testing.md)
- [Dense language skill for LLMs and agents](docs/lang/skill.md)
- [Facet tutorial](vendor/facet/docs/tutorial.md)
- [Facet application and navigation guide](vendor/facet/docs/navigation.md)
- [v0.0.28 changelog](changeslog.md#v0028--2026-09-18)

Every release should be read with its matching documentation and packages.
C+ is pre-1.0 and moves quickly; pin the toolchain version for real projects.

## Contributing

Contributions are welcome. Keep changes within the language's small-core,
package-extensible model, add tests for positive and negative behavior, run the
relevant package suite, and run `cargo test --workspace` before submitting a
toolchain change.

## License

C+ is available under the [MIT License](LICENSE).
