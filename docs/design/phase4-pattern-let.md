# Phase 4 Slice 4A.5 — `if let` / `guard let` (and `while let`)

> Status: `if let` + `guard let` landed. `while let` deferred (blocked on `break`/`continue` not yet being parsed as statements).
> Numbering: slotted as **slice 4A.5** — interstitial between 4A (multi-file modules) and 4B (`pub` visibility). The work depends only on slice 3I (`match`) and is independent of modules; the slot reflects when the slice landed, not a structural dependency.
> Decisions locked in during scoping (2026-05-11): single-binding patterns only (E0352 for multi-binding); `while let` deferred; `else |Pat|` complement form supported with non-overlap (E0350) check; exhaustiveness against the full enum delegated to slice-3I match-arm check (E0343) on the synthesized match.

---

## 1. Problem

§2.4 rejected `try` / `!T` on FFI-honesty grounds (§2.8b). The settled fallback is "write `match` at every fallible call site." For single-call functions this is fine. For realistic functions that chain three or four fallible operations, the result hides the actual flow under boilerplate:

```cp
fn process(path: string) -> ParseResult {
    let content = match read_file(path) {
        Ok(c) => c,
        Err(e) => return Err(ParseError::Io(e)),
    };
    let parsed = match parse(content) {
        Ok(p) => p,
        Err(e) => return Err(ParseError::Bad(e)),
    };
    let validated = match validate(parsed) {
        Ok(v) => v,
        Err(e) => return Err(ParseError::Invalid(e)),
    };
    return Ok(validated);
}
```

§2.8c ("verbosity is acceptable") was supposed to make this fine. In practice, nested fallible chains hide structure rather than expose it — the inverse of what §2.8c aims for. The pattern shows up wherever code does I/O, parsing, or validation; i.e. most code.

**This slice adds Swift-inspired `if let` / `guard let` as pure pattern-match sugar over the slice 3I `match` machinery.** No new control flow. No new types. No FFI implication. Bindings consumers still see plain tagged-union return values.

Rationale recap (for the §11 resolved-log entry that should replace the current "no try" line):

- §2.4 was specifically against `!T` as a *type* and against an error-propagation operator that implied unwinding machinery to FFI consumers. Pattern-match sugar at the call site is neither — function signatures are unchanged, the desugar produces ordinary `match` IR, and any binding consumer sees an ordinary tagged-union return.
- §2.8c is preserved by reframing: **verbosity is acceptable when it makes hidden rules visible; sugar is acceptable when it makes structure visible.** `match`-with-early-return hides the success path under boilerplate; `guard let` exposes it. The sharper §2.8c is recorded as part of this slice.

---

## 2. Syntax

Three forms. Existing keywords: `if`, `let`, `else`, `while`. One new reserved word: `guard`.

### 2.1 `if let`

```
if let PATTERN = EXPR { BODY }
if let PATTERN = EXPR { BODY } else { ELSE_BODY }
```

`PATTERN` must be refutable. `if let x = e { ... }` (irrefutable binding) is rejected as **E0347** — that's just `let`.

### 2.2 `guard let`

```
guard let PATTERN = EXPR else { DIVERGE_BODY };
guard let PATTERN = EXPR else |COMPLEMENT_PATTERN| { DIVERGE_BODY };
```

The else block must diverge — every control path through it must `return`, `break`, `continue`, or call a no-return function (**E0348**). Until C+ grows a `!` no-return type, the check is syntactic: the block's terminator must be one of those keywords, or the last statement must be a call to a function whose return type is `Never` (also nonexistent today, so practically: `return` / `break` / `continue` only, plus a future intrinsic like `trap()`).

Bindings introduced by `PATTERN` live in the **enclosing scope** after the guard statement, not just inside an arm body. This is the property that flattens nested chains.

The optional `else |COMPLEMENT_PATTERN|` form binds whatever `PATTERN` doesn't match. The compiler requires `COMPLEMENT_PATTERN` together with `PATTERN` to cover the scrutinee's type exhaustively (**E0349**) — same machinery as `match` exhaustiveness (slice 3I).

