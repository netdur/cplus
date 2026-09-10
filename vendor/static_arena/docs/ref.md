# Reference

Manual for the `static-arena` package. Signatures and behavior only.

```cplus
import "static-arena/static-arena" as sa;
```

Two fixed-size types share the same method set. Documented once below;
`N` is 16384 or 65536.

---

## `StaticArena16K`

```cplus
struct StaticArena16K {
    _buf: [u8; 16384],
    _used: usize,
}
```

### `StaticArena16K::new`

```cplus
fn new() -> StaticArena16K
```

Empty arena; `used() == 0`, full capacity available.

### `capacity` / `used` / `remaining`

```cplus
fn capacity(this) -> usize   // always 16384
fn used(this) -> usize
fn remaining(this) -> usize  // capacity - used
```

### `reset`

```cplus
fn reset(ref this)
```

Set `_used = 0`. Does not Drop prior allocations. Prior pointers invalid.

### `alloc_bytes`

```cplus
fn alloc_bytes(ref this, count: usize, aligned_to: usize = 8, zeroed: bool = false) -> option::Option[*u8]
```

Bump `count` bytes at alignment (0 → 1). `None` if it does not fit; cursor
unchanged on failure. Optional `memset` when `zeroed`.

### `alloc`

```cplus
fn alloc[T](ref this, take value: T) -> option::Option[*T]
```

Space for `T` at `align_of(T)`, store `value`. **`T` should be Copy.**
`None` if it does not fit.

### `alloc_str`

```cplus
fn alloc_str(ref this, s: str) -> option::Option[str]
```

Copy `s` into the buffer; return a `str` view. `None` on overflow.

---

## `StaticArena64K`

```cplus
struct StaticArena64K {
    _buf: [u8; 65536],
    _used: usize,
}
```

Identical API to `StaticArena16K` with capacity **65536**.

| Method | Same as 16K |
|---|---|
| `new` | empty 64 KiB arena |
| `capacity` | `65536` |
| `used` / `remaining` / `reset` | same semantics |
| `alloc_bytes` / `alloc` / `alloc_str` | same signatures |

---

## Package

| | |
|---|---|
| Package name | `static-arena` |
| Module path | `static-arena/static-arena` |
| Dependencies | `stdlib` (`option`) |
| Tests | `cpc test` (`src/static-arena.cplus`) |
| Sibling | `arena` — growing heap chunks |
