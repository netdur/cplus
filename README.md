# C+ Programming Language

## Welcome to C+

C+ is an experimental, safety-oriented systems programming language and toolchain. Its main design philosophy is to keep the core language as small as possible while moving high-level capabilities into explicit, vendored packages.

**Website:** <https://cplus-lang.dev> · **Source:** <https://github.com/netdur/cplus>

C+ provides the necessary primitives for low-level systems programming:
- **Safety**: Ownership, borrow checking, memory-safe abstractions, and raw-pointer accountability (every `*T` field must be released or marked `opaque`). No `null` in safe code.
- **Modern Constructs**: Structs, tagged enums, generics, interfaces, methods, and modules.
- **Low-Level Control**: Raw pointers, `unsafe` blocks, `#[repr(C)]`, SIMD primitives, atomics, threads, compiler-checked real-time contracts (`#[no_alloc]` / `#[realtime]`), and direct LLVM IR generation.
- **C Interoperability**: Seamless C ABI interop, `extern fn`, and clang-based linking.
- **Built for tools and LLMs**: a deliberately small, unambiguous surface, plus a resolved, typed **code-knowledge graph** the compiler exposes to editors and agents (`cpc query` / `cpc mcp`, and the LSP) — so navigation is by *symbol and type*, not text search.

Instead of relying on compiler magic for everything, C+ relies on an external-package architecture. Capabilities like the standard library (`stdlib`), 3D math (`simd`), GPU compute (`metal`), and UI bindings (`appkit`) are implemented as regular packages, keeping the compiler focused and fast.

- [Contributing to C+](#contributing-to-c)
- [Getting Started](#getting-started)
  - [Building the Compiler](#building-the-compiler)
  - [Language Tools](#language-tools)
- [Learning More](#learning-more)

## Contributing to C+

Contributions to C+ are welcomed and encouraged! 

The C+ toolchain is implemented as a Rust workspace containing:
- `cplus-core`: The core compiler library (lexer, parser, AST, semantic analyzer, borrow checker, monomorphizer, LLVM IR codegen).
- `cpc`: The command-line compiler and build driver.
- `cpc-lsp`: The JSON-RPC language server for editor integration.
- `cpc-bindgen`: A C header to C+ FFI generator.

To test your changes before submitting a pull request, you can run the full test suite locally:

```sh
$ cargo test --workspace
```

To be a truly great community, C+ needs to welcome developers from all walks of life, with different backgrounds, and with a wide range of experience. A diverse and friendly community will have more great ideas, more unique perspectives, and produce more great code. We work diligently to make the C+ community welcoming to everyone.

## Getting Started

### Building the Compiler

Since the C+ compiler is built in Rust, you will need a Rust toolchain installed. To build the compiler from source:

```sh
$ git clone https://github.com/netdur/cplus.git
$ cd cplus
$ cargo build --release
```

Once built, the `cpc` compiler binary will be available in `target/release/cpc`.

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

## Learning More

- Check the [`docs/`](docs/) directory — runnable [`docs/examples/`](docs/examples/), design deep-dives in [`docs/design/`](docs/design/), and [`docs/SKILL.md`](docs/SKILL.md) (a dense reference for LLMs writing C+).
- See the [`vendor/`](vendor/) directory to explore how major language features (stdlib, SIMD, GPU, AppKit, JNI) are implemented purely through the package system.
