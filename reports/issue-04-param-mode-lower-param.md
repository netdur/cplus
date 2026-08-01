# Issue 04 — `ParamMode` enum + one `lower_param` constructor (de-boolify the ABI plumbing)

- Type: structural consolidation
- Area: `cplus-core/src/codegen.rs` primarily; touches sema/mono AST-side helpers
- Effort: S-M (mechanical, wide)
- Retires / prevents: the v0.0.15 vendor/json double-free class (a sig-collection site
  using raw `p.move_` instead of `effective_move`); silent attr/ABI changes from
  transposed positional bools
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #4, §2 effective_move row)

## Problem

Parameter passing is encoded as positional bool soup: `(Ty, bool, bool, bool)` tuples in
`FnSig`, four positional bools into `param_attrs` at 23 call sites, and parallel
`Vec<bool>` pairs on fn-pointer types. The correctness rule "always derive the move flag
via `effective_move`, never raw `p.move_`" is enforced by convention at what the project
notes said were 3 sites — there are now 4, proving the convention decays. A transposed
bool at any site is a silent ABI or attribute change.

## Current state

- `effective_move` call sites (all in codegen.rs): 2135 (collect_sigs), 2592 (enum
  methods), 2638 (str methods, added v0.0.27), 2686 (struct methods) — four copies of the
  identical 5-line Param→tuple closure. The v0.0.15 double-free was a fifth site using
  raw `p.move_`.
- `FnSig::params: Vec<(Ty, bool, bool, bool)>` (codegen.rs:2087) — the doc comment still
  describes three fields, `restrict` bolted on as "(4th flag)"; destructured positionally
  at 20+ sites.
- `param_attrs(ty, move_, mutable, restrict, pointer_passed, types)` (codegen.rs:2922) —
  23 call sites of literal soup, e.g. codegen.rs:13383:
  `param_attrs(pty, false, true, false, true, self.types)`.
- FnPtr types carry parallel `param_takes: Vec<bool>` / `param_refs: Vec<bool>`
  (both-true is representable — an illegal state); the take-before-ref ordering rule is
  re-encoded at 4 mangler sites (monomorphize.rs:3723-3727, 3652-3656,
  sema.rs:20063-20069 plus the length mirror).
- `synth_bound_bridge` hand-builds `Param` with 6 positional bools
  (monomorphize.rs:418-431) and `Function` with 8 (monomorphize.rs:505-526).

## Target design

```rust
pub enum ParamMode { Borrow, Ref, Take }   // mutually exclusive by construction

pub struct ParamSig { pub ty: Ty, pub mode: ParamMode, pub restrict: bool }

/// THE only way to build a ParamSig from an AST Param. Owns effective_move.
pub fn lower_param(p: &Param, t: &TypeTable) -> ParamSig;
```

- `FnSig::params` becomes `Vec<ParamSig>`.
- `param_attrs(&ParamSig, pointer_passed, types)` — no positional bools.
- FnPtr: one `Vec<ParamMode>` replaces the two bool vecs; the mangler orders from the
  enum (one place).
- Pairs with `issue-03-abi-classifier.md` (`classify_param` consumes `ParamMode`), but
  lands independently and FIRST — it shrinks issue-03's diff.

## Migration plan

1. Introduce `ParamMode`/`ParamSig` + `lower_param`; convert the four `effective_move`
   sites to call it (delete the inline closures). Behavior identical; suites green.
2. Convert `FnSig::params` and its 20+ destructure sites (compiler errors are the
   worklist — that is the point of the enum).
3. Convert `param_attrs` and its 23 call sites.
4. Convert FnPtr's bool-vec pair + the 4 mangler order sites (coordinate with
   `issue-02-mangling-module.md` if it is landing concurrently — the mangler should
   consume `ParamMode` too).
5. Give AST `Param`/`Function` builders `Default` impls or a builder so
   `synth_bound_bridge` names its fields.

## Verification

- Full suites after each step; the change is behavior-preserving throughout — any IR
  diff is a bug in the migration.
- Cheap belt-and-braces: before step 1, dump IR for a representative program set
  (`--emit-ll` on a few e2e fixtures); diff after step 5 — must be byte-identical.

## Risks and constraints

- Do not "fix" any classification while migrating — transpositions discovered en route
  (if any) get their own bug report first, so the mechanical change stays reviewable.
