<!-- GENERATED from docs/errors.toml by docs/gen_errors.py — do not edit by hand. This is the maintainer reference; the public copy is the cplus-lang.dev /docs/error-codes page. -->

# Error codes

Every C+ diagnostic carries a numbered code, a source span, and often a machine-applicable suggestion. `cpc --diagnostics=json` emits the same information in a machine-readable shape for editors and agents. Codes prefixed with **W** are non-fatal warnings; the build continues. The normative ranges and what each phase owns are fixed in [§20 of the language specification](/docs/spec).

This is the complete index — **143 codes**. Each entry gives the meaning, a minimal example that triggers it, and the typical fix. **111** of the examples are reproduced directly by `cpc check`; the rest need a multi-file project, a `--target`, or a build-time file, and say so in the example.

## Lexical

### E0001 · Unexpected character

The lexer hit a byte it cannot start a token with (also fired for a bad char literal such as an empty `''`, a multi-byte `'ab'`, or a non-ASCII `'á'`).

```cplus
fn main() -> i32 { let x = 'ab'; return 0; }
```

**Fix.** Remove or correct the stray character; for UTF-8 text use a `str` instead of a char literal.

<sub>repro: checked · cplus-core/src/lexer.rs:457 · test cplus-core/src/lexer.rs:char_literal_multi_byte_rejected</sub>

### E0002 · Unterminated block comment

A `/* ... */` block comment was opened but never closed before end of input.

```cplus
/* hello
```

**Fix.** Close the comment with `*/`.

<sub>repro: checked · cplus-core/src/lexer.rs:316 · test cplus-core/src/lexer.rs:unterminated_block_comment_errors</sub>

### E0003 · Invalid number literal

A numeric literal has no valid digits or a malformed exponent (e.g. `0x` with no hex digits, or `1e` with no exponent).

```cplus
fn main() -> i32 { let x = 0x; return 0; }
```

**Fix.** Write a well-formed literal with at least one digit.

<sub>repro: checked · cplus-core/src/lexer.rs:879 · test cplus-core/src/lexer.rs:integers_with_bases_and_separators</sub>

### E0004 · Invalid numeric type suffix

A number literal carries a type suffix that is not one of i8/i16/i32/i64/u8/u16/u32/u64/isize/usize/f32/f64.

```cplus
fn main() -> i32 { let x = 42xyz; return 0; }
```

**Fix.** Use a valid suffix or drop it.

<sub>repro: checked · cplus-core/src/lexer.rs:938 · test cplus-core/src/lexer.rs:invalid_suffix_errors</sub>

### E0005 · Unterminated string literal

A string literal was opened with `"` but reached end of line or end of input before a closing quote.

```cplus
fn main() -> i32 { let s = "oops; return 0; }
```

**Fix.** Add the closing `"` (or use a `"""..."""` triple-quoted string for multi-line text).

<sub>repro: checked · cplus-core/src/lexer.rs:484 · test cplus-core/src/lexer.rs:string_unterminated_eof_errors</sub>

## Parser

### E0100 · Unexpected token

The parser found a token where a different one was expected (the most common case is a missing `;`).

```cplus
fn main() -> i32 { let x = 1 0 }
```

**Fix.** Insert the expected token; the compiler often suggests `;`.

<sub>repro: checked · cplus-core/src/parser.rs:201 · test cplus-core/src/parser.rs:missing_semicolon_errors</sub>

### E0101 · Unexpected end of input

Input ended while the parser was still expecting more tokens (e.g. an unmatched `{`).

```cplus
fn main() -> i32 { 
```

**Fix.** Close the open construct (e.g. add the missing `}`).

<sub>repro: checked · cplus-core/src/parser.rs:196 · test cplus-core/src/parser.rs:unmatched_brace_errors</sub>

### E0102 · Non-chainable comparison

Comparison operators were chained (e.g. `a < b < c`), which is not allowed.

```cplus
fn main() -> i32 { let r = 1 < 2 < 3; 0 }
```

**Fix.** Split into separate comparisons joined with `&&`, e.g. `a < b && b < c`.

<sub>repro: checked · cplus-core/src/parser.rs:2245 · test cplus-core/src/parser.rs:non_chainable_comparison_rejected</sub>

## Names, types, and items

### E0300 · Undefined name

A referenced name (variable, function, or `this` outside a method) is not in scope.

```cplus
fn main() -> i32 { return x; }
```

**Fix.** Fix the typo, add the missing import, or check the name isn't `_`-private (module-private) in its declaring file.

<sub>repro: checked · cplus-core/src/sema.rs:13154 · test cplus-core/src/sema.rs:undefined_name_e0300</sub>

### E0301 · Duplicate definition

Two items (functions, or types/interfaces) share the same name.

```cplus
fn f() -> i32 { 0 }
fn f() -> i32 { 1 }
fn main() -> i32 { return f(); }
```

**Fix.** Rename one of the conflicting items.

<sub>repro: checked · cplus-core/src/sema.rs:3352 · test cplus-core/src/sema.rs:duplicate_fn_e0301</sub>

### E0302 · Type mismatch

An expression's type does not match the type required by its context (declared type, argument, condition, etc.).

```cplus
fn main() -> i32 { let x: i32 = true; return 0; }
```

**Fix.** Insert an `as` cast or change the declared type.

<sub>repro: checked · cplus-core/src/sema.rs:6217 · test cplus-core/src/sema.rs:type_mismatch_e0302</sub>

### E0303 · Unknown type

A named type cannot be resolved to any declared type, enum, or in-scope generic parameter.

```cplus
fn main() -> Foo { return 0; }
```

**Fix.** Typo, missing import, or a generic param not in scope. The owned `string` type was removed: use `Text` and `import "stdlib/text"`.

<sub>repro: checked · cplus-core/src/sema.rs:12376 · test cplus-core/src/sema.rs:unknown_type_e0303</sub>

### E0304 · Condition must be `bool`

The condition of an `if` or `while` is not of type `bool`.

```cplus
fn main() -> i32 { return if 1 { 1 } else { 2 }; }
```

**Fix.** Use a boolean expression, e.g. compare with `!= 0`.

<sub>repro: checked · cplus-core/src/sema.rs:5442 · test cplus-core/src/sema.rs:nonbool_condition_e0304</sub>

### E0305 · Assignment to immutable binding

An assignment targets a binding (or a place rooted at one) that was not declared `var`.

```cplus
fn main() -> i32 { let x = 1; x = 2; return 0; }
```

**Fix.** Declare the binding as `var`.

<sub>repro: checked · cplus-core/src/sema.rs:12042 · test cplus-core/src/sema.rs:assign_to_immutable_e0305</sub>

### E0306 · Block produces no value but one is required

A function whose return type is non-`Unit` reaches the end of its body without an explicit `return ...;` or a diverging tail.

```cplus
fn f() -> i32 { 1; }
fn main() -> i32 { return f(); }
```

**Fix.** End the body with an explicit `return EXPR;`.

<sub>repro: checked · cplus-core/src/sema.rs:5020 · test cplus-core/src/sema.rs:trailing_semi_discards_value_e0306</sub>

### E0307 · `return` without a value

A bare `return;` appears in a function that declares a non-`Unit` return type.

```cplus
fn f() -> i32 { return; }
fn main() -> i32 { return f(); }
```

**Fix.** Return a value: `return EXPR;`.

<sub>repro: checked · cplus-core/src/sema.rs:5158 · test cplus-core/src/sema.rs:return_without_value_e0307</sub>

### E0308 · Wrong number of arguments

A call passes a different number of arguments than the function (or intrinsic) declares.

```cplus
fn main() -> i32 { #println(1, 2); return 0; }
```

**Fix.** Match the function's parameter count.

<sub>repro: checked · cplus-core/src/sema.rs:8279 · test cplus-core/src/sema.rs:arg_count_mismatch_e0308</sub>

### E0309 · Wrong `main` signature

`main` is declared with parameters or a return type other than `fn main() -> i32`.

```cplus
fn main() { }
```

**Fix.** Declare it as `fn main() -> i32`.

<sub>repro: checked · cplus-core/src/sema.rs:3597 · test cplus-core/src/sema.rs:main_must_return_i32_e0309</sub>

### E0312 · Function used as value

A function name is used as a bare value (or another unsupported form such as `&x`, a range outside `for`, or a malformed path) where a callable or value of the right shape was required.

```cplus
fn main() -> i32 { let x = 1; let y = &x; return 0; }
```

**Fix.** Assign it to a `fn(...)`-typed binding to take the address.

<sub>repro: checked · cplus-core/src/sema.rs:13138 · test cplus-core/src/sema.rs:ref_not_supported_e0312</sub>

### E0313 · Assignment target is not a place

The left-hand side of an assignment is not a place expression (e.g. a literal or temporary).

```cplus
fn main() -> i32 { 1 = 2; return 0; }
```

**Fix.** Assign to a variable, field, or index that names a storage location.

<sub>repro: checked · cplus-core/src/sema.rs:12089 · test cplus-core/src/sema.rs:assign_to_non_ident_e0313</sub>

### E0314 · Integer literal out of range

An integer literal does not fit the type it resolves to (the annotated type, the suffix type, or the i32 default). The lexer accepts any magnitude up to u64::MAX, so the value is range-checked against the target type. A leading `-` is a separate unary op, so a negated literal is checked against the type minimum's magnitude (`-128` fits i8, `9223372036854775808` does not fit i64 but `-9223372036854775808` does).

```cplus
fn main() -> i32 { let x: i8 = 300; return x as i32; }
```

**Fix.** Use a value within the type's range, or widen the type (e.g. `i32`/`i64`, or an unsigned type for large non-negative values).

<sub>repro: checked · cplus-core/src/sema.rs:8193 · test cplus-core/src/sema.rs:int_lit_overflow_i8_e0314</sub>

### E0315 · Invalid cast

An `as` cast is between a pair of types that the language forbids.

