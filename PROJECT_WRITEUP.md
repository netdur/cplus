# C+ Project Write-Up

## One-Sentence Summary

C+ is an experimental systems programming language and toolchain whose main design bet is: keep the core language small and safety-oriented, then move as much capability as possible into external C+ packages such as `stdlib`, `simd`, `metal`, `appkit`, `json`, and other vendored libraries.

## Core Idea

The project is not only a compiler experiment. It is also an experiment in language boundary design.

The compiler provides the minimum primitives needed for systems programming:

- parsing, type checking, ownership, borrow checking, and diagnostics
- structs, enums, generics, methods, interfaces, modules, and packages
- raw pointers, `unsafe`, C ABI interop, `extern fn`, `#[repr(C)]`, and link control
- LLVM IR generation and clang-based linking
- low-level primitives such as SIMD vector types, atomics, threads, async/futures, and raw memory access

Most higher-level capability is intentionally kept outside the core language:

- the standard library is a vendored package, not compiler magic
- SIMD math is a package built on compiler vector primitives
- GPU/Metal support is a package built through C/Objective-C FFI and framework linking
- AppKit/Cocoa UI bindings are a package
- JSON, UUID, arena allocation, logging, CLI parsing, and other utilities are packages

That matters because it keeps the compiler from becoming the place where every feature goes. The compiler should make external packages possible, safe enough, and ergonomic; the packages should carry domain APIs.

## What The Repository Contains

This is a Rust workspace with four main crates:

- `cplus-core`: the compiler library. It contains the lexer, parser, AST, resolver, semantic analyzer, borrow checker, formatter, documentation generator, monomorphizer, diagnostics, and LLVM IR code generator.
- `cpc`: the command-line compiler and build driver. It exposes `cpc FILE`, `cpc build`, `cpc check`, `cpc test`, `cpc fmt`, `cpc doc`, `cpc lsp`, `--emit-ll`, `--emit-obj`, and `--emit-header`.
- `cpc-lsp`: a stdio JSON-RPC language server for editor diagnostics, formatting, quick fixes, and goto-definition.
- `cpc-bindgen`: a C header to C+ FFI generator. It shells out to clang, reads clang's JSON AST, and emits C+ `extern fn` and `#[repr(C)]` declarations.

The compiler pipeline is:

```text
.cplus source
  -> lex
  -> parse into AST
  -> lower language sugar
  -> resolve modules/imports/packages
  -> semantic analysis
  -> borrow and ownership checks
  -> monomorphize generics
  -> generate LLVM IR
  -> call clang for object/library/binary output
```

## The External-Package Architecture

C+ projects use `Cplus.toml`. Imports have a strict shape:

- `import "./module"` means a local source file.
- `import "package/module"` means a vendored package under `vendor/<package>/src/`.
- local imports must use `./`
- package imports must name a declared dependency
- import strings omit the `.cplus` extension

This is deliberate. The compiler can classify imports without guessing from the filesystem, and packages remain explicit dependencies.

Packages can be:

- source-only: C+ source is compiled with the consumer
- bundled-binary: prebuilt static archives are linked
- mixed: C+ source plus declared link requirements

The package manifest is the source of truth. If a package declares a library, framework, bundled archive, host triple, or extra object, the build driver validates that declaration. It rejects missing declared artifacts and undeclared artifacts.

## Vendored Packages Are Central

The `vendor/` folder is not incidental. It is where the project proves that major features do not need to become core language constructs.

### `vendor/stdlib`

The standard library is external to the compiler. It is imported like any other package:

```cplus
import "stdlib/io" as io;
import "stdlib/hash_map" as map;
import "stdlib/result" as result;
```

It includes modules such as:

- `option`
- `result`
- `io`
- `fs`
- `net`
- `vec`
- `hash_map`
- `thread`
- `mutex`
- `atomic`
- `future`
- `executor`
- `reactor`
- `channel`
- `rc`
- `arc`
- `box`
- `cow`
- `iterator`
- `time`
- `env`

The important design point is that `stdlib` is just a package in `vendor/stdlib`, with a `Cplus.toml` manifest and `.cplus` source files. That forces the language and package system to be good enough to support normal library development instead of relying on hidden compiler-only behavior.

### `vendor/simd`

SIMD is handled in two layers:

- the compiler has primitive SIMD vector support and LLVM intrinsic lowering
- `vendor/simd` turns those primitives into domain-friendly math types

