# Bug 20 — `impl Foo: ToText` is impossible: blessed signature still uses legacy `Ty::String`

- Status: probed 2026-08-01 during the audit (E0505 on the documented impl shape); re-verify before fixing
- Severity: broken user surface (documented feature cannot be used)
- Area: sema (`cplus-core/src/sema.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B20)

Context for the fixer: the owned string type migrated from an internal `Ty::String` to the
stdlib `Text` struct marked `#[lang("string")]` (tracked as `designated_string_struct`).
The blessed `ToText` interface registration was never migrated, so its expected method
signature can never match a user impl. Zero in-tree impls exist, which is why no test
caught it. Build `cargo build --release`; binary `target/release/cpc`. Line numbers from
2026-08-01.

## Reproduction

```cplus
import "stdlib/text" as text;

struct Foo { v: i32 }

impl Foo: ToText {
    fn to_text(this) -> text::Text { return "foo"; }
}

fn main() -> i32 { return 0; }
```

Observed in audit: `error[E0505]: method 'to_text' ... does not match the interface
signature` — always, for every user impl. The doc comment at sema.rs:4504-4510 promises
exactly this shape.

## Root cause

- Registration: `("ToText", "to_text", Ty::String, false)` at sema.rs:4510 — the blessed
  return type is the legacy `Ty::String`.
- Comparison: sema.rs:20180 (`_ => iface_ty == impl_ty` inside `ty_eq_modulo_self`) —
  `Ty::String` never equals `Ty::Struct(text_id)`, so the signature check always fails.
- Collateral staleness in the same family: `check_interp_str`'s doc says "Result type is
  `Ty::String`" (sema.rs:12652-12655) while the body returns the Text struct; the
  `matches!(&ty, Ty::String)` arm at sema.rs:12665 is unreachable.

## Fix

1. Register the blessed return as "the designated string struct", resolved at check time:
   either store a sentinel that `ty_eq_modulo_self` resolves against
   `designated_string_struct`, or special-case `Ty::String ≡ Ty::Struct(designated)` in
   the comparison at 20180. The first is cleaner; the second is one line.
2. Confirm interface-bound dispatch through `[T: ToText]` works end-to-end with a user
   impl (the erased fn-ptr vtable path) — that is the point of the surface.
3. Sweep the collateral: fix the check_interp_str doc, remove the unreachable 12665 arm.

Companion: `issue-06-lang-item-registry.md` (blessed types resolved through one registry).
Interaction: `issue-11-dead-code-sweep.md` item 5 proposes retiring the legacy
`Ty::String` path entirely; landing THIS fix first removes the last blessed-surface
consumer of `Ty::String`.

## Verification

1. The repro compiles; calling `to_text()` on `Foo` through a `[T: ToText]`-bounded
   generic returns the right text at runtime (e2e test).
2. Interp strings on a user ToText type work if that is part of the contract (check the
   interp blessing table — see bug-24 for the table location).
3. Full suites.