```cplus
fn main() -> i32 { let _b: bool = 1 as bool; return 0; }
```

**Fix.** Some pairs are forbidden (for example `int` to `bool`, `*T` to `i32`); restructure the conversion.

<sub>repro: checked · cplus-core/src/sema.rs:7654 · test cplus-core/src/sema.rs:cast_int_to_bool_rejected_e0315</sub>

### E0316 · Modulo on float types

The `%` operator was applied to a floating-point operand, which is not supported.

```cplus
fn main() -> i32 { let x: f64 = 1.0 % 2.0; let _y: f64 = x; return 0; }
```

**Fix.** Use integer operands, or compute the remainder another way.

<sub>repro: checked · cplus-core/src/sema.rs:11568 · test cplus-core/src/sema.rs:float_modulo_rejected_e0316</sub>

### E0317 · Unknown enum variant

A path or expression names a variant that the enum does not declare.

```cplus
enum Color { Red }
fn main() -> i32 { let _c: Color = Color::Purple; return 0; }
```

**Fix.** Use a variant the enum actually declares.

<sub>repro: checked · cplus-core/src/sema.rs:7410 · test cplus-core/src/sema.rs:unknown_enum_variant_e0317</sub>

### E0318 · Duplicate enum variant

Two variants in the same enum share a name.

```cplus
enum E { A, A }
fn main() -> i32 { return 0; }
```

**Fix.** Rename one of the variants.

<sub>repro: checked · cplus-core/src/sema.rs:1315 · test cplus-core/src/sema.rs:duplicate_enum_variant_e0318</sub>

### E0319 · Duplicate field in struct literal

A struct literal lists the same field name twice.

```cplus
struct E { x: i32, x: i32 }
fn main() -> i32 { return 0; }
```

**Fix.** List each field once; match the declaration.

<sub>repro: checked · cplus-core/src/sema.rs:7494 · test cplus-core/src/sema.rs:duplicate_field_e0319</sub>

### E0320 · Unknown struct field

A field access (`s.f`) names a field the struct does not declare.

```cplus
struct A { x: i32 }
fn main() -> i32 { let a: A = A { x: 1 }; let _v: i32 = a.y; return 0; }
```

**Fix.** Access a field the struct actually declares.

<sub>repro: checked · cplus-core/src/sema.rs:7589 · test cplus-core/src/sema.rs:unknown_field_in_access_e0320</sub>

### E0321 · Missing field in struct literal

A struct literal omits a field the struct declares.

```cplus
struct A { x: i32, y: i32 }
fn main() -> i32 { let _a: A = A { x: 1 }; return 0; }
```

**Fix.** Provide every declared field; match the declaration.

<sub>repro: checked · cplus-core/src/sema.rs:7551 · test cplus-core/src/sema.rs:missing_field_in_literal_e0321</sub>

### E0322 · Extra field in struct literal

A struct literal includes a field the struct does not declare.

```cplus
struct A { x: i32 }
fn main() -> i32 { let _a: A = A { x: 1, y: 2 }; return 0; }
```

**Fix.** Remove the extra field; match the declaration.

<sub>repro: checked · cplus-core/src/sema.rs:7536 · test cplus-core/src/sema.rs:extra_field_in_literal_e0322</sub>

### E0323 · Field access on non-struct type

A `.field` access is performed on a value whose type is not a struct.

```cplus
fn main() -> i32 { let x: i32 = 5; let _v: i32 = x.foo; return 0; }
```

**Fix.** Only access fields on struct values.

<sub>repro: checked · cplus-core/src/sema.rs:7565 · test cplus-core/src/sema.rs:field_access_on_non_struct_e0323</sub>

### E0324 · Unknown method

