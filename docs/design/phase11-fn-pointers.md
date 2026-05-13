# Phase 11 — Function pointer types and values

**Status:** design only. Implementation pending user approval of this note.

**Motivation.** ObjC's `class_addMethod(class, sel, IMP imp, types)` takes a function pointer as the runtime method body. Without function pointer values in C+, the user's [hello_appkit_c.c](../../objc-c-interop/hello_appkit_c.c) reference can't be expressed — the `CAppDelegate` subclass can't be built, and the window-close-quits-app behavior is unreachable.

More generally, function pointers are how C libraries take callbacks: `signal(SIGTERM, handler)`, `qsort(..., comparator)`, `pthread_create(..., thread_fn, ...)`. Every nontrivial FFI consumer eventually needs them.

This is the third compiler gap blocking the Dart-style `cocoa-min` package. Items 1 (`#[link_name]`) and 2 (`0 as *T`) shipped 2026-05-13. This note specs item 3.

## Scope

What this note covers:
- **Function pointer types** at type position: parameter types, return types, struct fields, `let` annotations.
- **Function pointer values**: how to obtain a function pointer from a named C+ function.
- **Calling**: how to invoke a function through a pointer.
- **ABI**: which calling convention the pointer carries.
- **FFI interaction**: passing a C+ fn pointer to a C function expecting a callback.

What this note explicitly defers:
- **Closures / lambdas** — out of scope for C+ entirely per plan §2.8. Function pointers do not capture environment. If a user needs state-with-callback, the C-flavored pattern is `(fn_ptr, void* user_data)` — same as every C callback API.
- **Generic function pointer types** — e.g. `fn[T](T) -> T` as a type. Defer. Each instantiation of a generic fn produces its own monomorphized symbol; pointing at one of them works through the concrete-instantiation surface. A generic pointer type would imply runtime dispatch on the type argument, which conflicts with §2.8 (no dynamic dispatch in Phase 7).
- **Method pointers** — pointing at `Point::translate` or `p.translate`. Methods have a hidden `self` parameter and don't have a stable extern-style ABI; defer until a real use case appears.

## Surface syntax

### Type syntax

A function pointer type is written:

```
fn(T1, T2) -> R
```

The keyword `fn` followed by a parenthesized parameter type list and an optional `-> R` return type (defaulting to unit when absent). Examples:

```cplus
// A callback that takes two i32s and returns an i32.
let comparator: fn(i32, i32) -> i32 = ...;

// A callback that takes a string and returns nothing.
let handler: fn(str) = ...;

// As a struct field — the canonical "C-style callback with user data" pattern.
struct Listener {
    callback: fn(*u8, i32),
    user_data: *u8,
}

// As an extern fn parameter — the FFI callback case.
extern fn signal(sig: i32, handler: fn(i32)) -> fn(i32);
extern fn class_addMethod(cls: *u8, sel: *u8, imp: fn(*u8, *u8, *u8) -> i8, types: *u8) -> i8;
```

Disambiguation from regular `fn` declarations: declarations have a name immediately after `fn` (`fn name(...)`); types have an open-paren immediately after `fn` (`fn(...)`). The parser uses one-token lookahead.

Function pointer types are first-class types — they may appear anywhere a type may appear. They are `Copy` (a pointer is 8 bytes on our supported targets, atomic-Copy under the §2.9 model).

### Value syntax

A function pointer value is obtained by writing a named C+ function (or extern fn) in a position where a function pointer is expected. The coercion is **type-directed**:

```cplus
fn ascending(a: i32, b: i32) -> i32 { return a -% b; }

fn main() -> i32 {
    let cmp: fn(i32, i32) -> i32 = ascending;       // implicit coercion to fn pointer
    let result: i32 = cmp(3, 7);                    // -4
    return 0;
}
```

The bare identifier `ascending` is ambiguous in isolation — it could be a value (the function pointer) or a call referent. The expected type drives the choice:
- When the expected type is `fn(T1, ...) -> R` and the signature matches, the identifier resolves to a function pointer value.
- In a `Call { callee, args }` context, it resolves to the callee (existing behavior).

No new syntax. No `&fn`, no `addr_of(...)`, no `fn_ref!`. The type system disambiguates.

**Rejected alternatives:**
- **`&fn` operator** (Rust-style): `let cmp = &ascending;` — introduces the `&` token which C+ has explicitly avoided (§2.9 — no reference types, no address-of operator). Reusing `&` here for "function address" only would be a confusing one-off.
- **`fn_ref(name)` builtin**: explicit conversion call. Verbose at every call site, adds no information the type system doesn't already have.
- **`as fn(...)` cast**: works syntactically (`ascending as fn(i32, i32) -> i32`) but is redundant when the expected type is already known. Acceptable as a fallback when context is ambiguous (e.g. in a generic position or when storing in a tuple). Sema accepts but does not require it.

