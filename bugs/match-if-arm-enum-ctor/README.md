# Bug: codegen discards the value of an `if`-expression that builds a payload-carrying enum

**Status:** FIXED (v0.0.14) · **Severity:** miscompile (silent wrong value, no
diagnostic) · **Found via:** the `vendor/json` migration to a `ValParse` enum

This directory is kept as the minimal reproducer / documentation. With the fix
in place `cpc build && ./target/debug/repro` now exits **0**. The fix is a new
`Call`-with-`Path`-callee arm in `expr_value_ty_with_bindings`
([cplus-core/src/codegen.rs](../../cplus-core/src/codegen.rs)); regressions live
in `cpc/tests/e2e.rs` (`if_arm_payload_enum_ctor_value_not_discarded`) and in
the codegen unit tests (`if_arm_payload_enum_ctor_forwards_value_to_match_slot`).
The rest of this document describes the defect as originally diagnosed.

A `match` arm (or any value position) whose body is an `if`-expression that
yields a **payload-carrying enum variant** miscompiles: the `if`'s value is
built into a throwaway slot and never stored into the result slot the consumer
reads. The consumer reads uninitialized memory, so the program continues with a
garbage value of the right *type* but wrong *contents* — no crash, no
diagnostic.

This is a **deterministic** miscompile on a clean build (it is not flaky; the
"clean vs dirty build" inconsistency seen during the json investigation was a
stale-artifact red herring). It reproduces at `-O0` (the debug default), so it
is a real codegen logic error, not an LLVM optimization/aliasing artifact.

---

## Reproduce

```sh
cpc build && ./target/debug/repro ; echo "exit=$?"
```

- `exit=0`   → bug is fixed
- `exit!=0`  → bug present (here `exit=199`: the value `7` was lost and the
  match read a stale slot, landing in the `Out::Lo` arm with payload `99`)

The whole reproducer is [src/main.cplus](src/main.cplus) — two tiny enums, no
payload-heavy types, no generics, no `unsafe`, no imports:

```rust
enum Tag { A, B }
enum Out { Hi(i32), Lo(i32) }          // payload-carrying variants

fn pick(t: Tag, flag: bool) -> Out {
    let r: Out = match t {
        Tag::A => { if flag { Out::Hi(7) } else { Out::Lo(8) } }  // if-arm
        Tag::B => Out::Lo(30),                                    // direct arm
    };
    return r;
}

fn main() -> i32 {
    let o: Out = pick(Tag::A, true);   // should be Out::Hi(7)
    let code: i32 = match o {
        Out::Hi(x) => x,               // expect 7
        Out::Lo(_) => 99,
    };
    if code != 7 { return 100 +% code; }
    return 0;
}
```

---

## Root cause

In [`cplus-core/src/codegen.rs`](../../cplus-core/src/codegen.rs), `gen_if`
(~line 12178) decides whether an `if`-expression produces a value, and if so
pre-allocates a single **result slot** that both branches store into and the
merge block reloads:

```rust
let result_ty = self
    .block_value_ty_with_bindings(then)
    .or_else(|| else_branch.and_then(|e| self.expr_value_ty_with_bindings(e)));
let result_slot = match result_ty {
    Some(ty) if ty != Ty::Unit => Some((self.alloca_anon(ty.clone()), ty)),
    _ => None,                       // <-- no slot: branch values are discarded
};
```

That type prediction is done **statically** by
`expr_value_ty_with_bindings` (~line 7049) — a separate, hand-maintained
walk over expression shapes, *not* the real `gen_expr`. Its arms cover:

- `ExprKind::Path { segments }` — **payload-less** enum variants (`Color::Red`) ✓
- `ExprKind::StructLit` / `GenericStructLit` — struct literals ✓
- `ExprKind::Call { callee, .. }` — but **only when `callee` is an `Ident`**
  (free-function calls) ✓
- `ExprKind::GenericEnumCall { .. }` — residual unmonomorphized generic enum
  ctors ✓

A payload-carrying enum constructor like `Out::Hi(7)` is none of these. After
sema/monomorphize it has the shape:

```
Call { callee: Path { segments: [Out, Hi] }, args: [7] }
```

The `Call` arm rejects it (callee is a `Path`, not an `Ident`); the `Path` arm
never sees it (it is wrapped in a `Call`). So `expr_value_ty_with_bindings`
returns `None`, `result_ty` is `None`, and `gen_if` allocates **no result
slot**.

With no slot, each branch still builds its enum literal (into the literal's own
temporary alloca) and loads it to an SSA value — then `gen_block_into_slot` has
`slot = None`, so it **drops the value** and just branches to the merge. The
merge has nothing to reload and `gen_if` returns `None`. The enclosing
`match`-arm lowering (`gen_match`, the value-store at ~line 10152) therefore
sees "this arm produced no value" and stores nothing into the match-result
slot. Only the *direct* arm (`Tag::B => Out::Lo(30)`, a `Call{Path}` handled
directly by `gen_expr`, which has no such gap) writes the match-result slot.

### What the IR shows

`cpc --emit-ll src/main.cplus`, function `@pick` (`%enum.1` is `Out`,
`%a18` is the match-result slot):

