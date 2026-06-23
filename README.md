# C+ Programming Language

## Welcome to C+

C+ is an experimental, safety-oriented systems programming language and toolchain. Its main design philosophy is to keep the core language as small as possible while moving high-level capabilities into explicit, vendored packages.

**Website:** <https://cplus-lang.dev> · **Source:** <https://github.com/netdur/cplus>

C+ provides the necessary primitives for low-level systems programming:
- **Safety**: Ownership, borrow checking, memory-safe abstractions, and raw-pointer accountability (every `*T` field must be released or marked `opaque`). No `null` in safe code.
- **Modern Constructs**: Structs, tagged enums, generics, interfaces, methods, and modules.
- **Package-extensible DSLs**: `@view { ... }` contextual builder blocks let a package expose concise declarative construction syntax (UI trees, route tables, config) without macros, closures, or compiler plugins. The compiler owns only the `@context { ... }` syntax, contextual name lookup, leading-dot modifiers, and `if`/`for` item-control; a package supplies ordinary builder types and element constructors. Bare child elements (`vstack { ... }`) nest within the same context.
- **Low-Level Control**: Raw pointers, `#[repr(C)]`, SIMD primitives, atomics, threads, compiler-checked real-time contracts (`#[no_alloc]` / `#[realtime]`), and direct LLVM IR generation.
- **C Interoperability**: Seamless C ABI interop, `extern fn`, and clang-based linking.
- **Multi-target**: `--target` cross-compiles for iOS (`ios-arm64`), Android (`android-arm64`, via the NDK's clang), and ESP32 (`esp32-xtensa`, 32-bit, via Espressif's esp-clang). cpc emits the object or static library; the platform's build system (Xcode, Gradle/NDK, ESP-IDF) owns the final link. Compiler-checked `#[realtime]` code runs on a $4 microcontroller.
- **Built for tools and LLMs**: a deliberately small, unambiguous surface, plus a resolved, typed **code-knowledge graph** the compiler exposes to editors and agents (`cpc query` / `cpc mcp`, and the LSP) — so navigation is by *symbol and type*, not text search.

Instead of relying on compiler magic for everything, C+ relies on an external-package architecture. Capabilities like the standard library (`stdlib`), 3D math (`simd`), GPU compute (`metal`), and UI bindings (`appkit` for macOS, `uikit` for iOS, `gtk`/`adwaita` for Linux, `espidf` for ESP32 firmware) are implemented as regular packages, keeping the compiler focused and fast.

