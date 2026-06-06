# SKILL — writing C+ source

Dense reference for an LLM about to write or edit C+ code. Not a tutorial; not the spec.

**Project:** <https://cplus-lang.dev> · **Source:** <https://github.com/netdur/cplus>

This file is a standalone reference dropped into your project; the C+ repo (examples, design notes, stdlib source) is **not** local — find it online at <https://cplus-lang.dev> and <https://github.com/netdur/cplus> (runnable examples: `…/cplus/tree/main/docs/examples/`). The compiler is the source of truth; this doc is verified against it but if they ever disagree, the compiler wins — run `cpc check` / `cpc build` and trust the diagnostic.

**Use the code graph, not grep.** C+ ships a resolved, typed code-knowledge graph (`cpc query` / `cpc mcp`, and it backs `cpc lsp`). For *any* "where is X / who calls X / what's the type here / what does this function touch" question, query the graph instead of `grep`-ing and reasoning about the text. It returns the answer already resolved — which both removes grep passes **and** removes the reasoning you'd otherwise spend disambiguating names, following `prefix::Item` to its module, and stitching call sites together. See §15.

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

> Any file containing an `import` must be compiled with **`cpc build`** (which reads `Cplus.toml`). `cpc check FILE` does **not** read the manifest and will fail (E0852) on imported modules — it's for single-file, import-free snippets only. See §15.

### Paths

`cpc` comes from the C+ toolchain (build/clone <https://github.com/netdur/cplus>, call its checkout `$CPLUS`). Your project links the stdlib/vendor packages from there; examples live online, not in your project:

```text
Compiler:  cpc            (on PATH, or $CPLUS/target/release/cpc)
LSP:       cpc-lsp
Stdlib:    symlink $CPLUS/vendor/stdlib into your project's vendor/stdlib
Vendor:    $CPLUS/vendor/{appkit,accelerate,metal,simd,arena,clap,json,log,uuid,static-arena,jni,rt,rt_darwin}
Examples:  https://github.com/netdur/cplus/tree/main/docs/examples   (online — not in your project)
```

### Scaffold a new project

```bash
mkdir -p my_proj/src my_proj/vendor && cd my_proj
ln -s "$CPLUS"/vendor/stdlib vendor/stdlib
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
cpc build && ./target/debug/my_proj
```

---

## 2. Locked principles — never propose violating

All thirteen are **compiler-enforced**, not convention. The error code you hit when you break one is in the right column.

| # | Principle | What that means | If violated |
|---|---|---|---|
| 1 | No `null` | Use `Option[T]`. FFI null is `0 as *T` in `unsafe`. | E0300 |
| 2 | No closures / lambdas | Named `fn` only. Stateful callbacks via `(fn_ptr, user_data: *u8)`. | E0100 |
| 3 | No `&T` / `&mut T` types | Borrowing is a parameter marker, not a type. | E0100 |
| 4 | No exceptions / `try` / `?` | Errors are tagged-union values; `match` or `guard let`. | E0001 |
| 5 | No implicit conversions | Every width change needs `as`. | E0302 |
| 6 | No overloading | One name, one signature. | E0301 |
| 7 | No macros / decorators / comptime | Only compiler-known attributes; they are pure metadata. | E0354 |
| 8 | No `class` / `function` / `var` | `struct` + `impl`, `fn`, `let`. | E0100 |
| 9 | No mutable-by-default | `mut` is opt-in. | sema |
| 10 | Generics use `[T]`, not `<T>` | Avoids `a<b>(c)` ambiguity. | E0100 |
| 11 | Explicit `return` | No implicit tail returns at function level. | E0333 |
| 12 | `::` for types, `.` for instances | Strict separation. | E0303 / E0327 |
| 13 | Module-private by default | `pub` is the export marker. Public symbols are intentional, not accidental. | E0403 |

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
let v = vec::with_capacity::[i32](16 as usize);
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
let hf: f16 = 1.5f16;     // f16 suffix (or `let hf: f16 = 1.5;`)
let cs: *u8 = c"hi\n";    // c"..." — NUL-terminated *u8 for FFI (libc/JNI/Cocoa)
```

### Variables, const, static
```cplus
let x: i32 = 5;
let mut z: i32 = 0; z = 7;
let w: i32; w = 12;                  // late init; first write counts

const PI: f32 = 3.14159f32;          // const: literal-only initializer (E0X30 otherwise)
static OFFSET: i32 = 50;             // .rodata
static mut COUNTER: i32 = 0;         // reads + writes need unsafe { ... }
// `static` also takes array literals/fills AND non-generic struct literals:
static SCENE: [Sphere; 2] = [ Sphere { x: 0.0f32 }, Sphere { x: 1.0f32 } ];
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
for (let mut i: i32 = 0; i < 10; i = i +% 1) { ... }   // C-style
loop { if done { break; } continue; }
while let Option[i32]::Some(v) = next() { ... }
assert x > 0;                                 // traps on false
```

> **Arrays are NOT iterable with `for ... in`.** `for v in arr` is rejected (E0312 — `for...in` wants a range `0..n` or an `Iterator[T]`). Iterate by index instead:
> ```cplus
> let a: [i32; 3] = [10, 20, 30];
> for i in 0..3 { let v: i32 = a[i]; /* ... */ }
> ```

### Structs + methods + receivers
```cplus
struct Point { x: i32, y: i32 }
impl Point {
    fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }   // assoc fn
    fn read(self) -> i32 { return self.x +% self.y; }            // read access, doesn't consume
    fn translate(mut self, dx: i32) { self.x = self.x +% dx; }   // exclusive
    fn into_raw(move self) -> i32 { return self.x; }             // consumes self
}
struct Public { pub value: i32, internal: i32 }                  // field visibility
```

> **No struct-literal field shorthand.** Write `Point { x: x, y: y }`, not `Point { x, y }`.
> **Receivers are `self` / `mut self` / `move self` only — there is no `borrow self`.** `borrow` is a *parameter* marker (§4), not a receiver. Use bare `self` for read-only method access.

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

if let Maybe[i32]::Some(v) = m { #println(v); }

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

// N is a u32 literal OR a non-negative integer `const` name (folded before
// type-check; unknown/non-int -> E0X36). No length arithmetic (`[T; N*2]`).
const CAP: usize = 1024;
let buf: [u8; CAP] = [0u8; CAP];                  // const in the type AND fill count
```