### Calling through a pointer

Identical to calling a named function:

```cplus
let cmp: fn(i32, i32) -> i32 = ascending;
let r: i32 = cmp(3, 7);
```

The parser already handles arbitrary `expr(args)` call shapes — no change. Sema sees a `Call` whose `callee` has type `fn(i32, i32) -> i32` and validates argument types against the pointer's parameter types.

## Semantics

### Coercion rules

The compiler implicitly coerces a named fn to a fn-pointer value when:
1. The expected type is `fn(P1, P2, ...) -> R`.
2. The named fn's signature has matching parameter types and return type.

Parameter ownership markers (`mut`, `move`) are **not** part of the function pointer type. Two fn-pointer types are equal iff their parameter types and return type are equal. Reasoning: a fn pointer's user knows nothing about the original declaration's parameter markers — at the C ABI level, those markers don't appear in the signature. (§2.9 model: `mut`/`move` are call-site contracts, not type-level facts.)

Generic fns cannot be coerced to fn pointers directly — they have no concrete signature until monomorphized. To get a pointer to a specific instantiation, use the turbofish form: `identity::[i32]` as `fn(i32) -> i32`. (Optional convenience; defer if not motivated.)

### Calling convention

Every fn pointer carries the **C calling convention (`ccc`)** at the LLVM level. This matches:
1. **Pointers from extern fns**: the symbol already uses `ccc`.
2. **Pointers to C+ fns**: today every non-generic C+ fn already lowers to `ccc` (we don't use `fastcc`). Future: if we ever introduce `extern "C+" fn`-style declarations with a different calling convention, only `ccc`-compatible fns can be addressed.

This means: any C+ fn that's eligible to be turned into a pointer must have a C-ABI-compatible signature. Today every fn meets this bar; no source-level annotation needed.

### Null pointers

A fn pointer is **non-null** at the type level. A user can construct a null fn pointer via `0 as fn(...)` inside `unsafe { }` — the same P3 escape hatch as raw-data pointer NULL. Outside `unsafe`, every fn pointer value comes from a named C+ fn or extern fn and is provably non-null.

For FFI: when a C library function takes a "possibly null callback" parameter (e.g. `signal(SIG, NULL)`), the C+ user writes `unsafe { signal(SIGTERM, 0 as fn(i32)) }` to pass NULL. This mirrors how data-pointer NULL is expressed (P3 from null design).

### Drop interaction

Function pointer values are `Copy`. They never trigger Drop. No special handling.

### Borrow checker interaction

Function pointer values pass through the borrow checker as Copy values. No `noalias` / `readonly` LLVM attributes are emitted on fn-pointer parameters — they don't point at data, just at code.

## LLVM lowering

A fn pointer type lowers to LLVM `ptr` (opaque pointer model). Already what we use for raw data pointers.

A coercion from a named fn to a fn pointer value lowers to the SSA value `ptr @<symbol>` — no instruction needed, the LLVM IR literal is its own SSA value. Concrete codegen pattern:

```llvm
; let cmp: fn(i32, i32) -> i32 = ascending;
; — no allocation needed for the pointer itself; the SSA value is just @ascending.
;   When stored: store ptr @ascending, ptr %cmp.addr

; let r: i32 = cmp(3, 7);
%cmp.val = load ptr, ptr %cmp.addr
%r = call i32 %cmp.val(i32 3, i32 7)
```

Calling through a pointer (indirect call) is straightforward in LLVM — `call <retty> <ptr>(<args>)` accepts an SSA pointer as the callee, not just a `@name`.

## Implementation surface

This is a real new language feature. Touch points:

### Lexer
- No new tokens. `fn` already exists; `(` already tokenized.

### AST
- New `TypeKind::FnPtr { params: Vec<Type>, return_type: Option<Box<Type>> }` variant on `Type`.
- New `ExprKind::FnRef(String)` variant — OR just reuse `ExprKind::Ident` and resolve based on expected type. **Lean: reuse `Ident`** — keeps the AST smaller; sema's existing expected-type machinery (the `expected: Option<Ty>` parameter on `check_expr`) is the natural place to do the coercion.

### Parser
- `parse_type` admits `fn(T1, T2, ...) -> R` after the existing primitive/array/path/raw-pointer cases. Lookahead: after consuming `fn`, peek for `(` (type position) vs ident (declaration position, but this site is parse_type so declaration form is already ruled out).
- No changes to expression parsing — calls already work for arbitrary callees.

### Sema
- New `Ty::FnPtr { params: Vec<Ty>, return_type: Box<Ty> }` variant.
- `resolve_type` handles `TypeKind::FnPtr` by recursing on each param + return type.
- `is_atomic_copy()` returns true for `FnPtr`.
- `name()` renders as `"fn(...) -> ..."` for diagnostics.
- `check_expr` for `Ident(name)`: when expected is `Some(Ty::FnPtr { .. })` and the name resolves to a fn whose signature matches, return the fn pointer type instead of triggering "unknown variable" or "fn used as value." When expected is None and the name is a fn, today's behavior (function call) should still apply — this only kicks in when expected pulls toward FnPtr.
- New error code: **E0820** — "function `X` has signature `fn(A) -> B`; cannot coerce to expected type `fn(C) -> D`" (signature mismatch).
- New error code: **E0821** — "cannot take address of generic function `X` without specifying type parameters" (until turbofish-on-fn-ref lands).
- Cast support: `cast_allowed` extends to admit `fn(...) -> R as fn(...) -> R'` only when signatures are equal (no implicit re-signing). Integer-to-FnPtr cast in `unsafe` (same gate as data-pointer null).

### Codegen
- New `Ty::FnPtr` lowers to LLVM `ptr`.
- New expression handling for `Ident(name)` coerced to FnPtr: emit `ptr @<symbol>` as the SSA value. No instruction needed — LLVM accepts the symbol literal as a pointer.
- `gen_call` already accepts arbitrary callee expressions; the existing `Ident` callee path needs a branch: when the resolved type is `FnPtr`, emit `call <retty> %fn_ptr_value(<args>)` instead of `call <retty> @<symbol>(<args>)`. Reuse the existing call-args codegen.

### Tests
- Sema: positive coercion, signature mismatch (E0820), null cast in unsafe, fn-ptr passed through a struct field, fn-ptr returned from a fn, fn-ptr called through a let-binding, fn-ptr as extern fn param type.
- Codegen: SSA shape pin (`ptr @<symbol>` literal), indirect-call shape (`call <ret> %v(...)`).
- E2E: real C library callback — pass a C+ fn to `signal(SIGTERM, handler)` and verify it runs. Plus the headline ObjC test: `class_addMethod` with a C+ fn as IMP, run the runtime-built class's method, observe the side effect.

### Effort estimate
- AST + parser: ~2 hr (new TypeKind, new Ty variant, one parser branch)
- Sema: ~3 hr (resolve_type, name() / is_atomic_copy, coercion in check_expr, E0820/E0821 emission, cast_allowed extension)
- Codegen: ~2 hr (lty for FnPtr, Ident-as-pointer-value, indirect call branch)
- Tests: ~2 hr (sema unit + codegen unit + e2e with signal())
- ObjC delegate end-to-end demo: ~1 hr (combine with `#[link_name]` and `class_addMethod`)
- **Total: ~1 day** focused work.

## Open questions

1. **Should `fn` in type position require `extern "C"` or similar qualifier for FFI use?** Today every C+ fn uses `ccc`, so the question is moot — but if a future slice introduces `fastcc`-using internal fns, we'd need a way to say "this fn ptr is for the C ABI." Defer; cross when motivated.
2. **Variadic fn pointers** — `fn(*u8, ...) -> i32` for `printf`. Defer; not needed for ObjC interop (msgSend isn't variadic at the C level; the C user picks a typed signature per call site, which is exactly what `#[link_name]` already gives us).
3. **Function pointer equality** — does `cmp1 == cmp2` work? Two pointers to the same function should compare equal. Lower to LLVM `icmp eq ptr`. Trivial to add. Defer until a real use case (typically only matters in callback de-registration APIs).
4. **Generic-fn instantiation as pointer** — `let id_i32: fn(i32) -> i32 = identity::[i32];`. The turbofish syntax is parseable today; sema needs to recognize the instantiation in expected-FnPtr position and emit the mangled symbol. Defer unless motivated.
5. **Method pointers** — `let translate: fn(*Point, i32, i32) = Point::translate;`. Methods have an implicit `self` parameter; modeling that requires either (a) exposing `self` as the first explicit parameter in the pointer type, or (b) introducing a separate `method_ptr` type. Defer; ObjC doesn't need this (delegates are written as bare C+ fns that take `*u8` receivers explicitly).

## Decision

Pending user approval. The shape locked in:
- Type: `fn(T1, T2, ...) -> R`
- Value: bare ident, type-directed coercion when expected is FnPtr
- Calling convention: ccc (C ABI), implicit
- Null: `0 as fn(...)` in unsafe (P3 escape hatch reused)
- Copy: yes; never triggers Drop
- ABI: passes by value as LLVM `ptr`
- No new tokens; one new `TypeKind` and one new `Ty` variant

Once approved, implement in slice **11.FN_PTR**, then close out the `cocoa-min` package as the proof point.
