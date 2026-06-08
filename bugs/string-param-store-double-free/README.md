# Bug: owned value parameter that is stored (not returned) double-frees

**Status:** FIXED (v0.0.17) · **Severity:** double free / use-after-free (memory
unsafety, no diagnostic) · **Found via:** the `vendor/agent_core` identity
registry (`push_node` storing an owned id into a `Vec`).

`src/main.cplus` is now a **regression guard** (`cpc build && ./target/debug/repro`
→ prints `reached end`, exits 0; ASan-clean). It uses `Vec[Text]` — the exact
shape that surfaced the bug — because the original `string` spelling has since
been removed (the owned string is the stdlib `Text`).

## Symptom (pre-fix)

An owned value bound to a local and then **moved into a method whose body stores
the param** (into a `Vec`, a struct field, or forwards it to another consuming
call) was freed twice: once by the callee's new owner at its drop, and again by
the caller's scope-exit drop of the original local. The program trapped (SIGTRAP,
exit 133) at `-O0`.

## Trigger (all required)

1. an owned builtin (`Text`, `Vec[T]`, `HashMap`) **local binding** —
   `let x = ...`, not an inline temporary;
2. passed by **bare value** (not `move`, `mut`, or `borrow`) into a **method**
   call (free-function calls take a different, sound path);
3. whose body **stores or forwards** the param rather than just reading it or
   returning it.

Confirmed CLEAN counterexamples: inline temp (`k.set(text::from_str("x"))`);
explicit `move v: Text`; read-only param; `return v` (covered by the
auto-clone-on-return net); a user-`drop` struct in place of the builtin.

## Root cause

`effective_move` ([cplus-core/src/codegen.rs](../../cplus-core/src/codegen.rs),
~line 2263) routes bare non-Copy params through the sound move lowering
(value-pass + caller `mark_moved` to flip its drop flag + callee `register_drop`)
**only for `Ty::Struct | Ty::Enum`**. `Ty::String` and `Vec[T]` are deliberately
excluded; they use the borrow ABI (caller keeps the drop, param registered in
`borrowed_params`, ~line 5176) guarded by an **auto-clone-on-return** safety net
(~line 8048). That net only rewrites `return param`; it does nothing when the
param is **stored or forwarded**, so the caller's drop and the new owner's drop
both free the same heap.

This is the same class as the v0.0.14 `vendor/json` enum double-free, which was
fixed by adding `Ty::Enum` to `effective_move` (v0.0.15). `Ty::String` (and
`Vec[T]`/owning containers) are the remaining gap.

## Fix (shipped v0.0.17)

`effective_move` was extended to cover the non-Copy owning builtins (the
string-move fix + the ownership-safe `Vec` rewrite), mirroring the enum fix:
value-pass, caller `mark_moved`, callee `register_drop` — so a stored/forwarded
bare param is no longer double-freed. Validated against the full stdlib + vendor
suites. This guard (`Vec[Text]` storing-param) exits 0, ASan-clean.

The `string` spelling itself was later removed entirely (R4): the owned string is
the import-required stdlib `Text`. The non-Copy-param-store class is also covered
in the e2e suite by `bare_noncopy_param_move_forwarded_no_double_free` and
exercised live by `vendor/agent_core`'s `Vec[Text]` registry.
