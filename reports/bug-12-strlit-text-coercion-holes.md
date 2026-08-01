# Bug 12 — StrLit→Text coercion is per-site: enum payloads and generic args miss it

- Status: probed 2026-08-01 during the audit (spurious E0302 confirmed); re-verify repros before fixing
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

## Fix

Structural (preferred — this is how the OPPOSITE direction already works):

1. Move the rule into `check_expr`'s expected-type path, exactly like the Text→str
   coercion at sema.rs:7879-7886: when `expected` is the designated string struct, the
   actual is `str`, and the expression is a `StrLit`, record the span in a coercion table
   and return the struct type.
2. Codegen consumes the recorded spans (one table lookup at literal emission) instead of
   re-detecting per site; delete the five inline sema copies and their codegen twins.
3. Re-express the `==` carve-out through the same mechanism if possible (thread the
   expected type into the BinOp-Eq operand check).

Tactical (if the table change is too wide now): add the missing condition to the
enum-payload check and both generic-arg paths — noting each is another copy of the drift.

## Verification

1. Both repros compile; `Holder::Some("lit")` constructs a live `Text` (print it via a
   match to verify content end-to-end).
2. All five existing positions still coerce (let/return/field/arg/assign) — the e2e suite
   has string tests; grep for `Text` coercion tests and run full suites.
3. Negative control unchanged: passing a literal where `str` is expected stays `str`
   (no accidental double-coercion), and `Text == Text` still errors if that is the current
   contract (check the E0302 carve-out tests).
