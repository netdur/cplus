# cplus-pm

The C+ package manager: it materializes the packages a `Cplus.toml`
declares — into the per-user store (`~/.cplus/<tier>/vendor/`, shared by
every project on the machine) by default, or into the project's own
`vendor/` with `--local`. Install, update, add, remove — nothing else. It does
not build, publish, or audit, and it has no dependency on the compiler
crates. It ships as `cpc pm` inside the toolchain (which supplies the
toolchain context automatically) and as this standalone `cplus-pm` binary.

```text
cpc pm install [DIR]      resolve DIR/Cplus.toml deps into the store
cpc pm update  [DIR]      the same materialization as install (pre-1.0)
cpc pm add DIR NAME [SPEC]  write NAME + its closure into the manifest, then install
cpc pm remove DIR NAME    delete DIR/vendor/NAME (never touches the store)
cpc pm manifest [DIR]     parse DIR/Cplus.toml, print the pm's merged view as JSON

install/update flags:  --local  --store DIR  --cache DIR  --repo-url URL
                       --toolchain-repo R  --toolchain-version V
```

A dependency is either the toolchain's own package (bare `*` — resolved to
the toolchain repo at the toolchain version, so stdlib is version-locked to
the compiler by construction) or a directory of a git repo at a repo-wide
version tag, written as the folder's browser URL plus an exact version:

```toml
[dependencies]
stdlib = "*"
parser = "https://github.com/acme/tools/tree/main/parser@1.4.2"
```

`install` clones each repo at its tag (once, into `~/.cplus/cache/`),
copies the package subtree into the store tier, records the tag's commit
(a re-pushed tag is a hard error), and walks the package's own manifest
for its siblings. Builds resolve project `vendor/` first, then the store —
local always wins. Installs are incremental and deterministic; there are
no version ranges, no solver, and no lockfile — every spec is an exact
pin, so the manifest is the lockfile.

## Docs

- [docs/model.md](docs/model.md) — what a package is, the two spec forms,
  siblings, platforms, and how the pm's view relates to the compiler's.
- [docs/operations.md](docs/operations.md) — exactly what each command does:
  the cache, the `.cplus-vendor` stamp, the incremental rules, the security
  boundary, the errors.
- [docs/decisions.md](docs/decisions.md) — the decision record: why there is
  no lockfile, how conflicts resolve, what is deliberately absent and what
  is an open gap. **Read this before proposing a feature.**

The language-level view of packages (manifest shape, platform sections, how
imports resolve against `vendor/`) is
[docs/lang/packages.md](../docs/lang/packages.md).

## Tests

```text
cargo test -p cplus-pm
```

Unit tests live in each module; `tests/install.rs` is an offline e2e that
builds a throwaway git monorepo, tags it, and installs against it through
the same CLI path `cpc pm` uses.
