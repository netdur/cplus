# SKILL — writing C+ source

Dense reference for an LLM about to write or edit C+ code. Not a tutorial; not the spec. For the friendly walkthrough see [tutorial.md](tutorial.md); for history see [plan.md](plan.md) and the per-cycle `plan-0.0.N.md` archives.

When in doubt about syntax, **read [docs/examples/](docs/examples/)** — every file there compiles and runs.

---

## 1. What C+ is

Systems language. LLVM backend. Manual memory, no GC. Rust-style borrow checker. One-way C ABI (cpc emits standard object files; `.c` doesn't compile). Designed for LLMs to write correctly: explicit beats clever, locality is paramount, the type system carries weight.

File extension `.cplus`. Compiler `cpc`. Project layout: `Cplus.toml` at root, sources in `src/`, deps in `vendor/`. Imports are explicit + aliased, no `.cplus` extension:

```cplus
import "./math" as math;          // local, starts with `./`
import "stdlib/io" as io;         // vendored, first segment is dep name
math::area(2, 3);
io::println("hi");
```

### Paths

```text
Compiler:  /Users/adel/Workspace/C+/target/debug/cpc
LSP:       /Users/adel/Workspace/C+/target/debug/cpc-lsp
Stdlib:    /Users/adel/Workspace/C+/vendor/stdlib   (symlink into your project)
Vendor:    /Users/adel/Workspace/C+/vendor/{appkit,accelerate,metal,simd,arena,clap,json,log,uuid,static-arena}
Examples:  /Users/adel/Workspace/C+/docs/examples/
```

### Scaffold a new project

```bash
mkdir -p my_proj/src my_proj/vendor && cd my_proj
ln -s /Users/adel/Workspace/C+/vendor/stdlib vendor/stdlib
cat > Cplus.toml <<'EOF'
[package]
name = "my_proj"
version = "0.0.1"
edition = "2026"
[[bin]]
name = "my_proj"
path = "src/main.cplus"
[dependencies]
stdlib = "*"
EOF
cat > src/main.cplus <<'EOF'
import "stdlib/io" as io;
fn main() -> i32 { io::println("hi"); return 0; }
EOF
/Users/adel/Workspace/C+/target/debug/cpc build && ./target/debug/my_proj
```

---

## 2. Locked principles — never propose violating

| # | Principle | What that means |
|---|---|---|
| 1 | No `null` | Use `Option[T]`. FFI null is `0 as *T` in `unsafe`. |
| 2 | No closures / lambdas | Named `fn` only. Stateful callbacks via `(fn_ptr, user_data: *u8)`. |
| 3 | No `&T` / `&mut T` types | Borrowing is a parameter marker, not a type. |
| 4 | No exceptions / `try` / `?` | Errors are tagged-union values; `match` or `guard let`. |
| 5 | No implicit conversions | Every width change needs `as`. |
| 6 | No overloading | One name, one signature. |
| 7 | No macros / decorators / comptime | Attributes are pure metadata. |
| 8 | No `class` / `function` / `var` | `struct` + `impl`, `fn`, `let`. |
| 9 | No mutable-by-default | `mut` is opt-in. |
| 10 | Generics use `[T]`, not `<T>` | Avoids `a<b>(c)` ambiguity. |
| 11 | Explicit `return` | No implicit tail returns at function level (E0333). |
| 12 | `::` for types, `.` for instances | Strict separation. |

Compact examples of the non-obvious ones:

```cplus
// 1 — Option, not null
fn find(k: str) -> Option[i32] {
    if k == "answer" { return Option[i32]::Some(42); }
    return Option[i32]::None;
}

// 2 — named fn + user_data instead of closure
fn on_tick(ud: *u8, n: i32) { /* ... */ }
extern fn lib_subscribe(cb: fn(*u8, i32), ud: *u8);

// 4 — exhaustive match on a user enum
enum Parse { Ok(i32), Bad, Overflow }
return match parse(s) {
    Parse::Ok(v)    => v,
    Parse::Bad      => 0 -% 1,
    Parse::Overflow => 0 -% 2,
};

// 10 — generics with [T], turbofish with ::[T]
let v = vec::Vec[i32]::with_capacity(16);
let h = thread::spawn::[i32](worker);
```

---

## 3. Syntax cheat sheet

### Primitives
`i8 i16 i32 i64 isize` · `u8 u16 u32 u64 usize` · `f32 f64` · `bool` · `()` · `str` (16-byte view) · `string` (heap-owned) · `*T` (raw ptr) · `fn(...) -> R`

### Literals
```cplus
let a: i32 = 42;          let b: u64 = 42u64;
let c: f64 = 3.14;        let d: bool = true;
let e: str = "hello";     let h: i32 = 1_000_000;
let f: i32 = 0x1F;        let g: i32 = 0b1010;
let ch: u8 = 'a';         // u8 byte literal; '\n' '\xFF' escapes supported
```

### Variables, const, static
```cplus
let x: i32 = 5;
let mut z: i32 = 0; z = 7;
let w: i32; w = 12;                  // late init; first write counts

const PI: f32 = 3.14159f32;          // literal-only initializer (E0X30 otherwise)
static OFFSET: i32 = 50;             // .rodata
static mut COUNTER: i32 = 0;         // reads + writes need unsafe { ... }
```

### Operators
- Arithmetic `+ - * / %` traps on overflow in debug, wraps in release. Division by zero **always** traps.
- Wrapping `+% -% *%` always wrap — use when you want it.
- Bitwise `& | ^ ~ << >>`. Shift-right on signed = arithmetic; on unsigned = logical.
- Compare `< <= > >= == !=` produce `bool`, no coercion.
- Cast `as` is the only width-change tool. Pointer ↔ int goes through `usize`.

### Control flow
```cplus
if cond { ... } else if other { ... } else { ... }
let r: i32 = if cond { 1 } else { 2 };
while x < 10 { x = x +% 1; }
for i in 0..10 { ... }                       // 0..n exclusive; 0..=n inclusive
for v in arr { ... }                          // array
for (let mut i: i32 = 0; i < 10; i = i +% 1) { ... }   // C-style
loop { if done { break; } continue; }
while let Option[i32]::Some(v) = next() { ... }
```

### Structs + methods + receivers
```cplus
struct Point { x: i32, y: i32 }
impl Point {
    fn new(x: i32, y: i32) -> Point { return Point { x, y }; }   // assoc fn
    fn read(self) -> i32 { return self.x +% self.y; }            // shared borrow
    fn translate(mut self, dx: i32) { self.x = self.x +% dx; }   // exclusive
    fn into_raw(move self) -> i32 { return self.x; }             // consumes self
}
struct Public { pub value: i32, internal: i32 }                  // field visibility
```

### Enums
```cplus
enum Color { Red, Green, Blue }                  // plain, lowers to i32, Copy
enum Shape { Circle(f64), Rect(f64, f64) }       // tagged
enum Maybe[T] { Some(T), None }                  // generic

let s = Shape::Circle(3.14);
let m: Maybe[i32] = Maybe[i32]::Some(7);         // ALWAYS spell type args at source
```

### Pattern matching
```cplus
return match s {                                  // exhaustive — missing arm = E0340
    Shape::Circle(r)    => (r as i32) *% 2,
    Shape::Rect(w, h)   => (w as i32) *% (h as i32),
};

if let Maybe[i32]::Some(v) = m { println(v); }

// guard let — pattern-or-diverge; else must return/break/continue/loop
fn process(m: Maybe[i32]) -> i32 {
    guard let Maybe[i32]::Some(v) = m else { return 0 -% 1; };
    return v +% 1;
}
```

### Arrays + fill-array literal
```cplus
let a: [i32; 4] = [10, 20, 30, 40];
let x: i32 = a[2];                                // bounds-checked; OOB traps

let zeros: [u8; 64]    = [0u8; 64];               // memset fast path
let ones:  [i32; 4]    = [1; 4];                  // (1,1,1,1)
let big:   [u8; 16384] = [0u8; 16384];            // single llvm.memset

// N must be a u32 literal — no const-eval today.
```

### Generics + bounds + turbofish
```cplus
fn identity[T](x: T) -> T { return x; }
fn max[T: Ord](a: T, b: T) -> T { ... }            // bounds: Ord, Eq, Hash
struct Pair[A, B] { pub first: A, pub second: B }

let v = vec::Vec[i32]::with_capacity(16);
let s = #size_of::[Point]();
```

Always write source-level type args (`Option[i32]::Some(v)`). Mangled names like `Option__i32` are internal and never user-typeable.

### Strings
| Type | Shape | Owns? |
|---|---|---|
| `str` | `(*u8, usize)` | No, borrowed |
| `string` | `(*u8, usize, usize)` | Yes, heap |

```cplus
let a: str = "hello";                             // literal — always str
let b: string = "hello".to_string();              // copies to heap
str_ptr(s); str_len(s);                           // safe accessors
unsafe { str_from_raw_parts(p, n) };              // unsafe constructor
```

`str` is forbidden in `async fn` signatures (E0900). Pass `string` instead.

### String interpolation
```cplus
let n: i32 = 42;
let s: string = "answer is \{n}, name is \{name}".to_string();
```

Format specifiers (`\{x:04d}`) not implemented — convert numbers manually if needed.

---

## 4. Ownership model (the one truly novel part)

**No `&T`, no `&mut T`.** Borrowing is a parameter marker. As of v0.0.10 the default for non-Copy is **move**.

| Form | On non-Copy | On Copy |
|---|---|---|
| `x: T` | **Move** — caller can't use `x` after | Pass-by-value copy |
| `mut x: T` | Exclusive borrow — mutations propagate | Pass-by-value, locally mutable |
| `move x: T` | Move (explicit; same as `x: T`) | Pass-by-value |
| `borrow x: T` | Shared borrow — caller retains | (redundant on Copy) |
| `restrict p: *T` | Adds LLVM `noalias` to a raw pointer | — |

Method receivers follow the same model: `self`, `mut self`, `move self`, `borrow self`.

```cplus
fn echo(x: string) -> string { return x; }        // x moves in, returns out — fine
fn read_only(borrow s: string) -> usize { return s.len(); }  // caller keeps s

let s = "hi".to_string();
let r = echo(s);            // s consumed; using s again = E0335
```

### `Copy` is structural
Every component Copy → struct is Copy. Defining `fn drop(mut self)` forces non-Copy (else copying a resource → double-free).

```cplus
struct Point { x: i32, y: i32 }                  // Copy
struct Buf { ptr: *u8, len: usize }
impl Buf { fn drop(mut self) { unsafe { free(self.ptr); } } }   // non-Copy
```

### Return values always move

```cplus
fn make_buf() -> Buf { ... }    // no marker — return is always a move
```

### Borrow checker — aliasing XOR mutability

```cplus
let mut v = vec::Vec[i32]::new();
v.push(1);
let n = v.len();         // shared borrow
let p = v.get(0);        // shared borrow — fine
v.push(2);               // exclusive — but no live shared borrow, fine
```

Common errors: `E0372` move out of borrowed, `E0383` read while exclusively borrowed, `E0370-family` overlapping. Fix is almost always a `{ ... }` scope boundary so the conflicting borrows don't co-exist.

### Drop + defer
```cplus
fn main() -> i32 {
    println(1);
    defer println(4);
    defer println(3);
    println(2);
    return 0;            // prints 1, 2, 3, 4 (defer is LIFO at scope exit)
}
```

`defer` shares one scope-exit stack with `Drop` — they interleave in declaration order, popped LIFO.

---

## 5. Error handling

No `try`, `catch`, `throw`, `?`. Fallible fns return a tagged union.

```cplus
enum ParseResult { Ok(i32), BadInput, Overflow }

// Verbose
fn or_zero(s: str) -> i32 {
    return match parse(s) {
        ParseResult::Ok(v)       => v,
        ParseResult::BadInput    => 0,
        ParseResult::Overflow    => 0,
    };
}

// Readable
fn handle(s: str) -> i32 {
    guard let ParseResult::Ok(v) = parse(s) else { return 0 -% 1; };
    return v +% 100;
}
```

Generic Result + Option live in stdlib:
```cplus
import "stdlib/result" as result;
import "stdlib/option" as option;
result::Result[i32, result::IoError]    // ok or err
option::Option[i32]                     // some or none
```

---

## 6. FFI — calling C

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
extern fn printf(fmt: *u8, ...) -> i32;          // varargs OK on extern

let p: *u8 = unsafe { malloc(64 as usize) };
unsafe {
    p[0] = 65 as u8;
    let b: u8 = p[1];
    let q: *u8 = p + 1;                          // arithmetic strides by sizeof(T)
    free(p);
}
```

`unsafe { ... }` required for: pointer deref, indexing, `extern fn` calls, `str_from_raw_parts`, int↔ptr casts.

```cplus
#[repr(C)] struct NSRect { origin: NSPoint, size: NSSize }   // stable C layout

#[link_name = "objc_msgSend"] extern fn msg_void(r: *u8, s: *u8);
#[link_name = "objc_msgSend"] extern fn msg_str(r: *u8, s: *u8) -> *u8;

// FFI null
let nil: *u8 = unsafe { 0 as *u8 };

// Variadic C fns: MUST declare `...`. AArch64-darwin passes named args in
// registers but varargs on the stack — fixed-arity decl silently passes garbage.
extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;
```

Pointer ↔ int casts go through `usize`, never directly to `i32` (E0315).

---

## 7. Compile-time intrinsics — all spelled `#name(...)`

| Intrinsic | Returns | Notes |
|---|---|---|
| `#size_of::[T]()` | `usize` | Safe; LLVM folds to constant |
| `#align_of::[T]()` | `usize` | Safe |
| `#addr_of(x)` | `*T` | Unsafe; arg must be bare ident |
| `#include_bytes("path")` | `*[u8; N]` | Path relative to source file |
| `#include_str("path")` | `str` | UTF-8 validated at sema time |
| `#env("NAME")` | `str` | Resolved at sema; E0876 if unset |
| `#selector("name")` | `*u8` | ObjC SEL pointer, cached |
| `#msg_send(recv, "sel", ...) -> RetTy` | RetTy | Typed objc_msgSend call |
| `#compile_shader("file.metal", "msl")` | `*[u8; N]` | xcrun metal at sema time |

```cplus
let bytes: usize = #size_of::[T]() *% (n as usize);
let p = unsafe { malloc(bytes) };

fn now() -> i64 {
    let mut t: i64 = 0;
    unsafe { time(#addr_of(t)); }
    return t;
}

let metallib: *[u8; 2048] = #include_bytes("../shaders/double.metallib");
let greeting: str = #env("GREETING");
```

---

## 8. Standard library — `import "stdlib/X" as X;`

| Module | What |
|---|---|
| `io` | `print` / `println` / `eprintln` over printf |
| `result` / `option` | Generic `Result[T, E]` / `Option[T]` |
| `vec` | `Vec[T]` growable vector (Drop on scope exit) |
| `hash_map` | `HashMap[K, V]` (K: Hash + Eq; primitives + str) |
| `string` | builtin type (no module needed) |
| `fs` | File I/O |
| `net` | TCP (IPv4, numeric IPs only) |
| `env` | env vars + argv |
| `thread` | `spawn::[T](fn)` / `spawn_with::[I, O](data, fn)` / `JoinHandle[T]` |
| `atomic` | `atomic_fetch_add_*` + `Ordering::{Relaxed,Acquire,Release,AcqRel,SeqCst}` |
| `mutex` | pthread-backed, internally refcounted |
| `box` / `arc` / `rc` | Owned-on-heap; atomic refcount; non-atomic refcount |
| `channel` | typed MPMC message passing |
| `future` / `executor` / `reactor` / `time` | `async fn`, `await`, kqueue reactor |
| `iterator` | `gen fn` + adapters (`map`, `filter`, `take`) |
| `cow` | clone-on-write string |
| `range` | `0..n` lowers to `Range[i32]` |
| `marker` | Copy / Send / Sync framework |

---

## 9. Vendor packages — `import "<name>/..." as ...;`

| Package | Adds | One-liner example |
|---|---|---|
| `accelerate` | BLAS + vDSP via Apple Accelerate.framework | `cblas::sdot(n, x_ptr, 1, y_ptr, 1)` |
| `appkit` | Cocoa/AppKit bindings, 15 sub-modules | `application::Application::shared().run()` |
| `arena` | Growable bump-pointer arena | `let mut a = arena::Arena::new(4096 as usize);` |
| `clap` | Fluent argparse | `App::new("x").arg(Arg::new("v").short("v").flag())` |
| `json` | Typed-enum JSON parser + serializer | `json::parse(s) -> Result[Value, ParseError]` |
| `log` | Leveled stderr logger, zero malloc per call | `log::info("started")` |
| `metal` + `metal/mps` | Metal compute + MPS gemm/conv/FFT | `mps::MatrixMultiplication::new(dev, ...)` |
| `simd` | `Vec3` / `Vec4` / `Mat4x4` on f32x4 | `vec3::Vec3::new(1,2,3).dot(other)` |
| `static-arena` | Fixed-size stack arena (16K / 64K shapes) | `StaticArena16K::new(); a.alloc_bytes(n)` |
| `uuid` | RFC 4122 v4 from /dev/urandom | `Uuid::new_v4() -> Option[Uuid]` |

Each ships in-package `#[test]` fns runnable via `cd vendor/<pkg> && cpc test`.

---

## 10. Threads + async snapshots

```cplus
// Safe pattern: partition + join. No shared memory = no race.
import "stdlib/thread" as thread;
struct Range { start: i64, end: i64 }
fn sum_r(r: Range) -> i64 { /* ... */ }
let h1 = thread::spawn_with::[Range, i64](left,  sum_r);
let h2 = thread::spawn_with::[Range, i64](right, sum_r);
let total = h1.join() +% h2.join();

// Async
import "stdlib/executor" as executor;
async fn outer() -> i32 { return (await inner()) +% 1; }
fn main() -> i32 { return executor::block_on::[i32](outer()); }
```

Borrow-shaped params (`str`, `T[]`, `mut x: NonCopy`) are rejected in `async fn` (E0900). Use `string`, `Vec[T]`.

---

## 11. SIMD types (one-paragraph summary)

Nineteen widths: `f32x4 f64x2 f32x8 f64x4 i{8,16,32,64}x{16,8,4,2} u...` plus 256-bit doublings, plus `mask{N}x{M}` types distinct from signed-int SIMD. Constructors `splat`/`new`/`load`/`from_array`/`to_array`. Methods follow lane type: `add/sub/mul/div`, float `fma/sqrt/abs`, int `and/or/xor/shl/shr`. Compare returns `mask`, blend via `mask.select(a,b)`. SIMD does NOT cross `extern fn` boundaries — round-trip via `[f32; N]` (E0410 otherwise). Full reference: tutorial.md §32.

---

## 12. Attributes (pure metadata, no codegen by them)

```cplus
#[test]                                          // register a test fn
#[repr(C)] struct Foo { ... }                    // stable C layout
#[link_name = "real_sym"] extern fn alias(...);  // symbol aliasing
#[unroll(4)] while ... { ... }                   // loop hint
#[vectorize_width(8)] for i in ... { ... }       // vectorizer hint
#[no_alloc]                                      // real-time contract
fn rt_safe() { ... }
```

---

## 13. Common error codes

| Code | Meaning | Fix |
|---|---|---|
| E0300 | Undefined name | Typo / missing import / `pub` |
| E0302 | Type mismatch | Insert `as` or fix declared type |
| E0303 | Unknown type | Typo / missing import / generic param oos |
| E0315 | Invalid cast | Some pairs forbidden (`*T → i32`, `int → bool`) |
| E0327 | Wrong call form | `Type::method()` vs `value.method()` |
| E0333 | Implicit return | Add explicit `return EXPR;` |
| E0335 | Use of moved value | Don't read after move |
| E0337 | Move out of method-call result | Bind to local first |
| E0340 | Non-exhaustive match | Add missing arm or `_` |
| E0345 | Possibly-unassigned binding | Init on every path |
| E0370–86 | Borrow checker conflicts | Read the specific message |
| E0411 | `restrict` on non-pointer param | Only `*T` accepts `restrict` |
| E0500/E0501 | Inference fail / wrong type-arg count | Use `name::[T1, T2](...)` |
| E0801 | Needs `unsafe` | Wrap in `unsafe { ... }` |
| E0871 | Non-string-literal arg to `#include_*` / `#env` | Use a string literal |
| E0876 | `#env("X")` not set | Set the var at cpc invocation |
| E0900 | Borrow-shaped param in `async fn` | Use `string` / `Vec[T]` |
| E0902 | non-Copy moved by default — add `borrow` if caller should retain | Add `borrow` or accept the move |
| E0905 | Unknown `#name` intrinsic | Typo in intrinsic name |

`cpc --diagnostics=json` for tool-friendly output.

---

## 14. Gotchas worth remembering

```cplus
// 1. Don't malloc small fixed buffers in hot loops.
let mut tmp: [u8; 10] = [0u8; 10];               // ✅ stack
// let p = unsafe { malloc(10 as usize) };       // ❌ heap, 2-3× slowdown

// 2. Variadic C: declare with ... (AArch64-darwin ABI requires it).
extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;

// 3. Pointer cast goes through usize.
let n: usize = unsafe { p as usize };
let i: i32   = n as i32;

// 4. Two mutex guards in the same scope deadlock.
{ let g  = m.lock(); /* ... */ }                 // ✅ scope each
{ let g2 = m.lock(); /* ... */ }

// 5. `move self` does NOT auto-disarm exit-Drop.
pub fn unwrap(move self) -> T {
    return unsafe { *self.p };                   // ✅ let exit-Drop free
    // unsafe { free(self.p as *u8); }           // ❌ would double-free
}

// 6. String literal is `str`, not `string`.
let a: str    = "hello";
let b: string = "hello".to_string();
```

---

## 15. Tooling

```bash
cpc build                      # multi-file (reads Cplus.toml)
cpc FILE.cplus -o BIN          # single-file
cpc check FILE                 # parse + sema only
cpc fmt FILE                   # format in place
cpc fmt --check DIR            # CI mode
cpc test                       # run #[test] + doctests
cpc lsp                        # language server
cpc --emit-ll FILE             # pre-opt LLVM IR
cpc --emit-ll-opt FILE         # post-opt LLVM IR
cpc --emit-asm FILE            # native asm
cpc --diagnostics=json         # machine-readable
cpc --release                  # -O2 (default: debug -O0 with overflow traps)
```

### Linking against Apple frameworks

`cpc build` doesn't know `-framework`. For Cocoa / AppKit / Foundation / Metal / Accelerate, hand off to clang:

```bash
cpc --emit-ll src/main.cplus > out.ll
clang out.ll -framework Cocoa -lobjc -Wno-override-module -o bin
```

Or add `[link]` to `Cplus.toml`:
```toml
[link]
frameworks = ["Cocoa", "Metal", "MetalPerformanceShaders"]
libs       = ["objc"]
```

The vendor packages (`metal`, `appkit`, `accelerate`) already declare their `[link]` deps — consuming them is enough.

### Test discipline

Every new feature ships with **three** test shapes:
1. **Positive** — compiles and runs.
2. **Negative-with-code** — rejects with the specific Exxxx code.
3. **End-to-end** — drives `cpc build` from start to finish.

Canonical patterns: [cpc/tests/e2e.rs](cpc/tests/e2e.rs) for the compiler; in-package `#[test]` fns for vendor pkgs.

---

## 16. When in doubt

1. **Read a recipe** in [docs/examples/recipes/](docs/examples/recipes/) — every one compiles and runs.
2. **Read an example** in [docs/examples/](docs/examples/).
3. **Read a design note** in [docs/design/](docs/design/).
4. **Run `cpc fmt`** — if source doesn't round-trip, something is syntactically off.
5. **Read the diagnostic** — the compiler is the source of truth; this doc summarises.
6. **Check §2 (locked principles)** before suggesting a feature.

Don't guess; check.
