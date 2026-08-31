# Concurrency — threads, async, and generators

The decision this page settles: **which of the four concurrency shapes a
piece of work wants**, and what each one costs. Signatures are in
[ref.md](ref.md); the aliasing rules underneath are
[memory-model.md](memory-model.md) §7.

C+ has no runtime that schedules for you. A thread is a pthread, an
`async fn` is a coroutine frame driven by an executor you call, and a
`gen fn` is a suspendable function with no concurrency in it at all. Nothing
starts until you start it.

## 1. Choosing

| Your work is… | Reach for | Because |
|---|---|---|
| CPU-bound over data that can be **split** | `thread::spawn_with` + `join` | no sharing, so no race and no lock |
| CPU-bound over a local you cannot move | `thread::scope()` + `lend` | the borrow checker enforces the join |
| a **pipeline** of stages | `channel::Channel[T]` | backpressure and shutdown are values |
| **I/O-bound**, many waits, one thread | `async fn` + `executor` | a suspended frame costs a frame, not a stack |
| producing a **sequence** lazily | `gen fn` | not concurrency: one stack, resumed |
| genuinely shared mutable state | `arc::Arc[T]` + `mutex::Mutex[T]` | last resort, and it is a real one |

The order is the order to try them in. Most designs that reach for a mutex
early wanted a clearer owner or a channel.

## 2. Partition and join

```cplus
import "stdlib/thread" as thread;

struct Span { start: i64, end: i64 }
fn sum(take s: Span) -> i64 { … }

let h1 = thread::spawn_with::[Span, i64](left,  sum);
let h2 = thread::spawn_with::[Span, i64](right, sum);
let total: i64 = h1.join() +% h2.join();
```

`spawn_with::[I, O](take input, f: fn(take I) -> O)` **moves** the input
into the worker; `spawn::[O](f: fn() -> O)` takes none. `join(take this)`
consumes the handle and returns the worker's value. `is_finished(this)`
polls without blocking.

Both type parameters are bounded `Send`. A struct or enum that hides a raw
pointer is `!Send` and `!Sync` by default, and passing one across the bound
is **E0502**. When you know a pointer-holding type is safe to move across
threads, say so with a marker impl — writing the impl the compiler would
not derive *is* the assertion:

```cplus
struct Handle { opaque _h: *u8 }
impl Handle: Send {}                    // unconditional
impl Arc[T: Send + Sync]: Send {}       // conditional — the bounds are the condition
```

A **bare** `*T` used directly (`thread::spawn::[*u8]`) stays `Send`; it is
the *hiding* inside a named type that flips it. `Rc` and `MutexGuard` are
`!Send`, and `Rc` is `!Sync` as well.

**Forgetting to join is not an error.** `JoinHandle`'s `drop` detaches the
worker and releases its half of a refcounted context — fire-and-forget, no
block, no leak — but the value the worker computed is gone, and nothing
waits for it before `main` returns. If you want the result, join.

## 3. Lending a local: `thread::scope`

`spawn_with` needs to *own* what it gets. When the data is a local you
cannot move — because the rest of the function still uses it — the scope
form lends it by `ref` and guarantees the join before the borrow ends:

```cplus
struct Cell { v: i64 }
fn tally(ref c: Cell) { … }

var a: Cell = Cell { v: 0 };
{
    var s: thread::Scope = thread::scope();
    let _ = s.lend::[Cell](a, tally);      // fn(ref Cell)
}                                          // Scope's drop joins here
assert a.v == expected;                    // safe to read: the worker is done
```

`lend[T: Send](ref this, ref data: T, f: fn(ref T)) -> Status` returns a
`Status` because spawning can fail. `count(this)` reports how many workers
are outstanding. The join is in `Scope`'s `drop`, which is what makes the
lifetime sound: the borrow cannot outlive the scope, so the worker cannot
outlive the data.

**A scope is a cancellation boundary.** Cancel the thread that owns one while
its workers are still running and the drop still joins — the parent blocks
until every worker finishes on its own, in the middle of a teardown that asked
to be quick. The cancel token is per-thread, so a cancelled parent does not
reach a worker it lent data to, and nothing tries to make it: the borrow's
soundness rests entirely on that join, and a cancellation that could race it
would trade a hang for a use-after-free on lent data. `lend` is a commitment to
wait. A worker that must be stoppable takes its own `JoinHandle` through
`spawn` / `spawn_with` and polls `thread::cancelled()` — it just cannot borrow
a parent local while doing so.

Two workers cannot lend the *same* local: the second `lend` is **E0381**,
"cannot borrow `a` exclusively while it is borrowed by `s`". That is
aliasing XOR mutability arriving at the thread boundary. Lending two
*different* locals to two workers is fine.