### 2.3 `while let`

```
while let PATTERN = EXPR { BODY }
```

Body runs whenever `PATTERN` matches `EXPR`. Loop terminates on first non-match.

**Depends on `break` being in the language.** If `break` is not yet implemented (see Phase 1 grammar — it isn't listed), `while let` ships in a later slice or in this one bundled with `break`/`continue`. See §7 open questions.

---

## 3. Examples

```cp
// 3.1 — Optional-style unwrap with default
fn get_or_default(m: Maybe) -> i32 {
    if let Some(v) = m { return v; }
    return -1;
}

// 3.2 — Wrap-and-rethrow with else-pattern binding (the motivating case)
fn process(path: string) -> ParseResult {
    guard let Ok(content) = read_file(path) else |Err(e)| {
        return Err(ParseError::Io(e));
    };
    guard let Ok(parsed) = parse(content) else |Err(e)| {
        return Err(ParseError::Bad(e));
    };
    return Ok(parsed);
}

// 3.3 — Conditional consumption, no else needed
fn maybe_log(result: ParseResult) {
    if let Err(e) = result {
        log_error(e);
    }
}

// 3.4 — Two-arm if let (substitute for two-arm match where one arm is "the rest")
fn describe(m: Maybe) -> i32 {
    if let Some(v) = m {
        return v;
    } else {
        return 0;
    }
}

// 3.5 — while let iterator drain (depends on break)
fn sum_stack(mut s: Stack) -> i32 {
    let mut total = 0;
    while let Some(v) = s.pop() {
        total = total + v;
    }
    return total;
}
```

---

## 4. Semantics — pure desugar into `match`

The lowering happens in the parser or in an early sema pass; the AST after desugar is indistinguishable from a hand-written `match`. Codegen unchanged.

### 4.1 `if let` desugar

```
if let P = E { B }
==>
match E { P => B, _ => () }
```

```
if let P = E { B1 } else { B2 }
==>
match E { P => B1, _ => B2 }
```

In statement position both arms return unit. In expression position (future) the arms must agree on type — same rule as slice 3I match.

### 4.2 `guard let` desugar

The trick: bindings from `PATTERN` must survive past the guard statement. Desugar uses a let-binding fed by a `match` expression that returns the bound payload, with the else branch diverging:

```
guard let Ok(content) = read_file(path) else { return Err(X); };
// continuation uses content
==>
let content = match read_file(path) {
    Ok(__c) => __c,
    _ => { return Err(X); }
};
// continuation uses content
```

With `else |Pat|`:

```
guard let Ok(content) = read_file(path) else |Err(e)| { return Err(IoError::Io(e)); };
// continuation
==>
let content = match read_file(path) {
    Ok(__c) => __c,
    Err(e) => { return Err(IoError::Io(e)); }
};
```

**Multi-binding patterns** (e.g. `guard let Pair(a, b) = ...`) generate one `let` per binding, all fed by a single match. Implementation detail: introduce an internal tuple/anonymous-struct return, then destructure. For Phase 3K, restrict to single-binding patterns (the common case); revisit if needed.

### 4.3 `while let` desugar

```
while let P = E { B }
==>
while true {
    match E {
        P => B,
        _ => break,
    }
}
```

Depends on `while true` being legal (it is) and `break` (verify).

### 4.4 Diverge enforcement for `guard let`

The else block of `guard let` cannot fall through. The check is purely syntactic for Phase 3K:

- The block's last statement must be `return EXPR;`, `break;`, `continue;`, or `return;` (in unit-returning functions).
- Any branching inside the else block must have all arms terminate the same way (e.g. an `if`/`else` where both branches `return`).
- Calling a no-return function counts when C+ has them; not in scope for 3K.

Reuses match-arm flow analysis from slice 3I, which already proves each arm either produces a value of the result type or diverges.

### 4.5 Exhaustiveness check for `else |Pat|`

`PATTERN` and `COMPLEMENT_PATTERN` together must cover the scrutinee's type. The check:

1. Treat them as the two arms of a synthetic `match` and run the existing exhaustiveness checker.
2. If non-exhaustive → **E0349**.
3. If `COMPLEMENT_PATTERN` overlaps `PATTERN` (i.e. accepts values `PATTERN` would also have matched) → reject as **E0350** (overlapping patterns in guard-else complement). This stops users from writing misleading code where the same value could go to either branch in a hypothetical reordering.

---

## 5. Interactions

### 5.1 Definite assignment (slice 3J)

A `guard let P = E else { B };` binding behaves like a regular `let` initializer at the point after the statement — the binding is fully assigned in the continuation. Flow merging in 3J already handles the diverge-on-else case correctly because `B` is proven to diverge (so the post-merge state is unconditionally "assigned").

An `if let P = E { B }` binding is in scope **only inside B**. The continuation after the `if` does not see the binding. Same as match arm scoping today.

An `if let P = E { B1 } else { B2 }` binding is in scope **only inside B1**. `B2` does not see it.

### 5.2 Drop (slice 3F)

Bindings introduced by `guard let` register their scope-exit drop hook at the position of the `guard let` statement, in the enclosing scope. Same registration order, same reverse-LIFO drop, same drop-flag suppression on move.

Bindings from `if let` register inside the body block; they drop at body block exit. Symmetric with let-inside-block today.

### 5.3 Move tracking (slice 3A)

`guard let Ok(content) = read_file(path) else { ... };` — the move-tracking machinery sees this as `let content = <expr>;` after desugar. If `Ok`'s payload is non-`Copy`, the binding `content` owns it; using `result` (the scrutinee, if it had a name) afterward would be use-of-moved unless that's already its own thing. Standard scrutinee-consumption semantics from match.

### 5.4 `match` expression-position parity

`if let` lives in **statement position only** in Phase 3K. Expression-position (`let x = if let Some(v) = m { v } else { 0 };`) is allowed because it desugars to a match expression, which is already an expression in slice 3I. Verify in tests; no extra parser work expected.

`guard let` is statement-only. Cannot appear in expression position. (The whole point is to extract a binding into the enclosing scope.)

### 5.5 Style rules (§2.8a)

- Block bodies of `if let`, `else`, and the `guard let` else clause follow normal block rules: explicit `return` at function-body level still required if the block is the function's final tail (it can't be — `guard let` is a statement, not a tail expression).
- `::` vs `.` rules unchanged: patterns use `::` for variant paths (`Ok`, `Err`, `Maybe::Some`) just like in slice 3I match.

