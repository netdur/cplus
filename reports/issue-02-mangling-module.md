# Issue 02 — One mangling module (5 printers, 2 parsers, already diverged)

- Type: structural consolidation
- Area: new `cplus-core/src/mangling.rs`; `sema.rs`, `monomorphize.rs`, `codegen.rs`
- Effort: S-M (mostly moves)
- Retires / prevents: the Vec[*T]/Vec[fn] historical miscompile class; bug-08's adjacent
  lookup fragility; the fn-ptr `_ret_unit` latent lookup miss
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #2, §2 mangling row)

## Problem

The `name__T1__T2` type-mangling grammar is implemented five times as a printer and twice
as a parser, kept in sync by comments ("The shapes must match mangle_ty exactly"). Two of
the five printers were verified line-identical (~55 lines each) — pure duplication — and
divergences already exist. The parsers (`mangled_ty_take`, `ty_from_suffix`) have each
produced a recorded miscompile when they fell behind the printers (the Vec[*T] and
Vec[fn] fixes).

## Current state

Printers:

- sema.rs:20036 `mangle_ty_for_name` — verified identical to
- monomorphize.rs:3693 `mangle_ty`;
- monomorphize.rs:3632 `mangle_type_ast_arg` — fourth copy over AST `Type`;
- sema.rs:19972 `mangled_ty_name_len` — length-only third copy;
- codegen.rs:449 `mangle_o_for_tramp_with_types`, codegen.rs:2374 `tuple_elem_mangle`
  ("Must match sema's naming").

Join builders (the `name__arg__arg` composition, three copies):
monomorphize.rs:3252 `mangle_name`, monomorphize.rs:3273 `mangle_call_from_ast`
("producing the SAME symbol mangle_name builds"), sema.rs:20022
`mangle_generic_struct_name`.

Parsers: codegen.rs:931 `mangled_ty_take` (tokenizing), codegen.rs:1038 `ty_from_suffix`
(whole-string) — which must also agree with each other.

Known divergences (each becomes a regression test in the new module):

1. fn-ptr unit return: `mangle_type_ast_arg` appends `_ret_` for any `Some(rt)`
   (monomorphize.rs:3659-3662) while `mangle_ty` omits it for `Ty::Unit`
   (monomorphize.rs:3730). A user-written `fn(..) -> ()` renders `..._ret_unit`,
   missing every Ty-derived lookup key → unresolved node → downstream panic.
2. `Ty::Param` renders `"Param_T"` (monomorphize.rs:3740) vs AST `Path("T")` → `"T"`
   (monomorphize.rs:3639).
3. `ty_from_suffix` has no `f16` case (codegen.rs:1049-1099) though every printer emits it.
4. `mangled_ty_take` has no SIMD/Mask arms at all.
5. `ty_from_suffix`'s `rfind('x')` SIMD rule (codegen.rs:1130) runs BEFORE exact
   struct-name match: a user struct named `i8x2` parses as a SIMD vector.

Grammar ambiguity to document in the module header: `_` and `__` are both separators and
legal identifier characters; the load-bearing invariant is E0917 (interior `__` reserved
in user identifiers, sema.rs:~1881). `mangled_name_matches` (codegen.rs:536-544) and the
longest-prefix struct matching (codegen.rs:1018-1035) both lean on it.

## Target design

`cplus-core/src/mangling.rs`:

```rust
pub fn render(ty: &Ty, ctx: &dyn TypeNames) -> String;      // one printer
pub fn render_len(ty: &Ty, ctx: &dyn TypeNames) -> usize;   // derived, not re-implemented
pub fn join(base: &str, args: &[String]) -> String;         // name__a__b
pub fn take<'a>(s: &'a str, ctx: &dyn TypeTables) -> Option<(Ty, &'a str)>;  // one parser
```

`TypeNames`/`TypeTables` are small traits so sema, mono, and codegen can each supply
their own id universe (this respects the deliberate sema/codegen id separation — the
GRAMMAR is shared, the lookups stay local). The AST-side printer becomes
`render_ast(type)` in the same module, tested equal to `render` after `ty_to_type_ast`.

REQUIRED test: a property test in the module generating a Ty corpus (primitives, structs,
enums, nested generics, fn-ptrs with take/ref modes and unit/non-unit returns, tuples,
SIMD/mask, raw pointers) asserting `take(render(ty)) == ty` and
`render_len(ty) == render(ty).len()`.

## Migration plan

1. Create the module by MOVING `mangle_ty` (mono's copy) in; add the property test;
   resolve divergences 1-5 (each with a targeted unit test).
2. Point sema's `mangle_ty_for_name` and `mangled_ty_name_len` at it; delete the copies.
3. Point the three join builders at `join`.
4. Point codegen's `mangled_ty_take`/`ty_from_suffix` consumers at `take`; keep
   `ty_from_suffix`'s struct-table lookup local (grammar in the module, table lookup at
   the call site).
5. Longer-term direction (document in the module header, do not implement here): mono
   records instance-name → arg-Ty side tables so codegen never demangles at all
   (`TrampolineSpec::Spawn { o: Ty }`, codegen.rs:383-386, shows the shape).

## Verification

- Property test green; full suites after each step.
- The divergence fixes change emitted symbol names ONLY for the `_ret_unit` case —
  grep e2e IR assertions for `_ret_` and update deliberately.

## Risks and constraints

- Mangled names are internal-only (never shown in diagnostics) — renames inside the
  module must not leak into error messages.
- Symbol-name changes invalidate any prebuilt/vendored archives keyed by symbol; the
  project has none shipped today (binary packages are parked), but note it in the commit.