The `simd` package wraps lower-level vector types such as `f32x4` and `f64x2` into APIs like:

- `Vec3`
- `Vec4`
- `Mat4x4`
- `dot`
- `cross`
- `length`
- `normalize`
- `reflect`
- `refract`
- `lerp`
- `min`
- `max`
- `clamp`

For example, `Vec3` is implemented as a newtype over `f32x4`, with lane 3 held at zero. That gives 3D math a clean API while still compiling to vector operations such as vector multiply, swizzle, reductions, FMA, and sqrt.

This is a key proof point: the language core exposes enough low-level SIMD machinery, but the user-facing 3D math library lives outside the compiler.

### `vendor/metal`

GPU support is also external. `vendor/metal` is a C+ package that links Apple's Metal and Foundation frameworks plus `libobjc`:

```toml
[link]
frameworks = ["Metal", "Foundation"]
libs       = ["objc"]
```

It provides C+ wrappers around Metal concepts:

- `Device`
- `CommandQueue`
- `CommandBuffer`
- `ComputeCommandEncoder`
- `Library`
- `Function`
- `ComputePipelineState`
- `Buffer`
- `MTLSize`

Internally, the package uses:

- `extern fn` bindings to Objective-C runtime functions
- `#[link_name = "objc_msgSend"]` declarations
- raw pointers and `unsafe`
- C+ `drop` methods to release Objective-C objects
- manifest-declared framework and library linkage

The package facade is `vendor/metal/src/metal.cplus`, and the implementation is split across `runtime`, `device`, `queue`, `pipeline`, and `buffer` modules.

This is the GPU proof point: C+ does not need a special `gpu` keyword or a built-in Metal subsystem. The core language needs FFI, unsafe, packages, linking, raw pointers, and ownership cleanup. The GPU API itself can live in a package.

### `vendor/appkit`

`appkit` follows the same pattern for Cocoa/AppKit UI work. It is a package with a manifest that links Cocoa and `objc`, then exposes modules for:

- application and window management
- views and layout containers
- controls
- text inputs
- tables, outlines, and collections
- menus, dialogs, panels, toolbars, and controllers
- Cocoa data conversion helpers

This is another example of the project direction: OS/framework integration belongs in packages, not in the core compiler.

### Other Packages

Other vendored packages exercise the same package model:

- `json`: JSON parsing/handling package
- `arena`: arena allocation
- `uuid`: UUID helpers
- `log`: logging
- `clap`: command-line argument parsing

These packages are useful by themselves, but their larger role is to test whether C+ can grow a library ecosystem without adding every capability to the language core.

## Tooling Around The Language

The repository includes practical tooling:

- `cpc check`: parse, semantic analysis, and borrow checking without codegen
- `cpc fmt`: formatter with rewrite, check, stdin, and emit modes
- `cpc test`: discovers `#[test]` functions and doctests
- `cpc doc`: generates Markdown docs from public documented items
- `cpc lsp`: starts the language server
- VS Code extension: editor integration around `cpc lsp`
- `cpc-bindgen`: generates C+ FFI declarations from C headers
- `--emit-header`: emits C headers for C ABI-compatible public C+ items
- sanitizer/debug flags: ASan, UBSan, TSan, MSan, and DWARF debug info

Diagnostics are treated as a first-class part of the compiler. They have error codes, spans, labels, JSON output, and in many cases machine-applicable suggestions for editor quick fixes.

## Current Feature Surface

The current working tree supports a broad language surface:

- integer, float, bool, string, and character literals
- functions, methods, structs, enums, tagged enums, interfaces, and impls
- arrays, slices, raw pointers, function pointers, and unsafe blocks
- generics over structs, enums, functions, and methods
- module imports and vendored package imports
- ownership, moves, drops, and borrow checking
- `const`, `static`, and `static mut`
- pattern matching, `if let`, guard-let, loops, defer, assertions, and tests
- C ABI exports/imports, C header generation, and bindgen-generated declarations
- SIMD primitives, atomics, threads, async/future/executor/reactor primitives
- project builds, libraries, static/dynamic outputs, extra objects, and link metadata

The core language is therefore low-level and capable, but the project consistently tries to make reusable capability show up as packages.

## Performance Context

There is also an external benchmark suite at `/Users/adel/Workspace/bench-cplus/bench.md` comparing C, C+, Rust, and Swift on the same algorithms. The current benchmark set covers:

