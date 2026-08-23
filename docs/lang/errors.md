<!-- GENERATED from docs/lang/errors.toml by docs/lang/gen_errors.py — do not edit by hand. This is the maintainer reference; the public copy is the cplus-lang.dev /docs/error-codes page. -->

# Error codes

Every C+ diagnostic carries a numbered code, a source span, and often a machine-applicable suggestion. `cpc --diagnostics=json` emits the same information in a machine-readable shape for editors and agents. Codes prefixed with **W** are non-fatal warnings; the build continues. The normative ranges and what each phase owns are fixed in [§20 of the language specification](/docs/spec).

This is the complete index — **192 codes**. Each entry gives the meaning, a minimal example that triggers it, and the typical fix. **150** of the examples are reproduced directly by `cpc check`; the rest need a multi-file project, a `--target`, or a build-time file, and say so in the example.

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

### E0103 · Expression or statement nesting too deep

Source nests expressions, statement blocks, prefix operators, or types past the recursive-descent depth limit. Each level costs a native stack frame; without the bound the parser (and the later passes that recurse over the same tree) would overflow the stack and abort the process.

```cplus
fn main() -> i32 { return ((((((((((… 300 levels …)))))))))); }
```

**Fix.** Flatten the nesting: introduce intermediate `let` bindings, split deep expressions across statements, or reduce redundant parentheses. A limit this high is only reached by hostile or machine-generated input.

<sub>repro: checked · cplus-core/src/parser.rs:enter_depth · test cplus-core/src/parser.rs:pathological_nesting_reports_e0103_not_stack_overflow</sub>

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

A function name is used as a bare value (or another unsupported form such as `&x`, a range outside `for`, or a malformed path) where a callable or value of the right shape was required. An ASSOCIATED fn — `Type::f`, no receiver — is a namespaced fn and takes its address the same way; a METHOD is not, because `fn(this, …)` is not the `fn(…)` written at the binding, and there is nothing to supply its receiver.

```cplus
fn main() -> i32 { let x = 1; let y = &x; return 0; }
```

**Fix.** Assign it to a `fn(...)`-typed binding (or pass it where one is expected) to take the address. For a method, pass a bound method reference to a handler slot instead — the `*u8` after it carries the receiver.

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

An `as` cast is between a pair of types that the language forbids — or (v0.0.27) an `as?` checked narrowing was written with a non-integer side (`as?` supports plain integer source and target only).

```cplus
fn main() -> i32 { let _b: bool = 1 as bool; return 0; }
```

**Fix.** Some pairs are forbidden (for example `int` to `bool`, `*T` to `i32`); restructure the conversion. For `as?`, cast to a plain integer first — a distinct value via its base (`(x as i64) as? u8`).

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

