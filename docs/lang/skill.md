# SKILL — writing C+

**You are about to write a language with no training corpus.** Whatever you
remember about C+ you are remembering from Rust, C, Swift or Go, and C+ wears
Rust's vocabulary over C's semantics — the false friends are the whole problem.
So this file does not try to be a dictionary. The toolchain is the dictionary,
and it is offline, in your shell, version-matched to the code in front of you.

**What `cpc` tells you, don't learn from here:**

| Question | Ask |
|---|---|
| Is this spelled right? Does it compile? | `cpc check` (no imports) / `cpc build` |
| What does this error mean, and how is it fixed? | `cpc explain E0337` — 194 codes, each with a cause, a fix and a worked example |
| Where is X / who calls X / what is the type here? | `cpc query def\|refs\|callers\|type-at`, or `cpc mcp` |
| What methods does this type have? What are this function's parameters? | `cpc query members TYPE` / `cpc query def FN` |
| What is in the stdlib? | §11, then `cpc query symbols` |

**What this file is for: the things that COMPILE and are still wrong.** A clean
build is not a correct program, and the mistakes C+ invites are design mistakes —
an ownership shape that forces a clone on every call, a callback declared so no
caller can ever pass a method to it, a raw pointer nobody frees. The compiler
cannot report those. That is the whole content here.

Read it as direction, not as syntax to copy exactly. If you get a keyword wrong,
`cpc check` will say so in one line and you will fix it in one line. If you get
the ownership model wrong you will write a file that builds and rewrite it twice.

**§12 is how you build an app** — components, services, and how they meet. Those
mistakes all compile, so no diagnostic will find them for you. A package also
ships its own agent reference, and `cpc skill` prints this file plus every
dependency's, so run that rather than reading this file alone.

**Project:** <https://cplus-lang.dev> · **Source:** <https://github.com/netdur/cplus>
· every docs page is LLM-readable markdown — append `.md` to any URL.

---

## 1. What C+ is

A systems language: LLVM backend, manual memory, no GC, ownership with a borrow
checker (aliasing XOR mutability), one-way C ABI (cpc emits standard object
files; `.c` does not compile). It is designed to be written correctly by
machines, which shows up as three biases you should share: **explicit beats
clever, locality beats indirection, and the type system is asked to carry
weight.**

The language surface is **small and deliberately frozen.** New capability lands
as packages (§13) and tooling, not syntax. If a task seems to need a language
feature, it almost certainly needs a package — say so rather than inventing
syntax that will not parse.

Files are `.cplus`. A project is `Cplus.toml` at the root, sources in `src/`,
dependencies in `vendor/`. Imports are explicit and aliased, with no extension:

```cplus
import "./math" as math;          // local — starts with `./`
import "stdlib/io" as io;         // a dependency — first segment is its name
import "stdlib/str" as _;         // extension-only: enables `str` methods, binds no name
```

> **Anything with an `import` must go through `cpc build`.** `cpc check FILE`
> does not read the manifest and fails with E0852 on the first import — it is
> for single-file, import-free snippets. And `cpc check` never runs clang, so it
> cannot catch invalid IR: when the question is "does this actually compile",
> build it.

### Scaffolding

`cpc init` writes the manifest, an entry, and the agent files. Use it rather than
assembling a project by hand:

```bash
cpc init my_app                            # host CLI app
cpc init --kind gui --platform macos ui    # a facet app, backend closure included
```

The manifest has **no `[[bin]]` / `[lib]` sections.** A package with an entry is
an app (`src/main.cplus` by default, `entry = "..."` overrides); a package with
no entry is a library and `cpc build` archives its whole `src/`. What a build
*produces* is the target's fact: self-linked platforms (macos, linux, windows)
get an executable from `fn main`; external-builder platforms (ios, android,
esp32) get `lib<name>.a` plus a C header, entered through an `export extern fn`
the platform shell calls. Declaring any `[<platform>] entry` scopes the app to
those platforms and every other one fails (E0413) rather than guessing.

---

## 2. Thirteen principles, and they are compiler-enforced

These are not house style. Each one is a diagnostic, so proposing a violation
wastes a round-trip — the build will refuse it.

| # | Principle | The shape you write instead |
|---|---|---|
| 1 | No `null` | `Option[T]`. FFI null is `0 as *T`, tested with `p.is_null()` |
| 2 | No closures or lambdas | A named `fn`, plus a `*u8` context parameter beside it (§5) |
| 3 | No `&T` / `&mut T` types | The caller relation is a parameter *prefix* — `ref` / `take` / bare (§4) |
| 4 | No exceptions, no `try`, no `?` | Errors are enum values; `match` or `guard let` (§7) |
| 5 | No implicit conversions | Every width change is an explicit `as` |
| 6 | No overloading | One name, one signature |
| 7 | No macros, decorators, or comptime | Compiler-known attributes and `#name(...)` intrinsics only |
| 8 | No `class`, no `function` | `struct` + `impl`, `fn`; locals are `let` / `var` |
| 9 | No `mut` keyword | `var` is a mutable local, `static` a mutable global, `ref` a write-back parameter |
| 10 | Generics are `[T]`, never `<T>` | `Vec[i32]`, turbofish `::[i32]` |
| 11 | No implicit tail return | Explicit `return EXPR;` for a value. A unit fn needs no trailing `return;` |
| 12 | `::` for types, `.` for instances | `Type::assoc_fn()` vs `value.method()` |
| 13 | Private by leading `_`, public by default | `_field`, `_fn` are module-private; `export` marks the C ABI |

Two of these change how you *design*, not just how you type:

**No overloading (6) means a name is claimed once, for the whole build.** You
cannot add a second `parse` that takes something else. Pick names that say what
the argument is — `parse_i64`, `parse_header` — from the start, because renaming
later touches every caller.

**No closures (2) means state travels beside the code, not inside it.** Every
callback API in C+ is a pair of parameters, and getting that pair wrong is the
single most common design error in the language. §5 is about nothing else.

---

## 3. The memory model

This is the part with no analogue in what you already know, and it is where
generated C+ goes wrong. Learn the six facts below and most of the borrow
checker stops firing.

### 3.1 The relation to the caller is a PREFIX on the parameter

There is no `&T` and no `&mut T`, at the declaration or at the call site. How a
function relates to its caller's value is one keyword in front of the parameter,
and there is exactly one of them per parameter.

| Written | Means | The caller |
|---|---|---|
| `x: T` | **read-only borrow**, for every type | keeps `x`, may use it after |
| `ref x: T` | **write-back** — the callee mutates the caller's value | must have `x` as a `var` |
| `take x: T` | **ownership moves in** | cannot use `x` again |
| `restrict p: *T` | adds LLVM `noalias` to a raw pointer | — |

```cplus
import "stdlib/text" as text;

fn describe(t: text::Text) -> usize { return t.count(); }   // borrow: caller keeps t
fn bump(ref n: i32) { n = n +% 1; }                         // write-back
fn consume(take t: text::Text) -> usize { return t.count(); } // moves in

fn main() -> i32 {
    var n: i32 = 0;
    bump(n);                        // NOT bump(&n) — there is no `&` in this language
    // n is now 1

    let t: text::Text = "hi".to_text();
    let a: usize = describe(t);     // t still mine afterwards
    let b: usize = consume(t);      // t is gone
    // describe(t);                 // E0335 — use of a moved value
    return 0;
}
```

Method receivers are the same three, spelled `this` / `ref this` / `take this`.
The name is always `this`; `ref` and `take` are the modifier, and `This` is the
enclosing type.

**Both checks are made at the CALL SITE from the signature alone.** The compiler
never reads the callee's body to decide, which is why they keep working through
fn-pointers, interfaces and generics — and why the signature is the contract you
should design deliberately rather than let fall out of the implementation.

### 3.2 A BORROW MAY BE READ BUT MAY NOT ESCAPE

This is E0337, the error you will hit most, and it is not a borrow-checker
technicality — it is the model refusing to create a second owner. A bare
parameter can be read, indexed, printed, passed on to another bare parameter.
What it cannot do is outlive the call: be returned, stored in a field or a
global, or handed to a `take`.