- [Getting Started](#getting-started)
  - [Installing](#installing)
  - [Requirements](#requirements)
  - [Language Tools](#language-tools)
  - [Creating a C+ Project](#creating-a-c-project)
- [Contributing to C+](#contributing-to-c)
  - [Building from Source](#building-from-source)
- [Learning More](#learning-more)

## Getting Started

### Installing

On macOS (Apple Silicon), install C+ with Homebrew:

```sh
brew install netdur/cplus/cplus
```

This installs prebuilt `cpc` (compiler), `cpc-lsp` (language server), and `cpc-bindgen` (FFI generator) binaries — **no build step, installed in seconds**. Update later with `brew upgrade cplus`.

On Linux (`x86_64`, Debian/Ubuntu) and Windows (`x86_64`), prebuilt binaries are attached to each [GitHub release](https://github.com/netdur/cplus/releases/latest). These ports work but are not yet part of the tested matrix (see [Requirements](#requirements)):

- **Linux**: download the `.deb` and `sudo apt install ./cplus_*_amd64.deb` (this resolves the clang ≥ 19 dependency).
- **Windows**: download `cplus-x86_64-pc-windows-msvc.zip` and put `cpc.exe`, `cpc-lsp.exe`, and `cpc-bindgen.exe` on your `PATH`.

To build from source instead, see [Building from Source](#building-from-source).

### Requirements

C+ has a single external dependency: a C toolchain (**clang**), used to assemble and link the native binary. `cpc` emits textual LLVM IR and shells out to `clang`, detecting the host target with `clang -print-target-triple`. clang already bundles LLVM, so **no separate LLVM install is needed**. Cross-compiling with `--target` uses the platform's own toolchain: the Android NDK's clang for `android-arm64`, Espressif's esp-clang for `esp32-xtensa` (both auto-discovered from their default install locations).

On macOS, install the Xcode Command Line Tools (most developers already have them):

```sh
xcode-select --install
```

The front-end-only commands (`cpc check`, `cpc --emit-ll`, `cpc lsp`, `cpc graph`, `cpc query`, `cpc mcp`, `cpc fmt`, `cpc doc`) are self-contained and need no external tools.

C+ is developed and tested on macOS / Apple Silicon (`aarch64-apple-darwin`) against **Apple clang 21.0.0** — the configuration the test suite runs against. As of v0.0.25 there are working **Linux** (`x86_64`, GTK 4 / libadwaita) and **Windows** (`x86_64-pc-windows-msvc`, Win32) ports; those targets and other clang versions may work but are not yet part of the tested matrix.

### Language Tools

The C+ repository includes a robust suite of practical tooling to improve the developer experience:

- **`cpc build`**: Compiles C+ projects and handles linking.
- **`cpc check`**: Runs parsing, semantic analysis, and borrow checking without emitting code (whole-project front-end / CI gate; enforces any `[profile.realtime]`).
- **`cpc test`**: Discovers and runs `#[test]` functions and doctests.
- **`cpc fmt`**: Formats your C+ source code.
- **`cpc doc`**: Generates Markdown documentation from public items.
- **`cpc lsp`**: Starts the Language Server (goto-definition, references, hover, outline — served from the code graph).
- **`cpc graph` / `cpc query` / `cpc mcp`**: The resolved, typed code-knowledge graph — as JSON, as per-symbol queries (`def`/`refs`/`callers`/`callees`/`call-hierarchy`/`type-at`/`context`/…), or as a resident MCP server for agents.
- **`cpc --realtime-report`**: Whole-project digest of the real-time contract analysis.
- **`cpc-bindgen`**: Generates C+ FFI declarations from C headers.

### Creating a C+ Project

A C+ project uses a `Cplus.toml` manifest file. Imports in C+ have a strict, clean shape to ensure builds are predictable:

```cplus
import "./local_module" as local;
import "stdlib/io" as io;
import "metal/metal" as metal;
```
Every import names its source and binds an alias (`import "X" as Y;`) — local paths start with `./`, vendored packages match a `[dependencies]` entry in the manifest. The compiler validates these artifacts to ensure robust, reproducible builds.

## Contributing to C+

Contributions to C+ are welcomed and encouraged!

The C+ toolchain is implemented as a Rust workspace containing:
- `cplus-core`: The core compiler library (lexer, parser, AST, semantic analyzer, borrow checker, monomorphizer, LLVM IR codegen).
- `cpc`: The command-line compiler and build driver.
- `cpc-lsp`: The JSON-RPC language server for editor integration.
- `cpc-bindgen`: A C header to C+ FFI generator.
- `cpc-wasm`: A WebAssembly build of the front end powering the in-browser playground on [cplus-lang.dev](https://cplus-lang.dev) (source → diagnostics + LLVM IR, client-side).

### Building from Source

Building the compiler from source requires a Rust toolchain:

```sh
$ git clone https://github.com/netdur/cplus.git
$ cd cplus
$ cargo build --release
```

Once built, the `cpc` compiler binary will be available in `target/release/cpc`.

Run the full test suite before submitting a pull request:

```sh
$ cargo test --workspace
```

To be a truly great community, C+ needs to welcome developers from all walks of life, with different backgrounds, and with a wide range of experience. A diverse and friendly community will have more great ideas, more unique perspectives, and produce more great code. We work diligently to make the C+ community welcoming to everyone.

## Learning More

- Read [`docs/SPEC.md`](docs/SPEC.md) — the normative language specification (syntax, semantics, ownership model, the builder-block DSL, error-code catalog).
- Check the [`docs/`](docs/) directory — runnable [`docs/examples/`](docs/examples/), design deep-dives in [`docs/design/`](docs/design/), and [`docs/SKILL.md`](docs/SKILL.md) (a dense reference for LLMs writing C+).
- See the [`vendor/`](vendor/) directory to explore how major language features (stdlib, SIMD, GPU, AppKit, GTK, JNI) are implemented purely through the package system.
