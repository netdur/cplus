# Reference

Manual for the `stdlib` package. Per-module catalog and public signatures.
Internal helpers and `#[test]` functions are omitted.

Import pattern:

```cplus
import "stdlib/<module>" as <module>;
```

Source of truth for edge cases: the header comment and impl in
`src/<module>.cplus`.

---

## Module index

| Module | Primary types / entry points |
|---|---|
| [`option`](#option) | `Option[T]`, `some` |
| [`result`](#result) | `Result[T, E]`, `IoError`, `ok` / `err` / `io_ok` / `io_err` |
| [`status`](#status) | `Status` |
| [`vec`](#vec) | `Vec[T]` |
| [`text`](#text) | `Text`, `CString` |
| [`io`](#io) | `print`, `println`, `eprintln` |
| [`fs`](#fs) | `File`, path helpers |
| [`net`](#net) | `TcpStream`, `TcpListener` |
| [`netsys`](#netsys) | platform errno / constants (for `net`) |
| [`env`](#env) | `var`, `argc`, `arg` |
| [`hash_map`](#hash_map) | `HashMap[K, V]` |
| [`hash_set`](#hash_set) | `HashSet[T]` |
| [`string_map`](#string_map) | `StringMap[V]` |
| [`string_set`](#string_set) | `StringSet` |
| [`cow`](#cow) | `CowStr` |
| [`box`](#box) | `Box[T]` |
| [`rc`](#rc) | `Rc[T]`, `Weak[T]` |
| [`arc`](#arc) | `Arc[T]`, `Weak[T]` |
| [`iterator`](#iterator) | `Iterator[T]` |
| [`range`](#range) | `range`, `range_inclusive` |
| [`atomic`](#atomic) | `Ordering`, load/store/fetch/\* |
| [`thread`](#thread) | OS threads |
| [`mutex`](#mutex) | `Mutex[T]`, `MutexGuard[T]` |
| [`channel`](#channel) | `Channel[T]`, `ReceiveResult[T]` |
| [`future`](#future) | `Future[T]`, `Poll[T]` |
| [`executor`](#executor) | `block_on`, `spawn_local` |
| [`reactor`](#reactor) | event loop registration / poll |
| [`time`](#time) | async timers |
| [`marker`](#marker) | `Send` / `Sync` documentation anchor |
| [`stdlib`](#stdlib-test-entry) | test umbrella only |

Platform override sources (not imported by short name): `netsys_linux`,
`netsys_windows`, `reactor_linux`, `reactor_windows`.

---

## option

```cplus
enum Option[T] {
    Some(T),
    None,
}

fn some[T](take v: T) -> Option[T]
```

`some` moves `v` into `Some`. Construct `None` as `Option[T]::None`.

---

## result

```cplus
enum Result[T, E] {
    Ok(T),
    Err(E),
}

enum IoError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    Interrupted,
    UnexpectedEof,
    InvalidInput,
    InvalidData,
    WouldBlock,
    ConnectionRefused,
    ConnectionReset,
    AddrInUse,
    BrokenPipe,
    TimedOut,
    Other,
}

fn ok[T, E](take v: T) -> Result[T, E]
fn err[T, E](take e: E) -> Result[T, E]
fn io_ok[T](take v: T) -> Result[T, IoError]
fn io_err[T](e: IoError) -> Result[T, IoError]
```

Shared error type for filesystem and TCP APIs.

---

## status

```cplus
enum Status {
    Ok,
    OutOfMemory,
    OutOfBounds,
    InvalidInput,
    Shared,
}

fn is_ok(this) -> bool
fn is_err(this) -> bool
```

Outcome of fallible mutators that do not return a payload (e.g. `Vec.append`).
`Copy`; failed ops leave the receiver valid.

---

## vec

```cplus
struct Vec[T] { /* private */ }

fn new[T]() -> Vec[T]
fn with_capacity[T](minimum: usize) -> Vec[T]
fn collect[T](take source: iterator::Iterator[T]) -> Vec[T]
```

| Method | Role |
|---|---|
| `count` / `capacity` / `is_empty` | sizing |
| `reserve` / `shrink_to_fit` | capacity (`Status`) |
| `append` / `insert` / `set` | add/replace (`Status`) |
| `remove` / `remove_last` / `swap_remove` | take element out (`Option[T]`) |
| `remove_all` / `truncate` | clear / shorten |
| `at` | `Option[T]` when `T: Copy` |
| `at_ptr` | `Option[*T]` in-place |
| `first` / `last` | ends (`Copy`) |
| `as_slice` / `as_byte_view` | views |
| `each_ref` / `fold_ref` | borrow iteration |
| `append_slice` / `extend_from_raw` | bulk extend |

Does not bitwise-copy non-`Copy` `T` out of the buffer.

---

## text

```cplus
struct Text { /* private owned buffer */ }
struct CString { /* NUL-terminated, owned */ }

fn new() -> Text
fn with_capacity(minimum: usize) -> Text
fn from_str(s: str) -> Text
fn repeating(s: str, count: usize) -> Text
fn from_utf8(take bytes: vec::Vec[u8]) -> option::Option[Text]
fn join(parts: vec::Vec[Text], separator: str = "") -> Text
fn hash_str(s: str) -> u64
```

| Method group | Examples |
|---|---|
| Size | `count`, `capacity`, `is_empty`, `char_count` |
| Mutate | `append`, `insert`, `remove_range`, `remove_all`, `truncate`, `reserve` → often `Status` |
| Borrow | `view() -> str` |
| Search | `find`, `rfind`, `contains`, `has_prefix`, `has_suffix` |
| Transform | `slice`, `trim*`, `lowercased` / `uppercased*`, `split`, `replacing`, `pad_*` |
| Compare | `equals`, `equals_ignoring_case`, `compare` |
| FFI | `c_str() -> Option[CString]` |
| Traits | `Eq`, `Hash`, `Send`, `Sync` |

`CString`: `as_ptr`, `count`, drop frees.

---

## io

```cplus
fn print(s: str)
fn println(s: str)
fn eprintln(s: str)
```

Stdout / stderr via libc `write`. Best-effort; no `Result`.

---

## fs

```cplus
struct File { /* owned fd; Drop closes */ }

fn open_read(path: str) -> result::Result[File, result::IoError]
fn create(path: str) -> result::Result[File, result::IoError]
fn exists(path: str) -> bool
fn create_dir(path: str) -> result::Result[bool, result::IoError]
fn create_dir_all(path: str) -> result::Result[bool, result::IoError]
fn remove_file(path: str) -> result::Result[bool, result::IoError]
fn read_dir(path: str) -> option::Option[vec::Vec[text::Text]]
fn read_to_string(path: str) -> result::Result[text::Text, result::IoError]
```

| `File` method | Role |
|---|---|
| `read_to_end` | `Result[Vec[u8], IoError]` |
| `write_all` | `Result[usize, IoError]` |
| `close` | explicit close |
| `make_nonblocking` | `i32` status from fcntl-style op |

---

## net

```cplus
struct TcpStream { /* owned fd */ }
struct TcpListener { /* owned fd */ }

fn connect_tcp(ip: str, port: u16) -> result::Result[TcpStream, result::IoError]
fn listen_tcp(port: u16) -> result::Result[TcpListener, result::IoError]
```

| Type | Methods |
|---|---|
| `TcpStream` | `read_to_end`, `write_all`, `shutdown_write`, `close`, `make_nonblocking` |
| `TcpListener` | `accept`, `close`, `make_nonblocking` |

---

## netsys

Platform-specific constants and errno access for `net`. Import as
`stdlib/netsys`; resolver selects macOS/BSD, Linux, or Windows source.
Public surface is shared across overrides (numeric constants + errno
helper — see the active `netsys*.cplus`).

---

## env

```cplus
fn var(name: str) -> option::Option[text::Text]
fn has_var(name: str) -> bool
fn argc() -> usize
fn arg(index: usize) -> option::Option[text::Text]
```

Owned `Text` for values; `None` if unset / out of range.

---

## hash_map

```cplus
struct HashMap[K: Copy, V: Copy] { /* private */ }

fn new[K: Copy, V: Copy]() -> HashMap[K, V]
```

Typical methods: `insert`, `get`, `contains_key`, `remove`, `count`,
`is_empty`, grow on load factor ~0.75. Open addressing, linear probe.

---

## hash_set

```cplus
struct HashSet[T: Copy] { /* private */ }

fn new[T: Copy]() -> HashSet[T]
fn with_capacity[T: Copy](capacity: usize) -> HashSet[T]
```

`insert`, `contains`, `remove`, `clear`, set algebra helpers (`union`,
`intersection`, …), slot iteration — see source.

---

## string_map

```cplus
struct StringMap[V] { /* Text keys */ }

fn new[V]() -> StringMap[V]
```

Owned keys. Scalar/`Copy` values via `get`; non-copy values via pointer
APIs (see source). Overwrite drops old value.

---

## string_set

```cplus
struct StringSet { /* Text elements */ }

fn new() -> StringSet
fn with_capacity(capacity: usize) -> StringSet
```

Owned unique strings; same class of ops as `HashSet` for `Text`.

---

## cow

```cplus
enum CowStr { /* View / Owned — see source */ }

fn from_view(s: str) -> CowStr
fn from_owned(take s: text::Text) -> CowStr
fn is_owned(c: CowStr) -> bool
fn count(c: CowStr) -> usize
fn into_owned(take c: CowStr) -> text::Text
```

---

## box

```cplus
struct Box[T] { /* private */ }

fn new[T](take v: T) -> option::Option[Box[T]]
```

Unique heap ownership. `None` if allocation fails. Methods: get/set/unwrap
family (see source; `Copy` vs non-`Copy` impls).

---

## rc

```cplus
struct Rc[T] { /* private */ }
struct Weak[T] { /* private */ }

fn new[T](take v: T) -> option::Option[Rc[T]]
```

Single-threaded refcount. `clone`, strong/weak counts, `try_unwrap`,
`with_mut`, `downgrade` / `upgrade` — see source. Not `Send`/`Sync`.

---

## arc

```cplus
struct Arc[T] { /* private */ }
struct Weak[T] { /* private */ }

fn new[T](take v: T) -> option::Option[Arc[T]]
```

Atomic refcount. `Arc[T: Send + Sync]: Send + Sync`. Same conceptual API as
`Rc` with atomics.

---

## iterator

```cplus
struct Iterator[T] { /* compiler-known shape from gen fn */ }
```

Methods include `next`, and combinators such as `filter` / `map` / `prefix`
(see source). Produced by `gen fn`, not constructed by hand in normal code.

---

## range

```cplus
gen fn range(from: i32, to: i32) -> i32
gen fn range_inclusive(from: i32, through: i32) -> i32
```

Exclusive-end and inclusive `i32` ranges as iterators.

---

## atomic

```cplus
enum Ordering {
    // Relaxed, Acquire, Release, AcqRel, SeqCst — see source
}
```

Free functions on raw pointers, per width, e.g.:

```cplus
fn load_i32(p: *i32, ordering: Ordering) -> i32
fn store_i32(p: *i32, val: i32, ordering: Ordering)
fn swap_i32(...)
fn fetch_add_i32 / fetch_sub_i32 / fetch_and_i32 / fetch_or_i32 / fetch_xor_i32
fn compare_exchange_i32(...)
// i64, u32, u64 families likewise
fn fence(ordering: Ordering)
```

Backed by compiler `__cplus_atomic_*` intrinsics.

---

## thread

1:1 OS threads with value-returning join (see `src/thread.cplus` for
`spawn` / `join` surface and `Send` bounds).

---

## mutex

```cplus
struct Mutex[T] { /* private */ }
struct MutexGuard[T] { /* private; Drop unlocks */ }

fn new[T](take v: T) -> option::Option[Mutex[T]]
```

`lock() -> MutexGuard[T]`; guard provides read/write access (`with` /
`with_mut` style — see source). `Mutex[T: Send]: Send + Sync`.

---

## channel

```cplus
struct Channel[T] { /* private */ }
enum ReceiveResult[T] { /* value or closed — see source */ }

fn new[T]() -> option::Option[Channel[T]]
```

Unbounded MPMC. `send`, `receive`, `close`; `Channel[T: Send]: Send + Sync`.
`send` does not block; `receive` blocks until a value or closed.

---

## future

```cplus
struct Future[T] { /* compiler shape */ }
enum Poll[T] {
    Ready(T),
    Pending,
    // see source
}
```

Users obtain futures from `async fn`; they do not manually build the
control block in ordinary code.

---

## executor

```cplus
fn block_on[T](f: future::Future[T]) -> T
fn spawn_local[T: Send](take f: future::Future[T])
```

Single-threaded driver: poll until complete; optional local spawn.

---

## reactor

Process-global async I/O loop. Public registration/poll helpers include
forms such as:

```cplus
fn ensure() -> *u8
fn register_read(fd: i32, hdl: *u8)
fn register_write(fd: i32, hdl: *u8)
fn register_timer(ms: u64, hdl: *u8)
fn poll_one_event() -> bool
fn poll_one_event_nb() -> bool   // zero-timeout: consume one ready event or return
fn drain_pending() -> i32
// awaiter registration — see source
```

For external pumps (an event loop driving spawned futures without
`block_on`), stable C-ABI exports include `stdlib_reactor_kqfd_v1()` — the
kqueue fd, itself pollable, so a run loop can watch it — plus the `_v1`
forms of drain/poll above. facet's `spawn_ui` is the reference consumer.

Backend: kqueue (default), epoll (`reactor_linux`), Windows
(`reactor_windows`).

---

## time

Async timer primitives built on the reactor / futures (see
`src/time.cplus`).

---

## marker

Documentation anchor for blessed marker interfaces `Send` and `Sync`.
Registered by the compiler; names reserved. No runtime API beyond the
marker contracts.

---

## stdlib (test entry)

`src/stdlib.cplus` — not for application imports. Imports stdlib modules so
`cd vendor/stdlib && cpc test` discovers unit tests.

---

## Package

| | |
|---|---|
| Package name | `stdlib` |
| Modules | `stdlib/<name>` → `src/<name>.cplus` |
| Dependencies | none (libc via `extern fn` where needed) |
| Unit tests | `cpc test` from package root (`src/stdlib.cplus`) |
| Integration harness | `tests/lang_e2e.rs` (archived; not `cpc test`) |