A method call names a method (or free fn in the type's module) that the receiver's type does not have. On a `str` receiver this includes the `len()` habit (the stdlib spells it `count()`) and builds that never import `stdlib/str`, whose blessed `impl str` block declares the method set.

```cplus
struct P {}
impl P {}
fn main() -> i32 { let p: P = P {}; return p.missing(); }
```

**Fix.** Call a method the type actually declares, or define it in an `impl`. For `str`: use `count()`, add `import "stdlib/str"` somewhere in the build, or convert with `to_text()` when the operation needs an owned string.

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

Two methods on the same type share a name. Same block, two blocks, or an extension (E0388) walking into a name the type already has — an extension adds a method, it never replaces one. A method is declared once per program: two modules may not both add `one` to `abc`, whether or not any single file imports both. First declaration holds the name; the second reports.

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

### E0361 · Enum has no variants, or a `#[watch]` struct has no `on_value` hook

Two declaration shapes share this code. (1) An enum is declared with zero variants. Such a type is uninhabited (no value can ever be constructed), but match exhaustiveness treats it as vacuously covered and the tag ABI lowers it as a plain i32 — C+ has no uninhabited / never type. (2) A struct carries `#[watch]` but declares no `on_value` hook. The attribute installs a write barrier that calls the hook after every field store, so a `#[watch]` struct without one would mark a feature active that does nothing.

```cplus
enum Void {}
fn main() -> i32 { return 0; }
```

**Fix.** For the enum: declare at least one variant, or remove the enum. For the `#[watch]` struct: add `impl Name { fn on_value(ref this, field: str) { ... } }` — the signature is exact, and a wrong one is E0362.

<sub>repro: checked · cplus-core/src/sema.rs:1456 · test cplus-core/src/sema.rs:empty_enum_rejected_e0361</sub>

### E0364 · Cannot infer struct type of `{ ... }`

A type-inferred struct literal `{ field: ... }` appears where the expected type is absent or is not a known struct, so the compiler has no struct to construct.

```cplus
struct A { x: i32 }
fn main() -> i32 { let a = { x: 1 }; return 0; }
```

**Fix.** Name the struct (`A { field: ... }`), or give the binding a struct type annotation so the literal's type can be inferred.

<sub>repro: checked · cplus-core/src/sema.rs:check_inferred_struct_lit · test cplus-core/src/sema.rs:inferred_struct_lit_uninferable_e0364</sub>

### E0385 · Duplicate `impl str`

The builtin `str` view takes its method set from exactly one `impl str { ... }` block program-wide — stdlib's `src/str.cplus`. A second block, in any file or package, is a conflict.

```cplus
impl str { fn a(this) -> usize { return #str_len(this); } }
impl str { fn b(this) -> usize { return #str_len(this); } }
fn main() -> i32 { return 0; }
```

**Fix.** Remove the extra block. To add operations over `str`, write free functions taking a `str` parameter, or convert with `to_text()` and use `Text`.

<sub>repro: checked · cplus-core/src/sema.rs:collect_str_impl_methods · test cplus-core/src/sema.rs:impl_str_duplicate_block_e0385</sub>

### E0386 · Unsupported member in `impl str`

A method in the blessed `impl str` block has a shape the builtin does not support: generic parameters, `gen`/`async`, an associated fn (no receiver), a `ref this`/`take this` receiver (`str` is a Copy view — the receiver is always plain `this`), an interface conformance block, or a redeclaration of the compiler-provided `to_text`/`hash`/`eq`.

```cplus
impl str { fn m(ref this) -> usize { return #str_len(this); } }
fn main() -> i32 { return 0; }
```

**Fix.** Declare the method as a plain `fn name(this, ...)`; keep generics, interface impls, and the compiler-provided names off the block.

<sub>repro: checked · cplus-core/src/sema.rs:collect_str_impl_methods · test cplus-core/src/sema.rs:impl_str_bad_members_e0386</sub>

### E0387 · Generic impl away from its template

An `impl` on a generic type sits in a different file than the template it names. Concrete types may be extended from any module under the import gate (E0388); generic types may not — a generic impl stays in the template's own file.

```cplus
# in acme/src/a.cplus:  struct Holder[T] { v: T }
# in acme/src/b.cplus:
impl Holder[T] { fn get(this) -> T { return this.v; } }
fn main() -> i32 { return 0; }
```

**Fix.** Move the impl next to the `struct Name[T]` / `enum Name[T]` it extends.

<sub>repro: scenario · cplus-core/src/sema.rs:collect_generic_impl_methods · test cplus-core/src/sema.rs:ext_generic_impl_away_from_template_file_e0387</sub>

### E0389 · Extension declares a destructor

An extension declares `drop`. Every other extension method is opt-in — you see it where you imported it — but `drop` decides whether values of the type are torn down at all, everywhere they are owned. That cannot depend on which files imported what. A destructor also usually needs the private fields it releases, which an extension cannot see. It belongs to the module that declares the type.

```cplus
# in ext/ext.cplus, where `Point` is declared in dep/dep.cplus:
import "dep/dep" as d;
impl d::Point { fn drop(ref this) { } }
```

**Fix.** Move the destructor beside the `struct` it tears down, or expose a named release method the caller invokes.

<sub>repro: scenario · cplus-core/src/sema.rs:collect_methods · test cplus-core/src/sema.rs:ext_cross_package_may_not_declare_a_destructor_e0389</sub>

### E0390 · Unknown lang item

`#[lang("...")]` designates a declaration as one of the handful of types the compiler itself reaches for — the owned string, the `gen fn` and `async fn` protocol types, `Option`, `JoinHandle`. A name outside that set designates nothing, so the feature that was meant to find this type keeps looking and reports a missing-stdlib error somewhere else entirely.

```cplus
#[lang("iterater")] struct Iterator[T] { opaque _handle: *u8 }
```

**Fix.** Spell the lang item as one of the names the message lists, or drop the attribute.

<sub>repro: scenario · cplus-core/src/attrs.rs:emit_unknown_lang_item · test cplus-core/src/attrs.rs:lang_item_name_must_be_one_the_compiler_knows</sub>

### E0822 · Method cannot be used as a bound reference

`obj.method` in value position builds a handler from a function pointer plus the receiver's address. Not every method can be one: a `take this` method would consume the receiver on its first fire, a receiverless method has nothing to bind, and a generic method or one with `take` / `ref` parameters has no single fn-pointer shape to lower to. It is also refused anywhere inside a GENERIC impl body, whatever the receiver — the bridge is synthesized for one concrete type, and such a body is compiled once per instantiation (bug-29).

```cplus
struct S { n: i32 }
impl S { fn eat(take this) { return; } }
fn take_handler(f: fn(*u8), ctx: *u8 = 0 as *u8) -> i32 { return 1; }
fn main() -> i32 { var s: S = S { n: 0 }; return take_handler(s.eat); }
```

**Fix.** Give the method a `this` or `ref this` receiver and only by-value parameters, or pass a plain `fn` directly instead of a bound reference.

<sub>repro: checked · cplus-core/src/sema.rs:14629, :14640, :14655 · test cpc/tests/e2e.rs:bound_method_reference_misuse_rejected</sub>

### E0823 · Bound method reference does not fit the expected handler

Either the parameter's fn-pointer type does not match the method — a handler type must be the method's parameters plus a trailing `*u8` context, with the same return type — or the method takes `ref this` while the receiver expression is not a writable place (a `let` binding is not; a `var`, a field of one, or a `static` is).

```cplus
struct S { n: i32 }
impl S { fn tick(ref this) { this.n = this.n + 1; return; } }
fn take_handler(f: fn(*u8), ctx: *u8 = 0 as *u8) -> i32 { return 1; }
fn main() -> i32 { let s: S = S { n: 0 }; return take_handler(s.tick); }
```

**Fix.** Match the handler's declared fn-pointer shape, and bind a `ref this` method only to a writable receiver.

<sub>repro: checked · cplus-core/src/sema.rs:14617 (receiver place), :14673 (shape) · test cpc/tests/e2e.rs:bound_method_reference_misuse_rejected</sub>

### E0824 · Callee has no context slot for a bound method reference

A bound reference passes the receiver's address in the argument slot right after the handler, so the callee must declare a defaulted `*u8` context parameter there — and the call site must leave it to the compiler. Either the parameter is missing (or not a defaulted `*u8`), or the call wrote an explicit argument the bound reference would silently clobber.

```cplus
struct S { n: i32 }
impl S { fn tick(ref this) { this.n = this.n + 1; return; } }
fn take_handler(f: fn(*u8)) -> i32 { return 1; }
fn main() -> i32 { var s: S = S { n: 0 }; return take_handler(s.tick); }
```

**Fix.** Declare `ctx: *u8 = 0 as *u8` immediately after the handler parameter, and omit it at the call site.

<sub>repro: checked · cplus-core/src/sema.rs:14688, :14707, :14726 · test cpc/tests/e2e.rs:bound_method_reference_misuse_rejected</sub>

### E0913 · Recursive type has infinite size

A struct or enum contains itself by value — directly (`struct S { s: S }`), mutually (`A` holds `B`, `B` holds `A`), or through an inline array (`[S; N]`). Such a type has no finite size.

```cplus
struct S { s: S }
fn main() -> i32 { return 0; }
```

**Fix.** Break the cycle with an indirection: store the recursive field behind a pointer (`*S`). A raw-pointer field needs `opaque` or a `fn drop(ref this)` (see E0510).

<sub>repro: checked · cplus-core/src/sema.rs:check_recursive_types · test cplus-core/src/sema.rs:recursive_struct_by_value_rejected_e0913</sub>

### E0917 · Item name contains reserved `__`

A struct, enum, or (non-extern) function name contains an interior `__`. The double underscore is the compiler's monomorphization separator (`Box[i32]` mangles to `Box__i32`), so a literal `Box__i32` next to a `Box[T]` template would collide with the instantiation's symbol — two items under one name, one silently shadowing the other.

```cplus
struct Box__i32 { v: i32 }
fn main() -> i32 { return 0; }
// -> [E0917] struct name `Box__i32` contains `__`, which is reserved for compiler name mangling
```

**Fix.** Use a single underscore (`box_i32`). Exempt: `extern fn` names (existing C symbols like `__errno_location` never monomorphize) and names whose only `__` is leading (`__x` — an instantiation's template base is never empty).

<sub>repro: checked · cplus-core/src/sema.rs:reject_reserved_double_underscore · test cplus-core/src/sema.rs:double_underscore_item_names_reserved_e0917</sub>

### E0919 · Declaration claims the reserved `__cplus_` runtime-ABI prefix

A function is declared with a name starting `__cplus_`, the prefix the compiler reserves for symbols it generates itself — the reactor helpers `#reactor_get_state` lowers to, the coroutine hooks, the thread trampolines, the bound-method bridges. A declaration under that prefix is claiming to name one of those symbols, and an unmarked one could take a runtime symbol's place at link time by accident or on purpose.

```cplus
extern fn __cplus_reactor_get_state() -> *u8;
fn main() -> i32 { return 0; }
// -> [E0919] `__cplus_reactor_get_state` starts with `__cplus_`, the compiler's reserved runtime-ABI prefix
```

**Fix.** If the declaration really does name a compiler-generated symbol (the stdlib reactor bindings do), mark it `#[runtime_abi]` — the same doctrine as `opaque` and `#[lang]`: a small trusted surface, written down. Otherwise pick a name outside the prefix.

<sub>repro: checked · cplus-core/src/sema.rs:reject_unmarked_runtime_abi_name · test cplus-core/src/sema.rs:unmarked_runtime_abi_prefix_rejected_e0919</sub>

### E0920 · Field not derivable for this interface

An empty `impl Type: Interface {}` asked the compiler to derive a memberwise implementation, but one of the struct's fields has a shape the derived method cannot handle: an enum with payload variants (no generated `match` in v1), an array / slice / tuple field, a pointer field where the interface needs `hash` or `cmp` or a text form, or — for `ToText` — a build with no `#[lang("string")]` type.

```cplus
enum E { A, B(i32) }
struct P { e: E }
impl P: Eq {}
fn main() -> i32 { return 0; }
// -> [E0920] cannot derive `Eq` for `P`: field `e` — its enum type has payload variants; write `eq` manually
```

**Fix.** Write the named method by hand for this type, or change the field to a derivable shape. For payload enums the usual fix is a hand-written method that `match`es on the variants.

<sub>repro: checked · cplus-core/src/lower.rs:expand_derives · test cplus-core/src/lower.rs:derive_payload_enum_field_rejected_e0920</sub>

### E0922 · `distinct` requires a plain integer base type

A `type X = distinct BASE;` declaration named a base that is not a plain integer type (i8–i64, u8–u64, isize, usize). Distinct aliases exist to give integers nominal identity at zero ABI cost; floats, pointers, strings, and aggregates are not supported bases.

```cplus
type Speed = distinct f64;
fn main() -> i32 { return 0; }
// -> [E0922] `distinct` requires a plain integer base type; `Speed` is `f64`
```

**Fix.** Use an integer base, or a wrapper struct for non-integer types.

<sub>repro: checked · cplus-core/src/sema.rs:finalize_distincts · test cplus-core/src/sema.rs:distinct_requires_integer_base_e0922</sub>

### E0923 · Invalid enum discriminant or representation

A payload-free-enum feature was used on the wrong shape: an explicit discriminant (`Variant = N`) or an integer `#[repr(...)]` on an enum with payload variants, a discriminant outside the pinned representation's range, or two variants with the same value.

```cplus
#[repr(u8)]
enum Mode { Off = 0, On = 300 }
fn main() -> i32 { return 0; }
// -> [E0923] discriminant 300 of `On` does not fit the enum's representation
```

**Fix.** Explicit discriminants and integer reprs describe C enums: keep the enum payload-free, keep every value inside the `#[repr]` type's range, and give each variant a unique value.

<sub>repro: checked · cplus-core/src/sema.rs:collect_type_names · test cplus-core/src/sema.rs:enum_discriminant_shapes_rejected_e0923</sub>

### E0924 · Impure `#[requires]` expression

A `#[requires(...)]` precondition used a construct with effects or evaluation-order weight — a call, an assignment, a block. A contract that can change state changes the program it guards, so the expression grammar is restricted to operators, literals, parameter and `const` reads, field reads, and casts.

```cplus
fn probe(x: i32) -> bool { return x > 0; }
#[requires(probe(n))]
fn f(n: i32) -> i32 { return n; }
fn main() -> i32 { return 0; }
// -> [E0924] a `#[requires]` expression must be pure
```

**Fix.** Restate the condition with pure reads, or hoist the computed value into a parameter the contract can read.

<sub>repro: checked · cplus-core/src/sema.rs:check_requires_attrs · test cplus-core/src/sema.rs:requires_impure_rejected_e0924</sub>

### E0925 · Invalid `union`

A `union` broke one of the rules that follow from it having no tag: a member type is not `Copy` (nothing can know which member is live, so no destructor can be run correctly), the union is generic (its `Copy` rule cannot be checked until the members are known, and C headers are not generic), it declares no members, or a union literal named other than exactly one member.

```cplus
union U { a: i32, b: u32 }
fn main() -> i32 { let x = U { a: 1, b: 2 }; return 0; }
// -> [E0925] a `union` literal names exactly one member
```

**Fix.** Keep every member `Copy`, keep the union non-generic and non-empty, and name exactly one member when constructing it — the one being made live.

<sub>repro: checked · cplus-core/src/sema.rs:check_unions · test cplus-core/src/sema.rs:union_shape_rules_e0925</sub>

### E0926 · Invalid `#[repr(packed)]`

A `#[repr(..., packed)]` / `#[repr(..., packed = N)]` declaration broke one of the rules that follow from packing moving fields off their natural alignment: `N` is not a power of two from 1 to 16, the attribute is on an `enum` (a single integer, with no fields to pack), a field's type is not `Copy` (a destructor is handed the address of what it tears down, and a packed field has none it can believe), or a `ref` / `#addr_of` tried to take the address of a field sitting at an offset its own type is not aligned to.

```cplus
#[repr(C, packed)] struct P { x: u8, y: u32 }
fn bump(ref v: u32) -> () { v = v + (1 as u32); }
fn main() -> i32 { var p: P = P { x: 1 as u8, y: 7 as u32 }; bump(p.y); return 0; }
// -> [E0926] `y` sits at offset 1 of a packed struct
```

**Fix.** Keep `packed = N` a power of two in 1..=16, put it on a struct, keep every field `Copy`, and read or write an under-aligned field directly instead of pointing at it — copy it into a local when something needs an address.

<sub>repro: checked · cplus-core/src/sema.rs:check_packing · test cplus-core/src/sema.rs:packed_shape_rules_e0926</sub>

### E0927 · Invalid bitfield

A `#[bits(N)]` field broke one of the rules that follow from a bitfield being bits inside a storage unit it shares: its type is not an integer, `N` is 0 or wider than that type, the struct is not `#[repr(C)]` (a bit position is a claim about C storage units), the field is a union member (every union member starts at offset 0), the struct is generic (a C header is not), or a `ref` / `#addr_of` tried to take its address — it has none of its own, and a pointer to it would read and write its neighbours.

```cplus
#[repr(C)] struct S { #[bits(3)] a: u32, #[bits(5)] b: u32 }
fn bump(ref v: u32) -> () { v = v + (1 as u32); }
fn main() -> i32 { var s: S = S { a: 1 as u32, b: 2 as u32 }; bump(s.a); return 0; }
// -> [E0927] `a` is a bitfield: it has no address of its own
```

**Fix.** Give the field an integer type and a width from 1 to that type's bit count, declare the struct `#[repr(C)]` and non-generic, and read or write the field directly instead of borrowing it. C's `:0` (force the next field to a boundary) has no C+ spelling; declare padding as a named field with the width you want skipped.

<sub>repro: checked · cplus-core/src/sema.rs:check_packing · test cplus-core/src/sema.rs:bitfield_shape_rules_e0927</sub>

### E0928 · Invalid `#[ensures]`

An `#[ensures(EXPR)]` used `result` where there is nothing to name — the function returns `()` — or the function already has a binding called `result`, so the contract's `result` and the declared one cannot both be meant. (A postcondition's purity rule is E0924, the same one `#[requires]` follows.)

```cplus
#[ensures(result > 0)]
fn nothing(n: i32) { return; }
// -> [E0928] `result` names the value being returned, and this function returns nothing
```

**Fix.** On a function that returns nothing, write the postcondition about parameters and `this` instead; a `ref` parameter or `this` field is exactly what such a function changes. Otherwise rename the parameter that collides with `result`.

<sub>repro: checked · cplus-core/src/sema.rs:check_contract_attrs · test cplus-core/src/sema.rs:ensures_result_needs_something_to_return_e0928</sub>

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

A non-Copy binding is read after it was moved (into a call, a `take` parameter, or a `let y = x;`). Flow-sensitive: a move only on a branch that `return`s / `break`s does not poison the other path, and it also fires for non-Copy types whose Copy-ness depends on a generic payload. A `match` also moves: matching an owned binding of a Drop-carrying enum consumes it when any arm binds a name, so the binding cannot be read or matched again.

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

**Fix.** Do not read after a `take`; clone the value first, or restructure so the move and the use are on disjoint paths. For a `match`: bind nothing (`E::A(_)`) if you only need to test the discriminant — that form does not consume, so the binding stays matchable. For a `guard let`, reach the complement payload with `else |E::B(x)|` rather than re-matching the scrutinee in the else block.

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

### E0343 · A `match` mixes literal and variant patterns

One `match` used both literal patterns (`1 => ...`) and variant patterns (`M::A => ...`). The two ask different questions — a literal matches a VALUE, a variant matches a CASE — and they lower to different code, so a mixed match has no single meaning.

```cplus
enum M { A, B }
fn main() -> i32 { let m: M = M::A; return match m { 1 => 0, M::B => 1 }; }
```

**Fix.** Split the match, or convert the literal arms into variants of the same enum.

<sub>repro: checked · cplus-core/src/lower.rs:859 · test cpc/tests/e2e.rs:literal_pattern_refusals_are_specific</sub>

### E0344 · Literal `match` is non-exhaustive or has an unreachable arm

A `match` over literals either has no catch-all — each literal arm covers exactly one value, so the compiler cannot prove the rest are handled — or it has an arm after a catch-all, which can never run.

```cplus
fn main() -> i32 { let n: i32 = 3; return match n { 1 => 10, 2 => 20 }; }
```

**Fix.** End a literal match with `_` or a binding arm, and put nothing after it.

<sub>repro: checked · cplus-core/src/lower.rs:844 (unreachable arm), :871 (non-exhaustive) · test cpc/tests/e2e.rs:literal_pattern_refusals_are_specific</sub>

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

**Fix.** Take the value by value (`take`) so the callee owns it, or `.clone()` it; return an owned value rather than a borrow. For the raw-pointer case in a container — reading a value OUT of storage the container owns and then disarming the source so it is never dropped twice — use `#take::[T](p)`, which states that ownership transfers here; the analysis cannot see the disarm, so it is declared, the same way `opaque` (E0510) and `#[keeps]` (E0516) declare at the raw seam.

<sub>repro: checked · cplus-core/src/sema.rs:11226 · test cplus-core/src/sema.rs:return_borrow_marker_param_rejected_e0337</sub>

### E0365 · A value that captured a local's address escapes the frame

A bound method reference (`obj.handler` in value position) lowers to a function pointer plus the receiver's raw ADDRESS. When the receiver is storage this frame frees — a local, a `take` parameter, a `take this`, or a by-value parameter of a `Copy` type — and the value holding that address then leaves the frame (returned, stored into a `static` or a `ref` target, or handed to a call), the handler points at a stack slot that is gone before it fires. The analysis is transitive: a method that binds its own receiver taints every value it returns, and a binding that absorbs such a value carries the capture onward.

```cplus
struct Child { clicks: i32 }
impl Child {
    fn clicked(ref this) { this.clicks = this.clicks + 1; return; }
    fn build(ref this) -> i32 { return take_handler(this.clicked); }
}
fn take_handler(f: fn(*u8), ctx: *u8 = 0 as *u8) -> i32 { return 1; }
fn make() -> i32 {
    var c: Child = Child { clicks: 0 };
    return c.build();
}
```

**Fix.** Give the receiver storage that outlives the escaping value — a field of `this`, a `static`, or a `Box`. Binding a handler to a local is legal as long as it does not escape; binding to `this` or to a field is always legal, which is why this costs nothing in ordinary component code.

<sub>repro: checked · cplus-core/src/borrowck.rs (ViewRules::check_capture_return / _store / _arg) · test cpc/tests/e2e.rs:a_capture_of_a_local_escaping_the_frame_is_rejected_e0365</sub>

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

### E0383 · Access to a binding while it is exclusively borrowed

A place is read, or has a method called on it, while an exclusive borrow of that same place is still live. The exclusive borrow claims the place for its whole lifetime, so no overlapping access is admitted. E0374 is the same rule for a place that only partially overlaps; E0381 is the shared-borrow twin.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn cursor(ref b: B) -> B { return b; }
fn peek(b: B) -> i32 { return b.x; }
fn caller() {
  var v: B = B { x: 1 };
  let cur: B = cursor(v);
  let n: i32 = peek(v);
  return;
}
```

*Not reachable through the driver as of v0.0.26: a function that returns a borrow of a `ref` parameter is rejected by sema (E0337) first, and sema errors bail the pipeline before borrowck runs. The rule is live in borrowck and pinned by its unit tests.*

**Fix.** End the borrow before the access — let the borrower go out of scope, or move it — or restructure so the two do not overlap.

<sub>repro: scenario · cplus-core/src/borrowck.rs:6081 (projected read), :6889 (method call) · test cplus-core/src/borrowck.rs:e0383_releases_when_exclusive_borrower_is_moved</sub>

### E0384 · Cannot infer which parameter a returned borrow comes from

A function with two or more borrow-like parameters returns a value rooted at one of them, but the elision rules cannot pick which — so the caller has nothing to tie the returned borrow's lifetime to.

```cplus
struct B { x: i32 }
impl B { fn drop(ref this) { return; } }
fn longest(ref a: B, ref b: B) -> B {
  if a.x > b.x { return a; }
  return b;
}
```

*Not reachable through the driver as of v0.0.26, for the same reason as E0383: returning a parameter is a sema E0337, which bails before borrowck runs.*

**Fix.** Return an owned value (`Text` / `Vec[T]`), or restructure so exactly one parameter can be the borrow's source. The `borrow REGION T` annotation this once suggested is retired and is not the remedy.

<sub>repro: scenario · cplus-core/src/borrowck.rs:5264 (build_e0384) · test cplus-core/src/borrowck.rs (6BC.4 elision tests)</sub>

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

A `str` / `T[]` view is rooted at storage with no lifetime long enough for it. Two shapes. (1) The owner is a function-local non-Copy value and the view escapes the frame — returned directly, returned inside an aggregate (a struct with a `str` field CARRIES the view's borrow, including when built through a call like `store(local.view(), ..)` or returned via an alias), or stored into a place that outlives the frame (a `static`, or a `ref` target); the local is freed at return, so the escaped view would dangle. (2) The owner is a TEMPORARY nothing names — `let s: str = mk().view();`, the `Text`->`str` coercion of an rvalue (`let s: str = t.clone();`, `let s: str = "x ${i}";`), or either of those captured into an aggregate a binding keeps (`Slot { s: mk().view() }`). A temporary is an anonymous slot of the statement; a binding that outlives it has nothing to borrow from. The same temporary at an ARGUMENT position is fine — it outlives the call.

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

**Fix.** Own the bytes instead: store/return `Text` / `Vec[T]`, or borrow the view from a non-`take` parameter. For a temporary owner, give it a name first — `let owner: Text = mk(); let s: str = owner.view();` — or keep the binding owned. Literal-backed views ('static bytes) escape freely.

<sub>repro: checked · cplus-core/src/borrowck.rs (ViewRules: check_return / flag_view_leaves / check_view_of_temp / check_view_of_rvalue_owner / check_captured_view_of_temp / check_store_escape) · test cpc/tests/e2e.rs:return_borrow_of_local_owned_rejected_e0513</sub>

### E0514 · Owner goes out of scope while a view of it is still live

A binding declared inside a block owns bytes that a view (or a view-carrying struct/enum) bound OUTSIDE the block still reads. The owner is dropped when the block ends, so the outer binding dangles from that point on. Assigning a view outward is the same escape as moving the owner while borrowed (E0372), caught at the scope boundary instead of at a move.

```cplus
struct Buf { x: i32 }
impl Buf {
    fn drop(ref this) { return; }
    fn view(this) -> str { return ""; }
}
fn bad() {
    var s: str = "";
    {
        let t: Buf = Buf { x: 1 };
        s = t.view();
    }
    return;
}
```

**Fix.** Declare the borrower inside the block, extend the owner's scope past the borrower's last use, or store owned bytes instead: `Text`, or `text::intern` when a set-once process-lifetime view is wanted.

<sub>repro: checked · cplus-core/src/borrowck.rs (walk_block_in_scope scope-exit check) · test cplus-core/src/borrowck.rs:scope_exit_under_live_borrow_fires_e0514</sub>

### E0515 · Storing a borrowed view parameter into a target that outlives the call

A `str` / `T[]` parameter (or a view-carrying one) is stored into a `static`, a `ref` parameter target, or a field of `ref this`. The caller only guarantees the view's bytes for the duration of the call; the target outlives it, so the stored view dangles as soon as the caller's owner drops. `str` params previously slipped through the owned-root check because `str` is Copy — this was the laundering path behind the stored-key use-after-free family.

```cplus
struct Holder { view: str }
impl Holder {
    fn set(ref this, k: str) {
        this.view = k;
        return;
    }
}
```

**Fix.** Own the bytes (a `Text` field), intern them (`text::intern` returns a process-lifetime view), or — for the receiver-store case only — declare the method `#[keeps(this)]`: the store becomes a declared flow and every caller ties the receiver to the argument's owner (E0372/E0514 then guard the owner).

<sub>repro: checked · cplus-core/src/borrowck.rs (ViewRules::check_store_escape, param-view arm) · test cpc/tests/e2e.rs:view_param_stored_into_static_rejected_e0515</sub>

### E0516 · Storing a view through a raw pointer without a declared flow

A `str` / `T[]` / view-carrying value is stored through a raw-pointer deref — `*slot = v`, and equally any projection of the pointee (`(*sink).key = v`, `(*sink)[i] = v`). No flow analysis can see what the pointer points at, so a field of the pointee is exactly as opaque as the whole pointee, and the function's effect on view lifetimes is unknowable — silence at the raw seam is not neutral, the same doctrine as the raw-pointer field rule (drop-or-`opaque`, E0510).

```cplus
fn stash(slot: *str, v: str) {
    *slot = v;
    return;
}
```

**Fix.** Declare the function's flow: `#[keeps(this)]` if the view survives inside the receiver (callers then tie the receiver to the argument's owner), or `#[keeps(nothing)]` if the bytes are copied and no borrowed view escapes. Better still, remove the seam: store an owned `Text` so the struct outlives its own bytes. Byte and pointer stores never trigger this — only a view value does, and a store to a field of a plain local is not a raw store at all.

