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
| [`str`](#str) | the builtin `str` view's method set (the one `impl str` block) |
| [`text`](#text) | `Text`, `CString` |
| [`io`](#io) | `print`, `println`, `eprintln` |
| [`fs`](#fs) | `File`, path helpers |
| [`net`](#net) | `TcpStream`, `TcpListener` |
| [`netsys`](#netsys) | platform errno / constants (for `net`) |
| [`env`](#env) | `var`, `argc`, `arg` |
| [`flags`](#flags) | `Flags` option-set over u64 bits |
| [`slice`](#slice) | checked sub-views over `T[]` |
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

## str

Methods on the builtin string view. `str` itself is a language type
(`{ptr, len}`, Copy, borrowed); this module holds the single `impl str`
block that declares its method set — the compiler admits exactly one such
block program-wide (a second is E0385, anywhere).

Available in any file once the build imports `stdlib/str` — directly, or
transitively via `stdlib/text` (text imports str for its own reads, so
most programs already have it). With neither import, `s.count()` is E0324
with a note naming the fix.

```cplus
import "stdlib/str" as _;   // discard alias — or just import "stdlib/text"

"user@example.com".drop_first(count: 5);   // "example.com" — a view, no copy
"  42  ".trim().to_i64();                  // Option[i64]::Some(42)
"a,b,c".split(separator: ",").count();     // 3
```

```cplus
impl str {
    // reads — all #[no_alloc]
    fn count(this) -> usize                    // byte length; there is NO len()
    fn is_empty(this) -> bool
    fn char_count(this) -> usize               // Unicode scalars, not bytes
    fn is_ascii(this) -> bool
    fn byte_at(this, index: usize) -> option::Option[u8]
    fn has_prefix(this, prefix: str) -> bool
    fn has_suffix(this, suffix: str) -> bool
    fn contains(this, needle: str) -> bool
    fn find(this, needle: str) -> option::Option[usize]
    fn rfind(this, needle: str) -> option::Option[usize]
    fn count_of(this, needle: str) -> usize    // non-overlapping
    fn equals_ignoring_case(this, other: str) -> bool
    fn compare(this, other: str) -> i32        // -1 / 0 / 1, byte order

    // sub-views — endpoints move, no bytes copied; all #[no_alloc]
    fn slice(this, from: usize, to: usize) -> option::Option[str]
    fn prefix(this, count: usize) -> str       // clamps, never traps
    fn suffix(this, count: usize) -> str
    fn drop_first(this, count: usize = 1) -> str
    fn drop_last(this, count: usize = 1) -> str
    fn removing_prefix(this, prefix: str) -> str
    fn removing_suffix(this, suffix: str) -> str
    fn trim(this) -> str
    fn trim_start(this) -> str
    fn trim_end(this) -> str

    // the one allocating member: one Vec, pieces are views
    fn split(this, separator: str) -> vec::Vec[str]

    // parse — strict, no whitespace tolerance (trim() first)
    fn to_i64(this) -> option::Option[i64]
    fn to_f64(this) -> option::Option[f64]
}
```

- A sub-view shares the receiver's buffer: valid exactly as long as the
  backing bytes (a literal: forever; a `Text`'s view: until the `Text`
  mutates or drops). Empty results are the `""` literal, so the pointer is
  always valid and non-null.
- `to_i64`: optional `+`/`-`, decimal digits, `None` on stray bytes or
  overflow. `to_f64`: sign, digits, optional `.digits`, optional `e`/`E`
  exponent, at most 63 bytes; the value comes from libc `strtod`, so
  rounding is correct to the last bit; `inf`/`nan`/hex never pass.
- Compiler-provided, not from this block: `to_text()` (owned copy; needs
  `stdlib/text`), `hash()`, `eq()`.
- The `#str_ptr` / `#str_len` / `#str_from_raw_parts` intrinsics remain
  the FFI tier — for crossing into C, not for string work.
- Slices and arrays carry the same two core reads, compiler-provided:
  `xs.count()`, `xs.is_empty()` (their FFI tier is `#slice_*`).

---

## text

Reads are the same operations `str` has — `Text` delegates them through
`view()` — with the difference that `Text`'s transform family returns
owned copies where `str`'s returns borrowed sub-views.

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

## flags

A typed option-set over `u64` bits — set membership spelled as set membership
instead of mask arithmetic. Bit values come from `const` masks (constant
expressions fold: `const BOLD: u64 = 1u64 << 0;`) or a payload-free enum with
explicit discriminants, cast at the call site.

```cplus
import "stdlib/flags" as flags;

var style = flags::none().with(BOLD).with(UNDER);
style.contains(BOLD);          // true — holds EVERY bit of the argument
style.intersects(BOLD | IT);   // true — holds ANY bit
style = style.without(UNDER);
style = style.toggled(IT);
a.union_with(b); a.intersect_with(b); a.minus(b); a.eq(b);
style.bits();                  // the raw u64, for storage or C
flags::from_bits(raw);         // rehydrate
```

`Flags` is one machine word, Copy; every mutator returns the new set.

## slice

Checked sub-views over `T[]` — the generalization of `str.slice(from:to:)`
to every element type. Views of the SAME buffer, no allocation.

```cplus
import "stdlib/slice" as slice;

slice::sub::[i32](s, 1, 4);      // Option[i32[]] — None on a bad range
slice::prefix::[i32](s, 2);      // first 2 (clamped)
slice::suffix::[i32](s, 3);      // last 3 (clamped)
slice::drop_first::[i32](s, 1);
slice::drop_last::[i32](s, 1);
```

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
fn from_raw[T](raw: *u8) -> Box[T]

impl Box[T] {
    fn set(ref this, take v: T)
    fn unwrap(take this) -> T
    fn into_raw(take this) -> *u8
}

impl Box[T: Copy] {
    fn value(this) -> T
}
```

Unique heap ownership. `new` returns `None` if allocation fails. `value`
copies the payload out and is available only for `Copy` payloads; `unwrap`
consumes the handle and is the non-`Copy` route. `into_raw` releases
ownership to the caller, `from_raw` takes it back; pairing them is the
caller's responsibility.

---

## rc

```cplus
struct Rc[T] { /* private */ }
struct Weak[T] { /* private */ }

fn new[T](take v: T) -> option::Option[Rc[T]]

impl Rc[T] {
    fn clone(this) -> Rc[T]
    fn downgrade(this) -> Weak[T]
    fn strong_count(this) -> u64
    fn weak_count(this) -> u64
    fn is_unique(this) -> bool
    fn with_mut(ref this, f: fn(ref T)) -> status::Status
    fn try_unwrap(take this) -> option::Option[T]
}

impl Rc[T: Copy] {
    fn value(this) -> T
}

impl Weak[T] {
    fn clone(this) -> Weak[T]
    fn strong_count(this) -> u64
    fn weak_count(this) -> u64
    fn upgrade(this) -> option::Option[Rc[T]]
}
```

Single-threaded reference counting. `new` returns `None` if allocation
fails. Not `Send`/`Sync`: passing an `Rc` to a `Send`/`Sync`-bounded generic
is rejected at sema time (E0502). Use [`arc`](#arc) across threads.

A `Weak` keeps the control block addressable without keeping the value
alive, which is what breaks the reference cycles a graph of `Rc`s would
otherwise leak (parent/child, observer/subject). `upgrade` yields `None`
once the last strong handle is gone. The value is dropped when the strong
count reaches 0; the block itself is freed when the last `Weak` goes, so a
live `Weak` safely outlives the value.

`weak_count` reports user-held `Weak` handles only. The strong handles
collectively own one implicit weak, which this excludes while any strong
handle remains.

`with_mut` gives exclusive access to the payload in place, returning `Ok`
only when the caller holds the sole strong handle **and** no `Weak` exists —
a live `Weak` could be upgraded to a second strong handle mid-mutation. It
returns `Shared` otherwise. `try_unwrap` consumes the handle and recovers
the payload on the same uniqueness condition, and is the extraction route
for non-`Copy` payloads (`value` stays `Copy`-gated). There is no
`make_mut`: copy-on-write needs a `T: Clone` story C+ does not have, so use
`try_unwrap` plus `new`.

---

## arc

```cplus
struct Arc[T] { /* private */ }
struct Weak[T] { /* private */ }

fn new[T](take v: T) -> option::Option[Arc[T]]

impl Arc[T] {
    fn clone(this) -> Arc[T]
    fn downgrade(this) -> Weak[T]
    fn strong_count(this) -> u64
    fn weak_count(this) -> u64
    fn is_unique(this) -> bool
    fn with_mut(ref this, f: fn(ref T)) -> status::Status
    fn try_unwrap(take this) -> option::Option[T]
}

impl Arc[T: Copy] {
    fn value(this) -> T
}

impl Weak[T] {
    fn clone(this) -> Weak[T]
    fn strong_count(this) -> u64
    fn weak_count(this) -> u64
    fn upgrade(this) -> option::Option[Arc[T]]
}

impl Arc[T: Send + Sync]: Send {}
impl Arc[T: Send + Sync]: Sync {}
impl Weak[T: Send + Sync]: Send {}
impl Weak[T: Send + Sync]: Sync {}
```

Atomic reference counting. Same API and same weak/uniqueness rules as
[`rc`](#rc), with atomic counter updates. Both `Arc[T]` and its `Weak[T]`
are `Send + Sync` when `T` is. Prefer `Rc` within a single thread; the
atomics are the only difference and they are not free.

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

## base64

base64 and base64url, both directions (RFC 4648 §4 and §5).

```cplus
import "stdlib/base64" as base64;

base64::encode("foobar")          // "Zm9vYmFy"     (padded)
base64::encode_url(json_src)      // unpadded — the JWT form
base64::decode(s)                 // Option[Vec[u8]]
base64::decode_url(s)             // Option[Vec[u8]] — accepts padded AND unpadded
base64::decode_url_text(s)        // Option[Text], for a segment known to be text
```

| | |
|---|---|
| `encode` / `encode_bytes` | standard alphabet, `=`-padded |
| `encode_url` / `encode_url_bytes` | url-safe alphabet, **no padding** |
| `decode` / `decode_url` | `Option[Vec[u8]]`; None on anything malformed |
| `decode_url_text` | `Option[Text]`; validates base64, not UTF-8 |

**Decoding is strict.** A character outside the alphabet is a failure, not a
character to skip — a lenient decoder is one where two different inputs produce
the same bytes, which breaks anything that signs the encoded form. Refused:
non-alphabet bytes (including whitespace and newlines, so strip them yourself
for wrapped MIME), a length that cannot occur, and padding anywhere but the end.

Both decoders accept padded and unpadded input, because that is a genuine
difference between producers rather than malleability — the padding carries no
information.

---

## crypto

SHA-2 digests, HMAC and the system CSPRNG, over CommonCrypto (libSystem — no
link flag, no vendored dependency). **macOS/BSD only today**: this is the base
variant, and a `crypto_linux` / `crypto_windows` shadow would take over on
those targets the way `netsys_linux` does. Neither is written yet.

```cplus
import "stdlib/crypto" as crypto;

let d: crypto::Digest = crypto::sha256("abc");
d.hex()                      // "ba7816bf8f01cfea…"  (64 chars, lowercase)
d.byte(0 as usize)           // bounds-checked; 0 past the end
d.count()                    // 32

let mac: crypto::Digest = crypto::hmac_sha256(secret, signing_input);
mac.equals(supplied)         // CONSTANT TIME — see below
```

| | |
|---|---|
| `sha256(data: str) -> Digest` | 32 bytes |
| `sha256_bytes(p: *u8, n: usize) -> Digest` | |
| `sha512(data: str) -> Digest512` | 64 bytes |
| `sha512_bytes(p: *u8, n: usize) -> Digest512` | |
| `hmac_sha256(key: str, msg: str) -> Digest` | the JWT HS256 shape |
| `hmac_sha256_bytes(key, klen, msg, mlen) -> Digest` | any key length; RFC 2104 rules applied by the platform |
| `hmac_sha512(…) -> Digest512` | |
| `bytes_equal(a: *u8, b: *u8, n: usize) -> bool` | constant time |
| `random_bytes(p: *u8, n: usize) -> bool` | kernel CSPRNG; `false` = buffer NOT usable |
| `random_hex(n: usize) -> Option[Text]` | ≤ 64 bytes per call |

`Digest::equals` and `bytes_equal` **do not stop at the first differing byte**.
Comparing a computed MAC against a supplied one with an early-exit compare
leaks, through timing, how many leading bytes matched — enough to forge a
signature one byte at a time. Use them, not a byte loop of your own, wherever a
secret is on one side of the comparison.

Not a general crypto library: no public-key work, no ciphers, no KDFs. Those
want a real library and a real review. This is the corner a credential check
needs.

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
