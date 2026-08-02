# Bug 22 — Nonexistent methods reported as "private" with bogus fix-it advice

- Status: FIXED 2026-08-02, commit 8054dc6 — `all_methods` table + a three-way check
- Status (original): reproduced 2026-08-01 with `target/release/cpc`
- Severity: wrong diagnostic
- Area: resolver (`cplus-core/src/resolver.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B22)

Context for the fixer: privacy is name-based (`_` prefix). v0.0.12 (change G-030) split
"unknown item" from "private item" for top-level items so typos stop being reported as
privacy violations. The twin path for METHODS was never updated. Build
`cargo build --release`; binary `target/release/cpc`. Line numbers from 2026-08-01.

## Reproduction

Project `privtest/` (same fixture as bug-19) — `src/main.cplus`:

```cplus
import "./lib_mod" as lib;

fn main() -> i32 {
    let g = lib::Gadget::make();
    return lib::Gadget::nonexistent(g);
}
```

```
$ target/release/cpc check ...
error: method `Gadget::nonexistent` is private (its leading `_` marks it
module-private; drop the `_` to export)
```

There is no `nonexistent` method and no leading `_`. Expected: an unknown-method error
("no method named `nonexistent` on `Gadget`"), matching the item-side split.

## Root cause

`check_pub_method` (resolver.rs:2220-2235):

```rust
let is_pub = self.pub_methods.get(target_id)...unwrap_or(false);
if !is_pub { return Err(ResolveError::PrivateAccess { ... PrivateKind::Method ... }) }
```

`unwrap_or(false)` conflates "not present" with "present but private". The merge pre-pass
only collects PUB method names (resolver.rs:1878-1885), so existence cannot be
distinguished from privacy. The item-side split exists at resolver.rs:76-86 and
2129-2144 (`UnknownItem` vs `PrivateAccess`) — mirror it.

## Fix

1. In the merge pre-pass, collect ALL method names per type (either a second set, or
   change `pub_methods` to `methods: HashMap<TypeId, HashMap<String, bool /*exported*/>>`).
2. In `check_pub_method`, three-way: name absent → the UnknownItem-style error with the
   method message shape; present and private → the current PrivateAccess message; present
   and pub → Ok.
3. Optional but cheap: add a nearest-name suggestion using the `edit_distance` helper
   already in resolver.rs (~910).

Structural companion: `issue-09-resolver-program-index.md` (a full — not pub-only — item
and method index is its part A; this fix falls out of it).

## Verification

1. DONE: `no method named `nonexistent` on `Gadget`` (E0405), and a genuinely private
   method still reports E0403.
2. DONE: `method_privacy_and_unknown_methods_across_modules` in cpc/tests/e2e.rs asserts
   both messages AND that the unknown case does NOT mention privacy.
3. DONE: suites green.

Step 3 (a nearest-name suggestion via `edit_distance`) was NOT taken — it is a separate
diagnostic-quality improvement, not part of telling absent from private, and the
item-side `UnknownItem` does not offer one either. Adding it to one and not the other
would be a new asymmetry.
