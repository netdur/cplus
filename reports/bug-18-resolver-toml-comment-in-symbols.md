# Bug 18 — Resolver's hand-rolled TOML scanner leaks comment text into linker symbols

- Status: FIXED 2026-08-02, commit 8054dc6 — both scanners deleted; the resolver calls
  `manifest::load`
- Status (original): reproduced 2026-08-01 (nm-verified symbol `_tomly____the_app.src.util.helper`)
- Severity: wrong symbols (package identity diverges between subsystems)
- Area: resolver (`cplus-core/src/resolver.rs`), duplicating manifest (`manifest.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B18)

Context for the fixer: `manifest.rs` owns real `Cplus.toml` parsing. The resolver does not
use it — it re-scans the file with two hand-rolled string scanners, and one of them keeps
trailing comments in the package name. The package name is the mangled-symbol prefix, the
self-import rule input, and the prebuilt-archive linkage key — so the resolver's identity
for the package disagrees with everyone else's. Build `cargo build --release`; binary
`target/release/cpc`. Line numbers from 2026-08-01.

## Reproduction

Project `tomly/` — `Cplus.toml` (note the comment):

```toml
[package]
name = "tomly" # the app
```

`src/main.cplus`:

```cplus
import "./util" as util;
fn main() -> i32 { return util::helper(); }
```

`src/util.cplus`:

```cplus
fn helper() -> i32 { return 2; }
```

```
$ target/release/cpc build && nm target/debug/tomly | grep helper
... _tomly____the_app.src.util.helper ...
```

Expected symbol: `_tomly.src.util.helper`. The comment survived into the package
identity.

## Root cause

Two hand-rolled scanners in the resolver:

- `load_package_name` (resolver.rs:1009-1036): `v.trim().trim_matches('"').trim()` on the
  raw line — keeps `# the app`, which is then sanitized into `____the_app`.
- `package_resolves_through_headers` (resolver.rs:1502-1539): line 1538 is a verbatim
  duplicate of `manifest.rs:209-211` (`BuildSpec::resolves_through_headers`).

The in-code excuse ("would pull the manifest module into the resolver's dependency
surface") is void: same crate, and every driver call site already holds the parsed
`Manifest` (cpc/src/main.rs:2553, 3431, 3669, 4177).

## Fix — the smaller version of the same move

Both scanners are deleted and both functions now call `crate::manifest::load`, which is
the real parser, and `BuildSpec::resolves_through_headers`, which is the real decision.
The report's excuse-refutation is the whole fix: it is the same crate, so there is nothing
to thread.

Threading the already-parsed `Manifest` down from the driver (steps 1-2) is a strictly
larger change — four call sites plus the `load_project_full` family — and its value is
avoiding a re-parse, not correctness. That is issue-09's `ResolveConfig`, and this leaves
it exactly as far along as it was.

Structural companion: `issue-09-resolver-program-index.md` part B (a `ResolveConfig`
carrying manifest + target/platform explicitly) — this fix is its first slice.

## Verification

1. DONE: `nm` shows `_tomly.src.util.helper`. Pinned as
   `a_comment_after_the_package_name_stays_out_of_symbols` in cpc/tests/e2e.rs.
2. DONE: a comment-free project's `nm` output is byte-identical before and after
   (diffed the sorted symbol tables).
3. DONE: `cargo test -p cpc --test platform_deps` — 5 passed; full suites green; every
   vendor package still produces identical diagnostics.
