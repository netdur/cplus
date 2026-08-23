# The standard library — what to import for what

The decision this page settles: **which `stdlib` module answers a need**,
and which of two overlapping modules is the right one. It is a map, not a
manual: each entry names the type and the shape of its surface, and the
module's own source is the signature reference (`cpc query members
vec::Vec`, or read `vendor/stdlib/src/<name>.cplus`).

Everything here is imported the same way:

```cplus
import "stdlib/io"  as io;
import "stdlib/str" as _;      // extensions only — no name bound
```

`stdlib` must be in `[dependencies]`. Nothing is in scope implicitly; even
`Option` and `Text` are imports.

## 1. The modules you will import in almost every file

| Module | Type | Surface |
|---|---|---|
| `io` | — | `print` · `println` · `eprintln` · `write_all` |
| `str` | extends `str` | every string **read**: `count` `is_empty` `char_count` `byte_at` `has_prefix` `has_suffix` `contains` `find` `rfind` `count_of` `compare` `equals_ignoring_case` `slice(from:,to:)` `prefix` `suffix` `drop_first` `drop_last` `removing_prefix` `removing_suffix` `trim` `trim_start` `trim_end` `split(separator:)` `to_i64` `to_f64` |
| `text` | `Text` | owned string: `new` `with_capacity` `from_str` `repeating` `from_utf8` `join` · `append` `insert` `truncate` `remove_range` `reserve` `clone` `view` `uppercased` `lowercased` `appending` `replacing` `pad_start` `pad_end` `c_str` `intern` |
| `option` | `Option[T]` | `Some(T)` / `None` — variants and constructors only, no combinators |
| `result` | `Result[T, E]`, `IoError` | `Ok(T)` / `Err(E)`; `IoError` is the blessed I/O error enum |
| `status` | `Status` | `Ok` `OutOfMemory` `OutOfBounds` `InvalidInput` `Shared` `Cancelled` |
| `vec` | `Vec[T]` | `new` `with_capacity` `collect` · `append` `insert` `remove` `remove_last` `swap_remove` `truncate` `set` `at` `first` `last` `as_slice` `each_ref` `fold_ref` `iter` |

`str` is the one imported `as _`: it carries the blessed `impl str` block,
and a file that reads a string without importing it gets "no such method"
on a type that plainly has one.

## 2. Collections

| Need | Module | Type |
|---|---|---|
| growable sequence | `vec` | `Vec[T]` |
| keyed lookup, `Copy` keys | `hash_map` | `HashMap[K: Hash + Eq + Copy, V: Copy]` |
| membership, `Copy` elements | `hash_set` | `HashSet[T]` — plus `is_subset` `is_superset` `is_disjoint` `union_with` `intersection` `difference` |
| **string**-keyed map | `string_map` | `StringMap[V]` — owns its keys; `get` / `get_ptr` |
| **string** set | `string_set` | `StringSet` — same set algebra as `HashSet` |
| checked windows over `T[]` | `slice` | free fns `sub` (→ `Option[T[]]`) `prefix` `suffix` `drop_first` `drop_last` |
| a bit-flag option set | `flags` | `Flags` over `u64`: `none` `of` `from_bits` · `contains` `intersects` `with` `without` `toggled` and set algebra |
| borrow-or-own a string | `cow` | `CowStr` — `from_view` / `from_owned` / `into_owned` |

**`HashMap` versus `StringMap`** is the fork worth naming: `HashMap`
requires `Copy` keys and values and does not own them, so a `Text` key
there is a bug waiting to happen. `StringMap[V]` owns its keys and is the
right choice for anything keyed by a runtime-built string.

Flag bit values come from `const` masks or from repr-enum discriminants
(`Mode::Fast as u64`) — build wide masks arithmetically, never as a bare
shifted literal.

## 3. Ownership wrappers

| Need | Module | Type |
|---|---|---|
| one heap value, one owner | `box` | `Box[T]` — `unwrap()` moves the value back out |
| shared, one thread | `rc` | `Rc[T]` / `Weak[T]` |
| shared, across threads | `arc` | `Arc[T]` / `Weak[T]` — `Send + Sync` iff `T` is |

`Rc` and `Arc` both carry `downgrade() -> Weak[T]` for cycle-breaking
back-pointers, `with_mut(f) -> Status` (`Ok` only when this is the sole
strong handle and no `Weak` exists), and `try_unwrap() -> Option[T]`.

Coming from C++: `unique_ptr` → `Box`, `shared_ptr` → `Arc` (or `Rc` when
single-threaded), `weak_ptr` → `Weak`. There is no interior-mutability
escape hatch; shared mutation goes through `with_mut` or a mutex.

## 4. Concurrency

| Module | Type | Note |
|---|---|---|
| `thread` | `JoinHandle[O]`, `Scope` | `spawn` `spawn_with` · `join` `is_finished` `cancel`; `scope()` + `lend`; `cancelled()` `park_begin` `park_end` |
| `channel` | `Channel[T]` | MPMC; `send` `receive` `try_receive` `close` `clone` |
| `mutex` | `Mutex[T]`, `MutexGuard[T]` | internally refcounted — no `Arc` wrapper needed |
| `atomic` | `Ordering` | `load_*` `store_*` `swap_*` `fetch_add_*` `fetch_sub_*` `fetch_and_*` `fetch_or_*` `fetch_xor_*` `compare_exchange_*` over `i32 i64 u32 u64`, plus `fence` |
| `future` | `Future[T]`, `Poll[T]` | the `async fn` protocol type; `cancel(take this)` |
| `executor` | `RunResult[T]` | `block_on` `run` `spawn_local`, and the `join_worker` / `receive_or_cancel` bridge |
| `reactor` | — | the kqueue/epoll loop under `executor`; you rarely name it |
| `iterator` | `Iterator[T]` | the `gen fn` protocol type; `.filter` `.prefix`, free `map` |
| `marker` | — | the `Copy` / `Send` / `Sync` framework; import for the names |

The judgment — which of these a piece of work wants — is
[concurrency.md](concurrency.md).

## 5. The operating system

| Module | Surface |
|---|---|
| `fs` | `File` · `open_read` `create` `open_append` `read_to_string` `write_string` `append_string` `read_dir` `create_dir` `create_dir_all` `remove_file` `exists` `is_dir` `metadata` `size` `mtime`; `File`: `read_to_end` `write_all` `write_str` `lines` `close` |
| `net` | `TcpStream` / `TcpListener` · `connect_tcp` `listen_tcp` · `read_to_end` `write_all` `shutdown_write` `accept` (IPv4, numeric addresses) |
| `env` | `var` `has_var` `argc` `arg` |
| `process` | `Child`, `Output` · `spawn` `capture` · `wait` `output` `write_stdin` `close_stdin` `signal` `interrupt` `terminate` `kill_now` |
| `pty` | `PtyChild` · `spawn` `read` `write` `resize` `wait` and the same signal set |
| `time` | `now()` (epoch seconds) · `now_millis()` |
| `date` | `DateTime`, `Civil` · `now` `today` `yesterday` `from_unix` `to_unix` `diff_seconds` `parse_iso8601` `format_iso8601` `format_date` |
| `bundle` | `executable_path` `dir` `resource(name)` `exists` `find_up(marker)` `find_up_from` — locating files beside the binary |
| `platform` | `Os` / `Arch` enums · `os()` `arch()` `target()` `is_simulator()` `pointer_width()` `is_apple()` `is_posix()` `is_hosted()` `cpu_count()` `os_version()` `path_separator()` `line_terminator()` |

`platform` is the runtime face of the compile-time intrinsics: `#platform()`
gives you a `str` at check time, `platform::os()` gives you a matchable
enum, and `platform::is_simulator()` / `os_version()` / `cpu_count()` are
facts no intrinsic can fold. Reach for the module when you want to `match`
rather than compare strings.

