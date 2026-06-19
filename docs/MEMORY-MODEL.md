# The C+ Memory Model

This document explains how ownership, borrowing, and moves work in C+. The
runtime mechanics are close to C — values and pointers, no copy constructor, no
value-site references — and the static rules are what keep that safe. The
parameter and receiver keywords (`ref`, `take`) name those mechanics directly.

## 1. Principles

1. Every value has exactly one owner. When the owner's scope exits, the value's
   destructor runs (if any).
2. Memory safety is established statically. There is no garbage collector and no
   runtime ownership tracking.
3. A non-`Copy` value can only be *moved*, never duplicated. C+ has no copy
   constructor: passing a non-`Copy` value with `take` transfers ownership; it
   does not make a second independent value.
4. There are no value-site references. `&` and `&mut` parse but are rejected
   (E0312). Borrowing is expressed by parameter mode (`ref` / bare) and by the
   receiver forms, which lower to pointers internally.

## 2. Copy and non-Copy

A type is `Copy` when all of its components are `Copy` and it has no `Drop` impl.
Primitives (`i32`, `bool`, ...), raw pointers, `str`, slices, and aggregates of
those are `Copy`. A type with an owning field (`Text`, `Vec[T]`) or a `Drop`
impl is not `Copy`.

`Copy` values are duplicated on assignment and pass; the source stays valid. A
non-`Copy` value is moved when consumed; the source becomes invalid, and a later
use of it is rejected (E0335).

```cplus
let a: i32 = 5;
let b: i32 = a;     // copy; a still usable
let n: i32 = a;     // fine

let v: Text = "hi".to_text();
let w: Text = v;    // move; v is now invalid
let k: Text = v;    // E0335: use of moved value `v`
```

## 3. Move, and the absence of a copy constructor

Because there is no copy constructor, the only way to get a second value of a
non-`Copy` type is to construct one explicitly (for example `clone`, which
allocates a fresh copy of the contents). A plain bitwise duplication is never
inserted by the compiler for a non-`Copy` type; doing so would alias the same
owned resource and free it twice.

This is the single fact behind most of the rules below: a non-`Copy` value
cannot become two owned values for free.

## 4. Parameter modes

For a non-`Copy` type `T`, the parameter mode selects both the calling
convention (by value or by pointer) and the ownership effect (move or borrow).
The default — a bare `x: T` — is a **read-only borrow**, not a move.

| parameter      | passed     | ownership            | source after the call | callee may |
|----------------|------------|----------------------|-----------------------|------------|
| `x: T`         | by pointer | shared borrow        | still owned by caller | read through the pointer |
| `ref x: T`     | by pointer | exclusive borrow     | still owned by caller | read and write through the pointer |
| `take x: T`    | by value   | move (consumed)      | invalid               | own it, return it, store it |

For a `Copy` type, every mode passes by value; the markers only affect whether
the parameter is locally reassignable and (for `ref`) whether writes reach the
caller. A bare `Copy` parameter is an ordinary by-value copy.

```cplus
fn read(b: B)      -> i32 { return b.x; }                // shared borrow: read only
fn bump(ref b: B)         { b.x = b.x + 1; return; }     // exclusive borrow: writes reach the caller
fn into(take b: B) -> i32 { return b.x; }                // move: caller's b is consumed
```

```cplus
fn run() {
    var v: B = mkB();
    let n: i32 = read(v);   // shared borrow: v still owned
    bump(v);                // exclusive borrow: v.x changed in place, v still owned
    let w: B = into(v);     // move: v consumed
    let m: i32 = v.x;       // E0335: v was moved into `into`
}
```

### Exclusive vs shared

A bare `x: T` and a `ref x: T` both pass a pointer to the caller's value; the
caller keeps ownership and runs the destructor. They differ in what is allowed
while the borrow is live:

- bare (shared): read only. Other shared reads of the same value are permitted.
  Many readers, no writer.
- `ref` (exclusive): the callee may mutate, and the write reaches the caller —
  whose binding must therefore be a `var` (E0328 otherwise). While the borrow is
  live, no other access to the source is permitted, not even a read. One writer,
  no one else.

The invariant is: at any point a value has either one exclusive borrow or any
number of shared borrows, never both.

## 5. Borrows are not value-site references

There is no `&x` expression. A borrow exists only as a parameter mode (above) or
through the receiver forms:

