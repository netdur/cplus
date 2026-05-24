# C+ vs Rust

This comparison is grounded in the current project source, especially:

- `cplus-core/src/lexer.rs`
- `cplus-core/src/parser.rs`
- `cplus-core/src/ast.rs`
- `cplus-core/src/sema.rs`
- `cplus-core/src/borrowck.rs`
- `cplus-core/src/lower.rs`
- `cplus-core/src/monomorphize.rs`
- `cplus-core/src/codegen.rs`
- `cpc/src/main.rs`
- real C+ libraries under `vendor/stdlib` and `vendor/*`

The short version: C+ deliberately borrows a lot of Rust's surface vocabulary,
but it is not "Rust with different punctuation". It is a smaller systems
language with explicit move parameters, structural `Copy`, `interface` bounds,
direct LLVM IR generation, C-style FFI as a first-class path, and several
language features Rust either expresses through libraries/macros or does not
have as stable syntax.

## Source Anchors

Concrete source locations used while writing this:

- `cplus-core/src/lexer.rs:40-124`: token vocabulary, including keywords,
  wrapping operators, ranges, turbofish/path punctuation, and extern varargs.
- `cplus-core/src/parser.rs:179-245`: import ordering and top-level item
  dispatch.
- `cplus-core/src/parser.rs:1031-1304`: functions, generic parameter lists,
  parameter ownership prefixes, borrowed types, raw pointers, tuples, and
  function-pointer types.
- `cplus-core/src/ast.rs:300-890`: receivers, function flags, parameters,
  types, statements, expressions, patterns, operators.
- `cplus-core/src/sema.rs:60-138`: semantic type model.
- `cplus-core/src/sema.rs:281-329`: struct metadata and parameter signature
  ownership flags.
- `cplus-core/src/sema.rs:1234-1272`: structural `Copy` derivation.
- `cplus-core/src/sema.rs:7901-7964`: `move` parameter and `move self`
  consumption rules.
- `cplus-core/src/borrowck.rs:85-193`: place/projection overlap and
  place-state merging.
- `cplus-core/src/lower.rs:42-60`: lowering pass entry, const substitution,
  and pattern-let lowering setup.
- `cplus-core/src/monomorphize.rs:56-135`: generic expansion and call/type
  rewriting.
- `cplus-core/src/codegen.rs:966-1015`: LLVM IR generation setup and fastcc
  decision.
- `cplus-core/src/codegen.rs:4027-4078`: function codegen, async/gen routing,
  and extern declaration emission.
- `cplus-core/src/codegen.rs:8337-8421`: binary operation lowering, pointer
  arithmetic, overflow/zero checks.
- `cpc/src/main.rs:1964-2081`: end-to-end build pipeline.
- `vendor/stdlib/src/option.cplus:12-22`: generic enum plus `move` constructor.
- `vendor/stdlib/src/thread.cplus:55-87`: `Send` bounds, moved thread input,
  consuming join handle.
- `vendor/arena/src/arena.cplus:35-151`: extern declarations, unsafe pointer
  arithmetic, generic allocation.

## Compiler Pipeline

C+ currently has a direct pipeline:

1. `lexer::tokenize`
2. `parser::parse`
3. attribute validation
4. lowering
5. semantic analysis
6. borrow checking
7. monomorphization
8. LLVM IR generation
9. `clang` link/optimization

That sequence is visible in `cpc/src/main.rs`: lex/parse happen around
`build_ir`, then attributes, lowering, sema, borrowck, monomorphize, and codegen
run in order. `build_ir` calls `lower::lower`, `sema::check_multi_with_mono`,
`borrowck::check`, `run_monomorphize`, and finally `codegen::generate_with_mono`.

Rust's compiler pipeline is more layered: parse, macro expansion, name
resolution, HIR, type checking, trait solving, MIR construction, MIR borrow
checking, MIR optimizations, monomorphization, then LLVM or another backend.

The practical difference is that C+ is still much closer to its AST during
most checks. Rust relies heavily on MIR as the semantic center of ownership,
borrows, drops, and control flow.

## Lexical Surface

