# stdlib

The C+ standard library: collections, text, errors, I/O, concurrency, async
runtime pieces, and platform shims — including `platform`, which names the OS,
architecture and OS version the app is actually on.

```toml
[dependencies]
stdlib = "*"
```

```cplus
import "stdlib/vec" as vec;
import "stdlib/text" as text;
import "stdlib/option" as option;
import "stdlib/result" as result;
```

Import one module at a time (`stdlib/<module>` → `src/<module>.cplus`). There
is no single “import everything” umbrella for app code; `src/stdlib.cplus` is
only the `cpc test` entry that pulls modules for unit discovery.

## Docs

| File | Role |
|---|---|
| [docs/tutorial.md](docs/tutorial.md) | Fast path — depend and use the common modules |
| [docs/guide.md](docs/guide.md) | Module map, errors, ownership, platforms, gotchas |
| [docs/ref.md](docs/ref.md) | Per-module API manual (catalog + signatures) |

## Layout

```
vendor/stdlib/
├── Cplus.toml
├── README.md
├── docs/                 ← package documentation (tutorial / guide / ref)
├── tests/
│   └── lang_e2e.rs       ← archived Rust harness (not run by cpc test)
└── src/
    ├── *.cplus           ← modules (import "stdlib/<name>")
    ├── stdlib.cplus      ← cpc test umbrella
    └── lib/<triple>/     ← optional future prebuilt .a slots
```

## Tests

Unit tests: `#[test]` in `src/*.cplus`, discovered via `src/stdlib.cplus`.

```
cd vendor/stdlib && cpc test
```

`tests/lang_e2e.rs` is an archived Rust integration harness (builds temp C+
packages with `cpc`). It is not run by `cpc test` until re-wired.


## Distribution modes

Today the package is **source-only**. Future binary / mixed modes are declared
in `Cplus.toml` via `[link].bundled` / `[link].triples` and verified under
`src/lib/<triple>/` (see `Cplus.toml` comments and the package design notes in
the repo plans). Import paths stay `stdlib/<module>` either way.
