# Reference

Manual for the `arena` package. Signatures and behavior only.

```cplus
import "arena/arena" as arena;
```

---

## `Chunk`

Internal heap block header. Not constructed by app code.

| Field | Role |
|---|---|
| `_cap` | payload capacity (bytes after header) |
| `_used` | bytes used in payload (including alignment padding) |
| `_next` | previous head (linked list) |

---

## `Arena`

```cplus
struct Arena { /* private: _head, _chunk_size */ }
```

### Free function `new`

```cplus
fn new(chunk_size: usize) -> Arena
```

Empty arena. `chunk_size` is the preferred total chunk size; if smaller
than `size_of(Chunk) + 8`, it is raised to header + 1024 payload.

### `Arena::new`

```cplus
fn new(chunk_size: usize) -> Arena
```

Same as the free function `new`.

### `alloc_bytes`

```cplus
fn alloc_bytes(ref this, count: usize, aligned_to: usize = 8, zeroed: bool = false) -> *u8
```

Bump-allocate `count` bytes. Default alignment 8; `aligned_to == 0` → 1.
May grow a new chunk. Returns **null** on `malloc` failure. If `zeroed`,
the range is `memset` to 0.

### `alloc_bytes_opt`

```cplus
fn alloc_bytes_opt(ref this, count: usize, aligned_to: usize = 8, zeroed: bool = false) -> option::Option[*u8]
```

Same as `alloc_bytes`, but `None` instead of null.

### `alloc`

```cplus
fn alloc[T](ref this, take value: T) -> *T
```

Allocate space for `T` at `align_of(T)`, write `value`. **`T` must be
Copy** (no per-slot Drop). Returns **null** on OOM.

### `alloc_str`

```cplus
fn alloc_str(ref this, s: str) -> option::Option[str]
```

Copy `s` into the arena; return a `str` view. Empty `s` → `Some("")`
without allocating. `None` on OOM. View borrows the arena until reset/Drop.

### `capacity`

```cplus
fn capacity(this) -> usize
```

Sum of all chunk payload capacities (memory held). Not a live used-byte
counter. 0 after `reset` with no subsequent alloc.

### `reset`

```cplus
fn reset(ref this)
```

`free` every chunk; arena empty and reusable. Does not run Drop on
allocated values.

### `drop`

```cplus
fn drop(ref this)
```

Calls `reset`.

---

## Package

| | |
|---|---|
| Package name | `arena` |
| Module path | `arena/arena` |
| Dependencies | `stdlib` (`option`) |
| Tests | `cpc test` (`src/arena.cplus`) |
| Sibling | `static-arena` — fixed in-struct buffer |