C+ recognizes a Rust-like keyword set: `fn`, `let`, `mut`, `const`, `static`,
`if`, `else`, `while`, `for`, `in`, `return`, `struct`, `enum`, `match`, `impl`,
`pub`, `unsafe`, `extern`, `async`, `await`, `loop`, `break`, `continue`, and
`Self`/`self`. The lexer also reserves or implements C+-specific keywords:
`defer`, `move`, `restrict`, `guard`, `assert`, `gen`, `yield`, `borrow`,
`interface`, and `type`.

Source evidence: `TokenKind` in `cplus-core/src/lexer.rs` defines these tokens,
including C+-specific wrapping operators `+%`, `-%`, and `*%`, plus `...` for
variadic extern declarations.

Rust has `trait`, `where`, `crate`, `super`, `ref`, `dyn`, `async`, `await`,
`move`, `unsafe`, and macro punctuation, but it does not have C+'s `interface`,
`defer`, `guard`, `restrict`, `gen fn`, or `borrow REGION T` syntax.

## Top-Level Items

C+ top-level items include:

- `fn`
- `extern fn`
- `struct`
- `enum`
- `impl`
- `interface`
- `type`
- `const`
- `static`

The parser dispatch is in `parse_item` in `cplus-core/src/parser.rs`. It accepts
`pub` on items, rejects `pub impl`, and routes `async fn`, `gen fn`, `extern fn`,
interfaces, aliases, consts, and statics through distinct parser paths.

Rust top-level items include many more forms: modules, `use`, traits, impls,
type aliases, constants, statics, extern blocks, macros, visibility modifiers,
and attributes. C+ has a narrower item model, with modules/imports handled by a
string-path resolver rather than Rust's module tree.

## Imports and Modules

C+ imports look like this:

```cplus
import "stdlib/option" as option;
```

The parser requires imports to appear before all other items. `parse_program`
first consumes leading `import "path" as alias;` declarations and then parses
items. `parse_import_decl` requires a string literal path and a mandatory alias.

Rust uses module paths and `use`:

```rust
use std::option::Option;
mod math;
```

C+ import names are explicit aliases. Cross-file access is guarded later by the
resolver. `resolver.rs` checks whether imported items and methods are `pub` for
cross-file access. Rust's module privacy is also lexical and path-based, but it
is integrated with the module tree instead of string-path package loading.

## Visibility

C+ has `pub` on functions, structs, enums, interfaces, type aliases, consts,
statics, struct fields, and methods. Struct fields are private by default even
when the struct itself is public. Methods inside an `impl` are individually
public or private.

This is visible in the AST:

- `StructField` stores `is_pub`.
- `Method` stores `is_pub`.
- `Function`, `StructDecl`, `EnumDecl`, `InterfaceDecl`, aliases, consts, and
  statics store `is_pub`.

Rust has a richer visibility system: private by default, `pub`, `pub(crate)`,
`pub(super)`, `pub(in path)`. C+ currently appears closer to binary public vs
private visibility.

## Functions

C+ function syntax is Rust-like:

```cplus
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}
```

The parser reads optional `async`, optional `gen`, then `fn`, name, generic
parameter list, value parameters, optional return type, and body.

Important C+ differences:

- `main` must return `i32`. Sema enforces `fn main() -> i32`.
- Function generics use square brackets: `fn id[T](x: T) -> T`.
- Explicit type arguments use `::[...]`: `id::[i32](7)`.
- Parameter prefixes carry ownership and aliasing meaning: `mut`, `move`,
  `borrow`, and `restrict`.

Rust:

```rust
fn id<T>(x: T) -> T {
    x
}
```

Rust's `main` normally returns `()` or a type implementing `Termination`. Rust
generic parameters use angle brackets, and turbofish uses `::<T>`.

## Parameters and Move Semantics

C+ has an important semantic split that Rust does not:

```cplus
fn read(x: T) -> i32
fn take(move x: T) -> i32
fn bump(mut x: T)
fn raw(restrict p: *u8)
```

The `Param` AST stores:

- `mutable`
- `move_`
- `restrict`
- `borrow_`

