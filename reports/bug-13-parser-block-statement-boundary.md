# Bug 13 — Statement-position blocks absorb the next statement (E0312 family)

- Status: reproduced 2026-08-01 with `target/release/cpc check` (five misparse forms)
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

## Verification

1. All five repros parse and run (each returns 0).
2. Block-as-value still works: `let x = if c { 1 } else { 2 };`, match-as-value, block
   tail expressions, and `if c { 1 } else { 2 } as i64` if currently legal — grep parser
   tests for `is_block_like` and if-expression tests and keep them green.
3. Builder DSL (`@facet { ... }`) and match-arm parsing untouched: run the full suites —
   `cargo test -p cplus-core && cargo test -p cpc --test e2e` — plus a facet example
   build if available.
4. Add parser unit tests for each of the five forms.
