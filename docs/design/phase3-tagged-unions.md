# Phase 3 — Tagged Unions + Pattern Matching

> Status: draft
> Scope: extend the existing `enum` declaration to allow payloads; add `match` expression with exhaustiveness checking; cover construction, destructuring, codegen layout, and interactions with Drop / Copy. **This note is also C+'s error-handling surface** — there is no separate `!T` or `try`; see §6.6.
> Out of scope: generic tagged unions (Phase 7 — `Option[T]`, `Result[T, E]` come with generics), named-field payloads (`Variant { f: T }`), literal patterns (`match n { 0 => ..., _ => ... }`), guards (`Pat if cond`), `if let` / `while let` sugar, drop synthesis for tagged unions with Drop payloads (initial Phase 3 forbids this — see §8).

## 1. Problem

Plain enums (Phase 2A) carry no data — `enum Direction { North, South, East, West }`. They're useful for switching on a discrete state but can't express "the value is X, *and* here's some data associated with X." The two canonical uses in safe systems languages — `Option` (a value or its absence) and `Result` (a value or an error) — both need payloads.

C+ also wants `match` with exhaustiveness checking as the headline Phase 3 safety feature: an unhandled variant should be a compile error, not a runtime fallthrough.

This note picks the syntax and lowering, commits to a Phase 3 scope, and threads the design through Copy / Drop / move tracking.

## 2. Decision — syntax

### 2.1 Variant declarations

Extend `enum` to allow a parenthesized payload per variant:

```cp
enum Maybe {
    Some(i32),
    None,
}

enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Square(f64),
    Empty,
}
```

- Variants without parens are payload-less (the existing Phase-2A form).
- Variants with parens have a positional payload of 1 or more types. Named-field payloads (`Variant { x: i32, y: i32 }`) are **deferred** — keep the spec small in Phase 3.
- Payload types must be already-declared types (forward references are not allowed inside variant payloads in Phase 3; same rule as struct fields gets relaxed once generics exist).

### 2.2 Construction

Path expressions extend to optionally carry a payload:

```cp
let m: Maybe = Maybe::Some(42);
let n: Maybe = Maybe::None;
let s: Shape = Shape::Rectangle(3.0, 4.0);
```

The existing `Name::Variant` syntax (Phase 2A) continues to work for payload-less variants. `Name::Variant(args...)` is the new form for payload variants.

### 2.3 `match` expression

```cp
fn describe(m: Maybe) -> i32 {
    return match m {
        Maybe::Some(v) => v,
        Maybe::None => -1,
    };
}
```

Grammar:

```
match_expr   = 'match' expr '{' match_arm+ '}' ;
match_arm    = pattern '=>' (expr ',' | block) ;
pattern      = path_pat | wildcard_pat | binding_pat ;
path_pat     = ident '::' ident ( '(' pattern_list? ')' )? ;
wildcard_pat = '_' ;
binding_pat  = ident ;       (* not a known path; binds the scrutinee *)
pattern_list = pattern (',' pattern)* ;
```

- A `_` pattern matches anything (no binding).
- A bare lowercase identifier matches anything and binds the scrutinee to that name (Rust style).
- A `Name::Variant(p1, p2)` pattern matches that variant and recursively binds payload positions.

For Phase 3 we restrict patterns to one nesting level: payload patterns inside a variant pattern can only be `_` or bare identifiers (binding patterns). Nested variant patterns (`Maybe::Some(Maybe::Some(v))`) are deferred — they need recursive pattern checking and aren't blocking real programs.

### 2.4 Match is an expression

Every arm produces a value of the same type. The match-expression's type is that arm-type. Arms are either:

- `pattern => expr ,` — short form, expression value
- `pattern => { ... }` — block form, value is the block's tail expression (or `()` if Unit)

Match used as a statement is allowed if every arm is Unit-typed.

### 2.5 Exhaustiveness

Every match must cover every possible value of the scrutinee. The compiler walks the enum's variant list and verifies that for each variant there is either:

- a matching `Name::Variant` arm (with or without payload patterns), or
- a wildcard `_` arm, or
- a binding-pattern arm (`x => ...`) catching everything

Missing variants → E03XX (see §6).

Duplicate / unreachable arms (a later arm covers a variant that an earlier arm already matched) → warning, not error. Phase 3 implements this minimally; subsumption logic is Phase 5/6 polish.

## 3. Semantics

### 3.1 Memory layout

A tagged union lowers to a struct with two fields:

1. **Tag** — `i32`, with value `0..N-1` matching declaration order (same indexing as Phase-2A plain enums).
2. **Payload** — a byte array sized to the largest variant's payload, aligned at 8 bytes (Phase-3 conservative; LLVM's `align` attribute can be tightened later).

