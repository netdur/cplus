# Bug 19 — Method privacy enforced for the path spelling only; `g._hidden()` compiles

- Status: FIXED 2026-08-02, commit 8054dc6 — `deny_private_method` at sema dispatch,
  fed by a new per-method declaring-file table
- Status (original): reproduced 2026-08-01 with `target/release/cpc`
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

1. DONE: both spellings now give E0403, with the same message.
2. DONE: same-file `g._hidden()` and internal `this._hidden()` compile.
3. Semantics chosen: the defining module is the module whose `impl` block declares the
   METHOD, not the one declaring the type. That is what a new `method_origins` table
   records (`ext_origins` only covers extensions, since the import gate is all it answers).
   So an extension's own `_helper` is private to the extension's module — the reading that
   matches "privacy is a property of the declaring module".
4. DONE: `method_privacy_and_unknown_methods_across_modules` in cpc/tests/e2e.rs covers
   both spellings, the module's own use of its private helper, and bug-22's
   unknown-method case. Full suites green, and no vendor package trips the new rule
   (checked across all of `vendor/*`).

Step 2's "cover ALL dispatch paths": the gate is placed at each of the three sites that
already call `ext_out_of_scope` — struct receiver, enum receiver, and the `Type::method`
assoc path — which is the same surface that gate covers.