Sema tracks these in `ParamSig`. `check_arg_with_move` only consumes the caller's
argument when the callee parameter is marked `move` and the type is non-Copy.
The source place must currently be a whole local binding; partial moves from
fields or indexes are rejected.

This is a major difference from Rust. In Rust, passing a non-`Copy` value by
value consumes it by default:

```rust
fn take(x: String) {}
```

In C+, the ownership transfer is part of the callee signature:

```cplus
fn take(move x: string) { }
```

The standard library uses this pattern. For example, `vendor/stdlib/src/option.cplus`
defines:

```cplus
pub fn some[T](move v: T) -> Option[T] {
    return Option[T]::Some(v);
}
```

`vendor/stdlib/src/thread.cplus` also uses `spawn_with[I: Send, O: Send](move input: I, ...)`
and `JoinHandle[O]::join(move self) -> O` so moved thread input and one-shot join
handles are enforced by the type checker and borrow checker.

## Receivers and Methods

C+ receiver forms are:

```cplus
fn get(self) -> T
fn set(mut self, value: T)
fn into_value(move self) -> T
```

The AST represents these as `Receiver::Read`, `Receiver::Mut`, and
`Receiver::Move`. Comments in `ast.rs` note that `self` and `mut self` lower to
pointer parameters, while `move self` transfers ownership and makes the caller's
place unavailable.

Rust receiver forms are:

```rust
fn get(&self) -> T
fn set(&mut self, value: T)
fn into_value(self) -> T
```

C+ deliberately avoids Rust's `&self` spelling at method declaration sites.
`self` in C+ is closer to a read receiver than Rust's by-value `self`.

## Types

C+ semantic types are represented in `sema::Ty`. Current types include:

- signed ints: `i8`, `i16`, `i32`, `i64`
- unsigned ints: `u8`, `u16`, `u32`, `u64`
- pointer-sized ints: `isize`, `usize`
- floats: `f32`, `f64`
- `bool`
- `()`
- `str`
- `string`
- slices: `T[]`
- raw pointers: `*T`
- function pointers: `fn(T) -> U`
- structs
- enums
- arrays: `[T; N]`
- SIMD vectors and masks
- generic params

Rust has a comparable primitive set, `str`, `String`, arrays, slices, raw
pointers, function pointers, structs, enums, tuples, and generics. But Rust's
reference types `&T` and `&mut T` are central, while C+ currently has region
borrow syntax and place-state analysis layered around parameter/receiver forms.

## Strings

C+ has two built-in string concepts:

- `str`: a Copy string view lowered to `{ ptr, len }`
- `string`: an owned heap-backed string lowered to `{ ptr, len, cap }`

This is visible in `Ty::Str` and `Ty::String` in `sema.rs`. Interpolated string
literals produce owned `string`, and normal string literals produce `str`.

Rust has `&str` for borrowed string slices and `String` for owned strings.
C+ makes the borrowed string view a plain value type named `str`, not a reference
type with an explicit lifetime in source.

## Arrays and Slices

C+ supports:

```cplus
let xs: [i32; 3] = [1, 2, 3];
let view: u8[] = some_slice;
```

`TypeKind::Array` stores element type and length. `TypeKind::Slice` is a
fat-pointer view `{ptr, len}`. Codegen emits runtime bounds checks for indexing.

Rust's `[T; N]` and `[T]` are similar conceptually, but Rust normally handles
slices behind references such as `&[T]` and `&mut [T]`.

## Generics

C+ generic syntax:

```cplus
fn identity[T](x: T) -> T { return x; }
struct Pair[A, B] { first: A, second: B }
enum Option[T] { Some(T), None }
let x: Option[i32] = Option[i32]::Some(7);
let y = identity::[i32](7);
```

Bounds use `interface` names:

```cplus
fn spawn_with[I: Send, O: Send](move input: I, f: fn(I) -> O) -> JoinHandle[O]
```

`monomorphize.rs` expands generic functions, structs, and enums into concrete
instances and rewrites generic type and call sites to mangled names before
codegen. This is broadly similar to Rust's monomorphization of generic code, but
C+ exposes different syntax and has a smaller trait/interface model.

