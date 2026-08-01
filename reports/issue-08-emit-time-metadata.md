# Issue 08 — Attach IR metadata at emission; stop re-parsing our own IR text

- Type: structural consolidation
- Area: `cplus-core/src/codegen.rs`; small guard in `prune.rs`
- Effort: M
- Retires / prevents: bug-09's class (fail-open text matchers); hardens bug-03's fix;
  closes the fail-open seams adjacent to the recorded noalias-miscompile family
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #8)

## Problem

Codegen emits IR text, then three post-passes RE-PARSE that text with substring matchers
to attach metadata and attributes. All three fail OPEN: an instruction shape the matcher
does not recognize silently loses its annotation. bug-09 (`musttail call` missing `!dbg`
→ clang discards ALL debug info) is the live proof; the alias-scope applicator is the
same pattern one shape-change away from mis-attaching aliasing promises — next door to
the project's recorded unsound-IR-attribute family.

## Current state

1. DWARF pass (codegen.rs:1992-1998): matches `starts_with("call ")` /
   `contains("= call ")`; missed `musttail` (bug-09).
2. `attach_sanitizer_attrs` (codegen.rs:1799-1858): re-parses `define ` lines to add
   sanitizer attributes.
3. `annotate_alias_scope_metadata` / `annotate_one_line` / `extract_ptr_operand`
   (codegen.rs:3535-3618): re-parses function bodies matching ` = `, `load `, `, ptr `
   to attach `!alias.scope`/`!noalias`; comments admit bitcast/select/phi propagation is
   unhandled (3588-3590).
4. Same disease, different consumer: prune.rs block detection contracts codegen's exact
   text shape (`define ` at column 0, closing `}` alone at column 0 — prune.rs:163, 173).
   A formatting change turns pruning into a silent no-op (conservative direction, but
   unobservable).

## Target design

One emission funnel:

```rust
fn emit_instr(&mut self, text: &str, meta: InstrMeta) // appends !dbg, scopes, per-line metadata
fn emit_call(&mut self, callee: .., args: .., meta: InstrMeta)
```

- Loads/stores already funnel through `gen_load`/`gen_store` (verify names) — extend the
  funnel to calls and the remaining instruction emitters incrementally.
- `!dbg`: `FnState` knows the current function and source span at emission time — attach
  there; delete the DWARF text pass.
- Sanitizer attrs: known at `define` emission (the emitter knows the fn kind) — attach in
  the signature builder; delete the post-pass.
- Alias scopes: the scope assignment is computed per-fn before body emission
  (post-bug-03-fix content: locals only vs params); loads/stores attach their scope in
  `gen_load`/`gen_store`; the propagation limitation (bitcast/select/phi) disappears
  because annotation happens where the pointer's provenance is KNOWN, not re-inferred
  from text.
- prune.rs: keep its parser (it reads finished modules) but add the guard — if a
  nonempty module parses to zero blocks, emit a loud warning (or debug assert) instead
  of silently keeping everything.

## Migration plan

1. `!dbg` at emission for calls (fixes bug-09 structurally); delete text pass #1 once IR
   tests show every call line carries `!dbg` under `-g`. Tactical matcher fix from
   bug-09 can precede this.
2. Sanitizer attrs into the define builder; delete pass #2.
3. Alias scopes into `gen_load`/`gen_store` (AFTER bug-03's content fix lands, so the
   right scopes get attached); delete pass #3.
4. prune.rs zero-block guard.

## Verification

- Per step: codegen IR-text unit tests updated deliberately; full suites.
- bug-09's repro under `-g` clean; an ASan build still carries sanitizer attrs (grep IR);
  release alias-scope behavior matches bug-03's fixed expectations (probe prints 23 in
  both modes).
- prune: build iris or the largest vendor consumer with and without CPC_NO_PRUNE and
  compare symbol sets (existing prune tests cover this — run them).

## Risks and constraints

- Ordering with bug-03: land bug-03's metadata-content fix FIRST; this issue then moves
  the (correct) attachment mechanism. Doing both at once muddies bisection.
- IR attribute changes are the project's known miscompile family — every step needs the
  release-mode e2e run, not just debug.