```cplus
struct Cache { last: text::Text }

impl Cache {
    // ✗ E0337 — `s` is borrowed; storing it would make the field a second owner
    // fn remember(ref this, s: text::Text) { this.last = s; return; }

    // ✓ say what you meant. Either the caller gives it up …
    fn remember(ref this, take s: text::Text) { this.last = s; return; }
}

// … or you pay for a copy, and you pay for it VISIBLY:
fn remember_copy(ref this, s: text::Text) { this.last = s.clone(); return; }
```

**The fix belongs in the signature, not at the call site.** An author who hits
E0337 inside a method and reaches for `.clone()` has moved a cost onto every
caller forever, silently. Ask first whether this function should own the thing —
if it stores it, it should — and write `take`. `.clone()` is right when the
caller genuinely still needs its copy.

### 3.3 `Copy` IS STRUCTURAL, AND A `drop` REVOKES IT

You never write `Copy` and you never derive it. Every component Copy → the
struct is Copy. Define `fn drop(ref this)` and the type becomes non-Copy and
move-only, necessarily — a copyable value with a destructor is a double free.

```cplus
struct Point { x: i32, y: i32 }             // Copy: assignment duplicates it

struct Buf { _ptr: *u8, _len: usize }
impl Buf {
    fn drop(ref this) { free(this._ptr); }  // this line makes Buf move-only
}
```

The consequence to plan for: **the moment a struct owns heap memory, every
function that gives it away needs `take` or `.clone()`, transitively, all the way
up.** Deciding late that a type owns something is an edit across its whole call
graph. Decide when you declare it.

### 3.4 TEARDOWN IS AUTOMATIC AND RECURSIVE — DO NOT HAND-WRITE IT

When a value goes out of scope the compiler runs its `drop` if it has one, then
drops every **owning field** in reverse declaration order. It recurses into
structs, arrays and the active payload of a tagged enum.

```cplus
struct Person {
    name: text::Text,
    tags: vec::Vec[text::Text],
}
// No `drop` is written and none is wanted. Dropping a Person frees `tags`
// (and every Text inside it) and then `name`.
```

Writing a `drop` here would double-free. The only fields that are *not* handled
are raw pointers — see 3.6.

Three consequences that look like compiler bugs when you meet them cold:

- **You cannot move an owning field out of an aggregate** (E0509). The auto-drop
  would free it a second time. Clone the field, or `match` to consume the whole
  value.
- **`match` on an OWNED enum consumes it**, and what triggers consumption is
  *binding a name*. Its drop is suppressed and the payload becomes yours; the
  scrutinee cannot be read again (E0335). A `match` that binds nothing reads only
  the discriminant and consumes nothing — that is how you write a presence check
  on a value you still need.

  ```cplus
  match slot {                        // presence check — binds nothing, consumes nothing
      option::Option::Some(_) => { }
      option::Option::None    => { return 0; }
  }
  match slot {                        // still mine, still matchable for real
      option::Option::Some(t) => { return t.count() as i32; }
      option::Option::None    => { return 0; }
  }
  ```

  `Some(_v)` **binds** — a leading `_` is the privacy convention, never a
  wildcard — so it consumes exactly like any other name. `Some(_)` is the
  non-consuming form.
- **`take this` does not disarm the exit drop.** A consuming method still runs
  the destructor when its receiver dies at the end of it. Return the inner value
  and let the drop free the shell; freeing it by hand is a double free.

`defer` shares one scope-exit stack with `Drop`: both are popped LIFO,
interleaved in declaration order.

### 3.5 A VIEW NEEDS A NAMED OWNER

`str` is a borrowed `(ptr, len)` view; `text::Text` is the heap-owning string. A
`Text` coerces to `str` wherever one is expected, so a `str` slot takes a `Text`
directly — no `.as_str()`. But at a **binding**, the thing being viewed must be
somebody's binding. A temporary has no lifetime to lend, and that is E0513:

```cplus
// ✗ every one of these is E0513 — the owner is an anonymous temporary
// let s: str = t.clone();
// let s: str = "x = ${n}";
// let s: str = make().view();

let owner: text::Text = t.clone();     // name it …
let s: str = owner.view();             // … then view it

f("x = ${n}");                         // ✓ an ARGUMENT's temp outlives the call
```

**The borrow ends at the view's last use, not at scope end.** So this compiles:

```cplus
let v: str = t.trim();
use(v);
t.append("!");        // fine — `v` was last used above
```

A use inside a loop pins the borrow past the loop; a use in a `defer` or as a
block tail pins it to scope exit. When a write is rejected and the view looks
dead, look for a later use you forgot about.

### 3.6 EVERY RAW POINTER FIELD MUST BE ACCOUNTED FOR

A `*T` field is not auto-dropped, and C+ refuses to let that be silent. Either
the struct frees it, or you say in the declaration that somebody else does
(E0510 if you say neither):

```cplus
struct Owned { _ptr: *u8 }
impl Owned { fn drop(ref this) { free(this._ptr); } }   // I free it

struct Borrowed { opaque _ptr: *u8 }                     // somebody else frees it
```

The check is structural, not dataflow — it reads your `drop` body and grades what
it can prove. An unconditional `free(this.f)` is clean; a conditional one
(refcount, flag, loop) is **W0002** and is expected for `Arc`/`Rc`-shaped owners;
delegating the free to a helper reads as no free at all and is E0510.

**`opaque` is a claim you are making, not a check the compiler performs.** Use it
only when another owner really does free it — an FFI handle the runtime owns, a
borrowed view, a pointer a sibling owns. It is the same trust model as
`#[keeps(...)]`.

### 3.7 What the conflicts mean, in the order you will meet them

| Code | You wrote | Fix, best first |
|---|---|---|
| E0337 | a borrow escaped (returned, stored, re-passed to `take`) | change the signature to `take`; else `.clone()` |
| E0335 | used a value after it moved | reorder, or `.clone()` before the move |
| E0305 | assigned to a `let`, or to a field of one | make the binding `var` |
| E0328 | passed a `let` to `ref`, or called a mutating method on a `let` | make the binding `var` |
| E0509 | moved an owning field out of a struct | clone the field, or consume the whole value |
| E0513 | a view outlived its owner | give the owner a name |
| E0510 | an unaccounted `*T` field | free it in `drop`, or mark it `opaque` |
| E0370/E0380 family | overlapping borrows | add a `{ }` scope so the earlier borrow ends |

Reach for these in order: **narrow the scope** so a borrow ends earlier; make the
binding `var`; change the parameter to `take`; `.clone()`; restructure ownership.
Not every conflict is a scoping problem — some are the model telling you two
things claim to own one value, and no amount of bracket-shuffling fixes that.

---

## 4. Functions

### 4.1 The signature is the design decision

Because there is no overloading and no closures, a C+ signature carries more
weight than in most languages: it fixes the name for the whole build, it states
every ownership relation (§3.1), and it decides whether a caller will ever be
able to pass a method to it (§4.2). Write it deliberately.

```cplus
fn area(w: f64, h: f64) -> f64 { return w * h; }      // explicit `return` for a VALUE
fn log_it(m: str) { io::println(m); }                 // a unit fn needs NO trailing `return;`
```

E0333 is the missing `return`. It fires because there are no implicit tail
returns *at function level* — an `if` used as an expression still yields its
block tails (`let r = if c { 1 } else { 2 };`).

**Named parameters with defaults exist and are underused.** They are the way to
keep one name (principle 6) while letting a call say what it means:

```cplus
fn chrome(title: str, width: f64 = 800.0f64, height: f64 = 500.0f64,
          resizable: bool = true) -> Chrome { ... }

chrome("Iris");                                  // defaults
chrome("Iris", width: 1200.0f64, resizable: false);   // say only what differs
```

Positional arguments must all precede named ones (E1004).

### 4.2 A CALLBACK IS TWO PARAMETERS, ADJACENT, CONTEXT SECOND, DEFAULTED

There are no closures, so a stateful callback is the code *and* the object it
runs on — and C+ writes that pair as two parameters and wires them for you. The
shape is not negotiable, and getting it wrong is the most expensive mistake in
this section because **it fails in somebody else's file.**

```cplus
// DECLARING a function that takes a callback. The `*u8` slot is not optional.
fn each_row(on_click: fn(str, *u8) = 0 as fn(str, *u8),
            on_click_ctx: *u8 = 0 as *u8) { ... }

// CALLING it with a component's own method — no cast, no address, no ctx arg
each_row(on_click: this.open_project)

// or with a free fn, threading the context yourself
fn opened(path: str, ctx: *u8) { ... }
each_row(on_click: opened, on_click_ctx: #addr_of(this) as *u8)
```

