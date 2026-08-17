# Operations

Exactly what each command does, in the order it does it. The concepts
(identity, spec forms, siblings, the store) are defined in
[model.md](model.md).

## The commands

```text
cpc pm install [DIR]      resolve DIR/Cplus.toml deps into the store
cpc pm update  [DIR]      the same materialization as install (pre-1.0)
cpc pm add DIR NAME [SPEC]  write NAME + its declared closure into the
                          manifest, then install (see below)
cpc pm remove DIR NAME    delete DIR/vendor/NAME (the project copy only)
cpc pm manifest [DIR]     parse DIR/Cplus.toml and print the pm's view as JSON

install/update flags:
  --local                install into DIR/vendor/ instead of the store
  --store DIR            store root (default $CPLUS_HOME, else ~/.cplus)
  --cache DIR            clone cache (default <store>/cache)
  --repo-url URL         override every clone URL (a local path works —
                          offline installs, tests)
  --toolchain-repo R     toolchain monorepo (`cpc pm` supplies this)
  --toolchain-version V  toolchain version (`cpc pm` supplies this)
```

`DIR` defaults to the current directory, matching `cpc build`. All commands
are also available as the standalone `cplus-pm` binary — the difference is
that `cpc pm` passes the toolchain context (repo + version) automatically,
while the standalone binary takes it from the `--toolchain-*` flags. Without
a context, bare `*` deps cannot resolve and global install has no tier.

## The store

```text
~/.cplus/                        ($CPLUS_HOME overrides the root)
  cache/                         disposable git clones — safe to delete
  tags/<repo>/<tag>              first-seen commit per release tag (D8)
  v0.0.27/vendor/<name>/         the store tier: one package set per line
```

The tier is derived from the toolchain version: exact pre-1.0 (`v0.0.27`),
`major.minor` post-1.0 (`v1.2`). A different toolchain version is a
different universe the running binary never looks at (D13).

## install

1. **Load `DIR/Cplus.toml`.** Dependency names are validated
   (`[a-z][a-z0-9_]*`) and every `[<platform>.dependencies]` section is
   merged in (the union rule, model.md §5). A bad name or an E0869-style
   duplicate fails here, before anything touches the network or disk.
2. **Queue the root dependencies.** A pinned tree-URL stands on its own; a
   bare `*` resolves through the toolchain context to the toolchain repo at
   the toolchain version (D15). A bare root dep with no context is an
   error.
3. **Walk breadth-first.** The first occurrence of a name wins; order is
   deterministic (FIFO queue, each manifest iterated alphabetically), so
   the root manifest wins every name it declares. A later occurrence with a
   **different** pin is reported as a warning naming both sides (D9) and
   skipped — never silently dropped, never an error.
4. **Pick the destination** (D16). Default: the store tier. If the store
   already holds this name from a *different* pin, this project's copy goes
   to `<project>/vendor/<name>` instead and a warning says so — divergence
   creates locality, agreement shares. `--local` sends everything to the
   project.
5. **Decide fresh vs present.** Present = the destination's `.cplus-vendor`
   stamp matches the pin, includes a recorded commit, and the package's
   `Cplus.toml` loads. Present packages are left byte-for-byte untouched.
   Otherwise: ensure the cached checkout exists (one shallow clone per
   repo+tag), check the tag record (below), verify
   `<checkout>/<subpath>/Cplus.toml` exists, replace the destination
   wholesale, write the stamp.
6. **Walk the package's own manifest** (present or fresh alike), queueing
   its dependencies: siblings inherit this package's repo and tag; pinned
   URLs point wherever they point.

The report lists every package as `installed` or `present` with its
destination (`store` / `vendor/`); warnings go to stderr.

### The tag record (D8)

