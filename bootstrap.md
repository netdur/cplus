# Bootstrap: The Path to a Self-Hosting C+ Compiler

This document outlines the architectural roadmap for bootstrapping C+ — transitioning the compiler (`cpc` and `cplus-core`) from Rust into C+ itself. 

Because C+ is an **AI-native language**, the typical human hurdles of bootstrapping (massive manual refactoring, fighting verbosity, managing 10,000 lines of boilerplate) are largely mitigated. An AI agent can mechanically translate the existing Rust codebase into C+ with high fidelity, provided the underlying language features are in place.

---

## 1. The Current State: Why It's Possible Now

C+ already possesses the heavy-lifting capabilities required for a compiler:
*   **Tagged Enums & Pattern Matching:** Crucial for AST representation (e.g., `enum ExprKind { Binary(...), Literal(...) }`) and the parser/sema control flow (`match`).
*   **Memory Model:** The borrow checker, `move` semantics, and deterministic `drop` mean the compiler will be memory-safe without relying on a garbage collector.
*   **I/O & Collections:** The standard library already supports `Vec[T]`, `string`, `Option`, `Result`, file reads (`fs`), and environment parsing (`env`).

---

## 2. The Gap: Prerequisite Features

Before the rewrite can begin, the C+ standard library and language need four specific additions to support a compiler's workload. These should be treated as blocking prerequisites.

### A. `Box[T]` (Recursive Types)
Compilers require recursive data structures. An AST node often contains other AST nodes. In Rust, this is solved with `Box<Expr>`. Since C+ cannot embed a struct inside itself directly (infinite sizing), the stdlib needs a `Box[T]` type.
*   **Implementation:** A safe wrapper over `malloc` + `*T` with a `drop` method that calls `libc::free`.
*   **Usage:** `struct BinaryExpr { left: Box[Expr], right: Box[Expr] }`.

### B. Generic `HashMap[K, V]`
`sema.rs` relies heavily on mapping string names to internal IDs (e.g., `HashMap[String, StructId]`). Phase 1 shipped `StrIntMap`, but the fully generic map (planned for v0.0.4) is required for compiler symbol tables.
*   **Requirement:** Cross-module generic method instantiation and `Hash`/`Eq` interfaces.

### C. Subprocess Execution (`stdlib/process`)
The compiler driver (`cpc/src/main.rs`) shells out to `clang` to lower the emitted LLVM IR text into object files or binaries.
*   **Implementation:** An FFI wrapper around `posix_spawn` or `fork`/`execve` that allows C+ to build a command, pass arguments, and wait for the exit code.

### D. String & Slice Utilities
The `lexer.rs` relies on character iteration and string slicing. While C+ has `str` slices, ergonomic utilities (like finding substrings, checking prefixes, or parsing integers from slices) need to be fleshed out in the stdlib to keep the lexer clean.

---

## 3. The Execution Plan (Phase-by-Phase)

Once the prerequisites are met, the actual bootstrap is an exercise in AI-driven translation. The architecture stays exactly the same: `lexer` → `parser` → `sema` → `borrowck` → `codegen`.

### Phase A: AST and Lexer
*   **Goal:** Translate `ast.rs` and `lexer.rs`.
*   **Approach:** Define the giant `enum ItemKind` and `struct Expr` structures in C+ using the new `Box[T]`. Translate the state-machine logic of the lexer.
*   **Validation:** Write a small C+ program that reads a `.cplus` file, lexes it, and prints the token stream. Verify it matches the Rust compiler's `--tokens` output.

### Phase B: Parser and Resolver
*   **Goal:** Translate `parser.rs` and `resolver.rs`.
*   **Approach:** Convert the recursive descent parser. Rust's `Result<Expr, ParseError>` maps 1:1 to C+'s `Result[Expr, ParseError]`. 
*   **Validation:** Parse a project and emit the AST structure to stdout. Compare with the Rust compiler's `--ast` output.

### Phase C: Semantic Analysis & Borrowck (The Heavy Lift)
*   **Goal:** Translate the ~8,000 lines of `sema.rs` and `borrowck.rs`.
*   **Approach:** This is where `HashMap[K, V]` is heavily utilized to build the `TypeTable` and `SemaCx`. The logic is highly mechanical but dense. The AI should break this down function-by-function.
*   **Validation:** Run the C+ version of `sema` against the existing `proves/` benchmark suite and ensure it emits the exact same diagnostic codes (e.g., `E0302`, `E0335`).

### Phase D: LLVM Codegen & The Driver
*   **Goal:** Translate `codegen.rs` and `cpc/src/main.rs`.
*   **Approach:** `codegen.rs` is primarily string manipulation (emitting `define i32 @...`). The driver handles CLI arguments and invokes `clang` via `stdlib/process`.
*   **Validation:** The C+ compiler can now take a `.cplus` file and output a functional binary.

---

## 4. The Final Cutover: Self-Hosting

1. **Generation 1 (Rust compiles C+):** Use the existing Rust `cpc` binary to compile the new C+ source code of the compiler. This produces the `cpc.bin` executable.
2. **Generation 2 (C+ compiles C+):** Use `cpc.bin` to compile its own C+ source code. This produces `cpc.bin.v2`.
3. **Verification:** If `cpc.bin` and `cpc.bin.v2` are byte-for-byte identical (or behaviorally identical), the compiler is successfully bootstrapped.
4. **Retirement:** The Rust codebase (`cpc`, `cplus-core`) is archived. All future development of the C+ language happens entirely within C+.
