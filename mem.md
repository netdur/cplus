# C+ binding & storage keywords: `const`, `static`, `let`, `var`, `ref`

## Model at a glance

Mutability and storage form a 2×2, plus `ref` for cross-call mutation. There is no `mut` keyword.

| | immutable | mutable |
|---|---|---|
| **global / module** | `const` | `static` |
| **local** | `let` | `var` |

`ref` is not a binding tier; it is a parameter/receiver modifier that lets a callee write back to the caller's value.

Mental anchor: a C+ struct is a value type (a C struct, C ABI). The binding *is* the value, so an immutable binding is an immutable value: it freezes the fields too, not just reassignment. This matches Swift `struct` `let` and C++ `const`, and is the reason `let p; p.x = 1` is rejected.

---

## `const` — module-scope immutable value

A compile-time constant value. Inlined at use sites; it has no guaranteed address.

```rust
const MAX_CONN: i32 = 64;
const GREETING: str = "hi";
const ORIGIN: Point = Point { x: 0, y: 0 };
```

Rules:
- Module scope only. There is no `const` local; an immutable local is `let`.
- Immutable: no rebind, no field write.
- A value, not a place. You cannot take its address (`#addr_of(MAX_CONN)` is invalid). When you need an addressable immutable global, use a `static` you never write.

Use it for fixed configuration, numeric limits, and string-literal constants.

---

## `static` — the mutable, addressable global (foreign boundary)

A global with program lifetime and an address. This is the C-facing mechanism: C+ proper avoids globals, and `static` is where the foreign boundary lives.

```rust
static counter: i32 = 0;             // mutable global
static _LISTENER_CLASS: usize = 0;   // module-private (leading _)
```

Rules:
- Module scope only.
- Mutable: `counter = counter +% 1` is allowed. The `static` keyword is itself the marker that this is global, foreign-facing state.
- Addressable: its address is stable, which is why it backs associated-object keys and run-once caches at FFI boundaries.
- Visibility follows the uniform `_` rule: `static _x` is module-private, `static x` is public to importers. Contain a global by making it private and exposing accessor functions.
- Concurrency is the developer's responsibility: a `static` read and written from multiple threads is a data race. There is no automatic synchronization.

Typical uses are all at the C/ObjC/JNI boundary: bound C globals, associated-object keys, "register this once" flags. Native C+ logic should not reach for a `static`; pass state explicitly instead.

---

## `let` — immutable local

A frozen local binding. Because structs are value types, `let` freezes the whole value.

```rust
let p = Point { x: 0, y: 0 };
let name = Text::from("Ada");
```

Rejected on a `let`:
- rebinding: `p = Point { x: 1, y: 1 };`
- field write: `p.x = 1;`
- a mutating method (one declared `ref this`): `p.bump();`
- passing it to a `ref` parameter (see `ref` below)

A `let` may still be read freely, and may be consumed (its ownership given away with `take`), since consuming ends the binding rather than mutating it.

`let` is the default for locals: most bindings are computed once and read, never reassigned.

---

## `var` — mutable local

A mutable local binding: rebind, field write, and mutating methods are all allowed.

```rust
var i = 0;
i = i +% 1;                 // rebind

var p = Point { x: 0, y: 0 };
p.x = 5;                    // field write
p.bump();                   // mutating method (ref this)
```

Rules:
- Local scope only.
- Rebinding stays within the declared type: `p = Point { .. }` is fine, `p = OtherType { .. }` is not.

Use `var` for counters, accumulators, loop state, and "running best" values, and for any local you will hand to a `ref` parameter.

---

## `ref` — by-reference write-back

A parameter or receiver modifier. The callee writes back to the caller's value. It is not a binding; it composes with `let`/`var` at the call site.

```rust
fn bump(ref x: i32) { x = x +% 1; }          // ref parameter
fn grow(ref this) { this.n = this.n +% 1; }  // mutating method (receiver)
```

Call sites:

```rust
var k = 0;
bump(k);        // k is now 1

let j = 0;
bump(j);        // ERROR — `ref` needs a `var` place; `j` is immutable
```

Rules:
- A `ref` argument requires a `var` caller place. A `let` (immutable) cannot be passed to a `ref` parameter. This is a single `is_var` check.
- The check is made at the call from the signature's `ref` against the local's tier. It never inspects the callee body, so it stays modular through function pointers, interfaces, and generics.
- The same rule governs mutating methods: a `ref this` method requires a `var` receiver (`let p; p.bump()` is rejected).
- No call-site marker: a write-back call is `bump(k)`, not `bump(&k)`. The `var` declaration on the binding is the signal that it is mutable.
- `ref x: T` works for any type; it lowers to a C out-parameter (`T*`).

`ref` replaces the by-reference half of the old `mut`/`move`. It corresponds to C#/Swift `inout`, without the call-site keyword.

---

## What is gone: `mut`

`mut` is fully retired. There is no `let mut`, no `static mut`, and no `mut` parameter. Every mutation is expressed by one of: `var` (local), `static` (global), or `ref` (write-back through a call). A single surviving `mut` anywhere would re-invite the Rust reading, so it appears nowhere.

---

## Gotchas by background

These are the spots where a keyword reads one way in another language and means another in C+:

- From JavaScript/TypeScript: `let` is not reassignable here. `let` is frozen; the reassignable local is `var`. (`var` matches JS/TS.)
- From Rust: there is no `let mut`, use `var`. There is no `static mut`, use `static`. There is no `&mut x` at a call: pass a `var` and the function takes `ref`.
- From Kotlin/Swift classes or TS objects: `let p; p.x = 1` is rejected. Those are reference types, where an immutable binding still permits mutating the object. C+ structs are value types, so `let` freezes the fields. Use `var` to mutate fields.
- From C: a global defaults to `static` (mutable, addressable). An immutable compile-time value is `const` (a value, not an addressable global).

---

## One combined example

```rust
struct Counter { n: i32 }

impl Counter {
    fn make() -> Counter { return { n: 0 }; }   // type-inferred literal
    fn value(this) -> i32 { return this.n; }    // read receiver
    fn step(ref this) { this.n = this.n +% 1; } // mutating receiver
}

const START: i32 = 0;          // module immutable value

fn run() -> i32 {
    let base = START;          // immutable local
    var c = Counter::make();   // mutable local (we will mutate it)

    c.step();                  // OK: c is `var`, step takes `ref this`
    var total = base;          // mutable local
    total = total +% c.value();

    // let frozen = Counter::make();
    // frozen.step();          // ERROR: `ref this` needs a `var` receiver

    return total;
}
```

---

## Decision guide

- Immutable value, module scope: `const`
- Mutable global at the C/foreign boundary: `static`
- Immutable local: `let`
- Mutable local: `var`
- Let a function mutate the caller's value: `ref` parameter, and the caller's binding must be `var`
