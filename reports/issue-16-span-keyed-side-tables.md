# Issue 16 — Span-keyed side tables: assert the contracts, fix the span policy, scope NodeId

- Type: hazard hardening + design note
- Area: `cplus-core/src/sema.rs`, `monomorphize.rs`, `lower.rs`, `codegen.rs`
- Effort: S now (items 1-2); design-only for item 3; M optional for item 4
- Retires / prevents: silent mis-keying when passes clone/re-span AST; the
  positional-splice overwrite hazard
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 2; checking audit F12, mono audit F12)

## Problem

Eleven side tables flow sema → monomorphize → codegen keyed by `ByteSpan` as a stand-in
for node identity: `call_monos`, `default_splices`, `bound_method_refs`,
`assoc_free_fn_dispatches`, `assoc_method_dispatches`, `text_to_str_coercions`,
`compile_time_blobs`, `inferred_struct_lits`, `env_vars`, `shader_blobs`,
`msg_send_shapes` (declarations around sema.rs:632-714; consumers at
monomorphize.rs:2479/2643/3012/3071 and codegen.rs:10684). Spans became file-aware only
in v0.0.22 BECAUSE a cross-file collision fired (lexer.rs:4-12); synthesized nodes still
carry `file: 0`. Any pass that fabricates or re-spans expressions silently inherits
mis-keying.

Two live fragility exhibits:

- Coercion re-entry: `text_to_str_coercions` lookups are guarded by
  `text_coercion_suppress.replace(Some(e.span))` (codegen.rs:16918) — a suppress cell
  that already needed one save/restore patch for nesting.
- Default splicing is a three-pass distributed algorithm with a positional contract and
  no assert: lower splices type-free by name+arity (`lower_named_call`,
  lower.rs:292-367; its `method_params` map is keyed by BARE method name across ALL
  types — the cross-type mis-splice bug is memorialized at 329-338); sema records what
  lower could not (`default_splices`); mono appends them (monomorphize.rs:2643-2655) and
  then the bound-method-ref rewrite OVERWRITES `new_args[i+1]`
  (monomorphize.rs:2660-2670) on the contract "the FOLLOWING arg slot is the spliced ctx
  default — sema guaranteed it exists", silently skipping when `i+1 >= len`.

## Plan

1. (S, do now) Add the missing assert at the mono overwrite site: if the bound-method
   rewrite expects a spliced slot at `i+1` and it is absent, panic with a message naming
   the contract — never silently skip. Also have sema record the ctx slot INDEX
   explicitly in the table entry instead of implying "the following slot".
2. (S, do now) Synthesized-span policy: any pass fabricating expressions must copy the
   originating node's span — never `file: 0` / default spans. Audit and fix the known
   fabrication sites: codegen `snapshot_watched` (15905), defer re-emission per return
   site, lower's desugars (if-let/guard-let), mono's synthesized bridges
   (`synth_bound_bridge`). Grep for `Span::default()` / `file: 0` construction in the
   four files; each hit either copies a source span or gets a comment proving no
   span-keyed table can ever see it.
3. (Design note — scope, do not implement) `NodeId`: a u32 stamped at parse time,
   preserved verbatim by every clone/rewrite (issue-01's walker makes this mechanical),
   becoming the required key for the NEXT side table and the migration target for the
   existing eleven. Write the design section in this report's implementation PR: id
   allocation (per-file counter + file id), the walker's preserve rule, and the
   assertion that no two live nodes share an id after lowering.
4. (M, optional) Replace `text_to_str_coercions` + the suppress cell with an explicit
   Coerce node inserted at lower/mono: the coercion becomes AST, codegen just lowers it,
   re-entry cannot exist. Sites: the recording in sema (7879-7886 region), the lookup
   (codegen.rs:10684), the suppress cell (16918). This is also where bug-12's fix wants
   to record its StrLit→Text spans — coordinate so both directions end in the same
   mechanism.

## Verification

- Item 1: an e2e over the bound-method + defaulted-ctx surface (grep e2e for
  bound-method tests); the assert stays silent on the suite.
- Item 2: full suites; specifically the `#[watch]` snapshot tests and defer tests
  (their synthesized nodes are the ones re-keyed).
- Item 4 (if done): string/Text e2e groups plus nested-coercion cases
  (`f(g("${...}"))` shapes).

## Risks and constraints

- Do not convert any table to NodeId ad hoc before the design note exists — a half
  migration doubles the identity schemes in flight.