---

## 6. Implementation plan

Order: lexer → parser → AST → sema → tests. No codegen changes.

### 6.1 Lexer

- Reserve `guard` as a keyword.
- Update keyword table; add unit test confirming `guard` tokenizes as `Tok::Guard`.

### 6.2 Parser

- New AST nodes (or extend existing): `Stmt::IfLet { pattern, expr, body, else_body: Option<Block> }`, `Stmt::GuardLet { pattern, expr, complement: Option<Pattern>, else_body: Block }`, `Stmt::WhileLet { pattern, expr, body }`.
- Parse rules:
  - `if let` competes with `if` — lookahead one token after `if` to see `let`.
  - `guard let` is unambiguous (new keyword).
  - `while let` competes with `while` — lookahead after `while` for `let`.
- The `else |Pat| { ... }` form for `guard let`: after parsing `else`, if next token is `|`, parse a pattern then expect a closing `|`, then the block.

### 6.3 Desugar (in sema or a pre-sema pass)

Two options:

- **A. Desugar in the parser** into existing match-expression AST nodes. Simpler; means no IfLet/GuardLet nodes survive into sema.
- **B. Keep IfLet/GuardLet as first-class AST nodes**, desugar to match during sema's lowering to IR. Preserves source positions in diagnostics.

Recommended: **B**. Diagnostics referring to "the guard binding" are clearer than diagnostics that mention an internal match arm the user never wrote. Cost is small (the sema code path mostly delegates to existing match check).

### 6.4 Sema

