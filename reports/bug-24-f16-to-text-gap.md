# Bug 24 — f16 missing from the blessed `to_text` / interpolation table

- Status: probed 2026-08-01 during the audit (E0324 on `(1.5f16).to_text()`); re-verify before fixing
- Severity: gap (inconsistent type support)
- Area: sema (`cplus-core/src/sema.rs`), possibly codegen for the formatting lowering
- Master report: `core-drift-audit-2026-08-01.md` (B24)

Context for the fixer: blessed methods (`to_text`, `to_bits`, `hash`, …) are dispatched
from hand-maintained receiver tables in sema. `f16` was added to the language later than
the tables; the sibling `to_bits` arm handles it, the `to_text` table does not. Build
`cargo build --release`; binary `target/release/cpc`. Line numbers from 2026-08-01.

## Reproduction

```cplus
import "stdlib/text" as text;

fn main() -> i32 {
    let a: f32 = 1.5;
    let ta = a.to_text();        // compiles
    let b: f16 = 1.5 as f16;
    let tb = b.to_text();        // E0324
    let _ = ta; let _ = tb;
    return 0;
}
```

Observed in audit: E0324 on the f16 call while f32/f64 work. Interpolation `"${b}"` on an
f16 fails the same way (same table, used by `check_interp_str` at sema.rs:12664).

## Root cause

`is_blessed_to_text_receiver` (sema.rs:12718-12736) has no `Ty::F16` arm; the sibling
`to_bits` arm (sema.rs:12145-12158) supports F16. Hand-maintained receiver tables drift
per type — the audit's "type-list-sync disease".

## Fix

1. Check the codegen lowering first: find how `to_text` on f32/f64 is emitted (runtime
   formatting call) and confirm an f16 path exists or can widen to f32/f64 before
   formatting. If no runtime support exists, the fix is: widen f16 → f32 at the call and
   format as f32 (document the chosen precision).
2. Add `Ty::F16` to `is_blessed_to_text_receiver`.
3. Verify interpolation picks it up automatically (same table); if `Ty::Mask`/SIMD types
   are intentionally excluded, leave them.

Companion: `issue-06-lang-item-registry.md` proposes a declarative blessed-methods table
(receiver-class, name, arity, ret) so a type added to the language lands in ONE row
instead of N match arms.

## Verification

1. The repro compiles; printing the f16 text produces `1.5`.
2. `"${x}"` interpolation with an f16 compiles and prints correctly (e2e).
3. Full suites.