A method call names a method (or free fn in the type's module) that the struct does not have.

```cplus
struct P {}
impl P {}
fn main() -> i32 { let p: P = P {}; return p.missing(); }
```

**Fix.** Call a method the type actually declares, or define it in an `impl`.

<sub>repro: checked · cplus-core/src/sema.rs:7379 · test cplus-core/src/sema.rs:no_such_method_e0324</sub>

### E0325 · `impl` on an unknown or non-struct type

An `impl` names a target that is not a declared struct or (non-generic) enum in scope.

```cplus
impl Foo { fn f(this) {} }
fn main() -> i32 { return 0; }
```

**Fix.** The target must be a declared struct or enum in scope.

<sub>repro: checked · cplus-core/src/sema.rs:2440 · test cplus-core/src/sema.rs:impl_on_unknown_type_e0325</sub>

### E0326 · Duplicate method in `impl`

Two methods in the same `impl` block share a name.

```cplus
struct P {}
impl P { fn f(this) {} fn f(this) {} }
fn main() -> i32 { return 0; }
```

**Fix.** Rename one of the methods.

<sub>repro: checked · cplus-core/src/sema.rs:2374 · test cplus-core/src/sema.rs:duplicate_method_e0326</sub>

### E0327 · Wrong call form

An associated function was called as an instance method (or an instance method via the type, or an enum variant was called like a function).

```cplus
struct P { x: i32 }
impl P { fn make() -> P { return P { x: 0 }; } }
fn main() -> i32 { let p: P = P { x: 0 }; let _q: P = p.make(); return 0; }
```

**Fix.** `Type::method()` for associated, `value.method()` for instance.

<sub>repro: checked · cplus-core/src/sema.rs:9227 · test cplus-core/src/sema.rs:calling_assoc_fn_as_method_e0327</sub>

### E0328 · Mutable receiver required

A method declared with `ref this` is called on an immutable receiver.

```cplus
struct P { x: i32 }
impl P { fn bump(ref this) { this.x = this.x + 1; } }
fn main() -> i32 { let p: P = P { x: 0 }; p.bump(); return 0; }
```

**Fix.** Bind the receiver as `var`.

<sub>repro: checked · cplus-core/src/sema.rs:9241 · test cplus-core/src/sema.rs:calling_mut_method_on_immutable_e0328</sub>

### E0329 · Mixed element types in array literal

Elements of an array literal do not all share one type.

```cplus
fn main() -> i32 { let _xs: [i32; 2] = [1, true]; return 0; }
```

**Fix.** Make every element the same type.

<sub>repro: checked · cplus-core/src/sema.rs:6826 · test cplus-core/src/sema.rs:array_literal_mixed_types_e0329</sub>

### E0330 · Array literal length mismatch

An array literal has a different element count than its declared `[T; N]` length.

```cplus
fn main() -> i32 { let _xs: [i32; 3] = [1, 2]; return 0; }
```

**Fix.** Match the literal's element count to the declared length.

<sub>repro: checked · cplus-core/src/sema.rs:6841 · test cplus-core/src/sema.rs:array_literal_length_mismatch_e0330</sub>

### E0331 · Indexing a non-array type

The `[]` index operator is applied to a value that is not an array.

```cplus
fn main() -> i32 { let x: i32 = 5; return x[0 as usize]; }
```

**Fix.** Only index array (or array-like) values.

<sub>repro: checked · cplus-core/src/sema.rs:6991 · test cplus-core/src/sema.rs:indexing_non_array_e0331</sub>

### E0332 · Empty array literal

An empty array literal `[]` was written, which is not supported.

```cplus
fn main() -> i32 { let _xs: [i32; 0] = []; return 0; }
```

**Fix.** Provide at least one element.

<sub>repro: checked · cplus-core/src/sema.rs:6809 · test cplus-core/src/sema.rs:empty_array_literal_e0332</sub>

### E0339 · Fill-array element type is not `Copy`

A fill-array literal `[expr; N]` has a non-`Copy` (owning / `drop`-carrying) element type. The fill expression is evaluated once and copied into every slot, which would make N elements share one owned resource and double-free when they are dropped.

```cplus
struct Owner { id: i32 }
impl Owner { fn drop(ref this) {} }
fn mk() -> Owner { return Owner { id: 1 }; }
fn main() -> i32 { let _a: [Owner; 2] = [mk(); 2]; return 0; }
```

**Fix.** Use a `Copy` element type, or construct each element explicitly with `[expr0, expr1, ...]`.

<sub>repro: checked · cplus-core/src/sema.rs:6892 · test cplus-core/src/sema.rs:array_fill_noncopy_element_rejected_e0339</sub>

### E0361 · Enum has no variants

An enum is declared with zero variants. Such a type is uninhabited (no value can ever be constructed), but match exhaustiveness treats it as vacuously covered and the tag ABI lowers it as a plain i32. C+ has no uninhabited / never type.

```cplus
enum Void {}
fn main() -> i32 { return 0; }
```

**Fix.** Declare at least one variant, or remove the enum.

<sub>repro: checked · cplus-core/src/sema.rs:1456 · test cplus-core/src/sema.rs:empty_enum_rejected_e0361</sub>

### E0364 · Cannot infer struct type of `{ ... }`

A type-inferred struct literal `{ field: ... }` appears where the expected type is absent or is not a known struct, so the compiler has no struct to construct.

```cplus
struct A { x: i32 }
fn main() -> i32 { let a = { x: 1 }; return 0; }
```

**Fix.** Name the struct (`A { field: ... }`), or give the binding a struct type annotation so the literal's type can be inferred.

<sub>repro: checked · cplus-core/src/sema.rs:check_inferred_struct_lit · test cplus-core/src/sema.rs:inferred_struct_lit_uninferable_e0364</sub>

## Control flow and matching

### E0333 · Implicit return (function body ends with a tail expression)

A function body ends with an implicit tail expression instead of an explicit `return`; C+ function bodies never use a trailing value expression.

```cplus
fn f() -> i32 { 42 }
fn main() -> i32 { return f(); }
```

**Fix.** Add an explicit `return EXPR;` (or `;` after the closing `}` when the tail is unit-typed).

<sub>repro: checked · cplus-core/src/sema.rs:5012 · test cplus-core/src/sema.rs:e0333_value_tail_still_suggests_return_g022</sub>

### E0334 · Mutually-exclusive parameter ownership markers

A parameter carries two ownership markers that cannot combine, such as `ref` + `take`.

```cplus
fn f(ref take x: i32) -> i32 { return x; }
fn main() -> i32 { return f(1); }
```

**Fix.** Keep at most one marker: `ref` (exclusive borrow), `take` (consume), or bare (a read-only borrow).

<sub>repro: checked · cplus-core/src/sema.rs:3191 · test cplus-core/src/sema.rs:mut_and_move_on_param_e0334</sub>

### E0335 · Use of a moved value

A non-Copy binding is read after it was moved (into a call, a `take` parameter, or a `let y = x;`). Flow-sensitive: a move only on a branch that `return`s / `break`s does not poison the other path, and it also fires for non-Copy types whose Copy-ness depends on a generic payload.

```cplus
struct P { x: i32 }
impl P { fn drop(ref this) {} }
fn echo(take p: P) -> i32 { return p.x; }
fn main() -> i32 {
    let p: P = P { x: 1 };
    let r: i32 = echo(p);
    return p.x;
}
```

**Fix.** Do not read after a `take`; clone the value first, or restructure so the move and the use are on disjoint paths.

<sub>repro: checked · cplus-core/src/sema.rs:13097 · test cplus-core/src/sema.rs:phase5_implicit_non_copy_param_consumes_e0335</sub>

### E0338 · Destructor `drop` has the wrong signature

A `drop` method has a signature other than `fn drop(ref this)` (extra parameters, a return type, or a non-`ref this` receiver), or a `drop` was written on an enum.

```cplus
struct B { x: i32 }
impl B { fn drop(this) {} }
fn main() -> i32 { return 0; }
```

**Fix.** Declare it exactly `fn drop(ref this)` — no extra parameters, no return type; enums get a compiler-synthesized destructor instead.

<sub>repro: checked · cplus-core/src/sema.rs:2214 · test cplus-core/src/sema.rs:drop_wrong_receiver_e0338</sub>

### E0340 · Non-exhaustive `match`

A `match` on an enum does not cover every variant and has no catch-all arm.

```cplus
enum M { A, B, C }
fn main() -> i32 { let m: M = M::A; return match m { M::A => 0 }; }
```

**Fix.** Add the missing arm or a `_ =>` catch-all.

<sub>repro: checked · cplus-core/src/sema.rs:7131 · test cplus-core/src/sema.rs:match_non_exhaustive_e0340</sub>

### E0341 · Pattern type does not match the scrutinee

A `match` scrutinee is not an enum, a pattern names a different enum than the scrutinee, or a nested variant pattern appears in a payload position.

```cplus
fn main() -> i32 { let x: i32 = 5; return match x { _ => 0 }; }
```

**Fix.** Match on an enum value, and make each pattern name the scrutinee's enum (payload patterns must be `_` or a binding).

<sub>repro: checked · cplus-core/src/sema.rs:7023 · test cplus-core/src/sema.rs:match_on_non_enum_e0341</sub>

### E0342 · Wrong number of payload values for a variant

A variant pattern or construction supplies a different number of payload values than the variant declares.

```cplus
enum M { A(i32, i32) }
fn main() -> i32 { let m: M = M::A(1, 2); return match m { M::A(v) => v }; }
```

**Fix.** Match the variant's declared payload arity in both the pattern and the constructor.

<sub>repro: checked · cplus-core/src/sema.rs:7266 · test cplus-core/src/sema.rs:match_wrong_payload_arity_e0342</sub>

### E0345 · Use of a possibly-unassigned binding

A binding is read on a control-flow path where it is not definitely assigned.

```cplus
fn main() -> i32 { let x: i32; return x; }
```

**Fix.** Initialize the binding on every control-flow path before reading it.

<sub>repro: checked · cplus-core/src/sema.rs:13100 · test cplus-core/src/sema.rs:uninit_let_read_before_assign_e0345</sub>

### E0346 · Uninitialized `let` requires a type annotation

A `let` with no initializer has no type annotation, so there is nothing to infer the type from.

```cplus
fn main() -> i32 { let x; x = 5; return x; }
```

**Fix.** Add a type annotation (`let x: T;`) or give the `let` an initializer.

<sub>repro: checked · cplus-core/src/sema.rs:5088 · test cplus-core/src/sema.rs:uninit_let_no_type_e0346</sub>

### E0347 · Irrefutable `if let` / `while let` pattern

An `if let` or `while let` uses a pattern that always matches (a bare binding or `_`), so the conditional form is pointless.

```cplus
fn main() -> i32 {
    if let x = 7 { return x; }
    return 0;
}
```

**Fix.** Use a plain `let` (or `loop`) instead, or write a refutable variant pattern.

<sub>repro: checked · cplus-core/src/lower.rs:435 · test cplus-core/src/lower.rs:if_let_irrefutable_binding_rejected</sub>

### E0348 · `guard let` else block must diverge

The else block of a `guard let` falls through instead of diverging on every path.

```cplus
enum Maybe { Some(i32), None }
fn main() -> i32 {
    let m: Maybe = Maybe::Some(7);
    guard let Maybe::Some(v) = m else { let x: i32 = 1; };
    return v;
}
```

**Fix.** Make the else block diverge on every path (`return` / `break` / `continue`).

<sub>repro: checked · cplus-core/src/lower.rs:497 · test cplus-core/src/lower.rs:guard_let_non_diverging_else_rejected</sub>

### E0350 · `guard let` complement overlaps the success pattern

The explicit complement pattern in `else |Pat|` references the same enum variant as the success pattern, so the two overlap.

```cplus
enum Maybe { Some(i32), None }
fn main() -> i32 {
    let m: Maybe = Maybe::Some(7);
    guard let Maybe::Some(v) = m else |Maybe::Some(_)| { return 0; };
    return v;
}
```

**Fix.** Make the complement pattern cover only the cases the success pattern does not.

<sub>repro: checked · cplus-core/src/lower.rs:684 · test cplus-core/src/lower.rs:guard_let_complement_overlap_rejected</sub>

### E0351 · `guard let` must bind at least one value

A `guard let` pattern binds no names, so there is nothing for it to extract.

```cplus
enum Maybe { Some(i32), None }
fn main() -> i32 {
    let m: Maybe = Maybe::Some(7);
    guard let Maybe::None = m else { return 0; };
    return 0;
}
```

**Fix.** Use `if let` for inspection-only, or write a pattern that binds a value.

<sub>repro: checked · cplus-core/src/lower.rs:508 · test cplus-core/src/lower.rs:guard_let_no_binding_rejected</sub>

### E0352 · Multi-binding `guard let` is not supported

A `guard let` pattern binds more than one value; only single-binding patterns are supported.

```cplus
enum Pair { Both(i32, i32) }
fn main() -> i32 {
    let p: Pair = Pair::Both(1, 2);
    guard let Pair::Both(a, b) = p else { return 0; };
    return a;
}
```

**Fix.** Use one `guard let` per binding.

<sub>repro: checked · cplus-core/src/lower.rs:516 · test cplus-core/src/lower.rs:guard_let_multi_binding_rejected</sub>

### E0353 · `break` / `continue` outside a loop

A `break` or `continue` appears outside any loop body.

```cplus
fn main() -> i32 { break; return 0; }
```

**Fix.** Move it into a loop body.

<sub>repro: checked · cplus-core/src/sema.rs:5235 · test cpc/tests/e2e.rs:break_outside_loop_rejected</sub>

### E0363 · Name already declared in this scope (no same-scope shadowing)

Two bindings with the same name are declared in one block. C+ forbids redeclaring a name in a scope; same-scope shadowing would silently swap a binding's type, so it is rejected.

```cplus
fn main() -> i32 { let x: i32 = 1; let x: bool = true; return 0; }
```

**Fix.** Pick a new name, or assign to the existing binding. Shadowing in a nested block (or shadowing a parameter) is still allowed — only same-block re-declaration is rejected.

<sub>repro: checked · cplus-core/src/sema.rs:5294 · test cplus-core/src/sema.rs:same_scope_shadow_e0363</sub>

## Ownership and borrowing

### E0337 · A bare borrow escapes its call

A bare (read-only borrow) parameter, a raw-pointer dereference, or a value matched out of a borrow is made to outlive the call — returned, stored in a field, or re-passed to a `take` parameter. The borrow has no owner to keep its storage alive past the call.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) {} }
fn keep(b: B) -> B { return b; }
fn main() -> i32 { return 0; }
```

**Fix.** Take the value by value (`take`) so the callee owns it, or `.clone()` it; return an owned value rather than a borrow.

<sub>repro: checked · cplus-core/src/sema.rs:11226 · test cplus-core/src/sema.rs:return_borrow_marker_param_rejected_e0337</sub>

### E0370 · Move and shared-borrow of the same binding in one call

A non-Copy binding is moved at one argument position while a sibling argument in the same call reads (shared-borrows) the same place.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn drain(take b: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  var y: B = B { x: 1 };
  drain(y, peek(y));
  return;
}
```

*In this minimal single-call form `cpc` reports the broader use-after-move error E0335; E0370 is the borrow checker's name for the move / shared-borrow conflict.*

**Fix.** Split into two statements so the value is read before it is moved: `let tmp = peek(y); drain(take y, tmp);`

<sub>repro: scenario · cplus-core/src/borrowck.rs:2977 · test cplus-core/src/borrowck.rs:e0370_fires_on_move_and_read_of_same_non_copy_binding</sub>

### E0371 · Use of a possibly-moved binding