### Generics + bounds + turbofish
```cplus
fn identity[T](x: T) -> T { return x; }
fn max[T: Ord](a: T, b: T) -> T { ... }            // bounds: Ord, Eq, Hash
struct Pair[A, B] { pub first: A, pub second: B }

let v = vec::with_capacity::[i32](16 as usize);
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
#str_ptr(s); #str_len(s);                           // safe accessors
unsafe { #str_from_raw_parts(p, n) };              // unsafe constructor
b.len(); b.is_empty(); b.as_str(); b.clone();     // string methods
```

`str` is forbidden in `async fn` signatures (E0900). Pass `string` instead.

> **String ops are sparse.** There is **no `+` concatenation** and **no stdlib `split` / `parse` / `slice` / `find`** on strings. Build strings with interpolation (below), and do byte-level work via `str_ptr` / `str_len` + manual pointer logic (see the `http_get` recipe online).

### String interpolation
```cplus
let n: i32 = 42;
let s: string = "answer is ${n}, name is ${name}".to_string();
```

Syntax is `${expr}` (not `\{...}`). Format specifiers (`${x:04d}`) are **not** implemented — convert numbers manually if needed.

### Also supported
Type aliases (`type Name = ExistingType;`) and tuples (`(a, b)` literal, `(T, U)` type) parse and compile. Check the online examples for exact usage before relying on tuple method surface.

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