- `this` reads the receiver (a shared borrow).
- `ref this` mutates the receiver in place (an exclusive borrow; the receiver's
  binding must be a `var`).
- `take this` consumes the receiver.

A method that needs to read takes `this`, one that mutates takes `ref this`, and
a finalizer that consumes takes `take this`. By-value (`take`) parameters cover
the rest.

## 6. A borrow cannot escape

A bare borrow is valid only for the duration of the call. It cannot be returned,
stored in a field, or re-passed to a `take` parameter — doing so would outlive
the storage it points into (E0337). To hand a value back out, choose one of:

- **Return an owned value.** Take the input by `take` and return it (a move), or
  construct and return a fresh `Text` / `Vec[T]` / struct.
- **Return a blessed view.** `str` and slices are `{ptr,len}` views that may be
  returned *only* when the compiler can prove the backing storage outlives the
  return. Returning a view rooted at a function-local owned value is rejected
  (E0513), because that local is freed at return; returning a view rooted at a
  parameter is sound (the caller owns the storage).

```cplus
fn first_word(s: str) -> str { return s; }   // OK: view rooted at a parameter

fn bad() -> str {
    let t: Text = build();
    return t;                                 // E0513: `t` is freed at return;
}                                             //        the coerced str view would dangle
```

A `Text` coerces to its `str` view wherever a `str` is expected (§ Strings in
SPEC.md); the same escape rule applies to the coerced view.

## 7. Concurrency without references

Because there are no value-site references, a value cannot be lent to another
thread. Work that crosses a thread or task boundary takes ownership by value, or
shares through a reference-counted handle.

- Run work on another thread by moving the inputs in:

  ```cplus
  let h: thread::JoinHandle[O] = thread::spawn_with(take input, worker);
  let out: O = h.join();
  ```

- Share read access across threads with `Arc[T]` (atomic reference count). Each
  thread holds a clone; the last drop frees the value:

  ```cplus
  let a: Arc[T] = arc::new(value);
  let b: Arc[T] = a.clone();   // a and b refer to the same value
  ```

- Communicate by transferring ownership over a channel:

  ```cplus
  ch.send(take v);                       // ownership moves into the channel
  let r: channel::RecvResult[T] = ch.recv();
  ```

- Async futures own their captured state by value; an executor drives them:

  ```cplus
  let result: T = executor::block_on(amain());
  ```

The common thread is the same as everywhere else in C+: ownership is passed by
value (`take`), or shared explicitly (`Arc`), never lent as a bare reference.

## 8. Drop

Teardown is automatic and recursive. At scope exit the compiler runs the
value's own `drop(ref this)` (if it has one), then drops each **owning field**
in reverse declaration order, interleaved with `defer` statements (LIFO). Owning
fields are `Text`, `Vec`/`Box` and other library types with their own `drop`,
aggregates containing such a field, arrays of those, and the active payload of a
tagged enum. Raw `*T` fields are **not** auto-dropped — they stay the author's
responsibility, freed in `drop` or declared `opaque` (E0510, §6 of SPEC).

A type with a `Drop` impl, or any owning field, is never `Copy`. Moving an
owning field out of such a value is rejected (E0509), since the auto-drop would
then free it a second time — `clone` it, or `match` the whole value to consume
it.

## 9. Diagnostics

| code  | situation |
|-------|-----------|
| E0312 | a value-site reference (`&` / `&mut`) was written |
| E0328 | a `ref` argument or a mutating method needs a `var` place |
| E0335 | use of a moved (`take`-d) value |
| E0337 | a bare borrow escapes (returned, stored, or re-passed to `take`) |
| E0370–E0385 | access to a value that conflicts with a live borrow of it |
| E0509 | moving a field out of a `Drop` value |
| E0513 | a returned `str` / slice view is rooted at a function-local owned value |

## 10. Summary

- The default `x: T` and `this` are shared (read-only) borrows by pointer; the
  caller keeps ownership and drops.
- `ref` lends an exclusive (mutable) pointer, with write-back to the caller's
  `var`.
- `take` passes by value and transfers ownership.
- A borrow cannot escape its call. To hand a value out, return an owned value or
  a blessed view whose storage outlives the return.
- Concurrency moves ownership in (`take`) or shares through `Arc` and channels.
