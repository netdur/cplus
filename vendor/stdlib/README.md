# stdlib

The C+ standard library. **Bootstrap-stage**: the API surface is sketched,
the implementations are stubbed. Phase 3 of plan.md ([§"Phase 3 — Language
completeness + reference library + stdlib bootstrap"](../../plan.md))
fills in the bodies.

This directory replaces the two earlier scaffolds (`vendor/stdlib/` source
and `vendor/stdlib_bin/` binary). The package is now a single unified
target that can ship as **source**, **binary**, or **mixed** — see
"Distribution modes" below. Today it's source-only; the `lib/<triple>/`
slots are placeholders for future per-arch binary releases.

## Purpose

Two roles in one tree:

1. **Authoring target.** Where the actual stdlib implementations will land
   as Phase 3 progresses. The `src/*.cplus` files carry the public API
   signatures and `// TODO: Phase 3` markers for bodies.
2. **Layout reference.** A worked example of the v0.0.2 package shape for
   anyone writing their own C+ package.

## Layout

```
vendor/stdlib/
├── Cplus.toml                          ← package manifest
├── README.md                           ← you are here
└── src/                                ← C+ source files (importable as modules)
    ├── result.cplus                    ← stdlib/result — Result[T, E], IoError
    ├── io.cplus                        ← stdlib/io     — print, println, eprintln, read_stdin_line
    ├── fs.cplus                        ← stdlib/fs     — File: open / read_to_end / write_all / close
    ├── net.cplus                       ← stdlib/net    — TcpStream, TcpListener
    ├── vec.cplus                       ← stdlib/vec    — Vec[T]
    ├── hash_map.cplus                  ← stdlib/hash_map — HashMap[K, V]
    ├── env.cplus                       ← stdlib/env    — var, args
    └── lib/                            ← (optional) prebuilt static libraries, one dir per supported host triple
        ├── aarch64-apple-darwin/       ← drop stdlib.a here for macOS arm64 binary releases
        └── x86_64-unknown-linux-gnu/   ← drop stdlib.a here for Linux x86_64 binary releases
```

The `src/lib/<triple>/` directories are **empty placeholders today**.
They reserve the layout the binary-mode build driver will use once
Phase 2's package resolver knows how to find them (per plan.md
§"Phase 2 — Package layout", which canonicalizes `src/lib/<triple>/foo.a`
as the prebuilt-artifact location).

## Distribution modes

A package can ship in one of three forms. The mode is determined by what
the package author **declares in the manifest** (the `[link].bundled` and
`[link].triples` fields). The filesystem is verified against those
declarations, never scanned to discover artifacts behind the manifest's
back. See plan.md §"Phase 2 — Manifest = single source of truth" for the
rationale and E08xx error codes.

| Mode | `[link]` in `Cplus.toml` | Files under `src/lib/<triple>/` | Consumer build cost |
|---|---|---|---|
| **Source-only** *(today)* | no `bundled` / `triples` fields | none allowed (orphan files → E0861) | recompile-on-build |
| **Binary-only** *(future)* | `bundled = ["stdlib.a"]` + `triples = ["<host>", ...]` | one `stdlib.a` per declared triple | zero recompile; the declared `.a` is spliced into the consumer's link line |
| **Mixed** *(future)* | `bundled` lists only the binary-backed artifacts | only the declared files (one per triple) | partial recompile |

The unified package supports all three so a downstream user can iterate
in source mode, then ship binary-mode releases without changing the
import paths consumers use (`import "stdlib/io"` works either way).

The build driver enforces the manifest as authoritative:
- **E0860** — `[link].bundled` names a file that's not at `src/lib/<host-triple>/<name>`.
- **E0861** — a `.a` file is at `src/lib/<triple>/` but isn't in `[link].bundled`.
- **E0862** — consumer's host triple isn't in `[link].triples`.
- **E0863** — `bundled` is non-empty but `triples` is empty.

Phase 2's import resolver currently only consumes `src/*.cplus`; the
bundled-binary enforcement lands in Slice 2C (build driver). Today this
package's `[link]` is empty, so the source-only path applies.

## Module conventions

Per the v0.0.2 package design ([plan.md §"Phase 2 — Locked design decisions"](../../plan.md)):

- **No `.cplus` files at the package root.** Only `Cplus.toml`, `README.md`,
  and `src/` (with `src/lib/<triple>/` optional for prebuilt artifacts).
- **All importable code lives under `src/`.** `import "stdlib/env"` →
  `<pkg>/src/env.cplus`. Sub-dirs work too:
  `import "stdlib/collections/vec"` → `<pkg>/src/collections/vec.cplus`.
- **`pub` controls visibility.** `pub fn` / `pub struct` / `pub enum`
  items are reachable cross-package. Non-`pub` items are private to their
  declaring file — cross-file access fires E0403.
- **libc FFI is the implementation strategy.** Stdlib modules `extern fn`
  into libc (open/read/write/close/socket/connect/getenv/etc.) and present
  idiomatic C+ types (`Result`, `File`, `TcpStream`) to consumers. Users
  never see `extern fn`.

## Symbol-naming convention (binary-mode forward planning)

When the binary-mode path lands, each `pub fn` wrapper in `src/` will
call an `extern fn` with a stable C-ABI symbol name following
`stdlib_<module>_<fn>`. Concretely:

- `stdlib/io::println(s)` → `stdlib_io_println(ptr, len)`
- `stdlib/fs::File::open(p)` → `stdlib_fs_file_open(ptr, len, out)`
- `stdlib/vec::Vec[T]::push(self, x)` → `stdlib_vec_push_<mangled>` (per
  monomorphization — see "Generics + binaries" below)

This convention isn't enforced by the compiler yet. It documents the
shape so source-mode authors of new stdlib modules can write code that
later compiles unchanged when binaries take over.

### Generics + binaries

`Vec[T]` and `HashMap[K, V]` need per-instantiation symbols in the `.a`.
Same problem as Rust's `Vec<T>` in `.rlib`. The expected resolution
(deferred until Phase 3 ships any binary releases at all): ship a fixed
set of `T` (i32, u32, i64, u64, *u8, …) in the prebuilt `.a`; fall back
to source-mode recompile for exotic `T`. Open question; revisit when
real demand surfaces.

## Error model

All fallible APIs return `Result[T, IoError]`. `IoError` is a shared
tagged enum across `io`/`fs`/`net` so `Result` chains compose —
`Result[File, IoError]` and `Result[TcpStream, IoError]` flow through the
same combinators.

## Phase 3 scope (not yet implemented)

- Single-threaded only. No `Mutex`/`Atomic`/threading primitives.
- No async/`Future`. Blocking I/O via libc.
- No `BTreeMap`, no `Regex`, no `serde`-equivalent. Each is its own
  future package.
- macOS/arm64 primary target; Linux/x86_64 stretch (plan.md
  §"Phase 3 non-goals").
- Operator overloading is forbidden (§2.6). `Vec[T]::push(self, ...)`
  not `vec += ...`.

## How to use this package today

You can't `cpc build` it directly — function bodies are not implemented.
Use it as:

1. **Design reference** when answering "what should the API look like?"
   during Phase 3 implementation.
2. **Layout reference** when authoring a new C+ package — replace the
   stdlib API names with yours; keep the file conventions.
3. **Forward compatibility check** — once Phase 2's resolver lands,
   `cpc build` against a consumer that imports from this directory will
   exercise the import-resolution path. Errors should surface as parse /
   sema failures on the TODO bodies, not as import-resolution misses.