- Refutability check on `if let` pattern → E0347.
- Diverge check on `guard let` else block → E0348. Reuses match-arm flow analysis.
- Exhaustiveness + non-overlap on `guard let` complement → E0349, E0350.
- Binding scope propagation: `guard let` adds binding to enclosing scope after the statement; `if let` adds to body block only.
- Definite-assignment integration: same as `let` for `guard let` (assigned-true after the statement, given the else-diverges proof); inside-body-only for `if let`.

### 6.5 Codegen

Nothing new — the desugar produces match IR, which slice 3I already lowers.

### 6.6 Tests

- **Parser tests:** each form parses; ambiguity with bare `if` / `while` / `guard` (as ident-context, which is now an error) handled cleanly.
- **Sema tests, positive:** each form type-checks; bindings visible in expected scopes; complement-pattern exhaustiveness accepted; diverge enforcement accepted.
- **Sema tests, negative:** irrefutable `if let` → E0347; non-diverging guard else → E0348; non-exhaustive complement → E0349; overlapping complement → E0350.
- **E2E samples:**
  - `if_let_basic.cplus` — guard-or-default on a `Maybe`.
  - `guard_let_chain.cplus` — the §3 example 3.2 pattern with two chained guards.
  - `guard_let_complement.cplus` — uses `else |Pat|` to wrap an error.
  - `while_let_drain.cplus` — only if `break` exists; otherwise defer.
- Test count target: roughly **+15 sema tests + 4 e2e tests** based on prior slice scaling.

### 6.7 Plan-doc updates

- §2.4: replace the "no `try` / `!T`" paragraph with a more careful version that rejects `!T` and propagation-via-unwinding while explicitly allowing pattern-match sugar.
- §2.8c: replace the "verbosity is acceptable" framing with the sharper version ("verbosity exposes hidden rules; sugar exposes structure"). Both directions are honored.
- §11 resolved log: add an entry for this slice, replacing the current "no `try` / `!T`" line with the more precise rejection.
- §3 Phase 3 section: append slice 3K under the existing 3A–3J list.

---

## 7. Open questions

- **`while let` depends on `break`/`continue`.** Verify whether either is in the language. If not, decide: (a) add `break`/`continue` in this slice, (b) defer `while let` to a follow-up slice, (c) reject `while let` for now and revisit. Recommendation: (b) — keep slice 3K focused on the pattern-binding sugar; do `break`/`continue` + `while let` + `loop` in one later slice.
- **Multi-binding patterns in `guard let`** (e.g. `guard let Pair(a, b) = ...`). Phase 3K can restrict to single-binding (the common case). Revisit when a real example demands it.
- **`if let` chains** (Rust 2024: `if let Some(x) = a && let Some(y) = b { ... }`). Probably no — adds parsing complexity for a niche case. Users write nested `if let` or a single `match` on a tuple.
- **`guard case` / pattern-matching guards in `match`** (e.g. `match e { Ok(x) if x > 0 => ... }`). Not in slice 3I; not in this slice; revisit when match guards become useful.
- **No-return type `!`** for proper diverge enforcement instead of the syntactic check. Worth its own design note when it matters (function attribute, exit handling, etc.).
- **Naming the new keyword.** `guard` is the Swift choice and is reserved here. Alternatives considered: `assume` (Eiffel-flavored, but implies static reasoning), `letelse` (Rust's, but ugly). `guard` is the right read for what it does — "guarantee this pattern holds past this point, or leave."

---

## 8. Non-goals

- No change to `match` itself.
- No change to function signatures, calling conventions, or FFI.
- No new error-handling primitives. Errors remain plain tagged unions; the only addition is sugar over inspecting them.
- No `try!` / `try?` Swift forms. Those are different features (crash-on-failure, optional conversion). The single-feature scope here is "extract a payload, or diverge."

---

## 9. Summary

One new keyword (`guard`). Three statement forms. Pure desugar into existing `match`. ~15 sema tests + 4 e2e tests. No codegen changes. Doc-only edits to §2.4, §2.8c, §11.

Solves the only realistic pain point of the no-`try` decision (nested fallible chains) without compromising any of the principles that drove it.