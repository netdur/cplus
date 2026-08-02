# Bug 24 — f16 missing from the blessed `to_text` / interpolation table

- Status: FIXED 2026-08-02, commit c7c7051 — f16 added to BOTH tables, and the float
  widening now reads the receiver's own type
- Status (original): probed 2026-08-01 during the audit (E0324 on `(1.5f16).to_text()`); re-verify before fixing
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

1. DONE: re-verified the repro fired (E0324), then fixed. `(2.5f16).to_text()` prints
   `2.5`.
2. DONE: `"v=${half}"` with an f16 prints `v=3.5`. This needed a THIRD edit — the
   interpolation lowering in codegen keeps its own float receiver set, separate from both
   `is_blessed_to_text_receiver` tables, and hit `unreachable!("sema validated interp expr
   type")` until f16 was added there too. Step 3's "verify interpolation picks it up
   automatically" does not hold: sema's table is shared, codegen's is not.
3. DONE: full suites and the stdlib suite green.

## Note on step 1 (the codegen lowering)

No widening-at-the-call-site was needed and no precision decision arises: the float
formatter already widens to `double` for `%g`. It just hard-coded `fpext float` as the
source type, which is wrong for the `half` an f16 holds. It now reads the receiver's own
LLVM type, so f16 formats at full precision — `1.5`, not a re-rounded value.