<sub>repro: checked · cplus-core/src/borrowck.rs (ViewRules::check_raw_store) · test cpc/tests/e2e.rs:raw_view_store_requires_keeps_e0516</sub>

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

### E0915 · `Send` / `Sync` marker impl has a non-empty body

`Send` and `Sync` are marker interfaces with no methods; the assertion is the empty `impl Type: Send {}` itself. A non-empty body is rejected.

```cplus
struct Handle { opaque p: *u8 }
impl Handle: Send { fn x(this) -> i32 { return 0; } }
fn main() -> i32 { return 0; }
// -> [E0915] `impl Handle: Send` must have an empty body
```

**Fix.** Make the body empty: `impl Type: Send {}`.

<sub>repro: checked · cplus-core/src/sema.rs:2963 · test cplus-core/src/sema.rs:nonempty_send_marker_impl_rejected_e0915</sub>

### E0916 · Empty `impl` of an interface that is neither derivable nor a marker

An empty `impl Type: Interface {}` was written for an interface the compiler cannot fill in. An empty impl derives the memberwise implementation for the five blessed interfaces (`Eq`, `Ord`, `Hash`, `Clone`, `ToText`) on a struct target, or asserts the `Send` / `Sync` markers; a user interface's methods must be provided, and deriving needs a struct (not an enum) target.

