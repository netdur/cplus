# Bug 20 — `impl Foo: ToText` is impossible: blessed signature still uses legacy `Ty::String`

- Status: FIXED 2026-08-02, commit c7c7051 — `ToText` registers the DESIGNATED string
  struct; `Ty::String` in a blessed signature also compares equal to it
- Status (original): probed 2026-08-01 during the audit (E0505 on the documented impl shape); re-verify before fixing
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

1. DONE: re-verified the repro fired (E0505), then fixed. The impl compiles AND
   `show::[Foo](f)` through a `[T: ToText]` bound prints `foo` at runtime.
   BOTH halves of the fix were needed: the comparison change alone made the impl legal
   but left the bound's dispatch returning `Ty::String`, so the generic body failed with
   "expected `struct`, found `string`". Registering the return as the designated struct is
   what makes the surface actually usable; the comparison change keeps a `Ty::String`
   signature comparing equal for the primitive impls in a program with no stdlib/text.
2. Interp strings already accepted user ToText types (`check_interp_str` has its own
   `interface_impls` arm) — unchanged.
3. DONE: collateral swept — `check_interp_str`'s doc claimed a `Ty::String` result it has
   not produced since v0.0.24, and its unreachable `matches!(&ty, Ty::String)` arm is
   gone. Full suites and the stdlib suite green.
