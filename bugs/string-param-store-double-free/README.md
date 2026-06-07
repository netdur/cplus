# Bug: `string` value parameter that is stored (not returned) double-frees

**Status:** OPEN · **Severity:** double free / use-after-free (memory unsafety,
no diagnostic) · **Found via:** the `vendor/agent_core` identity registry
(`push_node` storing a `string` id into a `Vec[string]`).

## Symptom

A `string` bound to a local and then **moved into a method whose body stores the
param** (into a `Vec`, a struct field, or forwards it to another consuming call)
is freed twice: once by the callee's new owner at its drop, and again by the
caller's scope-exit drop of the original local. The program traps (SIGTRAP,
exit 133) at `-O0`. `src/main.cplus` is the minimal pure-C+ reproducer.

## Trigger (all required)

1. a `string` (or `Vec[T]` / `HashMap`) **local binding** — `let x = ...`,
   not an inline temporary;
2. passed by **bare value** (not `move`, `mut`, or `borrow`) into a **method**
   call (free-function calls take a different, sound path);
3. whose body **stores or forwards** the param rather than just reading it or
   returning it.

Confirmed CLEAN counterexamples: inline temp (`k.set("x".to_string())`);
explicit `move v: string`; read-only param; `return v` (covered by the
auto-clone-on-return net); a user-`drop` struct in place of `string`.

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

## Proposed fix

Extend `effective_move` to cover `Ty::String` (and `Vec[T]` / non-Copy owning
builtins), mirroring the enum fix: value-pass, caller `mark_moved`, callee
`register_drop`. Drop the now-redundant `borrowed_params` borrow-ABI special-case
and the string-only return-clone net for these types. Validate against the full
stdlib + vendor suites (every `string`/`Vec` value parameter changes ABI).

The explicit-`move` workaround already proves the move lowering is correct for
`string`: `fn set(mut self, move v: string)` makes the reproducer exit 0.