Rust generic syntax:

```rust
fn identity<T>(x: T) -> T { x }
struct Pair<A, B> { first: A, second: B }
enum Option<T> { Some(T), None }
```

Rust supports much richer bounds: trait bounds, associated types, lifetimes,
where clauses, higher-ranked trait bounds, const generics, blanket impls, and
special coherence rules.

## Interfaces vs Traits

C+ has `interface`:

```cplus
interface Ord {
    fn cmp(self, other: Self) -> i32;
}

impl Ord for Point {
    fn cmp(self, other: Point) -> i32 { ... }
}
```

The AST has `InterfaceDecl` and `InterfaceMethod`, and sema validates interface
implementation method coverage/signature matching. Generic params store bounds
as interface names.

Rust's equivalent is `trait`, but Rust traits are much more expressive:
associated types, associated constants, default methods, generic associated
types, object safety, `dyn Trait`, supertraits, blanket impls, and more.

C+ interfaces currently look like a focused compile-time method contract for
bounded generics.

## Enums, Matches, and Patterns

C+ enums can be plain or tagged:

```cplus
enum Maybe[T] {
    Some(T),
    None,
}

return match value {
    Maybe[i32]::Some(v) => v,
    Maybe[i32]::None => 0,
};
```

`ExprKind::Match` and `PatternKind::Variant` model matches and variant patterns.
Sema checks enum matches for exhaustiveness and pattern/type consistency.

Current C+ patterns are more limited than Rust's. The AST comments state that
variant payload patterns are one nesting level and payload patterns are wildcard
or binding. Rust supports deeply nested patterns, guards, ranges, slice
patterns, `@` bindings, destructuring structs/tuples/enums, reference patterns,
and more.

## If-Let, While-Let, Guard-Let

C+ has `if let`, `while let`, and `guard let`.

The lowering pass rewrites pattern-let constructs into `match`-using forms
before sema. `lower.rs` walks function and method bodies and transforms these
statements after recursively lowering nested bodies.

Rust has `if let`, `while let`, and `let ... else`, but not this exact
`guard let PATTERN = EXPR else { ... };` syntax. C+'s guard-let explicitly
keeps successful bindings in the enclosing scope after proving the else branch
diverges.

## Copy, Drop, and Destruction

C+ uses structural `Copy`:

- primitives, raw pointers, function pointers, string views, slices, SIMD, masks
  are atomic Copy
- arrays are Copy if their element type is Copy
- structs are Copy if all fields are Copy and the struct has no `drop`
- enums are Copy if payloads are Copy
- `string` is non-Copy and has Drop

`sema.rs` computes struct copy flags with a fixpoint, and `is_copy` dispatches
to array, struct, enum, or atomic-copy logic.

Rust requires explicit `Copy` impl/derive for user types and prohibits `Copy`
on types with destructors. C+ automatically derives Copy structurally.

C+ destructors are methods named `drop` with signature:

```cplus
fn drop(mut self)
```

Rust destructors implement `Drop`:

```rust
impl Drop for T {
    fn drop(&mut self) { ... }
}
```

C+ codegen tracks drops for non-Copy locals and strings. `codegen.rs` registers
drop slots and emits scope exits. C+ also has a `defer EXPR;` statement; Rust
does not have built-in defer syntax and normally uses RAII guard types.

## Borrow Checking

C+ borrow checking is place-state based. The borrow checker models:

- a `Place` root plus projections like fields and indexes
- overlap: same, contains, contained, disjoint
- states: `Owned`, `BorrowedShared(n)`, `BorrowedExclusive(name)`, `Moved`,
  `MaybePartial`
- branch merging through `PlaceState::merge`

This is visible in `borrowck.rs`.

Rust's borrow checker is deeper and MIR-based. It reasons about references,
lifetimes, non-lexical lifetimes, reborrows, drops, partial moves, two-phase
borrows, and trait interactions.

C+ currently implements a useful subset:

- explicit `move` parameters and `move self` consume non-Copy whole bindings
- read/mut receivers and mut parameters are tracked
- aliasing conflicts are based on place overlap
- branch merges can produce maybe-moved state

