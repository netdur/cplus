# Guide

How stdlib is organized, which module to pick, and the contracts that apply
across packages. Fast start: [tutorial.md](tutorial.md). Catalog:
[ref.md](ref.md).

## Import model

| Write | Loads |
|---|---|
| `import "stdlib/vec" as vec` | `src/vec.cplus` |
| `import "stdlib/text" as text` | `src/text.cplus` |

There is no `import "stdlib"` barrel for apps. `src/stdlib.cplus` only
imports many modules so `cpc test` discovers their `#[test]`s.

Cross-module use inside stdlib is normal (`vec` imports `option`, `status`,
`iterator`; `fs` imports `result`, `text`, …). Consumers depend on
`stdlib = "*"` once and import the modules they need.

## Module map

### Core types

| Module | Role |
|---|---|
| `option` | `Option[T]` — present / absent |
| `result` | `Result[T, E]`, shared `IoError` |
| `status` | Fallible mutator outcome (`Ok`, OOM, bounds, …) |
| `marker` | Docs anchor for blessed `Send` / `Sync` (compiler-known) |

### Collections and text

| Module | Role |
|---|---|
| `vec` | Growable `Vec[T]`; no bitwise copy of non-`Copy` `T` out of storage |
| `str` | The builtin `str` view's methods (`count`, `find`, `trim`, `split`, `to_i64`, …) — comes in with `text` |
| `text` | Owned `Text`, `CString`; `str` views |
| `hash_map` | `HashMap[K, V]` with `K: Copy, V: Copy` |
| `hash_set` | `HashSet[T]` with `T: Copy` |
| `string_map` | `StringMap[V]` — owned `Text` keys (non-`Copy` values via pointers) |
| `string_set` | `StringSet` — owned `Text` elements |
| `cow` | `CowStr` — borrowed `str` or owned `Text` |

### Iteration

| Module | Role |
|---|---|
| `iterator` | `Iterator[T]` shape for `gen fn` |
| `range` | `range` / `range_inclusive` generators |

### Ownership wrappers

| Module | Role |
|---|---|
| `box` | Unique heap `Box[T]` |
| `rc` | Single-thread refcount + `Weak` |
| `arc` | Atomic refcount + `Weak` (`Send`/`Sync` when `T` is) |

### Concurrency

| Module | Role |
|---|---|
| `atomic` | Free functions over `*T` + `Ordering` (compiler intrinsics) |
| `thread` | 1:1 OS threads, join with value |
| `mutex` | `Mutex[T]` + `MutexGuard[T]` (pthread) |
| `channel` | Unbounded MPMC `Channel[T]` |

### Async

| Module | Role |
|---|---|
| `future` | `Future[T]`, `Poll[T]` (compiler shapes) |
| `executor` | `block_on`, `spawn_local` |
| `reactor` | Process-global I/O reactor (kqueue / epoll / Windows via override) |
| `time` | Async timer helpers on the reactor |

### I/O and environment

| Module | Role |
|---|---|
| `io` | `print` / `println` / `eprintln` |
| `fs` | `File`, paths, dirs, `read_to_string` |
| `net` | `TcpStream`, `TcpListener` |
| `netsys` | Platform constants/errno for `net` (auto-overridden per OS) |
| `env` | env vars and argv |
| `crypto` | SHA-2, HMAC, CSPRNG — over the platform's own (macOS today) |

## Error model

Three complementary outcomes — pick by role:

| Type | When |
|---|---|
| `Option[T]` | Value may be absent (lookup, pop empty, parse optional) |
| `Result[T, E]` | Fallible op that yields `T` or an error value (`IoError` for I/O) |
| `Status` | Mutator that does not return a payload (`append` OOM, out of bounds) |

C+ libraries **do not trap** for ordinary failure. Match or `guard let`; do
not ignore `Status` / `Result` if you care about integrity.

`IoError` lives in `result` so `fs` and `net` share the same error type and
compose in one `Result` family.

## Ownership and Copy bounds

- **`Vec[T]`** never bitwise-copies a non-`Copy` `T` out of its buffer.
  `at` requires `Copy`; non-copy access uses `at_ptr`, `each_ref`, `fold_ref`,
  or consuming removes (`remove`, `remove_last`, …).
- **`HashMap` / `HashSet`** require `Copy` keys/values (and elements). For
  owned strings use **`StringMap` / `StringSet`**.
- **`Text`** owns a heap buffer; `.view()` / coercion to `str` is a borrow
  into that buffer — dangling if the `Text` is dropped or reallocated by
  `append` / growth.
- **`Rc` vs `Arc`**: same API idea; `Rc` is single-thread (cheaper); `Arc`
  for cross-thread sharing when `T: Send + Sync`.
- **`Mutex` / `Channel` / `Arc`**: respect `Send`/`Sync`; do not share
  non-`Send` types across threads.

## Platform overrides

The package resolver substitutes platform files by suffix:

| Import name | Default (e.g. macOS) | Override examples |
|---|---|---|
| `stdlib/netsys` | `netsys.cplus` | `netsys_linux`, `netsys_windows` |
| `stdlib/reactor` | `reactor.cplus` (kqueue) | `reactor_linux` (epoll), `reactor_windows` |

App code always imports the short name. Do not import `*_linux` paths
yourself unless you know you need them.

## Async shape

1. `async fn` produces a `Future[T]` (compiler).
2. `executor::block_on(fut)` drives it to completion on the current thread.
3. I/O waiters register with `reactor` (read/write/timer); the executor
   polls until ready.

This is a **current-thread** style runtime, not a multi-threaded work
stealing pool. See module headers for registration details.

## Gotchas

### Stale README lore

Older notes described stdlib as pure stubs. The tree is largely
implemented with unit tests. Prefer this guide and the source headers
over historical plan excerpts.

### `cpc test` entry vs library imports

Running tests from `vendor/stdlib` uses `stdlib.cplus` as the root. Your
app still imports individual modules; you do not depend on `stdlib.cplus`
as a public API.

### Global reactor / config

The reactor is process-global and lazily initialized. Treat it like other
process-wide resources: single-threaded async in v0, no free-for-all from
arbitrary OS threads without a design for it.

### HashMap keys are Copy

`str` keys work when the pointed-to bytes outlive the map. Owned string
keys → `StringMap`.

### Status vs Result on collections

`Vec.append` returns `Status` (e.g. OOM), not `Result`. Check it when
allocation failure matters.

### Integration harness under tests/

`tests/lang_e2e.rs` is an archived **Rust** harness (builds temp C+
programs via `cpc`). It is not package documentation and is not run by
`cpc test`. Unit coverage is the `#[test]` functions inside `src/*.cplus`.

## Choosing a collection

```
Need ordered, growable list?
  → vec::Vec[T]

Need unique membership, T: Copy?
  → hash_set::HashSet[T]
  Text elements?
  → string_set::StringSet

Need key → value, K/V Copy?
  → hash_map::HashMap[K, V]
  Text keys?
  → string_map::StringMap[V]

Need shared ownership?
  one thread → rc::Rc[T]
  many threads → arc::Arc[T]
  unique heap slot → box::Box[T]
```