- a single-threaded raytracer
- a string-to-integer open-addressing hashmap
- a JSON tokenizer

On the documented Apple Silicon run, C+ is competitive across all three and wins the raytracer outright:

| Benchmark | C | C+ | Rust | Swift |
|---|---:|---:|---:|---:|
| Raytracer | 1.16 s | 0.94 s | 1.16 s | 1.46 s |
| Hashmap insert | 19.9 ms | 20.2 ms | 21.7 ms | 23.9 ms |
| Hashmap lookup | 130.8 ms | 111.1 ms | 109.4 ms | 399.4 ms |
| JSON tokenizer | 6.60 ms | 6.51 ms | 6.27 ms | 9.28 ms |

The benchmark notes are useful because they show what kind of systems programming the language is aiming at:

- C+ produced the smallest raytracer binary in that run: 33,656 bytes versus 50,312 bytes for C and 302,496 bytes for Rust.
- C+ build times were in the same small-program range as C, around 0.10-0.12 seconds in those benchmarks.
- The raytracer result came from writing C+ in a C-like performance style: free numeric functions, explicit out-pointers for large struct results, `restrict` on non-aliasing heap pointers, and pre-allocated scratch pools.
- The `restrict x: *T` parameter form matters because it propagates to LLVM `noalias`.
- The compiler's codegen produced dense FMA-heavy scalar code on the raytracer, which beat the C version's NEON 2-lane strategy on that Apple Silicon run.

This benchmark suite also records current friction. Some source-level workarounds exist for compiler issues such as over-aggressive `musttail` marking and mixed if-arm codegen panics. That is useful context: C+ is already capable of competitive low-level output, but some of the performance story still depends on choosing compiler-friendly source shapes and closing long-tail codegen bugs.

## Repository Map

Important paths:

- `cplus-core/src/`: core compiler implementation
- `cpc/src/main.rs`: CLI and build/link driver
- `cpc-lsp/src/main.rs`: language server
- `cpc-bindgen/src/main.rs`: C header binding generator
- `vendor/stdlib/`: external standard library package
- `vendor/simd/`: SIMD math package
- `vendor/metal/`: Metal GPU compute package
- `vendor/appkit/`: Cocoa/AppKit UI package
- `vendor/json/`, `vendor/arena/`, `vendor/uuid/`, `vendor/log/`, `vendor/clap/`: additional package-system proof points
- `docs/examples/`: runnable C+ examples and package projects
- `docs/design/`: design notes for language phases and features
- `bench/` and `bench.md`: C vs C+ sanity benchmarks
- `/Users/adel/Workspace/bench-cplus/bench.md`: external C / C+ / Rust / Swift benchmark suite
- `editors/vscode/`: VS Code extension
- `plan.md` and `plan-*.md`: active and archived roadmap

## Current Status

The active roadmap says v0.0.8 shipped on 2026-05-22. The current `plan.md` focuses on v0.0.9 polish and correctness work, including safety defaults, character literals, codegen bug fixes, const/static items, pointer-to-integer casts, local import walking for object emission, and extra object linkage.

The working tree already contains code/tests for several of those areas. It also has uncommitted changes in compiler code, tests, plans, tutorial/docs, examples, and vendored packages. This write-up describes the current working tree rather than a clean tagged release.

## Verification

I ran:

```sh
cargo test --workspace
```

Result:

- `cpc` e2e tests: 394 passed
- `cpc-bindgen` unit tests: 4 passed
- `cpc-lsp` e2e tests: 11 passed
- `cplus-core` unit tests: 1015 passed
- doc tests: 0 run

The suite passed. The build produced warnings, mostly unused variables in `cplus-core/src/codegen.rs`, an unused helper in `resolver.rs`, an unused import in `cpc/src/main.rs`, and unused project-check helper functions in `cpc/src/main.rs`.

## What Someone Reading This Should Understand

The project is best understood as three things at once:

1. A working compiler for a C-compatible, safety-oriented systems language.
2. A tooling stack around that compiler: formatter, tests, docs, diagnostics, LSP, bindgen, and header generation.
3. A package architecture experiment where stdlib, SIMD math, GPU compute, UI bindings, JSON, logging, UUIDs, arenas, and CLI parsing live outside the compiler.

The third point is the most important design theme. C+ is trying to prove that the core language can stay compact while still enabling serious systems APIs through packages, FFI, and strict build metadata.
