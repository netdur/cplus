# The C+ Memory Model

This document explains how ownership, borrowing, and moves work in C+. It
exists because the keyword names (`borrow`, `mut`, `move`) are familiar from
Rust's vocabulary, while the runtime mechanics are closer to C: values and
pointers, no copy constructor, and no value-site references. Reading the names
with Rust's full model in mind leads to wrong conclusions, so the mechanics are
spelled out here.

## 1. Principles

1. Every value has exactly one owner. When the owner's scope exits, the value's
   destructor runs (if any).
2. Memory safety is established statically. There is no garbage collector and no
   runtime ownership tracking.
3. A non-`Copy` value can only be *moved*, never duplicated. C+ has no copy
   constructor: an assignment or pass of a non-`Copy` value transfers ownership;
   it does not make a second independent value.
4. There are no value-site references. `&` and `&mut` parse but are rejected
   (E0312). Borrowing is expressed by parameter mode and by the `borrow A T`
   type, both of which lower to pointers internally.

## 2. Copy and non-Copy

A type is `Copy` when all of its components are `Copy` and it has no `Drop` impl.
Primitives (`i32`, `bool`, ...), raw pointers, `str`, slices, and aggregates of
those are `Copy`. A type with an owning field (`Text`, `Vec[T]`) or a `Drop`
impl is not `Copy`.

`Copy` values are duplicated on assignment and pass; the source stays valid.
non-`Copy` values are moved; the source becomes invalid, and a later use of it
is rejected (E0335).

```rust
let a: i32 = 5;
let b: i32 = a;     // copy; a still usable
let n: i32 = a;     // fine

let v: Text = Text::from_str("hi");
let w: Text = v;    // move; v is now invalid
let k: Text = v;    // E0335: use of moved value `v`
```

## 3. Move, and the absence of a copy constructor

Because there is no copy constructor, the only way to get a second value of a
non-`Copy` type is to construct one explicitly (for example, a method that
allocates a fresh copy of the contents). A plain bitwise duplication is never
inserted by the compiler for a non-`Copy` type; doing so would alias the same
owned resource and free it twice.

This is the single fact behind most of the rules below: a non-`Copy` value
cannot become two owned values for free.

## 4. Parameter modes

For a non-`Copy` type `T`, the parameter mode selects both the calling
convention (by value or by pointer) and the ownership effect (move or borrow).

| parameter      | passed     | ownership            | source after the call | callee may |
|----------------|------------|----------------------|-----------------------|------------|
| `x: T`         | by value   | move (consumed)      | invalid               | own it, return it, store it |
| `move x: T`    | by value   | move (consumed)      | invalid               | same as default |
| `mut x: T`     | by pointer | exclusive borrow     | still owned by caller | read and write through the pointer |
| `borrow x: T`  | by pointer | shared borrow        | still owned by caller | read through the pointer |

For a `Copy` type, every mode passes by value (a copy); the markers only affect
whether the parameter is locally reassignable. Mutations to a `Copy` parameter
never reach the caller.

```rust
fn take(b: B)        -> i32 { return b.x; }   // move: caller's b is consumed
fn bump(mut b: B)           { b.x = b.x + 1; return; } // exclusive borrow: writes reach the caller
fn read(borrow b: B) -> i32 { return b.x; }   // shared borrow: read only
```

```rust
fn run() {
    let mut v: B = mkB();
    let n: i32 = read(v);   // borrow: v still owned
    bump(v);                // exclusive borrow: v.x changed in place, v still owned
    let w: B = take(v);     // move: v consumed
    let m: i32 = v.x;       // E0335: v was moved into take
}
```

### Exclusive vs shared

Both `mut` and `borrow` pass a pointer to the caller's value; the caller keeps
ownership and runs the destructor. They differ in what is allowed while the
borrow is live:

- `mut` (exclusive): the callee may mutate. While the borrow is live, no other
  access to the source is permitted, not even a read. One writer, no one else.
- `borrow` (shared): read only. While the borrow is live, other shared reads of
  the same value are permitted. Many readers, no writer.

The invariant is: at any point a value has either one exclusive borrow or any
number of shared borrows, never both.