`this.open_project` is a **bound method reference**: the compiler synthesizes a
bridge for the handler slot and fills the `*u8` after it with `#addr_of(this)`.
The method's shape must be the handler's parameters *minus* the trailing `*u8`,
with the same return type — here `fn open_project(ref this, path: str)`.

**Omit the ctx slot and no caller can ever pass a method — only a free fn.** The
author writes one parameter, a caller in another file writes `this.method`, and
E0824 fires at a call site that cannot be fixed without editing the declaration.
`W0824` warns at the declaration and prints the line to add; heed it there.
Store a handler in a struct field the same way: the `*u8` goes in the field
beside it.

**PASS MORE THAN ONE HANDLER BY NAME.** Each handler's context is the `*u8`
immediately after it, so positionally the second handler lands in the FIRST
one's context slot:

```cplus
row(on_click: this.open, on_long_press: this.menu)   // ✓
row(this.open, this.menu)                            // ✗ rejected
```

The compiler does stop this — E0824 for a bound method, E0312 for a free fn —
but neither message says "you wanted named arguments", so it reads as a problem
with the handler rather than with the call shape. With three or more handler
pairs, naming them is the only form that stays readable anyway.

**And when a shape looks like it cannot bind, write the three-line test before
believing it.** Guessing at what binds is how a top-level `fn handler(ctx: *u8)`
with a hand-cast context gets written — the manual form of what the compiler
already does, and the clearest tell of code written from memory rather than from
the compiler.

| Code | What it says |
|---|---|
| W0824 | this declaration has no ctx slot — a bound method will never fit it |
| W0825 | the ctx is FIRST; a bound method reads it from the LAST parameter |
| E0824 | the callee has no slot, or the call tried to fill it itself |
| E0823 | the method's shape does not fit the handler |
| E0822 | this method cannot be bound: `take this`, generic, or has `ref`/`take` params |

### 4.3 An ASSOCIATED FN has an address; a METHOD does not

A fn with no receiver is a namespaced free fn, so `Type::f` is a legal
fn-pointer value — which is what an ObjC IMP, a C callback, or any raw function
slot needs. It lets the callback live on the type it belongs to instead of as a
loose top-level fn beside it:

```cplus
impl LineGutter {
    fn draw_imp(view: *u8, rect: rt::Rect) { ... }      // no `this`
}
let imp: fn(*u8, rt::Rect) = LineGutter::draw_imp;      // its address
```

A method (one *with* a receiver) has no address of this shape — `fn(this, …)` is
not `fn(…)` — and says so with E0312. Both forms are type-directed: without an
expected `fn(...)` type on the other side, `Type::f` is E0312 too. A generic
associated fn has no single address and is E0821.

### 4.4 A LABEL NAMES A PARAMETER OF THE RECEIVER'S OWN METHOD

Two types may declare the same labeled method name. A call resolves against the
parameter list of the type the receiver actually has, so the two never interfere:

```cplus
impl A { fn go(ref this, v: i32, ctx: i32 = 0) -> i32 { ... } }
impl B { fn go(ref this, ctx: i32, v: i32 = 9) -> i32 { ... } }

a.go(v: 5, ctx: 3);   // A's order  — 5, 3
a.go(ctx: 3, v: 5);   // same call, labels written the other way round
a.go(v: 5);           // A's own default fills ctx — 0, not B's 9
b.go(ctx: 4);         // on a B receiver `ctx` is the FIRST parameter
a.go(ctx: 3);         // ✗ error — A's `v` has no default and none was given
```

The compiler reaches this in two passes, which matters only when it cannot: the
lowering pass rewrites a labeled call into a positional one but runs before
types exist, so it keys candidates by bare method name and settles only what
every candidate agrees on; anything type-dependent it leaves to sema, which
knows the receiver and arranges the call from that type's declaration.

**Two callees genuinely have no parameter names, and E1002 says so:**

- a **fn-pointer value** — `fn(i32, i32)` records parameter types, not names,
  and that holds for a fn-pointer in a struct field too;
- a **generic receiver** — until `T` is instantiated it is not one type, so no
  one parameter list belongs to it.

Both take positional arguments. Nothing else needs designing around: sharing a
labeled verb across types is fine, and `reload(then:, then_ctx:)` on two stores
no longer breaks either one's callers.

### 4.5 Generics: bounds, turbofish, and where the args are noise

```cplus
fn largest[T: Ord](a: T, b: T) -> T { ... }        // bounds: Ord, Eq, Hash, Send, Sync
let v = vec::with_capacity::[i32](16);             // turbofish is ::[T], never ::<T>
```

*Constructing* a generic enum value needs its type args — nothing else says which
instantiation you mean. *Matching* one does not, because the scrutinee already
fixed it:

```cplus
fn find(k: str) -> option::Option[i32] { return option::Option[i32]::Some(42); }

match find("answer") {
    option::Option::Some(v) => v,      // ✓ prefer this — no type args in patterns
    option::Option::None    => 0,
}
```

Restating the type in a pattern is not merely noise: it is the arm's only
dependency on a type the compiler already derived, so changing that type turns
every arm into an error (E0341) with nothing wrong in the logic. Written short,
the same change touches none of them.

Never write a mangled name (`Option__i32`). It is internal and is rejected in
source (E0405) even where an old diagnostic printed one.

---

## 5. Structs, enums, interfaces

### 5.1 A struct is a VALUE, and `let` freezes the whole of it

There is no `mut`. `let` freezes the value *and* its fields, so a field write
needs the binding to be `var` — and so does calling any `ref this` method on it.
Two different codes, because they are two different checks: **E0305** is the
assignment to an immutable place, **E0328** is a mutable receiver (or a `ref`
argument) that was not `var`.

```cplus
struct Point { x: i32, y: i32 }

impl Point {
    fn new(x: i32, y: i32) -> Point { return Point { x: x, y: y }; }   // assoc fn
    fn sum(this) -> i32 { return this.x +% this.y; }                   // reads
    fn shift(ref this, dx: i32) { this.x = this.x +% dx; }             // mutates
    fn into_x(take this) -> i32 { return this.x; }                     // consumes
}

let p: Point = Point::new(1, 2);
// p.x = 9;        // ✗ E0305 — assignment to an immutable place
// p.shift(1);     // ✗ E0328 — a mutating method needs a `var` receiver
var q: Point = Point::new(1, 2);
q.shift(1);        // ✓
```

**Field shorthand does not exist.** Write `Point { x: x, y: y }`, never
`Point { x, y }`. Where the type is already known — an annotated binding, a
`return`, an argument — drop the type name instead:

```cplus
let p: Point = { x: 1, y: 2 };
return { x: 1, y: 2 };
```

**A leading `_` is privacy, at every level** — items, fields, methods. It is not
a "don't care" marker anywhere in this language, which is why `Some(_v)` binds
and consumes (§3.4). `export` is the separate, louder marker for the C ABI
surface.

### 5.2 Derive with an EMPTY IMPL, for exactly five interfaces

An empty `impl Type: Interface {}` against one of the five blessed interfaces
asks the compiler to generate the memberwise implementation. No attribute, no
macro — the same idiom as a `Send` marker impl:

```cplus
struct Key { id: i64, name: str }
impl Key: Eq {}        // fn eq(this, other: Key) -> bool
impl Key: Ord {}       // fn cmp(this, other: Key) -> i32
impl Key: Hash {}      // fn hash(this) -> u64
impl Key: Clone {}     // fn clone(this) -> Key
impl Key: ToText {}    // fn to_text(this) -> text::Text

var m = hash_map::new::[Key, i32]();      // derived Hash + Eq satisfy K's bounds
```

What that generation cannot do, and what to write instead:

- **A payload-free enum needs no impl at all.** It is a bare discriminant, so
  `eq`/`cmp`/`hash`/`clone` and their bounds already work — which is what makes
  one usable as a `HashMap` key. An empty impl on one is E0916 telling you to
  delete it.
- **A payload-CARRYING enum satisfies none of them.** Write the method by hand.
- **Array, slice and tuple fields are not derivable** (E0920) — hand-write that
  one method.
