# Bug 13 — Statement-position blocks absorb the next statement (E0312 family)

- Status: FIXED 2026-08-02, commit fbd4fcd — a statement-position block ends at end of
  line; same-line continuations still parse as one expression
- Status (original): reproduced 2026-08-01 with `target/release/cpc check` (five misparse forms)
- Severity: misparse (valid programs rejected with misleading errors)
- Area: parser (`cplus-core/src/parser.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B13)

Context for the fixer: statements end at a newline or `;`. `if`/`match`/bare `{}` blocks
are also expressions. After a statement-position block, the parser currently keeps
extending the block expression across the newline — postfix (`(`→call, `[`→index) and
every binary operator — so the NEXT statement is swallowed. Build `cargo build --release`;
binary `target/release/cpc`; parser unit tests in parser.rs's `#[cfg(test)]` module;
`cargo test -p cplus-core`, `cargo test -p cpc --test e2e`. Line numbers from 2026-08-01.

## Reproduction (all five forms verified)

t1 — `(`-statement after `if` block → E0312 (Call whose callee is the If expression):

```cplus
fn main() -> i32 {
    let x = 1;
    if x == 1 { io_noop(); }
    (x + 1);
    return 0;
}
fn io_noop() {}
```

t3 — same after `match`:

```cplus
fn main() -> i32 {
    let x = 1;
    match x { _ => {} }
    (x);
    return 0;
}
```

t4 — same after a bare block:

```cplus
fn f(){}
fn main() -> i32 {
    { f(); }
    (1);
    return 0;
}
```

t2/t5 — `[`-statement → E0100 "expected ']', found ','" (Index misparse); `-x;` →
E0302 "'-' requires numeric operands, found '()'" (binary Sub across the boundary):

```cplus
fn main() -> i32 {
    var x = 1;
    if x == 1 { x = 2; }
    -x;
    return 0;
}
```

t6 — `*f();` → E0302 "'*' requires numeric ..." (binary Mul; this is the common C-style
deref-statement shape):

```cplus
fn main() -> i32 {
    var p = 1;
    if p == 1 { p = 2; }
    *f();
    return 0;
}
fn f() -> *i32 { return 0 as *i32; }
```

All are valid programs; all should parse as two statements.

## Root cause

Block-like expressions are ordinary primaries (parser.rs:4033-4034 →
`parse_if_expr`/`parse_match_expr`), so after a statement-position block BOTH
`parse_postfix_chain` (parser.rs:3143-3199 — LParen→Call, LBracket→Index, Dot→Field) and
the binary precedence cascade (`parse_mul` 2924, `parse_add` 2899, `parse_bit_and` 2857…)
continue across the newline. The statement dispatcher (`parse_block_body`,
parser.rs:2150-2168) only detects block statements afterwards via `is_block_like`
(parser.rs:4275-4280) — by then the continuation has consumed tokens.

Three divergent continuation policies exist for the same question:

1. statement position: continue with everything (the bug) — parser.rs:2150-2168;
2. match-arm position: continue with `as` ONLY, hand-whitelisted — parser.rs:4081-4098;
3. builder-entry position: line-leading `.`/`(`/`[` terminates via `stop_line_dot` +
   the lexer's `nl_before` — parser.rs:3081-3089.

## Fix (model-level, not a lookahead patch)

1. In `parse_block_body`, when a statement starts with `If`, `Match`, or `LBrace`,
   dispatch to a restricted statement-expression parse that returns immediately after the
   closing brace WITHOUT entering the precedence climb or postfix chain (this is Rust's
   `parse_stmt` model for expr-with-block).
2. Tail-expression position keeps working unchanged: when the block is the last
   expression of a fn body, the next token is `}` and the existing branch at
   parser.rs:2158 already handles it.
3. Retire `is_block_like` (its callers become unreachable once dispatch happens up
   front).
4. Leave policies 2 and 3 alone in this change; note in a comment that the match-arm `as`
   whitelist exists because `.`/`(`/`[` could begin the next arm's pattern.

The lexer already stamps `nl_before` on every token (lexer.rs:333-357) if a line-aware
rule is ever preferred, but the statement-position rule above is the smaller, principled
change.

## Which fix was taken, and why not step 1 as written

Step 1 as written — "return immediately after the closing brace", Rust's model — BREAKS
currently-legal code. `{ { 1 } as i64 }` and `{ if c { 1 } else { 2 } as i64 }` both
compile today (they are the tail of a nested block, and a fn body cannot have an implicit
tail at all, so this is where block-as-value actually appears). Under the Rust rule the
`as` follows a statement and is a parse error.

The rule taken instead is the line-aware one the report offers as the alternative: a
statement-position block-like ends at END OF LINE. Every reported form puts the next
statement on a new line, so all six are fixed; every same-line continuation still parses
as one expression. The language already relies on this kind of line sensitivity in the
builder DSL (`stop_line_dot`), so it is consistent rather than novel.

Implementation is `parse_stmt_expr`: parse the block-like primary, and if the next token
is NOT at a line boundary, rewind and re-parse the whole thing as one expression.
Backtracking is safe here — the parser carries no state but position and the two context
flags, and every synthesized binding name is span-derived, not counter-derived.
`is_block_like` is still used (by the caller's stmt-vs-tail decision), so step 3 does not
apply.

## Verification

1. DONE: all six repros parse and run. t3's own repro needed an enum — `match x` on an
   `i32` is rejected by sema for an unrelated reason (literal patterns, bug-25); the
   misparse itself is gone.
2. DONE: `same_line_continuation_of_a_block_still_parses_as_one_expression` pins
   `{ 1 } as i64`, and the nested-block-tail forms were verified by hand.
3. DONE: full suites green; every vendor package produces byte-identical diagnostics
   before and after (compared across all of `vendor/*`), and every `examples/*` project
   has the same error count as before.
4. DONE: `statement_after_a_block_is_its_own_statement` in parser.rs covers `(`, `[`,
   `-`, `*` after both an `if` and a bare block; the e2e test
   `parser_statement_boundaries_and_delimited_struct_literals` runs them.