```llvm
bb2:                                  ; Tag::A arm  →  the if
  br i1 %t5, label %bb5, label %bb6

bb5:                                  ; if-then: build Out::Hi(7) ...
  store i32 0, ... %a6 ...            ;   tag
  store i32 7, ... %a6 ...            ;   payload
  %t9 = load %enum.1, ptr %a6         ;   ... loaded ...
  br label %bb7                       ;   ... and then DROPPED (never stored)

bb6:                                  ; if-else: build Out::Lo(8), same fate
  %t13 = load %enum.1, ptr %a10
  br label %bb7

bb7:
  br label %bb1                       ; <-- if-merge: NO store into %a18

bb3:                                  ; Tag::B arm (direct)
  %t17 = load %enum.1, ptr %a14
  store %enum.1 %t17, ptr %a18        ; <-- ONLY writer of the result slot
  br label %bb1

bb1:
  %t19 = load %enum.1, ptr %a18       ; <-- reads %a18; UNINITIALIZED on the
  ...                                 ;     Tag::A path → garbage Out value
  ret %enum.1 %t19
```

`%a18` is written only on the `Tag::B` path; the `Tag::A` path reaches `bb1`
having never written it. Taking `Tag::A` returns whatever was on the stack.

---

## Trigger boundary (minimization ladder)

Each row is a one-variable change from the failing case. Confirmed by running
each variant at `-O0`:

| # | Source enum | Arm-body shape | Result type | Result |
|---|-------------|----------------|-------------|--------|
| pure `if`-as-arm | `Tag{A,B}` (no payload) | `if` | `i32` (scalar) | ✓ correct |
| single payload + `if` | `E{A(i32),B}` | `if` referencing payload | `i32` | ✓ correct |
| bare block (no `if`) | `Tag{A,B}` | `{ Out::Hi(7) }` | enum | ✓ correct |
| **payload-less enum** | `Tag{A,B}` | `if` | `Out{Hi,Lo}` (no payload) | ✓ correct |
| **THIS BUG** | `Tag{A,B}` | `if` | `Out{Hi(i32),Lo(i32)}` | ✗ **miscompiles** |

So the bug needs **all** of:

1. a `match` (or other value position) used as a value, where
2. an arm's body is an **`if`** (a nested branch — a bare block does not
   trigger it), and
3. the `if`'s branches yield a **payload-carrying enum constructor**
   (`Variant(args)` → `Call{callee: Path}`).

Scalars, payload-less enum variants, and struct literals are all predicted
correctly and work — the gap is exactly the `Call{Path}` enum-ctor shape.

---

## Why it surfaced in `vendor/json`

The v0.0.14 json rewrite changed the parser's internal result from a struct to
an enum and added this shape to `parse()`:

```rust
let res: result::Result[Value, ParseError] = match r {
    ValParse::Ok(v, rp) => {
        // tail of the arm is an `if` yielding Result::{Ok,Err} — both are
        // payload-carrying enum constructors → the if's value is discarded
        if rp.pos != rp.len {
            result::Result[Value, ParseError]::Err(ParseError { pos: rp.pos })
        } else {
            result::Result[Value, ParseError]::Ok(v)
        }
    }
    ValParse::Fail(rp) => result::Result[Value, ParseError]::Err(ParseError { pos: rp.pos }),
};
return res;
```

For input `"42"` the `Ok` arm is taken, its `if` value (`Result::Ok(v)`) is
dropped, and `parse` returns the stale match-result slot — observed as `parse`
spuriously returning `Err`, or parsed `Value`s reading back as `Null`. The
earlier hypothesis (a multi-payload-enum byte-offset / `static_layout(Value)`
sizing problem) was **wrong**: the multi-payload `ValParse` and the recursive
`Value` are incidental. The defect is the generic if-result-type predictor.

---

## Suggested fix

Teach `expr_value_ty_with_bindings` (codegen.rs ~7049) the payload-carrying
enum-constructor shape, so `gen_if` allocates a result slot. Mirror the existing
payload-less `Path` arm: when an `ExprKind::Call` has a `Path` callee whose
first segment names an enum, return `Ty::Enum(id)`.

```rust
ExprKind::Call { callee, .. } => {
    match &callee.kind {
        ExprKind::Ident(name) =>
            self.sigs.get(name).map(|sig| sig.return_type.clone()),
        // payload-carrying enum constructor: `Out::Hi(7)`
        ExprKind::Path { segments } if segments.len() >= 2 =>
            self.types.enum_by_name.get(&segments[0].name).map(|&id| Ty::Enum(id)),
        _ => None,
    }
}
```

(Also worth checking the `MethodCall`/UFCS enum-ctor spellings if any reach
codegen in this position.)

**Deeper, preferred fix:** `gen_if`/`gen_block_into_slot` should not depend on a
second, hand-maintained type oracle that drifts from `gen_expr`. The robust
shape is to lower the branches first, observe the `Ty` that `gen_expr` actually
returns (exactly how `gen_match` lazily allocates its result slot on the first
value-producing arm at codegen.rs ~10154), and only then materialize the merge
slot — eliminating this whole class of "predictor missed a shape → value
silently dropped" bugs. The v0.0.8/v0.0.9 history of this function (the
`Call`/`StructLit`/`Field`/`Index`/`Unsafe`/`Match`/`Cast` arms were each added
to patch one missed shape at a time) shows the predictor approach is
leak-prone.

After fixing, add an e2e regression covering the table above (at minimum the
payload-carrying-enum row), then re-verify `vendor/json` (`cpc test` in
`vendor/json`, and the `docs/examples/projects/json_smoke` smoke binary).