## 4. Channels

```cplus
import "stdlib/channel" as channel;

guard let option::Option[channel::Channel[Job]]::Some(ch) = channel::new::[Job]()
    else { return 1; };
let producer = ch.clone();              // cheap: a refcounted handle
```

`Channel[T]` is MPMC, internally refcounted, `Send + Sync` when `T` is.
The surface is small and every outcome is a value:

| Call | Returns |
|---|---|
| `send(this, take v: T)` | `Status` — `Ok`, or a failure when the channel is closed |
| `receive(this)` | `ReceiveResult[T]` — `Value(T)` / `Closed` / `Cancelled` |
| `try_receive(this)` | `TryReceiveResult[T]` — adds `Empty`, never blocks |
| `close(this)` | wakes every blocked receiver with `Closed` |

Shutdown is `close()`, not a sentinel message: every receiver learns at
once and the last one out drops the buffer.

## 5. `async fn` and the executor

```cplus
import "stdlib/executor" as executor;

async fn fetch(take url: text::Text) -> i32 { return (await get(url)) +% 1; }

fn main() -> i32 { return executor::block_on::[i32](fetch(u)); }
```

The rules that shape every async signature you will write:

- **`main` is never `async`.** The entry point is a plain `fn main` that
  calls a drive function. There is no hidden runtime to install.
- **Borrow-shaped parameters are rejected: E0900.** No `str`, no `T[]`, no
  `ref x: NonCopy` in an `async fn` signature. A coroutine frame outlives
  the call that created it, so a borrow in it has no owner to point at.
  Pass `Text` and `Vec[T]` — owned, moved in.
- **A `Future` cleans up however it ends.** `await`, `block_on`, `run` and
  `cancel` all consume one; a future that is merely dropped destroys its
  frame through its destructor, running the cancel edges as `cancel` would.
  There is nothing you have to remember to do.
- **32-bit targets have no async: E0867.**
- Each thread gets its own reactor, created on first use (kqueue on
  Darwin, epoll on Linux and Android).

Three ways to drive:

| Call | Behavior |
|---|---|
| `executor::block_on[T](f) -> T` | drive to completion; a cancel request does not stop it |
| `executor::run[T](take f) -> RunResult[T]` | drive **cancellably**: `Done(T)` or `Cancelled` |
| `executor::spawn_local[T](take f)` | hand the future to this thread's executor and return |

## 6. Cancellation

A worker is **asked** to stop, never killed. That is the whole design: a
killed thread cannot run its drops, and C+ has no way to express a value
whose destructor was skipped.

```cplus
let h = thread::spawn_with::[i64, i64](fd, serve);
h.cancel();                 // request — idempotent, non-consuming
let r = h.join();           // still waits, still returns the worker's value
```

- `cancel()` sets the flag and kicks the worker out of its *current*
  blocking call. It returns as soon as that kick is delivered, not when the
  worker stops.
- Inside the worker, `thread::cancelled()` is the ambient check for compute
  loops — a bare atomic load, safe anywhere, no token threaded through
  signatures. A loop that never checks it simply runs to completion; that
  is a correct program, just not a cancellable one.
- Blocking stdlib calls surface the request **as a value** instead of
  blocking forever: `Channel::receive` → `ReceiveResult::Cancelled`
  (buffered data still wins), `TcpStream::read_to_end` / `write_all` /
  `TcpListener::accept` → `IoError::Cancelled`, mutators →
  `Status::Cancelled`.
- `executor::run` destroys the suspended frame tree on a cancel request:
  every `await`'s destroy edge runs the drops of the locals live at that
  suspension point, transitively. Cancellation cannot skip a drop.
- **Dropping a future cancels it.** `Future` has a destructor: the frame is
  destroyed and every suspend point's cancel edge runs, wherever the value
  goes out of scope. `Future::cancel(take this)` is the same thing said as a
  verb, for when "I am giving up on this" should read at the call site.
- The async↔thread bridge: `executor::join_worker[O](take h, timeout: f64 = 0)`
  awaits a spawned thread's result without blocking the executor, and
  `executor::receive_or_cancel[T](take ch, timeout: f64 = 0)` is an async
  channel receive that surfaces `Cancelled`. Both poll on the reactor's timer.
  `timeout` is in seconds and anything `<= 0` means no deadline.

  An expiry answers with the vocabulary each already has: `receive_or_cancel`
  returns `Cancelled`, and `join_worker` **requests cancellation and still
  joins** — a cancelled worker's `join` returns its value, so the deadline is
  on the asking rather than a kill, and a worker that ignores the request is
  still waited for. Buffered data beats an expiry the same way it beats a
  cancellation. The clock is `time::now_millis`, so a system clock stepped
  backwards defers an expiry rather than firing it early.
