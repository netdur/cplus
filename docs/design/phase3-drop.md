# Phase 3 — `Drop` (Destructors)

> Status: draft
> Scope: how a type declares a destructor; when destructors run; what `Drop` does to `Copy`-ness and move tracking; interaction with `defer`.
> Out of scope: panic-safe drop (we trap, we don't unwind); drop in async/await contexts (no async); generic `Drop` constraints (Phase 7); destructors that themselves return errors (orthogonal — see §8).

## 1. Problem

Three things need `Drop` before they're sound:

1. **A way to express non-`Copy` aggregates.** Right now every user-definable aggregate is `Copy` under auto-derive (slice 3C). Move semantics has no surface programs to act on — 6 dormant tests sit waiting for a non-Copy type to exist.
2. **Resource cleanup.** Real systems work needs RAII-style scope-bound cleanup: a file handle that closes, a lock that releases, a heap allocation that frees. Currently the only mechanism is `defer`, which puts cleanup logic at the use site instead of with the type. That's repetitive and easy to forget.
3. **Soundness for the eventual heap types.** When Phase 5+ adds `Vec`, `String`, `Box`, those types will allocate, and the language must guarantee deallocation runs exactly once when ownership ends.

This note picks the surface form and semantics, locks them in, and clears the path to (a) reviving the dormant slice-3A tests and (b) starting on the next move-semantics slice (implicit moves on assignment, partial moves).

## 2. Syntax

A destructor is declared as a method named `drop` on the type, inside an `impl` block, taking `mut self`:

```cp
struct Buffer { data: [i32; 4], used: i32 }

impl Buffer {
    fn new() -> Buffer { return Buffer { data: [0, 0, 0, 0], used: 0 }; }
    fn push(mut self, x: i32) {
        self.data[self.used as usize] = x;
        self.used = self.used +% 1;
    }
    fn drop(mut self) {
        // Cleanup runs here. For Buffer this is trivial — no heap, no
        // handle — but the existence of the method is enough to make
        // Buffer non-Copy and to register the drop hook with the scope.
        #println(self.used);
    }
}
```

Grammar: no change. `drop` is a normal method name; the compiler treats it as magic only in three places — Copy derivation (§4.2), move tracking (§4.3), and end-of-scope codegen (§5).

Receiver form: **`mut self`** — not `move self`, not `self`.

- `mut self`: gives the destructor exclusive access to the value's interior so it can release fields (close handles, free heap, etc.).
- Not `move self`: the destructor *is* the consumer; there's no further `move` to do. Saying `move self` would suggest "and then this body can pass self to another move-consuming function," which would be wrong (you'd run drop twice).
- Not plain `self`: shared/read-only access is too restrictive — most real destructors need to mutate.

The destructor returns nothing (`()` / no `->` clause). Returning a value from a destructor has no caller to receive it.

## 3. Decision — what makes a type `Drop`

A type is **`Drop`** iff its `impl` block defines a method named `drop` with signature `fn drop(mut self)`.

- One drop method per type, max. Defining two is E03XX duplicate (same as any duplicate method).
- The compiler checks the signature: receiver must be `mut self`, parameter list must be empty, return type must be `()`. Anything else is E03XX wrong-drop-signature.
- A type whose `impl` block does not define `drop` is **not Drop**, regardless of what its fields are. Drop does not propagate up structurally the way `Copy` does — wrapping a `Drop` field in a non-`Drop` struct gives a non-`Drop` struct, *but* the outer struct's destructor (if it had one) would be responsible for invoking inner destructors. (See §5 on field drop order.)

This is asymmetric with `Copy` (which auto-derives structurally) on purpose: destructors are user-written code, not derivable; the language can't guess what cleanup logic the user wants.

## 4. Semantics

### 4.1 Drop runs at scope exit

For every binding `x: T` where `T` is `Drop`, the compiler inserts a call to `T::drop(&mut x)` at every point where `x`'s lifetime ends — primarily the end of the lexical scope, but also at every `return` that exits through that scope.

Drop order **within a scope**: reverse order of declaration. The last `let` is the first to drop. This matches Rust, C++ stack unwinding, and what users expect.

