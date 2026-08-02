# Issue 08 — Attach IR metadata at emission; stop re-parsing our own IR text

- Status: PARTIAL 2026-08-02, commit a9dd146 — steps 2 and 4 done; steps 1
  (`!dbg`) and 3 (alias scopes) not done, see "What is still open"
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

## Outcome

**Step 2 — sanitizer attributes, attached at emission.** `ModuleMetadata`
carries the module's `sanitize_*` string, computed once by
`sanitizer_fn_attrs`, and every emitter appends it where it opens the body: the
nine function/method/coroutine emitters, and the compiler's own glue — the
reactor state accessors, the two `llvm.coro.*` wrappers, both thread
trampolines and the Windows binary-mode ctor. `attach_sanitizer_attrs`, which
re-parsed the finished module for lines starting with `define `, is gone.

`assert_every_define_is_sanitized` runs over the finished module in debug builds
and asserts what the post-pass used to do. That assertion earned its keep
immediately: the first version of this change missed the runtime glue (six
`define`s emitted as raw text, which the old text pass had happily instrumented
along with everything else), and the assertion named the exact line rather than
letting an ASan build quietly not instrument the reactor.

**Step 4 — the prune guard.** `prune_unreachable` reads codegen's finished text
and contracts its exact shape. Finding zero blocks in a module that contains
`define ` now trips a debug assertion instead of silently pruning nothing: the
failure was conservative and therefore invisible — the module just stops
shrinking.

## What is still open

**Step 1 — `!dbg` at emission.** The DWARF pass still re-parses call lines to
attach `!dbg`. Attaching at emission needs the current source span available at
every `emit` — `FnState` knows the function but not the statement — so it is a
threading change through the instruction emitters, not a move. bug-09 fixed the
matcher tactically and `run_clang` now fails hard on clang's "ignoring invalid
debug info", so a regression in this class is loud rather than silent, which
takes the urgency out of it.

**Step 3 — alias scopes.** `annotate_alias_scope_metadata` still re-parses
function bodies. This one attaches aliasing PROMISES, so it is the one that most
wants moving — but it is also next door to the project's recorded
unsound-IR-attribute family, and moving it means deciding scope membership at
`gen_load`/`gen_store` from known provenance rather than re-inferred text. That
is a change worth doing with the release-mode e2e run per rule, not at the tail
of a session.

## Verification (as run)

- `cargo test -p cplus-core` 1847 + 8, `cargo test -p cpc` 607 + 16 + 5 + 6;
  `cpc test` in `vendor/stdlib` 290 green.
- IR identity without sanitizers: `--emit-ll` over 40 `docs/examples` plus the
  ABI probes — unchanged.
- IR identity WITH `--asan`: a probe covering a free fn, a struct method and an
  enum method is byte-identical before and after the move, and both carry
  `sanitize_address`.
- `phase11_asan_attaches_function_attr` and `phase11_ubsan_no_function_attr`
  green (the second pins that UBSan attaches nothing).