- `thread::cancelled()` works inside `async fn` bodies — the token belongs
  to the thread, not to the coroutine.
- **Cancellation does not cross a process boundary.** A PTY child or a
  `process::Process` stops through `interrupt` / `terminate`.

Wrapping your own blocking FFI call so it participates:

```cplus
if thread::park_begin() { return; }     // true = already cancelled, don't park
let n = read(fd, buf, len);             // retry on EINTR: netsys::eintr()
thread::park_end();
```

For a `pthread_cond_wait`-shaped park, call `park_begin_cond(cond, mutex)`
*before* taking the mutex, re-check `cancelled()` inside the wait loop, and
`park_end()` *after* releasing it. stdlib owns SIGURG for the syscall kick.

## 7. Shared state, when nothing else fits

```cplus
guard let option::Option[mutex::Mutex[Counter]]::Some(m) = mutex::new::[Counter](c)
    else { return 1; };
let m2 = m.clone();                     // a second handle, same value

{
    var g = m.lock();                   // MutexGuard[T] — `var`: with_mut takes `ref this`
    g.with_mut(bump);                   // fn(ref T) — mutate in place
}                                       // the guard's scope IS the lock's scope
```

`Mutex[T]` is **internally refcounted**: there is no `Arc[Mutex[T]]`
wrapper to build, `clone()` gives another handle to the same value.
`MutexGuard` exposes `with(f: fn(T))` and `with_mut(f: fn(ref T))` — and
`value()` when `T: Copy` — rather than a dereferenceable field, so the lock
scope is a call and the guard is `!Send`.

Two guards live in one scope deadlock. Scope each lock:

```cplus
{ var g = a.lock(); g.with_mut(f); }
{ var g = b.lock(); g.with_mut(h); }
```

`stdlib/atomic` is the lock-free tier: `load_*` / `store_*` / `swap_*` /
`fetch_add_*` / `fetch_sub_*` / `fetch_and_*` / `fetch_or_*` /
`fetch_xor_*` / `compare_exchange_*` over `i32 i64 u32 u64`, each taking an
`Ordering` (`Relaxed`, `Acquire`, `Release`, `AcqRel`, `SeqCst`). They take
a `*T`, which is the honest shape: they are the raw tier and you supply the
address.

`arc::Arc[T]` shares ownership across threads (`Send + Sync` iff `T` is);
`rc::Rc[T]` is the single-threaded twin. Both offer `with_mut(f) -> Status`
— `Ok` only when this is the sole strong handle and no `Weak` exists — and
`try_unwrap() -> Option[T]`. There is no interior-mutability escape hatch:
shared mutation goes through that gate or through a mutex.

## 8. `gen fn` — sequences, not concurrency

A `gen fn` returns `Iterator[T]` and suspends at each `yield`. One stack,
one thread, no scheduler:

```cplus
gen fn ints_below(n: i32) -> i32 {
    var i: i32 = 0;
    while i < n { yield i; i = i +% 1; }
}

while let option::Option[i32]::Some(v) = it.next() { … }
```

Adapters: `it.filter(pred)` and `it.prefix(count)` are methods;
`iterator::map::[T, U](source, f)` is a free function (a method cannot
introduce a type parameter of its own). The name is `prefix`, not `take` —
`take` is the ownership keyword and cannot be an identifier.

`Iterator[T]` is the only thing `while let … = it.next()` drives, and
`for … in` takes exactly a range or an `Iterator[T]` (E0312). `Vec` supplies
one — `for x in v.iter()` — while arrays and slices have no `iter()` and are
indexed over `0..n`.

## 9. Gotchas

- **A `!Send` type gets a bound error at the spawn, not at the field that
  caused it.** E0502 names the type; the raw pointer inside it is what
  flipped the marker. Vouch with `impl T: Send {}` only when you have
  actually reasoned about it.
- **`block_on` ignores a cancel request.** If a worker must be stoppable,
  drive with `run` and handle `RunResult::Cancelled`.
- **A dropped `Future` leaks.** The frame is heap-allocated and only the
  four consuming paths free it.
- **`join` consumes the handle; `cancel` does not.** Cancel first, join
  second, in that order.
- **Two `lend`s of the same local do not compile**, and that is the feature
  — the alternative is a data race the type system already knows about.
- **`async` is not parallelism.** One executor drives one thread's futures.
  Parallel *and* asynchronous means threads that each run an executor, and
  `join_worker` to bridge them.
