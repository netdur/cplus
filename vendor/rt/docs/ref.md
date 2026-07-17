# Reference

Manual for the `rt` package. Signatures and behavior only.

```cplus
import "rt/rt" as rt;
import "rt/pool" as pool;
```

---

## Module `rt` — `SpscRingU64`

```cplus
struct SpscRingU64 {
    _buf: [u64; 1024],
    _head: u64,   // consumer
    _tail: u64,   // producer
}
```

Fixed 1024-slot single-producer / single-consumer ring. Inline buffer; no heap.

### `SpscRingU64::new`

```cplus
fn new() -> SpscRingU64
```

Empty ring. `#[no_alloc]` `#[no_block]`.

### `capacity`

```cplus
fn capacity(this) -> usize
```

Always `1024`.

### `count`

```cplus
fn count(this) -> usize
```

Published unread elements (`tail - head`). Acquire loads. Snapshot under
concurrency.

### `is_empty` / `is_full`

```cplus
fn is_empty(this) -> bool
fn is_full(this) -> bool
```

Based on `count()`.

### `push`

```cplus
fn push(ref this, v: u64) -> bool
```

Producer only. Writes slot then release-stores `_tail`. Returns `false` if
full (no block, no overwrite).

### `pop`

```cplus
fn pop(ref this) -> option::Option[u64]
```

Consumer only. Acquire-loads `_tail`; release-stores `_head` after read.
`None` if empty.

---

## Module `pool` — `FixedPoolU64`

```cplus
struct FixedPoolU64 {
    _slots: [u64; 1024],
    _links: [u32; 1024],
    _free_head: u32,
    _in_use: usize,
}
```

Fixed 1024-slot free-list pool. Single-owner; no atomics.

### `FixedPoolU64::new`

```cplus
fn new() -> FixedPoolU64
```

All slots free, linked in order. `#[no_alloc]` `#[no_block]`.

### `capacity` / `count` / `available`

```cplus
fn capacity(this) -> usize   // 1024
fn count(this) -> usize      // acquired, not yet released
fn available(this) -> usize  // capacity - count
```

### `is_full` / `is_empty`

```cplus
fn is_full(this) -> bool
fn is_empty(this) -> bool
```

### `acquire`

```cplus
fn acquire(ref this) -> option::Option[u32]
```

Next free slot index, or `None` if exhausted.

### `release`

```cplus
fn release(ref this, at: u32)
```

Return slot `at` to the free list (LIFO). Caller must release each acquired
slot **once**.

### `get` / `set`

```cplus
fn get(this, at: u32) -> u64
fn set(ref this, value: u64, at: u32)
```

Payload in an acquired slot. Labeled `at:` / `value:` free order on `set`.

### `pool_none` (internal)

```cplus
fn pool_none() -> u32
```

Free-list sentinel (`u32` max). Not part of normal app API.

---

## Package

| | |
|---|---|
| Package name | `rt` |
| Modules | `rt/rt`, `rt/pool` |
| Dependencies | `stdlib` (`atomic`, `option`) |
| Tests | `cpc test` |
| Sibling OS packages | `rt_darwin` (clock, QoS, mlock) |
