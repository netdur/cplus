# Bug 04 — Generic call inside a tuple literal: monomorphize never rewrites it (ICE)

- Status: FIXED 2026-08-01, commit f212467 — TupleLit + Asm arms in both walkers
- Status (original): reproduced 2026-08-01 with `target/release/cpc` (panic at codegen.rs:13640)
- Severity: ICE
- Area: monomorphize (`cplus-core/src/monomorphize.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B4)

Context for the fixer: monomorphize runs after sema. It synthesizes concrete copies of
generic functions (mangled `name__T1__T2`), rewrites call sites to the mangled names, and
deletes the generic templates. If a call site is not rewritten, codegen later looks up a
function that no longer exists and panics. Build: `cargo build --release`; binary
`target/release/cpc`. Tests: `cargo test -p cplus-core`, `cargo test -p cpc --test e2e`.
Line numbers from 2026-08-01; locate by symbol name if drifted.

## Reproduction

`tup.cplus`:

```cplus
fn double_it[T](take x: T) -> T { return x; }
fn main() -> i32 {
    let t: (i32, i32) = (double_it::[i32](7), 1);
    return t.0 - 7;
}
```

```
$ target/release/cpc check tup.cplus
thread 'main' panicked at cplus-core/src/codegen.rs:13640:32:
sema validated function exists: missing `double_it`
```

Expected: compiles; `main` returns 0.

## Root cause

`rewrite_expr` (monomorphize.rs:2426-3246) is a hand-rolled walker with one match arm per
expression kind and a fallthrough `other => other.clone()` at monomorphize.rs:3240. It has
no `TupleLit` arm, so a generic call in a tuple-literal element keeps its template callee
name while the template itself is removed from the program.

The discovery walker `visit_ident_calls` DOES see the call (the instantiation
`double_it__i32` is synthesized), so the failure is the rewrite half only — the same
discovered/not-rewritten asymmetry memorialized in-code for Await/Yield at
monomorphize.rs:2826-2830.

## Fix

1. Add a `TupleLit` arm to `rewrite_expr` that rewrites each element expression
   (mirror the array-literal arm).
2. Audit both walkers for the remaining known asymmetry: `ExprKind::Asm` operands are
   missing from BOTH `rewrite_expr` and `visit_ident_calls`. Add both arms (a generic call
   in an `#asm` operand value should mono correctly or at least be rewritten).
3. Do not add a defensive lookup fallback in codegen — the invariant is "every call site
   is rewritten"; keep the loud panic.

Structural companion: `issue-01-generic-ast-walker.md` (a default-recursion walker makes
this whole missing-arm family impossible). Related sibling bugs from the same walker:
bug-06 (InterpStr), bug-07 (Self walker).

## Verification

1. `tup.cplus` compiles and exits 0.
2. DONE: `generic_calls_in_tuple_asm_and_interp_positions_are_rewritten` in
   monomorphize.rs. The `#asm` operand case was a REAL ICE, not a hypothetical — verified
   against the pre-fix binary ("sema validated function exists: missing `id_it`").
3. DONE: `mono_rewrites_generic_calls_and_self_in_every_position` in cpc/tests/e2e.rs.
4. DONE: full suites green.

A NESTED tuple index (`t.0.0`) does not parse — the lexer reads `0.0` as a float literal.
Separate from this bug; the nested case is covered through a struct wrapper instead.
`let p: (T, i32)` inside a generic body is a separate pre-existing ICE, written up as
`bug-27-tuple-type-in-generic-body-ice.md`.