```cp
fn f() {
    let a: T = ...;   // declared first
    let b: T = ...;   // declared second
    // scope ends → b.drop() runs, then a.drop()
}
```

### 4.2 `Drop` types are non-`Copy`

If a struct is `Drop`, the compiler forces `is_copy = false` regardless of field structure. Allowing a `Drop` type to be `Copy` would cause double-free — bitwise-copying a `Drop` value gives you two owners, both of which run drop. E03XX rejects:

- A type whose fields are all `Copy` but which defines `drop` → `is_copy` is forced to `false` (no error; `Drop` wins over auto-derive).
- An explicit attempt to mark a `Drop` type as `Copy` (when explicit Copy markers exist — currently they don't; tracked as future work).

This gives users the **mechanism they need to declare a non-Copy aggregate**: write an empty `fn drop(mut self) {}`. The compiler-inserted drop call is a no-op at runtime; the *only* effect is making the type non-Copy. Crude but functional. A cleaner `nocopy` marker remains a future option (deferred per copy-derivation note §8).

### 4.3 Drop interacts with moves

When a `Drop` binding is moved out (via `move`-marked parameter, `move self` receiver, or — once implemented — `let y = x` consumption), the source's scope-exit drop is **suppressed**. Drop runs at the destination instead.

Implementation: a **drop flag** — a hidden `bool` per `Drop` binding, set to `true` at initialization, cleared on move. At scope exit, drop is conditional on the flag being `true`. LLVM eliminates the flag when static analysis proves the binding is either always moved or never moved on every path; in mixed cases the flag stays.

```cp
fn maybe_take(c: bool, x: Buffer) {
    if c {
        consume(x);   // moves x; drop flag for x set to false
    }
    // scope ends: drop x iff flag is still true
}
```

In Phase 3 we have move tracking but only linear (no flow-sensitive merging). So:

- Unconditional move (`consume(x);` directly in scope body, no branch): static. Drop suppressed.
- Conditional move (inside `if`/`while`): drop flag generated. Runtime check.

The flag generation is unconditional in Phase 3 for any `Drop` binding that has any move on any path; flow-sensitive elision is Phase 5/6 work.

### 4.4 Interaction with `defer`

`defer` is a *separate*, *additive* scope-exit hook. Both `defer`'d statements and destructor calls run at scope exit, in **reverse registration order** — they share a single LIFO stack.

```cp
fn f() {
    let a: Buffer = Buffer::new();   // registers drop for a
    defer cleanup_x();                // registers defer #1
    let b: Buffer = Buffer::new();   // registers drop for b
    defer cleanup_y();                // registers defer #2
    // scope exit, in order:
    //   1. cleanup_y()
    //   2. b.drop()
    //   3. cleanup_x()
    //   4. a.drop()
}
```

The compiler emits a unified scope-exit handler that walks the registration stack in reverse. Mental model: every `let x: Drop_T = ...` is equivalent to immediately writing `defer x.drop();` after it (with the consumption-suppression behavior of §4.3 layered on top).

This makes `Drop` and `defer` interoperable without surprise. Users who want fine-grained control reach for `defer`; users who want type-bound cleanup write a `drop` method.

### 4.5 Drop and traps

If `+ - *` overflows (debug) or a divide-by-zero hits (any mode), `llvm.trap` aborts the process. **Destructors do not run on trap.** Same as our broader "no unwind, no panic handler" stance. This is consistent with C-like systems languages; users who need post-trap cleanup write it before the trapping operation.

### 4.6 Recursive drops are caller-protected, not compiler-checked

A destructor body can do anything legal C+. If it tries to take a `move` of `self` (which it can't — `self` is `mut self`, not `move self`) or call its own type's drop on a field (which would also be ill-formed because field types aren't typically the same), the compiler simply produces a normal type error. We don't add a special "recursive drop" check; the type system catches it.

A destructor that calls *another* function that calls *back* into the same destructor on the same value via aliasing — the borrow checker will reject in Phase 6 (since `mut self` is an exclusive borrow). For now (Phase 3) this is theoretically possible via tricks, but no language feature currently exposes the aliasing needed to trigger it.

## 5. Implementation

### 5.1 Sema

- New flag on `StructDef`: `pub is_drop: bool`. Populated during `collect_methods`: if a method named `drop` is registered, set `is_drop = true` and validate the signature (`mut self`, no params, `()` return). Diagnostic E03XX wrong-drop-signature for mismatches.
- `compute_struct_copy_flags` learns to skip Drop types: a Drop struct keeps `is_copy = false` regardless of fields. (One line of logic, in front of the existing all-fields-copy check.)
- Move tracking: no surface change. `cx.is_copy(&ty)` already returns false for Drop structs (via the above), so the existing `move` consumption path fires correctly. Slice-3A's 6 dormant tests will pass once their `struct B { x: i32 }` is upgraded to `struct B { x: i32 } impl B { fn drop(mut self) {} }`.

### 5.2 Codegen

- For every `Drop` binding, emit a stack-allocated `i1` drop flag (`%x.drop_flag = alloca i1`), initialized to `true` after the binding's initializer.
- On every move out of the binding, set the flag to `false` (`store i1 false, ptr %x.drop_flag`).
- At scope exit (end of block; before `ret`; before `br` to a parent scope's exit), emit a conditional branch:
  ```
  %f = load i1, ptr %x.drop_flag
  br i1 %f, label %drop_x, label %skip_x
  drop_x:
    call void @T.drop(ptr %x)
    br label %skip_x
  skip_x:
  ```
- Multiple Drop bindings in the same scope → reverse-order chain of the above blocks.
- `defer` statements interleave into the same chain by registration order (reverse on emit).

### 5.3 Drop method dispatch

`T::drop(mut self)` is a regular instance method. The mangled name is `@T.drop`. At call sites — both compiler-inserted scope-exit calls and any user code that happens to call `x.drop()` directly — the same codegen path is used.

**Should users be allowed to call `x.drop()` directly?** Lean **yes**, with no special restriction. Calling drop directly is unusual but legal; the result is that the binding's destructor runs early. The drop flag is set to `false` immediately after the call site so the scope-exit handler doesn't double-drop. This is the same machinery Rust's `std::mem::drop` (a one-liner generic function) uses.

A `let x: Drop_T = ...; x.drop(); #println(x);` — the explicit call moves the drop flag to false, but x's bits are still readable. Reading after explicit drop is fine for plain `Copy`-like field reads. Calling another method that would re-invoke drop is *not* fine and is caught by the regular move-tracking machinery once we treat `x.drop()` as a `move self` consumption (which is what it effectively is, at the language level). So in implementation: model an explicit `x.drop()` as a `move self` call. Sema and codegen both already handle that.

### 5.4 New error codes

| Code | Meaning |
|---|---|
| E0338 | `drop` method has wrong signature (must be `fn drop(mut self)` with no extra params and no return type) |
| E0339 | `drop` method declared on a non-struct type (placeholder; only structs can be Drop in Phase 3) |

`E0336` (lint: `move` on Copy-typed parameter is redundant) is still deferred. With Drop landing, more aggregate types become non-Copy, but the lint's scope is unchanged — it targets the obvious-redundancy case, not the design.

## 6. Sample programs

### 6.1 Must compile and run

`docs/examples/drop_basic.cplus`:

```cp
struct Tracker { id: i32 }

impl Tracker {
    fn new(id: i32) -> Tracker {
        #println(id);
        return Tracker { id: id };
    }
    fn drop(mut self) {
        // Negate so we can tell construction from destruction in the output.
        #println(0 -% self.id);
    }
}

fn main() -> i32 {
    let a: Tracker = Tracker::new(1);
    let b: Tracker = Tracker::new(2);
    return 0;
    // scope exit: b drops (prints -2), then a drops (prints -1)
}
```

Expected output: `1\n2\n-2\n-1\n`.

### 6.2 Must compile and run (revives slice-3A move tracking)

`docs/examples/drop_move.cplus`:

```cp
struct Handle { id: i32 }

impl Handle {
    fn new(id: i32) -> Handle { return Handle { id: id }; }
    fn drop(mut self) { #println(0 -% self.id); }
}

fn take(move h: Handle) -> i32 {
    let id: i32 = h.id;
    return id;
    // h drops at end of take's scope (prints -id).
}

fn main() -> i32 {
    let h: Handle = Handle::new(7);
    let id: i32 = take(h);  // h is moved; main's scope-exit drop is suppressed.
    #println(id);
    return 0;
}
```

Expected output: `-7\n7\n`.

### 6.3 Must reject

| Program | Error |
|---|---|
| `impl T { fn drop(self) {} }` (wrong receiver) | E0338 |
| `impl T { fn drop(mut self, x: i32) {} }` (extra param) | E0338 |
| `impl T { fn drop(mut self) -> i32 { return 0; } }` (return type) | E0338 |
| `impl T { fn drop(mut self) {} fn drop(mut self) {} }` (duplicate) | E0326 |

### 6.4 Slice-3A dormant tests revive

The 6 sema tests and 2 e2e tests `#[ignore]`d in slice 3C are revived by upgrading their `struct B { x: i32 }` to:

```cp
struct B { x: i32 }
impl B { fn drop(mut self) {} }
```

The empty drop method is enough to make B non-Copy, restore move-consumption behavior, and re-fire E0335 / E0337.

## 7. Interactions

### 7.1 Phase 3 slice 3A (move tracking)

Drop unlocks the dormant tests. No code change to move tracking itself.

### 7.2 Phase 3 slice 3C (Copy auto-derive)

`compute_struct_copy_flags` gains one extra short-circuit: if a struct is Drop, skip the all-fields-copy check. (`is_copy = false` if `is_drop`.) One line.

### 7.3 `defer` (future slice)

§4.4 specifies their interaction. The implementation must unify their registration into a single scope-exit emit pass — easier if Drop lands first (concrete scope-exit machinery exists) and `defer` extends it.

### 7.4 Phase 5/6 borrow checker

`drop(mut self)` is an exclusive borrow at the type level. When the borrow checker is real (Phase 6), the destructor body gets the standard exclusive-access rules. No special-casing needed.

### 7.5 Phase 7 traits

In Phase 7, `Drop` becomes a real trait (`impl Drop for T { fn drop(mut self) { ... } }`) rather than a magic method name. The migration is purely syntactic — semantics stay identical. Either:

- Keep magic-method form forever and never trait-ify (cleanest, smallest spec).
- Or convert at trait-introduction time (more uniform with Phase 7's other traits).

Lean: defer the decision. The magic-method form works fine; the trait form is a refactor that costs little when we know what we're doing.

## 8. Open questions

- [ ] **Drop method visibility.** Should `drop` be implicitly private (only callable by the compiler-inserted scope-exit handler), or implicitly public (users can call `x.drop()` directly)? §5.3 leans public; revisit if users misuse it.
- [ ] **Destructor cannot fail.** Phase 3 has no error unions yet. When error unions land, should `drop` be allowed to return `!()`? Lean: no. Rust solved this with abort-on-failed-drop; we follow. Same reason we don't unwind on trap.
- [ ] **Explicit `nocopy` marker** — still deferred (copy-derivation note §8). With Drop in place, the "I want a non-Copy type for type-system reasons, but no actual cleanup logic" case is satisfied by writing an empty `fn drop(mut self) {}`. Ugly but functional. The `nocopy` keyword stays a future polish item.
- [ ] **Drop for enums and arrays.** Phase 3 only allows `impl` blocks on structs. Tagged unions (later) will need Drop. Arrays-of-Drop need element-wise drop. Both deferred to their own slices.
- [ ] **Drop ordering across `mem::swap`-like operations.** Once we have something like `swap(mut a, mut b)`, the drop-flag bookkeeping needs to track that the *bits* of a and b swapped but the *bindings* didn't move. Probably solved by the borrow checker in Phase 6.
- [ ] **`Drop` and `move self`.** §2 says the destructor takes `mut self`, not `move self`. But a `move self` method also has full ownership at the LLVM level. Should a `move self` method that ends its body need to manually invoke drop on `self`? Lean **no** — compiler-inserted drop at end of the `move self` method's scope, same as any other Drop binding, suppressed by the flag if the body moved self elsewhere. Confirm at implementation time.