```cplus
interface Greet { fn hi(this) -> i32; }
struct S { x: i32 }
impl S: Greet {}
fn main() -> i32 { return 0; }
// -> [E0916] empty `impl` derives `Eq` / `Ord` / `Hash` / `Clone` / `ToText`
//            or asserts the `Send` / `Sync` markers; `Greet` requires a body
```

**Fix.** Implement the interface's methods, or — if you meant to derive — make the target a struct and the interface one of `Eq` / `Ord` / `Hash` / `Clone` / `ToText`.

<sub>repro: checked · cplus-core/src/sema.rs:validate_interface_impls · test cplus-core/src/sema.rs:empty_impl_on_regular_interface_rejected_e0916</sub>

## Modules, paths, and visibility

### E0388 · Extension method not in scope

A module other than the one declaring the type added this method, and the file making the call never imported that module. Extensions travel with their import: a file's method set is its types' own methods plus whatever the modules it imports added. Packages play no part — a sibling file is gated exactly like a vendored dependency. The method is real and there is exactly one of it in the program; it is simply not in scope here.

```cplus
# in ext/ext.cplus:  impl d::Point { fn sum(this) -> i32 { ... } }
# in this file — ext/ext never imported:
import "dep/dep" as d;
fn probe(p: d::Point) -> i32 { return p.sum(); }
```