- Deriving targets structs only. An empty impl of a *user* interface stays an
  error (E0916) unless every method it declares has a default body (§5.4).
- `Copy` is never written and never derived — it is structural (§3.3).

### 5.3 Enums are the error type, the option type, and the state machine

```cplus
enum Color { Red, Green, Blue }               // payload-free: lowers to i32, Copy
enum Shape { Circle(f64), Rect(f64, f64) }    // tagged
enum Maybe[T] { Some(T), None }               // generic

#[repr(u8)]                                   // FFI: payload-free + explicit width
enum Mode { Off = 0, Slow = 10, Fast = 200 }  // crosses the C ABI as uint8_t
```

Matching is exhaustive (a gap is E0340) and **payload patterns nest to any
depth, with the nesting counting toward exhaustiveness** — so these three arms
need no catch-all:

```cplus
return match r {
    Read::Ok(Maybe::Some(v)) => v,
    Read::Ok(Maybe::None)    => 0,
    Read::Err(e)             => 0 -% e,
};
```

`guard let` is the dominant idiom for the fallible path — pattern-or-diverge,
where the else must `return`/`break`/`continue`/`loop`:

```cplus
fn handle(s: str) -> i32 {
    guard let ParseResult::Ok(v) = parse(s) else { return 0 -% 1; };
    return v +% 100;                       // `v` is in scope for the rest of the fn
}
```

The else may name a pattern before its block, so the failure payload is not
thrown away. The two patterns together must cover the enum — the form lowers to
a match, so a gap is E0340 and an overlap is E0350:

```cplus
guard let ReadResult::Ok(v) = read(s) else ReadResult::Err(code) {
    return 0 -% code;
};
```

`if let`, `while let` and `guard let` all also take `var` instead of `let`,
making the bindings mutable.

**A distinct newtype is the cheapest bug prevention in the language.** Two ids of
the same underlying integer is the classic silent swap no borrow checker catches;
one line makes it a type error:

```cplus
type UserId = distinct i64;
type ChannelId = distinct i64;

let u = 7 as UserId;              // construct by casting in
let n: i64 = u as i64;            // leave by casting out
// take_user(channel);            // ✗ E0302 — brands do not mix
// let v: UserId = 5;             // ✗ E0302 — the base does not flow in silently
```

Same representation, same ABI, separate type. Arithmetic and ordering are
rejected on the brand (cast to the base first); `Hash`/`Eq`/`Copy` work, so it
serves as a `HashMap` key, and `Vec[UserId]` is genuinely its own type.

### 5.4 Interfaces are monomorphized — there is no `dyn`

An interface method may carry a body instead of a `;`. The body is **copied into
every impl block that omitted it, before sema**, so it monomorphizes like any
other method and `This` means the implementing type:

```cplus
interface Shape {
    fn area(this) -> i32;                                  // must be written
    fn describe(this) -> i32 { return this.area() *% 2; }  // default
}
impl Sq: Shape { fn area(this) -> i32 { return this.s *% this.s; } }   // gets describe
```

A default that calls a method the implementor lacks is an error against *that
type*, at that impl. An interface whose methods all have defaults takes an empty
impl.

**Marker impls are assertions you are making.** `Send`/`Sync` are refused
automatically to any struct or enum that hides a raw pointer (E0502 at the
bound). Writing the impl the compiler would not derive *is* the vouching — there
is no keyword, the empty body is the whole statement:

```cplus
struct Handle { opaque _h: *u8 }
impl Handle: Send {}                     // "I assert this is safe to move"
impl Arc[T: Send + Sync]: Send {}        // conditional — the bounds ARE the condition
```

### 5.5 Shaping a module

The module is the namespace — there is no second grouping construct, so the file
boundary is the design. Four habits that keep C+ modules readable:

- **Impl order is type-first.** Declare the `struct`, then its `impl`, then any
  `impl Type: Interface` blocks. There are no macros reordering anything, so the
  file reads top to bottom as the type's whole story.
- **The module's free functions are its public surface**, and the type behind
  them may stay private. A store spelled as a `static` plus `load()` / `add()` /
  `watch()` free functions gives every caller one vocabulary; a caller reaching
  the static directly is a caller that will spell it differently next time.
- **`_` is the boundary you actually enforce.** A leading underscore on items,
  fields and methods makes them module-private (E0403 across modules). Use it on
  everything that is not the surface, from the start — widening later is free,
  narrowing later is not.
- **Tests live beside the code** as `#[test]` fns, run by `cpc test`. A doctest
  in a `///` comment needs a fence of exactly three backticks (§16).

---

## 6. Errors are values, and they have NO combinators

No `try`, no `catch`, no `throw`, no `?`. A fallible function returns a tagged
union and the caller matches it.

> **`Result[T, E]` and `Option[T]` provide their variants and nothing else.**
> There is no `.unwrap()`, `.expect()`, `.map()`, `.and_then()`, `.unwrap_or()`,
> `.ok_or()`, `.is_ok()`, `.is_some()`. Handle them with `match`, `if let` or
> `guard let` — those are the only three. (`.unwrap()` on `Box[T]` is a
> different thing: it moves the owned value back off the heap.)
>
> **There is no `panic()` and no `abort()`.** The only hard bail is `assert`,
> which traps. A function that cannot continue returns an error variant; that is
> the whole error model, and a caller who must stop calls `assert`.

Writing any of the missing methods is the most common way generated C+ fails to
compile. If you catch yourself reaching for one, the shape you want is:

```cplus
// instead of  let v = parse(s).unwrap_or(0);
let v: i32 = match parse(s) {
    ParseResult::Ok(n) => n,
    _other             => 0,
};

// instead of  let v = parse(s)?;
guard let ParseResult::Ok(v) = parse(s) else { return ReadResult::Err(1); };
```

Generic `Result` and `Option` live in stdlib (`stdlib/result`, `stdlib/option`),
with `result::ok` / `result::err` / `option::some` helpers and a fixed
`result::IoError`. For a domain error, prefer your own enum — it names the cases
and the exhaustiveness check then works for you.

**There is no error context, chaining, or boxing** — no `anyhow` analogue. If a
caller needs context, it goes in your enum's payload. Decide that when you
declare the enum, because adding it later touches every construction site.

---

## 7. `str` borrows, `Text` owns

| Type | Shape | Owns |
|---|---|---|
| `str` | `(*u8, usize)` | no — a view |
| `text::Text` | `(*u8, usize, usize)` | yes — heap |

A string literal is always `str`. `Text` is **not** a builtin and never resolves
bare: import `stdlib/text` and spell it `text::Text` (bare `Text` is E0303).

**One read surface: reads live on `str` and return views.** `Text` declares only
what allocates or mutates — `append`, `insert`, `truncate`, `reserve`, `clone`,
`uppercased`, `replacing`, `pad_start`… plus `capacity`/`view`/`equals`. Every
read — `count`, `trim`, `slice`, `split`, `find` — lives in the blessed
`impl str` block, and a `Text` receiver reaches it through the coercion. So
`t.trim()` returns a **`str` view into `t`'s own buffer**, with no copy.

```cplus
import "stdlib/str" as _;          // `as _` — enables the methods, binds no name
                                   // (importing stdlib/text brings it in transitively)
s.count();                         // NOT len() — there is no len()
s.is_empty(); s.char_count(); s.is_ascii();
s.has_prefix("ab"); s.contains("x"); s.find("x");        // find -> Option[usize]
s.slice(from: 1, to: 4);           // Option[str] — a view, no copy
s.trim(); s.trim_start();          // views — the endpoints move, nothing is copied
s.split(separator: ",");           // Vec[str] of views (the Vec allocates; the parts don't)
s.to_i64(); s.to_f64();            // Option — strict decimal shapes
```

`cpc query members str` is the full list; do not memorise it from here.

Converting a view to an owned value is `.to_text()`, and **that is the only copy
you ever pay for — and you spell it.** While a view lives its owner is
write-locked (§3.5).

**There is no `+` on strings.** Build with interpolation or `Text::append`:

```cplus
let n: i32 = 42;
let s: text::Text = "answer is ${n}";     // a BINDING allocates an owned Text
io::println("i = ${n}");                  // a SINK writes the parts — ZERO heap
t.append("x = ${x}");                     // appends in place — one reserve, then copies
```

