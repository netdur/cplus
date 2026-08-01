# Bug 18 — Resolver's hand-rolled TOML scanner leaks comment text into linker symbols

- Status: reproduced 2026-08-01 (nm-verified symbol `_tomly____the_app.src.util.helper`)
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

## Fix

1. Extend the resolver's project-loading entry points (`load_project_full` family,
   resolver.rs:369-411) to accept the package name and per-dependency `BuildSpec` data
   from the caller.
2. In cpc's driver, pass them from the already-parsed `Manifest` at the four call sites.
3. Delete both scanners (`load_package_name`,
   `package_resolves_through_headers`'s TOML-reading half — keep the decision logic by
   calling `BuildSpec::resolves_through_headers`).

Structural companion: `issue-09-resolver-program-index.md` part B (a `ResolveConfig`
carrying manifest + target/platform explicitly) — this fix is its first slice.

## Verification

1. The repro builds with clean symbols: `nm target/debug/tomly | grep helper` shows
   `_tomly.src.util.helper`.
2. A comment-free project builds byte-identically to before the change (compare `nm`
   output).
3. Header-resolved / prebuilt deps still classify identically (the
   resolves_through_headers decision) — run the platform_deps test file:
   `cargo test -p cpc --test platform_deps`, plus full suites.