**Fix.** Import the extending module in this file, or call a method declared with the type itself.

<sub>repro: scenario · cplus-core/src/sema.rs:err_ext_out_of_scope · test cplus-core/src/sema.rs:ext_cross_package_extension_hidden_without_import_e0388</sub>

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

A cross-file reference touched a function, type, field, method, const, static, type alias, or interface whose name begins with `_` (module-private) in its declaring file — or an `extern fn`, whose C+ name is module-private however it is spelled (only the linker symbol it binds is global).

```cplus
import "./math" as math;
fn main() -> i32 { return math::square(7); }
```

**Fix.** Remove the leading `_` from the name to make it public (or `export` it for the C ABI). For an `extern fn`, no rename exports it: declare the extern in the file that calls it, or wrap it in a plain `fn` the module exports. (Requires an imported module; `math.cplus` declares `fn _square` as private.)

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

### E0408 · Removed target section, or an app entry conflicting with `[library]`

The manifest uses a removed Cargo-shaped target section (`[[bin]]` / `[lib]`), or declares both an app `entry` and a `[library]` section. An app is named by its `entry` (per platform when needed); what a build produces is the target platform's fact, not a manifest section.

```toml
[package]
name = "legacy"

[[bin]]
name = "exe"
```

**Fix.** Delete `[[bin]]` (`src/main.cplus` is the default entry) or replace it with `entry = "..."`; move its `frameworks`/`libs` to `[link]`. Replace `[lib]` with a `[<platform>] entry` (external-builder app) or a `[library]` section with `kind = "staticlib"|"cdylib"|"both"`. A package is an application or a C-ABI library, never both.

