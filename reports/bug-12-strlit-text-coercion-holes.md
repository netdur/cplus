# Bug 12 — StrLit→Text coercion is per-site: enum payloads and generic args miss it

- Status: FIXED 2026-08-02, commit 1c08f2c — one predicate for the rule; enum-payload
  position added on BOTH sides (sema coercion + codegen owning lowering)
- Status (original): probed 2026-08-01 during the audit (spurious E0302 confirmed); re-verify repros before fixing
- Severity: false error (rejects valid programs)
- Area: sema (`cplus-core/src/sema.rs`), with codegen twins
- Master report: `core-drift-audit-2026-08-01.md` (B12)

Context for the fixer: `Text` is the owned string type (stdlib `#[lang("string")]` struct,
tracked as `designated_string_struct`). A string literal is `str`; the compiler coerces a
literal to `Text` where a `Text` is expected. That rule is implemented as an inline
condition repeated at each syntactic position instead of once in `check_expr` — positions
without a copy reject valid code. Build `cargo build --release`; binary
`target/release/cpc`; tests `cargo test -p cplus-core`, `cargo test -p cpc --test e2e`.
Line numbers from 2026-08-01.

## Reproduction

Both need `import "stdlib/text" as text;` (and stdlib dep in a project). Verify each fires
before fixing:

Enum payload:

```cplus
import "stdlib/text" as text;

enum Holder { Some(text::Text), None }

fn main() -> i32 {
    let h = Holder::Some("lit");
    let _ = h;
    return 0;
}
```

Observed in audit: `error[E0302]` on the payload. Control: `fn f(t: text::Text)` called
as `f("lit")` compiles (call-arg position has a copy of the rule).

Generic argument:

```cplus
import "stdlib/text" as text;

fn take_g[T](take v: T) -> i32 { return 0; }

fn main() -> i32 {
    return take_g::[text::Text]("lit");
}
```

Observed in audit: `error[E0302]`.

## Root cause

The same condition — `expr is StrLit && expected type is the designated string struct` —
is copied inline at five positions, each guarding its own site:

- let: sema.rs:7427-7433
- return: sema.rs:7541-7543
- struct-literal field: sema.rs:10264-10283
- call argument: sema.rs:14685-14693
- assignment: sema.rs:16233-16243 (`is_str_lit_to_lang_string`, the only site with a named
  predicate)
- plus a special `==` carve-out at sema.rs:15957-15977.

Enum-constructor payloads and generic-call arguments have no copy, so they reject. Every
future expression position starts rejecting until someone adds a sixth copy. Each sema
site also has a codegen twin keyed by the same detection ("so sema and codegen stay in
lockstep", comment near sema.rs:7425).

## Fix — what was actually done, and why not the preferred one

Both repros were re-verified and both fired.

The generic-argument half was ALREADY closed by bug-01: routing the generic call paths
through `check_arg_with_move` means they inherit that site's copy of the rule. Confirmed —
`g_bare::[text::Text]("lit")` now compiles. `take_g::[text::Text]("lit")` still errors, and
that is CORRECT, not a residual hole: the concrete `fn f(take t: Text)` called as
`f("lit")` errors identically. The rule is deliberately scoped to owning implicit-move
params, because only that call-site lowering (`gen_place_coerced`) materializes the
literal into a temp. The report's "Expected: compiles" for the `take` spelling would have
made the generic path MORE permissive than the concrete one.

The preferred structural move (rule into `check_expr` + span table) was NOT taken, and
should not be taken as stated. Each site's `WHETHER` differs — the arg site excludes
`ref`/`take` for the codegen-capability reason above — so a blanket `check_expr` coercion
would silently start accepting `f(ref t: Text)("lit")`, for which no lowering exists.

What was done instead: the five inline copies now all call `is_str_lit_to_lang_string`,
which already existed with one caller. Each site keeps its own `whether`; none restates
the `what`. Then the missing position was added on BOTH sides:

- sema: the enum-payload arg loop coerces (was: spurious E0302).
- codegen: the enum-variant construction path lowers a `Text`-typed payload literal
  through `gen_strlit_as_lang_string`, the twin of the struct-lit field arm.

The codegen half is not optional. With only the sema fix the program compiled, printed
correctly, and then ABORTED: the payload held a view of the static `@.str` and the enum's
drop called `free()` on a constant.

## Verification

1. DONE: both repros compile. The enum-payload one is run, not just compiled — and under
   `--asan` as well, which is what caught the missing codegen half.
2. DONE: all five original positions still coerce, pinned together with the new one in
   `str_literal_coerces_to_text_in_every_owning_position` (cpc/tests/e2e.rs), which checks
   stdout so a wrong-but-compiling lowering fails too.
3. DONE: full suites green; `ref`/`take` params still reject a literal, matching the
   concrete path.

