# Bug 19 — Method privacy enforced for the path spelling only; `g._hidden()` compiles

- Status: reproduced 2026-08-01 with `target/release/cpc`
- Severity: visibility hole
- Area: resolver (`cplus-core/src/resolver.rs`) + sema (`cplus-core/src/sema.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B19)

Context for the fixer: privacy is name-based (`_` prefix = module-private; no `pub`
keyword). For methods it is enforced only on the explicit path spelling
`Type::method(recv)`; the ordinary receiver spelling bypasses it entirely. Build
`cargo build --release`; binary `target/release/cpc`. Line numbers from 2026-08-01.

## Reproduction

Project `privtest/` — `Cplus.toml`:

```toml
[package]
name = "privtest"
```

`src/lib_mod.cplus`:

```cplus
struct Gadget { v: i32 }
impl Gadget {
    fn make() -> Gadget { return Gadget { v: 3 }; }
    fn _hidden(this) -> i32 { return this.v; }
}
```

`src/main.cplus` — path spelling (correctly rejected):

```cplus
import "./lib_mod" as lib;

fn main() -> i32 {
    let g = lib::Gadget::make();
    return lib::Gadget::_hidden(g);
}
```

→ E0403. Receiver spelling (compiles clean — the hole):

```cplus
import "./lib_mod" as lib;

fn main() -> i32 {
    let g = lib::Gadget::make();
    return g._hidden();
}
```

Expected: both rejected with E0403 cross-file; both allowed same-file.

## Root cause

- The path form reaches `check_pub_method` (resolver.rs:2210-2236) via the 3-segment
  `Path` arm (resolver.rs:3081-3088).
- The receiver form: the resolver's `Field`/method arm just recurses into the receiver
  (resolver.rs:3167) — no privacy check — and sema has NO method-privacy check at all
  (its E0403 sites are fields only: sema.rs:10252, 10332).

## Fix

Enforce at method DISPATCH in sema (the resolver cannot see through receiver types; sema
resolves the method to its defining impl):

1. In `check_method_call`, once the method is resolved to its owner type and impl, if
   `is_private_name(method_name)` (helper at sema.rs:453) and the impl's defining file
   differs from the calling file, emit E0403 with the same message the field path uses.
   Sema knows the defining file through the impl's item origin — read how
   `check_method_contract` or the ext-scope gate (`ext_out_of_scope`) obtains the
   defining module, and reuse that plumbing.
2. Cover ALL dispatch paths, not just the concrete-struct one (enum methods, generic
   methods, assoc-fn spelling) — this is the same N-paths surface as
   `issue-05-sema-call-gate-unification.md`; if issue-05 lands first, the privacy gate is
   one line in the shared gate sequence.
3. Same-file (same-module) calls stay legal, including through `this._helper()` inside
   the impl.

## Verification

1. Cross-file `g._hidden()` → E0403; path spelling still E0403.
2. Same-file `g._hidden()` and internal `this._hidden()` compile.
3. Extension methods: a cross-package extension defining `_helper` for its own use — pick
   the intended semantics (defining module = the extension's module) and add a test
   pinning it.
4. Negative e2e tests for both spellings; full suites.
