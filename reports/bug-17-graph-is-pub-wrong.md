# Bug 17 — Knowledge graph reports every private item as public in multi-file projects

- Status: FIXED 2026-08-02, commit 8054dc6 — one `name_is_public` helper, applied to the leaf
- Status (original): reproduced 2026-08-01 with `target/release/cpc graph`
- Severity: wrong output (the agent-facing graph lies about visibility)
- Area: graph (`cplus-core/src/graph.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B17)

Context for the fixer: C+ privacy is name-based — a leading `_` marks an item
module-private; there is no `pub` keyword. The resolver qualifies top-level names to
`pkg.src.module.item` before graph.rs runs. Build `cargo build --release`; binary
`target/release/cpc`; graph unit tests in graph.rs's `#[cfg(test)]` module. Line numbers
from 2026-08-01.

## Reproduction

Project `gt/` — `Cplus.toml`:

```toml
[package]
name = "gt"
```

`src/main.cplus`:

```cplus
import "./util" as util;
fn main() -> i32 { return util::open(); }
```

`src/util.cplus`:

```cplus
fn _secret() -> i32 { return 1; }
fn open() -> i32 { return _secret(); }
```

```
$ target/release/cpc graph
... gt.src.util._secret   is_pub: true ...
```

Expected: `_secret` reported private. Every private top-level item in every multi-file
project is affected.

## Root cause

graph.rs computes visibility from the resolver-QUALIFIED name:

```rust
is_pub: !f.name.name.starts_with('_'),
```

at graph.rs:305 and nine sibling sites (346, 361, 389, 410, 441, 458, 476, 494; the
method site at 1163 is coincidentally correct because method names stay bare). After
qualification the name is `gt.src.util._secret` — it never starts with `_`.

This is the third divergent copy of the visibility predicate (resolver
`exported_name` resolver.rs:1794; sema `is_private_name` sema.rs:453) and the copy that
drifted.

## Fix

Tactical:

1. In each of the ten graph sites, apply the `_` test to the LEAF segment of the name
   (split on the final `.`), or reuse whatever short-name helper graph.rs already has for
   display. One shared local helper `leaf_is_private(&str) -> bool`, used by all ten.

Structural (companion `issue-09-resolver-program-index.md`):

2. The resolver knows exportedness at qualify time; stamping it into the AST item (or the
   LoadedProject tables) lets graph and sema read a fact instead of re-deriving it. That
   removes all three predicate copies.

## Verification

1. DONE: `cpc graph` on the repro reports `_secret` private and `open` public.
2. DONE: the leaf of an unqualified name is the name itself, so single-file mode is
   unchanged — the existing `struct_fields_become_nodes_with_has_field_edges` test covers
   it and still passes.
3. DONE: `qualified_names_do_not_hide_the_privacy_marker` in graph.rs, over both a
   function and a struct.
4. DONE: suites green.

Took the tactical fix (step 1). Step 2 — the resolver stamping exportedness into the AST
so nobody re-derives it — belongs to issue-09 and would retire all three predicate copies;
this one only removes the ten copies WITHIN graph.rs.