<sub>repro: scenario · cplus-core/src/manifest.rs:parse · test cpc/tests/e2e.rs:legacy_bin_section_emits_e0408_migration</sub>

### E0409 · `fn main` in a build that produces a library archive

The build produces a library archive — a `[library]` target, an entry-less library package, or an app entry on an external-builder platform (ios, android) — and the program defines `fn main`, which nothing would ever call: the consumer (or the platform shell, through an `export extern fn`) owns the entry point.

```cplus
fn add(a: i32, b: i32) -> i32 { return a + b; }
fn main() -> i32 { return 0; }
```

**Fix.** Remove `fn main`. An external-builder platform's entry is an `export extern fn` the platform shell calls; a self-linked platform's entry (with `fn main`) belongs in a package/platform `entry`, not a library.

<sub>repro: scenario · cpc/src/main.rs:build_lib_project · test cpc/tests/e2e.rs:lib_target_rejects_fn_main_with_e0409</sub>

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

### E0412 · Unsupported `[library] kind` value

A `[library] kind` value is not one of the accepted kinds.

```toml
[package]
name = "mathlib"

[library]
kind = "rlib"
```

**Fix.** Use one of `staticlib` (the default), `cdylib`, or `both`.

<sub>repro: source · cplus-core/src/manifest.rs:parse · test cplus-core/src/manifest.rs:library_rejects_unknown_kind_e0412</sub>

### E0413 · No entry for the target platform

The package declares app entries, but none that applies to the platform being built: no `[<platform>] entry` for it and no package-level `entry`. A declared platform entry also suppresses the `src/main.cplus` default everywhere else — the scoping is taken as deliberate.

```toml
[package]
name = "gallery_ios"

[ios]
entry = "src/main.cplus"
# -> building on macOS: E0413 `gallery_ios` declares no entry for platform `macos`
```

**Fix.** Add `[<platform>] entry = "src/..."` for the platform, or a package-level `entry` that every platform without an override shares.

<sub>repro: scenario · cpc/src/main.rs:build_project · test cpc/tests/e2e.rs:target_ios_app_entry_builds_the_archive</sub>

### E0414 · Self-linked platform entry defines no `fn main`

The entry for a self-linked platform (macos, linux, windows) — where cpc links an executable — defines no `fn main`. Without this check the only symptom is the linker's `undefined symbol: _main`.

```cplus
export extern fn app_main() -> i32 { return 0; }
// -> building for the host: E0414 entry defines no `fn main`
```

**Fix.** Define `fn main() -> i32` in the entry's import tree. An `export extern fn` entry is the external-builder shape (ios, android), where the platform shell calls it.

<sub>repro: scenario · cpc/src/main.rs:build_project</sub>

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

### E1002 · Named arguments not supported yet

A call used a named argument (`f(name: value)`). The parser accepts the syntax, but the argument-matching pass that reorders named arguments into positional order and splices defaults is not implemented yet (see docs/compiler/design/named-params-and-defaults.md). This is a temporary guard so a labeled call is rejected cleanly rather than silently bound by position.

```cplus
fn add(n1: i32, n2: i32) -> i32 { return n1 +% n2; }
fn main() -> i32 {
    return add(v: 1);  // -> E1002 on a method/other call form
}
```

**Fix.** Pass the arguments positionally for now: `f(value)`.

<sub>repro: checked · cplus-core/src/sema.rs</sub>

### E1004 · Positional argument after a named argument

A positional argument followed a named one. Positional arguments must all come before any named argument so the call has a single readable shape.

```cplus
fn add(n1: i32, n2: i32) -> i32 { return n1 +% n2; }
fn main() -> i32 {
    return add(n1: 1, 2);  // -> E1004 positional argument after a named argument
}
```

**Fix.** Move the positional argument before the first named argument, or give it a label.

<sub>repro: checked · cplus-core/src/lower.rs</sub>

### E1005 · Unknown argument label

A named argument used a label that is not a parameter of the called function.

```cplus
fn add(n1: i32, n2: i32) -> i32 { return n1 +% n2; }
fn main() -> i32 {
    return add(bogus: 1, n2: 2);  // -> E1005 unknown argument label `bogus`
}
```

**Fix.** Use a parameter name from the function's signature.

<sub>repro: checked · cplus-core/src/lower.rs</sub>

### E1006 · Argument provided more than once

The same parameter was given a value more than once — by position and by label, or by two labels.

```cplus
fn add(n1: i32, n2: i32) -> i32 { return n1 +% n2; }
fn main() -> i32 {
    return add(n1: 1, n1: 2);  // -> E1006 argument `n1` is provided more than once
}
```

**Fix.** Provide each argument exactly once.

<sub>repro: checked · cplus-core/src/lower.rs</sub>

### E1007 · Required parameter after a defaulted one

A parameter without a default value follows one that has a default. Defaults must be trailing so a positional call can omit them unambiguously.

```cplus
fn f(a: i32 = 0, b: i32) -> i32 { return a +% b; }
//                ^ -> E1007 required parameter `b` cannot follow a defaulted one
```

**Fix.** Move the defaulted parameters to the end, or give the later ones defaults too.

<sub>repro: checked · cplus-core/src/lower.rs</sub>

### E1008 · Default value on an extern fn parameter

An `extern fn` parameter declared a default value. The C ABI has no notion of default arguments, and `extern fn` declarations are call-shapes for a foreign symbol.

```cplus
extern fn g(x: i32 = 0) -> i32;  // -> E1008 extern parameter cannot have a default
```

**Fix.** Remove the default; pass the argument explicitly at every call.

<sub>repro: checked · cplus-core/src/lower.rs</sub>

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

### E0362 · `#[watch]` hook has the wrong signature

A `#[watch]` struct's `on_value` is the write barrier the compiler calls on every field write, so its shape is fixed. It must be `fn on_value(ref this, field: str)` or the snapshot form `fn on_value(ref this, field: str, old: S, new: S)` for the struct's own type.

```cplus
#[watch] struct S { x: i32 }
impl S { fn on_value(ref this, field: i32) { return; } }
```

**Fix.** Give the hook one of the two accepted signatures. The snapshot form additionally requires the struct to be `Copy` (see E0361).

<sub>repro: checked · cplus-core/src/sema.rs:2745 · test cpc/tests/e2e.rs:watch_struct_without_valid_hook_is_rejected</sub>

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

### E0911 · `const`/`static` initializer shape not accepted

A `const` or `static` initializer used a shape outside the accepted set. A scalar-typed const or static takes a literal or any constant expression (folded by lower, see E0921); a non-scalar `const` is literal-only; a non-scalar `static` allows `#zero::[T]()`, array literals/fills, and non-generic struct literals of such.

```cplus
const C: [i32; 4] = [1, 2, 3, 4];
```

**Fix.** Use a literal or constant expression (or, for a non-scalar `static`, one of the aggregate shapes).

