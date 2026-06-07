# cplus-pm

A small, standalone tool that **manages C+ packages in a project's `vendor/`
folder**: install, remove, update. It does not build, publish, audit, or inspect
symbols — those are separate concerns (or not concerns at all). It has no
dependency on the C+ compiler. See `plans/pm.md` for the design.

## What it does

```text
cplus-pm install DIR              resolve deps and place them in DIR/vendor/
cplus-pm remove DIR NAME          delete DIR/vendor/NAME
cplus-pm update DIR               re-resolve and refresh DIR/vendor/
```

`install` reads `DIR/pkg.toml`, resolves the dependency graph (transitively),
fetches each package at its pinned version from its git tag, verifies its
content hash, copies it into `DIR/vendor/<name>/`, and writes `DIR/pkg.lock` so
the result is reproducible. `<name>` is the package's import name (its subpath
leaf), matching how C+ imports resolve (`import "parser/..."` -> `vendor/parser`).

Lower-level commands used by the above (and useful on their own):

```text
cplus-pm manifest [PATH]          parse pkg.toml and print normalized JSON
cplus-pm resolve PATH             resolve transitive deps, print lockfile JSON
cplus-pm lock PATH [OUT]          resolve and write pkg.lock
cplus-pm fetch ID VERSION         fetch one tagged package into the cache
cplus-pm fetch-dep PATH DEP_ID    resolve + fetch one direct dependency
cplus-pm tag ID VERSION           print the canonical git tag for ID/VERSION
```

## The manifest

`pkg.toml` is deliberately small -- identity and dependencies:

```toml
[package]
id      = "github.com/sled/tools/parser"
version = "2.1.0"

[deps.public]
"github.com/sled/tools/types" = "^1.4"
```

There is **no source-vs-binary flag and no artifact/build/capability/API
sections**. The compiler doesn't distinguish a source package from a
binary-backed one -- it just consumes `.cplus` files (definitions, or `extern`
declarations plus a linked library) -- so the manifest doesn't either. (Unknown
tables in older manifests are ignored.)

## Scope

In scope: identity, versioning (git tags), resolution (`pubgrub`), content
addressing (SHA-256 of the canonical source tree), a shared content-addressed
cache, the lockfile, and `vendor/` install/remove/update.

Out of scope (a separate tool, or not at all): publishing, capability auditing,
running builds, `nm`/symbol-prefix API enforcement, SemVer publish checks,
workspace auto-discovery. Building a package into a distributable **binary**
(declarations + a bundled shared library, AAR-style) is a separate concern
described in `plans/pm.md`, not this tool's job.