Method receivers: `self` (read access), `mut self` (exclusive), `move self` (consume). **No `borrow self`** — `borrow` is a parameter marker only.

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
let mut v = vec::new::[i32]();
v.push(1);
let n = v.len();         // shared borrow
let p = v.get(0);        // shared borrow — fine
v.push(2);               // exclusive — but no live shared borrow, fine
```

Under default-move, the **most common** conflict you'll see is `E0335` (use of moved value). Others: `E0372` move out of borrowed, `E0383` read while exclusively borrowed, `E0370`-family overlapping borrows. Fixes, in order of preference: add a `{ ... }` scope so a borrow ends earlier; add a `borrow` marker so the callee doesn't consume; `.clone()`; or restructure ownership. **Not every conflict is fixable by scoping alone** — some are genuine ownership-restructuring problems.

### Drop + defer
```cplus
fn main() -> i32 {
    #println(1);
    defer #println(4);
    defer #println(3);
    #println(2);
    return 0;            // prints 1, 2, 3, 4 (defer is LIFO at scope exit)
}
```

`defer` shares one scope-exit stack with `Drop` — they interleave in declaration order, popped LIFO.

### Auto field-drop — v0.0.14

Teardown is recursive and automatic. When a value goes out of scope, the compiler runs any user `drop(mut self)` first, then drops each **owning field** in reverse declaration order — no hand-written per-field drops needed:

```cplus
struct Person { name: string, tags: vec::Vec[string] }   // no `drop` written
// dropping a Person auto-frees `tags` then `name` — both owning C+ types.
```

What counts as owning (dropped automatically): `string`, `Vec`/`Box`/other library types with their own `drop`, structs that contain any owning field, arrays of those, and **tagged-enum payloads** (the active variant's owning payload is dropped via a tag switch — `Option[string]`, a JSON-like `enum Value { Str(string), ... }`, etc.). Raw `*T` fields are **not** auto-dropped — they remain your responsibility via a freeing `drop` or `opaque` (§ above).

Consequences to know:
- A struct/enum that owns heap data is **non-Copy** and **move-only** (copying would double-free). Code that previously treated such a value as freely copyable now needs `move`/`borrow`/`.clone()`.
- You **cannot move an owning field out** of such an aggregate (**E0509**) — the auto-drop would free it twice. Clone it, or `match` to consume the whole value.
- `match`ing an *owned* enum **consumes** it (its drop is suppressed; the matched-out payload becomes the caller's). `match`ing through a `borrow` does not.
- A container's heap *elements* behind a raw pointer (a `Vec[T]`'s `T`s) are dropped by the container's own `drop` (which walks them via `__cplus_drop_in_place::[T]`), not by auto field-drop. Binding an owning payload from a consumed enum and then *not* moving it out drops it at arm exit (no leak).

### Raw-pointer accountability (`opaque`) — v0.0.13
Every raw-pointer (`*T`) struct field must be **accounted for**, or it's a compile error (**E0510**) — no silent-leak default. Account for it one of two ways:

```cplus
struct Buf { ptr: *u8 }
impl Buf { fn drop(mut self) { unsafe { free(self.ptr); } } }  // owned: drop frees it

struct View { opaque ptr: *u8 }                                 // borrowed: not mine
```

Severity tracks what the compiler can **prove** from the `drop` body (structural check, no dataflow):
- release is **unconditional**, or guarded only by a null-test on the *same* field → **clean**
- release is **conditional** (refcount/flag/loop — can't prove it always runs) → **W0002** warning (expected for `Arc`/`Rc`-style refcounted owners)
- **no** direct `free(self.f)` appears, or it's delegated to a helper, or there's no `drop` → **E0510**
- field marked **`opaque`** → clean ("managed elsewhere")

`free(self.ptr as *u8)` counts (cast is transparent). Use `opaque` only when another owner truly frees it: an FFI handle the runtime owns, a borrowed view, a sibling-owned pointer. **When you write a struct with a `*T` field, decide ownership: add a freeing `drop`, or mark it `opaque`.**

---

## 5. Error handling

No `try`, `catch`, `throw`, `?`. Fallible fns return a tagged union.

> **Critical — Result/Option have NO methods to lean on.** `Result[T,E]` and `Option[T]` provide **only** their variants (and a few constructors). There is **no** `.unwrap()`, `.expect()`, `.map()`, `.and_then()`, `.unwrap_or()`, `.ok_or()`, `.is_ok()`, `.is_some()`. Handle them **only** with `match`, `if let`, or `guard let`. (`.unwrap()` exists on `Box[T]` — that is unrelated.) There is also **no `panic()` / `abort()`**: the only hard bail is `assert` (which traps). Do not write any of the missing methods — they won't compile.

Constructors that exist:
- `Result`: variants `Result[T,E]::Ok(v)` / `Result[T,E]::Err(e)`; helpers `result::ok`, `result::err`, `result::io_ok`, `result::io_err`. `result::IoError` has fixed variants.
- `Option`: variants `Option[T]::Some(v)` / `Option[T]::None`; helper `option::some`.

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

// Readable — guard let is the dominant idiom across the recipes
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

> **No error context / wrapping.** There is no source-chaining, no message-attach, and no uniform/boxed error (no `anyhow` analog). If you need context, encode it in your own enum variants or carry it in the payload.

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
| `#addr_of(place)` | `*T` | Unsafe; arg must be an addressable place |
| `#include_bytes("path")` | `*[u8; N]` | Path relative to source file |
| `#include_str("path")` | `str` | UTF-8 validated at sema time |
| `#env("NAME")` | `str` | Resolved at sema; E0876 if unset |
| `#zero::[T]()` | `T` | Safe all-zero value |
| `#cpu_relax()` | `()` | Safe spin-loop hint |
| `#asm("tmpl", name = dir(reg) expr, clobber("r"))` | `()` | Unsafe inline asm; Tier 1 = bare template, Tier 2 = `in`/`out`/`inout` operands + clobbers |
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

