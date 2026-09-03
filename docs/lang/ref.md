# C+ reference

The manual: every construct, lookup-shaped, one entry each. Signatures and
behavior only — learning path in [tour.md](tour.md), judgment in
[guide.md](guide.md), normative text in [spec.md](spec.md) /
[memory-model.md](memory-model.md). Error codes: `cpc explain E0xxx` or
[errors.md](errors.md).

Sections: [Lexical](#lexical) · [Types](#types) · [Literals](#literals) ·
[Bindings](#bindings) · [Operators](#operators) ·
[Control flow](#control-flow) · [Functions](#functions) ·
[Structs](#structs) · [Unions](#unions) · [Enums](#enums) ·
[Tuples](#tuples) · [Patterns](#patterns) ·
[Generics & interfaces](#generics--interfaces) · [Strings](#strings) ·
[Arrays & slices](#arrays--slices) · [Pointers](#pointers) ·
[Modules & imports](#modules--imports) ·
[Platform-variant files](#platform-variant-files) ·
[Attributes](#attributes) · [Intrinsics](#intrinsics) ·
[Builder blocks](#builder-blocks) · [Async & threads](#async--threads) ·
[Tests & doctests](#tests--doctests) · [Manifest](#manifest) · [CLI](#cli)

## Lexical

```cplus
// line comment
/* block comment — nests */
/// doc comment: published by `cpc doc`, and a bare ``` fence in one
/// becomes a test (see Tests & doctests)
```

Identifiers are ASCII `[A-Za-z_][A-Za-z0-9_]*`. A leading `_` is privacy,
not a wildcard.

**Keywords:** `fn let const static if else while for in return true false
as extern struct enum union match impl export import this This defer break
continue loop assert guard opaque interface type async gen yield await
restrict`.

**Contextual keywords** — not reserved as tokens, but rejected as binding
names: `var`, `ref`, `take`.

**Reserved with a targeted rejection** — kept reserved so a habit from
another language gets a precise hint rather than a confusing parse error:

| Written | Diagnostic says |
|---|---|
| `mut` | retired — a mutating receiver is `ref this` |
| `move` | retired — a consuming receiver is `take this` |
| `borrow` | retired — a bare parameter `x: T` is already a read-only borrow |
| `trait` | C+ has no `trait`; declare a method contract with `interface` |
| `use` | C+ has no `use`; `import "path" as alias;` |
| `mod` | C+ has no `mod` and no module tree |
| `try` | there is no try / `?` propagation construct |
| `unsafe` | removed — write the block or operation directly |
| `pub` | retired — visibility is name-based; `export` marks the C-ABI surface |

The last two are **E0100**; the rest are parse errors carrying the same
kind of hint.

There are no loop labels: `break` and `continue` bind to the innermost
loop.

## Types

| Type | Description |
|---|---|
| `i8 i16 i32 i64 isize` | signed integers; `isize` is pointer-width |
| `u8 u16 u32 u64 usize` | unsigned; `usize` indexes, sizes, and bridges pointer↔int |
| `f16 f32 f64` | IEEE floats |
| `bool` | `true` / `false`; no integer coercion either way |
| `()` | unit; the return type of a `fn` with none written |
| `str` | borrowed string view: `(*u8, usize)`, Copy, never owns |
| `text::Text` | heap-owned string: `(*u8, len, cap)`, non-Copy (`import "stdlib/text"`) |
| `[T; N]` | fixed array, value type; `N` is a constant expression |
| `T[]` | slice — borrowed view over contiguous `T` |
| `*T` | raw pointer; deref `*p`, index `p[i]`, arithmetic strides by `sizeof(T)` |
| `fn(A, B) -> R` | function pointer; `fn(take A)` is a distinct, consuming pointer type |
| `(T, U)` | tuple type; literal `(a, b)` |
| `type X = T;` | transparent alias |
| `type X = distinct I;` | nominal integer brand: same ABI as `I`, separate type — construct/leave by `as`; comparison, `Hash`/`Eq`/`Copy` work; arithmetic rejected (cast out); mixing brands is E0302 |
| SIMD | `f32x4 f64x2 …` nineteen widths + `mask{N}x{M}`; `splat/new/load/from_array/to_array`, lane-typed methods; never crosses `extern fn` (round-trip `[f32; N]`). `sum()`/`product()` over narrow integer lanes wraps silently — W0001 |

## Literals

```cplus
42  42u64  3.14  1.5f16  true  1_000_000  0x1F  0b1010
'a'                 // u8 byte; '\n' '\xFF' escapes
"hello"             // str, always
c"hi\n"             // *u8, NUL-terminated, for C
"x = ${n}"          // interpolation — needs stdlib/text in the build (E0613);
                    // sink positions (io::print/println/eprintln, Text::append) never allocate;
                    // any other position builds an owned Text. No format specifiers.
[1, 2, 3]           // array literal
[0u8; 64]           // fill literal — memset fast path; count is any const expression
[]                  // only where the expected type is a zero-length array (E0332 elsewhere)
Point { x: 1, y: 2 }        // struct literal — fields always named
{ x: 1, y: 2 }              // type-inferred form where the target type is known
```

Unsuffixed integer literals evaluate as `i32` before any `as` — a wide mask
is built by widening the left operand first (`1u64 << 40`) or in a `const`
(which folds at the declared width and rejects overflow, E0921). Casting
after the shift is too late: `(1 << 40) as u64` is 256, and warns (W0007).

## Bindings

```cplus
let x: i32 = 5;        // immutable local: no rebind, no field writes, no ref/mutating methods
var z: i32 = 0;        // mutable local
let w: i32; w = 12;    // late init: first write counts; every path must assign (E0345)
const K: u64 = (1u64 << 40) - 1u64;   // module-scope VALUE: folded, typed, inlined, no address
static N: i32 = 0;     // module-scope mutable global: addressable, C-facing; access is bare
```

No `mut` exists. `const` initializers are constant expressions (may
reference other consts, any order, cycles rejected); `static` additionally
accepts array and non-generic struct literals, and a **function name** where
the type is a fn pointer — `static V: Vt = Vt { f: handler };` is the
dispatch table. A fn-pointer `const` is still E0911; use a `static`.
Cross-thread `static` safety is the developer's responsibility.

## Operators

| Class | Ops | Behavior |
|---|---|---|
| arithmetic | `+ - * / %` | overflow traps in debug, wraps in release; integer `/ 0` and `% 0` always trap. On floats, `/` and `%` are `fdiv`/`frem` — IEEE, no trap (`%` is C's `fmod`) |
| shifts | `<< >>` | `>>` arithmetic on signed, logical on unsigned. A **constant** distance at or past the left operand's width is W0007 — `(1 << 40) as u64` is 256, not 2^40 |
| wrapping | `+% -% *%` | always wrap |
| bitwise | `& \| ^ ~ << >>` | `>>` arithmetic on signed, logical on unsigned |
| comparison | `< <= > >= == !=` | `bool`, no coercion between operand types |
| logical | `&& \|\| !` | short-circuit |
| cast | `expr as T` | the only conversion; truncating on narrow; pointer↔int via `usize` only (E0315) |
| checked cast | `expr as? T` | integer→integer, `Option[T]`: `Some` iff the value fits |

No operator overloading. `==` compares scalars, pointers, payload-free enums,
and `str`/`Text` by content (either side may be the owned one). It does **not**
apply to any aggregate — struct, tuple, array, or payload-carrying enum — all
E0302. For a struct, an empty `impl T: Eq {}` derives a memberwise `a.eq(b)`;
arrays and payload enums are compared element-wise or by `match`.

## Control flow

```cplus
if cond { } else if other { } else { }
let r: i32 = if cond { 1 } else { 2 };          // expression form: both arms, same type
while cond { }
loop { break; continue; }
for i in 0..10 { }                              // exclusive; 0..=10 inclusive
for (var i: i32 = 0; i < n; i = i +% 1) { }     // C-style
while let Pat = expr { }
defer expr;                                     // runs at scope exit, LIFO, shared stack with drops
assert cond;                                    // traps on false — the only hard stop
```

`for … in` takes a range or an `Iterator[T]`, and nothing else (E0312).
`Vec` supplies one — `for x in v.iter()`. Arrays and slices do not: index
them over `0..n`. A parenthesized deref opening an `if` condition misparses:
write `if { (*p).field } == x`.

## Functions

```cplus
fn add(a: i32, b: i32) -> i32 { return a +% b; }
fn log(level: i32 = 1, tag: str = "app") { }     // defaults; call with names: log(tag: "net")
```

- `return` is explicit at function level (E0333). Exception: a unit `fn`
  whose body ends in `if`/`match`/block needs no trailing `return`.
- **Parameter modes** (the ownership grammar; details
  [ownership.md](ownership.md)): `x: T` read-only borrow · `ref x: T`
  exclusive write-back, caller's place must be `var` (E0328) ·
  `take x: T` move (source dead after, E0335) · `restrict p: *T` adds
  `noalias`. A bare borrow cannot escape the callee (E0337).
- Named arguments are call-site optional but follow Swift-guideline API
  style; two same-typed parameters should be called with names.
- **Callback pairs**: a stateful callback parameter is
  `(cb: fn(Args…, *u8) = 0 as fn(…), cb_ctx: *u8 = 0 as *u8)` — adjacent,
  ctx defaulted. A caller may then pass `recv.method` and the compiler
  fills the ctx (bound method reference). Codes: W0824/W0825 declaration
  shape, E0822/E0823/E0824 binding failures.
- An associated fn (no receiver) is a valid fn-pointer value
  (`Type::draw_imp`); a method with a receiver is not.

## Structs

```cplus
struct Point { x: i32, y: i32 }
struct S { value: i32, _hidden: i32 }        // _field = module-private

impl Point {
    fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }  // associated fn: Point::new
    fn norm(this) -> i32 { … }               // read method
    fn shift(ref this, dx: i32) { … }        // mutating: receiver must be var
    fn into_x(take this) -> i32 { … }        // consuming
}

impl Point { fn drop(ref this) { … } }       // destructor: a method NAMED drop,
                                             // not an interface impl; forces non-Copy
```

`Copy` is structural and never written: all components Copy and no `drop` →
Copy. Owning fields drop automatically in reverse declaration order after
the user `drop`; fields cannot be moved out of an owning aggregate (E0509).
Every `*T` field must be accounted for: freeing `drop` or `opaque _p: *T`
(E0510; conditional frees warn W0002).

## Unions

```cplus
#[repr(C)] union FloatBits { f: f32, bits: u32 }   // C layout, for binding headers
union Word { i: i32, u: u32 }                      // repr optional

var w: Word = Word { i: -1 };                      // exactly ONE member named
let bits: u32 = w.u;                               // reading another member reinterprets
```

One storage, several typed views, **no tag**. Consequences, both **E0925**:

- Every member type must be `Copy`. Without a tag the compiler cannot know
  which member is live, so no destructor could ever run.
- A union literal names exactly one member. Naming two is an error, not a
  last-write-wins.

Unions are for binding C headers. An either/or value in ordinary code is an
`enum`, which has a tag and can own its payload.

## Enums

```cplus
enum Color { Red, Green, Blue }                   // payload-free: i32, Copy
enum Shape { Circle(f64), Rect(f64, f64) }        // tagged union
enum Maybe[T] { Some(T), None }                   // generic

#[repr(u8)] enum Mode { Off = 0, Fast = 200 }     // FFI: pinned width, constant discriminants,
                                                  // C's prev+1 rule; payload-free only
```

Constructing a generic variant spells the type args
(`Maybe[i32]::Some(7)`, E0303 bare); matching may drop them where the
scrutinee fixes the instantiation — cross-module stdlib enums are spelled in
full (`option::Option[i64]::Some(v)`). An owning enum's payload drops via
tag switch; a `match` that binds a name consumes the enum
([ownership.md](ownership.md) §6).

## Tuples

```cplus
fn pair() -> (i32, i32) { return (1, 2); }

let t: (i32, i32) = pair();
let a: i32 = t.0;                     // positional access with `.`, zero-based
```

A structural product type: no name, no methods, no field labels. Use it for
a two-value return; anything with meaning to convey gets a `struct` with
named fields. Tuples inside a type are not derivable (E0920).

## Patterns

```cplus
match e {                                    // exhaustive or E0340; `_` is the catch-all
    Shape::Circle(r)  => …,
    Shape::Rect(w, h) => …,
}
match r {                                    // payload patterns nest, any depth
    Read::Ok(Option[i32]::Some(v)) => …,
    Read::Ok(Option[i32]::None)    => …,
    Read::Err(e)                   => …,
}
if let Maybe[i32]::Some(v) = m { }
while let option::Option[i64]::Some(v) = it.next() { }
guard let Read::Ok(v) = r else { return 1; };            // else must diverge
guard let Read::Ok(v) = r else |Read::Err(c)| { … };     // complement form: else binds the rest;
                                                         // both patterns together must cover the enum (E0340)
```

A payload position takes `_`, a binding name, or another variant pattern —
nesting is checked and counted toward exhaustiveness, so the three arms above
need no catch-all. One position per payload may discriminate; where a second
also does, coverage is not decided and E0340 asks for a catch-all. A variant
pattern over a non-enum payload is E0341.

All four take `var` in place of `let` for mutable bindings. `Some(_)` binds
nothing (reads the tag only, does not consume); `Some(_v)` binds — `_` on a
name is privacy, not a wildcard.

## Generics & interfaces

```cplus
fn larger[T: Ord](take a: T, take b: T) -> T {   // `take`: a bare param cannot be returned
    if a.cmp(b) > 0 { return a; }                // `.cmp`, not `>` — no operator overloading
    return b;
}
struct Pair[A, B] { first: A, second: B }
let v = vec::with_capacity::[i32](16 as usize);   // turbofish: name::[Args](…)

interface Shape {
    fn area(this) -> i32;                          // required
    fn describe(this) -> i32 { return this.area() *% 2; }   // default body — copied into impls
}
impl Sq: Shape { fn area(this) -> i32 { … } }
```

- Monomorphized; no `dyn`, no vtables — interface bounds dispatch through
  erased fn-pointers internally, but the source model is static.
- Bounds: `Eq`, `Ord`, `Hash`, `Clone`, `ToText`, plus any user interface.
  `Copy` is structural, `Send`/`Sync` are marker impls. Primitives satisfy all
  five: `cmp` is blessed on integers and floats (`-1` / `0` / `1`, unsigned
  and NaN-safe), `clone` on every Copy scalar and `str`. `bool` and `str` have
  no blessed `cmp` — bool has no useful order and `str` carries its own
  `compare` — so `T: Ord` still refuses them (E0502).
- **Deriving**: an *empty* `impl T: I {}` for the five blessed interfaces —
  `Eq`, `Ord`, `Hash`, `Clone`, `ToText` — generates the memberwise
  implementation for a **struct**. Arrays, tuples and payload-carrying enums
  inside one are not derivable (E0920): write by hand. An empty impl of any
  other interface is E0916 unless every method has a default.
- A **payload-free enum** needs no impl at all: it is a bare discriminant, so
  `eq` / `cmp` / `hash` / `clone` and the matching bounds already work on it —
  which is what lets it be a `HashMap` key. A payload-carrying enum is an
  aggregate and satisfies none of them.
- **Markers**: `impl Handle: Send {}` vouches a pointer-holding type across
  threads; conditional form `impl Arc[T: Send + Sync]: Send {}`. A type
  hiding a raw pointer is `!Send`/`!Sync` by default (E0502 at a bound).
- Never write mangled names in source (`Option__i32` is internal, E0405).

## Strings

| | `str` (view) | `text::Text` (owned) |
|---|---|---|
| build | literals; any read method; `t.view()` | `s.to_text()` · `text::from_str(s)` · `text::new()` · `text::with_capacity(n)` · interpolation at a binding |
| reads | **all of them live here** — `count` `is_empty` `char_count` `byte_at` `has_prefix` `has_suffix` `contains` `find` `rfind` `count_of` `compare` `equals_ignoring_case` `slice(from:,to:)` `prefix` `suffix` `drop_first` `drop_last` `removing_prefix` `removing_suffix` `trim` `trim_start` `trim_end` `split(separator:)` `to_i64` `to_f64` | reaches every `str` read through coercion; results are views into its buffer |
| mutation | — | `append` `insert` `truncate` `reserve` `clone` `uppercased` `appending` `replacing` `pad_start` `pad_end` … |

Enabled by `import "stdlib/str" as _;` (or transitively via `stdlib/text`).
Returns are views — no copy except `split`'s `Vec` and the spelled
`.to_text()`. `count()`, never `len()`. No `+` — interpolate or `append`.
View lifetime rules (E0513, write-lock, last-use end) —
[ownership.md](ownership.md) §5. `text::join(parts, separator:)` reverses
`split`. FFI tier: `#str_ptr` / `#str_len` / `#str_from_raw_parts`.

## Arrays & slices

```cplus
let a: [i32; 4] = [10, 20, 30, 40];   // a[i] bounds-checked; OOB traps
let z: [u8; CAP] = [0u8; CAP];        // fill; CAP any const expression
let s: T[] = v.as_slice();            // slice from Vec — arrays do NOT coerce to slices
slice::sub::[T](s, from, to)          // -> Option[T[]]; also prefix/suffix/drop_first/drop_last
```

Slices carry `count()` / `is_empty()`; `#slice_ptr`/`#slice_len` are the FFI
tier. Arrays and slices have no `iter()` — index them over `0..n`.
(`Vec` does: `for x in v.iter()`.)

## Pointers

```cplus
let p: *u8 = malloc(64 as usize);   // creation: `as *T` or an extern's return
*p; p[i]; p + 1;                    // deref, index, arithmetic (strides by sizeof)
let n: usize = #addr(p);            // pointer -> integer, loudly
let q: *T = #addr_of(place);        // place -> pointer
p.is_null(); p.is_not_null();       // blessed null tests, any raw/fn pointer
```

No `unsafe` blocks — the operations are individually visible instead
([ffi.md](ffi.md) §1). Struct fields of type `*T` demand an ownership
decision (E0510).

## Modules & imports

```cplus
import "stdlib/io" as io;      // dependency module — first segment is a [dependencies] name
import "./shell" as shell;     // file-relative
import "stdlib/str" as _;      // discard alias: extension methods only
```

Alias mandatory; `alias::item` is the only access path. `_`-prefixed items,
fields, and methods are module-private (E0403). Extensions on foreign types
apply only in files that import the defining module. Imports are
file-leading: an `import` after the first non-import item is an error.

A `.cplus` file under `src/` that no import reaches is **W0005** — it never
compiles, so nothing it declares is checked.

## Platform-variant files

```
src/reactor.cplus          # the base
src/reactor_linux.cplus    # shadows it when the target's platform is linux
```

`<module>_<platform>.cplus` shadows `<module>.cplus` for that target;
importers always write the base name (`import "./reactor" as reactor;`).
Platform names: `macos linux windows ios android esp32 wasm`.

| Rule | Detail |
|---|---|
| suffix source | the **target**'s platform (`--target ios-arm64` looks for `_ios`, never `_macos`) |
| android | tries `_android` first, then falls back to `_linux` — every other platform tries one suffix |
| base file | optional; a module may exist only as variants |
| no variant, no base | **E0401**, naming the *base* path |
| already suffixed | a `_linux` file is never re-suffixed |
| orphan check | platform-suffixed files are exempt from W0005 |
| library archive | the synthesized package entry imports base names only; a suffixed module with no base is kept only when its suffix names the active platform |

This is the only way to vary *imports* per platform. `#platform()` is
value-level and compiles both arms. Full model:
[platforms.md](platforms.md).

## Attributes

| Attribute | On | Effect |
|---|---|---|
| `#[test]` | fn | registered by `cpc test`; runs with a synthesized harness main |
| `#[requires(expr)]` / `#[ensures(expr)]` | fn | contracts, checked at entry / every return; `result` names the return value; traps on violation |
| `#[repr(C)]` · `#[repr(C, packed)]` · `#[repr(C, packed = N)]` | struct/union | C layout · no padding · `#pragma pack(N)` |
| `#[repr(u8…i64, C)]` | enum | pinned discriminant width (payload-free) |
| `#[bits(N)]` | integer field | C bitfield inside `#[repr(C)]` |
| `#[link_name = "sym"]` | extern fn | bind to a specific linker symbol |
| `#[inline]` / `(always)` / `(never)` | fn | LLVM inline hints; `always` works at -O0 |
| `#[no_alloc]` / `#[no_block]` / `#[max_stack(N)]` / `#[bounded_recursion]` | fn | real-time contracts, compiler-checked (E0901/E0906/E0907/E0908); project-wide via `[profile.realtime]` |
| `#[unroll(N)]` / `#[vectorize_width(N)]` | loop | optimizer hints |
| `#[deprecated("msg")]` | item | W0006 at each use |
| `#[naked]` | fn | asm-only body, no prologue/epilogue |
| `#[keeps(this)]` / `#[keeps(nothing)]` | fn/method | declared view-flow summary for a body the checker cannot read through: `this` = view arguments survive inside the receiver (lifts E0515); `nothing` = the function copies what it needs, so its return borrows no argument. Trusted, not verified |
| `#[watch]` | struct | field-write barrier: every store to a field is followed by `this.on_value(field_name)`. The struct must supply `fn on_value(ref this, field: str)` — missing is E0361, wrong signature is E0362, and an `on_value` on a struct *without* the attribute is W0004 (a hook that never fires) |
| `#[runtime_abi]` | extern fn | this declaration names a symbol the compiler generates (`__cplus_*`). The prefix is reserved; a declaration under it without the marker is E0919 |
| `#[lang("name")]` | struct/enum | marks the one stdlib declaration the compiler treats as a well-known type (`Text`, `Iterator`, `Future`, `Option`, `JoinHandle`). stdlib-authoring only |

Attribute-shape diagnostics: unknown name **E0354**, bad argument shape
**E0355**, wrong target **E0356**, illegal duplicate **E0357**. Attributes
never generate code by themselves — a pass reads the mark.

## Intrinsics

All spelled `#name(...)`:

| Intrinsic | Returns | Notes |
|---|---|---|
| `#size_of::[T]()` / `#align_of::[T]()` | `usize` | folds to a constant |
| `#addr_of(place)` | `*T` | place must be addressable |
| `#addr(p)` | `usize` | pointer → integer, loudly |
| `#zero::[T]()` | `T` | all-zero value; composes with field-sets |
| `#include_bytes("path")` | `*[u8; N]` | embeds the file; path relative to source |
| `#include_str("path")` | `str` | embedded, UTF-8 checked at build |
| `#env("NAME")` | `str` | build-time env var; E0876 if unset |
| `#platform()` | `str` | active *target's* platform name (`macos ios linux android windows esp32 wasm`); value-level only — both branches compile |
| `#arch()` | `str` | `aarch64` `x86_64` `xtensa` `riscv32` `wasm32` — crosses `#platform()`, does not refine it |
| `#target()` | `str` | the `--target` spec name (`host`, `ios-arm64`, `ios-arm64-simulator`, …) — the only axis that separates the iOS simulator from a device |
| `#println(x)` | `()` | type-dispatched primitive print; interpolation sinks |
| `#cpu_relax()` | `()` | spin-loop hint |
| `#asm("tmpl", name = in/out/inout(reg\|"x0") expr, clobber("r"))` | `()` | inline asm tiers 1–2; tier 3 is `#[naked]` |
| `#selector("name")` / `#msg_send(recv, "sel", …) -> R` | `*u8` / `R` | the ObjC tier |
| `#compile_shader("f.metal", "msl")` | `*[u8; N]` | platform shader compiler at build time |
| `#str_ptr(s)` `#str_len(s)` `#str_from_raw_parts(p, n)` | `*u8` / `usize` / `str` | the `str` ↔ C bridge |
| `#slice_ptr(s)` `#slice_len(s)` `#slice_from_raw_parts(p, n)` | `*T` / `usize` / `T[]` | the slice ↔ C bridge |
| `#bswap16` `#bswap32` `#bswap64` · `#htons` `#htonl` `#ntohs` `#ntohl` | same width in | byte-order tier |

An unknown `#name(...)` is **E0905**.

## Builder blocks

`@ctx { … }` — declarative construction with no macros; the compiler owns
syntax + lowering, a package supplies `Item`, leaf constructors,
`Builder { new / add(ref this, Item) / finish(take this) -> Root }`, and
container fns `fn name(b: Builder) -> Item`:

```cplus
let screen = @view {
    text("Inbox")
        .font = bigger              // line-leading .x = modifier on the item above
        .on_click(refresh)
    if n > 0 { badge(n) }           // collection-if adds into this block
    vstack {                        // container: bare name { } — never @name
        for r in rows { item(r) }
    }
};
```

Bare names resolve locals → same-file → `ctx::name`. Allowed inside: items,
modifiers, `let`, `if`/`else`, `for … in`, containers. Rejected:
`while`/`loop`, `return`/`break`/`continue`, `defer`/`guard`,
`yield`/`await`, nested `@`. Lowers before sema to `Builder::new` + `add` +
finisher — diagnostics land on your lines.

## Async & threads

```cplus
async fn fetch() -> i32 { return (await inner()) +% 1; }
fn main() -> i32 { return executor::block_on::[i32](fetch()); }   // main is never async

let h = thread::spawn_with::[In, Out](data, worker);  // moves data in; h.join() -> Out
var s: thread::Scope = thread::scope();               // s.lend::[T](local, f) — joined at scope drop
gen fn nums() -> i32 { yield 1; yield 2; }            // -> Iterator[i32]
// adapters: it.filter(pred) · it.prefix(n) · iterator::map::[T, U](it, f)
```

Borrow-shaped types are rejected in `async fn` signatures (E0900) — pass
`Text`/`Vec`. `spawn` requires `Send` payloads (E0502). Shared state:
`mutex::Mutex[T]` (internally refcounted; guards scope-per-lock),
`stdlib/atomic`, `arc::Arc[T]`, `channel` MPMC.

## Tests & doctests

```cplus
#[test] fn name() { assert cond; }          // fails by trapping
#[test] fn name() -> i32 { return 0; }      // fails on a nonzero return
```

Signature must be `fn()` or `fn() -> i32` (**E0358**). Free functions only:
on a method it is **E0356**, on an `export` fn **E0359**.

A fence inside a `///` comment becomes a test named
`DOC_TEST::<item>::<index>`:

````text
/// ```
/// assert add(2, 3) == 5;
/// ```
````

The fence opens **only** on a line that trims to exactly three backticks —
a ` ```cplus ` fence is not extracted and its example silently never runs.

`cpc test` entry ladder, first match wins: `src/test_main.cplus` → the
platform's app entry → the `[library]` target → `src/<package>.cplus`.
Discovery covers the whole resolved import tree, dependencies included.
Exit 0 on all-pass, 2 on any failure. Details: [testing.md](testing.md).

## Manifest

`Cplus.toml` — full model in [packages.md](packages.md):

| Key | Meaning |
|---|---|
| `[package] name/version/edition` | identity; edition is `"2026"` |
| `[package] entry = "src/…"` | app entry; default `src/main.cplus` when the file exists, no `[library]`, and no platform entry is declared |
| `[<platform>] entry` | per-platform entry; declaring any scopes the app (E0413 elsewhere). Platforms: `macos linux windows ios android esp32 wasm` |
| `[dependencies]` / `[<platform>.dependencies]` | flat, complete; `name = "*"` or a tree-URL spec |
| `[android.maven]` | third-party Maven/AAR pins: `"group:artifact" = "version"`, exact, no wildcard. Android only (E0877 elsewhere); `cpc pm add . --maven G:A:V` writes one and downloads its closure |
| `[library] kind/entry/name` | C-ABI product: `staticlib`(default)/`cdylib`/`both`; explicit `entry` = bare C names |
| `[link] frameworks/libs/search-paths/extra-objects` | the link surface; `${VAR}` expansion in paths. A dependency's `[link]` travels to its consumers |
| `[link] bundled` | basenames of binaries this package ships at `lib/<triple>/`; the triple is derived, never declared. Declared-but-missing is E0860, undeclared-but-present is E0861 |
| `[library] name/frameworks/libs` | the product library's output name and its own link additions |
| `[build] prebuild/dev` | prebuild defaults ON; `prebuild = false` opts out, `dev = true` = work-on-it override |
| `[profile.realtime] deny-alloc/deny-block/deny-unknown-extern/stack-limit` | project-wide real-time contracts, synthesized onto every fn in *this* package |

Keys are kebab-case and the schema is closed: an unknown key or a misspelled
platform section is **E0406**, never a silently-ignored line. `[[bin]]` and
`[lib]` were removed — they parse into **E0408** carrying the migration.

What a build produces is the target's: self-linked platforms → executable
(entry has `fn main`, E0414); external-builder → `lib<name>.a` + header
(entry is `export extern fn`, `fn main` is E0409); no entry → library
archive of all of `src/`.

## CLI

```bash
# build and run
cpc build [-o OUT]                  # multi-file: reads ./Cplus.toml, walks imports
cpc FILE.cplus [-o BIN]             # single file, no imports, no manifest
cpc check [FILE]                    # no FILE: whole project, front end only
cpc test [FILE] [--json]            # #[test] + doctest discovery and run
cpc init [--kind cli|gui] [--platform P]... [NAME]

# build flags (apply to `cpc FILE`, `cpc build`, `cpc test`)
--release | --debug                 # -O3 wrapping | -O0 trapping (default)
-g | --debug-info                   # DWARF
--asan | --ubsan | --tsan | --msan  # asan/tsan/msan mutually exclusive
--target NAME [--min-os VERSION]    # cross-compile; --min-os goes AFTER --target
--fp-contract=off|on|fast           # `off` = bit-identical-to-C float output
--warn-deps                         # dependencies' warnings too (default: own src/ only)
--timings                           # per-phase and per-package build cost
--diagnostics=human|short|json

# the code graph — prefer it over grep
cpc mcp                             # resident MCP server: live overlays, non-blocking reads
cpc query <kind> [args]             # def · members · symbols · refs · callers · callees
                                    # call-hierarchy [--depth N] · context
                                    # type-at/value-refs/scope-at FILE:LINE:COL
                                    # complete FILE:LINE:COL — the composed caret answer
cpc graph                           # the whole graph as JSON
cpc lsp [--log PATH]                # resident: goto-def, refs, hover, outline, completion

# source, docs, diagnostics
cpc fmt FILE|DIR [--check|--emit|--stdin]
cpc doc FILE                        # -> target/doc/<basename>.md
cpc headers                         # -> lib/include/ for this package
cpc explain E0xxx | --list          # offline diagnostic manual
cpc skill [--lang-only]             # the agent reference, embedded in the binary

# packages
cpc pm install|update [DIR]         # resolve deps into the store (~/.cplus)
cpc pm add DIR NAME [SPEC] [--platform P]...
cpc pm remove DIR NAME
cpc pm manifest [DIR]               # normalized JSON
#   flags: --local --store DIR --cache DIR --repo-url URL

# introspection
cpc --tokens|--ast|--emit-ll|--emit-ll-opt|--emit-asm FILE
cpc --emit-obj FILE -o OUT.o
cpc --emit-header FILE              # C header for export items
cpc --emit-ll-project               # merged IR for the project
cpc build --print-link-args         # what the DEPENDENCIES add to the link line
cpc --realtime-report[=json]        # contract digest; non-zero on any violation
```

Targets: `host` (default), `ios-arm64`, `ios-arm64-simulator`,
`android-arm64`, `esp32-xtensa`, `esp32c3-riscv32`. Place `--target` and
`--fp-contract` before an inline emit flag and its file. Cross-target
artifacts land in `target/<target-name>/<mode>/`.

`cpc check FILE` does not read the manifest — a file with any `import`
fails there with E0852 — and no form of `check` invokes clang, so invalid IR
passes it and fails only in a build. Full model: [tooling.md](tooling.md).
