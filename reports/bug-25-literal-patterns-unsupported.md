# Bug 25 — Literal patterns unsupported: `match x { 0 => ... }` rejected

- Status: FIXED 2026-08-02, commit aecadfd — `PatternKind::Lit` + a `lower` desugar to
  a temp binding and an if/else chain
- Status (original): reproduced 2026-08-01 with `target/release/cpc check` (known open gap, root cause located)
- Severity: language gap (common match shape unusable)
- Area: parser (`cplus-core/src/parser.rs`) + ast + lower + sema
- Master report: `core-drift-audit-2026-08-01.md` (parser audit, known bug 2)

Context for the fixer: patterns today are only wildcard, binding, and enum-variant.
Matching on an integer/bool literal fails at parse time. This is architectural (the
pattern model has no literal node), not a guard to remove. Build `cargo build --release`;
binary `target/release/cpc`. Line numbers from 2026-08-01.

## Reproduction

```cplus
fn main() -> i32 {
    let x = 5;
    match x { 0 => { return 1; }, _ => {} }
    return 0;
}
```

```
$ target/release/cpc check t8.cplus
error[E0100]: expected pattern, found integer literal
```

The `|0|` spelling fails the same way. Expected: literal patterns work for integers
(with optional leading `-`), bool, and u8 char literals.

## Root cause

- `parse_pattern` (parser.rs:4128-4237) has arms only for `Underscore` and Ident-driven
  forms (binding / `Enum::Variant` / generic-enum). No literal arm.
- `PatternKind` in ast.rs has only Wildcard/Binding/Variant — the model itself lacks the
  node, so lowering and exhaustiveness only reason about variant tags.

## Fix (feature slice, ordered)

1. ast.rs: add `PatternKind::Lit(LitKind)` covering integer (optional leading `-`), bool,
   u8 char.
2. parser.rs: in `parse_pattern`, accept those literal tokens (~20 lines; reuse the
   expression literal token handling for value+span).
3. lower.rs (match desugar) and/or sema match checking: a literal arm lowers to an
   equality test against the scrutinee; exhaustiveness rule: literal arms are never
   exhaustive on integer types — require a reachable `_` or binding arm, otherwise emit
   the existing non-exhaustive error.
4. sema: type-check the literal against the scrutinee type (i32 literal vs i64 scrutinee
   follows the language's literal-typing rules — check how expression literals unify and
   reuse).
5. codegen: verify the lowered form is plain `icmp` + branch chains (should fall out of
   the desugar; no new codegen if lowering produces existing AST shapes — follow the
   pattern the if-let desugar uses: it rewrites to existing nodes so downstream passes
   never see the new kind).

## Verification

1. DONE: `match` on 0 / 1 / -1 / bool dispatches correctly, over i32 and i64 scrutinees,
   as a statement AND as a value, with either `_` or a binding as the catch-all
   (`literal_patterns_dispatch_and_evaluate_the_scrutinee_once` in cpc/tests/e2e.rs). A u8
   CHAR literal is not covered — C+ has no char-literal token; a byte is written as an
   integer and matches as one.
2. DONE: no catch-all → E0344, saying why (each literal covers one value).
3. DECIDED: a clean "not yet". `Some(0)` gives E0341, the same code the payload rule
   already uses for nested variant patterns, with a message pointing at the workaround
   (bind the payload, compare in the body). The desugar handles top-level literal arms
   only. Mixing literal and variant arms in one match is E0343 with its own message.
4. DONE: full suites, the stdlib suite, and vendor-wide diagnostic parity.

## What was built

- ast.rs: `PatternKind::Lit(Box<Expr>)` — the literal as an ordinary expression, so its
  value, suffix and span come from the code expression position already uses, and the
  equality test the desugar builds has its operand ready.
- parser.rs: a literal arm in `parse_pattern_inner`, including a leading `-`.
- lower.rs: `desugar_literal_match` rewrites the whole match to
  `{ let __lit_m<span> = SCRUT; if __lit_m == L1 { .. } else ... else { .. } }`. The temp
  is load-bearing: without it a side-effecting scrutinee (`match tick() { .. }`) would be
  evaluated once per equality test. Pinned by the `CALLS != 1` assertion in the e2e test.
- Everything downstream is unchanged in behavior: sema and codegen carry an
  `unreachable!` for `Lit`, since lower removes it; the resolver and the AST/borrowck/graph
  binding-collectors treat it as binding-nothing, because they walk BEFORE lower.

Step 5's prediction held — no new codegen was needed.

## Collateral

Two stale statements went with it: sema's module header said "E0343: literal pattern not
supported in Phase 3", and the non-enum scrutinee message said "Literal patterns are
deferred (E0343)" — which is now the wrong advice, since the fix is to write literal arms.
Both now describe what the codes actually mean. A borrowck test comment claiming
match-on-int was unreachable is corrected too.

## Notes

- This was one of the two long-open parser-area bugs (noted 2026-07-21). The other is
  bug-13 (statement-boundary misparse family).
- Guard-rail: monomorphize walkers must traverse the new PatternKind if patterns carry
  nested types — check `issue-01-generic-ast-walker.md` interaction if it has landed.
