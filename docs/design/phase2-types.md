# Phase 2 — Integer/Float Types + Explicit Casts

> Status: draft — first slice of Phase 2
> Scope: extend the Phase-1 type system from `i32`/`bool` to the full primitive set, plus the `as` cast operator
> Out of scope (later Phase 2 slices): structs, arrays, raw pointers, slices, plain enums, methods

## 1. Problem

Phase 1 has two types: `i32` and `bool`. Useful programs need more — pointer arithmetic uses `usize`, file IO uses `i64` and `u64`, math uses `f32` / `f64`, byte buffers use `u8`. This slice adds the full primitive set plus `as`, so later Phase-2 work (structs, arrays, slices) has the types it needs to operate on.

## 2. Type set

| Family | Types | LLVM mapping |
|---|---|---|
| Signed integers | `i8 i16 i32 i64` | `i8 i16 i32 i64` |
| Unsigned integers | `u8 u16 u32 u64` | `i8 i16 i32 i64` (LLVM doesn't distinguish signedness; the *operations* do) |
| Pointer-sized integers | `isize` `usize` | `i64` on 64-bit targets (the only target Phase-2 supports) |
| Floats | `f32` `f64` | `float` `double` |
| Existing | `bool` | `i1` |

Sema's `Ty` enum expands to one variant per type. `Error` and `Unit` stay.

## 3. Literal typing

Lexer already handles all suffixes (Phase 1). Sema rules:

- **Suffixed literal** (`42i64`, `3.14f32`): type comes from the suffix; `expected` is ignored.
- **Unsuffixed integer literal** with `expected: Some(some integer T)`: the literal takes type `T`.
- **Unsuffixed integer literal** with `expected: None` or `expected = non-integer`: defaults to `i32`.
- **Unsuffixed float literal** with `expected: Some(F32 | F64)`: takes that type.
- **Unsuffixed float literal** otherwise: defaults to `f64` (matches Rust, IEEE common case).

We do **not** propagate types backwards through expressions in Phase 2. `0 + 1u64` is a type-mismatch error; user writes `0u64 + 1u64`. Bidirectional inference is a refinement we can add later if it hurts to live without; the strict version is much easier to type-check correctly.

**Range bounds checking** (a literal too large for its declared type, e.g. `let x: i8 = 1000`) is deferred. The lexer parses any integer up to `u64::MAX`. For Phase 2 the type check accepts the assignment; LLVM truncation handles it. Phase 3 (definite assignment / safety) will add a literal-range check with `E0314`-style code.

## 4. Operations

| Operator | Operand types | Result | Semantics |
|---|---|---|---|
| `+ - * /` | same numeric type | same type | int: trap on overflow in debug, wrap in release; float: IEEE 754, no trap |
| `%` | same integer type | same type | int: trap on zero divisor (always); float: rejected in Phase 2 |
| `== !=` | any same type (incl. `bool`) | `bool` | int: `icmp eq/ne`; float: `fcmp oeq/one` (ordered, NaN-aware) |
| `< <= > >=` | same numeric type | `bool` | signed int: `icmp slt/sle/sgt/sge`; unsigned: `icmp ult/ule/ugt/uge`; float: `fcmp olt/ole/ogt/oge` |
| `&& \|\|` | `bool` | `bool` | already done |
| Unary `-` | signed int or float | same | int: `sub i_ 0, x`; float: `fneg` |
| Unary `!` | `bool` | `bool` | already done |

Mixed-type operations are errors. **No implicit numeric conversions**, ever (§2.1, §2.8).

`/` on integers traps on zero divisor regardless of build mode (Phase 1 rule, unchanged). Float division by zero produces ±∞ or NaN per IEEE 754 — no trap.

Overflow trapping in debug applies to *signed* integer `+ - *` only. Unsigned overflow is well-defined wrapping in C, C++, Rust, and we follow suit — no `llvm.uadd.with.overflow.iN` traps in Phase 2. (Phase 3 may revisit; the Rust-style `+%` operators handle the explicit-wrap case.)

## 5. The `as` cast operator

Syntax already parsed (Phase 1). Sema currently rejects with E0312. Now allowed for numeric and `bool → integer` conversions.

| From | To | LLVM op |
|---|---|---|
| smaller int → larger signed | `i8 → i32`, etc. | `sext` |
| smaller int → larger unsigned (or both unsigned) | `u8 → u32`, etc. | `zext` |
| larger int → smaller int | `i32 → i8` | `trunc` |
| signed int → float | `i32 → f64` | `sitofp` |
| unsigned int → float | `u32 → f64` | `uitofp` |
| float → signed int | `f64 → i32` | `fptosi` |
| float → unsigned int | `f64 → u32` | `fptoui` |
| float → wider float | `f32 → f64` | `fpext` |
| float → narrower float | `f64 → f32` | `fptrunc` |
| `bool → integer` | `true → 1`, `false → 0` | `zext i1 to iN` |
| same type | no-op | (elided in IR) |

**Forbidden in Phase 2:**
- `integer → bool` (use `x != 0` explicitly — matches Rust, avoids C-style truthiness)
- `bool → float`
- pointer casts (no pointers yet)
- struct/array casts (no aggregates yet)

Casting between same-bitwidth signed and unsigned (`i32 → u32`) is allowed and is a no-op in IR (LLVM doesn't distinguish signedness at the type level).

## 6. The `println` builtin

Stays as `println(n: i32)` for Phase 2. Other types use `as i32` to print:

```cp
let x: u64 = 1234;
println(x as i32);   // ok; truncates if too large
```

Real formatted print is Phase 5 (`cpc test`/doctests need it; `println[T]` with a `Display` interface). Adding per-type intrinsics now would be wasted work that we throw away.

## 7. New error codes

| Code | Meaning |
|---|---|
| `E0314` | mixed-type arithmetic (e.g. `1 + 1u64`) — this is just E0302 with a hint, may stay as E0302 |
| `E0315` | invalid `as` cast (e.g. `true as f64`, or `42 as bool`) |
| `E0316` | float `%` operator (modulo on floats — pick `fmod` library function in stdlib later) |

Probably reuse E0302 for mixed-type arithmetic (it's already a type-mismatch error). `E0314` reserved for now if a more specific message helps.

## 8. Sample programs (Phase 2 slice 1)

### Must compile + run

**[mixed_ints.cplus](#)** — exercise i8/i16/i64/u32, all casts:
```cp
fn main() -> i32 {
    let a: i64 = 1_000_000_000;
    let b: i64 = a + a;
    let c: i32 = b as i32;       // truncates; we don't care about the value
    println(c);
    0
}
```

**[float_arith.cplus](#)** — exercise f64:
```cp
fn main() -> i32 {
    let x: f64 = 3.0;
    let y: f64 = 4.0;
    let z: f64 = x * x + y * y;   // 25.0
    let r: i32 = z as i32;
    println(r);
    0
}
```

**[unsigned.cplus](#)** — u8/u16/u32/u64 + comparisons:
```cp
fn main() -> i32 {
    let mut acc: u64 = 0;
    let n: u64 = 10;
    for i in 1..=10 {
        acc = acc + (i as u64);
    }
    println(acc as i32);   // 55
    let _check: bool = acc < n * n;
    0
}
```

### Must reject

| Program | Error |
|---|---|
| `let x: i32 = 1u64;` | E0302 type mismatch (suffix conflicts with declared type) |
| `1i32 + 1u32` | E0302 mixed-type arithmetic |
| `1.0 as bool` | E0315 invalid `as` target |
| `42 as bool` | E0315 invalid `as` target (must use `!= 0`) |
| `1.5 % 2.0` | E0316 float modulo |
| `let x: NotAType = 1;` | E0303 unknown type (already handled) |

## 9. Implementation order

1. Expand `Ty` enum in sema.rs (one variant per primitive type)
2. Update `llvm_ty()` and add helpers for signedness/family classification
3. Update `resolve_type()` to accept all the new names
4. Update `check_int_lit()` to take expected type
5. Update `check_binary()` for per-family op rules
6. Update `check_unary()::Neg` for floats (use `fneg`-equivalent)
7. Add `check_cast()` for the `as` expr rules
8. Update codegen `gen_binary()` to dispatch by operand type (icmp/fcmp, signed/unsigned, add/fadd, etc.)
9. Update `arith_with_overflow_check()` to dispatch by integer width (`llvm.sadd.with.overflow.i8/i16/i32/i64`)
10. Add codegen `gen_cast()` for the per-conversion-pair LLVM op
11. Update tests: extend sema unit tests, add codegen unit tests for each cast variant, add e2e tests for the three sample programs

## 10. Out of scope for this slice (next slices)

- Plain enums (`enum Color { Red, Green, Blue }`)
- Structs (decl, literal, field access — needs grammar changes for `struct`/`.`)
- Methods on structs (open question §11: methods vs UFCS)
- Fixed-size arrays `[T; N]`
- Raw pointers `*T`
- Slices `T[]`

## 11. Open questions for this slice

- [ ] Should `bool as integer` be allowed? Lean: yes (Rust does it). Cost: trivial codegen via `zext`. Some users find it useful.
- [ ] `isize`/`usize` width on non-64-bit targets — Phase 2 only supports 64-bit, so hardcoded `i64`. Phase 8 (C interop) will revisit per-target.
- [ ] Whether to validate integer literals fit in their declared type at Phase 2. Lean: defer to Phase 3 with definite-assignment work. LLVM `trunc`-on-cast handles the value silently for now.