<sub>repro: checked · cplus-core/src/lower.rs:resolve_const_scalar · test cplus-core/src/sema.rs:array_literal_in_const_still_rejected_e0911_g043</sub>

### E0912 · Unknown `const` array length

An array length named a `const` that is not in scope, is not an integer, is negative, or exceeds the u32 maximum.

```cplus
fn main() -> i32 { let a: [i32; NOPE] = [0; 1]; return a[0]; }
```

**Fix.** Use an integer literal, or a `const` in scope with a non-negative integer literal initializer.

<sub>repro: checked · cplus-core/src/lower.rs:871 · test cpc/tests/e2e.rs:unknown_const_array_length_rejected_e0912</sub>

### E0918 · Compile-time include/shader path escapes the package

A `#include_bytes` / `#include_str` / `#compile_shader` path resolves outside the including file's package directory — an absolute path or a `..` chain that leaves the package. In project mode (a `Cplus.toml` is present) these compile-time file reads are contained to the package tree, the same boundary imports (E0914) and `[[bin]]`/`[lib]` paths (E0868) enforce, so untrusted source can't bake an arbitrary readable host file into the artifact.

```cplus
fn main() -> i32 {
    let _ = #include_bytes("/etc/passwd");
    return 0;
}
// -> [E0918] `#include_bytes` path `/etc/passwd` resolves outside the package directory
```

**Fix.** Move the asset inside the package and reference it with a package-relative path. A `..` that stays within the package (e.g. `../adapter/asset.bin` from `src/`) is allowed; only paths that leave the package are rejected.

<sub>repro: checked · cplus-core/src/sema.rs:include_path_escapes_package · test cplus-core/src/sema.rs:include_bytes_escaping_package_is_rejected_e0918</sub>

### E0921 · Invalid constant expression

A `const`/`static` initializer or an array-length expression failed compile-time evaluation: arithmetic overflowed the declared type's width, a shift amount was out of range, a division by zero occurred, two consts reference each other in a cycle, operand types mixed without a cast, or the expression used a non-constant construct (a call, a field, a runtime name).

```cplus
const A: u8 = 255u8 + 1u8;
fn main() -> i32 { return 0; }
// -> [E0921] constant arithmetic overflows `u8`; use `+%` to wrap
```

**Fix.** Constant evaluation is typed: match operand types with suffixes or `as` casts, and keep results inside the declared type. Overflow is an error by design — the wrapping spellings `+%` / `-%` / `*%` wrap, exactly as at runtime.

<sub>repro: checked · cplus-core/src/lower.rs:const_eval · test cplus-core/src/sema.rs:const_overflow_rejected_e0921</sub>

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

### E0860 · Declared `[link].bundled` file missing on host

A vendored package's manifest declares a file in `[link].bundled`, but `lib/<host-triple>/<basename>` does not exist. The manifest says the package ships that binary for this triple; the file is missing.

```toml
[link]
triples = ["arm64-apple-darwin"]
bundled = ["libfoo.a"]
# lib/arm64-apple-darwin/libfoo.a absent
# -> [E0860] package declares bundled `libfoo.a` but the file is not present
```

**Fix.** Add the missing file under `lib/<host-triple>/`, or remove its entry from `[link].bundled`.

<sub>repro: scenario · cpc/src/main.rs:link_bundled_missing · test cpc/tests/e2e.rs:bundled_lib_missing_for_host_triple_is_e0860</sub>

### E0861 · Orphan binary under `lib/` not declared in `[link].bundled`

A binary artifact (`.a`, `.o`, `.lib`) sits under a package's `lib/<triple>/`, but the package manifest does not declare it in `[link].bundled`. The manifest is the single source of truth for shipped binaries.

```toml
# vendor/foo/lib/arm64-apple-darwin/liborphan.a exists
# vendor/foo/Cplus.toml has no [link] section
# -> [E0861] package ships `liborphan.a` but the manifest doesn't declare it
```

**Fix.** Add the file to `[link].bundled`, or delete it from the package.

<sub>repro: scenario · cpc/src/main.rs:link_orphan_binary · test cpc/tests/e2e.rs (orphan .a under lib/<host> without [link])</sub>

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

### E0868 · `[lib]` / `[[bin]]` path escapes the package directory

A `[lib].path` or `[[bin]].path` key resolves outside the package directory — an absolute path or a `..` chain. Source targets must live inside the package tree; a hostile vendored manifest must not point compilation at arbitrary host files. `[link]` search paths and `${VAR}`-expanded extra objects are exempt (they legitimately name external SDK locations).

```toml
[package]
name = "esc"

[[bin]]
name = "esc"
path = "../../outside/main.cplus"
# -> [E0868] `[[bin]] `esc`` path resolves outside the package directory
```

**Fix.** Move the source file into the package and use a package-relative path (e.g. `path = "src/main.cplus"`).

<sub>repro: checked · cplus-core/src/manifest.rs:target_path_escapes · test cplus-core/src/manifest.rs:bin_path_escaping_package_is_rejected_e0868</sub>

### E0869 · Conflicting declarations of one dependency

One package name is declared in two places that could both apply — `[dependencies]` and a platform section, or two platform sections — with different specs. There is no conflict resolver by design: the compiler will not silently pick a winner.

```toml
[package]
name = "app"

[dependencies]
objc = "*"

[macos.dependencies]
objc = "*"
```

**Fix.** Declare the package once, or give every declaration the same spec. Platform sections are for packages that only apply to that platform.

<sub>repro: scenario · cplus-core/src/manifest.rs:459 · test cpc/tests/platform_deps.rs:conflicting_dependency_declarations_fail_with_e0869</sub>

### E0914 · Relative import escapes the project directory

A file-relative import (`./x` / `../x`) has a `..` chain that resolves to a file outside the importing package's tree — the same escape the vendor import path (E0859) blocks, on the relative path that previously left it open.

```cplus
import "../../../../etc/whatever" as e;
fn main() -> i32 { return 0; }
// -> [E0914] relative import resolves outside the project directory
```

**Fix.** Keep relative imports inside the package. To use another package, add it to `[dependencies]` and import it by name (`import "dep/module"`).

<sub>repro: scenario · cplus-core/src/resolver.rs:relative_import_escapes_root · test cplus-core/src/resolver.rs:relative_import_escaping_project_is_rejected_e0914</sub>

### E1900 · Construct is outside the wasm subset

The wasm backend accepts a deliberately small subset of C+ — scalar types, arithmetic, control flow and direct calls — and the program uses something outside it. It is the single code the backend emits for any unsupported construct.

```cplus
fn main() -> i32 { let s: str = "hello"; return 0; }
```

*Emitted only by the in-process wasm backend, which the browser playground drives. The native driver refuses the wasm32 target before reaching it.*

**Fix.** Keep wasm-targeted code inside the subset, or build for a native target instead.

<sub>repro: scenario · cplus-core/src/wasm_emit.rs:200 · test cpc/tests/wasm_differential.rs</sub>

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

### W0004 · `on_value` has the `#[watch]` hook signature but the struct is not `#[watch]`

`on_value` in the watch-hook shape is a compiler-invoked name: the only thing that calls it is the `#[watch]` write barrier. Without the attribute the method is unreachable, so every field write skips it silently. This is the fail-open half of E0361 — that error stops a `#[watch]` struct from having no hook; this warning stops a hook from having no `#[watch]`.

