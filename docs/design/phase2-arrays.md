# Phase 2 Slice 2D — Fixed-Size Arrays

> Status: draft
> Scope: `[T; N]` fixed-size arrays with bounds-checked indexing
> Out of scope (defer): slices `T[]`, raw pointers `*T`, multi-dimensional indexing sugar, repetition syntax `[0; N]`

## 1. Problem and scope decision

The original Phase-2 plan §3 bundled three features: fixed arrays, raw pointers, slices. They naturally split into two pieces:

- **Fixed-size arrays** are an aggregate value type. They fit cleanly alongside structs.
- **Slices** and **raw pointers** are address-based and interact directly with the Phase-5/6 borrow checker (`&[T]`, `&T`, `*T` semantics). Implementing them now means designing the same machinery twice.

This slice handles fixed arrays only. Slices + raw pointers are deferred — they'll land in Phase 3 (with non-null pointers, `?*T`) or Phase 5/6 (with the borrow checker proper).

What we *do* get from this slice alone: stack-allocated arrays usable as function parameters, return values, struct fields, and local variables. Enough to write programs that "walk arrays of structs" (the original Phase-2 exit criterion).

## 2. Type representation

`Ty` becomes `Clone` instead of `Copy`. New variant:

```rust
Ty::Array(Box<Ty>, u32)   // element type + length
```

The `Box` makes `Ty` non-Copy. This cascades to `LocalInfo`, `MethodSig`, etc. which all hold or transit `Ty`. The refactor is ~20 `.clone()` insertions across sema and codegen — manageable, and gives us the right model for future generics (`Vec[T]`) and slices.

Alternative considered: keep `Ty: Copy` via interning (a global `ArrayId` registry shared between sema and codegen). Rejected — would require both subsystems to walk the AST in lockstep when interning, and that's brittle.

## 3. Syntax

```cp
let xs: [i32; 5] = [1, 2, 3, 4, 5];        // literal of length 5
let row: [i32; 3] = [10, 20, 30];
let v: i32 = xs[0];                          // bounds-checked indexing
let mut ys: [i32; 4] = [0, 0, 0, 0];
ys[2] = 42;                                  // indexed assignment

fn sum(xs: [i32; 5]) -> i32 { ... }          // pass by value
fn make_pair() -> [i32; 2] { [1, 2] }        // return by value

struct Buf { data: [u8; 16], len: usize }    // array as struct field
```

Grammar:

```
type             = type_path | array_type ;
array_type       = '[' type ';' int_lit ']' ;

primary          = ... | array_lit ;
array_lit        = '[' expr (',' expr)* ','? ']' ;

postfix          = primary (call_suffix | field_suffix | index_suffix)* ;
index_suffix     = '[' expr ']' ;
```

No empty array literal `[]` in this slice — element type can't be inferred without an annotation, and even with one, deferring `[]` simplifies the parser. Empty literals come back when we have richer inference.

No repetition syntax `[0; N]` in this slice. Useful for zero-init; can add later.

## 4. Semantics

- **Element type uniformity**: all elements in a literal must have the same type. The first element's type sets the expectation; subsequent elements check against it. (E0329)
- **Literal length must match annotation**: `let xs: [i32; 5] = [1, 2, 3]` is a length mismatch error. (E0330)
- **Indexing**: `a[i]` requires `a: [T; N]` (other indexable types come later) and `i: usize`. Numeric literal indices infer to `usize` by the existing literal-inference rule.
- **Bounds check**: runtime `icmp uge i, N` (unsigned compare; index must be `< N`); on out-of-bounds, branch to a trap block.
- **Indexed assignment**: `a[i] = v` requires `a` to be a writable place (extends the place-walk to `Index` chains).
- **Equality on arrays**: rejected in Phase 2D (need element-wise comparison; revisit later).
- **Pass-by-value**: LLVM aggregate parameter `[N x T]`. ABI may insert `byval` automatically; we don't worry about it.

New error codes:

| Code | Meaning |
|---|---|
| E0329 | mixed element types in array literal |
| E0330 | array literal length doesn't match declared length |
| E0331 | indexing a non-array type |
| E0332 | array literal must have at least one element (no `[]` in this slice) |

## 5. LLVM mapping

