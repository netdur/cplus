# bug-30 — an intrinsic inside an infinite loop makes the function demand an unreachable `return`

- Status: FIXED 2026-08-03 — `expr_can_break` recurses into the composites it
  used to fall through on.
- Severity: false rejection of valid code (E0306), user-visible, pre-existing
- Area: `cplus-core/src/sema.rs` (`expr_can_break`)
- Found by: finishing issue-18 — it showed up as 2 suppressed diagnostics in
  `channel.cplus`, and reduced to something with no generics in it at all.

## Repro

```cplus
enum R { Value(i32), Closed }
fn recv() -> R {
    loop {
        if #size_of::[i32]() > (0 as usize) { return R::Value(1); }
        let x: usize = (1 as usize);
    }
}
fn main() -> i32 { return match recv() { R::Value(v) => v - 1, R::Closed => 9 }; }
```

```
error[E0306]: function body must end with `return ...;` for type `enum`
```

The loop has no `break`, so the function diverges and the demanded `return`
would be unreachable — there is nowhere to put it. Replace the condition with
`1 > 0` and it compiles. ANY intrinsic anywhere in the loop is enough;
`#size_of`, `#align_of` and `#str_len` were all confirmed.

Present identically on a binary built at `3a7601d`, and reproducible with no
generics involved — the generic spelling is just where it was noticed.

## Cause

`body_returns_or_diverges` accepts a trailing `loop` when `!loop_can_break`,
which walks the loop for a statement-level `break`. Its expression half,
`expr_can_break`, names the composite kinds it recurses into, lists the true
leaves as `false`, and ends with `_ => true`.

The catch-all's comment called this conservative: "a `break` buried in a
sub-expression is possible in theory but vanishingly rare — stay conservative
(treat the loop as breakable)". Two things were wrong with that. It is not
rare — `ExprKind::Intrinsic` is the catch-all's most common member, and every
`#size_of` in a loop hit it. And the conservative direction it picked is only
conservative for ONE of the two answers: reporting "can break" for a loop that
cannot means the function is judged to fall off its end, which is a false
rejection of code that has no fix.

(A `break` cannot in fact appear inside an intrinsic's arguments at all —
`break` is a statement, not an expression. The only route is a nested block,
which the `ExprKind::Block` arm already handles.)

## Fix

The composites get their arms: `Intrinsic`, the three struct-literal forms,
`ArrayLit` / `TupleLit` / `GenericEnumCall`, `ArrayFill`, and `InterpStr` all
recurse into their sub-expressions. `FnRef` joins the leaves.

The catch-all stays `_ => true`, and the comment now says why that is the safe
default rather than calling it conservative in general: claiming a breakable
loop diverges would let a function fall off its end with no return at all,
which is worse than a false error.

## Verification

- `cpc/tests/e2e.rs:an_infinite_loop_containing_an_intrinsic_still_diverges` —
  three intrinsic conditions compile, plus the control that a loop which CAN
  break still requires the trailing return (E0306).
- `cargo test -p cplus-core` 1869 + 8; `cargo test -p cpc` 619 + 16 + 5 + 6.
- Vendor + examples `cpc check` over 274 sources: byte-identical against
  `3a7601d`. Nothing in the tree was written in the shape that tripped it,
  which is why it survived — the two live instances are inside `channel.cplus`
  generic bodies, which nothing checked until issue-18.
