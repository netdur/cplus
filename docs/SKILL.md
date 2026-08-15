# SKILL — writing C+ source

Dense reference for an LLM about to write or edit C+ code. Not a tutorial; for the normative spec (exact syntax + semantics) see [SPEC.md](SPEC.md).

**Project:** <https://cplus-lang.dev> · **Source:** <https://github.com/netdur/cplus>

**Getting unstuck (agent surfaces, all offline in this `cpc`):** hit a diagnostic? run `cpc explain <CODE>` (e.g. `cpc explain E0502`) for its cause, fix, and an example. Re-print this reference any time with `cpc skill`. For the full docs, every page at <https://cplus-lang.dev/docs> is LLM-readable as raw markdown — **append `.md` to any page URL** (e.g. `…/docs/control-flow` → `…/docs/control-flow.md`), so you can fetch the exact topic instead of scraping HTML.

This file is a standalone reference dropped into your project; the C+ repo (examples, design notes, stdlib source) is **not** local — find it online at <https://cplus-lang.dev> and <https://github.com/netdur/cplus> (runnable examples: `…/cplus/tree/main/docs/examples/`). The compiler is the source of truth; this doc is verified against it but if they ever disagree, the compiler wins — run `cpc check` / `cpc build` and trust the diagnostic.

**Use the code graph, not grep.** C+ ships a resolved, typed code-knowledge graph (`cpc query` / `cpc mcp`, and it backs `cpc lsp`). For *any* "where is X / who calls X / what's the type here / what does this function touch" question, query the graph instead of `grep`-ing and reasoning about the text. It returns the answer already resolved — which both removes grep passes **and** removes the reasoning you'd otherwise spend disambiguating names, following `prefix::Item` to its module, and stitching call sites together. See §15.

---

## 1. What C+ is

Systems language. LLVM backend. Manual memory, no GC. Ownership with a borrow checker (aliasing XOR mutability). One-way C ABI (cpc emits standard object files; `.c` doesn't compile). Designed for LLMs to write correctly: explicit beats clever, locality is paramount, the type system carries weight.

**The language surface is small and stable.** It changes only in deliberate, versioned releases; most new capability lives in **packages** (`vendor/`, §9) and tooling rather than the core language. Prefer expressing a need as a package or library over reaching for new language syntax.

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
Vendor:    $CPLUS/vendor/{appkit,accelerate,metal,simd,arena,json,log,uuid,static-arena,jni,rt,rt_darwin}
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
| 1 | No `null` | Use `Option[T]`. FFI null is `0 as *T`. | E0300 |
| 2 | No closures / lambdas | Named `fn` only. Stateful callbacks are a `(fn_ptr, ctx: *u8)` PAIR — adjacent params, ctx defaulted, then a caller may pass `recv.method` (§3). | E0100 |
| 3 | No `&T` / `&mut T` types | The caller relation is a parameter prefix (`ref`/`take`/bare), not a type. | E0100 |
| 4 | No exceptions / `try` / `?` | Errors are tagged-union values; `match` or `guard let`. | E0001 |
| 5 | No implicit conversions | Every width change needs `as`. | E0302 |
| 6 | No overloading | One name, one signature. | E0301 |
| 7 | No macros / decorators / comptime | Only compiler-known attributes + `#name(...)` intrinsics; pure metadata. | E0354 |
| 8 | No `class` / `function` | `struct` + `impl`, `fn`. Locals are `let`/`var`. | E0100 |
| 9 | Mutability is explicit, no `mut` | `var` (local), `static` (global), `ref` (write-back). | E0305 / E0328 |
| 10 | Generics use `[T]`, not `<T>` | Avoids `a<b>(c)` ambiguity. | E0100 |
| 11 | Explicit `return` | No implicit tail returns at function level. | E0333 |
| 12 | `::` for types, `.` for instances | Strict separation. | E0303 / E0327 |
| 13 | Module-private via `_`, public by default | Leading `_` = private (items, fields, methods); `export` marks the C-ABI surface. | E0403 |

Compact examples of the non-obvious ones:

```cplus
// 1 — Option, not null
fn find(k: str) -> Option[i32] {
    if k == "answer" { return Option[i32]::Some(42); }
    return Option[i32]::None;
}

// 2 — named fn + ctx instead of closure. The ctx param sits RIGHT AFTER the
//     handler and is defaulted, which is what lets a caller pass `this.method`.
fn on_tick(n: i32, ctx: *u8) { /* ... */ }
fn subscribe(cb: fn(i32, *u8) = 0 as fn(i32, *u8), cb_ctx: *u8 = 0 as *u8) { }
subscribe(cb: this.tick);        // bound method — compiler fills cb_ctx

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
`i8 i16 i32 i64 isize` · `u8 u16 u32 u64 usize` · `f32 f64` · `bool` · `()` · `str` (16-byte view) · `Text` (heap-owned string) · `*T` (raw ptr) · `fn(...) -> R`

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

### Bindings & storage — a 2×2, one keyword per cell, no `mut`
```cplus
let x: i32 = 5;                      // immutable local: no rebind, no field write
var z: i32 = 0; z = 7;               // mutable local: rebind + field writes + mutating methods
let w: i32; w = 12;                  // late init; first write counts

const PI: f32 = 3.14159f32;          // module-scope immutable VALUE (inlined; no address)
const MASK: u64 = (1u64 << 40) - 1u64;   // const EXPRESSIONS fold at compile time, typed at
const CAP2: usize = CAP * 2;             // the declared width; overflow is E0921 (use +% to wrap);
                                         // consts may reference consts (any order; cycles rejected)
static COUNTER: i32 = 0;             // module-scope mutable, addressable global (C/FFI boundary)
COUNTER = COUNTER +% 1;              // access is bare — the `static` keyword is the marker
// `static` also takes array literals/fills AND non-generic struct literals:
static SCENE: [Sphere; 2] = [ Sphere { x: 0.0f32 }, Sphere { x: 1.0f32 } ];
```

|  | immutable | mutable |
|---|---|---|
| **global / module** | `const` (a value, no address) | `static` (addressable; foreign-facing) |
| **local** | `let` (frozen value, frozen fields) | `var` |

`let` freezes the whole value (a C+ struct is a value type), so `let p; p.x = 1` is rejected — use `var`. There is **no `mut`**: a mutable local is `var`, a mutable global is `static`, and cross-call mutation is `ref` (§4). Cross-thread safety of a shared `static` is the developer's responsibility.

### Operators
- Arithmetic `+ - * / %` traps on overflow in debug, wraps in release. Division by zero **always** traps.
- Wrapping `+% -% *%` always wrap — use when you want it.
- Bitwise `& | ^ ~ << >>`. Shift-right on signed = arithmetic; on unsigned = logical.
- Compare `< <= > >= == !=` produce `bool`, no coercion.
- Cast `as` is the only width-change tool. Pointer ↔ int goes through `usize`.
- Checked narrowing `as?` produces `Option[T]`: `n as? u8` is `Some(converted)` when the value fits, `None` otherwise (integer → integer only; needs `stdlib/option`). Plain `as` keeps its truncating semantics for when truncation is intended.

### Control flow
```cplus
if cond { ... } else if other { ... } else { ... }
let r: i32 = if cond { 1 } else { 2 };
while x < 10 { x = x +% 1; }
for i in 0..10 { ... }                       // 0..n exclusive; 0..=n inclusive
for (var i: i32 = 0; i < 10; i = i +% 1) { ... }       // C-style
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
    fn read(this) -> i32 { return this.x +% this.y; }            // read access, doesn't mutate
    fn translate(ref this, dx: i32) { this.x = this.x +% dx; }   // mutating method (write-back)
    fn into_raw(take this) -> i32 { return this.x; }             // consumes the value
}
struct Public { value: i32, _internal: i32 }                     // `_` field = module-private
```

> **No struct-literal field shorthand.** Write `Point { x: x, y: y }`, not `Point { x, y }`.
> **Type-inferred struct literals**: where the type is known (annotated binding, `return`, argument), drop the name — `let p: Point = { x: 1, y: 2 };` and `return { x: 1, y: 2 };`.
> **Receivers are `this` / `ref this` / `take this`** (the enclosing type is `This`). The name is always `this`; `ref`/`take` are the modifier. A `ref this` (mutating) method requires a `var` receiver — calling it on a `let` is E0328.

### Deriving `Eq` / `Ord` / `Hash` / `Clone` / `ToText`

An **empty** `impl Type: Interface {}` against one of the five blessed interfaces asks the compiler to generate the memberwise implementation — the same idiom as a `Send` marker impl, extended to code generation. No attribute, no macro:

```cplus
struct Key { id: i64, name: str }
impl Key: Eq {}                                  // fn eq(this, other: Key) -> bool
impl Key: Ord {}                                 // fn cmp(this, other: Key) -> i32
impl Key: Hash {}                                // fn hash(this) -> u64  (FNV-1a fold)
impl Key: Clone {}                               // fn clone(this) -> Key
impl Key: ToText {}                              // fn to_text(this) -> Text ("Key { id: 1, name: a }")

