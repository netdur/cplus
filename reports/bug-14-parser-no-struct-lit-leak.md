# Bug 14 — `no_struct_lit` never restored at delimiter recursion (three misparses)

- Status: FIXED 2026-08-02, commit fbd4fcd — `ExprCtx` + one `in_delimited` combinator
- Status (original): reproduced 2026-08-01 with `target/release/cpc check`
- Severity: misparse (documented escape hatch does not work)
- Area: parser (`cplus-core/src/parser.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B14)

Context for the fixer: after `if`, `for … in`, `while`, and similar headers, the parser
sets a `no_struct_lit` flag so `if x == y { … }` does not parse `y { … }` as a struct
literal. Inside parentheses/brackets the ambiguity disappears, so the flag should reset —
it never does. Its sibling flag `stop_line_dot` IS reset at every delimiter, which is the
pattern to copy. Build `cargo build --release`; binary `target/release/cpc`; parser unit
tests in parser.rs; full suites via `cargo test -p cplus-core` and
`cargo test -p cpc --test e2e`. Line numbers from 2026-08-01.

## Reproduction (all verified)

t9 — struct literal inside a call inside an `if` header:

```cplus
struct Foo { x: i32 }
fn check(f: Foo) -> bool { return f.x == 1; }
fn main() -> i32 {
    if check(Foo { x: 1 }) { return 1; }
    return 0;
}
```

→ E0100. t10 — parenthesized, which the flag's own documentation (parser.rs:71-75,
"Force the literal by parenthesizing") says must work:

```cplus
struct Foo { x: i32 }
fn main() -> i32 {
    if (Foo { x: 1 }).x == 1 { return 1; }
    return 0;
}
```

→ E0100. t11 — array literal element in a `for` header:

```cplus
struct Foo { x: i32 }
fn main() -> i32 {
    for i in [Foo { x: 1 }] { return i.x; }
    return 0;
}
```

→ E0100. All three are valid programs.

## Root cause

`no_struct_lit` is set at 9 header sites (parser.rs:2448, 2480, 2513, 2534, 2645, 3347,
3368, 4053, 4242) and consulted at 3715, 3781, 3867, 3905, 4020 — but NO delimiter
recursion clears it. The sibling `stop_line_dot` is cleared at every delimiter via
`with_line_dots_allowed` (parser.rs:265, 3189, 3925/3929, 3961, 1994). Two flags, one
threading discipline applied to only one of them.

`stop_line_dot` itself is ALSO missed at a few recursion sites: enumerated-array sibling
elements (parser.rs:4000 — the first element at 3961 resets it, siblings do not),
generic-enum-ctor args (3752-3757, 3837-3842), intrinsic `#name(...)` args (3650-3654),
`#asm` operand values (~700).

## Fix

1. Introduce one expression-context holder: `ExprCtx { allow_struct_lit: bool,
   line_sensitive: bool }` on the parser (replacing the two loose flags).
2. Add a single combinator `in_delimited(|p| …)` that saves the ctx, resets BOTH fields to
   the neutral state (struct literals allowed, line-insensitive), runs the closure, and
   restores — and use it at EVERY `(` `[` `{` recursion into expression parsing.
3. Migrate the 9 set-sites and all consult-sites to the ctx; delete
   `with_line_dots_allowed` in favor of the one combinator.
4. Fix the missed `stop_line_dot` sites listed above by routing them through the same
   combinator.

This makes both flags impossible to leak by construction; a future third context flag
joins the struct instead of repeating the disease.

## Verification

1. DONE: t9 and t10 compile and run. t11 parses; `for … in` over an array literal is
   rejected for an unrelated reason (E0312 — the iterator must be a range or an
   `Iterator[T]`), which is a language limitation, not the misparse.
2. The original ambiguity stays resolved: `if x == y { return 1; }` where `y` is a plain
   variable must still parse the `{` as the if-body (grep parser tests for no_struct_lit
   / struct-literal-in-if tests).
3. Builder-DSL line sensitivity unchanged (stop_line_dot semantics inside `@facet` blocks)
   — the e2e suite plus a facet example build is the guard.
4. Add parser unit tests for t9/t10/t11 and one builder-entry regression.
