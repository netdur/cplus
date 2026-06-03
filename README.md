# C+ Programming Language

## Welcome to C+

C+ is an experimental, safety-oriented systems programming language and toolchain. Its main design philosophy is to keep the core language as small as possible while moving high-level capabilities into explicit, vendored packages. 

C+ provides the necessary primitives for low-level systems programming:
- **Safety**: Ownership, borrow checking, and memory-safe abstractions.
- **Modern Constructs**: Structs, tagged enums, generics, interfaces, methods, and modules.
- **Low-Level Control**: Raw pointers, `unsafe` blocks, `#[repr(C)]`, SIMD primitives, atomics, threads, and direct LLVM IR generation.
- **C Interoperability**: Seamless C ABI interop, `extern fn`, and clang-based linking.

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
- **`cpc check`**: Runs parsing, semantic analysis, and borrow checking without emitting code.
- **`cpc test`**: Discovers and runs `#[test]` functions and doctests.
- **`cpc fmt`**: Formats your C+ source code.
- **`cpc doc`**: Generates Markdown documentation from public items.
- **`cpc lsp`**: Starts the Language Server for your editor.
- **`cpc-bindgen`**: Generates C+ FFI declarations from C headers.

### Creating a C+ Project

A C+ project uses a `Cplus.toml` manifest file. Imports in C+ have a strict, clean shape to ensure builds are predictable:

```cplus
import "./local_module"
import "stdlib/io" as io;
import "metal/device" as metal;
```
Vendored packages (like `stdlib`, `simd`, and `metal`) must be declared in your manifest. The compiler validates these artifacts to ensure robust, reproducible builds.

## Learning More

- Check the `docs/` directory for examples, design notes, and deep dives into the language phases.
- See the `/vendor/` directory to explore how major language features are implemented purely through the package system.