## 6. Data and crypto

| Module | Surface |
|---|---|
| `base64` | `encode` `encode_bytes` `encode_url` · `decode` `decode_url` `decode_url_text` (→ `Option`) |
| `crypto` | `Digest` / `Digest512` · `sha256` `sha512` (`_bytes` variants) · `hmac_sha256` `hmac_sha512` · `random_bytes` `random_hex` · `Digest::hex` `equals` (constant-time byte compare) |

## 7. The ones that are mostly shims

`range` (`0..n` lowers to `Range[i32]`), `marker`, `stdlib` (the umbrella
module), `exe_path`, `argv_sys`, `netsys`, `crypto_sys`, `platform_sys`.
The `*_sys` modules are the platform-variant floor other modules stand on —
they exist so `fs`, `net`, `crypto`, and `platform` can be one portable file
each. You import them only when you are extending stdlib itself.

## 8. What is not here

- **No iterator protocol over collections.** `Vec`, arrays, and slices are
  not `for … in` iterable (E0312). Index over `0..n`, or produce an
  `Iterator[T]` from a `gen fn`.
- **No `Option`/`Result` combinators.** No `map`, `unwrap_or`,
  `and_then`, `is_none`. Consume them with `match` or `guard let` —
  [error-handling.md](error-handling.md).
- **No formatting mini-language.** Interpolation (`"x = ${n}"`) has no
  format specifiers; build the string you want with `pad_start` /
  `pad_end` and the numeric conversions.
- **No `+` on strings.** Interpolate or `append`.
- **No panic and no exceptions.** Every fallible call returns a value.

## 9. Beyond stdlib

Everything else is a vendor package, imported by its own name
(`import "json/json" as json;`) and declared in `[dependencies]`. The
current set includes `json`, `log`, `uuid`, `http`, `sqlite`, `arena`,
`static-arena`, `simd`, `flex_layout`, `events`, the Apple bindings
(`appkit`, `uikit`, `metal`, `accelerate`, `quartzcore`, `objc`), the GTK
family, and the `facet` UI stack. Each ships its own tests
(`cd vendor/<pkg> && cpc test`) and many ship a `SKILL.md` that `cpc skill`
prints alongside the language reference.

Targets differ: the ESP32 targets exclude `thread`, `mutex`, `channel`,
`env`, `net`, `netsys`, `reactor`, `executor`, `time`, and `fs` outright —
importing one is E0866 ([platforms.md](platforms.md) §7).
