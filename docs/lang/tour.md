# A tour of C+

One sitting, one program at the end. Judgment calls and gotchas live in
[guide.md](guide.md); exact shapes in [ref.md](ref.md); the normative rules in
[spec.md](spec.md). Every snippet here compiles against the current compiler.

C+ is a systems language: values and pointers with C's runtime shape, an
ownership model that makes memory safety a compile-time property, an LLVM
backend, and a one-way C ABI. There is no garbage collector, no runtime, no
hidden allocation.

## 1. A project

```bash
cpc init hello && cd hello
cpc build && ./target/debug/hello
```

`cpc init` writes three files. The manifest needs no target section —
`src/main.cplus` is the default entry:

```toml
[package]
name    = "hello"
version = "0.0.1"
edition = "2026"

[dependencies]
stdlib = "*"
```

Dependencies resolve under `vendor/<name>/`. In this repository, symlink it:
`ln -s "$CPLUS"/vendor vendor`.

## 2. Hello

```cplus
import "stdlib/io" as io;
import "stdlib/text" as text;

fn main() -> i32 {
    let name: str = "world";
    io::println("hello, ${name}");
    return 0;
}
```

Three things worth noticing on day one:

- Imports are quoted paths with a mandatory alias. `"stdlib/io"` is a module
  in a dependency; `"./helpers"` would be a file next to this one.
- Interpolation is `${expr}`, and it requires `stdlib/text` in the build
  (E0613 tells you so). Printed directly through `io::println` it writes the
  parts straight to the stream — no allocation.
- `return` is explicit, always, at function level (E0333 otherwise).

## 3. Values and bindings

Two keywords for locals, two for module scope. There is no `mut`:

```cplus
let x: i32 = 5;                 // immutable local: no rebind, no field writes
var z: i32 = 0; z = 7;          // mutable local

const PI: f32 = 3.14159f32;     // module-scope immutable VALUE (inlined, no address)
static COUNTER: i32 = 0;        // module-scope mutable global (addressable, C-facing)
```

|  | immutable | mutable |
|---|---|---|
| **local** | `let` | `var` |
| **module** | `const` | `static` |

`let` freezes the whole value — a struct behind `let` rejects field writes.

Numbers never convert silently. Every width change is spelled `as`:

```cplus
let n: i64 = 40;
let small: i32 = n as i32;         // truncating, on purpose
let a: i32 = 2_000_000_000;
let sum: i32 = a +% a;             // +% -% *% wrap; plain + traps on overflow in debug
```

## 4. Structs and methods

A struct is a value type. Methods live in `impl`; the receiver is always
`this`, and a prefix states its relation to the caller's value:

```cplus
import "stdlib/text" as text;

struct Item {
    name: text::Text,
    price: i64,
    stock: i64,
}

impl Item {
    fn new(name: str, price: i64) -> Item {
        return Item { name: name.to_text(), price: price, stock: 0 };
    }
    fn worth(this) -> i64 {                  // reads: borrows the receiver
        return this.price *% this.stock;
    }
    fn restock(ref this, count: i64) {       // mutates: writes back to the caller's var
        this.stock = this.stock +% count;
    }
}
```

Struct literals name every field (`Item { name: n, ... }` — no shorthand).
`::` reaches types and associated functions, `.` reaches instances. A
mutating method needs a `var` receiver — calling `restock` on a `let` is
E0328.

## 5. Enums and match

Enums carry payloads; `match` is exhaustive or it does not compile:

```cplus
enum Read { Ok(i64), Empty, Garbled }

fn parse(s: str) -> Read {
    if s.is_empty() { return Read::Empty; }
    match s.to_i64() {
        option::Option[i64]::Some(v) => { return Read::Ok(v); }
        option::Option[i64]::None    => { return Read::Garbled; }
    }
}
```

When only one arm matters, `guard let` binds it or diverges — this is the
dominant idiom for error handling:

```cplus
fn demo() -> i64 {
    guard let Read::Ok(v) = parse("42") else { return (0 -% 1) as i64; };
    return v;
}
```

There are no exceptions, no `try`, no `?`, and `Option`/`Result` have no
`.unwrap()` — a fallible result is a value you `match` (or `guard let`) like
any other. [error-handling.md](error-handling.md) covers designing with this.

## 6. Ownership in ten lines

No `&T` types. How a parameter relates to the caller's value is a prefix on
the parameter — and the default is a read-only borrow:

```cplus
fn read_only(s: text::Text) -> usize { return s.count(); }   // bare: borrow
fn bump(ref n: i32) { n = n +% 1; }                          // ref: write-back
fn sink(take t: text::Text) -> usize { return t.count(); }   // take: consume

var k: i32 = 0;
bump(k);                            // no `&` at the call site — the signature decides
let t: text::Text = "hello".to_text();
let n: usize = read_only(t);        // t still yours
let m: usize = sink(t);             // t consumed; reading t now is E0335
```

