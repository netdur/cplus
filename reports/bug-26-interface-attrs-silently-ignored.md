# Bug 26 — Struct-only attributes on an interface are silently accepted and ignored

- Status: verified 2026-08-01 during the audit (`#[watch]` / `#[repr(C)]` on an interface: exit 0, no effect)
- Severity: silent no-op (user believes a feature is active)
- Area: attrs (`cplus-core/src/attrs.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (resolver/attrs audit F6)

Context for the fixer: attrs.rs validates every `#[...]` attribute against a target mask
(struct, enum, fn, method, …) and rejects misplacements with E0356. Interfaces are
validated with the STRUCT mask, so struct-only attributes pass validation on interfaces —
and then nothing downstream consumes them there. Build `cargo build --release`; binary
`target/release/cpc`. Line numbers from 2026-08-01.

## Reproduction

```cplus
#[watch]
interface Observable {
    fn poke(this) -> i32;
}

fn main() -> i32 { return 0; }
```

Observed in audit: compiles, exit 0. The `#[watch]` write-barrier machinery only looks at
structs (its E0361/E0362 checks), so the attribute silently does nothing. Same for
`#[repr(C)]` and `#[lang]` on an interface.

## Root cause

attrs.rs:304-306:

```rust
ItemKind::Interface(i) => {
    ctx.check_attrs(&i.attributes, TARGET_STRUCT, "interface");
```

The interface arm borrows the struct target mask instead of having its own.

## Fix

1. Add a `TARGET_INTERFACE` bit to the target mask set.
2. Use it in the Interface arm. No current attribute sets the bit, so every attribute on
   an interface becomes E0356 ("not valid on an interface") — which is the correct state
   today.
3. If some attribute is meant to be legal on interfaces (doc attributes?), add the bit to
   exactly those registrations — grep the attribute registration table in attrs.rs for
   candidates and check each consumer.

## Verification

1. The repro now fails with E0356 naming the interface target.
2. Attributes on structs/enums/fns unchanged: `cargo test -p cplus-core` (attrs.rs has
   ~80 unit tests; add one for the interface case following the neighboring E0356 tests).
3. Grep vendor/ and examples/ for attributes on interfaces to confirm nothing legitimate
   breaks (`grep -rn '#\[' vendor/*/src --include=*.cplus | grep -B1 interface` style
   sweep, or just build the vendor suites).