Syntax is `${expr}` — not `\{...}` — and **format specifiers do not exist**
(`${x:04d}` will not parse). **An interpolation cannot contain a string
literal**: the lexer ends the outer literal at the inner quote, so
`"${f("x")}"` is E0001 pointing at a `"` for no stated reason. Hoist the call
into a local and interpolate that. An interpolated literal passed *directly* to
`io::print`/`println`/`eprintln` never materialises a `Text`; since those sinks
are `#[no_alloc]`, real-time code may log this way. Any other position builds an
owned `Text`.

`str` is forbidden in `async fn` signatures (E0900) — pass `Text`.

---

## 8. The syntax that will actually surprise you

Everything else you can get wrong and `cpc check` will correct in one line.
These are the ones that either compile-and-mislead, or fail with a message that
does not explain itself.

**Integer literals wrap through `i32` before `as`.** Build big masks
arithmetically, not by writing a wide literal:

```cplus
const MASK: u64 = (1u64 << 40) - 1u64;    // ✓ const expressions fold at compile time
```

**Arithmetic traps on overflow in debug and wraps in release.** `+% -% *%`
always wrap and are what you write when wrapping is the intent — they are
**integer-only**, so a float expression uses plain `+ - *` (on floats the
wrapping form is E0302, and that error will mask the E0333 underneath it).
Division by zero always traps. `as` is the only width-change tool; `as?` is the checked narrowing
that yields `Option[T]`. Pointer ↔ int goes through `usize` (E0315 otherwise).

**Arrays are not iterable.** `for v in arr` is E0312 — `for…in` wants a range or
an `Iterator[T]`. Index instead:

```cplus
for i in 0..3 { let v: i32 = a[i]; }
```

**`for x in it` OWNS each element**, and the binding drops at the end of every
trip unless the body moves it out. A `gen fn` is lazy: calling it runs nothing,
each trip resumes it for exactly one element, and `yield x` MOVES `x`.

**`take`, `guard` and `gen` are reserved keywords, including as local names.**
The parse error names the token without saying why (`expected ';' or '}'`), so a
variable called `take` costs ten minutes if you do not know this. The iterator
adapter is `prefix(n)`, not `take(n)`, for the same reason.

**There is no array→slice coercion** — go through `Vec::as_slice`.

**Bindings, all four cells:**

```cplus
let x: i32 = 5;              // frozen local — no rebind, no field write
var z: i32 = 0; z = 7;       // mutable local
const PI: f32 = 3.14159f32;  // module-scope value, inlined, has no address
static COUNTER: i32 = 0;     // module-scope mutable + addressable (the FFI boundary)
```

`const` initialisers fold at compile time and may reference other consts in any
order; overflow there is E0921. `static` also takes array literals, fills, and
non-generic struct literals. Access to a `static` is bare — the keyword at the
declaration is the whole marker. **Cross-thread safety of a shared `static` is
yours**, not the compiler's.

---

## 9. FFI — calling C, and being called

**There is no `unsafe` block, because every operation that can cause UB is
already syntactically visible.** A deref or index is `*p` / `p[i]` (the only
meaning `*` has), making a pointer is `x as *T`, pointer→int is the loud
`#addr(p)`, and a foreign call cannot appear without a preceding `extern fn`.
The declaration is the marker; the call site stays bare.

```cplus
extern fn malloc(n: usize) -> *u8;
extern fn free(p: *u8);
extern fn fcntl(fd: i32, cmd: i32, ...) -> i32;   // VARARGS MUST BE DECLARED

let p: *u8 = malloc(64);
p[0] = 65;
let q: *u8 = p + 1;              // arithmetic strides by sizeof(T)
if p.is_null() { return; }       // blessed on raw AND fn pointers; one icmp, no load
free(p);
```

**Declaring a variadic C function without `...` silently passes garbage** on
AArch64-darwin: named arguments go in registers and varargs on the stack, so a
fixed-arity declaration compiles, links, and is wrong at runtime.

Layout control for binding real headers: `#[repr(C)]` for stable layout,
`#[repr(C, packed)]` / `packed = N` for no padding / capped alignment,
`#[bits(N)]` for C bitfields (C's rules exactly, including sign extension and
storage-unit straddling), and `#[repr(C)] union` for one storage with several
typed views. Neither a bitfield nor an under-aligned packed field **has an
address** — no `ref` parameter, no `#addr_of` (E0927 / E0926) — so copy into a
local first. For an either/or *value* in ordinary code use an enum with
payloads; a union has no tag and so no destructor can be run correctly.

`#[link_name = "real_symbol"]` aliases a symbol, which is how one C entry point
gets several typed C+ declarations (`objc_msgSend` is the standing example).

**Calling INTO C+ is `export fn`, and it rejects `str`** (E0410): `str` is a fat
pointer with no C-ABI counterpart, so an exported entry takes `*u8` + `usize` and
rebuilds the view inside.

```cplus
export fn probe_emit(name_ptr: *u8, name_len: usize) {
    let name: str = #str_from_raw_parts(name_ptr, name_len);
    events::emit(name);
}
```

Everything not marked `export` is off limits from outside, **and the failure is
silent**: internal functions are `fastcc` with module-scoped mangled names, so an
lldb `call` or a hand-written C declaration reaches the symbol, passes garbage,
and nothing reports it. If a harness needs to drive an internal path, write the
two-line `export` wrapper — that is what it is for.

Never write a mangled name in source (E0405). Never spell a `__cplus_`-prefixed
symbol without `#[runtime_abi]` (E0919).

---

## 10. Threads, async, cancellation

**Partition and join is the idiomatic path.** No shared memory is no race:

```cplus
let h1 = thread::spawn_with::[Range, i64](left,  sum_r);
let h2 = thread::spawn_with::[Range, i64](right, sum_r);
let total: i64 = h1.join() +% h2.join();
```

`spawn`/`spawn_with` MOVE their data in. A **scope** lends a local instead and
guarantees the join before the loan ends — three mistakes are compile errors
rather than races: the lent value dying before the scope (E0514), writing it
while a worker holds it (E0381), and lending the same place twice (E0381).

```cplus
{
    var s: thread::Scope = thread::scope();
    s.lend::[Counts](counts, tally);      // tally: fn(ref Counts)
}                                          // Scope::drop joins every worker
use(counts.hits);                          // safe — they are done
```

**async is kept and is not going away.** `main` and `#[test]` fns may be `async`;
the compiler splits each into an async body and a synchronous drive-loop entry.
`await` is only ever a suspend and outside an `async fn` is E0901.

```cplus
async fn inner() -> i32 { await time::sleep(10); return 41; }
async fn main() -> i32 { return (await inner()) +% 1; }
```

Driving a future from synchronous code is a method on the value — no executor
import, no turbofish: `f.wait()` blocks this thread until the value is out,
`future::wait_or_cancel(f)` is the cancellable form, `f.cancel()` gives up. All
three consume the future.

**Cancellation is a request, never a kill, and it cannot skip a drop.**

- `h.cancel()` is idempotent and non-consuming; the worker observes it, runs its
  drops and `defer`s, and returns normally. `h.join()` still waits and still
  returns its value.
- In a compute loop, `thread::cancelled()` is the ambient check — a bare atomic
  load, safe anywhere. No token threads through signatures.
- Blocking stdlib calls surface it as a value instead of hanging:
  `ReceiveResult::Cancelled`, `IoError::Cancelled`, `Status::Cancelled`.
- **A `Future` has a destructor: dropping one cancels it.** Wherever the value
  goes out of scope the frame is destroyed and every suspend point's cancel edge
  runs. `f.cancel()` is the same thing said as a verb.
- Wrapping your own blocking FFI call: `thread::park_begin()` (true = already
  cancelled, do not park), the syscall, `thread::park_end()`, retrying on EINTR.
- A `thread::Scope` is a **cancellation boundary** — cancelling its owner does
  not reach the workers it lent data to, because the borrow's soundness rests on
  that join.
- Cancellation does not cross a process boundary; a `Process` or PTY child still
  stops via `interrupt`/`terminate`.

A struct or enum that hides a raw pointer is `!Send` and `!Sync` (E0502 at the
bound). Vouch for one with a marker impl when you know better (§5.4).

---

## 11. Do not rewrite what is already written

Before implementing anything below the level of your actual task, check here.
`cpc query symbols` in the package confirms the surface.