// Inline asm — always `unsafe`. Tier 1: bare template (fences/barriers/hints):
unsafe { #asm("dmb ish"); }
// Tier 2: named operands + clobbers. `{name}` placeholders bind to operands;
// `in`/`out`/`inout` set direction; `reg` lets the compiler pick a register
// (then you MUST use `{name}`), or `"x0"` pins one. `out`/`inout` targets must
// be `mut` variables. Operands are integer/pointer/bool (register-sized).
let mut sum: i64 = 0;
unsafe { #asm("add {s}, {a}, {b}", s = out(reg) sum, a = in(reg) a, b = in(reg) b); }
let mut v: i64 = x;
unsafe { #asm("add {v}, {v}, #1", v = inout(reg) v); }       // read-modify-write
unsafe { #asm("mov x16, #20", p = out("x0") pid, clobber("x16")); }  // pinned reg + clobber
// Tier 3: `#[naked]` — no prologue/epilogue; body is asm-only and returns
// itself (args arrive in ABI registers). For trampolines / entry stubs.
#[naked]
fn raw_add(a: i64, b: i64) -> i64 { unsafe { #asm("add x0, x0, x1\nret"); } }
```

---

## 8. Standard library — `import "stdlib/X" as X;`

| Module | What |
|---|---|
| `io` | `print` / `println` / `eprintln` over printf |
| `result` / `option` | Generic `Result[T, E]` / `Option[T]` (variants + constructors only — no combinators) |
| `vec` | `Vec[T]` growable vector (Drop on scope exit) |
| `hash_map` | `HashMap[K, V]` (K: Hash + Eq; primitives + str). `new` / `insert` / `get` / `contains_key` |
| `string` | builtin type (no module needed) |
| `fs` | File I/O |
| `net` | TCP (IPv4, numeric IPs only) |
| `env` | env vars + argv |
| `thread` | `spawn::[T](fn)` / `spawn_with::[I, O](data, fn)` / `JoinHandle[T]` |
| `atomic` | `atomic_fetch_add_*` + `Ordering::{Relaxed,Acquire,Release,AcqRel,SeqCst}` |
| `mutex` | pthread-backed, internally refcounted (collapses the `Arc<Mutex>` pair) |
| `box` / `arc` / `rc` | Owned-on-heap; atomic refcount; non-atomic refcount |
| `channel` | typed MPMC message passing |
| `future` / `executor` / `reactor` / `time` | `async fn`, `await`, kqueue reactor |
| `iterator` | `gen fn` + adapters (`map`, `filter`, `take`) |
| `cow` | clone-on-write string |
| `range` | `0..n` lowers to `Range[i32]` |
| `marker` | Copy / Send / Sync framework |

`marker`, `range`, and `time` are mostly import/marker shims with little public surface.

---

## 9. Vendor packages — `import "<name>/..." as ...;`

| Package | Adds | One-liner example |
|---|---|---|
| `accelerate` | BLAS + vDSP via Apple Accelerate.framework | `cblas::sdot(n, x_ptr, 1, y_ptr, 1)` |
| `appkit` | Cocoa/AppKit bindings, 15+ sub-modules | `application::Application::shared().run()` |
| `arena` | Growable bump-pointer arena | `let mut a = arena::Arena::new(4096 as usize);` |
| `clap` | Fluent argparse | `App::new("x").arg(Arg::new("v").short("v").flag())` |
| `json` | Typed-enum JSON parser + serializer | `json::parse(s) -> Result[Value, ParseError]` |
| `log` | Leveled stderr logger, zero malloc per call | `log::info("started")` |
| `metal` + `metal/mps` | Metal compute + MPS gemm/conv/FFT | `mps::MatrixMultiplication::new(dev, ...)` |
| `simd` | `Vec3` / `Vec4` / `Mat4x4` on f32x4 | `vec3::Vec3::new(1,2,3).dot(other)` |
| `static-arena` | Fixed-size stack arena (16K / 64K shapes) | `StaticArena16K::new(); a.alloc_bytes(n)` |
| `uuid` | RFC 4122 v4 from /dev/urandom | `Uuid::new_v4() -> Option[Uuid]` |

Each ships in-package `#[test]` fns runnable via `cd vendor/<pkg> && cpc test`. Vendor packages are self-contained (deps are stdlib or none) — `cpc` does not resolve transitive C+ dependencies, so there is no deep tree to audit.

---

## 10. Threads + async snapshots

```cplus
// Safe pattern: partition + join. No shared memory = no race. THIS is the idiomatic path.
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

Shared mutable state exists (`mutex`, `atomic`, `arc`), but prefer partition+join. There is no literal `Arc<Mutex<T>>` pattern — `Mutex[T]` is internally refcounted, so reach for it directly only when message-passing or partitioning won't do.

### `Send` / `Sync` and `unsafe impl` — v0.0.14

`spawn`/`spawn_with` require their type params to be `Send`. A struct or enum that **hides a raw pointer** (directly or through a field) is `!Send` and `!Sync` — passing one across a `Send`/`Sync` bound is a compile error (**E0502**). A *bare* `*T` used directly (e.g. `thread::spawn::[*u8]`) stays Send: it is visibly unsafe at every use. `Rc`/`MutexGuard` are `!Send` (Rc also `!Sync`).

When you know a pointer-holding type is safe to move/share across threads, vouch for it:

```cplus
struct Handle { opaque h: *u8 }
unsafe impl Send for Handle {}                 // must be `unsafe` (E0860 otherwise)

// Conditional generic form — the bounds ARE the condition:
unsafe impl Send for Arc[T: Send + Sync] {}    // Arc[X] is Send iff X is Send + Sync
```

`unsafe impl` applies only to `Send`/`Sync` (E0861 elsewhere); the body is empty. `Arc`/`Mutex`/`Channel` already carry the right conditional impls, so they work across threads when their payload does.

---

## 11. SIMD types (one-paragraph summary)

Nineteen widths: `f32x4 f64x2 f32x8 f64x4 i{8,16,32,64}x{16,8,4,2} u...` plus 256-bit doublings, plus `mask{N}x{M}` types distinct from signed-int SIMD. Constructors `splat`/`new`/`load`/`from_array`/`to_array`. Methods follow lane type: `add/sub/mul/div`, float `fma/sqrt/abs`, int `and/or/xor/shl/shr`. Compare returns `mask`, blend via `mask.select(a,b)`. SIMD does NOT cross `extern fn` boundaries — round-trip via `[f32; N]` (E0410 otherwise). Full reference: tutorial.md §32.

---

## 12. Attributes (pure metadata, no codegen by them)

Only compiler-known attributes are accepted; an unknown attribute is rejected (E0354).

```cplus
#[test]                                          // register a test fn
#[repr(C)] struct Foo { ... }                    // stable C layout
#[link_name = "real_sym"] extern fn alias(...);  // symbol aliasing
#[unroll(4)] while ... { ... }                   // loop hint
#[vectorize_width(8)] for i in ... { ... }       // vectorizer hint
#[no_alloc]                                      // real-time contract
fn rt_safe() { ... }
#[inline] / #[inline(always)] / #[inline(never)] // LLVM inlinehint/alwaysinline/noinline
fn hot(x: i32) -> i32 { return x; }              // (always) forces inline even at -O0
```

---

## 13. Common error codes

| Code | Meaning | Fix |
|---|---|---|
| E0001 | Lexer: unexpected character | Bad token (e.g. `?`, `\{`) — not part of C+ |
| E0100 | Parser: unexpected token | Wrong form (closure, `<T>`, `class`, `borrow self`, etc.) |
| E0300 | Undefined name | Typo / missing import / `pub` (also `null`) |
| E0301 | Duplicate definition | No overloading — rename |
| E0302 | Type mismatch | Insert `as` or fix declared type |
| E0303 | Unknown type | Typo / missing import / generic param oos |
| E0312 | `for ... in` needs range or `Iterator[T]` | Don't iterate arrays directly — index `0..n` |
| E0315 | Invalid cast | Some pairs forbidden (`*T → i32`, `int → bool`) |
| E0327 | Wrong call form | `Type::method()` vs `value.method()` |
| E0333 | Implicit return | Add explicit `return EXPR;` |
| E0335 | Use of moved value | Don't read after move (most common borrow error) |
| E0337 | Move out of method-call result | Bind to local first |
| E0340 | Non-exhaustive match | Add missing arm or `_` |
| E0345 | Possibly-unassigned binding | Init on every path |
| E0354 | Unknown attribute | Only compiler-known attributes allowed |
| E0370–86 | Borrow checker conflicts | Read the specific message; scope/borrow/clone/restructure |
| E0510 | Unaccounted raw-pointer field | Free it in `drop`, or mark `opaque f: *T` |
| E0513 | View of a local escapes (returned directly OR inside a returned struct/array) | Return owned, or borrow from a param |
| W0002 | *(warn)* raw-ptr field freed only conditionally in `drop` | Expected for refcounted types; confirm every owning path frees |
| E0X30 | Non-literal `const`/`static` initializer | Use a literal (or array/struct literal for `static`) |
| E0X36 | Array length isn't a literal or non-neg int `const` | Use a literal or an in-scope int `const` |
| E0403 | Private symbol used across modules | Mark it `pub` |
| E0411 | `restrict` on non-pointer param | Only `*T` accepts `restrict` |
| E0500/E0501 | Inference fail / wrong type-arg count | Use `name::[T1, T2](...)` |
| E0801 | Needs `unsafe` | Wrap in `unsafe { ... }` |
| E0852 | Import used outside a build | Use `cpc build` (reads `Cplus.toml`), not single-file `cpc check` |
| E0871 | Non-string-literal arg to `#include_*` / `#env` | Use a string literal |
| E0876 | `#env("X")` not set | Set the var at cpc invocation |
| E0900 | Borrow-shaped param in `async fn` | Use `string` / `Vec[T]` |
| E0902 | non-Copy moved by default | Add `borrow` or accept the move |
| E0905 | Unknown `#name` intrinsic | Typo in intrinsic name |

`cpc --diagnostics=json` for tool-friendly output (NDJSON: `severity`, `code`, `message`, `primary` span, optional `labels`/`notes`/`suggestions`).

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

Recurring traps for generated code:
- **No `.unwrap()` / `.map()` / `.is_some()` on Result/Option** — use `match` / `guard let`. No `panic()` either.
- **No string `+`, `split`, `parse`** — interpolate (`${x}`) or do pointer/length work.
- **`for v in arr` is invalid** — index with `for i in 0..n`.
- **Struct literals need named fields** — `Point { x: x, y: y }`, not `{ x, y }`.
- **`cpc check` can't see imports** — anything with `import` must go through `cpc build`.
- Interpolation is `${x}`, not `\{x}`; no format specifiers.

---

## 15. Tooling

```bash
cpc build                      # multi-file project (reads Cplus.toml) — REQUIRED for any code with imports
cpc FILE.cplus -o BIN          # single-file, no imports
cpc check FILE                 # parse + sema only, single-file no-import (does NOT read Cplus.toml)
cpc check                      # whole-project front-end (reads Cplus.toml + [profile.realtime]); no codegen — CI gate
cpc --realtime-report[=json]   # whole-project real-time contract digest (profile + per-contract violations)
cpc fmt FILE                   # format in place
cpc fmt --check DIR            # CI mode
cpc test                       # run #[test] + doctests
cpc lsp                        # language server — goto-def / references / hover / outline served from the graph
cpc graph                      # whole-project code knowledge graph as JSON
cpc query def|refs|callers|callees|call-hierarchy|members|symbols|context|type-at  # resolved navigation
cpc mcp                        # resident MCP server over the graph (point an agent's MCP client here)
cpc --emit-ll FILE             # pre-opt LLVM IR
cpc --emit-ll-opt FILE         # post-opt LLVM IR
cpc --emit-asm FILE            # native asm
cpc --diagnostics=json         # machine-readable (NDJSON)
cpc --release                  # -O2 (default: debug -O0 with overflow traps)
```

> Builds are fast (a small project compiles in well under a second). For the agentic edit→compile loop, prefer `cpc build` as the feedback command for any project with imports; reserve `cpc check FILE` for self-contained snippets.

### Navigating C+ code: query the graph, don't grep

To locate or trace a symbol, use the code graph — it is **resolved and typed**, `grep` is neither (it can't tell the `Point` type from a local `point`, follow `prefix::Item` to its module, or list real callers). `cpc query def|refs|callers|callees|context|type-at …` answer by symbol with clickable `file:line:col`, as JSON, and state their own coverage via `unresolved`/`scope`. Because C+ has no dynamic dispatch, every call to a *named* function or method resolves — so `unresolved` counts only genuine **function-pointer indirections** (`let f: fn(...) = ...; f(x)`), and a **zero count means the answer is complete** (no `grep` fallback needed). The same graph backs `cpc lsp`. In an agent loop, run `cpc mcp` once and call the tools (`find_definition`, `find_references`, `find_callers`, `code_context`, `type_at`, …) instead of spawning `cpc query` per lookup. Reach for the graph before reaching for `grep`.

**Why this saves you (the model) work — fewer tokens, less reasoning.** A `grep` gives you raw text hits that you then have to *reason* about: is this `area` the method or a local? does this `parse` call bind to `json::parse` or another? which of 30 hits are real callers? The graph has already done that disambiguation in the compiler. So the graph replaces *both* the search passes **and** the chain of inference you'd run over their results:

- `cpc query context FN` returns, in **one** call, the function's signature + callers + callees + the types it references — the whole edit-neighborhood, resolved. That's several `grep`s plus the work of stitching them together, collapsed into one authoritative answer you can paste straight back (symbol ids are source names like `src.geo::Shape::area`, never mangled).
- `cpc query type-at FILE:LINE:COL` gives the resolved type at a cursor — no reading surrounding code to infer it.
- `cpc query def SYMBOL` jumps to the real definition — no guessing which same-named thing matched.

Net: prefer one graph query over `grep` + manual reasoning. It is cheaper for you and the answer is correct by construction, not by your inference.

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
2. **Negative-with-code** — rejects with the specific Exxxx code (assert on `status != 0` + stderr contains the code).
3. **End-to-end** — drives `cpc build` from start to finish.

Canonical patterns: [cpc/tests/e2e.rs](cpc/tests/e2e.rs) for the compiler; in-package `#[test]` fns for vendor pkgs.

---

## 16. When in doubt

1. **Read a recipe / example online** — <https://github.com/netdur/cplus/tree/main/docs/examples> (`recipes/` are task-shaped, every file compiles and runs).
2. **Read a design note online** — <https://github.com/netdur/cplus/tree/main/plans> (or the site, <https://cplus-lang.dev>).
3. **Run `cpc fmt`** — if source doesn't round-trip, something is syntactically off.
5. **Read the diagnostic** — the compiler is the source of truth; this doc summarises.
6. **Check §2 (locked principles)** before suggesting a feature.
7. **Navigate by the graph, not `grep`** (§15) — `cpc query` / `cpc mcp` resolve names text search can't.

Don't guess; check.