- `[T; N]` → `[N x <T>]` (LLVM array type)
- Array literal: alloca + per-element `getelementptr` + `store`, then `load` the whole aggregate
- Indexing `a[i]`:
  1. `getelementptr` to the array's base
  2. bounds check: `%c = icmp uge i64 %i, N; br i1 %c, label %trap, label %ok`
  3. `getelementptr [N x T], ptr %arr, i64 0, i64 %i` → element ptr
  4. `load <T>, ptr %elt`
- Indexed assignment: same GEP, but `store` instead of `load`
- Pass-by-value: function signature has `[N x T]` parameter type

## 6. Sample programs

### 6.1 Must compile and run

[array_sum.cplus](#) — array literal, for-in, indexing:

```cp
fn main() -> i32 {
    let xs: [i32; 5] = [1, 2, 3, 4, 5];
    let mut total: i32 = 0;
    for i in 0..5 {
        total = total + xs[i as usize];
    }
    println(total);     // 15
    0
}
```

Wait — `0..5` produces `i32` (default integer type), not `usize`. Indexing needs `usize`. The cast `i as usize` is required. Let me lean into that — explicit casts everywhere is consistent with the design (no implicit conversions).

[array_struct.cplus](#) — array as struct field, indexed write:

```cp
struct Counters { values: [i32; 3] }

fn main() -> i32 {
    let mut c: Counters = Counters { values: [0, 0, 0] };
    c.values[0] = 100;
    c.values[1] = 200;
    c.values[2] = 50;
    let total: i32 = c.values[0] + c.values[1] + c.values[2];
    println(total);     // 350
    0
}
```

### 6.2 Must reject

| Program | Error |
|---|---|
| `let xs: [i32; 3] = [1, 2];` | E0330 length mismatch |
| `let xs: [i32; 2] = [1, true];` | E0329 mixed element types |
| `let xs = [1, 2, 3]; let y: i32 = xs.foo;` | E0323 field on non-struct |
| `let x: i32 = 5; let _y: i32 = x[0];` | E0331 indexing non-array |
| `let xs: [i32; 0] = [];` | E0332 empty literal not supported |
| `let xs: [i32; 3] = [1, 2, 3]; xs[0] = 5;` | E0305 not mut |

## 7. Implementation order

1. **Type refactor**: `Ty: Copy` → `Ty: Clone`. Add `Box<Ty>` for Array. Compile, fix the `.clone()` cascade.
2. **AST**: `TypeKind::Array { elem: Box<Type>, len: Box<Expr> }` (or store length as a literal directly); `ExprKind::ArrayLit { elements }`; `ExprKind::Index { receiver: Box<Expr>, index: Box<Expr> }`.
3. **Parser**:
   - `parse_type`: `[T; N]` form. N must be an int literal in this slice.
   - `parse_primary`: `[ ... ]` array literal — distinguish from indexing by context (only at primary position).
   - `parse_postfix`: add `[expr]` index suffix.
4. **Sema**:
   - `resolve_type` handles `[T; N]`.
   - `check_array_lit`: type-check all elements against first, return `Ty::Array(elem, len)`.
   - `check_index`: receiver must be Array; index coerces to `usize`.
   - Extend place-walk for `Index` targets.
5. **Codegen**:
   - `llvm_ty(Ty::Array(elem, n))` → `[N x <elem>]`.
   - `gen_array_lit`: alloca + per-element store + load.
   - `gen_index`: bounds-check trap + GEP + load.
   - `gen_place` for `Index`: same GEP, return pointer for stores.
6. **Tests**: ~25 new — sema for every error code, codegen for IR shape, e2e for two sample programs plus runtime bounds-check trap.

## 8. Out of scope (this slice)

- Slices `T[]` (fat pointer; depends on real reference types)
- Raw pointers `*T`, `*mut T`
- Indexing with negative integers
- Multi-dimensional sugar (`a[i][j]` works because each `[]` is one step)
- Repetition syntax `[0; N]`
- Empty array literal `[]`
- Equality / ordering on arrays

## 9. Open questions

- [ ] Should literal-length inference work? `let xs: [i32; _] = [1, 2, 3]` — would be `[i32; 3]`. Lean: no for this slice; require explicit length.
- [ ] Indexing with `i32` (auto-cast to `usize`)? Lean: no — explicit `i as usize` is consistent with our no-implicit-conversion rule.
- [ ] Should `[T; 0]` be allowed (zero-length arrays)? Lean: yes — useful as a marker. But this slice ships without; revisit when slices come.
