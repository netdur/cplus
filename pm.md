# A Package Manager: Design Notes

This is a design document, not a spec. It's opinionated, names tradeoffs out loud, and is organized to be implemented in order. Most of the design is language-agnostic; sections marked **(lang)** flag where the language you eventually target will change things.

---

## 0. What you're building

A package manager with these defining properties:

- **Decentralized identity.** A package's identity is a URL plus a version. There is no required central registry. A registry can exist as a cache or mirror, but the tool works without it.
- **Multiple packages per repo.** Identity includes a subpath, so `host.tld/user/repo/parser` and `host.tld/user/repo/lexer` are two packages from one repository.
- **Manifest-driven.** A directory is a package if and only if it contains a manifest file. The filesystem layout is not magically scanned for entry points.
- **Default-private API.** Anything not declared public in the manifest is invisible to consumers.
- **Reproducible builds.** Sandboxed, no network during build, output verified against the declared artifact contract.
- **No install-time code execution.** Fetching is byte-copying plus integrity verification. Building is a separate, sandboxed step.
- **Capability-declared.** Packages declare what they need (filesystem, network, subprocess) so consumers can audit.

Non-goals — things you are explicitly **not** building:

- A build system. Delegate to whatever the target language's build tools are.
- A test framework, formatter, or linter. Delegate.
- A registry server. The tool talks to git hosts and HTTP caches; what a community wants to run on top is their business.
- Universal language support. Start with one, design for extension.

---

## 1. Identity and versioning

### 1.1 The ID tuple

A package is identified by `(origin, path, version)`:

- **origin** — a URL host plus a base path, e.g. `github.com/sled/tools`
- **path** — a subdirectory within the origin, e.g. `parser` (or empty for the repo root)
- **version** — a SemVer string, e.g. `2.1.0`

Full canonical form: `github.com/sled/tools/parser@2.1.0`.

### 1.2 Where versions come from

Versions live on git tags. The tag format encodes the path:

- For root packages: `v1.2.3`
- For subdir packages: `<path>/v1.2.3`, e.g. `parser/v2.1.0`

This is Go modules' convention and it works. The advantage: no separate version registry to maintain. The disadvantage: you commit to git as a transport layer. You can later add tarball-over-HTTPS as a second source type.

### 1.3 Content addressing

Every resolved version carries a content hash (SHA-256 of the canonicalized source tree). The lockfile records the hash; the cache stores by hash; integrity is verified on every fetch. A tag that's been force-pushed (which shouldn't happen but does) is detected: hash mismatch → hard error.

### 1.4 Immutability

Once published — meaning: once a tag exists and has been seen by anyone — a version's content is frozen. If you need to retract, you publish a new version. The tool may support a "yanked" marker that warns when a yanked version is selected, but the bytes stay accessible by hash.

---

## 2. The manifest

### 2.1 File

One per package directory. Suggested name: `pkg.toml`. TOML, not YAML — YAML's implicit type coercion and indentation rules cost more than they save in a config language.

### 2.2 Schema (sketch)

```toml
[package]
id      = "github.com/sled/tools/parser"
version = "2.1.0"
license = "MIT OR Apache-2.0"

# (lang) The language the source is in. Determines toolchain, lint rules,
# what kind of artifact is produced, ABI rules, etc.
language = "c11"          # or "rust-2021", "go1.22", "dart3", ...

[api]
# Explicit list of what consumers can import/include/link against.
# Anything not listed here is not part of the contract.
public = [
  "include/sledparse/parser.h",
  "include/sledparse/ast.h",
]
unstable = []             # visible but not under SemVer; consumer must opt in
# Everything else in the source tree is internal.

# (lang) For languages without namespace enforcement (C, ASM), declare a
# symbol prefix. The publish step verifies every exported symbol matches.
symbol_prefix = "sledparse_"

[artifact]
# What this package produces when built. The build step must produce
# exactly this, and the publish step verifies it.
kind     = "staticlib"    # or "sharedlib", "headers-only", "module", ...
produces = "build/libsledparse.a"

[build]
# A single command run inside the sandbox. Inputs and outputs are
# constrained; environment is whitelisted.
command = "./scripts/build.sh"
inputs  = ["src/**", "include/**", "scripts/build.sh"]
toolchain = "c-toolchain >= 11"   # (lang) declared toolchain requirements

[deps.public]
# These types/symbols appear in your public API. Bumping them is part
# of your SemVer contract.
"github.com/sled/tools/types" = "^1.4"

[deps.private]
# Used internally only. Bumping them is a patch release for you.
"github.com/madler/zlib" = "^1.3"

[deps.build]
# Runs during build, not linked into the output. Code generators, etc.
"github.com/westes/flex" = "^2.6"

[capabilities]
# Declared so consumers can audit. Optionally enforced at build/run time.
build   = ["subprocess:flex", "fs:write:build/"]
runtime = ["fs:read:any"]

[conditions]
# Optional. Different artifacts/builds for different consumer contexts.
# Format must not change semantics; only swap files/flags.
[conditions.platform.linux]
build.command = "./scripts/build-linux.sh"
[conditions.platform.windows]
build.command = "./scripts/build-windows.cmd"
```

