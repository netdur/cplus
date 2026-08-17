# The package model

What a C+ package is to the package manager, and how a `[dependencies]` line
names one. This file is the *why*; the exact command behavior is in
[operations.md](operations.md), and the record of what was decided (and what
is still open) is in [decisions.md](decisions.md).

## 1. Scope

`cplus-pm` materializes packages: **install**, **update**, **remove**. The
whole loop is: read `Cplus.toml` → fetch each dependency's repo at its
pinned tag → copy the package's subtree into place. "Into place" is the
per-user store (`~/.cplus/<tier>/vendor/<name>` — shared by every project
on the machine) by default, or the project's own `vendor/<name>/` with
`--local`; builds look in the project first, then the store.

Non-goals, permanently out of scope here:

- **Building.** `cpc build` compiles and links; the pm never runs a build.
- **Publishing.** Releasing a package is pushing a git tag.
- **A registry server.** Identity is a git URL; there is no central index.
- **Auditing** (capabilities, symbol enforcement, API diffing).

The pm has no dependency on the compiler crates. It ships as `cpc pm`
inside the toolchain — which passes it the toolchain's identity (repo +
version), the context that resolves bare `*` deps and names the store
tier — and as the standalone `cplus-pm` binary, which takes the same
identity from flags.

## 2. Identity: a subtree of a repo at a tag

A package is identified by three coordinates:

| Coordinate | Example | Meaning |
|---|---|---|
| repo | `github.com/netdur/cplus` | the git repository (host/owner/name) |
| subpath | `vendor/stdlib` | the package's directory inside it (may be empty: the repo root) |
| version | `0.0.26` | selects the repo-wide git tag `v0.0.26` |

The unit that is fetched is the **repo at a tag**, never a single directory:
a C+ monorepo tags the whole repository `v<version>`, and every package
inside it shares that one tag. One clone therefore serves every dependency
drawn from the same `(repo, version)`.

There are no per-package tags and no version ranges. A version is exact,
and a release tag is immutable — the first commit a tag is seen at is
recorded under the store root, and a later fetch that disagrees is a hard
error (D8).

The **store tier** extends this identity machine-wide: packages install
into `~/.cplus/<tier>/vendor/<name>`, where the tier is the compatibility
line of the running toolchain — the exact version pre-1.0 (`v0.0.27`),
`major.minor` post-1.0 (`v1.2`). A toolchain can only see its own tier, so
cross-version drift is impossible by construction (D13).

## 3. The two spec forms

A `[dependencies]` value is one of two strings.

**Pinned — a git tree-URL with an `@version` suffix.** This is what a
project's direct dependencies look like, and what `cpc init` writes:

```toml
[dependencies]
stdlib = "https://github.com/netdur/cplus/tree/main/vendor/stdlib@0.0.26"
```

The URL is exactly what the GitHub web UI shows for the package's folder,
plus the version. It parses as repo `github.com/netdur/cplus`, subpath
`vendor/stdlib`, version `0.0.26`. The `https://` scheme and a trailing
`.git` on the repo are optional. The ref inside the URL (`main` here) is
**informational only** — the fetch checks out the tag `v0.0.26`, never that
ref; the ref exists so the string stays a working browser link.

**Sibling — a bare `*`.** This is how a package inside a monorepo declares
its own dependencies on the packages beside it:

```toml
# vendor/appkit/Cplus.toml
[dependencies]
stdlib = "*"
objc   = "*"
```

A sibling names no repo, so it inherits the repo and tag of the package that
declared it and resolves to `<sibling-root>/<name>` in the same checkout
(`vendor/objc` beside `vendor/appkit`).

At the **root** of a project, a bare `*` resolves through the **toolchain
context** instead (D15): `cpc pm` passes its own repo and version, so
`stdlib = "*"` means "the toolchain's stdlib at the toolchain's version" —
which is why `cpc init` scaffolds exactly that line, and why official
packages are version-locked to the compiler by construction. Without a
context (the standalone binary, no flags) a bare root dep has no repo and
is an error.

A bare version (`stdlib = "0.0.25"`) parses as a sibling too, and its
version is **ignored**: a sibling lives in the parent's checkout, and that
checkout is at one tag. The monorepo has one version; a sibling cannot pick
a different one.

## 4. Two readers of `[dependencies]`

The same table is read by two tools with different questions:

- **The compiler** cares about the **names** (and their platform scoping):
  which packages exist, so every `import` in the build can be validated
  against one flat set. It never parses the values as URLs.
- **The pm** cares about the **values**: where each name is fetched from.

The compiler's rule is normative and documented in
[docs/lang/packages.md](../../docs/lang/packages.md): resolution is **flat**.
The root manifest names the complete bill of materials — a backend's
transitive closure included — and the resolver does not read a dependency's
own manifest to find more.

The pm, by contrast, **does** walk each vendored package's own manifest and
installs the siblings it declares. In a correctly-declared project the walk
discovers nothing (every name was already a root dependency); its job is to
guarantee `vendor/` is complete from one clone regardless. If the root
manifest under-declares, the pm still materializes the missing package, and
the failure surfaces at compile time as a named missing dependency rather
than as a missing directory. The walk is a safety net, not a license to
under-declare.

## 5. Platforms: install the union

`[<platform>.dependencies]` sections (`[macos.dependencies]`, …) are merged
into the set the pm installs. `vendor/` is committed and must build on every
OS the manifest supports, so **install fetches the union of all platforms**;
filtering by the active platform is the build driver's job. The duplicate
rules mirror the compiler's E0869: a name in both the base table and a
platform section is a conflict, and two platform sections may share a name
only with an identical spec.

## 6. Source and binary packages are the same thing

The pm never distinguishes a source package from a binary-backed one,
because the compiler doesn't: `cpc` consumes `.cplus` files, and whether a
function has a body (compile it) or is a declaration backed by a `[link]`
library (link it) is visible in the code, not in any flag. The pm copies
whatever the package's subtree contains and is done.

Everything binary-shaped happens on the build side: `[build] prebuild` /
`dev`, slice layout under `lib/<triple>/`, and the vendor validation errors
(E0854/E0855/E0860/E0861) all belong to `cpc build`. The division of labor
is: **the pm places, the build validates.**