**stdlib** — `io` · `option` / `result` · `vec` · `hash_map` · `hash_set` ·
`string_map` (owns its `Text` keys — `HashMap` needs Copy keys) · `string_set` ·
`slice` · `flags` (option-set over u64 bits) · `text` · `str` · `cow` · `fs` ·
`net` (TCP, numeric IPv4) · `env` · `process` (spawn/capture/signal) · `pty` ·
`thread` · `atomic` · `mutex` · `channel` (typed MPMC) · `box` / `arc` / `rc`
(+ `Weak` via `downgrade`) · `future` / `executor` / `reactor` / `time` ·
`iterator` (`gen fn` + `filter`/`prefix`/`map`) · `date` (ISO-8601 parse and
format) · `base64` · `crypto` (sha256/512, hmac, random_bytes, constant-time
`equals`) · `uuid` · `bundle` (files beside the binary) · `platform` (runtime
target facts as matchable enums) · `range` · `marker`.

Three of those carry a trap worth stating. **`Vec::iter` yields Copy elements
only** — to move owned ones out use `Vec::drain`, which drains in order, lazily.
**`HashMap` needs Copy keys**, so a `Text`-keyed map is `string_map`, which owns
its keys. And **`Vec` has no `clone`**: combined with E0509 (§3.4) that means a
`Vec` field of a Drop type can be neither moved out nor copied wholesale, so
installing one into another field is an element-by-element copy you write
yourself.

**Smart pointers**, mapped from C++: `unique_ptr` → `box::Box[T]`, `shared_ptr` →
`arc::Arc[T]` (or `rc::Rc[T]` single-threaded), `weak_ptr` → `Weak[T]`. There is
**no interior-mutability escape hatch**: shared mutation goes through
`with_mut(f)` — which succeeds only when this is the sole strong handle — or a
`mutex::Mutex[T]`, which is internally refcounted and needs no wrapper.

**vendor packages** — `facet` (retained UI framework; `facet_appkit` /
`facet_uikit` / `facet_gtk` / `facet_android` backends, `facet_runtime`,
`facet_agent`) · `appkit` (Cocoa bindings) · `flex_layout` · `events` ·
`accelerate` (BLAS + vDSP) · `metal` + `metal/mps` · `simd` · `json` · `log` ·
`arena` / `static-arena` · `terminal` · `securestore` · `location` · `sensors` ·
`camera` · `notifications` · `applinks`.

**YOUR MANIFEST MUST NAME TRANSITIVE DEPENDENCIES TOO.** `cpc` does not read a
dependency's own `[dependencies]` when resolving imports — it validates every
import in the build against **one flat set taken from the consuming manifest**.
So depending on a package that itself imports `objc/runtime` fails unless you
also write `objc = "*"`:

    E0852: import `objc/runtime`: first segment `objc` is not a declared dependency

...and it points at a file under the dependency's `lib/include/`, which its
author never wrote. That is what makes it read like a package bug. **The fix is
always a manifest line in the consumer, never an edit to the generated header.**
`[link]` tables do travel automatically; imports do not. It is also why
`cpc init --kind gui` writes the backend's whole closure rather than one name.

**A dependency may ship its own SKILL.md**, and `cpc skill` inside a project
prints the language reference *plus* every dependency's. facet's is several
hundred lines about its retained, non-reactive model — the mistakes it describes
all compile. Read it before writing a screen.

### READ A PACKAGE'S DOCS BEFORE ITS SOURCE

Every `vendor/*` package is meant to be usable without reading its code, and the
docs are split by role so you always know which file to open. Go in this order
and stop as soon as you have the answer:

| You want | Open |
|---|---|
| the rules an agent must not break | `SKILL.md` (or just run `cpc skill`) |
| what it is, in one screen | `README.md` |
| to use it in minutes | `docs/tutorial.md` |
| how it works, why it is shaped that way, the gotchas | `docs/guide.md` |
| an exact signature | `docs/ref.md`, or `cpc query def`/`members` |
| **the answer to none of the above** | the source |

Source is the LAST stop, not the first. The `docs/guide.md` files carry the
reasoning — which lifetimes matter, which call order is load-bearing, what was
tried and rejected — and none of that is recoverable from signatures. A package
with no `docs/` yet still has its `src/*.cplus` header comments, which are
written for the same purpose; read those before the bodies.

And when you do end up in the source: **what you find there is a hypothesis, not
a verdict.** Code that *could* explain a symptom has not been shown to have
caused it. Measure before you conclude.

---

## 12. Building an app: components, services, and how they meet

Everything above is the language. This is the shape a C+ **application** takes —
the part no diagnostic teaches you, because every mistake in it compiles. The
framework is `facet`; `cpc skill` in a facet project appends its full reference
after this file, but the rules below are the ones you cannot get wrong.

### 12.1 A component is a struct, and EVERYTHING lives on it

State is fields. Handlers and node helpers are instance methods. **A screen has
zero top-level fns** — the only ones are the `boxed()` factory and, in the entry
module, `run()`.

```cplus
struct Counter {
    clicks: i64,                    // state is a FIELD — the component is retained,
}                                   // so fields live as long as the tree does

impl Counter {
    fn new() -> Counter { return Counter { clicks: 0 as i64 }; }

    // a node helper: structure `build` would otherwise repeat.
    // `ref this` because it BINDS A HANDLER — a helper that only reads may take `this`.
    fn step(ref this, key: str, title: str) -> core::Node {
        return @ui { button(title, key: key, on_click: this.on_step) };
    }

    // a setter: reach the live tree by key
    fn show_count(this) {
        let n: i64 = this.clicks;
        if let option::Option::Some(l) = label::find("count") {
            let _l: label::Label = l.set_text("${n}");
        }
        return;
    }

    // a handler: a bound method. One handler can serve many keyed controls —
    // ask the sender which one fired.
    fn on_step(ref this, sender: *u8) {
        let key: text::Text = component::key_of(sender);
        if key.view() == "step:up"   { this.clicks = this.clicks + (1 as i64); }
        if key.view() == "step:down" { this.clicks = this.clicks - (1 as i64); }
        this.show_count();
        return;
    }
}
```

**The single most common wrong shape** is a top-level
`fn on_click(sender: *u8, ctx: *u8)` that casts `ctx` back to your type. That is
the manual form of what the compiler already does for `this.method` (§4.2), and
it is the clearest sign of code written from memory.

### 12.2 `build` runs ONCE, and the tree it returns is LIVE

There is no render loop, no diff, no vdom, and nothing re-runs `build`. What it
returns is a live tree, like the DOM. **To change what is on screen you find the
node by key and set the property.** If you catch yourself calling `build` again
to show a change, stop — that is the React shape and it is wrong here.

```cplus
impl Counter: component::Component {
    fn build(ref this) -> core::Node {
        let start: i64 = this.clicks;
        return @ui {
            column {
                label("${start}", key: "count", font_size: 56.0f64)
                hstack {
                    this.step("step:down", "-")
                    this.step("step:up", "+")
                }
                    .gap(8.0f64)
            }
                .grow(1.0f64)
                .padding(20.0f64)
        };
    }
}
```

Five rules about that tree, each of which compiles wrong:

- **Every control needs a `key`.** It is how you find the node again, and it is
  the agent and test surface. Put it on the node the gesture is on, not on the
  content a helper wraps.
- **Never rebuild to toggle.** Mount the node always and show/hide it — hidden
  frees its space AND keeps its state (scroll, selection, typed text). **Showing
  it again needs `relayout`**: out of layout it cached a zero size, and restoring
  the display does not invalidate that, so it comes back visible and 0×0, drawing
  over whatever took its place.
- **Never repeat structure inside `build`.** Say it once as a node helper and
  call it. A long `build` is the tell; so is a helper that grew handlers — that
  one is a component in its own file.
- **`build` must not block.** It runs on the main thread during mount, so slow
  work there is a window that does not appear. Paint placeholders, work off-main,
  patch the tree when it lands.
- **Rows are not yours to build.** A list of anything is a recycled list, table,
  collection or tree with a data source: change the model, set the count, and it
  recycles. Never hand-mount, reorder or swap row nodes.

### 12.3 Lifecycle: both hooks take a REASON, and it is load-bearing

