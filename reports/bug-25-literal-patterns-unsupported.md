# Bug 25 — Literal patterns unsupported: `match x { 0 => ... }` rejected

- Status: reproduced 2026-08-01 with `target/release/cpc check` (known open gap, root cause located)
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

1. The repro compiles; `match` on 0/1/-1/bool/u8-char all dispatch correctly (runtime
   e2e with asserted returns).
2. Non-exhaustive literal match without `_` → error (negative e2e).
3. Nested: `Some(0)`-style variant-with-literal-payload patterns if step 1 allows nesting
   — decide and test either support or a clean "not yet" diagnostic.
4. Full suites.

## Notes

- This was one of the two long-open parser-area bugs (noted 2026-07-21). The other is
  bug-13 (statement-boundary misparse family).
- Guard-rail: monomorphize walkers must traverse the new PatternKind if patterns carry
  nested types — check `issue-01-generic-ast-walker.md` interaction if it has landed.