## 5. Borrows are not value-site references

There is no `&x` expression. A borrow exists only as a parameter mode (above)
or as the `borrow A T` type (below). The receiver forms cover method borrowing:

- `self` reads the receiver.
- `mut self` mutates the receiver in place (exclusive borrow).
- `move self` consumes the receiver.

A method that needs to borrow takes `self` or `mut self`; one that consumes
takes `move self`. By-value parameters cover the rest.

## 6. Returning a borrow

A borrow can be returned only through a *named region*. The `borrow A T` type
ties a parameter and a return together: the result is a reference into the
argument, valid for as long as the argument's region.

```rust
fn cursor(b: borrow A B) -> borrow A B { return b; }   // returns a reference into the caller's B
fn run() {
    let v: B = mkB();
    let cur: B = cursor(v);   // cur references v; it does not own a separate B
    let n: i32 = cur.x;       // read through the reference
    return;                   // only v drops
}
```

While the returned reference is live, the borrow checker tracks the source as
borrowed and rejects conflicting access (E0372, E0374, E0381, E0383).

What is **not** allowed is returning a borrow as an owned value:

```rust
fn steal(borrow r: B) -> B { return r; }   // E0337
fn steal2(mut b: B)   -> B { return b; }   // E0337
```

`return r` here would hand back an owned `B` while the caller still owns and
drops the original. With no copy constructor, the result would be a second owner
of the same resource, freed twice. The fix is to return a reference
(`-> borrow A B`) when handing back a borrow, or to take the value by value
(a move) when handing back ownership.

The shared region borrow (`b: borrow A B -> borrow A B`) is the supported form.
The exclusive returned borrow (`mut b: borrow A B -> borrow A B`) is currently
rejected as well: its lowering would copy the result, so it is treated like the
owned-return case until that lowering is completed.

## 7. Concurrency without references

Because there are no value-site references, a value cannot be lent to another
thread. Work that crosses a thread or task boundary takes ownership by value, or
shares through a reference-counted handle.

- Run work on another thread by moving the inputs in:

  ```rust
  let h: thread::JoinHandle[O] = thread::spawn_with(move input, worker);
  let out: O = h.join();
  ```

- Share read access across threads with `Arc[T]` (atomic reference count). Each
  thread holds a clone; the last drop frees the value:

  ```rust
  let a: Arc[T] = arc::new(value);
  let b: Arc[T] = a.clone();   // a and b refer to the same value
  ```

- Communicate by transferring ownership over a channel:

  ```rust
  ch.send(move v);                       // ownership moves into the channel
  let r: channel::RecvResult[T] = ch.recv();
  ```

- Async futures own their captured state by value; an executor drives them:

  ```rust
  let result: T = executor::block_on(amain());
  ```

The common thread is the same as everywhere else in C+: ownership is passed by
value (move), or shared explicitly (`Arc`), never lent as a bare reference.

## 8. Drop

A type implements `Drop` with a `drop(mut self)` method. Drop glue runs at scope
exit in reverse declaration order, interleaved with `defer` statements (LIFO).
A type with a `Drop` impl is never `Copy`. A destructor frees its own fields by
hand; the compiler does not synthesize per-field drops, and moving a field out
of a `Drop` value is rejected (E0509) because the destructor would free it
again.

## 9. Diagnostics

| code  | situation |
|-------|-----------|
| E0312 | a value-site reference (`&` / `&mut`) was written |
| E0335 | use of a moved value |
| E0337 | moving a non-binding place or a borrow into an owned value (partial move / borrow-to-owned) |
| E0372, E0374, E0381, E0383 | access to a value that conflicts with a live borrow of it |
| E0509 | moving a field out of a `Drop` value |

## 10. Summary

- Keyword names come from Rust; the mechanics are by-pointer borrows and
  by-value moves, with no copy constructor and no value-site references.
- `mut` and `borrow` lend a pointer; the caller keeps ownership and drops.
- The default and `move` pass by value and transfer ownership.
- A borrow becomes an owned value only by an explicit construction, never for
  free. Returning a borrow uses the `borrow A T` region type.
- Concurrency moves ownership in or shares through `Arc` and channels.