```cplus
impl Counter: component::Lifecycle {
    fn on_attach(ref this, why: component::Attach) {
        if why != component::Attach::Mount { return; }   // ONCE — see below
        this.show_count();
        return;
    }
    fn on_detach(ref this, why: component::Detach) { return; }
}
```

`Attach` is `Mount` (entered the tree — the only moment mounting means anything,
and where views are built) · `Foreground` (visible again after being gone; never
at launch) · `Active` (frontmost, taking input). `Detach` is `Inactive` (lost
FOCUS, possibly still fully visible) · `Background` (no longer frontmost; views
still live) · `Unmount` (leaving the tree) · `Terminate` (closing).

**`on_attach` fires on every un-park, not only at mount**, so one-time setup —
subscribing to a store, filling an outlet — must be guarded on `Mount` or a
re-show is a second subscription and a doubled handler. And **`Inactive` is never
a release signal**: it fires when the user clicks another window, or in split
screen while the app is fully on screen. Release devices on `Background`.

### 12.4 A service is one of TWO tiers, and the question is who else has to hear

- A **job** (`facet/services`) answers the ONE screen that asked.
- A **resource** (`facet/resource`) is a shared store, and every landed write
  BROADCASTS a typed `Change` to everyone watching.

It goes wrong both ways. A job whose result you hand-deliver to a second screen
wanted to be a resource. A resource with one watcher is a change channel paying
for a reader that does not exist.

**The interface IS the threading contract.** For a job it is two methods:

```cplus
impl Search: services::Job {
    fn run(ref this)   { ... }    // OFF main: db, fs, net, a child process
    fn apply(ref this) { ... }    // ON main: install what run prepared
}
let started: bool = services::run_job(this.search, then: this.on_hits);
```

A resource adds `state()` (one line: the address of its embedded
`resource::State`) and a `run` that dispatches on the request kind. Both tiers
give you the whole async pipeline from conformance alone — no per-job struct, no
boxing, no thread code.

Four things that are invisible in the types:

- **`run` writes STAGED fields only**; the plain field beside it is what the main
  thread paints from; `apply` is the one place the two meet. A `run` that assigns
  the live field compiles and usually looks fine — what it costs is a `Text`
  freed under a main thread that is reading it.
- **That install is a COPY, not a move.** The service owns heap fields so it has
  a destructor, and moving an owning field out of a Drop type is E0509 (§3.4) —
  and `Vec` has no `clone`. Write the element copy.
- **A second call QUEUES on a resource and is DROPPED on a job** (`run_job`
  returns `false`). So a job's caller must decide which ask wins and say why. And
  because the resource queue is faithful, the CALLER collapses bursts — a watcher
  firing once per file turns a hundred-file build into a hundred queued walks.
- **The app surface is the MODULE, not the struct.** The store is a `static` and
  the app reaches it through free functions in that same file — `load()`,
  `add(...)`, `watch(f)`, `by_id(id)` — each a line over
  `resource::get/post/put/delete/watch`. Components never name the static and
  never call `resource::` themselves.

### 12.5 Integrating them: the write IS the notification

**One component never updates another. Both watch the resource.**

```cplus
impl Panel: component::Lifecycle {
    fn on_attach(ref this, why: component::Attach) {
        if why != component::Attach::Mount { return; }
        this.sub = notes::watch(this.on_notes_changed);   // WATCH first …
        notes::load();                                     // … THEN ask
        return;
    }
    fn on_detach(ref this, why: component::Detach) { return; }
}

impl Panel {
    // ALL screen updating lives here.
    fn on_notes_changed(ref this, c: resource::Change) {
        match c.verb {
            resource::Verb::Loaded  => { this.reload(); }
            resource::Verb::Created => { this.reload(); }
            resource::Verb::Updated => { this.repaint(c.id); }
            resource::Verb::Deleted => { this.reload(); }
        }
        return;
    }

    // a mutating handler fills the draft, calls the verb, and STOPS.
    fn on_add(ref this, sender: *u8) { notes::add("untitled"); return; }
}
```

The order is the part that bites: **subscribe first, then call the verb.** A
landing with no listener is a screen that stays empty until something else
happens to write.

Five ways this goes wrong, all clean builds:

- **Never keep your own copy.** A snapshot parked in a field is a second truth,
  and it is the one that goes stale. Read the store.
- **If it broadcasts, do not also do the work.** The watch handler runs when the
  write lands; doing the update at the call site too mounts everything twice.
- **No UI code at the call site.** The handler fills the draft, calls the verb,
  stops. Everything else is the watch handler's job.
- **Never call the backing (sqlite/fs/net) from a handler** — that blocks the
  main thread. Backing code lives in `run`. A failed write broadcasts nothing;
  handle failure in `then`.
- **When one verb carries several writes, ask WHICH one landed, and ask it
  positively.** Three writes sharing `put` all broadcast `Updated`; asking "not
  a backup" makes every write added later raise a banner meant for exactly one
  of them.

`events::emit` (the bus) stays for UI-only facts that touch no store. When a bus
fact does feed a store, the component that owns the store translates it into a
verb and stops.

### 12.6 How you know it works

- **`click` proves an action FIRES; a pointer proves it can be REACHED.** The
  agent surface's `click` skips hit testing and the responder chain on purpose,
  so it can drive a control a pointer could never get to — and says nothing about
  whether a hand could. Drive the app to a state and measure that. For "can this
  be pressed", ask a person.
- **Typing is not `set_text`.** They land on different halves of the backend, so
  a screen driven only through the socket does not test what typing does.
- **A write onto the live tree is a claim until you count it.** A call that did
  nothing looks exactly like a value that arrived and was ignored.

Deeper on all of it — the cursor tier per control, recycled-list data sources,
`@ui` layout modifiers, screens and nav — is `vendor/facet/SKILL.md`, which
`cpc skill` prints straight after this file.

---

## 13. Platform variation — three mechanisms, and never `#if`

C+ has no conditional compilation. A per-OS difference goes in exactly one of
three places. The vocabulary is the same in all three: `macos linux windows ios
android esp32 wasm`.

1. **A `_<platform>.cplus` sibling file** shadows `<module>.cplus` for that
   target, and importers always write the base name. **This is the only way to
   vary imports** (kqueue vs epoll, AppKit vs UIKit). The suffix comes from
   `--target`, not the host. **`android` tries `_android` then falls back to
   `_linux`** — without that fallback an Android build silently picks the Darwin
   base and fails at `dlopen`. The base file is optional; a platform with
   neither a variant nor a base is E0401 naming the *base* path. Nothing checks
   that variants declare the same names — only a build per platform does.
2. **`[<platform>.dependencies]`** for a package that exists only there.
   Importing an off-platform package is E0866 naming the platform it was
   declared for; a misspelled platform section is E0406, not a silent no-op.
3. **`#platform()` / `#arch()` / `#target()`** — `str` constants, **value level
   only**. Both arms of an `if` on one compile everywhere, so they can pick a
   padding or a port and never an import. `#arch()` crosses `#platform()` (macos
   and ios are both aarch64); `#target()` is the only axis that separates the iOS
   simulator from a device. The runtime, matchable counterpart is
   `stdlib/platform`.

**OS decides files; form factor decides values.**

### The non-C+ files each platform needs

C+ builds the code; the platform still wants its own bundle metadata, and every
one of these is **convention over configuration — if the file is at the path
below, it is used, with nothing to wire.** `cpc init --platform <p>` writes them;
this is what they are for, so you can add one to a project that skipped it.

**macOS — `macos/Info.plist`.** `cpc build` embeds it into the binary's
`__TEXT,__info_plist` section whenever the file exists, which is how a bare
Mach-O with no bundle carries a plist at all. **This is not cosmetic.** A
permission (camera, microphone, location, photos) needs its usage-description
key here, and without it `permissions::state` keeps answering normally while
`permissions::request` **kills the process** — asynchronously, after the call
has already returned, so the crash names nothing that leads back to it. Leaving
in a key the app never requests is harmless; leaving out one it does request is
fatal at runtime. The string is shown to the person in the dialog, so write it
for them.

`cpc package` wraps the build in a real `.app` and uses the same file,
synthesizing a minimal one if it is absent. It fills `CFBundleExecutable` to
match the binary when the file does not set it — a mismatch there makes the
bundle refuse to launch and no error names it. Packaging is not a mode of
`--release`: a debug bundle is exactly what you want for testing permissions,
which need a bundle to exist.

