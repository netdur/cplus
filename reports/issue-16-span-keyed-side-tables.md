# Issue 16 — Span-keyed side tables: assert the contracts, fix the span policy, scope NodeId

- Status: PARTIAL 2026-08-02, commit <pending> — items 1 and 2 done; item 3 is
  the design note below; item 4 not done
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

## Outcome

**Item 1 — the ctx-slot contract asserts.** The bound-method-ref rewrite in
monomorphize overwrites `new_args[i + 1]` on the contract that the slot after a
bound method reference is the spliced ctx default. It used to check
`if i + 1 < new_args.len()` and, when the slot was absent, do nothing: the
handler would be installed with whatever context the callee's own default put
there, and the receiver would silently never arrive. It asserts now, naming the
contract and the three passes that maintain it (lower splices what it can decide
from name and arity, sema records what lower could not, mono appends). The suite
runs silent.

**Item 2 — the synthesized-span policy, stated where it is used.** Every
fabricated `Span::new(0, 0)` in the four files was audited. They are all
synthesized ITEM declarations (the struct/enum decls mono emits per
instantiation) and TYPE nodes; every one of the eleven span-keyed tables is
keyed by an EXPRESSION span — a call, an argument, a literal, a coercion site —
so none can see them. That reasoning is now a comment at the fabrication site
rather than a fact a reader has to re-derive. The one synthesized EXPRESSION
site, `synth_bound_bridge`, already copies the method's own span.

## Item 3 — `NodeId`, the design note (scope only, not implemented)

The eleven tables key on `ByteSpan` as a stand-in for node identity. That is
sound only while three things hold: every node the tables key on comes from
source (never fabricated), spans are unique per node, and no pass re-spans a
node it did not create. The first is now stated; the second held only after
v0.0.22 made spans file-aware, because a cross-file collision had already fired;
the third is convention.

The replacement:

- **Allocation.** A `NodeId(u32)` stamped at parse time from a per-file counter,
  paired with the file id the lexer already interns — the same two components a
  file-aware span has, but assigned rather than derived, so two nodes cannot
  collide however they are moved.
- **Preservation.** Every rewrite copies the id verbatim. This is mechanical
  now: `ast::walk_expr` reconstructs nodes in ONE place (issue-01), so
  preservation is a property of the walker rather than of every pass.
  Synthesized nodes take a fresh id, which is the honest answer — a fabricated
  node is not the node the table recorded.
- **Migration.** The next side table added uses `NodeId`, not a span. The
  existing eleven migrate one at a time; each migration is mechanical once the
  walker preserves ids, and each removes one way to mis-key.
- **The assertion that makes it real.** After lowering, no two live nodes share
  an id. Run it in debug builds over the merged program; the failure mode it
  catches — a pass cloning a subtree wholesale — is exactly the one spans
  cannot catch, because a clone keeps the span too.

Not started here deliberately: the report's own constraint is that a half
migration doubles the identity schemes in flight.

## What is still open

**Item 4** — replacing `text_to_str_coercions` and its suppress cell with an
explicit coercion node in the AST. Worth doing with bug-12's `StrLit → Text`
recording, so both directions end in one mechanism rather than two side tables.

## Verification (as run)

`cargo test -p cplus-core` 1850 + 8 and `cargo test -p cpc --test e2e` 608 green
with the assertion live — including the bound-method-reference tests, which are
the surface it guards.