```
%Maybe = type { i32, [4 x i8] }       ; i32 fits in 4 bytes; tag + 4-byte payload area
%Shape = type { i32, [16 x i8] }      ; max payload is Rectangle(f64, f64) = 16 bytes
```

Construction stores the tag, then bitcasts the payload area to the variant's layout type and stores the payload.

Match loads the tag, switches on it, and for each arm bitcasts the payload area to that variant's layout type and loads the payload values.

This is the same shape clang produces for `union` types with explicit tag fields in C. No fancy enum-niche optimization in Phase 3.

### 3.2 Plain enums vs tagged unions

Plain enums (Phase 2A: every variant payload-less) **stay** as the special case. Their LLVM lowering is unchanged — a bare `i32`. The compiler detects this case during enum collection and uses the cheaper representation. Mixed enums (some variants with payloads, some without) lower to the full tag+payload struct.

This keeps the existing Phase-2A code paths working and avoids regressing the IR for `enum Color { Red, Green }`.

### 3.3 Copy / Move / Drop

- **Copy:** a tagged union is `Copy` iff every variant's payload type is `Copy` (and the underlying enum is plain → already-Copy). Plain enums stay Copy as before. Recursive structural derivation, same machinery as struct Copy auto-derive (slice 3C).
- **Move:** a non-Copy tagged union is non-Copy → moves consume normally via the existing slice-3A machinery.
- **Drop:** Phase 3 *forbids* declaring a tagged union whose variants have any Drop payload type. The compiler rejects it with E03XX. Reason: synthesizing a tag-aware drop function (which calls each payload's destructor on the live variant) is straightforward but adds complexity that doesn't pay off until we have non-trivial Drop types (heap types — Phase 5+). Users who need this today write a manual `fn drop(mut self) { match self { ... } }`. The compiler allows that path and trusts the user.

In short: tagged unions can hold `i32`, `bool`, `Point` (Copy struct), plain enums, fixed arrays of Copy types. They *cannot* hold a struct with `fn drop` in Phase 3 unless the user writes a manual drop method for the tagged union itself.

### 3.4 Pattern bindings and ownership

A binding pattern in a variant arm consumes the variant's payload **in place** (the payload value is moved out of the scrutinee). For Copy payloads, this is a copy. For non-Copy payloads — same as struct field move-out, which is *deferred* via E0337. Phase 3 restricts variant payloads to Copy types (per §3.3), so the question doesn't arise.

After a match arm runs, the scrutinee's lifetime ends if the scrutinee was a local binding moved into the match — same rule as any other consume. Plain pass-by-value match doesn't consume.

### 3.5 Match arm scope

Each arm body has its own scope. Bindings introduced by the pattern (`Maybe::Some(v) => use(v)`) are visible only in that arm. Bindings drop / defer fire at end of arm scope, normal scope-exit machinery (slices 3F / 3G).

### 3.6 Match exhaustiveness in the type system

The compiler enforces exhaustiveness at sema time, before codegen. The check has access to the scrutinee's full type → variant list. If the scrutinee type is not an enum, the match is rejected with E03XX (only enums are matchable in Phase 3 — `match x: i32` is not yet supported; literal patterns are deferred).

## 4. Sample programs

### 4.1 Must compile and run

`docs/examples/maybe.cplus`:

```cp
enum Maybe {
    Some(i32),
    None,
}

fn first_positive(a: i32, b: i32) -> Maybe {
    if a > 0 {
        return Maybe::Some(a);
    }
    if b > 0 {
        return Maybe::Some(b);
    }
    return Maybe::None;
}

fn main() -> i32 {
    let r1: Maybe = first_positive(0, 7);
    let r2: Maybe = first_positive(-1, -1);
    #println(match r1 {
        Maybe::Some(v) => v,
        Maybe::None => -1,
    });
    #println(match r2 {
        Maybe::Some(v) => v,
        Maybe::None => -1,
    });
    return 0;
}
```

Expected output: `7\n-1\n`.

`docs/examples/shape.cplus`:

```cp
enum Shape {
    Circle(i32),         // radius
    Rectangle(i32, i32), // w, h
    Empty,
}

fn area_times_4(s: Shape) -> i32 {
    return match s {
        Shape::Circle(r) => r *% r *% 12,        // πr² ≈ 3 → 4*area_int approximation
        Shape::Rectangle(w, h) => w *% h *% 4,
        Shape::Empty => 0,
    };
}

fn main() -> i32 {
    #println(area_times_4(Shape::Circle(2)));     // 2*2*12 = 48
    #println(area_times_4(Shape::Rectangle(3, 5)));// 3*5*4 = 60
    #println(area_times_4(Shape::Empty));         // 0
    return 0;
}
```

Expected output: `48\n60\n0\n`.

### 4.2 Must reject

| Program | Error |
|---|---|
| `enum E { A(Buffer), B }` where Buffer has `fn drop` | E03XX — Drop payload in tagged union (write a manual `fn drop` on E) |
| `match m { Maybe::Some(v) => v }` (missing `None`) | E03XX — non-exhaustive match |
| `match m { Maybe::Some(v) => v, Maybe::Some(v) => v }` (duplicate variant) | warning — unreachable arm |
| `match m { Foo::Bar => 0 }` where m: Maybe | E0317 — unknown variant for type |
| `let x: i32 = match m { Maybe::Some(v) => v, Maybe::None => true };` | E0302 — arm type mismatch |
| `match (3 as i32) { 3 => 0, _ => 1 }` (literal pattern) | E03XX — literal patterns not supported in Phase 3 |
| `Maybe::Some()` (missing payload arg) | E0308 — wrong number of payload arguments |
| `Maybe::Some(1, 2)` (extra payload arg) | E0308 |

## 5. Implementation sketch

### 5.1 Lexer

No changes. `match` and `=>` already exist as tokens.

### 5.2 AST

```rust
// Existing EnumDecl already has variants: Vec<Ident> for payload-less.
// Extend to a per-variant payload:
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<EnumVariant>,
}

pub struct EnumVariant {
    pub name: Ident,
    pub payload: Vec<Type>,   // empty for payload-less variants
    pub span: Span,
}

// New expression kind: match
pub enum ExprKind {
    // ...existing...
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,           // block-or-expr, parser normalizes
    pub span: Span,
}

pub enum Pattern {
    Wildcard,                                // _
    Binding(Ident),                          // x
    Variant {
        enum_name: Ident,
        variant_name: Ident,
        payload: Vec<Pattern>,               // empty if no payload syntax
    },
}
```

Path-expression construction `Maybe::Some(v)` is parsed as a regular `Call { callee: Path(...), args }` shape that sema repurposes — no new ExprKind needed at construction sites.

### 5.3 Parser

- `parse_enum_decl` learns to consume an optional `( type (, type)* )` after each variant name.
- `parse_match_expr` for the new `match e { arm, arm, ... }` form; `parse_pattern` for the three forms.
- Match-expression parsing must integrate with the precedence climber as a primary expression.

### 5.4 Sema

- `EnumDef` grows `variant_payloads: Vec<Vec<Ty>>` parallel to `variants: Vec<String>`.
- `compute_enum_copy_flags` (new fixpoint pass, mirror of `compute_struct_copy_flags`): an enum is Copy iff every payload type is Copy.
- `is_atomic_copy` updated: an enum is atomic-Copy only when it's payload-less (the old plain-enum case). Mixed enums go through the cached flag.
- **Construction** (`Name::Variant(args)`): when sema sees a `Call` with a `Path` callee, check whether the path points to an enum variant. If yes: validate arg count and arg types against the variant's payload. If the variant has no payload, the bare `Name::Variant` path (no call) is sufficient.
- **`Match` expression** check:
   1. Type-check the scrutinee.
   2. If scrutinee is not `Ty::Enum`, E03XX.
   3. For each arm: bind the arm's pattern (Variant/Wildcard/Binding), introducing pattern bindings into a new scope.
   4. Pattern arity vs variant payload arity: enforced.
   5. Type-check the arm body in that scope. All arms must have the same result type.
   6. Exhaustiveness: collect the set of variants covered. Reject if any variant is uncovered AND there's no wildcard / binding catch-all.
- Reject tagged unions whose variants reference Drop types (Phase 3 conservative — §3.3).

### 5.5 Codegen

- Compute each enum's `payload_max_bytes` from its variant payloads' sizes (Phase 3: hardcoded sizes per primitive; arrays/structs sum up their fields).
- Two enum layouts:
  - **Plain enum** (no variants have payloads): a bare `i32`, same as Phase 2A.
  - **Tagged enum** (any variant has payloads): `%E = type { i32, [N x i8] }`.
- **Construction:** allocate stack slot, store tag, bitcast payload area to the variant's struct layout, store payload values.
- **Match codegen:** load tag, emit `switch i32` with one case per variant arm, default falls through to the wildcard arm. Each variant arm bitcasts the payload area, loads payload values, binds pattern names, emits the arm body, branches to a join block.
- Result-of-match uses the same `alloca`-then-`load` pattern as `if`/block expressions.

### 5.6 New error codes

| Code | Meaning |
|---|---|
| E0340 | non-exhaustive match (variant not covered) |
| E0341 | match arm pattern doesn't fit scrutinee type |
| E0342 | wrong number of payload arguments to variant constructor |
| E0343 | literal pattern not supported in Phase 3 |
| E0344 | tagged union variant payload type is `Drop` — write a manual `fn drop` |

Duplicate / unreachable arm detection → warning, no error code (Phase 3 just emits a string; refinements in Phase 5).

## 6. Interactions

### 6.1 Phase 2A (plain enums)

Plain enums (zero-payload variants) are the special case. They keep their bare-`i32` lowering. The Phase-2A cast `EnumValue as i32` works the same way. The Phase-2A path expression `Color::Red` is the no-payload-no-args case of the new construction form.

### 6.2 Phase 3 slice 3C (Copy auto-derive)

The Copy rule extends: tagged enum Copy iff every variant's payload is Copy. Same fixpoint mechanism. `SemaCx::is_copy(&Ty)` for `Ty::Enum(id)` reads the cached flag.

### 6.3 Phase 3 slice 3F (Drop)

Drop on tagged unions is forbidden by sema in Phase 3 (per §3.3). A future slice can lift this once we have a real reason to want it (heap types, Phase 5+).

### 6.4 Phase 3 slice 3A (move tracking)

`match e { ... }` consumes the scrutinee iff a payload value is bound out (Phase 3: doesn't happen because payloads must be Copy). Otherwise the scrutinee remains usable — the same shape as `let y = x.field` reading a Copy field.

### 6.5 Phase 4 (modules)

Enum variants are scoped to their enum, accessed via `Name::Variant`. Modules add a layer (`mod::Name::Variant`) but the parser already handles N-segment paths in principle; Phase 4 generalizes.

### 6.6 Errors are tagged unions

**Resolved (after this note was first drafted):** there is no separate `!T` type and no `try` operator in C+. The pre-Phase-3 plan listed both; both are out (see plan.md §2.4 / §2.8 / §2.8b for the FFI-honesty rationale).

Errors are *plain* tagged unions: users declare `enum FileResult { Ok(i32), NotFound, ... }` and write `match` at every fallible call site. The propagation pattern is explicit:

```cp
let r: FileResult = open(path);
let fd: i32 = match r {
    FileResult::Ok(fd) => fd,
    FileResult::NotFound => return SomeOtherError::Wrap,
    FileResult::Permission => return SomeOtherError::Wrap,
};
```

Verbose? Yes — that's the trade. Honesty across the FFI boundary (plan.md §2.8b) plus LLM-as-primary-writer (§2.8c) tip the scales toward "no magic operator." This note's machinery (variant construction, `match`, exhaustiveness) is therefore *also* the language's error-handling surface — no follow-on slice needed.

## 7. Sample-program test plan

Each new sample → one positive e2e test asserting stdout. Negative tests inline in sema (errors_for) per the existing convention.

Positive samples:
- `maybe.cplus` (§4.1) — basic Option-like
- `shape.cplus` (§4.1) — multi-variant + multi-payload

Negative test programs (sema-only, no file):
- non-exhaustive match → E0340
- unknown variant → E0317 (existing code, extended)
- wrong payload arg count → E0342
- arm type mismatch → E0302
- literal pattern → E0343
- tagged union with Drop payload → E0344

## 8. Open questions

- [ ] **Named-field payloads** (`Variant { x: i32, y: i32 }`). Useful for tagged unions whose payloads carry many fields. Deferred to a follow-up slice; the positional form covers Phase 3 needs.
- [ ] **Literal patterns** in `match` (`match n { 0 => ..., _ => ... }`). Needed if we eventually want `match` to subsume `if/else` chains. Deferred. Today: use plain `if`.
- [ ] **Guards** (`pattern if cond => ...`). Common in Rust. Deferred.
- [ ] **`if let` / `while let` sugar.** Common ergonomic. Deferred until enough tagged-union code exists to motivate.
- [ ] **Drop on tagged unions** with Drop variants. Phase 3 rejects this; later slice synthesizes drop logic (load tag, switch, drop the active payload).
- [ ] **Niche optimization.** Rust packs `Option<&T>` into a pointer-sized slot using the null niche. Phase 3 doesn't bother; tagged unions are always `i32 tag + payload bytes`. Phase 6+ work.
- [ ] **`match` as a statement vs expression.** Decided: it's an expression. But statement-position match where the body must be Unit-typed is allowed implicitly — phrasing in the spec needs care, no design decision needed.
- [ ] **Discriminant override.** Rust lets you write `enum Color { Red = 1, Green = 2 }`. Useful for C interop. Deferred to Phase 8.
- [ ] **Nested patterns** (`Maybe::Some(Maybe::Some(v))`). Phase-3 patterns are one level deep. Deferred to a follow-up slice once tagged unions are real.