The project source also notes some deferred pieces: return-borrow tracking,
lifetime elision, full partial-place tracking, and replacing older sema move
tracking with the more precise borrow checker path.

## Unsafe and FFI

C+ has raw pointers and extern declarations:

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn printf(fmt: *u8, ...) -> i32;

let p: *u8 = unsafe { malloc(4 as usize) };
unsafe { *p = 1u8; }
```

Unsafe blocks are AST expressions. Sema rejects extern calls, pointer deref, and
other unverifiable operations outside `unsafe`. Codegen emits non-public extern
functions as LLVM `declare` statements and relies on the platform C ABI.

The real libraries use this heavily:

- `vendor/arena/src/arena.cplus` declares `malloc`, `free`, `memcpy`, `memset`
  and performs raw pointer arithmetic inside `unsafe`.
- `vendor/log/src/log.cplus` declares C functions and uses `static mut`.
- `vendor/appkit/src/runtime.cplus` wraps Objective-C runtime calls.

Rust also has raw pointers, `unsafe`, and FFI, but extern functions are declared
inside `extern "C"` blocks and Rust's unsafe model is integrated with a much
larger aliasing/reference model.

## Consts and Statics

C+ supports module-level:

```cplus
const NAME: T = LIT;
static NAME: T = LIT;
static mut NAME: T = LIT;
```

The AST says `const` is lowered away: use sites are substituted with a clone of
the initializer. `static` survives to codegen and becomes an LLVM global.
`static mut` reads and writes require `unsafe`.

Rust has const evaluation and statics, including `static mut` requiring unsafe.
Rust consts are far more capable; C+ currently restricts const/static
initializers to literal-like shapes.

## Arithmetic

C+ has both normal and explicit wrapping operators:

```cplus
a + b
a +% b
a -% b
a *% b
```

The lexer recognizes `+%`, `-%`, and `*%`. Sema has separate `AddWrap`,
`SubWrap`, and `MulWrap` operators. Codegen emits debug overflow checks for
signed normal `+`, `-`, and `*`, while unsigned arithmetic wraps. Division and
modulo emit zero checks. Wrapping operators avoid overflow checks.

Rust uses normal `+`, `-`, `*`, debug overflow checks, optimized release
behavior, and library methods such as `wrapping_add`, `checked_add`,
`saturating_add`, plus `std::num::Wrapping`.

C+'s distinct wrapping operators are closer to a language-level spelling for a
common systems operation.

## Async and Generators

C+ supports:

```cplus
pub async fn read_async(...) -> isize {
    return await net::read_fd_async(...);
}

