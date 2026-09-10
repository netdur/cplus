# Guide

Fixed buffer bump allocation without the heap. Fast start:
[tutorial.md](tutorial.md). API: [ref.md](ref.md).

## What it is

The buffer is a field of the struct (`[u8; N]`). The allocator is a
cursor (`_used`). There is no `malloc` and no free of individual
allocations — only `reset` to rewind.

Shipped sizes (no const generics yet):

| Type | Capacity |
|---|---|
| `StaticArena16K` | 16 384 bytes |
| `StaticArena64K` | 65 536 bytes |

Same methods on both. Need another size: copy a shape and change `N`
(until const-generic structs exist).

## vs heap `arena`

| | `static-arena` | `arena` |
|---|---|---|
| Backing | in-struct array | heap chunks |
| Growth | never | new chunks |
| Failure | `Option` / `None` | null or `Option` |
| Real-time / no heap | yes | no |

API vocabulary matches: `alloc_bytes`, `alloc[T]`, `alloc_str`, `reset`,
plus `capacity` / `used` / `remaining` on the static shapes.

## Alignment and zeroing

```cplus
a.alloc_bytes(16, aligned_to: 64, zeroed: true)
```

Defaults: `aligned_to: 8`, `zeroed: false`. `aligned_to: 0` → 1 byte
alignment (avoids divide-by-zero in `%`).

Padding for alignment counts toward `used()`.

## OOM / overflow

If the request (after alignment) would pass the end of the buffer:

- return `None`;
- **do not** advance `_used` (failed `alloc_bytes` leaves the cursor).

There is no heap fallback.

## Copy-only `alloc[T]`

`reset` does not run Drop on placed values. Non-Copy types with their own
heap would leak. Only use `alloc[T]` for **Copy** `T`.

## Lifetimes

Returned `*u8` / `*T` / `str` borrow the arena buffer. Invalid after
`reset` or after the arena value is destroyed / moved in ways that
relocate the buffer (treat the arena as pinned while pointers are live).

`alloc_str` copies bytes into the buffer and returns a view into that
copy.

## Stack size ceiling

`new()` returns the arena **by value**. Large structs can be copied
several times in current codegen. Rough practical limit: stick to **16K /
64K** via stack `new()`. Bigger custom sizes should be **`static`** (or
heap `arena`) rather than stack-constructed megabyte arenas.

## Gotchas

### Not thread-safe

One arena, one thread (or external sync).

### `used` includes padding

`used() + remaining() == capacity()` still holds; useful data may be less
than `used()` because of align gaps.

### Sibling package name

Import path uses the hyphenated package name:
`import "static-arena/static-arena" as sa`.

## When to pick which

```
Fixed budget, no heap, small scratch?
  → StaticArena16K / 64K

Unknown size, request-scoped many allocs?
  → arena::Arena

Need Drop / free per object?
  → not an arena (Box / Vec / …)
```