The first time a release tag is fetched, the commit it resolved to is
recorded at `<store root>/tags/<repo>/<tag>` — beside the tiers, not in
`cache/`, so purging the cache forgets nothing. Every later fetch of that
tag is compared against the record; a mismatch is a **hard error** ("a
release tag is immutable"), never silently absorbed. Accepting a moved tag
is a deliberate act: delete the record and reinstall.

Install with a warm cache does no network at all and stays trusting — the
comparison happens exactly where a fetch already happened.

### The stamp

Each installed package carries `.cplus-vendor`, two lines:

```
github.com/netdur/cplus@0.0.27 vendor/stdlib     ← the pin
0b7d0716…                                        ← the commit the tag resolved to
```

The pin line is the source of truth for "is this already installed?" — it
records the *pin*, which a package's own `[package].version` does not. The
commit line makes "which bytes is this?" a one-line read. A stamp without a
commit line (pre-D8) triggers one reinstall to record it.

### The incremental matrix

| State of the destination | install does |
|---|---|
| stamp matches the pin, commit recorded, manifest loads | nothing — local edits survive |
| directory missing | fetch and copy |
| stamp missing (hand-copied dir) | replace |
| stamp differs at the store | leave the store; vendor this project's pin locally |
| stamp differs at the project (`--local`, or pin changed) | replace — **local edits are lost** |
| stamp lacks the commit line (pre-D8) | replace once, recording it |
| stamp matches but `Cplus.toml` is broken | replace |

Working *on* a vendored package is not the pm's mode — set `[build] dev =
true` in that package and `cpc build` compiles it from `src/`; the pm
leaves it alone as long as the pin doesn't change.

## update

Pre-1.0, `update` is `install` under another name: every spec is an exact
pin and the tier is exact, so there is nothing to advance — updating a
dependency means editing its pin and re-running. Post-1.0 the verbs split
(D14): `install` stays exact, `update` becomes the one verb that looks for
a newer patch within the line.

## add

`add DIR NAME [SPEC]` writes the dependency into `DIR/Cplus.toml` and runs
the install materialization. SPEC defaults to `*` (the toolchain package at
the toolchain version); a tree-URL pins a third-party package. The package
is fetched (one cached clone, tag record checked) and **its own manifest is
the source of truth**: its `[dependencies]` closure lands in the project's
base table, and its `[<platform>.dependencies]` sections land in the
project's matching sections — but only for the project's **target set**:
every platform the project manifest already mentions (a platform entry or
an existing section), the host when it mentions none, plus any `--platform`
flags. A third-party package's bare siblings are rewritten as pinned URLs
at its repo — writing `*` would re-resolve them against the toolchain repo.

The edit is surgical (`toml_edit`): comments and formatting survive, and
existing entries are never modified — a differing spec is reported and
kept, because the manifest is the user's file. Idempotent: re-running with
another `--platform` fills only the missing sections.

## remove

Validates `NAME` as a package name (the same `[a-z][a-z0-9_]*` rule — this
is what keeps the deletion inside `vendor/`), then deletes
`DIR/vendor/NAME/`. The shared store is never touched by remove. It does
not edit the manifest: if the dependency is still declared, the next
`install` restores it.

## manifest

Prints the pm's parsed view of `DIR/Cplus.toml` as JSON: `name`, `version`,
and the merged dependency map (base table plus all platform sections). It
is the merged view, not the raw file. The argument may be a project
directory or a direct path to a `Cplus.toml`.

## How the build finds packages

Resolution order, identical for import resolution and link-argument
collection (implemented in `cplus-core::resolver` and kept in lockstep with
this crate's `store` module):

1. `<project>/vendor/<name>` — local always wins: dev mode, divergent
   pins, deliberate vendoring.
2. the monorepo sibling (`<project>/../<name>`) — packages developing
   inside the C+ monorepo itself.
3. `~/.cplus/<tier>/vendor/<name>` — the store.

## The security boundary

Three fences keep a fetched package from writing outside its destination:

1. **Names are single path components.** `[a-z][a-z0-9_]*` admits no `/`,
   `..`, or absolute prefix. Enforced on every manifest load (transitive
   ones included) and on `remove`'s argument.
2. **Subpaths are contained.** A tree-URL's subpath is rejected if any
   component is `..`, absolute, or a prefix — it is joined onto the
   checkout root and must stay inside it.
3. **The copy skips symlinks** (and `.git`). A package cannot pull an
   external file in through a link, or redirect the copy walk out of its
   own tree via a symlinked directory.

## Errors

| Condition | Error |
|---|---|
| dep name not `[a-z][a-z0-9_]*` | invalid dependency name (load-time) |
| same name in base + platform section, or two sections with different specs | conflicting dependency (mirrors E0869) |
| bare root dep, no toolchain context | "has no source URL; pin it … or supply the toolchain context" |
| global install, no toolchain context | "global install needs the toolchain version … or install with --local" |
| no `--store`, no `$CPLUS_HOME`, no home | "no store location" |
| a release tag no longer matches its record | "tag `vX` of REPO moved from A to B; a release tag is immutable" |
| pinned URL without `@version`, `/tree/`, or host/owner/repo | spec parse error naming the missing piece |
| subpath escapes the repo | unsafe subpath |
| tag or repo unreachable | the `git clone` command line and its stderr |
| `<subpath>/Cplus.toml` absent at the tag | "package was not found in the fetched repo at PATH" |
| `remove` of a package not in `vendor/` | "not installed" |

Every error is printed as a single `error:` line; the process exits
nonzero. Losing version requests and divergent pins are **warnings** on
stderr, not errors.
