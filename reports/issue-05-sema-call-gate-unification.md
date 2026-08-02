# Issue 05 — One argument path and one dispatch-gate sequence for every call form

- Status: DONE 2026-08-02, commit 8f874b8
- Type: structural consolidation
- Area: `cplus-core/src/sema.rs`
- Effort: M-L
- Retires / prevents: bug-01 (live soundness hole), bug-11 (false E0335), bug-12's
  call-arg hole; three shipped holes memorialized in comments (enum bounds double-free,
  generic-path E0327/E0328/E0308, inference-branch take-consumption UAF)
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #5, §2 per-arg gates row)

## Problem

Sema has four per-argument checking pipelines and roughly six method-dispatch paths, each
of which must remember K gates (contract check, extension scope, impl-block bounds,
receiver check, argument rules, consumption). The historical record inside the file shows
this drift class shipping at least four holes; the audit reproduced a fifth (bug-01).
"Remember to call K gates on every new path" must become "impossible to skip."

## Current state

Per-arg pipelines:

1. `check_arg_with_move` (sema.rs:14669) — the FULL pipeline: capture-escape,
   bound-ref skip, StrLit→Text, E0328 writability (14710-14722), consume
   (`consume_value_arg`, 14740 — its own doc says it was meant to be "THE single
   place").
2. `check_generic_named_call`, both branches (11663-11694 inference, 11764-11770
   turbofish): check_expr + consume only. The inference branch ALSO double-checks each
   arg (bug-11).
3. `check_generic_method_call` (12596-12620): check_expr + consume_value_arg only;
   near-clone `consume_generic_take_arg` at 11794.
4. fn-pointer call path (10747-10770): its own loop with its own E0328 copy.

Dispatch paths × gates (gates: `check_method_contract`, `ext_out_of_scope`,
`check_impl_block_bounds_at_call`, `check_method_receiver`, args/return): struct
(12410-12424), enum (12342/12484/12486), assoc (14329/14377), generic-param-bound
(12369 — no ext/bounds gate), str (12278), SIMD (separate world). In-code confessions:
enum path shipped without the bounds gate — "silently unenforced … double-free"
(12480-12483); generic path shipped without E0327/E0328/E0308/move-consume
(11899-11902); inference branch shipped without take-consumption (11793-11800).

Also folded in here: the duplicated type-mismatch re-check producing double diagnostics
and leaking resolver-qualified names (11665-11678, 12599-12612).

## Target design

```rust
struct ArgCtx<'a> { expected: ParamSigView<'a>, callee_span: Span, .. }
fn check_one_arg(&mut self, arg: &Expr, cx: ArgCtx) { /* the rule set of
    check_arg_with_move, on an already-substituted expected signature */ }

struct ResolvedCall { owner: OwnerKind, sig: SigView, origin: CallOrigin }
fn run_call_gates(&mut self, rc: &ResolvedCall, span: Span) { contract();
    ext_scope(); impl_bounds(); receiver(); }
```

Every call form resolves FIRST (to owner/sig/origin), then runs `run_call_gates`, then
loops `check_one_arg` exactly once per argument. Inference produces the substitution
before the single arg pass (fixes bug-11's double-eval by construction: infer from a
side-effect-free probe or from a checked-once result, never re-check).

## Migration plan

1. Extract `check_one_arg` from `check_arg_with_move`; adopt in the concrete path.
   Pure refactor; suites green.
2. Adopt in `check_generic_named_call` both branches — FIXES bug-01 (E0328 now runs) and
   the StrLit→Text call-arg hole; make inference single-check — FIXES bug-11. Add the
   negative tests from those two bug reports. Delete `consume_generic_take_arg`.
3. Adopt in `check_generic_method_call` and the fn-ptr loop (delete its E0328 copy).
4. Extract `run_call_gates`; adopt path by path (struct → enum → assoc →
   generic-param-bound → str). The generic-param-bound path gains the ext/bounds gates
   it lacks — check for intentional exemptions before enabling (read the path's
   comments; if an exemption is real, encode it as an explicit flag in ResolvedCall,
   not by skipping the call).
5. Remove the duplicate mismatch re-checks; route the central E0302 through
   `ty_display_named` (sema.rs:3269) so no qualified names leak.

## Verification

- bug-01/bug-11/bug-12 repros as e2e tests (their reports carry the sources).
- Expect message churn where tests pinned the duplicated/qualified diagnostics — grep
  e2e for E0328/E0335/E0302 assertions and update deliberately.
- Full suites after each step; vendor suites (`cd vendor/stdlib && cpc test`) at the end.

## Risks and constraints

- SIMD's separate dispatch world stays separate in this pass (its gate needs differ);
  note it in the code as an explicit exception.
- Behavior-lock the receiver rules: `this`-consuming methods and take-receiver
  consumption have their own tests — keep them green at every step.

## Outcome

Steps 1-3 and 5 (the per-argument pipelines) landed with bug-01 in the bug
tier: `check_arg_with_move` / `gate_checked_arg` / `consume_value_arg` are the
one pipeline, `subst_param_sig` hands the generic paths a concrete `ParamSig`
so they run the same rules, and `consume_generic_take_arg` is gone. This commit
is step 4, the dispatch-gate half.

```rust
enum GateOwner { Struct(StructId), Enum(EnumId), NoNominal }

fn run_method_gates(&mut self, owner: GateOwner, shown: &str, name: &Ident,
                    args: &[Expr], call_span: ByteSpan) -> Result<(), Ty>
```

One function runs extension scope, `_`-privacy and impl-block generic bounds,
in one order, for every dispatch path: struct methods, enum methods,
associated fns of both, the builtin `str` methods and interface methods reached
through a type parameter's bound. `Err(ty)` means a gate rejected and the
arguments have already been walked for recovery, so the shape of the rejection
is shared too — that recovery walk was copied six times.

The two paths with no nominal owner now SAY so (`GateOwner::NoNominal`) rather
than simply not calling the gates. That is the whole point: a path that omits
a gate and a path that has nothing to check are indistinguishable in the old
shape, which is how the enum path shipped for a release without the bounds
gate — the bound that keeps a non-Copy payload from being bit-copied, i.e. a
double-free.

The exemptions are real, and the reasons are in the enum's doc comment: all
three gates key on a nominal type id (an extension extends a named type,
`_`-privacy is a name on a named type, impl-block bounds come from a named
type's `generic_origin`), and a builtin receiver or a type parameter has none.
An interface method's bounds are checked where the instantiation is created.

### One deliberate diagnostic change

On the associated-fn path the bounds gate now runs BEFORE the receiver-less
(E0327) and arity (E0308) diagnostics rather than after. A call that is wrong in
both ways reports both errors instead of only the first. No test or vendor
package changed.

### Not in scope, as the report set out

SIMD keeps its separate dispatch world.

## Verification (as run)

- New e2e `impl_block_bounds_are_enforced_on_every_dispatch_path`: one program
  calling a `T: Copy`-bounded method and associated fn on both a struct and an
  enum instantiated at a non-Copy type, asserting E0502 on all four paths.
- `--emit-ll` over 40 `docs/examples` plus the ABI probes: identical (this is a
  checking change, not a lowering one).
- `cargo test -p cplus-core` 1845 + 8, `cargo test -p cpc` 606 + 16 + 5 + 6;
  `cpc test` in `vendor/stdlib` 290 green; vendor-wide `cpc check` parity across
  54 packages — no change.