A non-Copy binding is moved on some control-flow branches but not others, then read at a point where it may already be moved (its merged state is MaybePartial).

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn sink(take b: B) { return; }
fn use_it(b: B) -> i32 { return b.x; }
fn caller(c: bool) {
  var y: B = B { x: 1 };
  if c { sink(y); }
  let z: i32 = use_it(y);
  return;
}
```

*Reported as E0335 in simple cases; E0371 specifically covers a use of a binding moved on only some control-flow paths.*

**Fix.** Ensure every branch either moves or preserves the binding, or clone it before the branch: `let y_owned = y.clone();`

<sub>repro: source · cplus-core/src/borrowck.rs:2638</sub>

### E0372 · Move of a binding while it is borrowed

A binding is moved while a live borrower still holds a borrow of it (or one of its sub-places) at an overlapping place.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest(a: B, b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}
fn drain(take b: B) { return; }
fn caller() {
  let a: B = B { x: 1 };
  let b: B = B { x: 2 };
  let r: B = longest(a, b);
  drain(a);
  return;
}
```

*In this minimal form `cpc` reports E0335; E0372 is the borrow checker's classification of moving a value while it is borrowed.*

**Fix.** Drop the borrower before moving the value, or clone it if both bindings must outlive the move.

<sub>repro: scenario · cplus-core/src/borrowck.rs:3181 · test cplus-core/src/borrowck.rs:e3_fires_e0372_on_move_of_other_source</sub>

### E0374 · Partial-place borrow conflict

A borrow of a place overlaps a sibling access to one of its sub-places (or vice versa) — a borrow of a place includes all of its sub-places.

```cplus
struct Inner { v: i32 }
impl Inner { fn drop(ref this) { return; } }
struct Pair { left: Inner, right: Inner }
impl Pair { fn drop(ref this) { return; } }
fn write_pair(ref a: Pair, b: Inner) { return; }
fn caller() {
  let p: Pair = Pair { left: Inner { v: 1 }, right: Inner { v: 2 } };
  write_pair(p, p.left);
  return;
}
```

*A whole-place / sub-field overlap in one call is reported as E0337; E0374 is the borrow checker's partial-place conflict.*

**Fix.** Split into two calls if the operations are independent, or restructure to operate on a single uniform place.

<sub>repro: scenario · cplus-core/src/borrowck.rs:1418 · test cplus-core/src/borrowck.rs:e0374_partial_overlap_parent_with_subfield_in_one_call</sub>

### E0380 · Two exclusive borrows of the same place in one call

The same non-Copy binding is exclusively borrowed (`ref`) at two argument positions in a single call, but at most one exclusive borrow of a place can be live at a time.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn modify_both(ref a: B, ref b: B) { return; }
fn caller() {
  var y: B = B { x: 1 };
  modify_both(y, y);
  return;
}
```

**Fix.** Split into two calls, or borrow distinct sub-places (e.g. `f(ref y.left, ref y.right)`).

<sub>repro: checked · cplus-core/src/borrowck.rs:1446 · test cplus-core/src/borrowck.rs:e0380_fires_on_two_mut_borrows_of_same_non_copy_binding</sub>

### E0381 · Exclusive borrow with a concurrent shared read

A place is exclusively borrowed (`ref`) while a sibling argument shared-reads it in the same call, or a method is called on a receiver that is currently shared-borrowed.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn write_thing(ref a: B, n: i32) { return; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  var y: B = B { x: 1 };
  write_thing(y, peek(y));
  return;
}
```

**Fix.** Split into two statements: `let tmp = peek(y); write_thing(ref y, tmp);`

<sub>repro: checked · cplus-core/src/borrowck.rs:2991 · test cplus-core/src/borrowck.rs:e0381_fires_on_mut_arg_with_sibling_read</sub>

### E0382 · Move and exclusive borrow of the same binding in one call

The same non-Copy binding is exclusively borrowed (`ref`) at one argument position and moved at another in a single call; the exclusive borrow claims access for the whole call, which conflicts with the move's consumption.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn write_and_take(ref a: B, take b: B) { return; }
fn caller() {
  var y: B = B { x: 1 };
  write_and_take(y, y);
  return;
}
```

**Fix.** Split into two statements so the exclusive borrow and the move do not overlap.

<sub>repro: checked · cplus-core/src/borrowck.rs:1482 · test cplus-core/src/borrowck.rs:e0382_fires_on_mut_arg_with_sibling_move</sub>

### E0503 · Interface impl missing a required method

An `impl Type: Interface` block omits a method that the interface declares.

```cplus
interface Two { fn first(this) -> i32; fn second(this) -> i32; }
struct P { x: i32 }
impl P: Two { fn first(this) -> i32 { return 0; } }
fn main() -> i32 { return 0; }
```

**Fix.** Implement every method the interface declares.

<sub>repro: checked · cplus-core/src/sema.rs:2895 · test cplus-core/src/sema.rs:impl_interface_missing_method_e0503</sub>

### E0504 · Interface impl declares a method the interface does not

An `impl Type: Interface` block contains a method that the interface does not declare.

```cplus
interface One { fn a(this) -> i32; }
struct P { x: i32 }
impl P: One { fn a(this) -> i32 { return 0; } fn extra(this) -> i32 { return 1; } }
fn main() -> i32 { return 0; }
```

**Fix.** Move the extra method to an inherent `impl Type { ... }` block.

<sub>repro: checked · cplus-core/src/sema.rs:2931 · test cplus-core/src/sema.rs:impl_interface_extra_method_e0504</sub>

### E0505 · Interface method signature mismatch

An impl method's signature does not match the interface's declared signature after substituting `This` with the target type.

```cplus
interface One { fn a(this) -> i32; }
struct P { x: i32 }
impl P: One { fn a(this) -> bool { return true; } }
fn main() -> i32 { return 0; }
```

**Fix.** Make the impl method's signature match the interface declaration exactly.

<sub>repro: checked · cplus-core/src/sema.rs:2914 · test cplus-core/src/sema.rs:impl_interface_signature_mismatch_e0505</sub>

### E0506 · Duplicate interface impl for the same type

Two `impl Type: Interface` blocks exist for the same (interface, type) pair; a type may have at most one impl of any given interface.

```cplus
interface One { fn a(this) -> i32; }
struct P { x: i32 }
impl P: One { fn a(this) -> i32 { return 0; } }
impl P: One { fn a(this) -> i32 { return 1; } }
fn main() -> i32 { return 0; }
```

**Fix.** Remove the duplicate impl block.

<sub>repro: checked · cplus-core/src/sema.rs:2871 · test cplus-core/src/sema.rs:impl_interface_duplicate_e0506</sub>

### E0507 · Orphan-rule violation for an interface impl

An `impl Type: Interface` block lives in a file that declares neither the interface nor the type; the orphan rule requires the impl to be co-located with one of them.

```cplus
// in a third file that imports both Iface and Ty:
impl Ty: Iface { fn a(this) -> i32 { return 0; } }
```

**Fix.** Declare the impl in the same file as either the interface or the type.

<sub>repro: source · cplus-core/src/sema.rs:2857</sub>

### E0508 · `This` used outside an interface or impl body

The type `This` is named where there is no surrounding `interface` or `impl` body to give it meaning.

```cplus
fn loose(x: This) -> i32 { return 0; }
fn main() -> i32 { return 0; }
```

**Fix.** Use a concrete type name, or move the code into an `interface` / `impl` body.

<sub>repro: checked · cplus-core/src/sema.rs:12332 · test cplus-core/src/sema.rs:self_outside_impl_or_interface_e0508</sub>

### E0509 · Move of a field out of a `Drop` type

A non-Copy value is moved out of a field or index of a place whose type implements `drop`, which would let the destructor free the moved field a second time.

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
struct Owned { ptr: *u8 }
impl Owned {
    fn make() -> Owned { return Owned { ptr: { malloc(16 as usize) } }; }
    fn drop(ref this) { { free(this.ptr); } return; }
}
struct Pair { a: Owned, b: Owned }
impl Pair {
    fn drop(ref this) { { free(this.a.ptr); } { free(this.b.ptr); } return; }
}
fn main() -> i32 {
    let p: Pair = Pair { a: Owned::make(), b: Owned::make() };
    let q: Owned = p.a;
    return 0;
}
```

**Fix.** Clone the field, or restructure so it is not owned by a `Drop` type.

<sub>repro: checked · cplus-core/src/sema.rs:11316 · test cpc/tests/e2e.rs:e0509_move_field_out_of_drop_type_rejected</sub>

### E0510 · Unaccounted raw-pointer field in a `Drop` type

A struct has a raw-pointer field that is neither released in a `drop` (no releasing `drop`, or only via a helper) nor marked `opaque`.

```cplus
extern fn malloc(n: usize) -> *u8;
struct Buf { ptr: *u8 }
fn main() -> i32 { return 0; }
```

**Fix.** Release it in `drop` (`free(this.f)`), or mark the field `opaque` if another owner frees it.

<sub>repro: checked · cplus-core/src/sema.rs:4446 · test cpc/tests/e2e.rs:phase11_type_alias_cycle_rejected_e0510</sub>

### E0513 · Returning a `str` / `T[]` view of a local that drops

