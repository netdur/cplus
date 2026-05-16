# SKILL — writing C+ source

This document is the **skill** of writing correct C+ programs. It is written for an LLM (or human) who is about to write or edit C+ code and needs to know what's in the language, what's not, and what the gotchas are. It is NOT a tutorial, NOT the language spec, and NOT the contributor guide.

If you find yourself wanting more detail:
- **History and rationale**: [plan.md](plan.md) — every settled decision and its reason
- **Per-feature deep dives**: [docs/design/](docs/design/) — the design notes for each phase
- **Working programs**: [docs/examples/](docs/examples/) — every file here compiles and runs. When in doubt about syntax, *read an example first; don't guess*.
- **What's locked vs. open**: §11 of plan.md (the resolved log and the open-questions log)

---

## 1. What C+ is

C+ is a systems language. LLVM backend. Manual memory management, no GC. Rust-level memory safety via a borrow checker. ABI-compatible with C (one-way: C+ emits standard object files, but `.c` files don't compile). Designed to be **easy for LLMs to write correctly** — explicit beats clever, locality of reasoning is paramount, the type system is load-bearing.

File extension: `.cplus`. Compiler: `cpc`. Multi-file project layout: `Cplus.toml` manifest at the project root, source in `src/`. Imports are explicit path strings with a mandatory alias, **no `.cplus` extension**. Local files start with `./`: `import "./math" as math;` then `math::function()`. Vendored packages start with the dep name declared in `[dependencies]`: `import "stdlib/io" as io;` resolves under `vendor/stdlib/src/io.cplus`. Bare paths (no `./`, no matching dep) are rejected — every import must declare whether it's local or vendored.

---

## 2. Locked principles — DO NOT propose violating these

These are settled. Each was decided after real analysis (see plan.md and the memory files). Reopening them costs trust. Don't suggest workarounds or alternatives.

### 2.1 No `null`, anywhere

The word `null` does not exist in C+ source. Absence is modeled with `Option[T]`. No nullable types, no `?T` sugar, no nullable struct-field annotations.

```cplus
// ❌ NEVER:
let x: ?i32 = null;
fn find(name: str) -> ?User { ... }

// ✅ The C+ way:
let x: Option[i32] = Option[i32]::None;
fn find(name: str) -> Option[User] { ... }
```

**Corollary**: don't propose adding `?.`, `??`, `!`, or `strictNullChecks`-style flags. They exist in TS/Kotlin/Swift specifically to survive `null`; C+ doesn't have the problem.

**The only escape**: at FFI boundaries, raw pointers can be null. Spelled `0 as *T` inside `unsafe { ... }`. The word `null` still doesn't appear.

### 2.2 No closures, no lambdas, no anonymous functions

Functions must be named and declared at the top level. No environment capture. No `|x| x + 1`, no `(x) => x + 1`, no `fn(x) { ... }` expressions.

```cplus
// ❌ NEVER:
let cb = |x| x + 1;
button.on_click(fn(e) { println("clicked"); });

// ✅ The C+ way:
fn click_handler(e: *Event) {
    println("clicked");
}
button.on_click(click_handler);   // named fn coerced to fn pointer
```

For stateful callbacks (the closure-style use case), use the C convention: `(fn_ptr, user_data: *u8)`. The function takes an opaque pointer the library threads back unchanged.

### 2.3 No `&T` / `&mut T` reference types

The `&` token is not part of the language type syntax. Borrowing is expressed via parameter markers, not reference types.

```cplus
// ❌ NEVER:
fn read(x: &i32) -> i32 { return *x; }
fn write(x: &mut Counter) { x.value = x.value + 1; }

// ✅ The C+ way:
fn read(x: i32) -> i32 { return x; }              // shared borrow on non-Copy types; pass-by-value on Copy
fn write(mut x: Counter) { x.value = x.value + 1; } // exclusive borrow
fn consume(move x: BigThing) { /* x is moved */ }   // ownership transfer
```

See §4 for the full model.

### 2.4 No exceptions, no try-catch

Errors are tagged-union values, matched exhaustively. No `try`/`catch`, no `throw`, no unwinding, no `!T` error type, no `?` propagation operator.

```cplus
// ❌ NEVER:
try { do_thing()? } catch (e: ParseError) { ... }

// ✅ The C+ way:
enum ParseResult { Ok(i32), BadInput, Overflow }
fn parse(s: str) -> ParseResult { ... }

let r = parse("123");
return match r {
    ParseResult::Ok(v) => v,
    ParseResult::BadInput => 0 -% 1,
    ParseResult::Overflow => 0 -% 2,
};
```

The `if let` / `guard let` sugar (§5) makes chained fallible code readable without inventing error-flow magic.

### 2.5 No implicit conversions

Every numeric width change requires explicit `as`. No int→bool. No silent narrowing.

```cplus
// ❌ NEVER:
let x: i64 = 5;
let y: i32 = x;       // E0302

// ✅ The C+ way:
let x: i64 = 5;
let y: i32 = x as i32;
```

### 2.6 No operator overloading, no function overloading

A function with a name has exactly one signature. `+` works on built-in numeric types only. User types use named methods.

```cplus
// ❌ NEVER:
fn add(a: i32, b: i32) -> i32 { ... }
fn add(a: f64, b: f64) -> f64 { ... }   // E0301 — duplicate definition

// ✅ The C+ way:
impl Vec3 {
    fn add(self, other: Vec3) -> Vec3 { ... }
}
let c = a.add(b);
```

### 2.7 No macros, no decorators, no comptime, no AST transformation

Attributes (`#[...]`) are **pure metadata** — they flip flags the compiler reads. They never generate code, transform the AST, or run user logic at compile time.

Phase-5+ blessed attributes: `#[test]`, `#[repr(C)]`, `#[link_name = "..."]`. New attributes need a design note explaining why they're declarative-only.

### 2.8 No `class`, no `function`, no `var`

C+ uses `struct` + `impl`, `fn`, and `let` / `let mut`. The TS-style alternatives were considered (Phase 9) and rejected because they either flip safety defaults, violate "no several ways to do the same thing", or pay parser complexity for purely visual wins. **Don't propose adding them as alternatives.**

### 2.9 No mutable-by-default

Bindings are immutable. `mut` is opt-in for both bindings and parameters.

```cplus
let x: i32 = 0;        // immutable (cannot reassign)
let mut y: i32 = 0;    // mutable
y = 5;                 // OK
```

### 2.10 Generics use `[T]`, not `<T>`

```cplus
// ❌ NEVER:
fn max<T: Ord>(a: T, b: T) -> T { ... }
let v: Vec<i32> = ...;

// ✅ The C+ way:
fn max[T: Ord](a: T, b: T) -> T { ... }
let v: Vec[i32] = Vec[i32]::new();
```

Use site: `Pair[i32, bool]::new(...)` and `Option[i32]::Some(7)`. Mangled internal names (`Option__i32`) are **implementation details** and must never appear in source.

### 2.11 Explicit `return`

Function bodies must end with `return EXPR;` — no implicit tail-expression return at the function-body level. Block expressions can still be tail expressions inside `let` initializers and `return` operands.

```cplus
// ❌ NEVER:
fn f() -> i32 { 42 }       // E0333

// ✅ The C+ way:
fn f() -> i32 { return 42; }
fn g() -> i32 { return if cond { 1 } else { 2 }; }
```

### 2.12 `::` for types, `.` for instances

Strict separation. Don't mix.

```cplus
let p = Point::new(3, 4);        // associated fn — ::
let x = p.x;                     // field — .
let m = p.magnitude_squared();   // method — .
Color::Red                        // enum variant — ::
```

---

## 3. Syntax in 10 minutes

### 3.1 Primitives

| Category | Types |
|---|---|
| Signed int | `i8 i16 i32 i64 isize` |
| Unsigned int | `u8 u16 u32 u64 usize` |
| Float | `f32 f64` |
| Other | `bool` `()` `str` `*T` (raw pointer) `fn(...) -> R` (fn pointer) |

No `int` or `long`. No implicit conversions between any of them.

### 3.2 Literals

```cplus
let a: i32 = 42;
let b: u64 = 42u64;            // typed integer literal
let c: f64 = 3.14;
let d: bool = true;
let e: str = "hello";          // string view, not owned string
let f: i32 = 0x1F;             // hex
let g: i32 = 0b1010;           // binary
let h: i32 = 1_000_000;        // underscore separators
```

### 3.3 Variables

```cplus
let x: i32 = 5;                // immutable; type optional with init
let x = 5;                     // inferred as i32
let mut y: i32 = 0;            // mutable
let z: i32;                    // uninitialized; sema enforces assignment before read
z = 7;                          // first write counts as init even without mut
```

### 3.4 Arithmetic

```cplus
// Default operators: trap on overflow in debug, wrap in release.
let a: i32 = 10 + 20;
let b: i32 = 10 * 30;
let c: i32 = 10 / 3;           // div-by-zero traps in both modes

// Wrapping operators: always wrap, regardless of build mode.
let d: u8 = 250u8 +% 10u8;     // 4 (wraps)
let e: i8 = 100i8 *% 3i8;      // overflow, wraps

// Bitwise + shifts on any integer width. Right shift on signed types
// is arithmetic (sign-preserving); on unsigned, logical (zero-fill).
let h: i32 = 0xff & 0x0f;      // 15
let i: i32 = 0xf0 | 0x0f;      // 255
let j: i32 = 0xff ^ 0xaa;      // 85
let k: i32 = 1 << 8;           // 256
let l: i32 = 256 >> 2;         // 64
let m: u32 = ~(0 as u32);      // 0xffffffff

// Byte-swap intrinsics (built-in, no FFI declaration needed).
// htons / htonl convert host-order → network-order on every C+ target.
let port_be: u16 = htons(8080 as u16);  // 0x901f on LE
let n32: u32    = bswap32(0x12345678 as u32);  // 0x78563412

// Comparisons return bool.
let f: bool = a < b;
let g: bool = a == b;          // strict equality, no coercion
```

**Raw-pointer arithmetic uses plain `+` / `-` (not `+%`).** `p + 1` advances
by one element width. `p +% 1` is a sema error — pointer offsets don't
participate in the wrapping-operator family.

### 3.5 Control flow

```cplus
// if as statement or expression.
if cond { ... } else if other { ... } else { ... }
let r: i32 = if cond { 1 } else { 2 };

// while.
while x < 10 { x = x +% 1; }

// for-range. Exclusive: 0..n. Inclusive: 0..=n.
for i in 0..10 { println(i); }

// for over an array.
let arr: [i32; 4] = [10, 20, 30, 40];
for v in arr { println(v); }

// C-style for.
for (let mut i: i32 = 0; i < 10; i = i +% 1) { ... }

// loop / break / continue.
loop {
    if done { break; }
    if skip { continue; }
}

// while let.
while let Option[i32]::Some(v) = next() { println(v); }
```

### 3.6 Functions

```cplus
fn name(x: i32, y: i32) -> i32 {
    return x +% y;
}

// No return type = unit.
fn print_it(n: i32) {
    println(n);
}

// pub for cross-file visibility (default private).
pub fn exported() -> i32 { return 42; }

// Generic.
fn identity[T](x: T) -> T { return x; }
fn max[T: Ord](a: T, b: T) -> T { ... }  // T must implement Ord

// extern (FFI to C). Body forbidden; symbol resolved at link time.
extern fn malloc(n: usize) -> *u8;
extern fn printf(fmt: *u8, ...) -> i32;   // varargs OK on extern fns only
```

### 3.7 Structs and impls

```cplus
struct Point {
    x: i32,
    y: i32,
}

impl Point {
    // Associated function (no receiver) — call via Point::new(...).
    fn new(x: i32, y: i32) -> Point {
        return Point { x: x, y: y };
    }

    // Instance method — call via p.translate(...).
    // `self` is the receiver. Three forms (see §4):
    //   self       = shared / by-value-on-Copy
    //   mut self   = exclusive / mutable
    //   move self  = consumes
    fn translate(mut self, dx: i32, dy: i32) {
        self.x = self.x +% dx;
        self.y = self.y +% dy;
    }
}

let mut p: Point = Point::new(0, 0);
p.translate(3, 4);

// Struct literal field-init shorthand when names match:
let x: i32 = 1;
let y: i32 = 2;
let p: Point = Point { x, y };

// `pub` field for cross-file access.
struct Public { pub value: i32 }
```

### 3.8 Enums (plain and tagged)

```cplus
// Plain enum (no payloads) — lowers to i32.
enum Color { Red, Green, Blue }
let c = Color::Red;

// Tagged enum (sum type). Variants may carry payloads.
enum Maybe[T] {
    Some(T),
    None,
}

let m: Maybe[i32] = Maybe[i32]::Some(7);
let n: Maybe[i32] = Maybe[i32]::None;

// Generic enum at use site: ALWAYS write the type args at the source level.
let r: Option[i32] = Option[i32]::Some(42);
// Mangled internal names like Option__i32 are NEVER source-typeable.
```

### 3.9 Pattern matching

```cplus
// match is exhaustive. Missing arm = E0340.
let n: i32 = match m {
    Maybe[i32]::Some(v) => v +% 1,
    Maybe[i32]::None    => 0,
};

// if let — sugar over match, for "happy path" extraction.
if let Maybe[i32]::Some(v) = m {
    println(v);
}

// guard let — pattern match or diverge.
fn process(m: Maybe[i32]) -> i32 {
    guard let Maybe[i32]::Some(v) = m else { return 0 -% 1; };
    return v +% 1;   // v is in scope past the guard let
}

// while let — loop until pattern fails.
while let Maybe[i32]::Some(v) = next() { println(v); }
```

### 3.10 Arrays

```cplus
let a: [i32; 4] = [10, 20, 30, 40];
let v: i32 = a[2];                          // 30; bounds-checked, traps on out-of-range
let mut a2: [i32; 4] = [0, 0, 0, 0];
a2[0] = 5;
```

Slices `T[]` and raw-pointer arithmetic exist as separate constructs; see §6 for FFI use.

---

## 4. Ownership model (§2.9 of plan.md — the truly unfamiliar part)

The single most important section. Read it before writing any non-trivial C+.

### 4.1 The three parameter forms

There is no `&T` / `&mut T`. Instead, parameters carry markers:

| Form | Non-`Copy` type semantics | `Copy` type semantics |
|---|---|---|
| `x: T` | Shared borrow — caller retains ownership, function reads only | Pass-by-value copy |
| `mut x: T` | Exclusive borrow — function may mutate; mutations propagate back | Pass-by-value, locally mutable |
| `move x: T` | Ownership transfer — caller gives up the value (E0335 if used after) | Silent bit-copy (today; future lint will suggest dropping `move`) |

Method receivers follow the same model: `self`, `mut self`, `move self`.

### 4.2 Copy is structural

A type is `Copy` iff every component is `Copy`. Primitives + plain enums are atomic-Copy. Aggregates auto-derive. A struct with `fn drop(mut self)` is forced non-`Copy`.

```cplus
struct Point { x: i32, y: i32 }            // Copy (all fields Copy)
struct WithDrop { v: i32 }
impl WithDrop { fn drop(mut self) { } }    // non-Copy (has Drop)
```

### 4.3 Return values always move

`fn f() -> T` transfers ownership of `T` to the caller. No return marker — moving is the only thing a return can mean.

### 4.4 Call sites carry no markers

`f(x)` is the syntax whether `f` borrows or consumes. The signature tells the story; the borrow checker enforces correct use.

### 4.5 Drop (destructors)

A struct with a method named `drop` runs that method at scope exit. The drop method signature is fixed: `fn drop(mut self)`, no return type.

```cplus
struct Buf { ptr: *u8, len: usize }
impl Buf {
    fn drop(mut self) {
        unsafe { free(self.ptr); }
    }
}
```

### 4.6 `defer`

Run an expression at scope exit (in LIFO order). Independent of Drop; both share one scope-exit stack.

```cplus
fn main() -> i32 {
    println(1);
    defer println(4);
    defer println(3);
    println(2);
    return 0;
}
// Output: 1\n2\n3\n4
```

### 4.7 The borrow checker enforces aliasing-XOR-mutability

At any program point, a place has either any number of shared borrows OR exactly one exclusive borrow, never both. Moving a value while it's borrowed is E0372. Reading a place while it's exclusively borrowed is E0383. **These are compile-time errors, not runtime exceptions.**

### 4.8 Lifetime annotations (rare)

Most cases elide. When sema can't infer, use `borrow REGION T`:

```cplus
fn longest(a: borrow A string, b: borrow A string) -> borrow A string {
    if a.len() > b.len() { return a; }
    return b;
}
```

`A` is a region name local to one signature; no separate declaration block. Composes with `mut` / `move`. You will rarely need to write these.

---

## 5. Error handling pattern

C+ has no exceptions. Fallible functions return a tagged-union result. Callers match.

```cplus
enum FileResult {
    Ok(i32),           // file handle
    NotFound,
    PermissionDenied,
}

fn open(path: str) -> FileResult { ... }

// The verbose form: explicit match.
fn read_or_zero(path: str) -> i32 {
    return match open(path) {
        FileResult::Ok(handle) => handle,
        FileResult::NotFound => 0 -% 1,
        FileResult::PermissionDenied => 0 -% 2,
    };
}

// The readable form: guard let chains.
fn process(path: str) -> i32 {
    guard let FileResult::Ok(handle) = open(path) else { return 0 -% 1; };
    // ... use handle ...
    return 0;
}
```

There is no `?` propagation operator. There is no `!T` magic type. **Don't propose adding them.** The FFI honesty principle (§2.8b in plan.md) rules out any surface syntax that implies machinery the C ABI doesn't carry.

---

## 6. FFI — calling C

C+ is one-way ABI-compatible: C+ emits standard objects, the system linker stitches with C-compiled objects. The only language-level interop primitive is `extern fn`.

### 6.1 Declaring external symbols

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
extern fn printf(fmt: *u8, ...) -> i32;   // varargs OK on extern only
```

### 6.2 Raw pointers

`*T` is an opaque address. Copy semantics (a pointer is 8 bytes). All operations on raw pointers require `unsafe { ... }`.

```cplus
let p: *u8 = unsafe { malloc(64 as usize) };
unsafe {
    p[0] = 65 as u8;          // store through pointer
    let b: u8 = p[1];         // load through pointer
    let q: *u8 = p + 1;       // pointer arithmetic strides by sizeof(T)
    free(p);
}

// Null pointer at FFI boundary — note: word "null" does not appear.
let null_ptr: *u8 = unsafe { 0 as *u8 };
```

### 6.3 `unsafe { ... }`

Required for: pointer dereference, pointer indexing, calls to `extern fn`, `str_from_raw_parts`, integer-to-pointer casts. Pointer arithmetic itself is safe (it's just math; no memory access).

### 6.4 Layout intrinsics (Phase 11 / 11.LAYOUT)

```cplus
let bytes: usize = size_of::[i32]();      // 4
let align: usize = align_of::[i32]();     // 4
let s: usize = size_of::[Point]();        // structural — depends on field layout
```

Both are safe, both take one type argument via turbofish, both return `usize`. Used by user-level `Allocator` libraries to compute byte counts for typed allocations.

### 6.5 `#[repr(C)]` for stable C-ABI struct layout

```cplus
#[repr(C)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}
```

Promises the layout matches C: field order preserved, padding/alignment per the platform C ABI. Use on every struct that crosses an `extern fn` boundary as a by-value parameter or return type.

### 6.6 `#[link_name = "..."]` for symbol aliasing (Phase 11 / 11.LINKNAME)

Used when one C symbol has multiple typed signatures (the ObjC `objc_msgSend` pattern):

```cplus
#[link_name = "objc_msgSend"] extern fn msg_void(recv: *u8, sel: *u8);
#[link_name = "objc_msgSend"] extern fn msg_get_str(recv: *u8, sel: *u8) -> *u8;
#[link_name = "objc_msgSend"] extern fn msg_init_window(
    recv: *u8, sel: *u8, frame: NSRect, mask: usize, back: usize, defer: i8,
) -> *u8;
```

All three resolve to the same `_objc_msgSend` symbol at link time.

### 6.7 Function pointers (Phase 11 / 11.FN_PTR)

```cplus
// Type position: fn(T1, T2) -> R, or fn(T1, T2) for unit return.
extern fn atexit(cb: fn()) -> i32;
extern fn signal(sig: i32, handler: fn(i32)) -> fn(i32);

// Value: bare ident in expected-FnPtr context coerces to fn pointer.
fn cleanup() { println(42); }
fn main() -> i32 {
    unsafe { atexit(cleanup); }   // pass C+ fn to C
    return 0;
}

// Struct fields work; the canonical "struct of callbacks" pattern.
struct Actions {
    on_click: fn(i32) -> i32,
    on_hover: fn(i32) -> i32,
}
let a: Actions = Actions { on_click: handle_click, on_hover: handle_hover };
let r: i32 = a.on_click(7);   // indirect call through field
```

C+ has **no closures** (see §2.2). Function pointers don't capture. For state-with-callback, use the C convention: `(fn_ptr, user_data: *u8)`.

### 6.8 String ↔ pointer bridges

```cplus
let s: str = "hello";
let p: *u8 = str_ptr(s);                     // safe: extracts the (ptr) field
let n: usize = str_len(s);                   // safe: extracts the (len) field
let v: str = unsafe { str_from_raw_parts(p, n) };  // unsafe: caller asserts validity
```

---

## 7. The standard library

**v0.0.3 Phase 1 (in progress):** `vendor/stdlib/` ships modules consumable via Phase 2's package resolution. The preferred way to print in a project-mode program is:

```cplus
import "stdlib/io" as io;

fn main() -> i32 {
    io::println("hello, world");
    io::eprintln("to stderr");
    return 0;
}
```

A project declares `stdlib = "*"` in `[dependencies]`; the consumer's `vendor/stdlib/` is a symlink or copy of the in-tree `vendor/stdlib/` package (Phase 2 MVP — no fetch tool yet).

**Available stdlib modules** (Phase 1 is filling these in incrementally):
- `stdlib/io` — `print(s)`, `println(s)`, `eprintln(s)`. ✓ shipped 2026-05-16.
- `stdlib/result` — `Result[T, E]` and `IoError` enums. (API only; consumers can construct/match.)
- `stdlib/fs` — `File` with open/read/write/close + Drop. (Bodies pending Slice 1B.)
- `stdlib/net` — `TcpStream`, `TcpListener`. (Bodies pending Slice 1C.)
- `stdlib/vec` — `Vec[T]`. (Surface polish pending Slice 1D.)
- `stdlib/hash_map` — `HashMap[K, V]`. (Bodies pending Slice 1D.)
- `stdlib/env` — `var`, `args`. (Bodies pending Slice 1E.)

**Compiler intrinsics that still exist** (single-file-mode fallbacks, plus low-level building blocks the stdlib itself uses):
- `println(n: i32)` / `println(s: str)` — magic intrinsic for **single-file mode** only. In project mode, prefer `stdlib/io::println` for one-canonical-way alignment.
- `str_ptr(s: str) -> *u8`
- `str_len(s: str) -> usize`
- `str_from_raw_parts(p: *u8, n: usize) -> str` — unsafe
- `size_of::[T]() -> usize`
- `align_of::[T]() -> usize`
- `assert EXPR;` (in `#[test]` builds — sets failure flag; in regular builds — traps)

**Decision rule:** if you're writing a single-file demo (`cpc file.cplus -o bin`), use the intrinsic `println`. If you're writing a project (`cpc build`), import `stdlib/io` — the project shape gives you cleaner error chains, drop integration, and forward-compatible idioms. Don't mix both in one project.

For types not yet in stdlib (`Option`, advanced collections, etc.), the user-level pattern still works: `extern fn malloc/free/memcpy` + `size_of[T]()` + raw pointers + generics + Drop. The reference example for owned strings: [docs/examples/owned_string.cplus](docs/examples/owned_string.cplus).

---

## 8. Common error codes

The codes you'll most often see in `cpc build` output. Full list: scan sema.rs / borrowck.rs / attrs.rs.

| Code | Meaning | Fix |
|---|---|---|
| E0300 | Undefined name | Typo, missing import, or forgotten `pub` |
| E0301 | Duplicate definition | Two items with the same name |
| E0302 | Type mismatch | Insert an `as` cast or change the declared type |
| E0303 | Unknown type | Typo, missing import, or generic param not in scope |
| E0312 | Function used as value | Assign to a `fn(...)`-typed binding to take its address |
| E0315 | Invalid cast | Some cast pairs are forbidden (e.g. int→bool) |
| E0319/0320/0321/0322 | Struct field issues (duplicate / unknown / missing / extra) | Match the struct's field declaration |
| E0325 | impl on unknown / non-struct type | The impl target must be a struct/enum declared in scope |
| E0327 | Wrong call form | `Type::method()` for assoc, `value.method()` for instance |
| E0333 | Implicit return | Add explicit `return EXPR;` at function-body level |
| E0335 | Use of moved value | Don't read after `move` |
| E0340 | Non-exhaustive match | Add the missing arm or `_ =>` catch-all |
| E0345 | Use of possibly-unassigned binding | Initialize on every control-flow path |
| E0353 | `break`/`continue` outside a loop | Move inside a loop body |
| E0354 | Unknown attribute | Typo (did-you-mean suggestion provided) |
| E0356 | Wrong attribute target | Some attrs are fn-only, others struct-only |
| E0370–0386 | Borrow checker conflicts | Read the message — each variant is specific |
| E0500 | Cannot infer type parameter | Use `name::[T1, T2](...)` turbofish |
| E0501 | Wrong type-arg count | Check the generic param list |
| E0502 | Bound not satisfied | `T: Ord` requires `impl Ord for T` |
| E0801 | Operation requires `unsafe` | Wrap in `unsafe { ... }` |
| E0821 | Cannot take address of generic fn | Specify type parameters at the take-address site |

Every diagnostic carries a span (line/col) and often a machine-applicable suggestion. The diagnostic JSON shape is stable; `--diagnostics=json` for tool consumption.

---

## 9. Tooling

The `cpc` binary lives at `/Users/adel/Workspace/C+/target/debug/cpc`
(the debug build of this repo). If `cpc` is not already on your `PATH`,
invoke it by absolute path or prepend that directory to `PATH`:

```bash
export PATH="/Users/adel/Workspace/C+/target/debug:$PATH"
# or just:
/Users/adel/Workspace/C+/target/debug/cpc build
```

To build it from source (only if the binary above is missing):

```bash
cargo build --manifest-path /Users/adel/Workspace/C+/cpc/Cargo.toml
```

`cpc build` reads `./Cplus.toml` in the current directory and writes the
final executable to `./target/debug/<bin-name>` (cargo-style layout). The
`<bin-name>` is the `name` field in the `[[bin]]` table of `Cplus.toml`.

### Linking against Apple frameworks (Cocoa / AppKit / Foundation / ...)

`cpc build` does **not** yet know about Apple framework search paths or
the ObjC runtime library — those are linker-level concerns. For any
program that needs `-framework X` or `-lobjc`, the workflow is:

```bash
# 1. Emit LLVM IR via cpc.
cpc --emit-ll src/main.cplus > out.ll
# 2. Hand off to clang for linking, with framework + library flags.
clang out.ll \
    -framework Cocoa \
    -lobjc \
    -Wno-override-module \
    -o my_binary
```

The `-Wno-override-module` silences a benign warning about clang seeing IR
that names a target triple it would have chosen anyway. Substitute the
framework you need (`Foundation`, `AppKit`, etc.). This is exactly the
pattern used by `objc-c-interop/cocoa-min/build.sh` in the parent project.

```bash
cpc build              # multi-file project (reads Cplus.toml)
cpc FILE.cplus -o BIN  # single-file build
cpc check FILE         # parse + sema, no codegen (fast feedback)
cpc fmt FILE           # canonical format in place
cpc fmt --check DIR    # CI mode — exit 1 on drift
cpc test               # run #[test] functions + doctests
cpc lsp                # start the language server
cpc --emit-ll FILE     # pre-pass LLVM IR (what cpc emitted)
cpc --emit-ll-opt FILE # post-pass LLVM IR (after clang's optimizer)
cpc --emit-asm FILE    # native assembly (after clang's optimizer)
cpc --diagnostics=json # structured diagnostic output
cpc --release          # -O2 (default is debug -O0 with overflow traps)
cpc -V                 # print version (alias: --version)
```

**Test pattern:** every new feature lands with at least three test cases — positive (program compiles and runs as expected), negative-with-code (program rejects with the specific Exxxx code), and an e2e test that drives `cpc build` end-to-end. See [cpc/tests/e2e.rs](cpc/tests/e2e.rs) for the canonical shape.

---

## 10. When in doubt

In rough priority order:

1. **Read a recipe.** [docs/examples/recipes/](docs/examples/recipes/) ships ten task-oriented `.cplus` programs (file I/O, stdin parsing, hash map, TCP client / server, JSON parser, HTTP GET, argv, env vars). Each is a complete `cpc build` project — the closest thing to "how do I do X" that exists. The 03-hello-appkit benchmark proved that a near-complete reference is worth more than a paragraph of prose; the recipes generalize that.
2. **Read an example.** Every file in [docs/examples/](docs/examples/) compiles and runs. The simplest sample that exercises the feature you're unsure about is more authoritative than this document.
3. **Read the design note.** [docs/design/](docs/design/) has per-phase deep dives. Recent additions: phase11-fn-pointers, phase10 FFI work, phase5 borrow-shared (the borrow checker is the most subtle part of the language), phase2-packages-mvp.
4. **Run `cpc fmt`.** If the source doesn't round-trip through the formatter, something is syntactically off.
5. **Read the diagnostic.** Every error code has a precise meaning. The compiler is the source of truth; this document is a summary.
6. **Check the locked principles in §2.** If you're about to suggest a feature, scan §2 first. If it's there, the answer is no.
7. **Consult plan.md §11.** The resolved-questions log records why settled decisions are settled. New requests that retread settled ground get the same answer.

The codebase is small. Reading it is feasible. **Don't guess; check.**