That is most of the model. The rest — why a borrowed value can't escape
(E0337), how string views borrow their owner (E0513), what drops when — is
[ownership.md](ownership.md), and it is worth reading before your first real
program.

Cleanup is automatic and deterministic: when an owning value (a `Text`, a
`Vec`, a struct containing them) goes out of scope, it frees itself, fields
in reverse order. `defer EXPR;` runs at scope exit for everything else.

## 7. Collections and strings

```cplus
import "stdlib/vec" as vec;
import "stdlib/status" as status;

var v: vec::Vec[i64] = vec::new::[i64]();
let _s: status::Status = v.append(41);        // mutators report Status — bind it
match v.at(0) {                               // reads return Option
    option::Option[i64]::Some(x) => { io::println("${x}"); }
    option::Option[i64]::None    => {}
}
```

Generics use `[T]`, and the explicit form at a call is `name::[T](...)`.
Fixed-size arrays are `[i32; 4]`, bounds-checked, and iterated by index
(`for i in 0..4` — arrays are not `for ... in` iterable).

Two string types, one rule:

| Type | What | Owns? |
|---|---|---|
| `str` | a 16-byte view (`ptr + len`) | no — borrowed |
| `text::Text` | heap-owned, growable | yes |

Literals are `str`. Reads (`count`, `trim`, `slice`, `split`, `find`, …)
live on `str`, and a `Text` reaches them by coercion. Mutation and
allocation (`append`, `to_text()`, interpolation into a binding) live on
`Text`. `count()`, never `len()`. There is no `+` concatenation — interpolate
or `append`.

## 8. A whole program

Everything above, in one file that compiles and runs (`apples cost 3`):

```cplus
import "stdlib/io" as io;
import "stdlib/text" as text;
import "stdlib/option" as option;
import "stdlib/vec" as vec;
import "stdlib/status" as status;

struct Item {
    name: text::Text,
    price: i64,
    stock: i64,
}

impl Item {
    fn new(name: str, price: i64) -> Item {
        return Item { name: name.to_text(), price: price, stock: 0 };
    }
    fn restock(ref this, count: i64) {
        this.stock = this.stock +% count;
    }
}

enum Lookup { Found(i64), Missing }

fn price_of(items: vec::Vec[Item], name: str) -> Lookup {
    for i in 0..(items.count() as i32) {
        match items.at_ptr(i as usize) {
            option::Option[*Item]::Some(p) => {
                if { (*p).name } == name { return Lookup::Found({ (*p).price }); }
            }
            option::Option[*Item]::None => {}
        }
    }
    return Lookup::Missing;
}

fn main() -> i32 {
    var inventory: vec::Vec[Item] = vec::new::[Item]();

    var apples: Item = Item::new("apples", 3);
    apples.restock(10);
    let _s1: status::Status = inventory.append(apples);
    let _s2: status::Status = inventory.append(Item::new("pears", 5));

    guard let Lookup::Found(p) = price_of(inventory, "apples") else {
        io::eprintln("no apples today");
        return 1;
    };
    io::println("apples cost ${p}");
    return 0;
}
```

Two details this program exercises deliberately: `Item` owns a `Text`, so
`append` takes it by `take` — after `inventory.append(apples)` the local is
gone; and `at_ptr` hands back a raw `*Item` view, dereferenced as `(*p)` —
wrapped in braces inside the `if` condition, which is the house style for
parenthesized deref reads.

## 9. Tests

Tests are functions in your source, marked and discovered:

```cplus
#[test]
fn restock_adds() {
    var it: Item = Item::new("x", 1);
    it.restock(3);
    assert it.stock == 3;
}
```

`cpc test` builds and runs every `#[test]` in the project. `assert` traps on
false — it is also the only hard stop the language has.

## 10. Day-one rules

The complete list of things that stop a first program from compiling:

1. `return` is explicit, everywhere (E0333).
2. Every numeric width change is `as`; pointer ↔ integer goes through
   `usize` (E0302 / E0315).
3. `${...}` interpolation needs `stdlib/text` imported (E0613).
4. Struct literals name fields: `Point { x: x, y: y }` (E0100).
5. No `null` — `Option[T]`; FFI null is `0 as *T` (E0300).
6. No closures — named `fn`s; stateful callbacks are a `(fn_ptr, ctx: *u8)`
   pair ([guide.md](guide.md)).
7. Arrays aren't `for ... in` iterable — index `0..n` (E0312).
8. Constructing a generic enum spells its args
   (`Option[i64]::Some(v)`); a `use`-site pattern can drop them only for
   same-module enums.
9. `count()`, not `len()`; `append`, not `push`.
10. A mutating method or `ref` argument needs a `var` place (E0328).

## Where next

- [guide.md](guide.md) — which construct to reach for, and the traps.
- [ownership.md](ownership.md) — the full ownership model; read it early.
- [ref.md](ref.md) — every construct, one entry each.
- `cpc explain E0xxx` — any diagnostic, explained offline.
- `docs/examples/recipes/` — task-shaped programs that all compile and run.
