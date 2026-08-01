# Bug 16 — Recursion-depth guard has holes: deep patterns abort the compiler

- Status: reproduced 2026-08-01 with `target/release/cpc check` (`fatal runtime error: stack overflow, aborting`)
- Severity: crash (compiler aborts instead of erroring)
- Area: parser (`cplus-core/src/parser.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B16)

Context for the fixer: the parser carries an `enter_depth` guard so hostile/degenerate
inputs produce a clean diagnostic instead of a stack overflow. The guard's own doc
(parser.rs:52-61) claims the parser is bounded. Two recursion paths are unguarded. Build
`cargo build --release`; binary `target/release/cpc`; parser unit tests in parser.rs.
Line numbers from 2026-08-01.

## Reproduction

Generate a deeply nested variant pattern (the audit used 200k deep; the file is ~1 MB so
generate it, do not check it in):

```sh
python3 - <<'EOF'
n = 200000
open("deep.cplus","w").write(
  "enum A { B(i32), C }\n"
  "fn main() -> i32 {\n  let x = A::C;\n  match x { "
  + "A::B(" * n + "_" + ")" * n
  + " => { return 1; }, _ => {} }\n  return 0;\n}\n")
EOF
target/release/cpc check deep.cplus
```

Observed: `fatal runtime error: stack overflow, aborting` (process abort). Expected: the
parser's existing depth-limit diagnostic.

## Root cause

`enter_depth` covers `parse_expr` (parser.rs:2709), `parse_unary` (2970), `parse_block`
(1993), `parse_type` (1731) — but NOT:

- `parse_pattern`'s payload recursion (parser.rs:4202) — the repro above;
- the builder `else if` chain / `parse_builder_entries` nesting (parser.rs:3352) — same
  class for `@facet` blocks.

## Fix

1. Add the same `enter_depth` guard (with its paired exit) to `parse_pattern` and to the
   builder-if recursion.
2. Find the existing depth-limit E-code (read `enter_depth`'s error path) and reuse it —
   no new code.
3. Optional hardening: a debug assertion that every recursive `parse_*` entry point either
   calls `enter_depth` or is documented as non-recursive, to keep the doc at 52-61 honest.

## Verification

1. The generated repro produces the depth-limit diagnostic and exits nonzero cleanly (no
   abort). Reduce n to just above the limit to keep the test fast.
2. A reasonable nesting depth (say 100) still parses — add a parser unit test for a
   moderately nested pattern.
3. Builder nesting: a deep `@facet` else-if chain gets the diagnostic too (unit test with
   a generated string, kept small).
4. Full suites.
