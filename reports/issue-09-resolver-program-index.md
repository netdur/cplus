# Issue 09 — Resolver: one ProgramIndex, one prefix resolver, one explicit config

- Status: PARTIAL 2026-08-02, commit <pending> — part (B) done; (A) and (C) not
  done, see "What is still open"
- Type: structural consolidation
- Area: `cplus-core/src/resolver.rs`; driver call sites in `cpc/src/main.rs`; consumers
  `graph.rs`, `sema.rs`
- Effort: M
- Retires / prevents: bug-17, bug-18, bug-22 (and enables bug-19's clean fix); the
  four-error-universes import inconsistency; the side-effecting global target state
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #9, §2 resolver rows)

## Problem

Three accretions in the resolver produce user-visible inconsistencies: (A) per-file
RewriteCtx clones four whole-project maps whose pub-only shape makes unknown-vs-private
undecidable; (B) the same "resolve `alias::name`" logic is inlined at 6+ expression arms
with divergent failure behavior; (C) project configuration (package name, dep specs,
target/platform) is re-derived from hand-rolled TOML scans and side-effecting process
globals instead of being passed in by the driver that already has it.

## Current state

(A) RewriteCtx (resolver.rs:2015-2019): every file deep-clones `pub_items`,
`pub_methods`, `item_kind`, `alias_targets` (plus `local_items`), each keyed differently;
pub-only data is why nonexistent methods report as "private" (bug-22; pre-pass collects
pub methods only at 1878-1885, items got the UnknownItem/PrivateAccess split at 76-86 +
2129-2144 but methods never did).

(B) Inline prefix resolution — `split_once("::")` + imports.get + check_pub_item +
qualify — at: 2-seg call path 3045-3061 (silent fall-through → sema E0303), 3-seg Path
3097-3101 (E0402), struct literal 3107-3124 (no else → sema E0303), GenericStructLit
3149-3158, GenericEnumCall 3186-3195, Pattern::Variant 3245-3255, rewrite_type_name
2547-2553 (E0402). Same typo, two error universes. Alias-facade re-exports resolve for
plain struct literals (3119-3123 consults `resolve_alias_target`) but not generic ones.

(C) Config by inference: two hand-rolled TOML scanners (1009-1036 `load_package_name` —
bug-18's comment leak; 1502-1539 duplicating manifest.rs:209-211);
`classify_import_path` reads Mutex'd globals `active_target()` (1314-1326),
`active_platform()` (1455-1470), `platform_gated_dep()` (1274-1283) — populated as a
SIDE EFFECT of cpc's `active_dep_names(&m)` (cpc/src/main.rs:1371-1384) that 8+ driver
sites must remember to call first; entry-point mode puns (`deps: None` vs `Some(&[])`
flips extension rules and escape checks, 369-419); vendor-sibling fallback (1367-1380)
silently retries `<root>/../<dep>/src/` for every project; `ImportNotFound` never lists
attempted roots. Also in this family: file identity re-inferred from path shape
(header_path_as_source 1655-1673, package_relative 1695-1711, path_is_under_vendor
1691-1693) though classification KNEW the package; three subsystems know the file-id
encoding (resolver derives 1589-1642, sema inverts it in `import_path_of` sema.rs:429-436,
graph re-joins via `replace("::", ".")` graph.rs:1312); the `first == "stdlib"` hardcoded
target gate (1314) that no other package can use; the second builtin list
(`is_builtin` 3270-3288) whose `"println"` entry shadows user `fn println` (checked
before local_items at 2946-2952 → orphaned definition + E0905 calls).

## Target design

(A) `ProgramIndex { fid -> name -> ItemMeta { kind, exported, methods: name -> exported } }`
built once at merge, BORROWED by every file's rewrite (no clones). Full index — not
pub-only — so unknown/private/pub is a three-way lookup (fixes bug-22, enables bug-19).
Exportedness is STAMPED on qualified AST items so graph and sema read a fact instead of
re-deriving (fixes bug-17's class; deletes the third predicate copy).

(B) One helper:
`resolve_prefixed(&self, name, span) -> Resolved::{Local, External{fid, name}, UnknownPrefix}`
used by all seven arms; every miss is the SAME diagnostic (E0402 with the alias name);
alias-facade resolution inside the helper so all spellings behave alike.

(C) `ResolveConfig { package_name, deps: Vec<DepSpec /* from manifest BuildSpec */>,
target, platform, search_roots, mode: Mode::{SingleFile, Project} }` built by the driver
from the parsed Manifest at the four call sites (cpc/src/main.rs:2553, 3431, 3669, 4177).
Deletes both TOML scanners, the global-population ordering hazard, and the None/Some
pun; the sibling fallback becomes an explicit search root and ImportNotFound lists the
roots it tried. File identity: classification returns `(path, PackageIdentity)`;
`derive_file_id` consumes it; `LoadedProject` carries `fid -> import_path` for sema and
graph. Drop `"println"` from `is_builtin` (sema owns that gate).

## Migration plan

1. (A) Build ProgramIndex at merge; convert RewriteCtx to borrow it; three-way method
   lookup (bug-22 fix + tests); stamp exportedness; fix graph's ten sites to read the
   stamp (bug-17 fix + tests).
2. (B) Introduce `resolve_prefixed`; convert the seven arms one by one (each conversion
   is a diagnostic-consistency fix; add the typo-in-each-position negative tests).
3. (C) Introduce ResolveConfig; driver passes Manifest data (bug-18 fix + nm test);
   remove global reads from classify_import_path; explicit search roots + enriched
   ImportNotFound; file-identity plumbing; delete the scanners and the println entry.

## Verification

- bug-17/18/22 repros as tests; a typo'd alias in all seven syntactic positions yields
  the SAME error code.
- The 8+ driver sites: grep cpc/src/main.rs for `load_project` calls and confirm each
  passes the config (the compiler now fails to build if one is missed — that is the
  point).
- Full suites + `cargo test -p cpc --test platform_deps` (platform-gated deps are the
  риskiest consumer of (C)).

## Risks and constraints

- Parallel/multi-target builds become possible after (C) — do not attempt them here;
  just remove the blockers.
- `_`-privacy semantics must not change; only WHERE they are computed.
- Keep `import "x" as _;` discard-alias and EXT.2 extension-gate behavior pinned by
  their existing tests throughout.

## Outcome — part (B), one prefix resolver

```rust
fn resolve_item_name(&self, name: &str, span: Span) -> Result<Option<String>, ResolveError>
```

`Ok(Some(qualified))` when it resolved, `Ok(None)` when the name is neither
prefixed nor local (a primitive, a builtin, a generic parameter — leave it
alone), `Err(UnknownPrefix)` when the prefix is not an import. Five arms now ask
it: struct literal, generic struct literal, generic enum call, variant pattern
and `rewrite_type_name`.

Two inconsistencies disappear with the duplication:

- **One typo, one error.** Four of the five arms had no `else` branch: a
  reference through an unknown prefix fell through unqualified, and sema then
  reported E0303 "unknown type `wrong::Point`" — naming a type the user never
  wrote — while the same typo in a type position was E0402 "unknown import
  prefix", which names the actual mistake. All five are E0402 now, pinned by
  e2e `a_typoed_import_prefix_reports_the_same_error_in_every_position`.
- **Alias facades resolve in every spelling.** The re-export hop
  (`resolve_alias_target`) was consulted for a plain struct literal only, so a
  module re-exporting another module's generic type resolved as `Holder` in one
  spelling and `dep::Holder` in the other. It is inside the helper now.

The two remaining `split_once("::")` sites in the resolver are a different
question (an import path's own shape, and a 2/3-segment CALL path), not item
resolution.

## What is still open

- **(A) ProgramIndex.** Every file still deep-clones four whole-project maps
  into its `RewriteCtx`, and the pub-only shape is why a nonexistent method
  reports as private. bug-22 added the existence table beside `pub_methods`, so
  the user-visible half is fixed; the clone-per-file and the exportedness stamp
  (which would delete graph's re-derivation, bug-17's class) are not.
- **(C) ResolveConfig.** Configuration is still re-derived from process globals
  populated as a side effect of a driver call that eight call sites must
  remember to make first. bug-18 deleted both hand-rolled TOML scanners (the
  resolver reads `manifest::load` now), which was the sharp edge; the ordering
  hazard, the `deps: None`/`Some(&[])` mode pun and the silent vendor-sibling
  fallback remain.

Both are threading changes across the driver and the graph, not local edits, and
neither is a prerequisite for (B).

## Verification (as run)

- e2e `a_typoed_import_prefix_reports_the_same_error_in_every_position`: the
  same typo in all five positions, in a real two-file project (single-file mode
  never runs the resolver rewrite, so the test has to be project-shaped).
- `cargo test -p cplus-core` 1847 + 8, `cargo test -p cpc` 608 + 16 + 5 + 6,
  including `--test platform_deps`, the riskiest consumer of the resolver's
  configuration.
- Vendor-wide `cpc check` across 54 packages: every package matches its recorded
  baseline count.
