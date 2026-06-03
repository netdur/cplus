# The C+ Tutorial

A complete walkthrough of C+: every language feature and every stdlib module, with runnable examples. C+ is a systems language with a Rust-style ownership model, a C ABI for FFI, and a deliberately small, unambiguous syntax.

For history and rationale, read [plan.md](plan.md). For a tight reference, read [SKILL.md](SKILL.md). This document walks through both.

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
12. [Ownership](#12-ownership)
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
28. [Vendor packages](#28-vendor-packages)
29. [C+ for LLMs](#29-c-for-llms)
30. [Tooling — `cpc`](#30-tooling--cpc)
31. [Common error codes](#31-common-error-codes)
32. [Gotchas worth memorising](#32-gotchas-worth-memorising)
33. [SIMD types](#33-simd-types)
34. [Where to go next](#34-where-to-go-next)

---

## 1. What C+ is — and what it isn't

C+ compiles to native code through LLVM. It has no GC, no exceptions, no closures, no overloading, no implicit conversions, and no `null`. It has manual memory management policed by a Rust-style borrow checker, plus one-way C ABI compatibility for FFI.

The point: every program is **locally legible**. You can read one function and know exactly what it does without chasing implicit destructors, hidden conversions, or surprise allocations. The compiler enforces that property. If you slip, the diagnostic tells you precisely where and why.

### Locked principles

These principles underlie every C+ design decision.

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
| 13 | Module-private by default | `pub` is the export marker. Public symbols are intentional, not accidental. Vendor-package model makes accidental exposure load-bearing. |

These are fixed design decisions, not open questions.

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

`println` is a built-in **intrinsic** in single-file mode; there is no import. It accepts `i32` or `str`.

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
| Float | `f16 f32 f64` |

No `int`, no `long`, no `byte`. The size is part of the name.

`f16` is the IEEE half-precision float (LLVM `half`, 2 bytes) — primarily a
**storage** type for ML/graphics data (e.g. ggml weight blocks). It converts to
and from the wider floats with `as` (hardware `fpext`/`fptrunc`), and exposes a
bit-preserving reinterpret to/from its raw `u16`:

```cplus
let h: f16 = 1.5f32 as f16;          // fptrunc
let x: f32 = h as f32;               // fpext

// Bit reinterpret (NOT a numeric convert) — pairs the storage u16 with the
// float. Also defined for f32↔u32 and f64↔u64.
let h2: f16 = f16::from_bits(0x3C00 as u16);   // IEEE half 1.0
let raw: u16 = h2.to_bits();
```

An `f16` literal is written with the `f16` suffix (`1.5f16`); an unsuffixed
literal also takes an `f16` annotation (`let h: f16 = 1.5;`). Arithmetic on
`f16` works (LLVM legalizes it), but the idiomatic hot-path pattern is to
convert to `f32`, compute, then convert back.

### Other primitives

- `bool`: `true` / `false`. **Cannot** be produced by an integer cast.
- `()`: the unit type, the implicit return of functions without an arrow.
- `str`: a string view (pointer + length), borrowed.
- `string`: an owned, heap-allocated string (provided by the stdlib).
- `*T`: a raw pointer. Operations require `unsafe`.
- `fn(...) -> R`: a function pointer.

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

Character literals (v0.0.9 Phase 2): `'a'` is a `u8` byte literal, a shorter way to write `97u8`. Backslash escapes `'\n'` `'\t'` `'\r'` `'\\'` `'\''` `'\"'` `'\0'` and the hex form `'\xFF'` all work. UTF-8 multi-byte codepoints (`'á'`) are rejected at parse time, since the type is `u8`, not a full Unicode codepoint. For UTF-8 use a `str`.

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

`let` lives inside a function. For named values shared across functions, or for the C-style "static storage" pattern where the value lives for the whole program's lifetime, use `const` or `static` at module scope.

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

1. **Initialiser must be a literal or `#zero::[T]()`**: integer, float, bool, string, a unary-negated numeric literal, or explicit zero-fill. Arithmetic (`const N: i32 = 1 + 2;`) is rejected with **E0X30**. Referring to another const or binding from the initialiser is the same error. A **`static`** additionally accepts an array literal or fill (`static T: [i64; 3] = [1, 2, 3];`, `static Z: [u8; 64] = [0u8; 64];`, nested arrays too) **and a (non-generic) struct literal** (`static S: Point = Point { x: 1, y: 2 };`, with struct-of-struct and array-of-struct composing recursively) — it becomes an LLVM constant aggregate, and bare numeric elements coerce to the declared field/element type. `const` stays literal-only (it is inlined at use sites).
2. **Type annotation is required**; there is no inference. `const FOO = 5;` and `static FOO = 5;` are rejected with **E0X31**.
3. **`static mut` reads need `unsafe`** (E0X33). Writes need `unsafe` (E0X34). Writing to an immutable `static` is **E0305** ("cannot assign to immutable static").

The choice between `const` and `static`:

| You want | Use |
|---|---|
| A named literal you'll reference at multiple sites | `const` |
| A module-private *fixed offset table* the program reads at runtime | `static` |
| A *mutable* counter / RNG state / lazy cache | `static mut` |

The C array-table pattern `static const int blck[42] = {1, 1, 32, ...};` is `static BLCK: [i64; 42] = [1, 1, 32, ...];` today, and the C struct-table pattern `static const sphere_t scene[10] = {...};` is `static SCENE: [Sphere; 10] = [ Sphere { ... }, ... ];` (array literals/fills and non-generic struct literals are both admitted as static initialisers). The C `static uint32_t rng_state` pattern is `static mut RNG_STATE: u32 = ...;` today.

---

## 5. Operators and arithmetic

### Default arithmetic: overflow-checked in debug, wraps in release

```cplus
let a: i32 = 10 + 20;
let b: i32 = 10 * 30;
let c: i32 = 10 / 3;        // 3
let d: i32 = 10 % 3;        // 1
let e: i32 = 10 - 20;       // -10
```

Division by zero **always** traps, in both modes.

### Wrapping operators: always wrap

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

### Casts: every width change is explicit

```cplus
let x: i64 = 5;
let y: i32 = x as i32;
let f: f64 = (x as f64);
let z: usize = 10 as usize;
```

No `int → bool`. No silent narrowing. Pointer ↔ integer must go through `usize`, never `i32`.

---

## 6. Control flow

### `if`: statement and expression

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

### `for`: two flavours plus iterators

```cplus
// Range. 0..n is exclusive; 0..=n is inclusive.
for i in 0..10 { println(i); }

// C-style. Standard `for` with init / cond / step.
for (let mut i: i32 = 0; i < 10; i = i +% 1) {
    println(i);
}
```

Arrays are not directly iterable. Iterate by index:

```cplus
let arr: [i32; 4] = [10, 20, 30, 40];
for i in 0..4 {
    println(arr[i as usize]);
}
```

Iterator values from `gen fn` and stdlib iterator adapters also work in
`for ... in`; see §26.

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

Every function body **must** end with `return EXPR;`; there is no implicit tail return at the function level (the rule is E0333). Block expressions can still be tail expressions inside `return` and `let`:

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

### Struct literals

```cplus
let x: i32 = 1;
let y: i32 = 2;
let p: Point = Point { x: x, y: y };
```

There is no field shorthand today; write every `name: value` pair explicitly.

### Field visibility

```cplus
struct Public {
    pub value: i32,                     // visible to other modules
    internal: i32,                      // module-private
}
```

### Three receiver forms: preview

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

**Always write the type args at the source level**: `Option[i32]::Some(v)`, `Option[i32]::None`. Internal mangled names like `Option__i32` exist but are never user-typeable.

---

## 10. Pattern matching

`match` is **exhaustive**: missing a variant is a compile error (**E0340**).

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

### `if let`: extract on the happy path

```cplus
if let Maybe[i32]::Some(v) = m {
    println(v);
}
```

### `guard let`: pattern match or diverge

Pattern-match or diverge, without nesting a `match`.

```cplus
fn process(m: Maybe[i32]) -> i32 {
    guard let Maybe[i32]::Some(v) = m else { return 0 -% 1; };
    return v +% 1;          // `v` is in scope after the guard
}
```

The `else` block must **diverge** via `return`, `break`, `continue`, or `loop`. The compiler enforces that.

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

for i in 0..4 {
    println(a[i as usize]);
}
```

**Use small `[u8; N]` arrays for scratch buffers in hot loops.** They live on the stack (or in registers after SROA). `malloc` is real heap allocation; it dominates tight loops.

```cplus
fn make_key(buf: *u8, n: u32) -> u32 {
    let mut tmp: [u8; 10] = [0u8; 10];   // fill-array literal: ten zero bytes
    // fill tmp ...
    return 0;
}
```

### Fill-array literal `[EXPR; N]`

v0.0.11 added the fill-array literal `[EXPR; N]`: an array of `N` copies of `EXPR`, with `N` a literal `u32`. The codegen fast-paths the `[0u8; N]` / `[0i8; N]` zero-fill case to a single `llvm.memset` call (essential for kilobyte-scale stack buffers like `vendor/static-arena`'s 16 KiB / 64 KiB shapes); other shapes lower to a tight N-iteration store loop the optimizer unrolls.

```cplus
let zeros: [u8; 64]     = [0u8; 64];           // memset fast path
let ones:  [i32; 4]     = [1; 4];              // (1, 1, 1, 1)
let bytes: [u8; 16384]  = [0u8; 16384];        // 16 KiB zero buffer — single memset
```

The count must be a `u32` literal; there is no const-eval today.

Slices `T[]` are fat-pointer views over contiguous elements. They are
borrow-shaped, so they are useful at FFI boundaries and inside the stdlib:

```cplus
let xs: i32[] = unsafe { slice_from_raw_parts(ptr, 3 as usize) };
let p: *i32 = slice_ptr(xs);
let n: usize = slice_len(xs);
```

---

## 12. Ownership

Ownership is the part of C+ that differs most from C. There is **no `&T` and no `&mut T`**. Borrowing is expressed by *parameter markers*, not by reference types.

### The parameter forms

v0.0.10 flipped the default: **non-Copy values move by default**. `borrow` is the opt-out for "caller keeps ownership". The previous `borrow`-by-default model proved too easy to footgun (the no-marker shape silently aliased; you needed `move` to consume, and forgetting `move` ran Drop on both sides). Now the no-marker shape consumes, which mirrors what most calls actually want, and `borrow` makes the rare "don't take ownership" case explicit.

| Form | On non-Copy types | On Copy types |
|---|---|---|
| `x: T` | **Move** — caller can't use the value after the call | Pass-by-value copy |
| `mut x: T` | Exclusive borrow — function may mutate; mutations propagate back | Pass-by-value, locally mutable |
| `move x: T` | Move (explicit; same as `x: T`) | Pass-by-value |
| `borrow x: T` | Shared borrow — caller keeps ownership, function reads only | (redundant on Copy) |

Method receivers mirror the param forms, with one deliberate difference in the *default*:

| Receiver | Meaning |
|---|---|
| `self` | Shared borrow — read-only access; caller keeps ownership |
| `mut self` | Exclusive borrow — may mutate; mutations propagate back to the caller |
| `move self` | Move — consumes the receiver; caller can't use it after |

There is no `borrow self`; bare `self` already *is* the shared borrow.

**Why bare `self` reads but a bare `x: T` param moves.** This asymmetry is intentional: each defaults to its common case. Most method calls (`p.len()`, `v.get(0)`) only want to *look* at the receiver, so bare `self` is a borrow. Most function calls hand a value *over* to the callee, so a bare param consumes. When you want the other behaviour, you say so: `move self` to consume a receiver, `borrow x: T` to borrow a param. The marker is always visible in the signature, so the reader never has to guess.

There is also a second axis, only relevant to `mut`: on a **Copy** type, `mut x: T` (or `mut self` on a Copy struct) is *local mutability*, where the callee gets its own copy and the caller's value is untouched. On a **non-Copy** type it's an exclusive borrow and the mutations *do* propagate back. Same syntax, but `Copy`-ness decides whether the caller sees the change. (`Copy` is structural, described next, so you can always tell which case you're in from the type.)

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

### `restrict`: opt-in `noalias` for raw pointer params

v0.0.8 addition. The borrow checker doesn't reason about `*T` raw pointers, so cpc emits just `noundef` on a raw-pointer param: LLVM has to assume any two pointer args may alias. For numeric hot paths (gemm, axpy, image / audio loops) that's a real perf tax: the autovectorizer inserts a runtime alias check + scalar fallback.

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

Hot-loop size (instructions) on an axpy kernel: **21 with `restrict` vs 36 without**. The savings are LLVM dropping the runtime alias check + scalar fallback.

Rules:
- Only valid on `*T` (raw pointer) params. Other shapes (`x: T` borrows, value-typed params) fire **E0411**.
- No `unsafe` required at the declaration site; `restrict` is a contract about the body, not a use-site assertion. Violations manifest as UB through the existing `unsafe` requirement on pointer ops.
- Composes with `mut` (e.g. `restrict mut p: *f32`, where the caller may write through `p` and `p` doesn't alias anything else). Each marker is orthogonal.
- C ABI compatible: LLVM `noalias` is an optimization hint, not part of the calling convention. A `pub extern fn` with `restrict` params exports the same C signature as without, so C callers see plain pointers.

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

### When to use `borrow`

Since non-Copy params move by default, the question flips: when does the callee *not* need to consume the value? Use `borrow`:

```cplus
// Default: x moves in. Caller can't use `s` after the call.
fn echo(x: string) -> string { return x; }

// Caller keeps `s`; callee only reads. Function must produce its own
// `string` if it wants to return one — typically via `.clone()`.
fn label(borrow x: string) -> string { return x.clone(); }

let s: string = "hello".to_string();
let r: string = label(s);     // s still usable after this call
println(s.as_str());
println(r.as_str());
```

`move x: T` is now redundant on non-Copy params (same as the default); it's kept as an explicit marker for readers who want the consumption visible at the signature. Old code with `move x: string` still works; new code can drop it. The two lower identically: both pass the value by value, flip the caller's drop flag, and make the callee responsible for the single drop. (This is a genuine move, not the old borrow-shaped lowering: forwarding a moved value back out, `fn f(x: T) -> T { return x; }`, frees the heap exactly once.)

The compiler suggests `borrow` when it spots a `borrow`-shaped use (read-only, no consume) inside a default-move body (**E0902** points to a precise fix-it).

### Partial moves out of a `Drop` type are rejected

A `Drop` type's destructor frees its fields by hand (the compiler does not synthesize per-field drops; see §13). So moving a field *out* from under a live destructor would double-free it. The compiler rejects this with **E0509**:

```cplus
struct Pair { a: string, b: string }
impl Pair { fn drop(mut self) { /* frees a and b */ } }

fn steal(p: Pair) -> string {
    return p.a;          // ❌ E0509 — `p`'s drop would free `a` again
}
```

The fix is to clone the field, or restructure so the value isn't owned by a `Drop` type. Moving a field out of a struct that has **no** `drop` impl is fine (no destructor, no double-free).

### Lifetime annotations (rare)

Most cases elide. When the compiler genuinely can't infer relations between borrows, name a region:

```cplus
fn longest(a: borrow A string, b: borrow A string) -> borrow A string {
    if a.len() > b.len() { return a; }
    return b;
}
```

`A` is a region name local to one signature; there's no separate declaration block. You will rarely write these.

The region is enforced, not decorative: a return region must be declared on some parameter (**E0511** otherwise), and a `return` must hand back a borrow from a *same-region* parameter (**E0512** on a mismatch, e.g. returning a `borrow B` value where the signature promised `borrow A`). And a function may not return a `str` / `T[]` view of one of its own **locals**: that local is freed when the function returns, so the view would dangle (**E0513**). Borrow a parameter (or return an owned `string` / `Vec[T]`) instead:

```cplus
fn bad() -> str {
    let s: string = "hi".to_string();
    return s.as_str();          // ❌ E0513 — view into a local that drops here
}
```

### What the compiler checks, and what it trusts

C+ ownership is **boundary-checked, not whole-program inferred**. There is no lifetime variable woven into your types: a `str` is just `{ptr, len}`, a `T[]` is `{ptr, len}`, and a region name like `A` is local to a single signature. That keeps the model simple and local, but it means it's worth knowing exactly where the compiler *enforces* a rule and where it *trusts you*. (A careful reader will ask "where is that borrow information stored, and what stops it escaping?" Here is the honest answer.)

**Enforced by the compiler:**

- **Use after move**: once a non-Copy value moves (into a default/`move` param, a `let`, or a struct field), the source is dead; reading it is **E0335**.
- **Aliasing XOR mutation**: within a function, a place has either shared borrows *or* one exclusive borrow, never both (§14).
- **Partial move out of a `Drop` type**: rejected (**E0509**); the destructor frees fields by hand, so stealing one would double-free.
- **Returned borrows**: a `str` / `T[]` / `borrow REGION` result must come from a parameter (with a matching region: **E0511** / **E0512**) or from `'static` data, never from a local that drops at return (**E0513**).
- **Borrows across `await`**: borrow-shaped params are banned in `async fn` (**E0900**), since a suspension can outlive the caller's frame.

**Trusted to you (the escape hatches):**

- **A `str` / `T[]` view stored into a longer-lived place.** These are `Copy` views, not tracked references. The compiler checks the *function boundary* (the rules above), but once you copy a view into a struct field, a `static`, or another binding, it no longer tracks that the backing storage outlives it. The contract is simple: **a view must not outlive the value it points into.**
- **Raw pointers (`*T`).** Completely outside the borrow checker: returning, storing, or aliasing one is allowed. The `unsafe` you write at each *dereference* is the point where you take on the validity obligation. A `*u8` returned from borrowed data and used after the source drops is a use-after-free that the language deliberately does not stop; that's the cost of the escape hatch.

One rule covers all of it: **a borrow, a view, or a raw pointer must not outlive the value it points into.** The compiler proves this for you at the enforced cases above; everywhere else it's a contract you keep, and `unsafe` marks the places where you've explicitly signed up for it.

---

## 13. Drop and `defer`

### Drop: your destructor

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

Defining `drop` makes the type non-`Copy`, which is necessary because copying a thing that owns a resource would lead to double-free.

### Raw-pointer fields are the author's responsibility

A `drop` frees its struct's fields **by hand** — the compiler does not synthesize per-field drops (see §12). For a raw-pointer field (`*T`) it cannot tell whether the struct *owns* the memory (and must free it) or only *borrows* it, so today it checks nothing: freeing what you own, and not freeing what you don't, is on you.

```cplus
struct Buf { ptr: *u8, len: usize }
impl Buf {
    fn drop(mut self) { unsafe { free(self.ptr); } }   // you free what you own
}
```

> **Planned.** A raw-pointer *accountability* model — an `opaque` field marker for "not mine", and a compile error when an owned raw pointer has no direct release in its `drop` — is designed in [plan.opaque.md](plan.opaque.md) but not yet implemented.

### `defer`: run at scope exit, LIFO

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

`defer` and `Drop` share one scope-exit stack: they interleave in declaration order, popped LIFO at exit.

---

## 14. The borrow checker

The rule: **aliasing XOR mutability**. At any program point, a place has either any number of shared borrows OR exactly one exclusive borrow, never both.

```cplus
let mut v: vec::Vec[i32] = vec::new::[i32]();
v.push(1);
let n: usize = v.len();      // shared borrow — fine
let p: i32 = v.get(0);       // shared borrow — fine
v.push(2);                   // exclusive — but no live shared borrow now; fine
```

The compiler enforces this at compile time. The common errors:

- **E0372**: move out of a borrowed value
- **E0383**: read while exclusively borrowed
- **E0370** family: overlapping incompatible borrows

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

- `Ord`: total ordering (`<`, `==`)
- `Eq`: equality
- `Hash`: hashable (for maps)

Implement a bound with an `impl <Bound> for <Type>` block when you define a new type that needs to participate.

### Turbofish

When the compiler can't infer a type parameter, supply it explicitly with `::[T]`:

```cplus
let h = thread::spawn::[i32](worker);
let v = vec::with_capacity::[i32](16 as usize);
let s = #size_of::[Point]();
```

Use `::[T]` for free functions and associated functions. For module-level
stdlib constructors like `vec::new::[T]()` and `vec::with_capacity::[T](n)`,
the type arguments attach to the function name.

### Internal vs source names

The compiler monomorphises generic instantiations to mangled names like `Option__i32`. **These are internal.** Always write `Option[i32]::Some(v)` in source, both at value sites and at pattern sites.

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

### The verbose form: explicit `match`

```cplus
fn parse_or_zero(s: str) -> i32 {
    return match parse(s) {
        ParseResult::Ok(v)       => v,
        ParseResult::BadInput    => 0 -% 1,
        ParseResult::Overflow    => 0 -% 2,
    };
}
```

### The readable form: `guard let`

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
- Owned string parameters move by default (`x: string` consumes the caller's value). Use `borrow x: string` if the callee should only read.
- `str` parameters are **not allowed** in `async fn` signatures; pass `string` instead (E0900).
- A function can't **return** a `str` (or `T[]`) that views one of its own locals: the local drops at return and the view would dangle (**E0513**). Return an owned `string` / `Vec[T]`, or borrow from a parameter. A `str` borrowing a parameter or a string literal is fine.
- For interop with libc, `str_ptr(s)` gives you a `*u8` you can hand to `printf`, `write`, etc.

---

## 18. String interpolation

Phase 8 added `${EXPR}` interpolation inside string literals:

```cplus
let name: str = "world";
let n: i32 = 42;
let s: string = "hello ${name}, the answer is ${n}";
io::println(s.as_str());
```

The interpolation lowers to an owned `string`, so any expression you can write in a position that produces `str` / `string` / a number is interpolable. Format specifiers (`${x:04d}`) are **not** in v0.0.4; convert numbers to strings explicitly when you need formatting.

---

## 19. FFI — calling C

C+ emits standard object files. The system linker stitches them with anything `clang` would. The language-level interop primitive is `extern fn`.

### Declaring symbols

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
extern fn printf(fmt: *u8, ...) -> i32;   // varargs OK on extern only
```

### C-string literals: `c"..."`

C wants NUL-terminated `char*`. A `c"..."` literal is exactly that: a bare `*u8` pointing at a NUL-terminated `.rodata` blob. It removes the `"...\0"` + `str_ptr(...)` workaround that FFI (libc, JNI, Cocoa) otherwise needs.

```cplus
extern fn printf(fmt: *u8, ...) -> i32;

fn main() -> i32 {
    unsafe { printf(c"hello, %d\n", 42 as i32); }   // c"..." is a *u8
    let banner: *u8 = c"=== ready ===\n";
    unsafe { printf(banner); }
    return 0;
}
```

A `c"..."` is `*u8` (not the fat-pointer `str`), and is **safe to form** — it's just a pointer to static data; only *dereferencing* a raw pointer needs `unsafe`, as always. Same escapes as a normal string (`\n`, `\t`, `\xHH`, …); the NUL is appended for you. (For an owned, length-carrying string use `string`/`str`; `c"..."` is specifically the C-interop shape.)

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

**Raw pointers are outside the borrow checker, by design.** Unlike a `str`/`T[]` view, the compiler tracks nothing about a `*T`'s lifetime: you can return one, store it in a global, or alias it freely, and none of that is an error. That's the escape hatch that makes FFI possible. The flip side is that the validity obligation is entirely yours: a pointer into a value that has since dropped is a use-after-free the language will not catch:

```cplus
fn leak(borrow s: string) -> *u8 {
    return str_ptr(s.as_str());   // compiles — returning a *u8 is always allowed
}                                 // ...but the caller must not deref it after `s` drops
```

The `unsafe` you write at each dereference is exactly where you acknowledge taking on that obligation (see §12, "What the compiler checks, and what it trusts").

Raw pointers also have a few blessed helper methods:

```cplus
if p.is_null() { return 1; }
if p.is_not_null() { unsafe { p.write_zeroed(); } }
```

`is_null()` / `is_not_null()` are safe bit-pattern checks. `write_zeroed()` is
unsafe because it writes through the pointer.

### `unsafe { ... }`

Required for: pointer dereference, pointer indexing, `extern fn` calls, `str_from_raw_parts`, integer-to-pointer casts.

### Null pointers

The word `null` never appears. At FFI boundaries:

```cplus
let p: *u8 = unsafe { 0 as *u8 };
```

### `#[repr(C)]`: stable C layout

```cplus
#[repr(C)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}
```

Promises field order is preserved and padding/alignment matches the platform C ABI. **Always** use it on structs that cross an `extern fn` boundary by value.

### `#[link_name = "..."]`: multiple signatures, one symbol

When one C symbol has multiple typed shapes (the ObjC `objc_msgSend` pattern):

```cplus
#[link_name = "objc_msgSend"] extern fn msg_void(recv: *u8, sel: *u8);
#[link_name = "objc_msgSend"] extern fn msg_get_str(recv: *u8, sel: *u8) -> *u8;
```

Both resolve to `_objc_msgSend` at link time.

### Objective-C interop

Objective-C is the one non-C-shaped ABI that C+ treats as a first-class
systems target, because AppKit, Foundation, Metal, and MPS all sit behind it on
macOS. The low-level surface is still explicit: object handles are opaque
`*u8`, selectors are data, and message sends are unsafe calls.

The direct compiler intrinsics are:

```cplus
let sel: *u8 = #selector("setTitle:");
unsafe {
    let title: *u8 = #msg_send(button, "title") -> *u8;
    #msg_send(button, "setEnabled:", true);
}
```

`#selector("name")` registers and caches the `SEL`. `#msg_send(recv, "sel",
...) -> T` emits a typed `objc_msgSend` call with the return type you spell at
the call site. This matters on Apple ABIs: `objc_msgSend` must not be declared
as a C varargs function, and each call must have the shape the runtime expects.

Use `#[repr(C)]` for structs that cross the boundary (`NSPoint`, `NSSize`,
`NSRect`), `#addr_of(place)` when Cocoa wants an out-pointer, and `#[link_name]`
only when you need to bind a C/ObjC runtime symbol manually. Most application
code should import the typed packages instead: `vendor/appkit` wraps Cocoa, and
`vendor/metal` wraps Metal/MPS. Those packages keep the unsafe ObjC details at
the edge and expose normal C+ structs, methods, and `Drop`.

### Variadic ABI gotcha

If the C header says `int fcntl(int fd, int cmd, ...);` then the C+ extern **must** be variadic. On AArch64-darwin, named args go in registers but varargs go on the stack, so a fixed-arity declaration silently passes garbage:

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

### Stateful callbacks: the C convention

Fn pointers don't capture environment. For callbacks that need state, do what C does: pass `(fn_ptr, user_data: *u8)`, and have the library thread `user_data` back to you unchanged:

```cplus
extern fn libfoo_subscribe(cb: fn(*u8, i32), user_data: *u8);
```

---

## 21. Compile-time intrinsics

Every compiler-known builtin uses the `#name(...)` sigil: one uniform spelling, distinct from regular function calls. v0.0.11 Phase 4 completed the cutover; the old bare-name (`addr_of`), turbofish-shaped (`size_of::[T]()`), and `!`-suffix (`include_bytes!`) forms are now parse / sema errors.

The intrinsics fall into three families:

| Family | Intrinsics |
|---|---|
| Typed query primitives | `#size_of::[T]()`, `#align_of::[T]()`, `#addr_of(place)`, `#zero::[T]()` |
| Compile-time data embedding | `#include_bytes("path")`, `#include_str("path")`, `#env("NAME")` |
| CPU hints | `#cpu_relax()` |
| ObjC + GPU FFI (v0.0.10) | `#selector("name")`, `#msg_send(recv, "sel", ...) -> RetTy`, `#compile_shader("file.metal", "msl")` |

### `#addr_of(place)`: address of a place expression as `*T`

Returns `*T` where `T` is the type of the addressed place. **Unsafe**: wrap in `unsafe { ... }` because the returned pointer aliases existing storage and the borrow checker does not track its lifetime.

Use it when a C function needs to write through a pointer (`time`, `arc4random_buf`, `snprintf`, `localtime`, `objc_msgSend` with by-pointer args, etc.). Pre-`#addr_of` the only option was a malloc-write-free dance:

```cplus
extern fn time(t: *i64) -> i64;

fn now() -> i64 {
    let mut t: i64 = 0;
    unsafe { time(#addr_of(t)); }
    return t;
}
```

Zero runtime cost: the alloca pointer is reused directly; codegen emits no GEP, no load, no extra store.

**Rules**:

- Exactly one argument; no turbofish (`#addr_of::[T](x)` is **E0501**).
- Argument must be a place expression: a bare identifier, field access (`p.x`,
  `(*p).x`), index (`a[2]`), dereference (`*p`), or a chain of those. Call
  results and arithmetic temporaries are rejected.
- Must appear inside `unsafe { ... }`; otherwise **E0801**.

For a struct or array binding where you want a `*u8` (byte pointer), cast the result: `#addr_of(my_struct) as *u8`.

```cplus
let p: *Point = unsafe { #addr_of(point) };
let xp: *i32 = unsafe { #addr_of((*p).x) };
let item: *i32 = unsafe { #addr_of(arr[2]) };
```

### `#zero::[T]()`: all-zero value of type `T`

Returns a value whose bytes are all zero. It is safe and useful for C-style
aggregate initialization when you will fill selected fields afterward.

```cplus
let mut p: Point = #zero::[Point]();
p.x = 10;
```

`#zero::[T]()` is also accepted in `const`, `static`, and `static mut`
initializers.

### `#cpu_relax()`

Spin-loop CPU hint. It lowers to the platform pause/yield instruction where
available and to no code elsewhere. It is safe and returns `()`.

### `#size_of::[T]()` and `#align_of::[T]()`

Return `usize`. **Safe**: no memory access; LLVM folds the call to a constant at `-O1+`.

```cplus
let s_i32: usize  = #size_of::[i32]();          // 4
let a_i32: usize  = #align_of::[i32]();         // 4
let s_p:   usize  = #size_of::[Point]();        // structural, depends on fields
```

Used by user-level allocator libraries to compute byte counts for typed allocations:

```cplus
let bytes: usize = #size_of::[T]() *% (n as usize);
let p: *u8       = unsafe { malloc(bytes) };
let typed: *T    = p as *T;
```

Type-arg substitution propagates through monomorphization, so `#size_of::[T]()` inside a generic body produces the right constant for every instantiation.

### `#include_bytes("relative/path")`

Embeds the raw bytes of a file as a `*[u8; N]` where `N` is the file's byte length, known at compile time. Path resolution is relative to the *source file containing the call*, not the project root.

```cplus
fn main() -> i32 {
    let shader: *[u8; 2048] = #include_bytes("../shaders/double.metallib");
    let bytes: *u8 = unsafe { shader as *u8 };
    // pass to FFI, etc.
    return 0;
}
```

The bytes live in `.rodata`; writing through the returned pointer is UB. Two calls with the same resolved path share one global. The argument must be a string literal; variables fire **E0871** at sema time. Errors:

- **E0870**: path not found at compile time. Diagnostic carries the resolved absolute path.
- **E0871**: non-string-literal argument.
- **E0872**: file exceeds 64 MiB sanity limit.

Used by GPU recipes to embed `.metallib` / `.cubin` / `.spv` shader blobs, by ML packages to embed pretrained weights, and by anyone shipping baked-in fixtures.

### `#include_str("relative/path")`

Same shape, but returns a `str` (fat pointer view; see §17). The byte length is part of the type, so the file's UTF-8 size is implicit:

```cplus
fn main() -> i32 {
    let manifest: str = #include_str("config.txt");
    println(manifest);   // str_len(manifest) == file size
    return 0;
}
```

The bytes must be valid UTF-8: invalid byte sequences fire **E0875** at sema time with the byte offset of the first bad byte. Same `E0870` / `E0871` / `E0872` error path as `#include_bytes`.

Use case the `metal_compute` recipe surfaced: `#include_str("../shaders/double.metallib.size")` to read the byte count produced by `xcrun metallib` at build time, with no shell-side source patching needed.

### `#env("NAME")`

Read an environment variable at compile time. Returns a `str` pointing at a `.rodata` global that contains the variable's value as the compiler saw it.

```cplus
fn main() -> i32 {
    let greeting: str = #env("GREETING");   // resolved at sema time
    println(greeting);
    return 0;
}
```

```bash
GREETING="hi from build" cpc env_demo.cplus -o env_demo
./env_demo
# → hi from build
```

Useful for baking build-time config into a binary (sample count for a benchmark, version string, build hostname, etc.) without recompiling for every value change.

Errors:
- **E0903**: non-string-literal argument.
- **E0876**: environment variable not set when cpc was invoked.

There is no `#env_opt` for "missing → None" semantics; the strict form covers the build-time-config case cleanly. If you need optional behavior, set a sentinel (`FOO_VAR="" cpc app.cplus`) and check `str_len(#env("FOO_VAR")) > 0` at runtime.

### `#selector("name")`, `#msg_send(recv, "sel", ...) -> RetTy`, `#compile_shader("path", "msl")`

The v0.0.10 GPU + ObjC interop wedge. `#selector` registers a method name once and caches the `SEL` pointer; `#msg_send` synthesizes a typed call to `objc_msgSend` with the correct ABI; `#compile_shader` invokes `xcrun metal` + `xcrun metallib` at sema time and embeds the resulting `.metallib` bytes.

```cplus
unsafe {
    let app: *u8 = #msg_send(class, "sharedApplication") -> *u8;
    #msg_send(app, "activateIgnoringOtherApps:", true as bool);
}
```

These are the load-bearing primitives the `vendor/appkit` and `vendor/metal` bindings (incl. MPS) sit on top of; direct use is rare, so consume them through those packages.

---

## 22. Modules, imports, packages

### Single-file mode

A `.cplus` file compiled with `cpc file.cplus -o bin` has no imports; only intrinsics are available.

### Project mode

Every import declares **where** the module comes from. Bare paths are rejected: the resolver demands you say "local" or "vendored":

```cplus
// Local file at src/math.cplus
import "./math" as math;
math::area(2, 3);

// Vendored package — the path's first segment is the dep name from Cplus.toml
import "stdlib/io" as io;
io::println("hi");
```

- Local: starts with `./`, resolved relative to the current file.
- Vendored: first segment matches a `[dependencies]` entry, resolved from `vendor/<dep>/src/<rest>.cplus`.

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

The stdlib is consumed like any other vendored package: symlink `/Users/adel/Workspace/C+/vendor/stdlib` into your project's `vendor/stdlib`.

---

## 23. Attributes

Attributes are pure metadata: they flip flags the compiler reads. They never generate code, transform the AST, or run user logic.

### `#[test]`: register a test function

```cplus
#[test]
fn it_adds() {
    let r: i32 = add(2, 3);
    assert r == 5;
}
```

Run with `cpc test`. The `assert` intrinsic, in a test build, sets a failure flag; in a regular build, it traps.

### `#[repr(C)]`: stable struct layout (see §19)

### `#[link_name = "..."]`: symbol aliasing (see §19)

### `#[unroll(N)]` and `#[vectorize_width(N)]`: loop hints

Statement-level attributes that flow through to LLVM's loop optimizer as `!llvm.loop` metadata. Apply to `while`, `loop`, or `for` statements. `N` must be a literal in `[1, 256]`.

```cplus
#[unroll(4)]
while i < n {
    sum = sum + buf[i as usize];
    i = i +% 1;
}

#[vectorize_width(8)]
for i in 0..count {
    out[i as usize] = a[i as usize] * b[i as usize];
}
```

`#[unroll(N)]` asks LLVM to unroll the loop N times; `#[vectorize_width(N)]` hints the autovectorizer toward an N-wide SIMD shape. Marginal for general code; **load-bearing for tight inner loops** that the compiler doesn't choose well by default.

### Real-time contracts

**First, what "real-time" means here, because it's the most misunderstood word in systems programming.** Real-time is **not** about speed or throughput. It's about *predictability*: a real-time task must finish within a fixed deadline **every single time**, including its worst case. An audio callback that's usually fast but occasionally stalls for 3 ms produces an audible click; a control loop that misses its deadline once can crash the machine. Average speed is irrelevant; the *worst case* is everything. A slow-but-bounded function is real-time-safe; a fast-on-average function with an unbounded worst case is not.

So the enemy isn't slowness; it's *operations whose duration you can't bound in advance*. There are a few classic ones, and C+ gives you a **compiler-checked attribute** for each. They're not optimizations; they're promises the compiler verifies by walking the function's entire **transitive call graph** (the function *and everything it calls*), so a hidden allocation three calls deep is still caught.

- **`#[no_alloc]`** (**E0901**): no heap allocation anywhere in the call graph. *Why it matters:* `malloc`/`free` have an unbounded worst case. They walk free lists, can take an internal lock, may fall into a syscall to grow the heap, and can trigger a page fault. None of that is bounded, so a single allocation can blow a deadline. Rejects the libc allocators (`malloc`, `calloc`, `realloc`, `free`, …), any unmarked user callee, unknown externs, **and string interpolation** (`"x = ${n}"` lowers to a `string` allocation). A whitelist of known non-allocating leaves (`memcpy`, `strlen`, the libc math functions, `printf`, …) is allowed.
- **`#[no_block]`** (**E0907**): no operation that parks the thread. *Why it matters:* taking a contended mutex, waiting on a condvar, `sleep`, or a blocking `read` hands the CPU to the OS scheduler for an *unbounded* time, so the deadline is now at the mercy of whatever else is running. Rejects mutex/rwlock locks and condvar/barrier waits, `pthread_join`, the `sleep` family, `poll`/`select`/`kevent`, and blocking file/socket I/O, plus unknown externs. Non-blocking leaves (try-locks, pure math/memory ops) are fine.
- **`#[bounded_recursion]`** (**E0906**): the call graph must not cycle back to the function. *Why it matters:* if recursion depth depends on input, both stack usage and running time are unbounded, so there's no static deadline to prove.
- **`#[max_stack(N)]`** (**E0908**): the function's estimated stack frame (parameters + every typed local across all nested blocks, summed conservatively with the real ABI layout) must be ≤ `N` bytes. *Why it matters:* real-time threads often run on small, fixed, sometimes page-locked stacks; an oversized frame overflows or faults. Catches large `[u8; N]` scratch arrays and big by-value aggregates.
- **`#[realtime]`** bundles `#[no_alloc]` + `#[no_block]` + `#[bounded_recursion]`. (It does **not** include `#[max_stack]`; add that separately, since the byte budget is task-specific.)

Note what these attributes do *not* do: they don't make code faster, they don't reorder anything, they don't change codegen. They're pure verification: they reject a program that *could* miss a deadline, turning "I think this audio callback is real-time-safe" into a fact the compiler checks on every build.

```cplus
#[realtime]
#[max_stack(256)]
fn process_frame(input: *f32, output: *f32, n: usize, gain: f32) {
    // Verified: no heap allocation, no blocking call, no recursion cycle,
    // and a stack frame <= 256 bytes — anywhere in this function's call graph.
    // ... apply gain to the buffer ...
    return;
}
```

Project-wide enforcement is opt-in via the manifest. A `[profile.realtime]`
table synthesizes the contract attributes onto every function in *your* package
(dependencies are exempt), turning the per-function opt-in into a CI gate:

```toml
[profile.realtime]
deny_alloc          = true
deny_block          = true
deny_unknown_extern = true
stack_limit         = 4096
```

`cpc check` (no FILE argument) runs the whole-project front-end, including the
profile gate, and stops before codegen; it's the fast CI command.
`--diagnostics=json` emits machine-readable violations.

`Send` / `Sync` are tightened so the threadsafe contract is real: `Rc[T]` is
`!Send` + `!Sync` and `MutexGuard[T]` is `!Send`, so passing one to a `Send` /
`Sync`-bounded generic (e.g. `thread::spawn`) is rejected (**E0502**); `Arc[T]`
stays `Send` + `Sync`.

The real-time data-structure work lives in `vendor/rt`: a lock-free
`SpscRingU64` and a fixed `FixedPoolU64`, hot methods marked `#[no_alloc]` /
`#[no_block]`. Platform controls live in `vendor/rt_darwin`: `clock`
(monotonic-ns timestamps), `thread` (QoS scheduling priority), and `mem`
(`mlock`/`munlock` page locking), and each fallible op returns an explicit
`Result`. The demo is `proves/realtime_audio`, where a `#[realtime]` audio
callback uses an SPSC control channel, raises thread QoS before the hot loop,
records per-frame latency with the monotonic clock, and shows E0901 / E0907 /
E0908 firing when an allocation, blocking call, or oversized frame is introduced.

This is still soft real-time on normal operating systems. The roadmap is tracked
in [realtime.md](realtime.md); remaining follow-ups are the broad
"raw-pointer structs are `!Send`" rule (needs an `unsafe impl Send` opt-in),
the `rt_linux` / `rt_posix` siblings, and method-dispatch hardening for the
allocation checker.

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

### The safe pattern: partition + join

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

### Atomics: for the rare cases that can't partition

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

**Two guards in the same scope deadlock**: the borrow checker doesn't yet prevent this. Use block scopes to bound each guard's lifetime.

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

### Reactor: concurrent I/O (v0.0.5)

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

`time::sleep(ms)` suspends the current task; the reactor wakes it when the timer fires. Multiple sleeps run concurrently, so total wall-clock is `max(durations)`, not `sum`.

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

### `stdlib/io`: basic I/O

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

Both types are generic. Match on the variant. There's no `?` propagation; use `guard let`.

### `stdlib/vec`: growable vector

```cplus
import "stdlib/vec" as vec;

let mut v: vec::Vec[i32] = vec::with_capacity::[i32](16 as usize);
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

### `stdlib/hash_map`: `HashMap[K, V]` + the `StrIntMap` legacy alias

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

### `stdlib/fs`: file I/O

```cplus
import "stdlib/fs" as fs;

let r: result::Result[fs::File, result::IoError] = fs::open_read("data.txt");
guard let result::Result::Ok(f) = r else { return 1; };

let bytes = f.read_to_end();
guard let result::Result::Ok(buf) = bytes else { return 1; };
let n: usize = buf.len();

let w = fs::create("out.txt");
// File implements Drop — closes on scope exit.
```

### `stdlib/net`: TCP

```cplus
import "stdlib/net" as net;

// Client
let c = net::connect_tcp("127.0.0.1", 8080 as u16);
guard let result::Result::Ok(sock) = c else { return 1; };
let written = sock.write_all(payload);
let bytes = sock.read_to_end();

// Server
let l = net::listen_tcp(8080 as u16);
guard let result::Result::Ok(listener) = l else { return 1; };
let accepted = listener.accept();
```

v0.0.4 supports IPv4 with numeric IPs only. For hostname resolution use `gethostbyname` directly via FFI.

### `stdlib/env`: environment variables and argv

```cplus
import "stdlib/env" as env;
import "stdlib/vec" as vec;

let mut port: vec::Vec[u8] = vec::new::[u8]();
if env::var_into("PORT", port) {
    // bytes were appended to port
}

// argv access is platform-specific — on darwin via _NSGetArgc/_NSGetArgv.
```

### `stdlib/thread` and `stdlib/atomic`: see §24

### `stdlib/box`: single heap-allocated owned value

```cplus
import "stdlib/box" as box;

let b = box::new::[i32](42);
let v: i32 = b.unwrap();        // consumes b; exit-Drop frees the slot
```

### `stdlib/arc`: atomic refcounted shared ownership

```cplus
import "stdlib/arc" as arc;

let root = arc::new::[i32](7);
let c1 = root.clone();          // atomic refcount increment
let c2 = root.clone();
// All three drop normally; the last reference frees.
```

### `stdlib/rc`: single-threaded refcount

Same as `Arc` but non-atomic. Cheaper, single-thread only. `Rc[T]` is `!Send` and `!Sync`: the compiler rejects passing one to a `Send`/`Sync`-bounded generic such as `thread::spawn` (**E0502**). Use `Arc[T]` to share across threads.

### `stdlib/mutex`: pthread-backed mutual exclusion

See §24. Internally refcounted (collapses `Arc` into itself, since C+ has no `&T` to make `Arc[Mutex[T]]` work safely).

### `stdlib/channel`: typed message passing

```cplus
import "stdlib/channel" as channel;

let tx = channel::new::[i32]();
let rx = tx.clone();
// Channel handles can be cloned for multi-producer / multi-consumer use.
tx.send(42);
let v: channel::RecvResult[i32] = rx.recv();
```

### `stdlib/future`, `stdlib/executor`, `stdlib/reactor`, `stdlib/time`: see §25

### `stdlib/iterator`: see §26

### `stdlib/cow`: clone-on-write string

```cplus
import "stdlib/cow" as cow;

let c1: cow::CowStr = cow::from_view("hello");                 // borrows the literal
let c2: cow::CowStr = cow::from_owned("world".to_string());    // takes ownership
let n: usize = cow::len(c1);                                   // uniform read access
```

API is free-functions, a pre-v0.0.5 shape from when sema rejected `impl` on enum types. That restriction lifted in v0.0.5 Slice 2C, but the library hasn't been re-shaped yet. Method-style migration is on the v0.0.7+ stdlib polish list.

### `stdlib/range`: numeric ranges (used by `for in`)

The `0..n` syntax lowers to a value of type `Range[i32]` (or similar) defined here.

### `stdlib/marker`: marker traits

Type-level markers used by the compiler (`Copy`, `Send`, `Sync` framework). You rarely interact with these directly.

For the full set of blessed vendor packages beyond stdlib (`vendor/appkit`, `vendor/simd`, `vendor/arena`, `vendor/clap`, `vendor/json`, `vendor/log`, `vendor/metal`, `vendor/accelerate`, `vendor/static-arena`, `vendor/uuid`), see §28.

---

## 28. Vendor packages

Beyond `stdlib`, C+ ships a curated set of vendored packages: typed bindings to platform SDKs (Apple frameworks, ObjC runtime), self-contained utilities (allocators, parsers, loggers), and 3D-math helpers. They share `stdlib`'s deployment model: a directory under `vendor/`, a `Cplus.toml` manifest, a `<package-name>.cplus` library entry, and in-package `#[test]` fns runnable via `cd vendor/<pkg> && cpc test`.

To consume one, add `<name> = "*"` to your `[dependencies]` and `import "<name>/..." as alias;`. The driver walks one directory up from your project to resolve sibling vendor deps, so no per-package symlinks are needed beyond the canonical `vendor/` checkout.

(The real-time packages `vendor/rt` and `vendor/rt_darwin` follow the same
deployment model but are documented with the real-time contracts in §23.)

The eleven packages, in alphabetical order:

### `vendor/accelerate`: Apple CPU-SIMD numerics (BLAS, vDSP)

Bindings to Apple's `Accelerate.framework`: pre-tuned CPU numerics that already ship in every macOS binary. The "no GPU available" fallback path for matmul / matvec / dot / axpy, and the reference implementation when GPU results need checking.

Two sub-modules:
- `accelerate/cblas`: BLAS Level 1 / 2 / 3 (`sdot`, `ddot`, `saxpy`, `daxpy`, `sscal`, `dscal`, `snrm2`, `dnrm2`, `sasum`, `dasum`, `sgemv`, `dgemv`, `sgemm`, `dgemm`). Typed `Order` (RowMajor / ColMajor) + `Transpose` (NoTrans / Trans / ConjTrans) enums.
- `accelerate/vdsp`: element-wise + reductions: `vadd`, `vmul`, `vsmul`, `dotpr`, `meanv`, `maxv` and `_d` (f64) variants.

```cplus
import "accelerate/cblas" as cblas;

let x: [f32; 3] = [1.0f32, 2.0f32, 3.0f32];
let y: [f32; 3] = [4.0f32, 5.0f32, 6.0f32];
let dot: f32 = cblas::sdot(
    3 as i32,
    unsafe { #addr_of(x) as *f32 }, 1 as i32,
    unsafe { #addr_of(y) as *f32 }, 1 as i32,
);   // 32.0
```

### `vendor/appkit`: typed Cocoa/AppKit bindings

15 sub-modules covering Cocoa for desktop apps: `runtime`, `application`, `window`, `view`, `controls`, `text`, `containers`, `data`, `graphics`, `menu`, `dialogs`, `panels`, `toolbar`, `controllers`, `convert`. Closure-free callbacks via `Button::set_on_click(fn(*u8))` (the runtime stashes the fn on the sender via `objc_setAssociatedObject`). `appkit/convert` is the C+ ↔ ObjC data bridge for `string`, `Vec[T]`, NSData; primitives + `#[repr(C)]` structs (NSPoint / NSSize / NSRect) cross the boundary verbatim.

```cplus
import "appkit/application" as application;
import "appkit/window" as window;

fn main() -> i32 {
    let pool = application::AutoreleasePool::new();
    let app  = application::Application::shared();
    app.set_activation_policy(0 as i64);
    // ... build window + controls ...
    app.run();
    pool.drain();
    return 0;
}
```

Runnable reference: [`docs/examples/recipes/appkit_hello/`](docs/examples/recipes/appkit_hello/).

### `vendor/arena`: growable bump-pointer arena

Multi-chunk arena for "allocate many, free all" workloads (parsers, compilers, request-scoped allocations). `Arena::new(chunk_size)` rounds up to a chosen minimum; allocations stride through chunks, growing as needed. `reset()` (and auto-Drop) returns every chunk to the heap. OOM convention: raw APIs return `0 as *u8`; parallel `_opt` variants return `Option[*u8]`.

```cplus
import "arena/arena" as arena;

let mut a: arena::Arena = arena::Arena::new(4096 as usize);
let p: *u8 = unsafe { a.alloc_bytes(64 as usize) };
let s: str = a.alloc_str("hello");
// dropping `a` frees every chunk
```

### `vendor/clap`: argparse with a fluent builder

`App::new(name).version(...).about(...).arg(Arg::new("v").short("v").long("verbose").flag())`. Long options accept both `--name value` and `--name=value`; short flags accept the combined `-abc` form. `get_matches_from(args: Vec[str])` is the cross-platform entry point; `ArgMatches` exposes typed `is_present(name)`, `value_of(name) -> Option[str]`, `positional_count()`, `positional_at(i)`.

```cplus
import "clap/clap" as clap;

let app = clap::App::new("mytool")
    .version("0.1.0")
    .arg(clap::Arg::new("verbose").short("v").long("verbose").flag());
let m = app.get_matches_from(argv);
if m.is_present("verbose") { /* ... */ }
```

### `vendor/jni`: minimal JNI (Java Native Interface) bindings

`#[repr(C)]` mirrors of the JVM's `jni.h` dispatch tables — `JNINativeInterface`
(the `JNIEnv` function table) and `JNIInvokeInterface` (the `JavaVM` table) —
plus the `jint`/`jlong`/`jobject`/… type aliases. The JVM hands you a
pointer to one of these tables; you read a function pointer out of it and call
it through `env`, exactly as C's `(*env)->FindClass(env, ...)` does:

```cplus
import "jni/jni" as jni;

fn get_version(env: jni::JNIEnv) -> jni::jint {
    let f: fn(jni::JNIEnv) -> jni::jint = unsafe { (*env).GetVersion };
    return unsafe { f(env) };
}
```

It's deliberately "min": the `JNINativeInterface` table is defined through the
`Call*Method` family (enough preceding fields to keep every offset correct);
extend it for later methods (`GetMethodID`, `NewStringUTF`, array ops, …) by
appending fields in `jni.h` order. A layout `#[test]` pins the table sizes
(344 / 64 bytes on 64-bit) so a dropped or mistyped field is caught immediately.
This package is also the smallest demonstration that cpc handles
function-pointer struct fields and a type that references itself through a
pointer (`JNIEnv = *JNINativeInterface`, used inside the struct's own fields).

### `vendor/json`: typed-enum JSON parser + serializer

`Value` is a recursive enum: `Null / Bool(bool) / Number(f64) / Str(string) / Array(Vec[Value]) / Object(Vec[Member])`. Entry points: `json::parse(s: str) -> Result[Value, ParseError]` and `v.to_string() -> string` (shortest-round-trip number formatting; surrogate pairs handled). Accessors via `is_*` / `as_*` / `array_*` / `object_*`. Dropping the outer `Value` recursively drops payloads.

```cplus
import "json/json" as json;

guard let result::Result::Ok(v) = json::parse("{\"x\":42}") else { return 1; };
if v.is_object() {
    guard let option::Option::Some(x) = v.object_get("x") else { return 0; };
    println(x.as_number() as i32);   // 42
}
```

### `vendor/log`: leveled structured logger

Single-threaded stderr logger: `set_max_level(Level::Info)` + `set_use_colors(true)` configure once, then `log::trace/debug/info/warn/error(s)` at every call site. Zero malloc per call: ANSI escapes are `static str`; the timestamp buffer + `time_t` slot live on the stack via `#addr_of`.

```cplus
import "log/log" as log;

log::set_max_level(log::Level::Info);
log::set_use_colors(true);
log::info("server started on port 8080");
log::warn("config file missing; using defaults");
```

### `vendor/metal`: typed Metal + MPS bindings

Apple Metal compute via the `Foundation` + `Metal` + `MetalPerformanceShaders` frameworks. `metal::default_device()` returns a `Device`; chain into `new_command_queue()`, `new_buffer(len)`, `new_library_with_data(bytes)`, `new_compute_pipeline_state(fn)`. Every wrapper has a `Drop` impl that `objc_release`s the underlying object.

The `metal/mps` sub-module adds **MPS bindings** (v0.0.11): Apple's pre-tuned matmul / FFT / softmax. `MPSMatrixMultiplication` for batched gemm:

```cplus
import "metal/metal" as metal;
import "metal/mps" as mps;

guard let result::Result::Ok(dev) = metal::default_device() else { return 1; };
let q   = dev.new_command_queue();
let lhs = /* MPSMatrix wrapping an MTLBuffer */;
let rhs = /* ... */;
let out = /* ... */;
let mm  = mps::MatrixMultiplication::new(dev, false, false, M, N, K, 1.0f64, 0.0f64);
mm.encode_to(cmd_buf, lhs, rhs, out);
```

The v0.0.11 `test_2x2_matmul_identity_correctness` test exercises this end-to-end on real GPU hardware.

### `vendor/simd`: 3D math on f32x4 + integer lane helpers

Float geometry plus integer-widening lane ops. Four modules:
- `simd/vec3`: `Vec3` (lane-3-zero invariant) with `dot / cross / length / normalize / reflect / refract / lerp / clamp / ...`.
- `simd/vec4`: full 4-lane vector with `raw()` / `from_raw()` for matrix code.
- `simd/mat4x4`: column-major `[Vec4; 4]` with `mul_vec` (four `fma <4 x float>` ops) and `mul`.
- `simd/integer`: integer-widening lane helpers composed from the builtin Tier-1 SIMD primitives (`widen`/`low`/`high`/`swizzle`): `mull_i8`/`mull_lo_i8`/`mull_hi_i8` (widening multiply), `mlal_i8` (widening multiply-accumulate), `paddl_i8` (widening pairwise add), and `dot_i32` — a 16-lane signed-byte dot product accumulated in i32 (the composable answer to NEON `vdotq_s32`, exact where `i8x16.mul().sum()` would wrap). This is the lane surface quantized kernels build on.

```cplus
import "simd/integer" as si;
let acc: i32 = si::dot_i32(i8x16::splat(2i8), i8x16::splat(3i8));   // 96
```

```cplus
import "simd/vec3" as vec3;

let a = vec3::Vec3::new(1.0f32, 2.0f32, 3.0f32);
let b = vec3::Vec3::new(4.0f32, 5.0f32, 6.0f32);
let d: f32 = a.dot(b);     // 32.0
let c = a.cross(b);        // (-3, 6, -3)
```

Use when the SIMD shape should be visible at the source level; the scalar FMA codegen can beat explicit Vec3 SIMD on Apple Silicon when the 4th lane is wasted. Measure before betting on SIMD types.

### `vendor/static-arena`: fixed-size stack arena

Bump-pointer arena whose buffer lives entirely on the stack (or in `static mut` storage). Zero `malloc`, zero `free`; composes with the `#[no_alloc]` real-time contract. Ships two fixed shapes, `StaticArena16K` (16 KiB) and `StaticArena64K` (64 KiB), because C+ doesn't yet have const-generic struct params. The 16K shape has the full surface (`alloc_bytes`, `alloc_bytes_aligned`, `alloc_zeroed_bytes`, `alloc_str`, `reset`); the 64K shape is the same minus `alloc_str` / `alloc_zeroed_bytes`.

```cplus
import "static-arena/static-arena" as sa;

let mut a: sa::StaticArena16K = sa::StaticArena16K::new();
guard let option::Option::Some(p) = a.alloc_bytes_aligned(64 as usize, 8 as usize) else {
    return 1;   // OOM
};
// ... use p ...
a.reset();      // recover full capacity, reuse
```

Size ceiling ~128 KiB on the stack; for larger arenas, allocate in `static mut` and reference by pointer.

### `vendor/uuid`: RFC 4122 v4 UUIDs

Random UUIDs sourced from `/dev/urandom` (portable across macOS / Linux / BSD). `Uuid::new_v4() -> Option[Uuid]`, `Uuid::parse(s: str) -> Option[Uuid]`, `uuid.to_string() -> string` (infallible; formats into a stack `[u8; 37]` buffer).

```cplus
import "uuid/uuid" as uuid;

guard let option::Option::Some(u) = uuid::Uuid::new_v4() else { return 1; };
println(u.to_string().as_str());   // "550e8400-e29b-41d4-a716-446655440000"
```

---

## 29. C+ for LLMs

C+ is deliberately shaped so an LLM can produce useful systems code with a
small correction loop. The language avoids features that require hidden global
knowledge: no overload sets, no implicit conversions, no closures with capture
rules, no exceptions, no macros, no `null`, and no reference types. Most
meaning is visible in the local function signature: ownership markers are on
parameters, `unsafe` is written at the operation, imports name their source, and
generic arguments use `::[T]` instead of grammar-ambiguous `<T>`.

### Small surface, strong diagnostics

The compiler is expected to be part of the writing process. A model can emit a
first draft, run `cpc check` or `cpc build`, then use the diagnostic code and
span to repair the program. Errors are intentionally specific: E0302 means a
type mismatch, E0335 means use after move, E0340 means a non-exhaustive match,
E0801 means an unsafe operation needs an `unsafe` block, and so on. The
`--diagnostics=json` mode exposes the same information in a machine-readable
shape for editors and agents.

Formatting is also part of the contract. `cpc fmt` gives one canonical layout,
so repeated LLM edits do not accumulate formatting noise. `cpc fmt --check`
lets CI reject drift without arguing about style.

### Examples over lore

The repository is organized so examples are executable documentation:

- `docs/examples/` contains small language examples.
- `docs/examples/recipes/` contains task-shaped programs.
- `vendor/<pkg>` packages carry in-package `#[test]` functions.
- `cpc/tests/e2e.rs` is the source of truth for accepted and rejected compiler
  behavior.

When a generated snippet is uncertain, prefer proving it with `cpc check`,
`cpc build`, or a focused e2e-style test instead of relying on prose. This is
why examples avoid pseudocode when a runnable shape exists.

### Querying code as a graph

Navigating C+ by `grep` is lossy: text search cannot tell the `Point` struct from a local named `point`, follow `prefix::Item` to the module that defines it, or answer "who calls this". `cpc` exposes a resolved, typed **code graph** that answers these by symbol and type rather than by text. It is the same resolution, type, and call-reachability information the compiler already computes on every build, kept and made queryable instead of discarded.

```bash
cpc query def    math::area              # resolved definition site(s)
cpc query refs   Point::translate        # every use site
cpc query callers process_frame          # who calls it
cpc query call-hierarchy render --depth 3
cpc query type-at src/main.cplus:42:10   # type of a param/field/local under a cursor
cpc query members Vec                     # fields + methods of a type
cpc query context parse                   # one-shot edit pack: signature, callers, callees, referenced types
```

Every query returns JSON with clickable `file:line:col` locations, the same format diagnostics emit, so an agent acts on a result without parsing prose. Because the queries are resolved (not name-based), `math::area` and a local `area` are distinguished, and a method call binds to the concrete `Type::method` it dispatches to. The call and reference answers carry an explicit `unresolved` / `scope` field, so an agent knows exactly where coverage ends and a `grep` fallback is still needed. `cpc query` runs each lookup as a one-shot subprocess; for the agent loop, **`cpc mcp`** is a resident MCP server — it builds the graph once, keeps it warm, and exposes the queries as tools over stdio (newline-delimited JSON-RPC 2.0), so an agent calls `find_definition` / `find_references` / `find_callers` / `code_context` / `type_at` (and friends) directly. Point an MCP client at `cpc mcp` to give an agent resolved, typed C+ navigation in place of `grep`. (Folding the same index under `cpc lsp` so editor and agent share one graph is still to come.) For C+ navigation, query the graph before reaching for `grep`: it resolves names text search cannot.

A composite query returns a function's whole neighborhood in one call:

```bash
cpc query context sum_range
```
```json
{
  "kind": "context",
  "target": {
    "id": "src.geo::Shape::area",
    "kind": "method",
    "name": "area",
    "location": { "file": "src/geo.cplus", "line": 12, "col": 8 },
    "signature": "fn area(self) -> f64",
    "is_pub": true
  },
  "callers": [
    { "id": "src.main::render", "kind": "function", "name": "render",
      "location": { "file": "src/main.cplus", "line": 4, "col": 1 }, "is_pub": false }
  ],
  "callees": [
    { "id": "src.geo::Shape::perimeter", "kind": "method", "name": "perimeter",
      "location": { "file": "src/geo.cplus", "line": 18, "col": 8 }, "is_pub": true }
  ],
  "type_refs": [
    { "symbol": "src.geo::Shape", "kind": "type",
      "location": { "file": "src/geo.cplus", "line": 12, "col": 12 }, "in_context": "src.geo::Shape::area" }
  ],
  "unresolved": 0
}
```

One call gives an agent the signature, who calls it, what it calls, the types it touches, and how many calls inside it the graph couldn't resolve — instead of several `grep` passes and a guess at which `area` matched. Symbol IDs use source names (`src.geo::Shape::area`), never a monomorphized `area__Shape`, so a query answer is something you can paste straight back into source.

### Hand-emitted LLVM IR

`cpc` does not build IR through LLVM's C++ API. It emits textual LLVM IR from
Rust code, then hands that `.ll` to the normal LLVM/Clang toolchain for
optimization, assembly, and linking.

That choice is pragmatic for a young language and unusually friendly to agents:

- `cpc --emit-ll FILE` prints the exact pre-optimization IR.
- `cpc --emit-ll-opt FILE` shows what LLVM kept after optimization.
- `cpc --emit-asm FILE` shows the native output when performance or ABI details
  matter.
- IR diffs are plain text, so codegen bugs can be inspected and patched without
  knowing LLVM's C++ builder API.

The generated IR is not a public language surface, but it is an intentional
debugging surface. If a benchmark regresses, an agent can compare C+'s emitted
IR against C/Clang output, find redundant loads or missing attributes, and fix
the textual emission path in `cplus-core/src/codegen.rs`.

### Contracts an agent can check

C+ prefers explicit contracts that the compiler can reject. `#[repr(C)]` says a
type crosses an ABI boundary. `restrict` says raw pointer parameters do not
alias. `#[no_alloc]`, `#[no_block]`, `#[max_stack(N)]`, and `#[realtime]` say a
hot path must avoid allocation, blocking, an oversized stack frame, and
recursion cycles, and a `[profile.realtime]` manifest table applies them
project-wide so `cpc check` becomes a CI gate. These are useful for humans, but
they are especially useful for LLM-generated code because the compiler can turn
a vague requirement ("make this audio callback real-time safe") into concrete
diagnostics.

The practical loop is:

```bash
cpc fmt src/main.cplus
cpc build
cpc build --diagnostics=json
```

For imported project code, use `cpc build`; single-file `cpc check FILE` is for
import-free snippets. If an answer depends on current compiler behavior, the
compiler wins over this tutorial.

---

## 30. Tooling — `cpc`

```bash
cpc build                      # multi-file project (reads Cplus.toml)
cpc FILE.cplus -o BIN          # single-file build
cpc check FILE                 # parse + sema only — fast feedback (single file)
cpc check                      # whole-project front-end check (reads Cplus.toml,
                               #   enforces [profile.realtime]); no codegen — CI gate
cpc fmt FILE                   # canonical format in place
cpc fmt --check DIR            # CI mode — exits 1 on drift
cpc test                       # run #[test] functions + doctests
cpc lsp                        # start the language server
cpc graph                      # whole-project code graph as JSON (nodes + edges)
cpc query def SYMBOL           # resolved definition site(s)
cpc query refs SYMBOL          # every use site
cpc query callers FN           # who calls FN
cpc query callees FN           # what FN calls
cpc query call-hierarchy FN --depth N
cpc query type-at FILE:LINE:COL # type of a param/field/local at a position
cpc query members TYPE         # fields + methods of a struct/enum
cpc query symbols [FILE]       # outline of a file or the whole project
cpc query context FN           # edit pack: signature, callers, callees, referenced types
cpc mcp                        # resident MCP server over the graph (stdio JSON-RPC, for agents)
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

1. **Positive**: the program compiles and runs as expected.
2. **Negative-with-code**: the program rejects with a specific Exxxx code.
3. **End-to-end**: drives `cpc build` from start to finish.

See [cpc/tests/e2e.rs](cpc/tests/e2e.rs) for the canonical pattern.

Vendor packages run their own in-package `#[test]` fns the same way: `cd vendor/<pkg> && cpc test`. The driver auto-discovers `src/<pkg-name>.cplus` as the entry, propagates the package's `[link]` frameworks/libs to the test binary's link line, threads `static` initializers through the same path a real build does, and walks one directory up to find sibling vendor deps, so a package like `vendor/uuid` that depends on `stdlib` resolves it from `vendor/stdlib` without per-package symlinks.

---

## 31. Common error codes

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
| E0509 | Move of a field out of a `Drop` type | Clone the field, or restructure so it isn't owned by a Drop type |
| E0511 | Return type names a borrow region no parameter declares | Add a same-region parameter, or drop the region |
| E0512 | Returned borrow's region ≠ the declared return region | Return a borrow from a same-region parameter |
| E0513 | Returning a `str` / `T[]` view of a local that drops | Return an owned value (`string` / `Vec[T]`), or borrow from a parameter |
| E0801 | Operation requires `unsafe` | Wrap in `unsafe { ... }` |
| E0821 | Cannot take address of generic fn | Specify type parameters at the take-address site |
| E0876 | `#env("X")` — env var not set at compile time | Set the var when invoking cpc, or pick a different default |
| E0901 | `#[no_alloc]` violation | Function (or a callee) heap-allocates or interpolates a string — remove it or drop the contract |
| E0905 | Unknown compiler intrinsic `#name` | Typo; check the §21 list of supported intrinsic names |
| E0906 | `#[bounded_recursion]` violation | The call graph cycles back to the function — break the recursion |
| E0907 | `#[no_block]` violation | Function (or a callee) calls a blocking primitive — use a non-blocking API |
| E0908 | `#[max_stack(N)]` exceeded | Estimated frame > N bytes — shrink locals/arrays or raise the budget |
| E0900 | Borrow-shaped param in `async fn` | Use `string` / `Vec[T]` instead of `str` / `T[]` |
| W0001 | *(warning)* `sum()`/`product()` over narrow integer SIMD lanes — silently wraps | `.widen()` the lanes first, or use `simd/integer::dot_i32` |

Every diagnostic carries a span and often a machine-applicable suggestion. Use `--diagnostics=json` for tool consumption. **W**-prefixed codes are non-fatal warnings; the build continues.

---

## 32. Gotchas worth memorising

Common mistakes and their fixes.

### `borrow` is the opt-out for non-Copy params: `move` is the default

Since v0.0.10, `x: T` on a non-Copy type **moves** the caller's value into the callee. If the callee should only read, mark it `borrow`:

```cplus
// Default: x moves in; caller can't use `s` after the call.
fn echo(x: string) -> string { return x; }

// Caller keeps ownership; callee reads only.
fn label(borrow x: string) -> string { return x.clone(); }
```

Pre-v0.0.10 code that wrote `move x: string` to prevent a double-free is now redundant but harmless; new code can drop the marker.

### You can't move a field out of a `Drop` type

A `Drop` type frees its own fields by hand, so the compiler won't let you steal one out from under the destructor; it would double-free:

```cplus
struct Pair { a: string, b: string }
impl Pair { fn drop(mut self) { /* frees a, b */ } }

let p: Pair = make_pair();
let a: string = p.a;     // ❌ E0509 — clone it, or don't make Pair a Drop type
```

(A struct with **no** `drop` impl has no destructor, so moving a field out of it is fine.)

### You can't return a borrow of a local

A `str` / `T[]` view of a function-local owned value dangles once the local drops at return (**E0513**). Return an owned value, or borrow a parameter:

```cplus
// ❌ E0513 — view into `s`, which drops when `bad` returns.
fn bad() -> str { let s: string = "x".to_string(); return s.as_str(); }

// ✅ return the owned string instead.
fn good() -> string { return "x".to_string(); }
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

## 33. SIMD types

cpc ships fixed-width SIMD as primitive types. Nineteen widths cover the 128-bit and 256-bit families that map directly to NEON / SSE / AVX2 / AVX:

- **64-bit (sub-128) widths**: `i8x8`, `u8x8`, `i16x4`, `u16x4`, `i32x2`, `u32x2`, `f32x2` (the NEON D-register family; mainly produced by `.low()`/`.high()` and consumed by `.widen()`/`.combine()`)
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

### Methods: by element type

Arithmetic (all numeric widths): `.add(b)`, `.sub(b)`, `.mul(b)`, `.div(b)`.

Float-only: `.fma(b, c)` (fused multiply-add), `.sqrt()`, `.abs()`.

Signed-int-only: `.abs()` (a no-op on unsigned would be misleading, so it's rejected with **E0324**).

All numeric: `.min(b)`, `.max(b)` (NaN-as-missing for floats; signed/unsigned per lane type).

Integer-only: `.and(b)`, `.or(b)`, `.xor(b)`, `.not()`, `.shl(count)`, `.shr(count)` (logical for unsigned, arithmetic for signed; count is a literal `u32` in `0..lane_bits`).

### Lane-type conversion and reinterpret

Three associated constructors on the target type convert between lane shapes. They take the source vector as their argument and return the target type (the same `::new`/`::splat` call form):

```cplus
let i: i32x4 = i32x4::new(1, 2, 3, 4);
let f: f32x4 = f32x4::from_int(i);          // int → float, lane-wise (sitofp/uitofp)
let j: i32x4 = i32x4::from_float(f);        // float → int, truncates toward zero (fptosi/fptoui)

let bytes: u8x16 = u8x16::splat(255u8);
let signed: i8x16 = i8x16::reinterpret(bytes);   // same bits, different lane type (bitcast)
let shorts: i16x8 = i16x8::reinterpret(signed);  // same 128 bits, fewer/wider lanes
```

- `FLOATxN::from_int(v)` / `INTxN::from_float(v)`: the source must be a SIMD of the **same lane count and lane width** (`i32x4` ↔ `f32x4`, `i64x2` ↔ `f64x2`). Signedness of the integer side picks signed vs unsigned conversion. Mismatches are **E0324**.
- `TARGET::reinterpret(v)`: a bit-preserving cast. The source must have the **same total width** (e.g. `i8x16` → `i16x8`, both 128 bits); lane count and type may differ. A width mismatch is **E0324**. Safe — no memory access, no value change.

### Half-width splits, joins, widen, and narrow

These instance methods move between a full-width vector and its 64-bit halves, and between adjacent integer lane widths — the building blocks of integer widening pipelines (NEON `vget_low`/`vget_high`/`vcombine`/`vmovl`/`vmovn`):

```cplus
let v: i8x16 = i8x16::splat(3i8);
let lo: i8x8 = v.low();              // bottom 8 lanes (shufflevector)
let hi: i8x8 = v.high();             // top 8 lanes
let back: i8x16 = lo.combine(hi);    // join two halves, lo fills the low lanes

let wide:   i16x8 = lo.widen();      // each lane to the next int size up: sext (signed) / zext (unsigned)
let narrow: i8x8  = wide.narrow();   // each lane to the next int size down: trunc
```

- `.low()` / `.high()`: a full vector → its 64-bit half (same lane type, half the lanes). The receiver must have an even lane count.
- `.combine(other)`: two equal half-width vectors → a full-width one (twice the lanes); the receiver fills the low lanes.
- `.widen()`: each **integer** lane to the next size up (`i8x8` → `i16x8`, `u16x4` → `u32x4`), lane count unchanged. Signed lanes sign-extend, unsigned zero-extend. 64-bit or float lanes have nothing wider — **E0324**.
- `.narrow()`: each **integer** lane to the next size down by truncation (`i16x8` → `i8x8`), lane count unchanged. 8-bit or float lanes have nothing narrower — **E0324**.

Together these make a widening integer dot product (the core quantized-kernel op) expressible without a dedicated builtin — widen the operands, multiply, widen the products, accumulate, reduce:

```cplus
fn dot8(a: i8x8, b: i8x8) -> i32 {
    let prod: i16x8 = a.widen().mul(b.widen());   // products fit in i16, no i8 wrap
    let plo: i32x4 = prod.low().widen();
    let phi: i32x4 = prod.high().widen();
    return plo.add(phi).sum();
}
```

These are the lane primitives that let integer pipelines widen and dequantize; the scalar `as` cast works per-value but does not convert a whole vector.

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

A horizontal `sum()` / `product()` returns the **lane** type, so on narrow integer lanes (`i8`/`u8`/`i16`/`u16`) it cannot hold the reduction and silently wraps — the classic `i8x16.mul().sum()` mistake. The compiler emits a **W0001 warning** (non-fatal) at that site; the fix is to `.widen()` the lanes first, or use `simd/integer::dot_i32` (§28) for a widening dot product. Same-width arithmetic and reductions stay legal — the warning just flags the overflow-prone shape.

`swizzle` needs **literal** indices. For a **runtime** index vector, use `table` on a 16-byte SIMD (NEON `vqtbl1q`):

```cplus
let t:   u8x16 = u8x16::splat(0u8);   // a 16-entry byte lookup table
let idx: u8x16 = u8x16::splat(3u8);   // per-lane indices (runtime values)
let r:   u8x16 = t.table(idx);        // r[i] = t[idx[i]]; out-of-range index -> 0
```

`tbl.table(idx)` requires a 16-byte receiver (`i8x16`/`u8x16`) and a `u8x16` index vector, and returns the receiver's type. On aarch64 it lowers to a single `vqtbl1q`; elsewhere to a per-lane gather with the same out-of-range-zeroing. This is the primitive behind 4-bit nibble dequant (expand packed nibbles through a lookup table).

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

**Mask types are distinct from integer SIMD.** `mask32x4` is its own `Ty::Mask`, not an alias for `i32x4`. Sema enforces:

- Comparison ops (`.lt` / `.gt` / ...) on a numeric SIMD return `mask{N}x{M}`, NOT `i{N}x{M}`.
- `.select` / `.any` / `.all` require a mask receiver; calling them on a plain integer SIMD fires **E0324**.
- Arithmetic on masks (`.add` / `.sub` / `.mul` / ...) is rejected (the 0/all-ones bitmask invariant has no useful arithmetic).
- Mask ↔ Simd assignment is rejected (**E0302**); no implicit coercion.
- `mask{N}x{M}::splat` / `::new` / `::from_array` are rejected: masks are produced by comparisons, never lane-by-lane.

Cross between the two with explicit, zero-cost methods:

```cplus
let m: mask32x4 = a.lt(b);
let bits: i32x4 = m.to_bits();      // mask → signed-int SIMD (no-op at LLVM level)
let m2: mask32x4 = bits.to_mask();  // signed-int SIMD → mask (same)
```

Bitwise ops (`.and` / `.or` / `.xor` / `.not`) work on masks for mask combining; they preserve the receiver kind.

At the LLVM level masks lower to `<N x iN>` exactly like the matching signed-int SIMD; the distinction is type-system-only, kept for safety. Codegen is identical, so adding the type check costs nothing at runtime.

### `#[repr(C)]` boundaries: SIMD does NOT cross by default

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

### Worked example: dot product

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

`av.fma(bv, acc)` lowers to one `@llvm.fma.v4f32` call; `acc.sum()` to one `@llvm.vector.reduce.fadd.v4f32`. On AArch64-darwin, the inner loop emits `fmla.4s v0, v1, v2`, the native NEON fused multiply-add on four floats. See [`docs/examples/recipes/simd_dot/`](docs/examples/recipes/simd_dot/) for the full reference port.

### When (and when not) to reach for SIMD

Reach for SIMD when:

- You have a tight loop over a homogeneous primitive array (`[f32; N]`, `[i32; N]`, `[u8; N]`).
- The operation is lane-independent (vector arithmetic, lane permutation, mask-driven blends).
- You want the SIMD shape to be **visible** at the source level: explicit `f32x4::new` + `.mul` reads better than hoping the autovectorizer fires on a scalar loop.

Don't reach for SIMD when:

- The data has irregular structure (struct-of-struct, pointer-chasing graphs).
- The hot loop is already vectorized well by LLVM at `--release` (check `--emit-asm`).
- You'd need to fight the type system to express it. Keep SIMD where it's natural.

---

## 34. Where to go next

In rough priority order:

1. **Read a recipe.** [docs/examples/recipes/](docs/examples/recipes/) ships task-oriented complete programs:
   - `file_read`, `file_write`, `stdin_lines`: basic I/O
   - `argv_parse`, `env_var`: process input
   - `hash_table`: full hashmap usage
   - `tcp_client`, `tcp_server`, `http_get`: networking
   - `json_parse`: parser example
   - `parallel_sum`: safe concurrency (partition + join)
   - `concurrent_counter`: unsafe concurrency (shared `*u64` + atomic fetch_add)
   - `async_compute`, `async_fetch`, `async_yield_demo`: async patterns
   - `simd_dot`: `f32x4` dot product, NEON `fmla.4s` end-to-end
   - `metal_compute`: GPU compute dispatch via ObjC interop + `#include_bytes`-embedded `.metallib` (macOS, needs the Metal toolchain)
   - `appkit_hello`: Cocoa GUI app in pure C+ via `vendor/appkit` + the convert bridge
2. **Read an example.** Every file in [docs/examples/](docs/examples/) compiles and runs.
3. **Read a design note.** [docs/design/](docs/design/) has per-phase deep dives: pattern matching, generics, borrow rules, FFI, async, and the v0.0.6 "external-package enable" doc that explains the stdlib-model bet for SIMD and GPU.
4. **Run `cpc fmt`.** If your source doesn't round-trip, something is syntactically off.
5. **Read the diagnostic.** Every error code has a precise meaning. The compiler is the source of truth; this tutorial is a summary.

The compiler is the source of truth; this tutorial is a summary of it.