### 2.3 What's deliberately not in the manifest

- Install hooks, postinstall scripts, lifecycle scripts.
- Compiler flags, include paths, link order. (Derived from the dep graph by the tool.)
- Test commands. (Convention: `pkg test` invokes a separate, declared entry.)
- Format/lint config. (External tools' problem.)
- Registry URLs. (The tool accepts mirrors via CLI flag or env; no manifest-level pinning.)

---

## 3. Resolution

### 3.1 Inputs and outputs

Input: the root package's manifest, the manifests of its transitive dependencies (fetched as needed), and version constraints throughout.

Output: a flat list of `(id, version, content_hash, source_url)` tuples — the lockfile.

### 3.2 The algorithm

For v0.1 of your implementation, write the naive backtracking solver. It will be slow and you'll hate it, but it'll work for small graphs and you'll understand what you actually need.

For v0.3 or so, replace it with **PubGrub** (Natalie Weizenbaum's algorithm, used by pub, uv, Poetry). The paper is short and the algorithm is approachable. Reference implementations exist in Dart, Rust, and Python. Don't write your own algorithm; this is a solved problem.

### 3.3 Multi-version coexistence policy

Two stances; you must pick one (or make it configurable):

- **Pub-style:** only one version of any given package may appear in a resolved graph. Conflicts become resolution errors. Forces the dependency graph to stay clean. Cost: occasional unresolvable graphs that would have been fine.
- **Cargo-style:** multiple major versions may coexist if their types don't cross consumer boundaries. Requires the language to support this (Rust does via mangled symbols and crate-local types; most languages don't).

My recommendation: start pub-style (simpler, fewer footguns). If the language you target supports the Cargo model and you want it later, add it.

### 3.4 Public-dependency leak rule

If a type/symbol from dep X appears in your public API, X must be declared under `[deps.public]`. The tool checks this mechanically (for languages where this can be checked — Rust's `cargo-semver-checks` is the model). The rule then is: bumping a public dep's major version requires a major bump of *your* package too.

---

## 4. The lockfile

### 4.1 File

Suggested name: `pkg.lock`. TOML or canonical JSON; you want something a human can read in a diff but a machine writes.

### 4.2 Schema (sketch)

```toml
version = 1
generated-by = "pkgtool 0.4.2"

[[package]]
id       = "github.com/sled/tools/parser"
version  = "2.1.0"
source   = "git+https://github.com/sled/tools.git#parser/v2.1.0"
hash     = "sha256:abc123..."
deps     = ["github.com/sled/tools/types@1.4.2"]

[[package]]
id       = "github.com/sled/tools/types"
version  = "1.4.2"
source   = "git+https://github.com/sled/tools.git#types/v1.4.2"
hash     = "sha256:def456..."
deps     = []
```

### 4.3 Rules

- Never hand-edited. The tool refuses to start if the lockfile has been touched in a way it can't verify.
- For applications: committed to source control, required for reproducible builds.
- For libraries: optional. A library's lockfile is used for its own testing, not imposed on consumers.
- Migration: when the lockfile schema version bumps, the tool migrates in-place and warns once.

---

## 5. Fetch and cache

### 5.1 The cache

A content-addressed local store, e.g. `~/.pkgcache/<hash-prefix>/<hash>/`. Files in the cache are read-only and never mutated. Multiple projects share the cache.

### 5.2 Fetch sources, in order of preference

1. **Local cache** (by hash). If present, use it.
2. **Mirror/proxy** (configurable list). HTTPS endpoint serving content-addressed blobs.
3. **Origin git host**. Clone shallow, checkout tag, hash, store.

The lockfile's `source` field records the *canonical* origin. Mirrors are configuration; they never change identity.

### 5.3 Garbage collection

`pkg cache gc` walks all known lockfiles (configurable roots) and marks reachable hashes. Everything else is collected. There is no "auto-cleanup on install" — the cache only grows during normal use.

---

## 6. Build and sandbox

### 6.1 The artifact contract

The manifest's `[artifact]` block declares what the build produces. The tool verifies:

- The declared output file(s) exist after the build.
- For native languages (lang): every exported symbol matches the declared `symbol_prefix`.
- For module-aware languages (lang): the public module's exported names match the declared `[api].public` list.
- Hash the artifact for caching.

Mismatch → build refused, package not consumable.

### 6.2 The sandbox

Minimum guarantees during the build:

- **No network.** Period. Build inputs are the source tree plus resolved dep artifacts; if you need something else, it's a build dep.
- **Read-only mounts** of: source tree, dep artifacts, declared toolchain.
- **Writable mount** only for the declared output directory.
- **Environment** restricted to a whitelist (typically: `PATH`, `HOME=/tmp/build-home`, `TMPDIR`, plus what the manifest explicitly requests).
- **No host filesystem access** outside the above.

Implementation: on Linux, use namespaces (mount, user, network, pid). On macOS, `sandbox-exec` profiles. On Windows, this is harder; Job Objects + AppContainer is the direction. v0.4 should at minimum disable network and restrict filesystem; tighter sandboxing iterates.

Reference: read Nix's `nix-build` sandbox implementation. It's the most thought-through example.

### 6.3 Build caching

Cache key: `hash(manifest + sorted(dep hashes) + source tree hash + toolchain hash + platform)`. Same key → reuse the artifact. This means `pkg build` is fast on repeat invocations and CI doesn't redo work.

---

## 7. API surface model

### 7.1 Three tiers

- **public** — under SemVer. Removing or changing the signature of anything here is a major-version event.
- **unstable** — visible to consumers, but the consumer manifest must explicitly opt in (`unstable_apis = ["github.com/sled/tools/parser/advanced"]`). Breakage allowed in minor versions.
- **internal** — not exported. Consumers cannot import/include it. Enforced mechanically per the language's mechanism.

### 7.2 Enforcement is language-specific (lang)

| Language family | Enforcement mechanism |
|-----------------|----------------------|
| C, asm          | Symbol prefix check (`nm` filter); header inclusion path scoping |
| Rust            | `pub` keyword; `cargo-semver-checks` for diffs |
| Go              | Capitalized identifiers; `internal/` package convention |
| JS/TS           | `exports` field; consumers can only see what's listed |
| Java/Kotlin     | `module-info.java` `exports` directive |
| Dart            | (no built-in; would need a linter) |

Your tool wraps the language-appropriate mechanism in a uniform manifest section. The language adapter (a plugin or built-in module of your tool) maps `[api].public = [...]` onto whatever the language can enforce.

---

## 8. Capabilities

### 8.1 What goes in the manifest

A flat list of capability strings, scoped to build and runtime:

- `net:<host-pattern>` — can connect to matching hosts
- `fs:read:<path-pattern>`, `fs:write:<path-pattern>` — filesystem
- `subprocess:<binary-name>` — can fork binaries by name
- `ffi:<lib-pattern>` — can load native libraries
- `env:<var-name>` — reads named environment variables
- `clock:realtime`, `clock:monotonic` — clock access (paranoid, but real for some domains)

### 8.2 What the tool does with them

- **Audit:** `pkg audit` lists every capability in the resolved tree, grouped by package. Diff against a previous audit to spot scope creep.
- **Build-time enforcement:** the sandbox honors the build capability set.
- **Runtime enforcement (lang):** if the language has a runtime permission model (Deno, browser, JVM SecurityManager-like), the tool emits a manifest the runtime consumes. Most languages don't; that's fine — audit is still valuable on its own.

### 8.3 Capability changes are SemVer events

Adding a new capability is a minor version bump. Adding a `net:*` to a package that previously needed none is a major event from a security standpoint, and the tool flags it loudly during publish.

---

## 9. Multi-package repositories

This is the case your design started from, and it's first-class here.

### 9.1 Layout

```
github.com/sled/tools/
├── parser/
│   ├── pkg.toml
│   └── src/...
├── lexer/
│   ├── pkg.toml
│   └── src/...
├── types/
│   ├── pkg.toml
│   └── include/...
└── README.md         # no manifest → not a package, just a repo
```

### 9.2 Local cross-references

During development, packages in the same repo can refer to each other by relative path:

```toml
[deps.public]
"github.com/sled/tools/types" = { path = "../types" }
```

At publish time (`pkg publish`) the tool rewrites these to URL form with a version constraint inferred from the sibling's current version. Lockfiles only ever contain the URL form.

### 9.3 No special "workspace root"

A directory containing multiple packages doesn't need its own manifest. The tool auto-discovers sibling packages when invoked from a subdirectory and a `--workspace` flag is passed. This avoids pub's retrofit-workspace ergonomics issues.

### 9.4 Tag conventions

- `parser/v2.1.0` releases just the parser package.
- A multi-package coordinated release uses multiple tags on the same commit: `parser/v2.1.0`, `lexer/v2.1.0`, `types/v2.0.0`.
- Bare `v1.0.0` only means something if the repo root has a manifest.

---

## 10. Publishing

### 10.1 What publishing is

In this design, "publishing" is just **creating a git tag in the canonical form**. That's it. The tool can push to mirrors as a side effect, but the source of truth is the tag.

### 10.2 Pre-publish checks (refuses on failure, override available)

- Manifest is valid.
- Source tree is clean (no uncommitted changes).
- Build sandbox runs green.
- Artifact contract is satisfied.
- (lang) Symbol prefix / API surface checks pass.
- SemVer diff vs the previous version is consistent with the version bump:
  - Removed public API → major required.
  - Added public API → minor required.
  - Same public API, different content → patch is fine.
  - New capability added → flagged for review.

Overriding any check is allowed but recorded in commit metadata so consumers can see the override.

### 10.3 Signing (optional but recommended)

Tags can be GPG-signed (git supports it) or Sigstore-style signed via OIDC. The lockfile can record the signing identity; consumers can require signed packages via policy.

---

## 11. Implementation roadmap

Build in this order. Each phase is independently useful and lets you discover what you actually need before adding the next layer.

### v0.1 — Manifest and fetch
- Parse `pkg.toml`.
- Resolve a single direct dependency from a git URL and a tag.
- Store the fetched source somewhere predictable.
- **Skip:** transitive deps, locking, building, sandboxing.
- **Learn:** what the manifest actually wants to express.

### v0.2 — Transitive deps and naive resolution
- Walk the dep graph by recursive fetch.
- Naive solver: depth-first, fail on conflict.
- Lockfile writer.
- Content hashing.
- **Skip:** PubGrub, sandbox, capabilities.
- **Learn:** how dep graphs really tangle.

### v0.3 — PubGrub
- Replace naive solver with PubGrub.
- Real diagnostics on resolution failure.
- **Learn:** why people write papers about this.

### v0.4 — Build invocation and basic sandbox
- Run the declared build command.
- Initial sandbox: no network, restricted filesystem.
- Artifact verification.
- Build cache.
- **Learn:** how fragile "no network during build" is in practice.

### v0.5 — API surface enforcement (lang)
- Pick your first target language.
- Implement the language adapter: symbol/header checks, public-API diff.
- Pre-publish enforcement.
- **Learn:** how much language-specific knowledge actually leaks into "language-agnostic" tools.

### v0.6 — Capabilities
- Manifest parsing for capability lists.
- `pkg audit` command.
- Sandbox honors build capabilities.
- **Learn:** what real packages actually need; your initial capability vocabulary will be wrong.

### v0.7 — Multi-package repos
- Subdirectory packages.
- Local path deps with publish-time rewriting.
- `--workspace` discovery.
- **Learn:** how much of your earlier design assumed one-package-per-repo.

### v0.8 — SemVer enforcement on publish
- API-diff tooling.
- Refusal-with-override on inconsistent bumps.
- **Learn:** how much "SemVer" people actually do.

### v1.0 — Stabilize
- Freeze manifest schema (version field exists from v0.1; you've been bumping it).
- Document migration policy.
- Multiple language adapters if you're feeling brave.

---

## 12. Open questions you'll need to answer

These aren't decided in this doc. You'll want to pick before too long, but you don't need to pick them all on day one.

1. **First language?** Pick one that you can produce real artifacts in and that has a usable existing build tool you can delegate to. Rust + Cargo is "easy mode" because so much is already done; C is "hard mode" because nothing is. I'd go Rust first, port to C second.
2. **Tagged versions only, or also branch/commit deps?** Branch/commit deps are useful for development but a security hole; if you allow them, make sure they're loud in the lockfile.
3. **Pre-built binaries?** Always build from source is more reproducible; pre-built is what users want. Compromise: support binary deps for build-time tools (compilers, codegen), require source for library deps.
4. **Solver strategy on conflict?** Backtrack and report a single conflict, or backtrack and report all of them? (PubGrub does the latter and it's a usability win.)
5. **Registry-aware or purely git-driven?** If you stay purely git-driven, search and discovery are someone else's problem. If you add an optional registry protocol, design it as a cache, not as a source of truth.
6. **What's your stance on `dependency_overrides`?** Pub allows them; they're an escape hatch that papers over real conflicts. I'd ban them for libraries and allow them only at the application root, marked loudly in the lockfile.
7. **Network access during build for code generators?** Useful for things like protobuf-from-URL. Dangerous in general. My instinct: no, ever. Code generators should be regular packages.
8. **Cross-compilation as a first-class feature, or a v2 concern?** First-class is more work upfront. v2 means you'll redesign the build invocation later. Decide based on whether your target language needs it (C/Rust: yes; JS/Dart: not really).

---

## 13. References to study, in order

1. **Go modules reference** (`go.dev/ref/mod`). Read for: identity, subdirectory modules, MVS algorithm, proxy protocol. The closest existing thing to what you're building.
2. **PubGrub paper** (Natalie Weizenbaum). Read for: the solver. Implementations to crib from: `pubgrub-rs` (Rust), `dart-lang/pub` (Dart).
3. **The Cargo book** (`doc.rust-lang.org/cargo`). Read for: workspaces, features, the manifest schema, publishing flow.
4. **Nix manual: Stdenv** and **Fixed-output derivations**. Read for: what a serious build sandbox looks like, and why "fixed-output" is the right abstraction for cache-friendly fetches.
5. **NPM RFC 0006 (Package Exports)** and the `exports` field docs. Read for: declarative public-API surface in a manifest, and what conditional resolution looks like.
6. **Java's JPMS** (`module-info.java`). Read for: a serious attempt at module visibility, and how painful retrofitting it is.
7. **Pub's `pana` source code**. Read for: mechanical quality scoring you can adapt as pre-publish hygiene checks.

---

## 14. A final principle

Every existing package manager started with a small problem and grew features until it became the platform it is now. Most of the design decisions you regret in npm, pip, gem, and friends are not stupid — they're decisions that were correct for a smaller scope, that aged poorly when the ecosystem grew.

Your tool will follow the same path unless you do one specific thing: **keep the scope narrow on purpose**. Resist becoming the build system. Resist becoming the test runner. Resist becoming the registry. Resist becoming the formatter or the linter. Every one of those is a tar pit; every one of them dilutes the small thing you actually want to be excellent at, which is "given a manifest, give me a reproducible, audit-able set of artifacts."

If a feature request smells like "make this tool do more," the right answer is usually "no, but here's the extension point."

---

*Save this, change your mind about half of it as you implement v0.1, and that's normal.*