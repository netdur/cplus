# Guide

How the heap bump arena works, when to use it vs `static-arena`, and the
lifetime / OOM contracts. Fast start: [tutorial.md](tutorial.md). API:
[ref.md](ref.md).

## What it is

`Arena` is an **append-only** allocator: you bump a cursor inside heap
**chunks**; you never free a single allocation. Freeing is whole-arena
(`reset` or Drop), which walks the chunk list and `free`s each block.

Typical uses: parse trees, per-request scratch, compiler IR for one
function — many small objects, one lifetime.

## Arena vs static-arena

| | `arena` | `static-arena` |
|---|---|---|
| Storage | heap chunks via `malloc` | in-struct buffer (stack / `static`) |
| Growth | new chunks as needed | fixed 16 KiB or 64 KiB |
| OOM | null `*u8` / `*T` (or `Option` variants) | always `Option` / `None` |
| `#[no_alloc]` | no | yes (no heap) |
| Import | `arena/arena` | `static-arena/static-arena` |

Same call shape: `alloc_bytes`, `alloc[T]`, `alloc_str`, `reset`.

## Chunks and capacity

`Arena::new(chunk_size)` sets the preferred payload size per chunk.
Very small values are rounded up so the first alloc has room (at least
header + 1 KiB payload).

When the current chunk cannot satisfy alignment + size, a new chunk is
allocated (payload at least large enough for the request) and linked at
the head.

`capacity()` is the **sum of all chunk capacities** — memory held, not a
byte-accurate high-water mark of live useful data. After `reset()`,
capacity is 0 until the next successful alloc.

## Allocation API

```cplus
alloc_bytes(count, aligned_to: 8, zeroed: false) -> *u8
alloc_bytes_opt(...) -> Option[*u8]
alloc[T](value) -> *T          // Copy T
alloc_str(s) -> Option[str]
```

- Default alignment is **8**. `aligned_to: 0` becomes 1 (avoids `% 0` trap).
- `zeroed: true` `memset`s the returned range.
- Failed heap alloc → **null** for raw APIs; use `alloc_bytes_opt` /
  `alloc_str` for `Option`.

## Copy-only `alloc[T]`

The arena does **not** run Drop on individual slots when you `reset`.
Putting a `Vec` or `Text` in the arena would leak their heap on reset.
Use `Box` / own those types outside, or only place **Copy** values with
`alloc[T]`.

## Lifetimes

Every returned pointer and every `str` from `alloc_str` **borrows the
arena**. After `reset` or Drop, they dangle. Do not store them longer than
the arena’s current generation.

## Gotchas

### Null is intentional

Unlike most C+ APIs, raw `alloc_bytes` / `alloc` use a null sentinel so
the hot path stays a bare pointer. Prefer `*_opt` if you want the usual
`Option` style.

### Not a general malloc

No free of one object. Fragmentation is “solved” by reset. Long-lived
mixed lifetimes need a different allocator.

### Alignment padding burns space

Large `aligned_to` values skip bytes in the chunk. That is normal for
bump allocators; size chunks accordingly.

### Multi-thread

Not synchronized. One arena per thread or external locking.

## Typical pattern

```cplus
fn handle_request(body: str) {
    var scratch: arena::Arena = arena::Arena::new(64 * 1024usize);
    // parse / build using scratch.alloc_* …
    // all freed when scratch drops
    return;
}
```