```cplus
struct Counter { n: i32 }
impl Counter {
    fn on_value(ref this, field: str) { return; }   // nothing ever calls this
}
// -> W0004 `Counter::on_value` has the `#[watch]` hook signature but
//    `struct Counter` is not `#[watch]`, so nothing calls it
```

**Fix.** Add `#[watch]` to the struct, or rename the method if it is not meant to be a write hook. Only the two accepted hook shapes are flagged, so an `on_value` with any other signature stays an ordinary method.

<sub>repro: checked · cplus-core/src/sema.rs:check_unwatched_watch_hook · test cplus-core/src/sema.rs:hook_shaped_on_value_without_watch_warns_w0004</sub>

### W0005 · Source file is not reachable from the entry

A file under `src/` compiles only when something reachable from the entry imports it. An unimported file is invisible: it compiles never, warns never, and reads as if it described the live API — a call to an undefined function in one still builds exit 0. For anyone reasoning from the source (a reviewer, an agent), unreachable code is false evidence. Platform-suffixed siblings (`runtime_linux.cplus` beside a loaded `runtime.cplus`) are the resolver's convention for `reachable on another target` and are exempt; the scan stays inside the package's own `src/`, since a vendored dependency legitimately ships more modules than one consumer imports.

```cplus
// src/dead.cplus — no reachable module imports it
fn never_compiled() -> i32 { return undefined_function(); }   // builds exit-0 today
// -> W0005 `src/dead.cplus` is not reachable from the entry — it never
//    compiles, and nothing it says is checked
```

**Fix.** Import the file from a reachable module, or delete it. If it is a platform variant, name it `<module>_<platform>.cplus` beside its base module so the resolver owns the choice.

<sub>repro: checked · cpc/src/main.rs:warn_orphan_sources · test cpc/tests/e2e.rs:orphan_source_file_warns_w0005_and_success_prints_module_count</sub>

### W0006 · Use of a `#[deprecated]` item

A call resolved to a function or method carrying `#[deprecated]`. The item still exists and still works; the attribute says it is on its way out, and the optional string is the author's migration note, printed verbatim. Reported at the USE, never at the declaration — a deprecated item is expected to still be defined and still be exercised by its own tests.

```cplus
#[deprecated("use parse_v2 instead")]
fn parse() -> i32 { return 1; }

fn main() -> i32 { return parse(); }
// -> W0006 `parse` is deprecated: use parse_v2 instead
```

**Fix.** Move to the replacement the note names. Nothing breaks until the item is actually removed, so a warning list can be worked through at leisure; that is the point of the attribute over a hard rename.

<sub>repro: checked · cplus-core/src/sema.rs:warn_if_deprecated · test cplus-core/src/sema.rs:deprecated_fn_and_method_uses_warn_w0006</sub>

### W0824 · Handler parameter cannot receive a bound method

A caller may pass `this.method` where a fn-pointer is expected, but only if the callee declares a `*u8` context parameter IMMEDIATELY after the handler — that is the slot the compiler fills with the receiver's address (E0824). Nothing in the declaration says so, so without this warning the author of a handler-taking function learns the rule from a CALLER hitting E0824 in another file, where it cannot be fixed. Only the wired-handler shape is flagged: a fn-pointer taking at least one real parameter plus a trailing `*u8`. A bare `fn(*u8)` is the release-hook shape and is left alone, as is a handler that already has an adjacent `*u8` (defaulted or not — an undefaulted one is a deliberate `the caller always supplies it` API).

```cplus
struct Row { n: i32 }
impl Row { fn clicked(ref this, sender: str) { this.n = this.n + 1; return; } }

// -> W0824: `on_click` cannot receive a bound method
fn bad(on_click: fn(str, *u8) = 0 as fn(str, *u8)) -> i32 { return 1; }

// the shape that accepts `bad(on_click: row.clicked)`
fn good(on_click: fn(str, *u8) = 0 as fn(str, *u8),
        on_click_ctx: *u8 = 0 as *u8) -> i32 { return 2; }
```

**Fix.** Add `<handler>_ctx: *u8 = 0 as *u8` immediately after the handler parameter. Leave it out only if callers are meant to pass free functions and thread the context themselves.

<sub>repro: checked · cplus-core/src/sema.rs:check_handler_ctx_slots · test cpc/tests/e2e.rs:handler_without_a_ctx_slot_warns_w0824</sub>

### W0825 · Handler takes its context FIRST, so it cannot receive a bound method

The mirror of W0824. A defaulted `*u8` sits right after the handler, so a wired handler is plainly what was meant — but the fn-pointer takes its `*u8` FIRST. A bound reference fills the slot after the handler with the receiver's address and the bridge reads it from the fn's LAST parameter, so a ctx-first handler can never receive a method however correct the slot beside it looks. W0824 does not see this shape, because it looks for a TRAILING `*u8`. Quiet on `fn(*u8, *u8)` (the ordinary sender-plus-ctx handler), on a bare `fn(*u8)` (the release-hook shape), and on a ctx-first fn with no context slot beside it (not a wired handler at all).

```cplus
struct Row { n: i32 }
impl Row { fn build(ref this, at: usize) -> i32 { return 1; } }

// -> W0825: `f` takes its context first
fn set_row(f: fn(*u8, usize) -> i32, ctx: *u8 = 0 as *u8) -> i32 { return 1; }

// the shape that accepts `set_row(row.build)`
fn good(f: fn(usize, *u8) -> i32, ctx: *u8 = 0 as *u8) -> i32 { return 2; }
```

**Fix.** Move the `*u8` to the end of the handler's parameter list: `fn(usize, *u8)` rather than `fn(*u8, usize)`. The context parameter beside it is already right; it is the fn type that is reversed.

<sub>repro: checked · cplus-core/src/sema.rs:check_handler_ctx_slots · test cpc/tests/e2e.rs:ctx_first_handler_warns_w0825</sub>

## Generics

### E0910 · Generic instantiation exceeds the recursion limit

A generic function calls itself (directly or through a cycle) with a type argument that grows on every step — `rec::[*T]`, `rec::[[T; 2]]`, `rec::[Box[T]]`. Each step is a distinct concrete type, so monomorphization never converges and the compiler would hang. Two limits catch this: a ceiling on the number of instantiations, and a ceiling on the size of any one synthesized type name. A wrapper that names its parameter more than once (`rec::[Pair[T, T]]`) doubles that name at every step, so it hits the size ceiling while the instantiation count is still small.

```cplus
fn rec[T]() -> i32 { let _z: i32 = rec::[*T](); return 0; }
fn main() -> i32 { return rec::[i32](); }
```

**Fix.** Reduce the type argument toward a non-generic base case, or drop the wrapper so the recursive call reuses the same type (`rec::[T]`). Runtime recursion on a value parameter is fine; only the *type* argument must not grow.

<sub>repro: checked · cplus-core/src/monomorphize.rs:check_instantiation_bounds, cplus-core/src/sema.rs:reject_oversized_instantiation · test cplus-core/src/monomorphize.rs:self_growing_generic_instantiation_reports_e0910, cplus-core/src/sema.rs:self_growing_struct_generic_reports_e0910_not_oom</sub>