pub gen fn range(start: i32, end: i32) -> i32 {
    yield i;
}
```

The AST stores `is_async` and `is_gen` on functions and methods. Sema rewrites
`async fn foo() -> T` to `Future[T]` and `gen fn foo() -> T` to `Iterator[T]`.
Codegen routes both to coroutine lowering paths.

Rust async uses:

```rust
async fn foo() -> T { ... }
let x = future.await;
```

Rust's stable iterator story is normally library-based `Iterator`
implementations, iterator adapters, and `async` futures. C+ has a direct
`gen fn` plus `yield` surface in this project.

## Builtins and Macros

C+ currently has compiler builtins shaped like macro calls:

```cplus
include_bytes!("file.bin")
include_str!("file.txt")
env!("NAME")
size_of::[T]()
align_of::[T]()
```

`ExprKind` has explicit variants for `IncludeBytes`, `IncludeStr`, and `EnvVar`.
Sema resolves file/env data at compile time, and codegen emits globals.

Rust has a general macro system and standard built-in macros such as
`include_bytes!`, `include_str!`, `env!`, `println!`, etc. C+ does not appear to
have user-defined macros in the current compiler source.

## Code Generation and ABI

C+ emits textual LLVM IR directly. Codegen allocates locals with `alloca` and
relies on LLVM optimization passes like mem2reg. `cpc` then invokes `clang`,
using `-O0` for debug and `-O3` for release.

Codegen also makes ABI choices visible:

- non-public internal functions may use `fastcc`
- `main` keeps C calling convention
- extern declarations emit LLVM `declare`
- raw pointers lower to opaque LLVM `ptr`
- `str` and slices are fat pointers
- `string` is a three-field owned aggregate
- tagged enums lower to tagged payload storage

Rust also usually uses LLVM for optimized native builds, but rustc hides most
ABI/lowering detail behind MIR, ABI classification, and backend abstractions.
C+ is currently more direct and explicit.

## Standard Library Style

C+ stdlib code looks like systems Rust mixed with C FFI:

- `Option[T]` is a generic enum in source.
- `thread::spawn_with` uses `move input: I` to transfer ownership into a worker.
- `JoinHandle[O]::join(move self)` makes double-join a compile-time move error.
- `Arena` wraps `malloc` and pointer arithmetic behind safe-ish C+ methods but
  uses explicit `unsafe` internally.
- `Arc[T]`, `Vec[T]`, `HashMap[K, V]`, IO, networking, futures, reactor, and
  appkit bindings are source-level C+ libraries.

Rust's standard library has similar concepts, but it relies on mature traits,
lifetimes, unsafe abstractions, allocators, panic/unwind behavior, and decades of
API polish.

## Practical Syntax Mapping

| Concept | C+ | Rust |
| --- | --- | --- |
| Function | `fn f(x: i32) -> i32 { return x; }` | `fn f(x: i32) -> i32 { x }` |
| Generic fn | `fn id[T](x: T) -> T` | `fn id<T>(x: T) -> T` |
| Explicit generic call | `id::[i32](7)` | `id::<i32>(7)` |
| Generic type | `Option[i32]` | `Option<i32>` |
| Bound | `fn f[T: Send](x: T)` | `fn f<T: Send>(x: T)` |
| Trait/interface | `interface Send { ... }` | `trait Send { ... }` |
| Impl interface | `impl Ord for Point { ... }` | `impl Ord for Point { ... }` |
| Read receiver | `fn get(self)` | `fn get(&self)` |
| Mut receiver | `fn set(mut self)` | `fn set(&mut self)` |
| Consuming receiver | `fn join(move self)` | `fn join(self)` |
| Consuming parameter | `move x: T` | `x: T` for non-Copy values |
| Shared string | `str` | `&str` |
| Owned string | `string` | `String` |
| Slice | `T[]` | `[T]`, usually behind `&[T]` |
| Raw pointer | `*T` | `*const T` / `*mut T` |
| Unsafe block | `unsafe { ... }` | `unsafe { ... }` |
| Wrapping add | `a +% b` | `a.wrapping_add(b)` |
| Defer | `defer expr;` | no built-in, use RAII guard |
| Async wait | `await future` | `future.await` |
| Generator | `gen fn ... { yield x; }` | usually manual/library `Iterator` |
| Import | `import "path" as alias;` | `use path::item;` / `mod` |

## How C+ Feels Compared to Rust

C+ feels Rust-inspired where it matters for systems programming:

- expression blocks
- `fn`, `let`, `mut`
- enums and `match`
- `impl`
- `pub`
- `unsafe`
- generics and bounds
- ownership and `Copy`
- `Drop`-like destruction

But C+ chooses different pressure points:

- Move is explicit at the parameter/receiver declaration.
- Shared passing is the default for non-Copy values unless a `move` signature
  asks to consume.
- Generic syntax is square-bracket based.
- Interfaces are simpler than traits.
- Compile-time lowering and monomorphization are direct AST-to-AST phases.
- Unsafe FFI is a common library-building path, not a rare escape hatch.
- The compiler emits LLVM IR directly, with many ABI decisions visible in codegen.
- `defer`, `guard let`, wrapping operators, and `gen fn` are first-class syntax.

So, if Rust is a mature language centered on lifetimes, traits, MIR, and a large
ecosystem, this C+ project is closer to a compact Rust/C hybrid: Rust-like
syntax and ownership ideas, but with explicit move signatures, simpler generic
contracts, direct LLVM lowering, and a standard library that exposes the C/OS
boundary more openly.