var m = hash_map::new::[Key, i32]();             // derived Hash + Eq satisfy K's bounds
m.insert(Key { id: 1, name: "a" }, 100);
```

Field-by-field: primitives compare/hash directly, `str` orders through its `compare`, nested structs recurse through their own (derived or hand-written) method, and a generic target works with the bounds you declare (`impl Pair[T: Eq]: Eq {}`). Payload-carrying enum fields and array/slice/tuple fields are not derivable (**E0920**) — write that method by hand. `ToText` needs `stdlib/text` in the build. Deriving targets structs only; an empty impl of a user interface stays an error (E0916), and `Copy` stays structural (never written, never derived).

### Callbacks: pass a method with `recv.method`

There are no closures, so a stateful callback is a **pair** — the code and the object it runs on. C+ writes that pair as two parameters and wires it for you. **The two parameters must be adjacent, in this order, and the context defaulted:**

```cplus
// DECLARING a function that accepts a callback — the `*u8` slot is not optional
fn row(on_click: fn(str, *u8) = 0 as fn(str, *u8),
       on_click_ctx: *u8 = 0 as *u8) -> Node { ... }

// CALLING it with a component's own method — no cast, no address, no ctx argument
row(on_click: this.open_project)

// or with a free fn, threading the context yourself
fn opened(path: str, ctx: *u8) { ... }
row(on_click: opened, on_click_ctx: #addr_of(this) as *u8)
```

`this.open_project` is a **bound method reference**: the compiler synthesizes a bridge fn for the handler slot and fills the `*u8` after it with `#addr_of(this)`. The method's shape must be the handler's parameters minus the trailing `*u8`, same return type — here `fn open_project(ref this, path: str)`.

> **Omit the `_ctx` slot and callers can never pass a method** — only a free fn. That is the single most common mistake in this area: the author writes one parameter, a caller in another file writes `this.method`, and E0824 fires where it cannot be fixed. **W0824 warns at the declaration** and prints the line to add. Same rule for a struct field that stores a handler: store the `*u8` beside it.

Codes: **W0824** (declaration has no ctx slot) · **W0825** (ctx is FIRST — a bound method reads it from the LAST parameter) · **E0824** (callee has no slot, or the call filled it) · **E0823** (method shape does not fit) · **E0822** (`take this`, generic, or `ref`/`take` params — none can be bound).

An **associated fn** (no receiver) is a namespaced fn, so `Type::f` is a legal fn-pointer value — which is what an objc IMP or any other C callback needs, and it lets the callback live on the type it belongs to instead of as a top-level fn beside it:

```cplus
impl LineGutter {
    fn draw_imp(view: *u8, rect: rt::Rect) { ... }        // no `this`
}
let imp: fn(*u8, rt::Rect) = LineGutter::draw_imp;        // its address
synth::add_method(cls, sel, LineGutter::draw_imp, types); // or straight in
```

Type-directed, exactly like a free fn: without an expected `fn(...)` type it is **E0312**. A **method** (one with a receiver) has no address of this shape — `fn(this, …)` is not `fn(…)` — and says so with E0312; a generic associated fn is **E0821**.

### Enums
```cplus
enum Color { Red, Green, Blue }                  // plain, lowers to i32, Copy
enum Shape { Circle(f64), Rect(f64, f64) }       // tagged
enum Maybe[T] { Some(T), None }                  // generic

// FFI enums (payload-free only): explicit discriminants (any constant
// expression) + #[repr(u8|u16|u32|u64|i8|..|i64|C)] pinning the C width.
#[repr(u8)]
enum Mode { Off = 0, Slow = 10, Fast = 200 }     // crosses the C ABI as uint8_t
enum Status { Ok, NotFound = 404, Gone }         // Gone = 405 (C rules: prev + 1)

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

// All three forms also take `var` instead of `let` — the bindings become
// mutable (`guard var` = mutable in the enclosing scope; `if var` /
// `while var` = mutable inside the body). `let` bindings are frozen.
fn bump(m: Maybe[i32]) -> i32 {
    guard var Maybe[i32]::Some(v) = m else { return 0 -% 1; };
    v = v +% 1;
    return v;
}
```

### Arrays + fill-array literal
```cplus
let a: [i32; 4] = [10, 20, 30, 40];
let x: i32 = a[2];                                // bounds-checked; OOB traps

let zeros: [u8; 64]    = [0u8; 64];               // memset fast path
let ones:  [i32; 4]    = [1; 4];                  // (1,1,1,1)
let big:   [u8; 16384] = [0u8; 16384];            // single llvm.memset

// N is a u32 literal, a `const` name, or any constant expression evaluated
// at `usize` (folded before type-check; unknown name/non-int -> E0912/E0921).
const CAP: usize = 1024;
let buf: [u8; CAP] = [0u8; CAP];                  // const in the type AND fill count
let big: [u8; CAP * 2] = [0u8; CAP * 2];          // length arithmetic folds at compile time
```

### Distinct newtypes — `type X = distinct i64`

A nominal integer alias: same representation and ABI as the base, but a separate type. Mixing two ids of the same underlying integer is the classic silent bug no borrow checker catches — a distinct type makes it a compile error:

```cplus
type UserId = distinct i64;
type ChannelId = distinct i64;

let u = 7 as UserId;                     // construct by casting (any integer casts in)
let n: i64 = u as i64;                   // leave by casting out
take_user(channel);                      // E0302 — brands don't mix
let v: UserId = 5;                       // E0302 — base doesn't flow in silently
u == u2; u.eq(u2); u.hash();             // same-brand comparison + blessed Hash/Eq work
```

Arithmetic and ordering are rejected on brands (cast to the base). A distinct type satisfies `Hash`/`Eq`/`Copy` bounds, so it works as a `HashMap` key, and generic instantiations keep the brand: `Vec[UserId]` is its own type — `append` takes `UserId`, `at` returns `Option[UserId]`, and passing it where `Vec[i64]` is expected is E0302. The base must be a plain integer type (E0922 otherwise).

### Generics + bounds + turbofish
```cplus
fn identity[T](x: T) -> T { return x; }
fn max[T: Ord](a: T, b: T) -> T { ... }            // bounds: Ord, Eq, Hash
struct Pair[A, B] { first: A, second: B }

let v = vec::with_capacity::[i32](16 as usize);
let s = #size_of::[Point]();
```

Always write source-level type args, never mangled names — `Option__i32` is internal and is not accepted in source (E0405), even though older diagnostics printed it.

**Where the args are required, and where they are noise.** *Constructing* a generic enum value needs them — nothing else says which instantiation you mean (`Option[i32]::Some(1)`; the bare form is E0303). *Matching* one does not: the scrutinee already fixes the instantiation, so the enum path alone resolves in `match`, `if let`, `while let` and `guard let`.

```cplus
fn find(k: str) -> Option[i32] { return Option[i32]::Some(42); }  // construct: args REQUIRED

match find("answer") {
    Option::Some(v) => v,        // match: args optional — prefer this
    Option::None    => 0,
}
```

Prefer the short form in patterns. Restating the type is not just noise: it is the arm's only dependency on a type the compiler already derived, so a later change to that type turns every arm into an error (E0341) with nothing wrong in the logic. Written the short way, the same change touches none of them.

### Strings
| Type | Shape | Owns? |
|---|---|---|
| `str` | `(*u8, usize)` | No, borrowed |
| `Text` | `(*u8, usize, usize)` | Yes, heap |

```cplus
let a: str = "hello";                             // literal — always str
let b: Text = Text::from("hello");                // copies to heap
b.count(); b.is_empty(); b.clone();               // Text methods
a.count(); a.contains("ell");                     // str methods (import "stdlib/str")
```

A borrowed `Text` **coerces to `str`** at argument, binding, return, and receiver positions, and when compared with a `str` (`name == "x"`), so a `str`-typed slot accepts a `Text` directly — no `.as_str()`. The coercion borrows; returning the view of a *local* `Text` is rejected (E0513). `Text::clone` copies/owns. `str` is forbidden in `async fn` signatures (E0900); pass `Text` instead.

**A view needs a named owner.** At a BINDING (or an assignment, or a field of an aggregate a binding keeps) the thing being viewed must be somebody's binding — a temporary has no lifetime to lend:

```cplus
let s: str = t.clone();          // E0513 — clone's Text is an anonymous temp
let s: str = "x = ${n}";         // E0513 — so is the interpolation's
let s: str = mk().view();        // E0513 — same, one accessor deeper
let owner: Text = t.clone();     // name it, then view it
let s: str = { owner.view() };   // fine — `owner` outlives the statement

f("x = ${n}");                   // fine — an ARGUMENT's temp outlives the call
```

**One read surface: reads live on `str` and return views.** `Text` declares only what allocates or mutates (`append`, `insert`, `truncate`, `reserve`, `clone`, `uppercased`, `appending`, `replacing`, `pad_start/pad_end`, …) plus `capacity`/`view`/`equals`. Every read — `count`, `trim`, `slice`, `split`, `find`, … — lives in the blessed `impl str` block, and a `Text` receiver reaches it through the coercion: `t.trim()` returns a **`str` view into `t`'s buffer**, no copy. While a view lives, its owner is write-locked (mutating/moving/dropping it is rejected; reads stay fine); **the borrow ends at the view's last use**, not at scope end, so `let v = t.trim(); use(v); t.append("!")` compiles. A use inside a loop pins the borrow past the loop; a use in a `defer` or block tail pins it to scope exit. Convert a view to an owned value with `.to_text()` — that is the only copy you ever pay, and you spell it.

**`str` methods.** stdlib declares the builtin view's method set (`import "stdlib/str" as _;` anywhere in the build enables it — `as _` is the discard alias for extension-only imports; importing `stdlib/text` brings it in transitively). Everything reads or returns sub-views of the same buffer — no allocation except `split`:

```cplus
s.count(); s.is_empty(); s.char_count(); s.is_ascii();          // NOT len()
s.byte_at(index: 0);                                            // Option[u8]
s.has_prefix("ab"); s.has_suffix("yz"); s.contains("x");
s.find("x"); s.rfind("x");                                      // Option[usize]
s.count_of(","); s.compare(other); s.equals_ignoring_case(t);
s.slice(from: 1, to: 4);                                        // Option[str] — a view, no copy
s.prefix(count: 2); s.suffix(count: 2);                         // clamped views
s.drop_first(); s.drop_last(count: 2);
s.removing_prefix("src/"); s.removing_suffix(".txt");
s.trim(); s.trim_start(); s.trim_end();                         // views — endpoints move, no copy
s.split(separator: ",");                                        // Vec[str] of views (allocates the Vec)
s.to_i64();  s.to_f64();                                        // Option — strict decimal shapes
```

All of these work identically on a `Text` receiver (same names, same view
returns). `text::join(parts, separator:)` takes the `Vec[str]` that `split`
returns. Slices and arrays carry the same two core reads — `xs.count()`,
`xs.is_empty()` — with `#slice_*` as their FFI tier.

There is still **no `+` concatenation**: build strings with interpolation (below) or `Text::append`. Operations that must allocate (uppercasing, replacing, padding) live on `Text` — convert with `s.to_text()`. The `#str_ptr(s)` / `#str_len(s)` / `#str_from_raw_parts(p, n)` intrinsics remain the FFI tier — passing bytes to C and building views over foreign memory — not the way to do string work.

### String interpolation
```cplus
let n: i32 = 42;
let s: Text = "answer is ${n}, name is ${name}";   // bound: an owned Text (allocates)
io::println("i = ${n}");                            // sink: writes parts, ZERO heap
```

Syntax is `${expr}` (not `\{...}`). An interpolated literal passed **directly** to `io::print` / `io::println` / `io::eprintln` never materializes a `Text`: each part writes straight to the stream (numbers via stack scratch), so a print loop compiles to what the equivalent `printf` does — and since the sinks are `#[no_alloc]`, real-time code may log this way. `t.append("x = ${x}")` likewise appends the parts **in place** (one `reserve`, then copies; atomic on OOM) — unless a part reads `t` itself, which takes the copying path. Any other position (a binding, an argument to anything else) builds an owned `Text`. Format specifiers (`${x:04d}`) are **not** implemented — convert numbers manually if needed.

### Also supported
Type aliases (`type Name = ExistingType;`) and tuples (`(a, b)` literal, `(T, U)` type) parse and compile. Check the online examples for exact usage before relying on tuple method surface.

---

## 4. Ownership model (the one truly novel part)

**No `&T`, no `&mut T`.** How a parameter relates to the caller's value is a *prefix* on the parameter, one keyword per relation. The default (a bare `x: T`) is a **read-only borrow for every type** — the caller keeps ownership.

| Form | Meaning | Caller |
|---|---|---|
| `x: T` | **Read-only borrow** (any type) | keeps `x`; may read it after |
| `ref x: T` | **By-reference write-back** — the callee mutates the caller's value | place must be `var`; no call-site `&` |
| `take x: T` | **Ownership transfer** (move) | can't use `x` after |
| `restrict p: *T` | Adds LLVM `noalias` to a raw pointer | — |

Method receivers mirror it: `this` (read), `ref this` (mutating method, write-back), `take this` (consume). The name is always `this`; `ref`/`take` are the modifier.

```cplus
fn read_only(s: Text) -> usize { return s.len(); }   // bare = borrow; caller keeps s
fn bump(ref n: i32) { n = n +% 1; }                   // writes back into the caller's var
fn sink(take t: Text) -> usize { return t.len(); }    // consumes t

var k: i32 = 0;
bump(k);                    // k is now 1 — a write-back call is `bump(k)`, not `bump(&k)`
let s: Text = Text::from("hi");
let n = sink(s);            // s consumed; using s again = E0335
```

A bare non-Copy value can be read freely but not *escape* the callee (returned, stored in a field/global, or re-passed to a `take`) — that would create a second owner, so it is **E0337** ("use `take`, or `.clone()`"). `ref` requires a `var` caller place: passing a `let` (immutable) to a `ref` parameter is **E0328** (the same rule that rejects a mutating method on a `let` receiver). Both checks are made at the call site from the signature alone — no callee-body inspection — so they stay modular through fn-pointers, interfaces, and generics.

### `Copy` is structural
Every component Copy → struct is Copy. Defining `fn drop(ref this)` forces non-Copy (else copying a resource → double-free).

```cplus
struct Point { x: i32, y: i32 }                  // Copy
struct Buf { _ptr: *u8, _len: usize }
impl Buf { fn drop(ref this) { free(this._ptr); } }   // non-Copy
```

### Return values move

```cplus
fn make_buf() -> Buf { ... }    // returning an owned local moves it out — no marker
```

To give a parameter's ownership back out (return it, store it), the parameter must be `take` — a bare param is a borrow and can't escape (E0337).

### Borrow checker — aliasing XOR mutability

```cplus
var v = vec::new::[i32]();
v.push(1);
let n = v.len();         // shared borrow
let p = v.get(0);        // shared borrow — fine
v.push(2);               // exclusive — but no live shared borrow, fine
```

The conflicts you'll see: `E0337` (escaping a bare borrow — return/store it via `take` or `.clone()`), `E0335` (use of a `take`-moved value), `E0328` (passing a `let` to a `ref` parameter, or a mutating method on a `let`), and the `E0370`/`E0380`-family overlapping-borrow checks. Fixes, in order of preference: add a `{ ... }` scope so a borrow ends earlier; make the binding `var` (for `ref`); `take` / `.clone()`; or restructure ownership. **Not every conflict is fixable by scoping alone** — some are genuine ownership-restructuring problems.

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

### Auto field-drop

Teardown is recursive and automatic. When a value goes out of scope, the compiler runs any user `drop(ref this)` first, then drops each **owning field** in reverse declaration order — no hand-written per-field drops needed:

```cplus
struct Person { name: Text, tags: vec::Vec[Text] }   // no `drop` written
// dropping a Person auto-frees `tags` then `name` — both owning C+ types.
```

What counts as owning (dropped automatically): `Text`, `Vec`/`Box`/other library types with their own `drop`, structs that contain any owning field, arrays of those, and **tagged-enum payloads** (the active variant's owning payload is dropped via a tag switch — `Option[Text]`, a JSON-like `enum Value { Str(Text), ... }`, etc.). Raw `*T` fields are **not** auto-dropped — they remain your responsibility via a freeing `drop` or `opaque` (§ above).

Consequences to know:
- A struct/enum that owns heap data is **non-Copy** and **move-only** (copying would double-free). Code that gives such a value away needs `take`/`.clone()`.
- You **cannot move an owning field out** of such an aggregate (**E0509**) — the auto-drop would free it twice. Clone it, or `match` to consume the whole value.
- `match`ing an *owned* enum **consumes** it (its drop is suppressed; the matched-out payload becomes the caller's), so the binding cannot be read or matched again afterwards (**E0335**). `match`ing through a `borrow` does not consume.
- Consumption is triggered by **binding a name** anywhere in the patterns. A match that binds nothing reads only the discriminant and leaves the value intact, which is how you write a presence check on a value you still need:

```cplus
match maybe_thing {                  // presence check — binds nothing, consumes nothing
    Option[Text]::Some(_) => {}
    Option[Text]::None    => { return 0; }
}
match maybe_thing {                  // still yours to match for real
    Option[Text]::Some(t) => { return t.count() as i32; }
    Option[Text]::None    => { return 0; }
}
```

  `Some(_v)` **binds** — the leading `_` is the privacy convention, not a wildcard — so it consumes like any other name. Use `Some(_)` for the non-consuming form.
- In a `guard let`, the else block must not re-match the scrutinee (E0335): the payload's destructor has already run by then. Bind the complement instead — `guard let E::A(v) = e else |E::B(x)| { ... }`.
- A container's heap *elements* behind a raw pointer (a `Vec[T]`'s `T`s) are dropped by the container's own `drop` (which walks them via `__cplus_drop_in_place::[T]`), not by auto field-drop. Binding an owning payload from a consumed enum and then *not* moving it out drops it at arm exit (no leak).

### Raw-pointer accountability (`opaque`)
Every raw-pointer (`*T`) struct field must be **accounted for**, or it's a compile error (**E0510**) — no silent-leak default. Account for it one of two ways:

```cplus
struct Buf { _ptr: *u8 }
impl Buf { fn drop(ref this) { free(this._ptr); } }            // owned: drop frees it

struct View { opaque _ptr: *u8 }                                // borrowed: not mine
```

Severity tracks what the compiler can **prove** from the `drop` body (structural check, no dataflow):
- release is **unconditional**, or guarded only by a null-test on the *same* field → **clean**
- release is **conditional** (refcount/flag/loop — can't prove it always runs) → **W0002** warning (expected for `Arc`/`Rc`-style refcounted owners)
- **no** direct `free(this.f)` appears, or it's delegated to a helper, or there's no `drop` → **E0510**
- field marked **`opaque`** → clean ("managed elsewhere")

`free(this.ptr as *u8)` counts (cast is transparent). Use `opaque` only when another owner truly frees it: an FFI handle the runtime owns, a borrowed view, a sibling-owned pointer. **When you write a struct with a `*T` field, decide ownership: add a freeing `drop`, or mark it `opaque`.**

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

// When the failure payload matters, capture it with the complement form
// `else |Pat|` — the else block receives the failure value instead of
// losing it. The two patterns together must cover the enum (E0349).
enum ReadResult { Ok(i32), Err(i32) }

fn handle_or_report(s: str) -> i32 {
    guard let ReadResult::Ok(v) = read(s) else |ReadResult::Err(code)| {
        return 0 -% code;
    };
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

let p: *u8 = malloc(64 as usize);
p[0] = 65 as u8;
let b: u8 = p[1];
let q: *u8 = p + 1;                          // arithmetic strides by sizeof(T)
free(p);
```

There is **no `unsafe` block**. Every operation that can cause undefined behaviour is already syntactically visible — a deref/index is `*p` / `p[i]` (the only meaning of `*`), making a pointer is `x as *T`, pointer→int is the loud `#addr(p)` intrinsic, and a foreign call can't appear without a preceding `extern fn` declaration or `c::` import. The declaration is the marker; the call stays bare. (UB is *single-threaded* never-accidental — data races through a shared `static` remain the developer's responsibility.)

```cplus
#[repr(C)] struct NSRect { origin: NSPoint, size: NSSize }   // stable C layout

// C UNION — one storage, several typed views. For binding real headers; for
// an either/or VALUE in ordinary code use an enum with payloads (it has a tag).
#[repr(C)] union FloatBits { f: f32, bits: u32 }
var u: FloatBits = FloatBits { f: 1.0f32 };   // a literal names exactly ONE member
u.bits;                                        // reading another reinterprets the bytes
// size = largest member, align = strictest, verified against clang. Members
// must be Copy (no tag -> no destructor can be run correctly), not generic,
// at least one. E0925 otherwise.

// PACKED — no padding between fields. `packed` is `packed = 1`; `packed = N`
// caps every field's alignment at N (C's `#pragma pack(N)`).
#[repr(C, packed)] struct Wire { kind: u8, len: u32 }     // size 5, align 1
#[repr(C, packed = 2)] struct Legacy { a: u8, b: u32 }    // b at offset 2

// BITFIELDS — `#[bits(N)]` on an integer field. C's rules exactly: a field
// that would cross its type's storage unit starts a new one, packing lets it
// straddle instead, and a signed field sign-extends when read.
#[repr(C)] struct Flags {
    #[bits(3)] kind: u32,       // bits 0..3
    #[bits(5)] level: u32,      // bits 3..8, same 4-byte unit
    #[bits(4)] delta: i32,      // signed: reads back negative
}
var f: Flags = Flags { kind: 5 as u32, level: 21 as u32, delta: -3 as i32 };
f.kind = 2 as u32;              // read-modify-write; neighbours preserved

// Neither a bitfield nor an under-aligned packed field has an ADDRESS: no
// `ref` parameter, no `#addr_of` (E0927 / E0926). Read and write them
// directly, or copy into a local first. Packed fields must be Copy, and a
// bitfield needs `#[repr(C)]`, an integer type, a width of 1..=its bits, and
// a non-generic, non-union struct.

#[link_name = "objc_msgSend"] extern fn msg_void(r: *u8, s: *u8);
#[link_name = "objc_msgSend"] extern fn msg_str(r: *u8, s: *u8) -> *u8;

// FFI null — and the way to TEST for one. `is_null()` / `is_not_null()` are
// blessed on any raw pointer AND any fn-pointer; each is a single
// `icmp ptr, null` with no memory access, so both are safe anywhere.
let nil: *u8 = 0 as *u8;
if p.is_null() { return; }              // instead of `p == (0 as *Data)`
if this.on_click.is_not_null() { ... }  // instead of repeating the fn type

// Variadic C fns: MUST declare `...`. AArch64-darwin passes named args in
// registers but varargs on the stack — fixed-arity decl silently passes garbage.
extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;
```

Pointer ↔ int casts go through `usize`, never directly to `i32` (E0315).

### Calling INTO C+ — from lldb, a test harness, or any C caller

The one supported door is `export fn`, and it deliberately rejects `str`
(E0410): `str` is a fat pointer with no C-ABI counterpart, so an exported
entry takes `*u8` + `usize` and rebuilds the view inside:

```cplus
export fn probe_emit(name_ptr: *u8, name_len: usize) {
    let name: str = { #str_from_raw_parts(name_ptr, name_len) };
    events::emit(name);
    return;
}
```

Everything that is NOT `export` is off limits from outside, and the failure
mode is silent: internal functions are `fastcc` with module-scoped mangled
names, and a `str` there is an LLVM `{ ptr, i64 }` aggregate under that
convention — an lldb `call` or a C declaration reaches the symbol, passes
garbage, and nothing says so (there is no way to tell a bad ABI from a no-op
handler from the outside). If a harness needs to drive an internal path,
write the two-line `export` wrapper above; that is what it is for.

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
let p = malloc(bytes);

fn now() -> i64 {
    var t: i64 = 0;
    time(#addr_of(t));
    return t;
}

let metallib: *[u8; 2048] = #include_bytes("../shaders/double.metallib");
let greeting: str = #env("GREETING");

// Inline asm. Tier 1: bare template (fences/barriers/hints):
#asm("dmb ish");
// Tier 2: named operands + clobbers. `{name}` placeholders bind to operands;
// `in`/`out`/`inout` set direction; `reg` lets the compiler pick a register
// (then you MUST use `{name}`), or `"x0"` pins one. `out`/`inout` targets must
// be `mut` variables. Operands are integer/pointer/bool (register-sized).
var sum: i64 = 0;
#asm("add {s}, {a}, {b}", s = out(reg) sum, a = in(reg) a, b = in(reg) b);
var v: i64 = x;
#asm("add {v}, {v}, #1", v = inout(reg) v);                  // read-modify-write
#asm("mov x16, #20", p = out("x0") pid, clobber("x16"));             // pinned reg + clobber
// Tier 3: `#[naked]` — no prologue/epilogue; body is asm-only and returns
// itself (args arrive in ABI registers). For trampolines / entry stubs.
#[naked]
fn raw_add(a: i64, b: i64) -> i64 { #asm("add x0, x0, x1\nret"); }
```

---

## 8. Standard library — `import "stdlib/X" as X;`

| Module | What |
|---|---|
| `io` | `print` / `println` / `eprintln` over printf |
| `result` / `option` | Generic `Result[T, E]` / `Option[T]` (variants + constructors only — no combinators) |
| `vec` | `Vec[T]` growable vector (Drop on scope exit) |
| `hash_map` | `HashMap[K, V]` (K: Hash + Eq; primitives + str). `new` / `insert` / `get` / `contains_key` |
| `slice` | checked sub-views over `T[]`: `sub` (→ `Option[T[]]`), `prefix`/`suffix`/`drop_first`/`drop_last`. Free fns (`slice::sub::[T](s, from, to)`) — method form waits on generic slice impls |
| `flags` | `Flags` option-set over u64 bits: `none`/`of`/`from_bits`, `contains`/`intersects`/`with`/`without`/`toggled`, set algebra. Bit values from `const` masks or repr-enum discriminants (`Mode::Fast as u64`) |
| `Text` | builtin type (no module needed) |
| `fs` | File I/O |
| `net` | TCP (IPv4, numeric IPs only) |
| `env` | env vars + argv |
| `thread` | `spawn::[T](fn)` / `spawn_with::[I, O](data, fn)` / `JoinHandle[T]` |
| `atomic` | `atomic_fetch_add_*` + `Ordering::{Relaxed,Acquire,Release,AcqRel,SeqCst}` |
| `mutex` | pthread-backed, internally refcounted (no separate reference-count wrapper) |
| `box` / `arc` / `rc` | Owned-on-heap: `Box` one owner, `Arc` atomic-refcount shared, `Rc` non-atomic shared. `Arc`/`Rc` add `downgrade() -> Weak[T]` for cycle-breaking back-pointers |
| `channel` | typed MPMC message passing |
| `future` / `executor` / `reactor` / `time` | `async fn`, `await`, kqueue reactor |
| `iterator` | `gen fn` + adapters (`map`, `filter`, `take`) |
| `cow` | clone-on-write `Text` |
| `range` | `0..n` lowers to `Range[i32]` |
| `marker` | Copy / Send / Sync framework |

`marker`, `range`, and `time` are mostly import/marker shims with little public surface.

### Smart pointers (and their C++ equivalents)

| Need | Use | Notes |
|---|---|---|
| unique heap ownership | `box::Box[T]` | one owner; `unwrap() -> T` moves the value back out |
| shared ownership, one thread | `rc::Rc[T]` | non-atomic refcount; `!Send` / `!Sync` |
| shared ownership, across threads | `arc::Arc[T]` | atomic refcount; `Send + Sync` iff `T` is |
| non-owning reference | `rc::Weak[T]` / `arc::Weak[T]` | `downgrade()` from a strong handle; `upgrade() -> Option[..]` while the value lives; the tool for cycle-breaking back-pointers (a `Weak` does not keep the value alive) |
| exclusive access to a shared value | `Rc/Arc::with_mut(f: fn(ref T)) -> Status` | `Ok` only when this is the sole strong handle and no `Weak` exists, else `Shared` |
| recover the owned value | `Rc/Arc::try_unwrap() -> Option[T]` | `Some` when this is the sole strong handle |

Coming from C++: `unique_ptr` → `Box`, `shared_ptr` → `Arc` (or `Rc` when single-threaded), `weak_ptr` → `Weak`. There is no interior-mutability escape hatch: shared mutation goes through the `with_mut` gate or a `mutex::Mutex[T]`.

---

## 9. Vendor packages — `import "<name>/..." as ...;`

| Package | Adds | One-liner example |
|---|---|---|
| `accelerate` | BLAS + vDSP via Apple Accelerate.framework | `cblas::sdot(n, x_ptr, 1, y_ptr, 1)` |
| `appkit` | Cocoa/AppKit bindings, 15+ sub-modules | `application::Application::shared().run()` |
| `arena` | Growable bump-pointer arena | `var a = arena::Arena::new(4096 as usize);` |
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

Borrow-shaped params (`str`, `T[]`, `ref x: NonCopy`) are rejected in `async fn` (E0900). Use `Text`, `Vec[T]`.

Shared mutable state exists (`mutex`, `atomic`, `arc`), but prefer partition+join. `Mutex[T]` is internally refcounted (no separate wrapper needed) — reach for it directly only when message-passing or partitioning won't do.

### Interface default method bodies

An interface method may carry a body instead of a `;`. Implementors may omit
it; those that declare it override it.

```cplus
interface Shape {
    fn area(this) -> i32;                                  // must be written
    fn describe(this) -> i32 { return this.area() * 2; }   // default
}
impl Sq: Shape { fn area(this) -> i32 { return this.s * this.s; } }  // gets describe
impl Rect: Shape { fn area(this) -> i32 { ... }
                   fn describe(this) -> i32 { return 99; } }         // overrides it
```

The body is COPIED into each impl block that omitted it, before sema — so it
is monomorphized like any other method, there is no `dyn`, and `This` means
the implementing type. A default that calls a method the implementor lacks is
an error against **that type**. An interface whose methods all have defaults
takes an empty impl (`impl A: Greet {}`).

### `Send` / `Sync` marker impls

`spawn`/`spawn_with` require their type params to be `Send`. A struct or enum that **hides a raw pointer** (directly or through a field) is `!Send` and `!Sync` — passing one across a `Send`/`Sync` bound is a compile error (**E0502**). A *bare* `*T` used directly (e.g. `thread::spawn::[*u8]`) stays Send. `Rc`/`MutexGuard` are `!Send` (Rc also `!Sync`).

When you know a pointer-holding type is safe to move/share across threads, vouch for it with a hand-written marker impl (the `: I` conformance connector; no prefix keyword — writing the impl the compiler would not derive *is* the assertion):

```cplus
struct Handle { opaque _h: *u8 }
impl Handle: Send {}                           // marker impl = the manual Send assertion

// Conditional generic form — the bounds ARE the condition:
impl Arc[T: Send + Sync]: Send {}              // Arc[X] is Send iff X is Send + Sync
```

A marker impl's body is empty — and an empty impl of `Eq`/`Ord`/`Hash`/`Clone`/`ToText` derives the memberwise implementation instead (§3); any other interface requires a body **unless every method it declares has a default** (below). `Arc`/`Mutex`/`Channel` already carry the right conditional impls, so they work across threads when their payload does.

---

## 11. SIMD types (one-paragraph summary)

Nineteen widths: `f32x4 f64x2 f32x8 f64x4 i{8,16,32,64}x{16,8,4,2} u...` plus 256-bit doublings, plus `mask{N}x{M}` types distinct from signed-int SIMD. Constructors `splat`/`new`/`load`/`from_array`/`to_array`. Methods follow lane type: `add/sub/mul/div`, float `fma/sqrt/abs`, int `and/or/xor/shl/shr`. Compare returns `mask`, blend via `mask.select(a,b)`. SIMD does NOT cross `extern fn` boundaries — round-trip via `[f32; N]` (E0410 otherwise). Full reference: SPEC.md.

---

### Scoped threads — lend a local, no `Arc`

`spawn` / `spawn_with` MOVE their data into the worker. A **scope** lends it
instead, and guarantees the worker is joined before the loan ends:

```cplus
var counts: Counts = Counts { hits: 0 };
{
    var s: thread::Scope = thread::scope();
    s.lend::[Counts](counts, tally);      // tally: fn(ref Counts)
}                                          // Scope::drop joins every worker
use(counts.hits);                          // safe: the workers are done
```

`Scope::lend` is `#[keeps(this)]` on a `ref` parameter, so the borrow checker
knows the scope holds the loan. Three mistakes are compile errors, not races:
the lent value dying before the scope (**E0514**), writing it while a worker
holds it (**E0381**), and lending the same place twice (**E0381** — two
workers with exclusive access). Workers return through the lent value; a
thread that must return a VALUE is `spawn`/`spawn_with`'s job.

## 12. Attributes (pure metadata, no codegen by them)

Only compiler-known attributes are accepted; an unknown attribute is rejected (E0354).

```cplus
#[test]                                          // register a test fn
#[requires(n > 0)] fn f(n: i32) ...              // precondition, checked at entry (traps on
                                                 // violation; test builds report). Pure exprs
                                                 // over params/consts/fields; E0924 otherwise
#[ensures(result >= n)] fn g(n: i32) -> i32 ...  // postcondition, checked at EVERY return.
                                                 // `result` names the returned value; on a
                                                 // fn returning nothing it names nothing
                                                 // (E0928) — write it about `this`/params
#[repr(C)] struct Foo { ... }                    // stable C layout
#[repr(C, packed)] / #[repr(C, packed = 2)]      // no padding / cap alignment at N (§6)
#[bits(3)] kind: u32,                            // bitfield inside a #[repr(C)] struct (§6)
#[link_name = "real_sym"] extern fn alias(...);  // symbol aliasing
#[unroll(4)] while ... { ... }                   // loop hint
#[vectorize_width(8)] for i in ... { ... }       // vectorizer hint
#[no_alloc]                                      // real-time contract
fn rt_safe() { ... }
#[inline] / #[inline(always)] / #[inline(never)] // LLVM inlinehint/alwaysinline/noinline
fn hot(x: i32) -> i32 { return x; }              // (always) forces inline even at -O0
#[deprecated("use parse_v2 instead")]            // W0006 at each USE, never at the
fn parse() -> i32 { return 1; }                  // declaration; the string is optional and
                                                 // is printed verbatim. A WARNING — the call
                                                 // still builds, so a rename can land as a
                                                 // list consumers work through and break a
                                                 // release later. On fn/method/struct/enum/
                                                 // field/variant
```

---

## 13. Common error codes

| Code | Meaning | Fix |
|---|---|---|
| E0001 | Lexer: unexpected character | Bad token (e.g. `?`, `\{`) — not part of C+ |
| E0100 | Parser: unexpected token | Wrong form (closure, `<T>`, `class`, `&T`, etc.) |
| E0300 | Undefined name | Typo / missing import (also `null`) |
| E0301 | Duplicate definition | No overloading — rename |
| E0302 | Type mismatch | Insert `as` or fix declared type |
| E0303 | Unknown type | Typo / missing import / generic param oos |
| E0312 | `for ... in` needs range or `Iterator[T]` | Don't iterate arrays directly — index `0..n` |
| E0315 | Invalid cast | Some pairs forbidden (`*T → i32`, `int → bool`) |
| E0327 | Wrong call form | `Type::method()` vs `value.method()` |
| E0333 | Implicit return | Add explicit `return EXPR;` |
| E0335 | Use of `take`-moved value | Don't read after `take` |
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
| E0403 | Private symbol used across modules | Drop the leading `_` (or `export` it for the C ABI) |
| E0411 | `restrict` on non-pointer param | Only `*T` accepts `restrict` |
| E0500/E0501 | Inference fail / wrong type-arg count | Use `name::[T1, T2](...)` |
| E0337 | A bare borrow escapes (return / field-store / re-pass to `take`) | Take it by value (`take`) or `.clone()` |
| E0852 | Import used outside a build | Use `cpc build` (reads `Cplus.toml`), not single-file `cpc check` |
| E0871 | Non-string-literal arg to `#include_*` / `#env` | Use a string literal |
| E0876 | `#env("X")` not set | Set the var at cpc invocation |
| E0900 | Borrow-shaped param in `async fn` | Use `Text` / `Vec[T]` |
| E0328 | `ref` arg / mutating method needs a `var` place | Declare the binding `var` |
| E0905 | Unknown `#name` intrinsic | Typo in intrinsic name |

`cpc --diagnostics=json` for tool-friendly output (NDJSON: `severity`, `code`, `message`, `primary` span, optional `labels`/`notes`/`suggestions`).

---

## 14. Gotchas worth remembering

```cplus
// 1. Don't malloc small fixed buffers in hot loops.
var tmp: [u8; 10] = [0u8; 10];               // ✅ stack
// let p = malloc(10 as usize);                  // ❌ heap, 2-3× slowdown

// 2. Variadic C: declare with ... (AArch64-darwin ABI requires it).
extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;

// 3. Pointer cast goes through usize.
let n: usize = #addr(p);
let i: i32   = n as i32;

// 4. Two mutex guards in the same scope deadlock.
{ let g  = m.lock(); /* ... */ }                 // ✅ scope each
{ let g2 = m.lock(); /* ... */ }

// 5. `take this` does NOT auto-disarm exit-Drop.
fn unwrap(take this) -> T {
    return *this.p;                   // ✅ let exit-Drop free
    // free(this.p as *u8);           // ❌ would double-free
}

// 6. String literal is `str`, not `Text`.
let a: str    = "hello";
let b: Text = Text::from("hello");
```

Recurring traps for generated code:
- **No `.unwrap()` / `.map()` / `.is_some()` on Result/Option** — use `match` / `guard let`. No `panic()` either.
- **No `Text` `+`, `split`, `parse`** — interpolate (`${x}`) or do pointer/length work.
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
cpc build --asan               # AddressSanitizer (also -g, --ubsan / --tsan / --msan)
```

> Builds are fast (a small project compiles in well under a second). For the agentic edit→compile loop, prefer `cpc build` as the feedback command for any project with imports; reserve `cpc check FILE` for self-contained snippets.

### Debugging and sanitizers

`-g` emits DWARF debug info. The LLVM sanitizers instrument cpc-emitted code the same way they instrument clang's:

```bash
cpc build --asan  file.cplus     # AddressSanitizer: use-after-free, overflows, leaks
cpc build --ubsan file.cplus     # UndefinedBehaviorSanitizer
cpc build --tsan  file.cplus     # ThreadSanitizer: data races
cpc build --msan  file.cplus     # MemorySanitizer: reads of uninitialized memory
cpc test  --asan                 # run the test suite under a sanitizer
```

`--asan` / `--tsan` / `--msan` are mutually exclusive (they contend for shadow memory); `--ubsan` composes with any of them. The safe subset prevents most memory errors at compile time; the sanitizers cover the raw-pointer and FFI surface, where a use-after-free or leak is a runtime concern rather than a compile-time guarantee. On macOS, Apple's `leaks` complements `--asan` for allocation-balance checks.

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

Canonical patterns: [`cpc/tests/e2e.rs`](https://github.com/netdur/cplus/blob/main/cpc/tests/e2e.rs) for the compiler; in-package `#[test]` fns for vendor pkgs.

---

## 16. Contextual builder blocks — `@ctx { ... }`

A package-extensible *declarative construction* syntax (UI trees, route tables, config) with **no macros, closures, or compiler plugins**. The compiler owns only the syntax + lowering; a package supplies ordinary types and functions. `@` was an unused character — purely additive.

```cplus
import "ui/view" as view;

let screen = @view {                 // @ctx opens a builder block; ctx is a module path
    text("Inbox")                    // leaf element; bare name resolves as view::text
        .font = bigger               // leading-dot modifier: field assign on the item
        .on_click(refresh)           // leading-dot modifier: method call on the item
    let n = unread_count();          // `let` setup is allowed
    if n > 0 {                       // collection-if: body items add into THIS block
        badge(n)
    }
    vstack {                         // container element (same context) — NO `@`
        for row in rows {            // collection-for: one+ items per iteration
            item(row)
        }
    }
};
```

**Name lookup inside a block:** bare names resolve **locals → same-file top-level → `ctx::name`**. So `text`/`vstack`/`badge`/`item` above are `view::*`; `n`/`row` are locals; a same-file `fn` of the same name shadows the package. A bare name that is no member at all is the ordinary "undefined" error. Modifier names (`.font`, `.on_click`) are fields/methods on the item, never contextual.

**Leading-dot modifiers** attach to the item on the line(s) above. The rule is line-oriented: a *line-leading* `.x` is a modifier; a *same-line* `.x` is ordinary postfix (`text("a").trimmed()` is one item, then a newline `.font = …` modifies it).

**Container elements** are a bare `name { ... }` (never `@name`) — a child of the *same* context, not a nested DSL. A nested *different* `@`-DSL block is **rejected** (parse error): write a same-context container without `@`.

**Allowed in a block:** item lines, `.modifier` lines, `let`, `if`/`else`/`else if`, `for … in …`, nested container elements. **Rejected:** `while`/`loop`, `return`/`break`/`continue`, `defer`/`guard`, `yield`/`await`, and nested `@`.

### The package contract (what a builder package author writes)

Fixed protocol names, single `Item` type per context (C+ has no overloading):

```cplus
struct Item { ... }                           // one element type for the context

fn text(s: str) -> Item { ... }               // leaf constructor (any args)
fn badge(n: i32) -> Item { ... }

struct Builder { ... }                         // the accumulator
impl Builder {
    fn new() -> Builder { ... }
    fn add(ref this, item: Item) { ... }       // called once per item
    fn finish(take this) -> Root { ... }       // root finisher (Root may differ from Item)
}

fn vstack(b: Builder) -> Item { ... }          // CONTAINER: takes a filled Builder -> Item
```

A container constructor takes a **`Builder`** (not a collection): the author stores children however they like, so the compiler's lowering never names `Vec` — DSL packages work even where stdlib `Vec` is gated (esp32). Modifier fields/methods are just fields/methods on `Item`.

### What it lowers to (before sema — ordinary C+)

Everything reduces to `Builder::new` + `add`, differing only in the finisher (root `.finish()`, container `ctx::name(builder)`); `if`/`for` add into the same builder:

```cplus
// @view { text("Inbox").font=bigger  if n>0 { badge(n) }  vstack { for r in rows { item(r) } } }
var __b = view::Builder::new();
var __i = view::text("Inbox");
__i.font = bigger;
__b.add(__i);
if n > 0 { __b.add(view::badge(n)); }
let __c = { var __cb = view::Builder::new();
            for r in rows { __cb.add(view::item(r)); }
            view::vstack(__cb) };           // container finisher
__b.add(__c);
let screen = __b.finish();
```

No new codegen: sema and the borrow checker see only ordinary locals, calls, and blocks. Diagnostics land on the user-written DSL line (the desugar reuses spans).

---

## 17. When in doubt

1. **Read a recipe / example online** — <https://github.com/netdur/cplus/tree/main/docs/examples> (`recipes/` are task-shaped, every file compiles and runs).
2. **Read a design note online** — <https://github.com/netdur/cplus/tree/main/plans> (or the site, <https://cplus-lang.dev>).
3. **Run `cpc fmt`** — if source doesn't round-trip, something is syntactically off.
5. **Read the diagnostic** — the compiler is the source of truth; this doc summarises.
6. **Check §2 (locked principles)** before suggesting a feature.
7. **Navigate by the graph, not `grep`** (§15) — `cpc query` / `cpc mcp` resolve names text search can't.

Don't guess; check.
