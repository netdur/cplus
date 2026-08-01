# Bug 15 — `can_start_expr` drifted from `parse_primary`: `0..this.n` fails to parse

- Status: reproduced 2026-08-01 with `target/release/cpc check`
- Severity: misparse (common method-body shapes rejected)
- Area: parser (`cplus-core/src/parser.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B15)

Context for the fixer: `a..b` ranges may be open-ended (`a..`). To decide, `parse_range`
asks `can_start_expr(next_token)` — a hand-copied duplicate of `parse_primary`'s FIRST
set that has drifted as the grammar grew. Build `cargo build --release`; binary
`target/release/cpc`; parser unit tests in parser.rs; suites `cargo test -p cplus-core`,
`cargo test -p cpc --test e2e`. Line numbers from 2026-08-01.

## Reproduction (verified)

t12 — range bounded by `this` field:

```cplus
struct S { n: i32 }
impl S {
  fn sum(this) -> i32 {
    var t = 0;
    for i in 0..this.n { t += i; }
    return t;
  }
}
fn main() -> i32 { let s = S { n: 3 }; return s.sum(); }
```

→ `E0100: expected '{', found token`. t13 — range bounded by an intrinsic:

```cplus
fn main() -> i32 {
  var t = 0;
  let a: [i32; 3] = [5, 6, 7];
  for i in 0..#len(a) { t += a[i]; }
  return t;
}
```

→ same error. t14 — `0..await f()` also fails. All are valid.

## Root cause

`can_start_expr` (parser.rs:4282-4301; sole consumer: the open-ended-range decision in
`parse_range`, parser.rs:2742) is missing at least: `SelfLower` (`this`), `Pound`
(intrinsics), `At`, `LBracket`, `CStr`, `InterpStr`, `Await`, `Yield`. Tokens absent from
the list make the parser treat the range as open-ended and then choke on the "extra"
expression.

Related diagnostic-quality hole surfaced by the same repro: `tok_name`
(parser.rs:4332-4372) falls through to the string `"token"` for `SelfLower`, `Match`,
`LBracket`, `Struct`, `Enum`, `Impl`, `Interface`, `Pound`, … — hence the unhelpful
"found token".

## Fix

1. Own the predicate next to the grammar it mirrors: define `starts_expr(kind)` adjacent
   to `parse_primary`, derived from the actual primary/prefix token set, and make
   `parse_range` use it. Delete `can_start_expr`.
   Alternative (even more robust): invert the question — treat only tokens that CANNOT
   start an expression as range terminators: `;` `)` `]` `}` `,` `{` and EOF.
2. Make `tok_name` exhaustive: remove the `_` arm so adding a `TokenKind` without a
   display name fails to compile.

## Verification

1. t12 returns 3; t13 returns 18; t14 parses (whatever its semantic fate).
2. Open-ended ranges still work: `0..` in the slicing/iteration positions the suite
   already covers (grep parser and e2e tests for `..`).
3. Error messages: a deliberate misparse now names the real token, not "token".
4. Add parser unit tests for the three repros + one open-ended-range control.
