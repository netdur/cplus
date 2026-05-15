# stdlib

C+ standard library — API-only at this point in time. Phase 3 (plan.md §"Phase 3 — Language completeness + reference library + stdlib bootstrap") fills in the bodies.

## Purpose

This is the canonical C+ stdlib package: the file layout, module names, and public API signatures that future code will fill in. It is **not buildable** today: function bodies are `// TODO: Phase 3` markers.

The directory sits at `vendor/stdlib/` at the repo root — the same on-disk shape a consumer project gets when they:

```sh
git clone https://github.com/<owner>/cplus-stdlib vendor/stdlib
```

Once Phase 3 ships, the stdlib will likely move to its own repository (`github.com/<owner>/cplus-stdlib`, per plan.md §"Phase 3C — Stdlib bootstrap"). The path inside the consumer's tree stays `vendor/stdlib/`.

## Layout

```
vendor/stdlib/
├── Cplus.toml              ← package manifest (name, version, edition; [link] empty for now)
├── README.md               ← you are here
└── src/                    ← all importable C+ code lives here
    ├── result.cplus        ← Result[T, E] — fallible computation type
    ├── io.cplus            ← print/println/eprintln/read_stdin_line
    ├── fs.cplus            ← File, open/read/write/close (libc-backed)
    ├── net.cplus           ← TcpStream, TcpListener
    ├── vec.cplus           ← Vec[T] (growable buffer)
    ├── hash_map.cplus      ← HashMap[K, V] (open addressing, linear probing)
    └── env.cplus           ← var, args (env vars + argv)
```

Consumers import modules with no extension; the resolver appends `.cplus`
and looks under the package's `src/`. So `import "stdlib/env" as env;`
in consumer code resolves to `<consumer>/vendor/stdlib/src/env.cplus`.

## Module conventions

Per the v0.0.2 package design ([plan.md §"Locked design decisions"](../../plan.md)):

- **No `.cplus` files at the package root.** Only `Cplus.toml` + `README.md` + `src/`.
- **All importable code lives under `src/`.** `import "stdlib/env"` → `<pkg>/src/env.cplus`. Sub-dirs work too: `import "stdlib/collections/vec"` → `<pkg>/src/collections/vec.cplus`.
- **`pub` controls visibility.** `pub fn` / `pub struct` / `pub enum` items are reachable cross-file (and cross-package). Non-`pub` items are private to their declaring file — cross-file access fires E0403 as usual.
- **Bundled artifacts (if any) live at `src/lib/<arch>/`.** Static-libraries shipped alongside the source for FFI-heavy packages. Stdlib doesn't ship prebuilts; this directory is absent here.
- **libc FFI is the implementation strategy.** Stdlib modules `extern fn` into libc (open/read/write/close/socket/connect/getenv/etc.) and present idiomatic C+ types (`Result`, `File`, `TcpStream`) to consumers. Users never see `extern fn`.

## Error model

All fallible APIs return `Result[T, IoError]`. `IoError` is a shared tagged enum across `io`/`fs`/`net` so `Result` chains compose — `Result[File, IoError]` and `Result[TcpStream, IoError]` flow through the same combinators.

## Phase 3 scope (not yet implemented)

- Single-threaded only. No `Mutex`/`Atomic`/threading primitives.
- No async/`Future`. Blocking I/O via libc.
- No `BTreeMap`, no `Regex`, no `serde`-equivalent. Each is its own future package.
- macOS/arm64 primary target; Linux/x86_64 stretch (plan.md §"Phase 3 non-goals").
- Operator overloading is forbidden (§2.6). `Vec[T]::push(self, ...)` not `vec += ...`.

## How to use this package today

You can't `cpc build` it directly — the function bodies are not implemented. Use it as:

1. **Design reference** when answering "what should the API look like?" during Phase 3 implementation.
2. **Layout reference** when authoring a new C+ package — replace the stdlib API names with yours; keep the file conventions.
3. **Forward compatibility check** — when Phase 2 ships, `cpc build` against a consumer that imports from this directory will exercise the import-resolution path. Errors should surface as parse / sema failures on the TODO bodies, not as import-resolution misses.
