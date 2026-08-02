# Issue 02 — One mangling module (5 printers, 2 parsers, already diverged)

- Status: DONE 2026-08-02, commit <pending>
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

## Outcome

`cplus-core/src/mangling.rs` now holds the grammar, written once:

```rust
pub fn render(ty: &Ty, nominal: &dyn Fn(&Ty) -> String) -> String;
pub fn render_len(ty: &Ty, nominal: &dyn Fn(&Ty) -> String) -> usize;
pub fn render_ast(t: &Type) -> String;
pub fn join(base: &str, args: &[String]) -> String;
pub fn join_len(base: &str, arg_lens: impl Iterator<Item = usize>) -> usize;
pub fn take<'a>(s: &'a str, nominal: &dyn Fn(&str) -> Option<(Ty, usize)>) -> Option<(Ty, &'a str)>;
pub fn from_suffix(suffix: &str, nominal: &..., fallback: &dyn Fn(&str) -> Option<Ty>) -> Ty;
```

The id universes stay local, as the report required: every nominal lookup is a
callback the caller supplies from its own tables. Sema keeps `nominal_name`
over its `StructDef`/`EnumDef` slices, codegen keeps `nominal_name`,
`nominal_prefix` (longest name at a token boundary) and `nominal_tail_match`
(the qualified-tail fallback) over its own re-derived table.

Two API differences from the report's sketch, both from writing it:

- `render_len` is not a re-implementation and not `render().len()` either.
  Both go through one `write_ty` over a `Sink`, which is either a `String` or a
  counter — a shared grammar that never materializes the name the
  instantiation-size guard is about to refuse to build.
- `from_suffix` takes the same prefix-matching `nominal` callback as `take`
  rather than a separate "exact" one, and derives the exact test from it
  (consumed == whole length). The fn-pointer branch delegates to `take`, which
  needs the prefix form anyway.

Now pointing at it: sema's `mangle_ty_for_name`, `mangled_ty_name_len`,
`mangle_generic_struct_name`, `projected_generic_name_len`; monomorphize's
`mangle_ty`, `mangle_type_ast_arg`, `mangle_name`, `mangle_call_from_ast`;
codegen's `mangle_o_for_tramp_with_types`, `tuple_elem_mangle`,
`ty_from_suffix`. Codegen's `mangled_ty_take` is deleted — its callers go
through `crate::mangling::take`.

### The divergences

1. **fn-ptr unit return — a live ICE, now fixed and covered by an e2e test.**
   `Cell[fn(i32) -> ()]` (a generic instantiated at a unit-returning
   fn-pointer) aborted with "codegen reached TypeKind::Generic — monomorphize
   did not rewrite this site" on the pre-change binary: the AST printer built
   the key `fn_i32_ret_unit`, the Ty printer had registered `fn_i32`, the lookup
   missed and the node was left alone. e2e
   `a_generic_over_a_unit_returning_fn_pointer_instantiates`.
2. **`Ty::Param` vs a bare AST `Path` — NOT a divergence to fix.** The AST
   cannot tell an unsubstituted parameter `T` from a struct named `T`, so it
   renders `T` where the `Ty` side renders `Param_T`. The consequence is a
   lookup miss on an instantiation that still mentions a type parameter — which
   is the correct answer, since that is not a concrete instantiation. Recorded
   in `render_ast`'s doc comment rather than "fixed".
3. **`f16` in the whole-string parser** — fixed; one keyword table now serves
   both parsers, so a primitive cannot be missing from one of them. Unit test.
   No end-to-end repro was found: the async/thread paths that call the parser
   resolve `Future__f16` by name match before the suffix decode is reached.
4. **SIMD/mask in the tokenizing parser** — fixed; both vector forms parse in
   both parsers, including inside a fn-pointer parameter list
   (`fn_f32x4_maskf32x4`). Unit test.
5. **A struct named like a vector** — fixed, and the fix is a DECISION worth
   recording: `i8x2` is both a legal struct name and a legal vector spelling.
   Both parsers now resolve a declared nominal name first, so the user's
   declaration wins; a vector spelling no nominal name shadows still decodes.
   The prefix rules (`ptr_`, `slice_`, `arr`, `fn`, `Param_`) still beat a
   nominal name, which is what every earlier parser did — a struct called `ptr`
   does not capture `ptr_i32`.

The property test (`every_type_round_trips_through_the_grammar`) runs a corpus
of primitives, nominals (including a qualified one), pointers, slices, arrays,
fn-pointers with take/ref modes and unit/non-unit returns, and SIMD/mask
vectors, asserting `take(render(ty)) == ty`, `from_suffix(render(ty)) == ty`
and `render_len(ty) == render(ty).len()`. Two exclusions, both documented in the
test: `ERR` in composite position (`Ty::Error` doubles as the parser's "did not
parse"), and any structural type whose rendering a nominal name shadows (case 5
above).

## Verification (as run)

- 7 unit tests in `mangling.rs` (the property test plus one per divergence).
- e2e `a_generic_over_a_unit_returning_fn_pointer_instantiates`.
- `cargo test -p cplus-core` 1844 + 8, `cargo test -p cpc` 605 + 16 + 5 + 6;
  `cpc test` in `vendor/stdlib` 290 green in debug and `--release`; vendor-wide
  `cpc check` diagnostic parity across 54 packages — no change.
- Symbol-name impact: only the `_ret_unit` case changes, and it changes from a
  name that resolved to nothing. No prebuilt archive in the tree is keyed by a
  symbol name (binary packages are parked), so nothing needs rebuilding beyond
  a normal `cpc build`.
