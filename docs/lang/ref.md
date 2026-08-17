# C+ reference

The manual: every construct, lookup-shaped, one entry each. Signatures and
behavior only — learning path in [tour.md](tour.md), judgment in
[guide.md](guide.md), normative text in [spec.md](spec.md) /
[memory-model.md](memory-model.md). Error codes: `cpc explain E0xxx` or
[errors.md](errors.md).

Sections: [Types](#types) · [Literals](#literals) · [Bindings](#bindings) ·
[Operators](#operators) · [Control flow](#control-flow) ·
[Functions](#functions) · [Structs](#structs) · [Enums](#enums) ·
[Patterns](#patterns) · [Generics & interfaces](#generics--interfaces) ·
[Strings](#strings) · [Arrays & slices](#arrays--slices) ·
[Pointers](#pointers) · [Modules & imports](#modules--imports) ·
[Attributes](#attributes) · [Intrinsics](#intrinsics) ·
[Builder blocks](#builder-blocks) · [Async & threads](#async--threads) ·
[Manifest](#manifest) · [CLI](#cli)

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
| SIMD | `f32x4 f64x2 …` nineteen widths + `mask{N}x{M}`; `splat/new/load/from_array/to_array`, lane-typed methods; never crosses `extern fn` (round-trip `[f32; N]`) |

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
Point { x: 1, y: 2 }        // struct literal — fields always named
{ x: 1, y: 2 }              // type-inferred form where the target type is known
```

Unsuffixed integer literals evaluate as `i32` before any `as` — a wide mask
is built arithmetically or in a `const` (which folds at the declared width
and rejects overflow, E0921).

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
accepts array and non-generic struct literals. Cross-thread `static` safety
is the developer's responsibility.

## Operators

| Class | Ops | Behavior |
|---|---|---|
| arithmetic | `+ - * / %` | overflow traps in debug, wraps in release; `/ 0` always traps |
| wrapping | `+% -% *%` | always wrap |
| bitwise | `& \| ^ ~ << >>` | `>>` arithmetic on signed, logical on unsigned |
| comparison | `< <= > >= == !=` | `bool`, no coercion between operand types |
| logical | `&& \|\| !` | short-circuit |
| cast | `expr as T` | the only conversion; truncating on narrow; pointer↔int via `usize` only (E0315) |
| checked cast | `expr as? T` | integer→integer, `Option[T]`: `Some` iff the value fits |

No operator overloading. `==` on `str`/`Text` compares contents (and the
two compare with each other through coercion).

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

Arrays and `Vec` are not `for … in` iterable (E0312) — iterate `0..n` by
index, or an `Iterator[T]` from `gen fn`. A parenthesized deref opening an
`if` condition misparses: write `if { (*p).field } == x`.

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

impl Point: Drop { fn drop(ref this) { … } } // destructor; forces non-Copy
```

`Copy` is structural and never written: all components Copy and no `drop` →
Copy. Owning fields drop automatically in reverse declaration order after
the user `drop`; fields cannot be moved out of an owning aggregate (E0509).
Every `*T` field must be accounted for: freeing `drop` or `opaque _p: *T`
(E0510; conditional frees warn W0002).

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

## Patterns

```cplus
match e {                                    // exhaustive or E0340; `_` is the catch-all
    Shape::Circle(r)  => …,
    Shape::Rect(w, h) => …,
}
if let Maybe[i32]::Some(v) = m { }
while let option::Option[i64]::Some(v) = it.next() { }
guard let Read::Ok(v) = r else { return 1; };            // else must diverge
guard let Read::Ok(v) = r else |Read::Err(c)| { … };     // complement form: else binds the rest;
                                                         // both patterns together cover the enum (E0349)
```

All four take `var` in place of `let` for mutable bindings. `Some(_)` binds
nothing (reads the tag only, does not consume); `Some(_v)` binds — `_` on a
name is privacy, not a wildcard.

## Generics & interfaces

```cplus
fn max[T: Ord](a: T, b: T) -> T { … }
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
- Bounds: `Ord`, `Eq`, `Hash`, plus any user interface. `Copy` is
  structural, `Send`/`Sync` are marker impls.
- **Deriving**: an *empty* `impl T: I {}` for the five blessed interfaces —
  `Eq`, `Ord`, `Hash`, `Clone`, `ToText` — generates the memberwise
  implementation. Payload enums / arrays / tuples inside are not derivable
  (E0920): write by hand. An empty impl of any other interface is E0916
  unless every method has a default.
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
tier. Iterate arrays and slices by index.

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
apply only in files that import the defining module. Platform-variant
implementation: `<module>_<platform>.cplus` shadows `<module>.cplus` for
that target ([packages.md](packages.md) §6).

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
| `#[keeps(this)]` | param | the receiver retains the borrow (library-authoring tier) |

Unknown attributes are rejected (E0354); attributes never generate code by
themselves.

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
| `#println(x)` | `()` | type-dispatched primitive print; interpolation sinks |
| `#cpu_relax()` | `()` | spin-loop hint |
| `#asm("tmpl", name = in/out/inout(reg\|"x0") expr, clobber("r"))` | `()` | inline asm tiers 1–2; tier 3 is `#[naked]` |
| `#selector("name")` / `#msg_send(recv, "sel", …) -> R` | `*u8` / `R` | the ObjC tier |
| `#compile_shader("f.metal", "msl")` | `*[u8; N]` | platform shader compiler at build time |
| `#str_ptr` `#str_len` `#str_from_raw_parts` `#slice_ptr` `#slice_len` `#bswap32` … | — | FFI/byte tier |

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
gen fn nums() -> i32 { yield 1; yield 2; }            // iterator; map/filter/take adapters
```

Borrow-shaped types are rejected in `async fn` signatures (E0900) — pass
`Text`/`Vec`. `spawn` requires `Send` payloads (E0502). Shared state:
`mutex::Mutex[T]` (internally refcounted; guards scope-per-lock),
`stdlib/atomic`, `arc::Arc[T]`, `channel` MPMC.

## Manifest

`Cplus.toml` — full model in [packages.md](packages.md):

| Key | Meaning |
|---|---|
| `[package] name/version/edition` | identity; edition is `"2026"` |
| `[package] entry = "src/…"` | app entry; default `src/main.cplus` when the file exists, no `[library]`, and no platform entry is declared |
| `[<platform>] entry` | per-platform entry; declaring any scopes the app (E0413 elsewhere). Platforms: `macos linux windows ios android esp32 wasm` |
| `[dependencies]` / `[<platform>.dependencies]` | flat, complete; `name = "*"` or a tree-URL spec |
| `[library] kind/entry/name` | C-ABI product: `staticlib`(default)/`cdylib`/`both`; explicit `entry` = bare C names |
| `[link] frameworks/libs/search-paths/extra-objects` | the app's link surface; `${VAR}` expansion in paths |
| `[build] prebuild/dev` | prebuild defaults ON; `prebuild = false` opts out, `dev = true` = work-on-it override |
| `[profile.realtime] deny-alloc/deny-block/deny-unknown-extern/stack-limit` | project-wide real-time contracts |

What a build produces is the target's: self-linked platforms → executable
(entry has `fn main`, E0414); external-builder → `lib<name>.a` + header
(entry is `export extern fn`, `fn main` is E0409); no entry → library
archive of all of `src/`.

## CLI

```bash
cpc init [--platform P]... [NAME]  # scaffold; --platform scopes the app ([ios] entry, E0413 elsewhere)
cpc build [--release] [--target NAME] [--asan|--ubsan|--tsan|--msan] [-g]
cpc FILE.cplus -o BIN           # single file, no imports
cpc check [FILE]                # front-end only; project mode reads Cplus.toml
cpc test [FILE] [--json]        # #[test] discovery + run
cpc fmt FILE | --check DIR
cpc explain E0xxx | --list      # offline diagnostic manual
cpc skill                       # print the agent reference (docs/lang/skill.md)
cpc graph | query … | mcp | lsp # the resolved code graph — prefer it over grep
cpc headers                     # C+ declaration files for a package
cpc --emit-ll|--emit-ll-opt|--emit-asm|--emit-obj|--emit-header FILE
cpc --diagnostics=json          # NDJSON diagnostics
cpc --realtime-report[=json]
cpc pm …                        # fetch tree-URL dependencies into vendor/
```

Targets: `host` (default), `ios-arm64`, `ios-arm64-simulator`,
`android-arm64`, `esp32-xtensa`, `esp32c3-riscv32`. Place `--target` before
inline emit flags. Debug builds are `-O0` with overflow traps; `--release`
is optimized with wrapping arithmetic.