A returned `str` / `T[]` view is rooted at a function-local non-Copy owned value (a coerced `Text`, or an explicit `as_str` / `as_slice` view, including inside a returned aggregate), so the view would dangle when that local is freed at return.

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
struct Buf { ptr: *u8 }
impl Buf {
    fn drop(ref this) { { free(this.ptr); } return; }
    fn as_str(this) -> str { return { #str_from_raw_parts(this.ptr, 4 as usize) }; }
}
fn mk_buf() -> Buf { return Buf { ptr: { malloc(4 as usize) } }; }
fn bad() -> str {
    let s: Buf = mk_buf();
    return s.as_str();
}
```

**Fix.** Return an owned value (`Text` / `Vec[T]`), or borrow from a parameter.

<sub>repro: checked · cplus-core/src/sema.rs:12259 · test cpc/tests/e2e.rs:return_borrow_of_local_owned_rejected_e0513</sub>

### E0612 · Interpolated type does not implement `ToText`

A `${...}` interpolation segment embeds a value whose type does not implement `ToText` (and is not a blessed/numeric type or an owned `Text`).

```cplus
struct Point { x: i32, y: i32 }
fn main() -> i32 {
  let p: Point = Point { x: 1, y: 2 };
  let s = "point: ${p}";
  return 0;
}
```

**Fix.** Implement `ToText` for the type, or interpolate a field that is already `ToText`-able.

<sub>repro: checked · cplus-core/src/sema.rs:9517 · test cpc/tests/e2e.rs (E0612 interpolation of non-ToText Point)</sub>

### E0613 · Owned string (`Text`) named without its import

An expression produces an owned string (via `.to_text()` or string interpolation) but the `Text` type is not in scope because `stdlib/text` was not imported.

```cplus
fn f() -> i32 { let n: i32 = 1; let s = n.to_text(); return 0; }
```

**Fix.** Add `import "stdlib/text"`; borrowed `str` views need no import.

<sub>repro: checked · cplus-core/src/sema.rs:9020 · test cplus-core/src/sema.rs:to_text_without_text_import_rejected_e0613_v0019</sub>

### E0860 · `Send` / `Sync` marker impl has a non-empty body

`Send` and `Sync` are marker interfaces with no methods; the assertion is the empty `impl Type: Send {}` itself. A non-empty body is rejected.

```cplus
struct Handle { opaque p: *u8 }
impl Handle: Send { fn x(this) -> i32 { return 0; } }
fn main() -> i32 { return 0; }
// -> [E0860] `impl Handle: Send` must have an empty body
```

**Fix.** Make the body empty: `impl Type: Send {}`.

<sub>repro: checked · cplus-core/src/sema.rs:2963 · test cplus-core/src/sema.rs:bare_impl_send_registers_marker_override</sub>

### E0861 · Empty `impl` of an interface that is not `Send` / `Sync`

An empty `impl Type: Interface {}` was written for an interface other than `Send` / `Sync`. An empty impl is meaningful only as a `Send` / `Sync` marker assertion; every other interface's methods must be provided.

```cplus
interface Greet { fn hi(this) -> i32; }
struct S { x: i32 }
impl S: Greet {}
fn main() -> i32 { return 0; }
// -> [E0861] empty `impl` applies only to the `Send` / `Sync` markers
```

**Fix.** Implement the interface's methods, or remove the impl.

<sub>repro: checked · cplus-core/src/sema.rs:2993 · test cplus-core/src/sema.rs:empty_impl_on_regular_interface_rejected_e0861</sub>

## Modules, paths, and visibility

### E0401 · Imported file not found

An `import "..."` string did not resolve to an existing `.cplus` file on disk.

```cplus
import "./missing" as m;
fn main() -> i32 { return 0; }
```

**Fix.** Correct the import path (the compiler offers a did-you-mean for the closest existing filename), or create the file.

<sub>repro: scenario · cplus-core/src/resolver.rs:556 · test cpc/tests/e2e.rs:import_not_found_emits_e0401</sub>

### E0402 · Unknown import prefix

A `prefix::Item` path uses an `as` prefix that was never bound by an `import` declaration in this file.

```cplus
import "ghost/widget" as g;   // `ghost` is not a declared dependency
fn use_it() -> i32 { return g::value(); }
```

*Needs a project: the import path's first segment names no dependency in `Cplus.toml`. A bare unknown name in code is reported as E0300/E0303 instead.*

**Fix.** Add the matching `import "./module" as prefix;`, or fix the prefix to one that is imported.

<sub>repro: source · cplus-core/src/resolver.rs:585</sub>

### E0403 · Private item accessed across a file boundary

A cross-file reference touched a function, type, field, method, const, static, type alias, or interface whose name begins with `_` (module-private) in its declaring file.

```cplus
import "./math" as math;
fn main() -> i32 { return math::square(7); }
```

**Fix.** Remove the leading `_` from the name to make it public (or `export` it for the C ABI). (Requires an imported module; `math.cplus` declares `fn _square` as private.)

<sub>repro: scenario · cplus-core/src/resolver.rs:624 · test cpc/tests/e2e.rs:cross_file_private_fn_emits_e0403</sub>

### E0404 · Cyclic import dependency

The `import` graph contains a cycle, so the files mutually depend on each other and cannot be ordered.

```cplus
import "./a" as a;
fn main() -> i32 { return 0; }
```

**Fix.** Break the cycle: factor the shared declarations into a third module that both files import. (Requires multiple files; here `a.cplus` imports `b.cplus` which imports `a.cplus`.)

<sub>repro: scenario · cplus-core/src/resolver.rs:597 · test cpc/tests/e2e.rs:cyclic_imports_emit_e0404</sub>

### E0405 · No such item in module

A `prefix::name` path (or duplicate `as` prefix) names an item that does not exist in the imported module at all, or two imports share an `as` prefix.

```cplus
import "./lib" as lib;
fn main() -> i32 { return lib::nope(); }
```

**Fix.** Fix the name to one the module actually exports, or give each import a distinct `as` prefix. (Requires an imported module; `lib.cplus` has no item named `nope`.)

<sub>repro: scenario · cplus-core/src/resolver.rs:628 · test cpc/tests/e2e.rs:cross_module_unknown_item_reports_e0405_g030</sub>

### E0406 · Malformed or incomplete manifest

`Cplus.toml` failed to parse, is missing a required field, or names an unsupported `edition`.

```toml
[[[ not valid toml
```

**Fix.** Repair the TOML, supply the missing field, or set `edition = "2026"`.

<sub>repro: scenario · cplus-core/src/manifest.rs:311 · test cpc/tests/e2e.rs:malformed_manifest_emits_e0406_json</sub>

### E0407 · Cannot read the manifest

An I/O error occurred while reading `Cplus.toml` (for example the file is unreadable or vanished mid-build).

```toml
[package]
name = "x"
```

**Fix.** Ensure `Cplus.toml` exists and is readable from the build directory.

<sub>repro: source · cplus-core/src/manifest.rs:307</sub>

### E0408 · Both `[[bin]]` and `[lib]` declared

A single manifest declares both a binary target and a library target, which are mutually exclusive.

```toml
[package]
name = "both"

[[bin]]
name = "exe"

[lib]
```

**Fix.** A manifest is either an executable or a library; split it into two crates if you need both.

<sub>repro: scenario · cplus-core/src/manifest.rs:332 · test cpc/tests/e2e.rs:bin_and_lib_in_one_manifest_emit_e0408</sub>

### E0409 · `fn main` defined in a library target

A manifest that declares `[lib]` also defines a `fn main`, but a library has no entry point.

```cplus
fn add(a: i32, b: i32) -> i32 { return a + b; }
fn main() -> i32 { return 0; }
```

**Fix.** Remove `fn main`, or use `[[bin]]` instead of `[lib]` if you meant to build an executable. (Requires a `[lib]` manifest.)

<sub>repro: scenario · cpc/src/main.rs:1810 · test cpc/tests/e2e.rs:lib_target_rejects_fn_main_with_e0409</sub>

### E0410 · Type in `export extern fn` is not C-ABI compatible

A parameter or return type in an `export extern fn` cannot cross the C function-call ABI (for example a `str`/slice fat pointer, a tagged enum, a non-`#[repr(C)]` struct, or a `Drop` type).

```cplus
export extern fn echo(s: str) -> i32 { return 0; }
fn main() -> i32 { return 0; }
```

**Fix.** Use C-representable types: pass a `*u8` plus a `usize` length instead of a fat pointer, or mark structs `#[repr(C)]`.

<sub>repro: checked · cplus-core/src/sema.rs:4833 · test cplus-core/src/sema.rs:pub_extern_fn_with_str_rejected_e0410</sub>

### E0411 · `restrict` on a non-pointer parameter

The `restrict` marker was placed on a parameter whose type is not a raw pointer.

```cplus
fn bad(restrict x: i32) -> i32 { return x; }
fn main() -> i32 { return bad(0); }
```

**Fix.** Only `*T` accepts `restrict`; remove it or change the parameter to a raw-pointer type.

<sub>repro: checked · cplus-core/src/sema.rs:3199 · test cplus-core/src/sema.rs:restrict_on_integer_param_e0411</sub>

### E0412 · Unsupported `crate-type` value

A `[lib]` `crate-type` value is not one of the accepted kinds.

```toml
[package]
name = "mathlib"

[lib]
crate-type = "rlib"
```

**Fix.** Use one of `staticlib`, `cdylib`, or `both`.

<sub>repro: source · cplus-core/src/manifest.rs:337</sub>

## Generics and bounds

### E0500 · Cannot infer a type parameter

A declared generic parameter never appears in an argument position, so the compiler cannot infer it from the call's arguments.

```cplus
fn make[T]() -> i32 { return 0; }
fn main() -> i32 { return make(); }
```

**Fix.** Supply the `name::[T1, T2](...)` turbofish, or use the parameter in an argument so inference can pin it.

<sub>repro: checked · cplus-core/src/sema.rs:8908</sub>

### E0501 · Wrong type-argument count

A turbofish or generic instantiation supplied a different number of type arguments than the generic parameter list declares (including supplying any on a non-generic item).

```cplus
fn id[T](x: T) -> T { return x; }
fn main() -> i32 { let a: i32 = id::[i32, bool](7); return a; }
```

**Fix.** Match the generic parameter list: supply exactly as many type arguments as the declaration has.

<sub>repro: checked · cplus-core/src/sema.rs:12404 · test cplus-core/src/sema.rs:turbofish_wrong_arity_e0501</sub>

### E0502 · Bound not satisfied

A concrete type argument does not satisfy a declared bound on its type parameter (also fired for a `!Send` / `!Sync` type passed where `Send` / `Sync` is required across threads).

```cplus
fn max[T: Ord](a: T, b: T) -> T { return a; }
struct Point { x: i32 }
fn main() -> i32 { let p: Point = Point { x: 0 }; let r: Point = max(p, p); return 0; }
```

**Fix.** `T: Ord` requires `impl Point: Ord`; provide the impl, or for thread-crossing use `impl T: Send {}` when the marker holds.

<sub>repro: checked · cplus-core/src/sema.rs:1838 · test cplus-core/src/sema.rs:bound_violation_at_generic_fn_call_e0502</sub>

## Unsafe, FFI, and intrinsics

### E0700 · Tuple literal with fewer than two elements

A tuple literal was written with zero or one element, but `()` is the unit value and `(x)` is grouping, so a tuple must have at least two elements.

```cplus
fn main() -> i32 {
    let t = (1,);
    return 0;
}
```

**Fix.** Add a second element, or use `()`/`(x)` if you meant the unit value or a parenthesized expression.

<sub>repro: checked · cplus-core/src/sema.rs:6894</sub>

### E0821 · Cannot take the address of a generic function

A generic function name was used as a function-pointer value without specifying its type parameters, so there is no single monomorphized instance to point at.

```cplus
fn identity[T](x: T) -> T { return x; }
fn main() -> i32 { let f: fn(i32) -> i32 = identity; return 0; }
```

**Fix.** Specify the type parameters at the take-address site (turbofish), so a concrete instance is selected.

<sub>repro: checked · cplus-core/src/sema.rs:13148 · test cplus-core/src/sema.rs:generic_fn_as_pointer_rejected_e0821</sub>

### E0905 · Unknown compiler intrinsic `#name`

A `#name(...)` intrinsic is not recognized, or a compiler builtin was called as a bare name instead of with the `#` sigil.

```cplus
fn main() -> i32 { return #not_a_real_intrinsic(1); }
```

**Fix.** Fix the typo; check the [intrinsics](/docs/intrinsics) list, and spell builtins with the `#` sigil.

<sub>repro: checked · cplus-core/src/sema.rs:5860 · test cplus-core/src/sema.rs:unknown_intrinsic_still_e0905_v0019</sub>

## Compile-time builtins

### E0870 · `#include_bytes`/`#include_str` file not found

The path passed to `#include_bytes`/`#include_str` could not be resolved or read relative to the including file at compile time.

```cplus
fn main() -> i32 { let s: str = #include_str("missing.txt"); return 0; }
```

**Fix.** Correct the path (it is resolved relative to the file containing the call) or create the missing file.

<sub>repro: checked · cplus-core/src/sema.rs:6764 · test cplus-core/src/sema.rs:include_str_missing_file_e0870</sub>

### E0871 · `#include_bytes`/`#include_str` argument must be a string literal

The path argument to `#include_bytes`/`#include_str` was not a string literal, so the file cannot be resolved at compile time.

```cplus
fn main() -> i32 { let s: str = #include_str(some_var); return 0; }
```

**Fix.** Pass a string literal path, e.g. `#include_str("data.txt")`.

<sub>repro: checked · cplus-core/src/sema.rs:6325</sub>

### E0872 · `#include_bytes`/`#include_str` file exceeds the 64 MiB cap

The file embedded via `#include_bytes`/`#include_str` is larger than the 64 MiB sanity limit the compiler will read at compile time.

```cplus
fn main() -> i32 { let b: *const [u8; 0] = #include_bytes("huge.bin"); return 0; }
// where huge.bin is larger than 64 MiB
```

**Fix.** Embed a smaller file, or load the data at runtime instead of compile time.

<sub>repro: source · cplus-core/src/sema.rs:6793</sub>

### E0873 · SIMD lane/shift index must be a literal

A SIMD `.lane(...)` or shift method was given a non-literal `u32` index, but the lane/shift count must be a compile-time literal.

```cplus
fn main() -> i32 {
    let v: f32x4 = f32x4::splat(1.0f32);
    var i: u32 = 0 as u32;
    let x: f32 = v.lane(i);
    return 0;
}
```

**Fix.** Pass a literal `u32` index, e.g. `v.lane(0 as u32)`.

<sub>repro: checked · cplus-core/src/sema.rs:9921 · test cplus-core/src/sema.rs:simd_lane_non_literal_e0873</sub>

### E0874 · SIMD lane/shift index out of range

A SIMD `.lane(...)` index or shift count is at or beyond the vector's lane count (or the per-lane bit width for shifts).

```cplus
fn main() -> i32 {
    let v: f32x4 = f32x4::splat(1.0f32);
    let x: f32 = v.lane(7 as u32);
    return 0;
}
```

**Fix.** Use an index within range (0..lane_count), or a shift count below the lane bit width.

<sub>repro: checked · cplus-core/src/sema.rs:9933 · test cplus-core/src/sema.rs:simd_lane_out_of_range_e0874</sub>

### E0875 · `#include_str` file is not valid UTF-8

The file embedded via `#include_str` contains bytes that are not valid UTF-8; the message reports the byte offset of the first invalid byte.

```cplus
fn main() -> i32 { let s: str = #include_str("bad.bin"); return 0; }
// where bad.bin contains a stray 0xFF byte
```

**Fix.** Use `#include_bytes` for binary data, or fix the file so it is valid UTF-8.

<sub>repro: scenario · cplus-core/src/sema.rs:6711 · test cpc/tests/e2e.rs:include_str_rejects_non_utf8_file_with_e0875</sub>

### E0876 · `#env("X")`: env var not set at compile time

The environment variable named in `#env("NAME")` was not set in the compiler's own process environment when `cpc` was invoked.

```cplus
fn main() -> i32 {
    let _v: str = #env("CPC_TEST_DEFINITELY_MISSING_99");
    return 0;
}
```

**Fix.** Set the variable when invoking `cpc`, or pick a different default.

<sub>repro: checked · cplus-core/src/sema.rs:6664 · test cplus-core/src/sema.rs:env_macro_missing_var_e0876</sub>

### E1000 · Missing stdlib type for `gen fn` / `Iterator::next`

A `gen fn` was used without `Iterator[T]` from `stdlib/iterator` in scope (or `Iterator::next` was reached without `Option[T]` from `stdlib/option`), so the compiler cannot synthesize the iterator/option type.

```cplus
gen fn count_up(n: i32) -> i32 {
    var i: i32 = 1;
    while i <= n { yield i; i = i +% (1 as i32); }
    return;
}
fn main() -> i32 { return 0; }
// fails when `import "stdlib/iterator"` is absent
```

**Fix.** Add `import "stdlib/iterator"` (and `import "stdlib/option"`) so the required generic types are available.

<sub>repro: source · cplus-core/src/sema.rs:3390</sub>

### E1001 · `yield` outside a `gen fn` body

A `yield` expression appeared outside the body of a `gen fn`, where there is no iterator to produce values into.

```cplus
fn main() -> i32 {
    yield 1;
    return 0;
}
```

**Fix.** Move the `yield` into a `gen fn` body, or remove it.

<sub>repro: checked · cplus-core/src/sema.rs:5551</sub>

## Real-time contracts

### E0900 · Borrow-shaped parameter in an `async fn`

An `async fn` parameter is borrow-shaped (`str` / `T[]`) or a `ref`-bound non-Copy value (pointer-passed), which may dangle once a borrow lives across an `await`.

```cplus
struct Future[T] { opaque handle: *u8 } async fn fetch(url: str) -> i32 { return 0 as i32; }
```

**Fix.** Use `Text` / `Vec[T]` instead of `str` / `T[]`, or `take` ownership in / bind locally instead of `ref`.

<sub>repro: checked · cplus-core/src/sema.rs:4771 · test cplus-core/src/sema.rs:async_fn_with_str_param_emits_e0900</sub>

### E0901 · `#[no_alloc]` violation (or `await` outside `async fn`)

A `#[no_alloc]` function or a callee heap-allocates, builds an interpolated `Text`, runs allocating drop-glue at scope exit, or calls something not proven non-allocating; the code reused for the contract also rejects `await` outside an `async fn`.

```cplus
fn helper(x: i32) -> i32 { return x +% 1; }
#[no_alloc] fn caller(x: i32) -> i32 { return helper(x); }
fn main() -> i32 { return 0; }
```

**Fix.** Remove the allocation (or the offending call), drop the `#[no_alloc]` contract, or mark the callee `#[no_alloc]`.

<sub>repro: checked · cplus-core/src/sema.rs:3895 · test cplus-core/src/sema.rs:no_alloc_calls_unmarked_user_fn_e0901</sub>

### E0902 · `await` of a non-`Future` expression

An `await` is applied to an expression that does not evaluate to a `Future[T]`.

```cplus
struct Future[T] { opaque handle: *u8 } async fn bad() -> i32 { let x: i32 = await (7 as i32); return x; }
```

**Fix.** Await a `Future[T]` value (the result of calling an `async fn`).

<sub>repro: checked · cplus-core/src/sema.rs:5527 · test cplus-core/src/sema.rs:await_of_non_future_e0902</sub>

### E0903 · Invalid compiler-intrinsic call shape

A `#name(...)` intrinsic (such as `#selector` or `#compile_shader`) is called with the wrong number/kind of arguments, stray type arguments, or an unsupported `-> T` return ascription.

```cplus
fn main() -> i32 {
    let n: i32 = 42;
    let p: *u8 = #selector(n);
    return 0;
}
```

**Fix.** Call the intrinsic with the exact argument shape it documents (e.g. `#selector` takes one string literal).

<sub>repro: checked · cplus-core/src/sema.rs:5886 · test cplus-core/src/sema.rs:intrinsic_selector_non_string_e0903</sub>

### E0904 · `#compile_shader` target or toolchain error

A `#compile_shader(...)` names an unsupported target, or the shader toolchain invocation (xcrun metal / metallib) failed or produced no output.

```cplus
fn main() -> i32 {
    let p: *u8 = #compile_shader("k.spv", "spirv") as *u8;
    return 0;
}
```

**Fix.** Use a supported target (`"msl"`) and make sure the shader source compiles with the toolchain.

<sub>repro: checked · cplus-core/src/sema.rs:6006 · test cplus-core/src/sema.rs:intrinsic_compile_shader_bad_target_e0904</sub>

### E0906 · `#[bounded_recursion]` violation

The call graph of a `#[bounded_recursion]` function cycles back to itself, directly or transitively.

```cplus
#[bounded_recursion] fn r(x: i32) -> i32 {
    if x == 0 { return 0; }
    return r(x -% 1);
}
fn main() -> i32 { return 0; }
```

**Fix.** Break the recursion so the call graph no longer cycles back to the function.

<sub>repro: checked · cplus-core/src/sema.rs:4128 · test cplus-core/src/sema.rs:bounded_recursion_self_recursive_e0906</sub>

### E0907 · `#[no_block]` violation

A `#[no_block]` function or a callee calls a blocking primitive directly or transitively, or an extern/user function not proven non-blocking.

```cplus
extern fn sleep(secs: u32) -> u32;
#[no_block] fn f() { { sleep(1); } return; }
fn main() -> i32 { return 0; }
```

**Fix.** Use a non-blocking API, or mark the callee `#[no_block]` if it is known not to block.

<sub>repro: checked · cplus-core/src/sema.rs:4204 · test cplus-core/src/sema.rs:no_block_direct_sleep_e0907</sub>

### E0908 · `#[max_stack(N)]` exceeded

A function's estimated stack frame (parameters plus locals with known types) is larger than the `#[max_stack(N)]` byte budget.

```cplus
#[max_stack(64)] fn f() { let buf: [u8; 100] = [0u8; 100]; return; }
fn main() -> i32 { return 0; }
```

**Fix.** Shrink locals/parameters, or raise the `N` budget.

<sub>repro: checked · cplus-core/src/sema.rs:4512 · test cplus-core/src/sema.rs:max_stack_large_array_over_budget_e0908</sub>

### E0909 · Non-asm statement in a `#[naked]` function

A `#[naked]` function body contains a statement (or a value tail) other than inline `#asm(...)`; no prologue/epilogue is emitted, so there is no stack frame to use.

```cplus
#[naked]
fn bad() -> i64 { let x: i64 = 1; return x; }
fn main() -> i32 { return 0; }
```

**Fix.** Keep a `#[naked]` body [inline assembly](/docs/inline-assembly) only; move other code into a normal function the asm calls.

<sub>repro: checked · cplus-core/src/sema.rs:3933 · test cplus-core/src/sema.rs:naked_non_asm_statement_e0909</sub>

## Attributes

### E0354 · Unknown attribute

An attribute name is not recognized.

```cplus
#[tset] fn x() { return; }
```

**Fix.** Fix the typo (the compiler suggests a did-you-mean fix).

<sub>repro: checked · cplus-core/src/attrs.rs:611 · test cplus-core/src/attrs.rs:unknown_attribute_e0354</sub>

### E0355 · Bad attribute argument shape

An attribute is given the wrong arguments — too many, too few, or the wrong literal kind for what the attribute expects.

```cplus
#[repr] struct P { x: i32 }
```

**Fix.** Supply the exact argument shape the attribute expects (e.g. `#[repr(C)]`).

<sub>repro: checked · cplus-core/src/attrs.rs:675 · test cplus-core/src/attrs.rs:repr_missing_arg_e0355</sub>

### E0356 · Wrong attribute target

An attribute is placed on a kind of item it does not apply to; some attributes are function-only, others struct-only.

```cplus
#[test] struct X { v: i32 }
```

**Fix.** Move the attribute to the item kind it is valid on.

<sub>repro: checked · cplus-core/src/attrs.rs:697 · test cplus-core/src/attrs.rs:test_attribute_on_struct_rejected_e0356</sub>

### E0357 · Duplicate attribute

An attribute that must be unique appears more than once on the same item.

```cplus
#[test] #[test] fn x() { return; }
```

**Fix.** Remove the duplicate; the attribute may appear only once.

<sub>repro: checked · cplus-core/src/attrs.rs:713 · test cplus-core/src/attrs.rs:duplicate_test_attribute_e0357</sub>

### E0358 · Invalid `#[test]` function signature

A `#[test]` function does not have the signature `fn() -> i32` or `fn()` — it takes parameters or returns some other type.

```cplus
#[test] fn t(n: i32) { return; }
fn main() -> i32 { return 0; }
```

**Fix.** Give the test function the signature `fn() -> i32` or `fn()` (no parameters).

<sub>repro: checked · cplus-core/src/sema.rs:4628 · test cplus-core/src/sema.rs:test_fn_with_param_rejected_e0358</sub>

### E0359 · `#[test]` function cannot be `export`

A `#[test]` function is marked `export`; tests are project-internal helpers discovered by the runner, never part of the exported C-ABI surface.

```cplus
#[test] export fn t() { return; }
fn main() -> i32 { return 0; }
```

**Fix.** Remove `export` from the test function.

<sub>repro: checked · cplus-core/src/sema.rs:4874 · test cplus-core/src/sema.rs:test_fn_export_rejected_e0359</sub>

### E0890 · Duplicate `#asm` operand name

Two operands of an inline `#asm(...)` share the same operand name.

```cplus
fn f(a: i64) { { #asm("mov {a}, {a}", a = in(reg) a, a = in(reg) a); } return; }
fn main() -> i32 { return 0; }
```

**Fix.** Give each `#asm` operand a distinct name.

<sub>repro: checked · cplus-core/src/sema.rs:6562</sub>

### E0892 · Non-register-sized `#asm` operand

An inline `#asm(...)` operand has a type that does not fit a register; only integer, pointer, and `bool` operands are allowed.

```cplus
struct Owned { x: i32 } impl Owned { fn drop(ref this) { return; } } fn f(a: Owned) { { #asm("nop {a}", a = in(reg) a); } return; }
fn main() -> i32 { return 0; }
```

**Fix.** Pass a register-sized scalar (integer, pointer, or `bool`) instead of an aggregate.

<sub>repro: checked · cplus-core/src/sema.rs:6630 · test cplus-core/src/sema.rs:asm_tier2_non_scalar_operand_e0892</sub>

### E0893 · `#asm` `reg` operand has no template placeholder

A compiler-chosen (`reg`) inline-asm operand has no matching `{name}` placeholder in the template, so the template cannot name the register the compiler picked.

```cplus
fn f(a: i64) { { #asm("nop", a = in(reg) a); } return; }
fn main() -> i32 { return 0; }
```

**Fix.** Reference the operand by its `{name}` placeholder in the template, or use an explicit-register operand.

<sub>repro: checked · cplus-core/src/sema.rs:6576 · test cplus-core/src/sema.rs:asm_tier2_reg_missing_placeholder_e0893</sub>

### E0895 · `#asm` `out`/`inout` operand must be a variable

An `out` or `inout` inline-asm operand binds to a general place (a field or index) rather than a plain variable; those are not yet supported.

```cplus
struct P { x: i64 }
fn f(ref p: P, a: i64) {
    { #asm("mov {o}, {a}", o = out(reg) p.x, a = in(reg) a); }
    return;
}
fn main() -> i32 { return 0; }
```

**Fix.** Write the output into a `var` variable, then copy it into the field/index afterward.

<sub>repro: checked · cplus-core/src/sema.rs:6617 · test cplus-core/src/sema.rs:asm_tier2_out_must_be_variable_e0895</sub>

## const / static / char

### E0X30 · `const`/`static` initializer is not a literal

A `const` or `static` initializer used a non-literal shape (arithmetic, an identifier, a call, or a generic struct literal); `const` is literal-only and `static` allows only literals, `#zero::[T]()`, array literals/fills, or non-generic struct literals of such.

```cplus
const FOO: i32 = 1 + 2;
```

**Fix.** Use a literal initializer (or an accepted `static` shape such as `#zero::[T]()` or an array/struct literal of literals).

<sub>repro: checked · cplus-core/src/lower.rs:729 · test cplus-core/src/sema.rs:const_with_non_literal_initializer_e0x30</sub>

### E0X36 · Unknown `const` array length

An array length named a `const` that is not in scope, is not an integer, is negative, or exceeds the u32 maximum.

```cplus
fn main() -> i32 { let a: [i32; NOPE] = [0; 1]; return a[0]; }
```

**Fix.** Use an integer literal, or a `const` in scope with a non-negative integer literal initializer.

<sub>repro: checked · cplus-core/src/lower.rs:871 · test cpc/tests/e2e.rs:unknown_const_array_length_rejected_e0x36</sub>

## Targets and packages

### E0852 · Import names an undeclared dependency (or no manifest is reachable)

An import's first path segment looks like a package name but is not a declared `[dependencies]` entry in `Cplus.toml` (or there is no reachable manifest at all, so the bare `package/...` import has nothing to resolve against).

```cplus
// bare.cplus, compiled with `cpc --emit-obj bare.cplus` and no Cplus.toml in reach:
import "stdlib/atomic" as atomic;
fn f() -> i32 { return 0; }
// -> [E0852] first segment `stdlib` is not a declared dependency
```

**Fix.** Add `package = "*"` to `[dependencies]` in `Cplus.toml`, or change the import to `./path` for a file-relative one.

<sub>repro: scenario · cplus-core/src/resolver.rs:658 · test cpc/tests/e2e.rs:emit_obj_auto_detects_cplus_toml_g029</sub>

### E0853 · Bare import that is neither file-relative nor a declared dependency

An import path is not prefixed with `./` or `../` (so it is not file-relative) and its first segment does not match any declared `[dependencies]` entry, so the resolver cannot classify it.

```toml
import "bare" as b;
fn main() -> i32 { return 0; }
// -> [E0853] bare import `bare` — paths must start with `./`/`../` or match a `[dependencies]` entry
```

**Fix.** Use `./bare` for a file-relative import, or add `bare` to `[dependencies]` in `Cplus.toml` for a vendor import.

<sub>repro: scenario · cplus-core/src/resolver.rs:678 · test cpc/tests/e2e.rs:bare_import_emits_e0853</sub>

### E0854 · Vendor package missing its `Cplus.toml`

A `[dependencies]` entry resolves to a `vendor/<name>/` directory that has no `Cplus.toml`, so the vendor package's manifest cannot be loaded.

```toml
# consumer Cplus.toml
[package]
name = "app"
[dependencies]
foo = "*"
# but vendor/foo/Cplus.toml does not exist
# -> [E0854] vendor package `foo` is missing `Cplus.toml`
```

**Fix.** Create `vendor/<name>/Cplus.toml` for the dependency, or remove the `[dependencies]` entry.

<sub>repro: scenario · cpc/src/main.rs:1301 · test cpc/tests/e2e.rs:missing_vendor_manifest_emits_e0854</sub>

### E0855 · Vendor package name does not match its directory

A vendor package's `Cplus.toml` declares a `[package].name` that differs from the `vendor/<name>/` directory it lives in.

```toml
# vendor/foo/Cplus.toml
[package]
name = "bar"   # but the directory is vendor/foo/
# -> [E0855] declares name `bar` but lives in `vendor/foo/`
```

**Fix.** Make `[package].name` match the directory name (a vendor package's name must equal its directory).

<sub>repro: scenario · cpc/src/main.rs:1325 · test cpc/tests/e2e.rs:vendor_name_dir_mismatch_emits_e0855</sub>

### E0857 · Invalid dependency name

A `[dependencies]` key does not match `[a-z][a-z0-9_]*` (it contains dots, slashes, or uppercase), so the first segment of an import path would be ambiguous.

```toml
[package]
name = "x"

[dependencies]
Stdlib = "*"
# -> [E0857] dependency name `Stdlib` must match `[a-z][a-z0-9_]*`
```

**Fix.** Rename the dependency key to a lowercase identifier (no dots, slashes, or uppercase).

<sub>repro: scenario · cplus-core/src/manifest.rs:341 · test cplus-core/src/manifest.rs:invalid_dep_name_uppercase_rejected_e0857</sub>

### E0858 · Import path carries a `.cplus` extension

An import path ends in `.cplus`, but Phase 2 imports are extension-less, so the trailing extension is rejected.

```cplus
import "utils/math.cplus" as math;
fn main() -> i32 { return 0; }
// -> [E0858] import has a `.cplus` extension — drop it
```

**Fix.** Drop the `.cplus` extension from the import path (the compiler offers a machine-applicable suggestion).

<sub>repro: scenario · cplus-core/src/resolver.rs:714 · test cpc/tests/e2e.rs:stale_cplus_extension_in_import_emits_e0858</sub>

### E0859 · Vendor import escapes its `src/` directory

A vendor import path contains a `..` segment, which would let a package reach files outside its own `src/` directory — disallowed for security.

```cplus
import "utils/../escape" as e;
fn main() -> i32 { return 0; }
// -> [E0859] vendor import contains `..` — packages cannot reach outside their own `src/`
```

**Fix.** Remove the `..` segment; a package may only import files within its own `src/` tree.

<sub>repro: scenario · cplus-core/src/resolver.rs:728 · test cpc/tests/e2e.rs:vendor_escape_emits_e0859</sub>

### E0862 · Host vs target triple mismatch

A dependency declares bundled binaries but its `[link].triples` does not include the triple actually being linked (the host triple for a native build, or the selected `--target`'s artifact triple for a cross build), so no matching prebuilt artifact exists.

```toml
# vendor/foo/Cplus.toml
[package]
name = "foo"
[link]
bundled = ["libfoo.a"]
triples = ["aarch64-apple-darwin"]
# linking on/ for a triple not in that list:
# -> [E0862] package `foo` does not ship a build for host/target triple `<triple>`
```

**Fix.** Add the host/target triple to `[link].triples` and ship the matching binaries, or build the package from source for that triple.

<sub>repro: scenario · cpc/src/main.rs:1361 · test cpc/tests/e2e.rs:host_triple_unsupported_emits_e0862</sub>

### E0863 · `[link].bundled` set without `[link].triples`

A manifest's `[link].bundled` lists prebuilt binaries but `[link].triples` is empty, so the compiler cannot tell which host triples those binaries are built for.

```toml
# vendor/foo/Cplus.toml
[package]
name = "foo"
[link]
bundled = ["libfoo.a"]
# no triples = [...]
# -> [E0863] `[link].bundled` is non-empty but `[link].triples` is empty
```

**Fix.** Declare the host triples your bundled binaries target, e.g. `triples = ["aarch64-apple-darwin"]`.

<sub>repro: scenario · cplus-core/src/manifest.rs:345 · test cpc/tests/e2e.rs:bundled_without_triples_emits_e0863_via_build</sub>

### E0864 · `[link]` extra-objects entry not found

A `[link].extra-objects` path (resolved relative to the manifest) does not exist on disk, caught before clang is invoked so the user gets a clean diagnostic instead of a linker error.

```toml
[package]
name = "missing-obj"
[[bin]]
name = "missing-obj"
path = "src/main.cplus"
[link]
extra-objects = ["does-not-exist.o"]
# -> [E0864] [link] extra-objects entry `does-not-exist.o` not found
```

**Fix.** Provide the object file at the declared path, or remove the entry from `[link].extra-objects`.

<sub>repro: scenario · cpc/src/main.rs:1490 · test cpc/tests/e2e.rs:link_extra_objects_missing_file_rejected_e0864</sub>

### E0865 · `[link]` `${VAR}` not set and has no fallback

A `${VAR}` reference in `[link].search-paths` or `[link].extra-objects` names an environment variable that is unset at manifest-parse time and the reference carries no `:-default` fallback.

```toml
[package]
name = "x"
[link]
search-paths = ["${CPLUS_DEFINITELY_UNSET_VAR}/lib"]
# with the var unset:
# -> [E0865] cannot expand `${CPLUS_DEFINITELY_UNSET_VAR}/lib` in `[link]`
```

**Fix.** Set the variable, or give a default with `${VAR:-/path}` (caught at manifest parse time).

<sub>repro: scenario · cplus-core/src/manifest.rs:349 · test cplus-core/src/manifest.rs:link_search_paths_unset_env_var_rejected_e0865</sub>

### E0866 · A stdlib module the target lacks was imported

An import names a stdlib module excluded from the selected target's package profile — on an embedded target (e.g. `esp32-xtensa`) the POSIX half (`thread`, `net`, `fs`, the async `executor`/`reactor`, etc.) is unavailable.

```cplus
import "stdlib/thread" as m;
fn f() -> i32 { return 0; }
// compiled with `cpc check --target esp32-xtensa`
// -> [E0866] import `stdlib/thread` is not available on target `esp32-xtensa`
```

**Fix.** On an embedded target the POSIX modules are unavailable; use [`espidf`](/docs/packages/espidf) for the embedded equivalents.

<sub>repro: scenario · cplus-core/src/resolver.rs:696 · test cpc/tests/e2e.rs:target_esp32_gated_stdlib_modules_fire_e0866</sub>

### E0867 · `async fn` on a 32-bit target

An `async fn` is checked against a target whose pointer width is under 64 bits; the async runtime (reactor plus coroutine frames) is 64-bit-only today.

```cplus
fn helper() -> i32 { return 1; }
async fn fetch() -> i32 { return helper(); }
fn main() -> i32 { return 0; }
// compiled with `cpc check --target esp32-xtensa`
// -> [E0867] async functions are not supported on 32-bit target `esp32-xtensa`
```

**Fix.** The coroutine runtime is 64-bit only; restructure without `async` on that target.

<sub>repro: scenario · cplus-core/src/attrs.rs:559 · test cpc/tests/e2e.rs:target_esp32_async_fn_fires_e0867</sub>

## Warnings

### W0001 · `sum()` / `product()` over narrow integer SIMD lanes silently wraps

A horizontal `sum()` or `product()` over integer SIMD lanes narrower than 32 bits returns that same narrow lane type, which cannot hold the reduction of more than a couple of near-max lanes, so the result silently wraps.

```cplus
fn main() -> i32 {
    let a: i8x16 = i8x16::splat(50i8);
    let prod: i8x16 = a.mul(i8x16::splat(50i8));
    return prod.sum() as i32;
}
// -> W0001 `sum` over narrow integer lanes (`i8x16`) silently wraps
```

**Fix.** `.widen()` the lanes first, or use [`simd/integer::dot_i32`](/docs/packages/simd).

<sub>repro: checked · cplus-core/src/sema.rs:10191 · test cpc/tests/e2e.rs:simd_narrow_int_sum_warns_but_compiles</sub>

### W0002 · Conditionally-freed raw-pointer field in a `Drop` type

A raw-pointer field in a `Drop` type is freed inside `drop` only under some condition, so the compiler cannot prove the release always runs on every owning path.

```cplus
struct Cell { p: *u8 }
impl Cell: Drop {
    fn drop(this) {
        if some_condition() { free(this.p); }  // freed only conditionally
    }
}
// -> W0002 raw-pointer field `p` is freed only conditionally in `drop`
```

**Fix.** Confirm it frees on every owning path (expected for refcounted types).

<sub>repro: checked · cplus-core/src/sema.rs:4429</sub>

### W0003 · `[[bin]]` `[link]` libs / frameworks ignored

A `[[bin]]` package declares its own top-level `[link]` `libs` / `frameworks`, but those are read only when a package is a dependency of another — a `[[bin]]` is never a dependency, so they are ignored when building the binary.

```toml
[package]
name = "app"
[[bin]]
name = "app"
path = "src/main.cplus"
[link]
libs = ["boguslib"]
# -> W0003 `[link] libs` on a `[[bin]]` package is ignored when building the binary
```

**Fix.** Move them under `[[bin]]` `libs` / `frameworks` (top-level `[link]` libs apply only when the package is a dependency).

<sub>repro: scenario · cpc/src/main.rs:1654 · test cpc/tests/e2e.rs:bin_package_link_libs_warns_w0003</sub>