**iOS — `ios/main.m` and `ios/Info.plist`.** `main.m` is the whole of the
Objective-C in a facet app: it includes the header `cpc` generates next to the
archive — `target/<target>/debug/<name>.h`, e.g.
`target/ios-arm64-simulator/debug/myapp.h`, named by the `--target` and not by
the LLVM triple — and calls `<name>_main()`, which calls
`UIApplicationMain` and never returns. There is **no AppDelegate.m and no
storyboard** — facet_uikit synthesizes both its delegate and its scene delegate
at runtime. So the plist must NAME THE SCENE (every windowing API on iPadOS
hangs off one) and must NOT name a storyboard: UIKit would wait for a nib that
does not exist and the screen stays black.

On a **simulator**, an entitlement must be in the binary's `__TEXT,__entitlements`
section that the LINKER embeds, and the ad-hoc signature must stay plain —
`codesign --entitlements` on a simulator bundle makes SpringBoard refuse the
launch, and every error it prints points somewhere else. On a **device** it is
the other way round: entitlements ride in the signature and are validated
against a provisioning profile.

**Android — `android/AndroidManifest.xml`.** The no-Gradle path, so `aapt2`
consumes it directly and still requires the deprecated `package=` attribute (a
Java package may not begin with a digit). Four things in it are load-bearing:

- `<meta-data cplus.facet.lib>` — the `.so` name, and `<meta-data
  cplus.facet.main>` — the entry SYMBOL, `dlsym`'d by name; it is the
  `export extern fn <name>_main` in `src/main_android.cplus`. Neither name is
  derivable from the other, so whoever packages the APK reads these values.
- `android:configChanges="orientation|screenSize|keyboardHidden"` — without it
  Android destroys and recreates the Activity on every rotation, tearing down
  the mounted facet tree.
- `INTERNET` permission, if the app serves the agent surface. Android gates
  `socket()` on the app's membership of the inet group; without the permission
  the loopback bind fails `EACCES`, the accept loop ends the instant it starts,
  and a forwarded port connects to nothing while the app runs perfectly.
- minSdk/targetSdk are **aapt2 flags at package time**, deliberately not a
  `<uses-sdk>` element — one source of truth rather than two that disagree.

Third-party AARs need no Gradle: pin them in `[android.maven]` as
`"group:artifact" = "version"` (exact versions; `cpc pm add . --maven G:A:V`
writes the line, and it is E0877 on any other platform).

---

## 14. Attributes are metadata — they never generate code

Only compiler-known attributes are accepted (E0354 otherwise; bad shape E0355,
wrong target E0356, illegal duplicate E0357, and none is legal on an
`interface`). The ones worth knowing:

- `#[test]` — register a test fn. `#[repr(C)]`, `#[repr(C, packed[= N])]`,
  `#[bits(N)]`, `#[link_name = "…"]` — the FFI set (§9).
- `#[requires(expr)]` / `#[ensures(expr)]` — pre/postconditions checked at entry
  and at *every* return; `result` names the returned value. Pure expressions over
  params, consts and fields (E0924 otherwise).
- `#[deprecated("use parse_v2")]` — **W0006 at each USE, never at the
  declaration**, and the call still builds. That is what lets a rename land as a
  list consumers work through, and break in a later release.
- `#[watch] struct Model { … }` — a field-write barrier: every store to a field
  calls `on_value(field_name)` after it. A missing hook is E0361; an `on_value`
  *without* `#[watch]` is W0004, a hook that silently never fires.
- `#[keeps(this)]` / `#[keeps(nothing)]` — a declared view-flow summary for a
  body the checker cannot read through. **Trusted, not verified** — the same
  model as `opaque`.
- `#[no_alloc]` / `#[no_block]` / `#[bounded_recursion]` / `#[max_stack(N)]`, and
  `#[realtime]` which bundles the first three. `cpc --realtime-report` is the
  whole-project digest.
- `#[inline]` / `#[inline(always)]` / `#[inline(never)]`, `#[unroll(N)]`,
  `#[vectorize_width(N)]` — optimiser hints.

### Intrinsics — every one is spelled `#name(...)`

These are compiler builtins, so the graph cannot show them to you and there is
no module to import. This is the whole set worth writing:

| Intrinsic | Gives | Note |
|---|---|---|
| `#size_of::[T]()` · `#align_of::[T]()` | `usize` | folded to a constant |
| `#zero::[T]()` | `T` | the all-zero value; how a `static` struct is initialised |
| `#addr_of(place)` | `*T` | the argument must be an addressable place |
| `#addr(p)` | `usize` | pointer → int, deliberately loud |
| `#include_bytes("path")` · `#include_str("path")` | `*[u8; N]` · `str` | path is relative to the source file; `_str` is UTF-8 validated at sema |
| `#env("NAME")` | `str` | resolved at sema; E0876 if unset |
| `#platform()` · `#arch()` · `#target()` | `str` | the active TARGET, value-level only (§13) |
| `#str_ptr(s)` · `#str_len(s)` · `#str_from_raw_parts(p, n)` | — | the FFI tier for `str`, not the way to do string work |
| `#println(x)` | — | no-import debug print |
| `#asm("…")` | — | only inside a `#[naked]` fn (E0909) |

An unknown `#name` is E0905; a non-literal argument to `#include_*` / `#env` is
E0871.

**SIMD** exists as nineteen concrete widths (`f32x4`, `i32x8`, `mask32x4`, …)
with `splat`/`new`/`load`/`from_array` constructors and lane-typed methods;
compares yield a `mask` and `mask.select(a, b)` blends. SIMD does **not** cross
an `extern fn` boundary — round-trip through `[f32; N]` (E0410 otherwise). Full
widths and methods: `spec.md`.

---

## 15. Contextual builder blocks — `@ctx { … }`

A package may declare a builder context; inside `@name { … }` bare names resolve
to that package's constructors, and leading-dot modifiers chain onto the value
they follow. This is how `@ui { column { label("hi") } .gap(8.0f64) }` reads as a
tree. It desugars to ordinary locals and calls **before sema**, so there is no
new codegen and diagnostics land on the line you wrote. It is a package
capability, not syntax you can invent inline.

---

## 16. Tooling

```bash
cpc build                   # the project (reads Cplus.toml) — REQUIRED for anything with imports
cpc check                   # whole-project front end, no codegen — the CI gate
cpc check FILE              # single file, NO imports, does not read the manifest
cpc test                    # #[test] fns + doctests
cpc fmt                     # canonical formatting (no arg = this project)
cpc explain E0337           # a code's cause, fix and worked example — 194 of them
cpc skill                   # this file + every dependency's, version-matched
cpc query def|refs|callers|callees|members|symbols|type-at|scope-at|complete
cpc mcp                     # resident MCP server over the same graph
cpc pm add . <pkg>          # dependencies
cpc --emit-ll[-opt] / --emit-asm      # IR before/after opt, native asm
cpc build --warn-deps       # dependency warnings too (default: this project's src/ only)
cpc --diagnostics=json      # NDJSON for tools
cpc --release               # -O2 (default: debug -O0 with overflow traps)
```

**Navigate by the graph, not by grep.** C+ has no dynamic dispatch, so every call
to a named function resolves and the graph's answer is *complete* — which a text
search's never is. `cpc query` rebuilds the whole graph per invocation and throws
it away; `cpc mcp` builds once and answers in microseconds, so use the server for
anything past a single lookup.

**`cpc fmt` is a syntax check you get for free**: if source does not round-trip,
something is off.

**A doctest fence opens only on a line that is exactly three backticks.** A
```` ```cplus ```` fence in a `///` comment is *not* extracted and its example
silently never runs.

---

## 17. When in doubt

1. **Build it.** `cpc check` cannot catch invalid IR; only a real build can.
2. **Read the diagnostic, then `cpc explain` its code.** The compiler is the
   source of truth and this file is a summary of it — where they disagree, the
   compiler is right and this file is stale.
3. **Ask the graph** (`cpc query`, `cpc mcp`) rather than grepping.
4. **Check §2** before proposing anything that looks like a language feature.
5. **Read the dependency's own SKILL.md** (`cpc skill`) before writing against
   it — especially facet, where the wrong model compiles cleanly.

Do not guess. Every question this file leaves open, the toolchain answers in a
second, offline.
