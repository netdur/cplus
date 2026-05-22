# The C+ Tutorial

A complete walkthrough of C+ — every language feature and every stdlib module — with runnable examples. C+ is a systems language with a Rust-style ownership model, a C ABI for FFI, and a syntax engineered so an LLM can write correct code on the first try.

If you want history and rationale, read [plan.md](plan.md). If you want a tight reference, read [SKILL.md](SKILL.md). This document is the friendly path through both.

---

## Table of contents

1. [What C+ is — and what it isn't](#1-what-c-is--and-what-it-isnt)
2. [Hello, world](#2-hello-world)
3. [Primitives and literals](#3-primitives-and-literals)
4. [Variables, mutability, and scope](#4-variables-mutability-and-scope)
5. [Operators and arithmetic](#5-operators-and-arithmetic)
6. [Control flow](#6-control-flow)
7. [Functions](#7-functions)
8. [Structs and methods](#8-structs-and-methods)
9. [Enums — plain and tagged](#9-enums--plain-and-tagged)
10. [Pattern matching](#10-pattern-matching)
11. [Arrays](#11-arrays)
12. [Ownership — the heart of C+](#12-ownership--the-heart-of-c)
13. [Drop and `defer`](#13-drop-and-defer)
14. [The borrow checker](#14-the-borrow-checker)
15. [Generics](#15-generics)
16. [Error handling without exceptions](#16-error-handling-without-exceptions)
17. [Strings — `str` vs `string`](#17-strings--str-vs-string)
18. [String interpolation](#18-string-interpolation)
19. [FFI — calling C](#19-ffi--calling-c)
20. [Function pointers](#20-function-pointers)
21. [Compile-time intrinsics](#21-compile-time-intrinsics)
22. [Modules, imports, packages](#22-modules-imports-packages)
23. [Attributes](#23-attributes)
24. [Threads and atomics](#24-threads-and-atomics)
25. [Async / await](#25-async--await)
26. [Iterators and `gen fn`](#26-iterators-and-gen-fn)
27. [Standard library tour](#27-standard-library-tour)
28. [Tooling — `cpc`](#28-tooling--cpc)
29. [Common error codes](#29-common-error-codes)
30. [Gotchas worth memorising](#30-gotchas-worth-memorising)
31. [SIMD types](#31-simd-types)
32. [Where to go next](#32-where-to-go-next)

---

## 1. What C+ is — and what it isn't

C+ compiles to native code through LLVM. It has no GC, no exceptions, no closures, no overloading, no implicit conversions, and no `null`. It has manual memory management policed by a Rust-style borrow checker, plus one-way C ABI compatibility for FFI.

The point: every program is **locally legible**. You can read one function and know exactly what it does without chasing implicit destructors, hidden conversions, or surprise allocations. The compiler enforces that property — if you slip, the diagnostic tells you precisely where and why.

### Locked principles

Memorise these. They are the spine of every C+ design decision.

| # | Principle | Why |
|---|---|---|
| 1 | No `null` anywhere | Use `Option[T]`. Avoids the billion-dollar mistake. |
| 2 | No closures or lambdas | Use named `fn` + `(fn_ptr, user_data)`. Eliminates capture semantics. |
| 3 | No `&T` / `&mut T` reference types | Borrowing is a parameter marker, not a type. |
| 4 | No exceptions | Errors are tagged-union values matched exhaustively. |
| 5 | No implicit conversions | Every width change requires `as`. |
| 6 | No operator or function overloading | One name, one signature. |
| 7 | No macros, decorators, or comptime | Attributes are pure metadata. |
| 8 | No `class`, `function`, or `var` | Use `struct` + `impl`, `fn`, `let`. |
| 9 | No mutable-by-default | `mut` is opt-in. |
| 10 | Generics use `[T]`, not `<T>` | Avoids `a<b>(c)` ambiguity. |
| 11 | Explicit `return` | No implicit tail returns at the function level. |
| 12 | `::` for types, `.` for instances | Strict separation. |

Reopening any of these is a non-starter. Build around them; they hold.

---

## 2. Hello, world

C+ has two modes: **single-file** (one file, intrinsic helpers) and **project** (a `Cplus.toml` manifest with imports).

### Single-file

```cplus
fn main() -> i32 {
    println("hello, world");
    return 0;
}
```

Build and run:

```bash
cpc hello.cplus -o hello
./hello
```

`println` is a built-in **intrinsic** in single-file mode — there is no import. It accepts `i32` or `str`.

### Project mode

```text
hello/
├── Cplus.toml
├── vendor/stdlib -> /Users/adel/Workspace/C+/vendor/stdlib   (symlink)
└── src/main.cplus
```

`Cplus.toml`:

```toml
[package]
name    = "hello"
version = "0.0.1"
edition = "2026"

[[bin]]
name = "hello"
path = "src/main.cplus"

[dependencies]
stdlib = "*"
```

`src/main.cplus`:

```cplus
import "stdlib/io" as io;

fn main() -> i32 {
    io::println("hello from a project");
    return 0;
}
```

Build:

```bash
cpc build
./target/debug/hello
```

Pick **one** mode per program. Don't mix intrinsic `println` with `io::println` in the same project.

---

## 3. Primitives and literals

### Integer and float types

| Category | Types |
|---|---|
| Signed | `i8 i16 i32 i64 isize` |
| Unsigned | `u8 u16 u32 u64 usize` |
| Float | `f32 f64` |

No `int`, no `long`, no `byte`. The size is part of the name.

### Other primitives

- `bool` — `true` / `false`. **Cannot** be produced by an integer cast.
- `()` — the unit type, the implicit return of functions without an arrow.
- `str` — a string view: pointer + length, borrowed.
- `string` — an owned, heap-allocated string (provided by the stdlib).
- `*T` — a raw pointer. Operations require `unsafe`.
- `fn(...) -> R` — a function pointer.

### Literals

```cplus
let a: i32 = 42;
let b: u64 = 42u64;            // typed literal
let c: f64 = 3.14;
let d: bool = true;
let e: str = "hello";
let f: i32 = 0x1F;             // hex
let g: i32 = 0b1010;           // binary
let h: i32 = 1_000_000;        // underscore separators
```

There is no `'a'` character literal in v0.0.4. Use `65u8` for ASCII bytes.

---

## 4. Variables, mutability, and scope

```cplus
let x: i32 = 5;             // immutable
let y = 5;                  // inferred as i32
let mut z: i32 = 0;         // mutable
z = 7;

let w: i32;                 // uninitialised
w = 12;                     // first write counts as init; subsequent writes need `mut`
```

If you forget to initialise on a path, the compiler tells you (**E0345**). If you reassign without `mut`, the compiler tells you. Shadowing is allowed: a new `let` with the same name introduces a new binding.

Scope is curly-brace lexical. A binding lives until its enclosing block exits, at which point its `Drop` runs (see §13).

### Module-scope `const` and `static`

`let` lives inside a function. For named values shared across functions — or for the C-style "static storage" pattern where the value lives for the whole program's lifetime — use `const` or `static` at module scope.

```cplus
// `const` — a typed alias for a literal. No storage, no address.
// Every use site is rewritten to the literal at compile time, like a
// C `#define` but type-checked.
const HEADER_BYTES: usize = 176;
const PI: f32 = 3.14159f32;
const VERSION: str = "0.0.9";

// `static` — a global with a real address. Lives for the whole program;
// initialised once before `main` runs. Immutable form lives in `.rodata`.
static IMMUTABLE_OFFSET: i32 = 50;

// `static mut` — mutable global. Reads and writes require an enclosing
// `unsafe { ... }` block, since the borrow checker can't prove absence
// of data races on module-scope mutable state.
static mut COUNTER: i32 = 0;

fn bump(by: i32) {
    unsafe { COUNTER = COUNTER + by; }
    return;
}

fn current() -> i32 {
    return unsafe { COUNTER };
}
```

Three rules:

1. **Initialiser must be a literal** — integer, float, bool, string, or a unary-negated numeric literal. Arithmetic (`const N: i32 = 1 + 2;`) is rejected with **E0X30**. Referring to another const or binding from the initialiser is the same error.
2. **Type annotation is required** — no inference. `const FOO = 5;` and `static FOO = 5;` are rejected with **E0X31**.
3. **`static mut` reads need `unsafe`** (E0X33). Writes need `unsafe` (E0X34). Writing to an immutable `static` is **E0305** ("cannot assign to immutable static").

The choice between `const` and `static`:

| You want | Use |
|---|---|
| A named literal you'll reference at multiple sites | `const` |
| A module-private *fixed offset table* the program reads at runtime | `static` |
| A *mutable* counter / RNG state / lazy cache | `static mut` |

The C `static const sphere_t scene[10] = {...}` pattern is `static SCENE: ...` in C+ (once struct/array initialisers are admitted in a follow-up slice — v0.0.9 ships literal-only). The C `static uint32_t rng_state` pattern is `static mut RNG_STATE: u32 = ...;` today.

---

## 5. Operators and arithmetic

### Default arithmetic — overflow-checked in debug, wraps in release

```cplus
let a: i32 = 10 + 20;
let b: i32 = 10 * 30;
let c: i32 = 10 / 3;        // 3
let d: i32 = 10 % 3;        // 1
let e: i32 = 10 - 20;       // -10
```

Division by zero **always** traps, in both modes.

### Wrapping operators — always wrap

When you genuinely want wrap-on-overflow, use the `%`-suffixed family:

```cplus
let a: u8 = 250u8 +% 10u8;  // 4
let b: i8 = 100i8 *% 3i8;   // overflows, wraps silently
let c: i32 = 0 -% 1;        // -1, the canonical "this can underflow" idiom
```

These exist because *unchecked* overflow is a footgun and you should opt into it explicitly.

### Bitwise and shifts

```cplus
let h: i32 = 0xff & 0x0f;   // 15
let i: i32 = 0xf0 | 0x0f;   // 255
let j: i32 = 0xff ^ 0xaa;   // 85
let k: i32 = 1 << 8;        // 256
let l: i32 = 256 >> 2;      // 64
let m: u32 = ~(0 as u32);   // 0xffffffff
```

Right shift on signed types is arithmetic (sign-preserving); on unsigned, logical (zero-fill).

### Byte-swap intrinsics

```cplus
let port_be: u16 = htons(8080 as u16);
let n: u32      = bswap32(0x12345678 as u32);
```

`htons` / `htonl` always convert host order to network order. `bswap16` / `bswap32` / `bswap64` are unconditional swaps.

### Comparisons

```cplus
let lt: bool = a < b;
let eq: bool = a == b;      // no coercion
let ne: bool = a != b;
```

### Casts — every width change is explicit

```cplus
let x: i64 = 5;
let y: i32 = x as i32;
let f: f64 = (x as f64);
let z: usize = 10 as usize;
```

No `int → bool`. No silent narrowing. Pointer ↔ integer must go through `usize`, never `i32`.

---

## 6. Control flow

### `if` — statement and expression

```cplus
if cond { println("yes"); } else if other { println("no"); } else { println("?"); }

let r: i32 = if cond { 1 } else { 2 };
```

The condition must be `bool`. `if 1 { ... }` is a type error.

### `while`

```cplus
let mut x: i32 = 0;
while x < 10 { x = x +% 1; }
```

### `for` — three flavours

```cplus
// Range. 0..n is exclusive; 0..=n is inclusive.
for i in 0..10 { println(i); }

// Array.
let arr: [i32; 4] = [10, 20, 30, 40];
for v in arr { println(v); }

// C-style. Standard `for` with init / cond / step.
for (let mut i: i32 = 0; i < 10; i = i +% 1) {
    println(i);
}
```

### `loop` / `break` / `continue`

```cplus
let mut n: i32 = 0;
loop {
    if n == 5 { break; }
    if n % 2 == 0 { n = n +% 1; continue; }
    println(n);
    n = n +% 1;
}
```

### `while let`

```cplus
while let Option[i32]::Some(v) = next() {
    println(v);
}
```

Match-binds each iteration; exits when the pattern fails.

---

## 7. Functions

```cplus
fn add(x: i32, y: i32) -> i32 {
    return x +% y;
}

// No return type = unit `()`.
fn shout(msg: str) {
    println(msg);
}

// `pub` for cross-file visibility (default is module-private).
pub fn answer() -> i32 { return 42; }
```

Every function body **must** end with `return EXPR;` — there is no implicit tail return at the function level (the rule is E0333). Block expressions can still be tail expressions inside `return` and `let`:

```cplus
fn classify(n: i32) -> i32 {
    return if n < 0 { -1 } else if n == 0 { 0 } else { 1 };
}
```

Generics use square brackets (see §15):

```cplus
fn identity[T](x: T) -> T { return x; }
fn max[T: Ord](a: T, b: T) -> T { ... }
```

There is **no function overloading**. A name has one signature, period.

---

## 8. Structs and methods

```cplus
struct Point {
    x: i32,
    y: i32,
}

impl Point {
    // Associated function — no receiver. Called via `Point::new(...)`.
    fn new(x: i32, y: i32) -> Point {
        return Point { x: x, y: y };
    }

    // Instance method — receiver is `self`. Called via `p.translate(...)`.
    fn translate(mut self, dx: i32, dy: i32) {
        self.x = self.x +% dx;
        self.y = self.y +% dy;
    }

    fn magnitude_squared(self) -> i32 {
        return self.x *% self.x +% self.y *% self.y;
    }
}

fn main() -> i32 {
    let mut p: Point = Point::new(1, 2);
    p.translate(3, 4);
    return p.magnitude_squared();
}
```

### Struct-literal shorthand

```cplus
let x: i32 = 1;
let y: i32 = 2;
let p: Point = Point { x, y };          // shorthand when names match
```

### Field visibility

```cplus
struct Public {
    pub value: i32,                     // visible to other modules
    internal: i32,                      // module-private
}
```

### Three receiver forms — preview

```cplus
impl Buf {
    fn read(self) { ... }            // shared / by-value-on-Copy
    fn write(mut self) { ... }       // exclusive / mutable
    fn into_raw(move self) -> *u8 { ... }   // consumes self
}
```

Details in §12.

---

## 9. Enums — plain and tagged

### Plain enums (C-like)

```cplus
enum Color { Red, Green, Blue }

let c = Color::Red;
```

Plain enums lower to `i32` and are `Copy`.

### Tagged enums (sum types)

```cplus
enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Square(f64),
}

let s = Shape::Circle(3.14);
```

### Generic enums

```cplus
enum Maybe[T] {
    Some(T),
    None,
}

let m: Maybe[i32] = Maybe[i32]::Some(7);
let n: Maybe[i32] = Maybe[i32]::None;
```

**Always write the type args at the source level** — `Option[i32]::Some(v)`, `Option[i32]::None`. Internal mangled names like `Option__i32` exist but are never user-typeable.

---

## 10. Pattern matching

`match` is the workhorse. It is **exhaustive** — missing a variant is a compile error (**E0340**).

```cplus
fn describe(s: Shape) -> i32 {
    return match s {
        Shape::Circle(r)         => (r as i32) *% 2,
        Shape::Rectangle(w, h)   => (w as i32) *% (h as i32),
        Shape::Square(side)      => (side as i32),
    };
}
```

Add a catch-all when you genuinely don't care about the rest:

```cplus
return match c {
    Color::Red => 1,
    _          => 0,
};
```

### `if let` — extract on the happy path

```cplus
if let Maybe[i32]::Some(v) = m {
    println(v);
}
```

### `guard let` — pattern match or diverge

The most useful sugar in C+. Lets you write tight, linear code without nested `match`.

```cplus
fn process(m: Maybe[i32]) -> i32 {
    guard let Maybe[i32]::Some(v) = m else { return 0 -% 1; };
    return v +% 1;          // `v` is in scope after the guard
}
```

The `else` block must **diverge** — `return`, `break`, `continue`, or `loop`. The compiler enforces that.

### `while let`

```cplus
while let Maybe[i32]::Some(v) = next() {
    println(v);
}
```

---

## 11. Arrays

Fixed-size, stack-allocated, bounds-checked.

```cplus
let a: [i32; 4] = [10, 20, 30, 40];
let x: i32 = a[2];               // 30; out-of-range traps

let mut buf: [i32; 4] = [0, 0, 0, 0];
buf[0] = 5;

for v in a { println(v); }
```

**Use small `[u8; N]` arrays for scratch buffers in hot loops** — they live on the stack (or in registers after SROA). `malloc` is real heap allocation; it dominates tight loops.

```cplus
fn make_key(buf: *u8, n: u32) -> u32 {
    let mut tmp: [u8; 10] = [0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8];
    // fill tmp ...
    return 0;
}
```

**Known parser gap:** the repeat syntax `[0u8; 10]` doesn't parse yet. List elements explicitly.

Slices `T[]` exist as a borrow-shaped construct used at FFI boundaries and inside the stdlib.

---

## 12. Ownership — the heart of C+

This is the section that makes C+ feel unfamiliar at first, then second nature. Read it twice.

There is **no `&T` and no `&mut T`**. Borrowing is expressed by *parameter markers*, not by reference types.

### The three parameter forms

| Form | On non-Copy types | On Copy types |
|---|---|---|
| `x: T` | Shared borrow — caller keeps ownership, function reads only | Pass-by-value copy |
| `mut x: T` | Exclusive borrow — function may mutate; mutations propagate back | Pass-by-value, locally mutable |
| `move x: T` | Ownership transfer — caller can't use the value after the call | Silent bit-copy (`move` is a no-op marker) |

Method receivers follow the same model: `self`, `mut self`, `move self`.

### `Copy` is structural

A type is `Copy` if every component is. Primitives and plain enums are `Copy`. A struct of `Copy` fields is `Copy` automatically. A struct that defines `fn drop(mut self)` is forced to be **non-Copy** (you can't silently bit-copy a thing that owns a resource).

```cplus
struct Point { x: i32, y: i32 }            // Copy (all fields Copy)

struct Buf { ptr: *u8, len: usize }
impl Buf {
    fn drop(mut self) { unsafe { free(self.ptr); } }   // forces non-Copy
}
```

### Return values always move

```cplus
fn make_buf() -> Buf { ... }    // no marker; returning is always a move
```

### `restrict` — opt-in `noalias` for raw pointer params

v0.0.8 addition. The borrow checker doesn't reason about `*T` raw pointers, so cpc emits just `noundef` on a raw-pointer param — LLVM has to assume any two pointer args may alias. For numeric hot paths (gemm, axpy, image / audio loops) that's a real perf tax: the autovectorizer inserts a runtime alias check + scalar fallback.

`restrict` is a parameter prefix marker, alongside `mut` / `move`. It asserts that the pointer does not alias any other pointer reachable in the function body during this call. Lowers to LLVM `noalias` at both the function definition and at every call site.

```cplus
fn axpy(n: usize, a: f32, restrict x: *f32, restrict y: *f32) {
    let mut i: usize = 0 as usize;
    while i < n {
        unsafe { y[i] = a * x[i] + y[i]; }
        i = i +% (1 as usize);
    }
    return;
}
```

Hot-loop size (instructions) on an axpy kernel: **21 with `restrict` vs 36 without** — the savings are LLVM dropping the runtime alias check + scalar fallback.

Rules:
- Only valid on `*T` (raw pointer) params. Other shapes (`x: T` borrows, value-typed params) fire **E0411**.
- No `unsafe` required at the declaration site — `restrict` is a contract about the body, not a use-site assertion. Violations manifest as UB through the existing `unsafe` requirement on pointer ops.
- Composes with `mut` (e.g. `restrict mut p: *f32` — caller may write through `p`, and `p` doesn't alias anything else). Each marker is orthogonal.
- C ABI compatible: LLVM `noalias` is an optimization hint, not part of the calling convention. A `pub extern fn` with `restrict` params exports the same C signature as without — C callers see plain pointers.

### Call sites carry **no** markers

```cplus
write_it(buf);    // could be borrow, mut borrow, or move — the signature decides
```

The reader looks at the *function signature*, not the call site, to understand the data flow. That's the whole point of the model.

### Worked example

```cplus
struct Counter { value: i32 }
impl Counter {
    fn new() -> Counter { return Counter { value: 0 }; }
    fn drop(mut self) { /* hypothetical resource cleanup */ }
    fn read(self) -> i32 { return self.value; }
    fn inc(mut self)     { self.value = self.value +% 1; }
    fn into_value(move self) -> i32 { return self.value; }
}

fn main() -> i32 {
    let mut c: Counter = Counter::new();
    c.inc();                    // exclusive borrow
    c.inc();
    let v: i32 = c.read();      // shared borrow
    let total: i32 = c.into_value();   // consumes c; can't use c after
    return v +% total;
}
```

### When to use `move`

For **non-Copy value parameters**, you almost always need `move`:

```cplus
// ❌ Footgun — caller's `s` and callee's `x` both run Drop. Double-free.
fn echo(x: string) -> string { return x; }

// ✅ Marks `s` as moved at the call site; only the result drops.
fn echo(move x: string) -> string { return x; }
```

For `Copy` types, `move` is harmless; for non-`Copy`, it's load-bearing.

### Lifetime annotations (rare)

Most cases elide. When the compiler genuinely can't infer relations between borrows, name a region:

```cplus
fn longest(a: borrow A string, b: borrow A string) -> borrow A string {
    if a.len() > b.len() { return a; }
    return b;
}
```

`A` is a region name local to one signature; there's no separate declaration block. You will rarely write these.

---

## 13. Drop and `defer`

### Drop — your destructor

A struct that defines a method literally named `drop` runs that method on scope exit. The signature is fixed: `fn drop(mut self)`, no return type.

```cplus
struct Buf { ptr: *u8, len: usize }
impl Buf {
    fn drop(mut self) {
        unsafe { free(self.ptr); }
    }
}

fn main() -> i32 {
    let b: Buf = make_buf();
    // ... use b ...
    return 0;
}                                    // b.drop() runs here
```

Defining `drop` makes the type non-`Copy` — necessary, because copying a thing that owns a resource would lead to double-free.

### `defer` — run at scope exit, LIFO

```cplus
fn main() -> i32 {
    println(1);
    defer println(4);
    defer println(3);
    println(2);
    return 0;
}
// Prints 1, 2, 3, 4
```

`defer` and `Drop` share one scope-exit stack — they interleave in declaration order, popped LIFO at exit.

---

## 14. The borrow checker

The rule: **aliasing XOR mutability**. At any program point, a place has either any number of shared borrows OR exactly one exclusive borrow, never both.

```cplus
let mut v: vec::Vec[i32] = vec::Vec[i32]::new();
v.push(1);
let n: usize = v.len();      // shared borrow — fine
let p: i32 = v.get(0);       // shared borrow — fine
v.push(2);                   // exclusive — but no live shared borrow now; fine
```

The compiler enforces this at compile time. The common errors:

- **E0372** — move out of a borrowed value
- **E0383** — read while exclusively borrowed
- **E0370** family — overlapping incompatible borrows

When you see one, the fix is almost always to introduce a scope boundary so the conflicting borrows don't co-exist:

```cplus
{
    let r: i32 = v.get(0);
    println(r);
}                            // shared borrow ends here
v.push(99);                  // exclusive borrow now fine
```

---

## 15. Generics

Generics use `[T]`, not `<T>` (this avoids the `a<b>(c)` grammar ambiguity).

### Generic functions

```cplus
fn identity[T](x: T) -> T { return x; }

fn pair[A, B](a: A, b: B) -> Pair[A, B] {
    return Pair[A, B] { first: a, second: b };
}
```

### Generic structs and enums

```cplus
struct Pair[A, B] {
    pub first: A,
    pub second: B,
}

impl Pair[A, B] {
    pub fn new(a: A, b: B) -> Pair[A, B] {
        return Pair[A, B] { first: a, second: b };
    }
}

enum Option[T] {
    Some(T),
    None,
}
```

### Bounds

```cplus
fn max[T: Ord](a: T, b: T) -> T { ... }
```

Standard bounds (when implemented for `T`):

- `Ord` — total ordering (`<`, `==`)
- `Eq` — equality
- `Hash` — hashable (for maps)

Implement a bound with an `impl <Bound> for <Type>` block when you define a new type that needs to participate.

### Turbofish

When the compiler can't infer a type parameter, supply it explicitly with `::[T]`:

```cplus
let h = thread::spawn::[i32](worker);
let v = vec::Vec[i32]::with_capacity(16);
let s = size_of::[Point]();
```

Use `::[T]` for free fns / associated fns. Use `Vec[T]::new()` for type-attached associated fns. Both work; the difference is purely syntactic.

### Internal vs source names

The compiler monomorphises generic instantiations to mangled names like `Option__i32`. **These are internal.** Always write `Option[i32]::Some(v)` in source — at value sites and at pattern sites.

---

## 16. Error handling without exceptions

No `try`, no `catch`, no `throw`, no `?` operator. Fallible functions return a tagged-union value and callers match on it.

### Define your own result type

```cplus
enum ParseResult {
    Ok(i32),
    BadInput,
    Overflow,
}

fn parse(s: str) -> ParseResult { ... }
```

### The verbose form — explicit `match`

```cplus
fn parse_or_zero(s: str) -> i32 {
    return match parse(s) {
        ParseResult::Ok(v)       => v,
        ParseResult::BadInput    => 0 -% 1,
        ParseResult::Overflow    => 0 -% 2,
    };
}
```

### The readable form — `guard let`

```cplus
fn handle(s: str) -> i32 {
    guard let ParseResult::Ok(v) = parse(s) else { return 0 -% 1; };
    return v +% 100;
}
```

### Generic Result and Option from stdlib

```cplus
import "stdlib/result" as result;
import "stdlib/option" as option;

fn maybe_lookup(k: str) -> option::Option[i32] {
    if k == "answer" { return option::Option[i32]::Some(42); }
    return option::Option[i32]::None;
}
```

There is no `?` propagation operator and no `!T` magic. The control-flow primitives plus `guard let` give you the same ergonomics with full locality.

---

## 17. Strings — `str` vs `string`

Two distinct types, and the distinction matters.

| Type | What it is | Size | Owns memory? | Where it lives |
|---|---|---|---|---|
| `str` | A string view: `(*u8, usize)` | 16 bytes | No, borrowed | Stack / argument |
| `string` | Owned heap-allocated string | 24 bytes (`(*u8, usize, usize)`) | Yes | Heap (stdlib type) |

```cplus
let a: str = "hello";                       // string literal — always str
let b: string = "hello".to_string();        // copies to a heap allocation
```

The bridges:

```cplus
let s: str = "hi";
let p: *u8     = str_ptr(s);                            // safe
let n: usize   = str_len(s);                            // safe
let v: str     = unsafe { str_from_raw_parts(p, n) };   // unsafe (caller asserts validity)
```

### Rules of thumb

- String literals are `str`. Treat them as program-lifetime constants.
- For owned string parameters, use `move x: string` to avoid double-free.
- `str` parameters are **not allowed** in `async fn` signatures — pass `string` instead (E0900).
- For interop with libc, `str_ptr(s)` gives you a `*u8` you can hand to `printf`, `write`, etc.

---

## 18. String interpolation

Phase 8 added `\{EXPR}` interpolation inside string literals:

```cplus
let name: str = "world";
let n: i32 = 42;
let s: string = "hello \{name}, the answer is \{n}".to_string();
io::println(s.as_str());
```

The interpolation lowers to a series of concatenations, so any expression you can write in a position that produces `str` / `string` / a number is interpolable. Format specifiers (`\{x:04d}`) are **not** in v0.0.4 — convert numbers to strings explicitly when you need formatting.

---

## 19. FFI — calling C

C+ emits standard object files. The system linker stitches them with anything `clang` would. The language-level interop primitive is `extern fn`.

### Declaring symbols

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
extern fn printf(fmt: *u8, ...) -> i32;   // varargs OK on extern only
```

### Raw pointers

`*T` is an 8-byte opaque address. It's `Copy`. **Every operation** on it requires `unsafe`:

```cplus
let p: *u8 = unsafe { malloc(64 as usize) };
unsafe {
    p[0] = 65 as u8;                  // store
    let b: u8 = p[1];                 // load
    let q: *u8 = p + 1;               // pointer arithmetic (strides by sizeof(T))
    free(p);
}
```

Pointer arithmetic itself is "safe" math (no memory access), but in practice you almost always use it inside `unsafe` since the next thing you do is dereference.

### `unsafe { ... }`

Required for: pointer dereference, pointer indexing, `extern fn` calls, `str_from_raw_parts`, integer-to-pointer casts.

### Null pointers

The word `null` never appears. At FFI boundaries:

```cplus
let p: *u8 = unsafe { 0 as *u8 };
```

### `#[repr(C)]` — stable C layout

```cplus
#[repr(C)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}
```

Promises field order is preserved and padding/alignment matches the platform C ABI. **Always** use it on structs that cross an `extern fn` boundary by value.

### `#[link_name = "..."]` — multiple signatures, one symbol

When one C symbol has multiple typed shapes (the ObjC `objc_msgSend` pattern):

```cplus
#[link_name = "objc_msgSend"] extern fn msg_void(recv: *u8, sel: *u8);
#[link_name = "objc_msgSend"] extern fn msg_get_str(recv: *u8, sel: *u8) -> *u8;
```

Both resolve to `_objc_msgSend` at link time.

### Variadic ABI gotcha

If the C header says `int fcntl(int fd, int cmd, ...);` then the C+ extern **must** be variadic. On AArch64-darwin, named args go in registers but varargs go on the stack — a fixed-arity declaration silently passes garbage:

```cplus
// ✅
extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;

// ❌ Returns 0 ("success") but the call actually no-ops.
extern fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
```

### Pointer ↔ integer casts go through `usize`

```cplus
// ✅
let n: usize = unsafe { p as usize };
let i: i32   = n as i32;

// ❌ E0315 — cannot cast pointer to i32.
let bad: i32 = unsafe { p as i32 };
```

---

## 20. Function pointers

Function pointers exist; closures do not.

### Type position

```cplus
fn(i32, i32) -> i32          // takes two i32, returns i32
fn(*u8)                       // takes *u8, returns unit
```

### Coercion

A bare function identifier in an expected-fn-pointer position coerces to a fn pointer:

```cplus
fn handle(e: i32) -> i32 { return e +% 1; }

extern fn atexit(cb: fn()) -> i32;
fn cleanup() { println(99); }

fn main() -> i32 {
    unsafe { atexit(cleanup); }    // bare name coerces
    return 0;
}
```

### Struct of callbacks

```cplus
struct Actions {
    on_click: fn(i32) -> i32,
    on_hover: fn(i32) -> i32,
}

let a: Actions = Actions { on_click: handle_click, on_hover: handle_hover };
let r: i32 = a.on_click(7);     // indirect call through the field
```

### Stateful callbacks — the C convention

Fn pointers don't capture environment. For callbacks that need state, do what C does: pass `(fn_ptr, user_data: *u8)`, and have the library thread `user_data` back to you unchanged:

```cplus
extern fn libfoo_subscribe(cb: fn(*u8, i32), user_data: *u8);
```

---

## 21. Compile-time intrinsics

Five built-ins evaluate at compile time: `size_of::[T]()`, `align_of::[T]()`, `include_bytes!(...)`, `include_str!(...)`, and `env!(...)`. The first two are typed query primitives; the next two embed file contents into the binary as constants; the last reads an environment variable at build time.

### `size_of::[T]()` and `align_of::[T]()`

Return `usize`. **Safe** — no memory access; LLVM folds the call to a constant at `-O1+`.

```cplus
let s_i32: usize  = size_of::[i32]();          // 4
let a_i32: usize  = align_of::[i32]();         // 4
let s_p: usize    = size_of::[Point]();        // structural, depends on fields
```

Used by user-level allocator libraries to compute byte counts for typed allocations:

```cplus
let bytes: usize = size_of::[T]() *% (n as usize);
let p: *u8       = unsafe { malloc(bytes) };
let typed: *T    = p as *T;
```

### `include_bytes!("relative/path")`

Embeds the raw bytes of a file as a `*[u8; N]` where `N` is the file's byte length, known at compile time. Path resolution is relative to the *source file containing the call*, not the project root.

```cplus
fn main() -> i32 {
    let shader: *[u8; 2048] = include_bytes!("../shaders/double.metallib");
    let bytes: *u8 = unsafe { shader as *u8 };
    // pass to FFI, etc.
    return 0;
}
```

The bytes live in `.rodata`; writing through the returned pointer is UB. Two calls with the same resolved path share one global. The argument must be a string literal — variables fire **E0871** at parse time. Errors:

- **E0870** — path not found at compile time. Diagnostic carries the resolved absolute path.
- **E0871** — non-string-literal argument.
- **E0872** — file exceeds 64 MiB sanity limit.

Used by GPU recipes to embed `.metallib` / `.cubin` / `.spv` shader blobs, by ML packages to embed pretrained weights, and by anyone shipping baked-in fixtures.

### `include_str!("relative/path")`

Same shape, but returns a `str` (fat pointer view; see §17). The byte length is part of the type, so the file's UTF-8 size is implicit:

```cplus
fn main() -> i32 {
    let manifest: str = include_str!("config.txt");
    println(manifest);   // str_len(manifest) == file size
    return 0;
}
```

The bytes must be valid UTF-8 — invalid byte sequences fire **E0875** at sema time with the byte offset of the first bad byte. Same `E0870` / `E0871` / `E0872` error path as `include_bytes!`.

Use case the `metal_compute` recipe surfaced: `include_str!("../shaders/double.metallib.size")` to read the byte count produced by `xcrun metallib` at build time — no shell-side source patching needed.

### `env!("NAME")`

v0.0.8 addition: read an environment variable at compile time. Returns a `str` pointing at a `.rodata` global that contains the variable's value as the compiler saw it.

```cplus
fn main() -> i32 {
    let greeting: str = env!("GREETING");   // resolved at sema time
    println(greeting);
    return 0;
}
```

```bash
GREETING="hi from build" cpc env_demo.cplus -o env_demo
./env_demo
# → hi from build
```

Useful for baking build-time config into a binary — sample count for a benchmark, version string, build hostname, etc. — without recompiling for every value change.

Errors:
- **E0871** — non-string-literal argument (same as `include_*!`).
- **E0876** — environment variable not set when cpc was invoked.

There is no `option_env!` for "missing → None" semantics; the strict form covers the build-time-config case cleanly, and the nullable variant complicates the type signature. If you need an optional, wrap the call:

```cplus
// Not a real macro — illustrative pattern.
// If you genuinely need optional build config, set a sentinel:
//
//   FOO_VAR="" cpc app.cplus
//
// and check `str_len(env!("FOO_VAR")) > 0` at runtime.
```

---

## 22. Modules, imports, packages

### Single-file mode

A `.cplus` file compiled with `cpc file.cplus -o bin` has no imports — only intrinsics are available.

### Project mode

Every import declares **where** the module comes from. Bare paths are rejected — the resolver demands you say "local" or "vendored":

```cplus
// Local file at src/math.cplus
import "./math" as math;
math::area(2, 3);

// Vendored package — the path's first segment is the dep name from Cplus.toml
import "stdlib/io" as io;
io::println("hi");
```

- Local: starts with `./` — resolved relative to the current file.
- Vendored: first segment matches a `[dependencies]` entry — resolved from `vendor/<dep>/src/<rest>.cplus`.

The alias is **mandatory**: `import "X" as Y;` and you call into the module as `Y::thing(...)`. No glob imports, no `use`.

### `pub` for cross-file visibility

By default, everything is module-private. `pub` exports.

```cplus
pub fn answer() -> i32 { return 42; }
pub struct Public { pub field: i32 }
pub enum Color { Red, Green, Blue }
```

### `Cplus.toml`

```toml
[package]
name    = "myproj"
version = "0.0.1"
edition = "2026"

[[bin]]
name = "myproj"
path = "src/main.cplus"

[dependencies]
stdlib = "*"
```

The stdlib is consumed like any other vendored package — symlink `/Users/adel/Workspace/C+/vendor/stdlib` into your project's `vendor/stdlib`.

---

## 23. Attributes

Attributes are pure metadata — they flip flags the compiler reads. They never generate code, transform the AST, or run user logic.

### `#[test]` — register a test function

```cplus
#[test]
fn it_adds() {
    let r: i32 = add(2, 3);
    assert r == 5;
}
```

Run with `cpc test`. The `assert` intrinsic, in a test build, sets a failure flag; in a regular build, it traps.

### `#[repr(C)]` — stable struct layout (see §19)

### `#[link_name = "..."]` — symbol aliasing (see §19)

### `#[unroll(N)]` and `#[vectorize_width(N)]` — loop hints

Statement-level attributes that flow through to LLVM's loop optimizer as `!llvm.loop` metadata. Apply to `while`, `loop`, or `for` statements. `N` must be a literal in `[1, 256]`.

```cplus
#[unroll(4)]
while i < n {
    sum = sum + buf[i as usize];
    i = i +% 1;
}

#[vectorize_width(8)]
for i in 0..count {
    out[i] = a[i] * b[i];
}
```

`#[unroll(N)]` asks LLVM to unroll the loop N times; `#[vectorize_width(N)]` hints the autovectorizer toward an N-wide SIMD shape. Marginal for general code; **load-bearing for tight inner loops** that the compiler doesn't choose well by default.

### Doc comments

```cplus
/// Returns the square of x.
fn sq(x: i32) -> i32 { return x *% x; }
```

`///` comments are doc comments. `cpc test` will pick up fenced code blocks in them as doctests.

---

## 24. Threads and atomics

```cplus
import "stdlib/thread" as thread;

fn worker() -> i32 { return 42; }

fn main() -> i32 {
    let h: thread::JoinHandle[i32] = thread::spawn::[i32](worker);
    return h.join();
}
```

### Passing data into the worker

For non-Copy input, use `spawn_with`:

```cplus
fn proc(move s: string) -> i32 { return s.len() as i32; }

let s = "hello".to_string();
let h = thread::spawn_with::[string, i32](s, proc);
let n = h.join();
```

### The safe pattern — partition + join

```cplus
import "stdlib/thread" as thread;

struct Range { start: i64, end: i64 }

fn sum_range(r: Range) -> i64 {
    let mut total: i64 = 0 as i64;
    let mut i: i64 = r.start;
    while i < r.end { total = total +% i; i = i +% (1 as i64); }
    return total;
}

pub fn main() -> i32 {
    let left:  Range = Range { start: 1   as i64, end: 501  as i64 };
    let right: Range = Range { start: 501 as i64, end: 1001 as i64 };

    let h1 = thread::spawn_with::[Range, i64](left,  sum_range);
    let h2 = thread::spawn_with::[Range, i64](right, sum_range);
    let total: i64 = h1.join() +% h2.join();
    if total != (500500 as i64) { return 1; }
    return 0;
}
```

This is the *first* pattern to reach for. Race-freedom is mechanical: no shared memory means no race.

### Atomics — for the rare cases that can't partition

```cplus
import "stdlib/atomic" as atomic;

let counter: u64 = 0 as u64;
let p: *u64 = unsafe { ... };       // pointer to shared u64
unsafe {
    atomic::atomic_fetch_add_u64(p, 1 as u64, atomic::Ordering::Relaxed);
}
```

Ordering values: `Relaxed | Acquire | Release | AcqRel | SeqCst`. Widths: i32/i64/u32/u64.

### Mutex

```cplus
import "stdlib/mutex" as mutex;

let m = mutex::new::[i32](10);
let m2 = m.clone();              // share across threads (Mutex is internally refcounted)
{
    let mut g = m.lock();
    g.set(g.get() +% 1);
}                                 // guard's Drop releases
```

**Two guards in the same scope deadlock** — the borrow checker doesn't yet prevent this. Use block scopes to bound each guard's lifetime.

---

## 25. Async / await

```cplus
import "stdlib/future" as future;
import "stdlib/executor" as executor;

async fn inner() -> i32 { return 7; }

async fn outer() -> i32 {
    let x: i32 = await inner();
    return x +% 1;
}

fn main() -> i32 {
    let f: future::Future[i32] = outer();
    return executor::block_on::[i32](f);
}
```

### Signature rules

- `async fn` returns `Future[T]`, written as the bare `T` in the signature.
- `await EXPR` suspends until `EXPR` (a future) resolves and yields its value.
- Borrow-shaped parameters (`str`, `T[]`, `mut x: NonCopyT`) are rejected in `async fn` signatures (**E0900**). Pass `string` and `Vec[T]` instead.

### Reactor — concurrent I/O (v0.0.5)

The v0.0.5 reactor (kqueue on darwin) makes `async` actually concurrent. Two cooperative primitives:

```cplus
import "stdlib/executor" as executor;
import "stdlib/time" as time;

async fn task_a() {
    await time::sleep(100 as u64);
    println("a done");
    return;
}

async fn task_b() {
    await time::sleep(50 as u64);
    println("b done");
    return;
}

async fn main_async() {
    executor::spawn_local::[()](task_a());
    executor::spawn_local::[()](task_b());
    await executor::yield_now();
    return;
}

fn main() -> i32 {
    executor::block_on::[()](main_async());
    return 0;
}
```

`time::sleep(ms)` suspends the current task; the reactor wakes it when the timer fires. Multiple sleeps run concurrently — total wall-clock is `max(durations)`, not `sum`.

---

## 26. Iterators and `gen fn`

A `gen fn` is a generator: each `yield` suspends the function and produces a value; the consumer pulls values until the generator returns.

```cplus
import "stdlib/iterator" as iterator;

gen fn count_up(n: i32) -> i32 {
    let mut i: i32 = 0;
    while i < n {
        yield i;
        i = i +% 1;
    }
    return;
}

fn main() -> i32 {
    for v in count_up(5) {
        println(v);
    }
    return 0;
}
```

The return type of a `gen fn` is `Iterator[T]`. The `for x in iter { ... }` loop lowers to a `while let Some(x) = iter.next() { ... }` over `Option[T]`.

### Adapters

The stdlib provides composable adapters as free functions:

```cplus
import "stdlib/iterator" as iter;

gen fn squares(n: i32) -> i32 {
    let mut i: i32 = 0;
    while i < n { yield i *% i; i = i +% 1; }
    return;
}

fn double(x: i32) -> i32 { return x *% 2; }

for v in iter::map::[i32, i32](squares(5), double) {
    println(v);
}
```

---

## 27. Standard library tour

Every module here lives in `vendor/stdlib/src/<name>.cplus` and imports as `"stdlib/<name>"`.

### `stdlib/io` — basic I/O

```cplus
import "stdlib/io" as io;
io::print("no newline");
io::println("with newline");
io::eprintln("to stderr");
```

Backed by `printf`. Buffered through stdio.

### `stdlib/result` and `stdlib/option`

```cplus
import "stdlib/result" as result;
import "stdlib/option" as option;

let r: result::Result[i32, result::IoError] = result::io_ok::[i32](42);
let e: result::Result[i32, result::IoError] = result::io_err::[i32](result::IoError::NotFound);

let some_n: option::Option[i32] = option::some::[i32](7);
let no_n:   option::Option[i32] = option::Option[i32]::None;
```

Both types are generic. Match on the variant. There's no `?` propagation — use `guard let`.

### `stdlib/vec` — growable vector

```cplus
import "stdlib/vec" as vec;

let mut v: vec::Vec[i32] = vec::Vec[i32]::with_capacity(16);
v.push(1);
v.push(2);
v.push(3);

let n: usize = v.len();
let cap: usize = v.capacity();
let first: option::Option[i32] = v.get(0);

let popped: option::Option[i32] = v.pop();

// Bulk-copy fast path:
unsafe { v.extend_from_raw(some_ptr, count); }

// Vec implements Drop — when v goes out of scope, the buffer is freed.
```

Other methods: `as_slice()`, `reserve(extra: usize)`, `clear()`.

### `stdlib/hash_map` — `HashMap[K, V]` + the `StrIntMap` legacy alias

```cplus
import "stdlib/hash_map" as hash_map;

// Generic — shipped in v0.0.4. K must be Hash + Eq; primitives + str work today.
let mut m: hash_map::HashMap[str, i32] = hash_map::new::[str, i32]();
m.insert("hello", 42);
m.insert("world", 7);

let r: result::Result[i32, result::IoError] = m.get("hello");
let present: bool = m.contains_key("hello");
let count: usize = m.len();

// `new_str_int_map()` is retained as a thin v0.0.3-era constructor;
// the return type is the same `HashMap[str, i32]`.
let mut legacy: hash_map::HashMap[str, i32] = hash_map::new_str_int_map();
legacy.insert("k", 1);
```

Open addressing + linear probing + 0.75 load-factor grow.

### `stdlib/fs` — file I/O

```cplus
import "stdlib/fs" as fs;

let r: result::Result[fs::File, result::IoError] = fs::open_read("data.txt");
guard let result::Result::Ok(f) = r else { return 1; };

let mut buf: [u8; 256] = [0u8, 0u8, ...];   // expand as needed
let n: isize = f.read(str_ptr_from_raw(...), 256 as usize);

let w = fs::create("out.txt");
// File implements Drop — closes on scope exit.
```

### `stdlib/net` — TCP

```cplus
import "stdlib/net" as net;

// Client
let c = net::connect_tcp("127.0.0.1", 8080 as u16);
guard let result::Result::Ok(sock) = c else { return 1; };
sock.send_bytes(payload_ptr, payload_len);
let n: isize = sock.recv_bytes(buf_ptr, buf_cap);

// Server
let l = net::listen_tcp(8080 as u16);
guard let result::Result::Ok(listener) = l else { return 1; };
let a = listener.accept();
```

v0.0.4 supports IPv4 with numeric IPs only. For hostname resolution use `gethostbyname` directly via FFI.

### `stdlib/env` — environment variables and argv

```cplus
import "stdlib/env" as env;

let port = env::var("PORT");                            // Result[string, IoError]
guard let result::Result::Ok(p) = port else { ... };

// argv access is platform-specific — on darwin via _NSGetArgc/_NSGetArgv.
```

### `stdlib/thread` and `stdlib/atomic` — see §24

### `stdlib/box` — single heap-allocated owned value

```cplus
import "stdlib/box" as box;

let b = box::new::[i32](42);
let v: i32 = b.unwrap();        // consumes b; exit-Drop frees the slot
```

### `stdlib/arc` — atomic refcounted shared ownership

```cplus
import "stdlib/arc" as arc;

let root = arc::new::[i32](7);
let c1 = root.clone();          // atomic refcount increment
let c2 = root.clone();
// All three drop normally; the last reference frees.
```

### `stdlib/rc` — single-threaded refcount

Same as `Arc` but non-atomic. Cheaper, single-thread only. The compiler doesn't yet enforce `!Send` for `Rc`, so don't ship it across threads by hand.

### `stdlib/mutex` — pthread-backed mutual exclusion

See §24. Internally refcounted (collapses `Arc` into itself — C+ has no `&T` to make `Arc[Mutex[T]]` work safely).

### `stdlib/channel` — typed message passing

```cplus
import "stdlib/channel" as channel;

let (tx, rx) = channel::unbounded::[i32]();
// tx and rx can be cloned for multi-producer / multi-consumer use.
tx.send(42);
let v: option::Option[i32] = rx.recv();
```

### `stdlib/future`, `stdlib/executor`, `stdlib/reactor`, `stdlib/time` — see §25

### `stdlib/iterator` — see §26

### `stdlib/cow` — clone-on-write string

```cplus
import "stdlib/cow" as cow;

let c1: cow::CowStr = cow::from_view("hello");                 // borrows the literal
let c2: cow::CowStr = cow::from_owned("world".to_string());    // takes ownership
let n: usize = cow::len(c1);                                   // uniform read access
```

API is free-functions — a pre-v0.0.5 shape from when sema rejected `impl` on enum types. That restriction lifted in v0.0.5 Slice 2C, but the library hasn't been re-shaped yet. Method-style migration is on the v0.0.7+ stdlib polish list.

### `stdlib/range` — numeric ranges (used by `for in`)

The `0..n` syntax lowers to a value of type `Range[i32]` (or similar) defined here.

### `stdlib/marker` — marker traits

Type-level markers used by the compiler (`Copy`, `Send`, `Sync` framework). You rarely interact with these directly.

### Beyond stdlib: blessed vendored packages

The same `vendor/<name>` model that hosts `stdlib` hosts other blessed binding packages. Consumers add `<name> = "*"` to `[dependencies]` and import as `import "<name>/..." as alias;`. Today's in-tree set:

- **`vendor/appkit`** — typed Cocoa/AppKit bindings. 15 sub-modules (`runtime`, `application`, `window`, `view`, `controls`, `text`, `containers`, `data`, `graphics`, `menu`, `dialogs`, `panels`, `toolbar`, `controllers`, `convert`). Closure-free callbacks via `Button::set_on_click(fn(*u8))` (the runtime stashes the callback on the sender via `objc_setAssociatedObject`).

  ```cplus
  import "appkit/application" as application;
  import "appkit/window" as window;
  import "appkit/convert" as bridge;

  fn on_click(sender: *u8) { /* ... */ }

  fn main() -> i32 {
      let pool = application::AutoreleasePool::new();
      let app = application::Application::shared();
      app.set_activation_policy(0 as i64);
      let win = window::Window::new(frame, 15 as u64, 2 as u64, 0 as i8);
      // ... build UI ...
      app.run();
      pool.drain();
      return 0;
  }
  ```

  `appkit/convert` is the C+ ↔ ObjC data bridge: `cplus_str_to_nsstring(s) -> *u8`, `nsstring_to_cplus_string(ns) -> string`, `nsarray_to_vec_{i32,i64,f32,f64}`, `nsdata_to_vec_u8` / `vec_u8_to_nsdata`. Use it whenever you need to round-trip C+'s richer types (`string`, `Vec[T]`) through Cocoa APIs. Primitives + `#[repr(C)]` structs (NSPoint/NSSize/NSRect) cross the boundary verbatim — no bridge needed for those.

  Cross-reference: [`docs/examples/recipes/appkit_hello/`](docs/examples/recipes/appkit_hello/) is the runnable reference (Window + label + button + bridge round-trip).

- **`vendor/simd`** — 3D math built on `f32x4`. Three modules:
  - `simd/vec3` — `Vec3` (f32x4 newtype with lane-3-zero invariant). Methods: `new / splat / zero / x / y / z / add / sub / mul / scale / neg / dot / cross / len2 / length / normalize / reflect / refract / min / max / clamp / lerp`.
  - `simd/vec4` — `Vec4` (full 4 lanes). Same surface minus the cross/reflect/refract triad (no canonical 4D meaning); plus `raw()` / `from_raw()` escape hatches for matrix code.
  - `simd/mat4x4` — `Mat4x4` as `[Vec4; 4]` columns (column-major). `mul_vec` is four `fma <4 x float>` ops; `mul` composes via four `mul_vec` calls. `identity / zero / add / scale` round out the basics.

  ```cplus
  import "simd/vec3" as vec3;
  import "simd/vec4" as vec4;
  import "simd/mat4x4" as mat;

  let a: vec3::Vec3 = vec3::Vec3::new(1.0f32, 2.0f32, 3.0f32);
  let b: vec3::Vec3 = vec3::Vec3::new(4.0f32, 5.0f32, 6.0f32);
  let d: f32        = a.dot(b);                  // 32.0
  let c: vec3::Vec3 = a.cross(b);                // (-3, 6, -3)

  let id: mat::Mat4x4 = mat::Mat4x4::identity();
  let p:  vec4::Vec4  = id.mul_vec(vec4::Vec4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32));
  ```

  **Where this wins**: vector-math workloads that genuinely use all four lanes (Vec4, Mat4x4 chains, FMA-heavy gemm-style code). For Vec3-only code on Apple Silicon, the scalar FMA-chain codegen the compiler emits from straightforward source can be faster than the explicit-SIMD path because the shuffle overhead for the unused 4th lane dominates — measure before assuming SIMD types are a perf win for 3-wide data.

  **Where this wins for sure**: code where readability of "this is a SIMD operation" matters more than the last few percent of perf, or hardware with narrower scalar FP issue width.

  Cross-reference: [`docs/examples/recipes/simd_dot/`](docs/examples/recipes/simd_dot/) uses `f32x4` directly; `vendor/simd` is the wrapper for when you want named types + methods instead of raw vectors.

---

## 28. Tooling — `cpc`

```bash
cpc build                      # multi-file project (reads Cplus.toml)
cpc FILE.cplus -o BIN          # single-file build
cpc check FILE                 # parse + sema only — fast feedback
cpc fmt FILE                   # canonical format in place
cpc fmt --check DIR            # CI mode — exits 1 on drift
cpc test                       # run #[test] functions + doctests
cpc lsp                        # start the language server
cpc --emit-ll FILE             # pre-optimisation LLVM IR
cpc --emit-ll-opt FILE         # post-optimisation LLVM IR
cpc --emit-asm FILE            # native assembly
cpc --diagnostics=json         # machine-readable diagnostics
cpc --release                  # -O2 (default is debug -O0 with overflow traps)
cpc -V                          # print version
```

### Linking against Apple frameworks

`cpc build` doesn't yet know about framework search paths or the ObjC runtime. For programs needing `-framework X` or `-lobjc`:

```bash
cpc --emit-ll src/main.cplus > out.ll
clang out.ll \
    -framework Cocoa \
    -lobjc \
    -Wno-override-module \
    -o my_binary
```

### Tests

Every new feature should ship with at least three test shapes:

1. **Positive** — the program compiles and runs as expected.
2. **Negative-with-code** — the program rejects with a specific Exxxx code.
3. **End-to-end** — drives `cpc build` from start to finish.

See [cpc/tests/e2e.rs](cpc/tests/e2e.rs) for the canonical pattern.

---

## 29. Common error codes

The error codes you'll see most often. The full list lives in `cplus-core/src/sema.rs` and `borrowck.rs`.

| Code | Meaning | Typical fix |
|---|---|---|
| E0300 | Undefined name | Typo, missing import, or forgotten `pub` |
| E0301 | Duplicate definition | Two items with the same name |
| E0302 | Type mismatch | Insert `as` cast or change declared type |
| E0303 | Unknown type | Typo, missing import, or generic param not in scope |
| E0312 | Function used as value | Assign to a `fn(...)`-typed binding to take the address |
| E0315 | Invalid cast | Some pairs are forbidden (e.g. int→bool, `*T → i32`) |
| E0319/20/21/22 | Struct field issues (dup / unknown / missing / extra) | Match the declaration |
| E0325 | `impl` on unknown / non-struct type | Target must be a declared struct/enum in scope |
| E0327 | Wrong call form | `Type::method()` for assoc, `value.method()` for instance |
| E0333 | Implicit return | Add explicit `return EXPR;` |
| E0335 | Use of moved value | Don't read after `move` |
| E0340 | Non-exhaustive match | Add the missing arm or `_ =>` catch-all |
| E0345 | Use of possibly-unassigned binding | Initialize on every control-flow path |
| E0353 | `break` / `continue` outside loop | Move into a loop body |
| E0354 | Unknown attribute | Typo (compiler suggests fix) |
| E0356 | Wrong attribute target | Some attrs are fn-only, others struct-only |
| E0370–0386 | Borrow checker conflicts | Each variant has a specific message; read it |
| E0411 | `restrict` on a non-pointer param | Only `*T` accepts `restrict`; remove or change the type |
| E0500 | Cannot infer type parameter | Use `name::[T1, T2](...)` turbofish |
| E0501 | Wrong type-arg count | Match the generic param list |
| E0502 | Bound not satisfied | `T: Ord` requires `impl Ord for T` |
| E0801 | Operation requires `unsafe` | Wrap in `unsafe { ... }` |
| E0821 | Cannot take address of generic fn | Specify type parameters at the take-address site |
| E0876 | `env!("X")` — env var not set at compile time | Set the var when invoking cpc, or pick a different default |
| E0900 | Borrow-shaped param in `async fn` | Use `string` / `Vec[T]` instead of `str` / `T[]` |

Every diagnostic carries a span and often a machine-applicable suggestion. Use `--diagnostics=json` for tool consumption.

---

## 30. Gotchas worth memorising

These bite once and are remembered forever. Read them now.

### Use `move v: T` for non-Copy value parameters

```cplus
// ❌ Caller's `s` and callee's `x` both run Drop. Double-free under ASan.
fn echo(x: string) -> string { return x; }

// ✅ Marks `s` moved at the call site.
fn echo(move x: string) -> string { return x; }
```

### `move self` doesn't auto-disarm the callee's exit-Drop

```cplus
// ❌ Frees twice: explicit free + exit-Drop both fire.
pub fn unwrap(move self) -> T {
    let v: T = unsafe { *self.p };
    unsafe { free(self.p as *u8); }   // BUG
    return v;
}

// ✅ Let exit-Drop do the cleanup.
pub fn unwrap(move self) -> T {
    return unsafe { *self.p };
}
```

### Bind clone results to a local before passing as `move`

```cplus
// ❌ E0337 — cannot move out of method-call result.
worker(root.clone());

// ✅
let c = root.clone();
worker(c);
```

### Mutex guards in the same scope deadlock

```cplus
// ❌
let g = m.lock();
let g2 = m.lock();    // deadlock

// ✅
{ let g  = m.lock(); /* ... */ }
{ let g2 = m.lock(); /* ... */ }
```

### String literals are `str`, not `string`

```cplus
let a: str    = "hello";
let b: string = "hello".to_string();
```

### Don't `malloc` small fixed buffers in hot loops

```cplus
// ❌ 2M malloc/free pairs killed the hashmap bench by 2.4×.
let tmp_ptr: *u8 = unsafe { malloc(10 as usize) };

// ✅ Stack array — zero allocation.
let mut tmp: [u8; 10] = [0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8];
```

### Variadic libc functions: declare with `...`

```cplus
// ✅ Mirrors the C header.
extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;

// ❌ Silently broken on AArch64-darwin.
extern fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
```

### Pointer casts go through `usize`

```cplus
// ✅
let n: usize = unsafe { p as usize };
let i: i32   = n as i32;

// ❌ E0315
let n: i32 = unsafe { p as i32 };
```

---

## 31. SIMD types

cpc ships fixed-width SIMD as primitive types. Nineteen widths cover the 128-bit and 256-bit families that map directly to NEON / SSE / AVX2 / AVX:

- **128-bit floats**: `f32x4`, `f64x2`
- **128-bit signed ints**: `i8x16`, `i16x8`, `i32x4`, `i64x2`
- **128-bit unsigned ints**: `u8x16`, `u16x8`, `u32x4`, `u64x2`
- **256-bit floats**: `f32x8`, `f64x4`
- **256-bit signed ints**: `i8x32`, `i16x16`, `i32x8`, `i64x4`
- **256-bit unsigned ints**: `u8x32`, `u16x16`, `u32x8`, `u64x4`

Plus mask types: `mask8x16`, `mask16x8`, `mask32x4`, `mask64x2`, `mask8x32`, `mask16x16`, `mask32x8`, `mask64x4`. Lower to `<N x i1>` conceptually; codegen stores them as `<N x iN>` for NEON/SSE compatibility.

512-bit widths (AVX-512 / SVE2) are deferred until those targets become tier-1.

### Constructors

```cplus
let v: f32x4 = f32x4::splat(1.0f32);                       // broadcast
let w: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32); // per-lane

// load/store through raw pointers (unsafe; lane-aligned)
let v2: f32x4 = unsafe { f32x4::load(p as *f32) };
unsafe { v.store(p as *f32); }

// FFI escape — bitcast to/from a plain array
let arr: [f32; 4] = v.to_array();
let v3: f32x4    = f32x4::from_array(arr);
```

### Methods — by element type

Arithmetic (all numeric widths): `.add(b)`, `.sub(b)`, `.mul(b)`, `.div(b)`.

Float-only: `.fma(b, c)` (fused multiply-add), `.sqrt()`, `.abs()`.

Signed-int-only: `.abs()` (no-op on unsigned would be misleading — rejected with **E0324**).

All numeric: `.min(b)`, `.max(b)` (NaN-as-missing for floats; signed/unsigned per lane type).

Integer-only: `.and(b)`, `.or(b)`, `.xor(b)`, `.not()`, `.shl(count)`, `.shr(count)` (logical for unsigned, arithmetic for signed; count is a literal `u32` in `0..lane_bits`).

### Lane access

```cplus
let v: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32);
let x: f32 = v.lane(0 as u32);                       // 1.0
let v2: f32x4 = v.with_lane(3 as u32, 9.0f32);       // (1, 2, 3, 9)
```

The lane index must be a **literal** `u32` in `0..N`. Non-literals fire **E0873**; out-of-range fires **E0874**. (Constraint matches LLVM's `extractelement` / `insertelement` constant-operand requirement.)

### Shuffles + reductions

```cplus
let v: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32);

let r: f32x4 = v.reverse();                          // (4, 3, 2, 1)
let s: f32   = v.sum();                              // 10.0 — llvm.vector.reduce.fadd
let p: f32   = v.product();                          // 24.0

// Per-element permutation. `lanes` is a literal const array.
let p: f32x4 = v.swizzle([3 as u32, 2 as u32, 1 as u32, 0 as u32]);

let lo: f32x4 = v.interleave_lo(other);
let hi: f32x4 = v.interleave_hi(other);
```

`sum()` / `product()` lower to `llvm.vector.reduce.{fadd,fmul}.<vN>`; `min_across()` / `max_across()` to `llvm.vector.reduce.{fmin,fmax,smin,smax,umin,umax}.<vN>`.

### Masks + select

Compare-and-blend is the canonical branchless pattern:

```cplus
let a: f32x4 = f32x4::new(1.0f32, 2.0f32, 3.0f32, 4.0f32);
let b: f32x4 = f32x4::splat(2.5f32);

let mask: mask32x4 = a.lt(b);                        // <true, true, false, false>
let result: f32x4  = mask.select(a, b);              // pick from a where mask, else b

if mask.any() { /* at least one lane true */ }
if mask.all() { /* every lane true */ }
```

Comparison methods (`lt` / `le` / `gt` / `ge` / `eq` / `ne`) produce a mask of the matching width; `select(true_v, false_v)` is the mask-receiver blend.

### `#[repr(C)]` boundaries — SIMD does NOT cross by default

SIMD types have no portable C-ABI representation. Passing `f32x4` across an `extern fn` boundary fires **E0410** with a "cast to `[f32; 4]` via `.to_array()`" hint. Use the array round-trip at the boundary:

```cplus
// ❌ E0410
pub extern fn process(v: f32x4) -> f32x4 { return v; }

// ✅ FFI-safe shape
pub extern fn process(v: [f32; 4]) -> [f32; 4] {
    let s: f32x4 = f32x4::from_array(v);
    return s.mul(f32x4::splat(2.0f32)).to_array();
}
```

### Worked example — dot product

```cplus
fn dot(a: [f32; 16], b: [f32; 16]) -> f32 {
    let mut acc: f32x4 = f32x4::splat(0.0f32);
    let mut i: i32 = 0;
    while i < 16 {
        let av: f32x4 = f32x4::new(
            a[(i +% 0) as usize], a[(i +% 1) as usize],
            a[(i +% 2) as usize], a[(i +% 3) as usize],
        );
        let bv: f32x4 = f32x4::new(
            b[(i +% 0) as usize], b[(i +% 1) as usize],
            b[(i +% 2) as usize], b[(i +% 3) as usize],
        );
        acc = av.fma(bv, acc);
        i = i +% 4;
    }
    return acc.sum();
}
```

`av.fma(bv, acc)` lowers to one `@llvm.fma.v4f32` call; `acc.sum()` to one `@llvm.vector.reduce.fadd.v4f32`. On AArch64-darwin, the inner loop emits `fmla.4s v0, v1, v2` — the native NEON fused multiply-add on four floats. See [`docs/examples/recipes/simd_dot/`](docs/examples/recipes/simd_dot/) for the full reference port.

### When (and when not) to reach for SIMD

Reach for SIMD when:

- You have a tight loop over a homogeneous primitive array (`[f32; N]`, `[i32; N]`, `[u8; N]`).
- The operation is lane-independent (vector arithmetic, lane permutation, mask-driven blends).
- You want the SIMD shape to be **visible** at the source level — explicit `f32x4::new` + `.mul` reads better than hoping the autovectorizer fires on a scalar loop.

Don't reach for SIMD when:

- The data has irregular structure (struct-of-struct, pointer-chasing graphs).
- The hot loop is already vectorized well by LLVM at `--release` (check `--emit-asm`).
- You'd need to fight the type system to express it. Keep SIMD where it's natural.

---

## 32. Where to go next

In rough priority order:

1. **Read a recipe.** [docs/examples/recipes/](docs/examples/recipes/) ships task-oriented complete programs:
   - `file_read`, `file_write`, `stdin_lines` — basic I/O
   - `argv_parse`, `env_var` — process input
   - `hash_table` — full hashmap usage
   - `tcp_client`, `tcp_server`, `http_get` — networking
   - `json_parse` — parser example
   - `parallel_sum` — safe concurrency (partition + join)
   - `concurrent_counter` — unsafe concurrency (shared `*u64` + atomic fetch_add)
   - `async_compute`, `async_fetch`, `async_yield_demo` — async patterns
   - `simd_dot` — `f32x4` dot product, NEON `fmla.4s` end-to-end
   - `metal_compute` — GPU compute dispatch via ObjC interop + `include_bytes!`-embedded `.metallib` (macOS, needs the Metal toolchain)
   - `appkit_hello` — Cocoa GUI app in pure C+ via `vendor/appkit` + the convert bridge
2. **Read an example.** Every file in [docs/examples/](docs/examples/) compiles and runs.
3. **Read a design note.** [docs/design/](docs/design/) has per-phase deep dives — pattern matching, generics, borrow rules, FFI, async, and the v0.0.6 "external-package enable" doc that explains the stdlib-model bet for SIMD and GPU.
4. **Run `cpc fmt`.** If your source doesn't round-trip, something is syntactically off.
5. **Read the diagnostic.** Every error code has a precise meaning. The compiler is the source of truth; this tutorial is a summary.

Welcome to C+. Code is a tool; precision is the point.
