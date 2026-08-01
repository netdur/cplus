# Issue 05 — One argument path and one dispatch-gate sequence for every call form

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
