## Purpose

Precompiled-binary distribution of the C+ standard library. Same public API
as the [stdlib](../stdlib/) package — but where `stdlib` ships C+ source
that gets recompiled with the consumer, `stdlib_bin` ships:

- `src/*.cplus` — **declarations only**. `extern fn` signatures into the
  bundled `.a` + thin `pub fn` / `pub struct` wrappers. No bodies.
- `lib/<host-triple>/stdlib_bin.a` — the actual implementation, built once
  upstream by the stdlib's release pipeline.

A consumer's `cpc build` runs the same `import "stdlib_bin/<module>"`
resolution as for `stdlib`, then the build driver finds
`vendor/stdlib_bin/lib/<host-triple>/stdlib_bin.a` and adds it to the link
line. The result: consumer compile time drops to zero for stdlib code (no
recompilation), at the cost of needing a per-arch binary release.

## Layout

```
vendor/stdlib_bin/
├── Cplus.toml                          ← package manifest
├── README.md                           ← you are here
├── src/                                ← declarations only — no bodies
│   ├── result.cplus                    ← stdlib_bin/result (full source — pure enums)
│   ├── io.cplus                        ← stdlib_bin/io (decl + wrappers)
│   ├── fs.cplus                        ← stdlib_bin/fs
│   ├── net.cplus                       ← stdlib_bin/net
│   ├── vec.cplus                       ← stdlib_bin/vec
│   ├── hash_map.cplus                  ← stdlib_bin/hash_map
│   └── env.cplus                       ← stdlib_bin/env
└── lib/                                ← prebuilt static libraries, one per supported arch
    ├── aarch64-apple-darwin/
    │   └── stdlib_bin.a                ← (not yet built — Phase 3 produces this)
    └── x86_64-unknown-linux-gnu/
        └── stdlib_bin.a                ← (not yet built — Phase 3 produces this)
```

Note: `result.cplus` is full C+ source (the `Result[T, E]` and `IoError`
enums are pure tagged unions defined in C+ — there's nothing to put in the
.a for them). Every other module is the declaration + wrapper pattern.

## Build pipeline (Phase 3+)

The stdlib's own release pipeline produces `stdlib_bin.a` from the source
in [vendor/stdlib/](../stdlib/) like this (sketch):

```sh
# Once per supported arch, on a machine matching that arch:
cpc build --release --emit-ll vendor/stdlib/src/*.cplus > stdlib.ll
clang -O2 -c stdlib.ll -o stdlib.o
ar rcs stdlib_bin.a stdlib.o
# Drop into vendor/stdlib_bin/lib/<triple>/.
```

The .a is **not** built by `cpc build` of a consumer project. It's a
shipped artifact, produced upstream by whoever cuts the release.

## Symbol-naming convention

Each `pub fn` wrapper in `src/` calls an extern with a stable C-ABI symbol
name. The convention: `stdlib_<module>_<fn>`. So:

- `stdlib_bin/io::println(s)` calls `stdlib_io_println(ptr, len)`
- `stdlib_bin/fs::File::open(p)` calls `stdlib_fs_file_open(ptr, len, out)`
- `stdlib_bin/vec::Vec[T]::push(self, x)` calls `stdlib_vec_push_<mangled>` (per
  monomorphization)

For generics like `Vec[T]`, the .a needs per-instantiation symbols. Same
problem as Rust's `Vec<T>` in `.rlib` files. Phase 3+ decides the
monomorphization-export contract (probably: a fixed set of `T` shipped in
the .a, fall back to source-recompile via the source `stdlib` for
exotic `T`).

## When to use which

| Use stdlib | Use stdlib_bin |
|---|---|
| You want zero binary deps and full source visibility | You want fast incremental compiles |
| You're cross-compiling to an unsupported arch | Your arch is officially supported |
| You're debugging stdlib internals | You trust the release |
| Default for development | Default for CI / production builds |

Both can coexist in the same `vendor/` — but the consumer picks one in
`[dependencies]`, not both (the import paths differ — `stdlib/...` vs
`stdlib_bin/...`).
