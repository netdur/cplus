# Phase 2 — Packages MVP

> Status: shipped 2026-05-15 (v0.0.2 Slices 2A / 2B / 2C / 2D).
> Scope: a minimum-viable package system that lets a C+ project depend on vendored packages, with system libraries and prebuilt static archives declared in TOML. Importers use a strict path shape that disambiguates local files from packages at lex time.
> Out of scope: `cpc fetch`, lockfiles, SemVer resolution, transitive C+ deps, dynamic `.dylib`/`.so` packaging, cross-compilation, sandbox/capability/signing — all forward-compatible follow-ups (see Phase 2 non-goals in [plan.md](../../plan.md)).

## 1. Problem

C+ entered v0.0.2 with imports but no package concept: every cross-file `import "foo.cplus"` was file-relative, and there was no way to depend on code that wasn't physically a sibling of `main.cplus`. A stdlib couldn't exist outside the language, FFI bindings couldn't be modeled, and the `04-curl-lite` benchmark stalled because there was no way to factor reusable helpers behind an import boundary.

Three constraints shaped the design:
- **AI-first ergonomics.** The compiler's import-rejection messages had to leave no doubt about whether a path was meant to be local or a package — guesswork is a tax the agent pays every session ([proves/stats.md](../../proves/stats.md) recorded ~10 turns of SKILL.md spelunking on the 04-curl-lite run).
- **Manifest is the single source of truth.** A real package may ship as C+ source, as a prebuilt static archive, or as both. The compiler must trust the manifest's claims and reject anything that disagrees — silent fallthrough on a missing artifact is a footgun.
- **No package manager.** `cpc` doesn't fetch, doesn't resolve, doesn't sandbox. Whatever ends up under `vendor/<name>/` is what `cpc build` uses; flattening and integrity are the AI agent's (or human's) job.

## 2. Decisions

### 2.1 Three distinct package modes

A package is one of:
1. **Source-only.** `vendor/foo/src/*.cplus` only; no `[link]` table. Consumers compile the source as part of their own program.
2. **Bundled-binary.** `[link].bundled` names a static archive under `src/lib/<triple>/`. Consumers link the archive; the C+ side is a thin `extern fn` wrapper.
3. **Mixed.** Source files plus declared `[link]` requirements (system libs / frameworks / bundled archives). Consumers compile the source AND inherit the link contributions.

The first segment of an import path always maps to the dep name. The package decides what consumers see by what it ships under `src/`.

### 2.2 Import shape

Every import has exactly one syntactic shape:

```cplus
import "./module"        as alias;   // local file in the consumer's tree
import "depname/module"  as alias;   // vendored package member
```

- **No `.cplus` extension** — the resolver appends it; passing one trips E0858.
- **`./` prefix is mandatory for local files** — bare `"foo"` trips E0853.
- **First segment of a non-`./` path must be a declared dep** — unmatched name trips E0852.
- **No `..` segments** allowed inside a package — escapes trip E0859.

These rules turn import classification into a lex-time decision: the agent (and the compiler) never has to consult the filesystem to know whether `foo` is a sibling file or a vendored package.

### 2.3 Manifest schema

```toml
# Consumer
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "myapp"
path = "src/main.cplus"

[dependencies]
stdlib        = "*"          # name = "version-string"; version is parsed and ignored
curl_bindings = "0.1.0"
```

```toml
# Vendor package
[package]
name = "curl_bindings"

[link]
frameworks = ["Security"]               # macOS frameworks
libs       = ["z"]                      # system libs (-l...)
bundled    = ["libcurl.a"]              # archives shipped by THIS package
triples    = [                          # host triples the archives are built for
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
]
```

Dependency names match `[a-z][a-z0-9_]*` (E0857). This keeps the first segment of an import path unambiguous — no dots, no slashes, no uppercase.

### 2.4 The manifest-is-truth contract

Whatever the vendor `Cplus.toml` says, the filesystem must match. Both directions hard-error:

| Code  | Trigger |
|-------|---------|
| E0854 | `vendor/<name>/Cplus.toml` missing |
| E0855 | Vendor `[package].name` ≠ dir name |
| E0860 | `[link].bundled` names a file that isn't on disk for the host |
| E0861 | A `.a`/`.dylib`/`.so` exists under `src/lib/<triple>/` but isn't in `[link].bundled` |
| E0862 | Host triple not in `[link].triples` |
| E0863 | `[link].bundled` non-empty but `[link].triples` empty |

No graceful degradation, no auto-discovery. If a package author ships a binary they forgot to declare, the consumer's build fails. If they declare a binary they forgot to ship, the consumer's build fails. The compiler refuses to guess.

### 2.5 Resolution flow

```
cpc build
  │
  ├── parse Cplus.toml — extract [dependencies] dep names
  │
  ├── for each .cplus file:
  │     classify each `import "X"`:
  │       starts with "./"      → local-relative
  │       first segment ∈ deps  → vendor under vendor/<seg>/src/...
  │       otherwise             → E0852 or E0853
  │
  ├── walk [dependencies]:
  │     for each dep:
  │       load vendor/<name>/Cplus.toml  (E0854 if absent)
  │       check [package].name == name  (E0855)
  │       if [link].bundled:
  │         host in [link].triples?     (E0862)
  │         each file present?          (E0860)
  │       any binary under src/lib/?    declared in bundled? (E0861)
  │     accumulate: [-framework X] [-lY] [absolute/path/libfoo.a]
  │
  ├── sema / borrowck / monomorphize / codegen
  │
  └── clang — consumer's frameworks/libs first, then dep contributions
```

The dep walk runs on every code-emitting entry point (`cpc build`, `cpc test`, `--emit-ll-project`, the `[lib]` cdylib path) so manifest-is-truth violations are caught equally in CI loops that never reach the linker.

## 3. Implementation map

| Concern | Code |
|---------|------|
| Manifest schema + E0857/E0863 | [cplus-core/src/manifest.rs](../../cplus-core/src/manifest.rs) |
| Import classification + E0852/E0853/E0858/E0859 | [cplus-core/src/resolver.rs](../../cplus-core/src/resolver.rs) (`classify_import_path`) |
| Host-triple detection + dep walk + E0854/E0855/E0860/E0861/E0862 | [cpc/src/main.rs](../../cpc/src/main.rs) (`detect_host_triple`, `collect_dep_link_args`) |
| Smoke tests | [docs/examples/projects/tiny_source/](../examples/projects/tiny_source/), [docs/examples/projects/tiny_artifact/](../examples/projects/tiny_artifact/) |
| End-to-end test coverage | [cpc/tests/e2e.rs](../../cpc/tests/e2e.rs) — 14 Phase-2 tests (8 of them Slice 2C) |

## 4. What the smoke tests demonstrate

[`tiny_source`](../examples/projects/tiny_source/) — pure-C+ vendor package. A consumer declares `tiny = "*"`, imports `tiny/lib`, and calls `tiny::echo(42)`. Vendor's `Cplus.toml` is just `[package] name = "tiny"`. No binaries, no `[link]`. The canonical reference shape for stdlib (Phase 3).

[`tiny_artifact`](../examples/projects/tiny_artifact/) — bundled-binary vendor package. Vendor's `Cplus.toml` declares `[link] bundled = ["libtiny_artifact.a"] triples = [...]`. The implementation is a C file under `upstream/` (not built by cpc); the prebuilt `.a` lives at `src/lib/<host-triple>/libtiny_artifact.a`. The C+ side is a thin `extern fn tiny_artifact_double(...)` plus a `pub fn double(n)` wrapper. The canonical reference for `cpc-bindgen` output (Phase 4).

## 5. Forward path

What's intentionally deferred:

- **`cpc fetch`.** Today users populate `vendor/` themselves (git submodule, manual copy, AI-driven flattening). When demand for automation shows up, fetch lands as a separate command that walks transitive `[dependencies]` and writes a flattened tree.
- **Lockfile.** The contents of `vendor/` together with the manifest *are* the lockfile; integrity is whatever git gives you.
- **Transitive C+ deps.** A vendor package's own `[dependencies]` is ignored by `cpc`. The AI agent flattens them into the consumer's `Cplus.toml` and `vendor/` directory at install time. The compiler stays simple and deterministic.
- **Dynamic-loaded artifacts.** Phase 2 is `.a` only — link-time, no runtime loader dance. `.dylib`/`.so` bundling lands when a real use case shows up.
- **Cross-compile.** The dep walker uses `clang -print-target-triple` to pick the host's binaries. A `--target` flag plus a triple-aware lookup is a one-day follow-up when needed.
- **Sandbox / capabilities / signing.** Every pm.md goody lands when actual demand for it shows up.
