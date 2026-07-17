# Guide

Real-time-friendly structures: no heap, no blocking APIs. Fast start:
[tutorial.md](tutorial.md). API: [ref.md](ref.md).

## Why this package exists

Language contracts like `#[no_alloc]` / `#[no_block]` need **libraries** that
actually obey them on hot paths. `rt` ships two building blocks:

1. **`SpscRingU64`** — wait-free control/message queue between two threads  
2. **`FixedPoolU64`** — O(1) slot recycle without malloc  

Neither talks to the OS scheduler or clocks. For Darwin QoS, monotonic time,
and `mlock`, use **`rt_darwin`** (and future `rt_linux`, etc.).

Do not confuse with `import "objc/runtime" as rt` in UI code — that is ObjC
messaging, not this package.

## SPSC ring model

| Role | May call | Owns |
|---|---|---|
| Producer | `push` only | advances `_tail` |
| Consumer | `pop` only | advances `_head` |

- Full `push` → `false` (never blocks, never overwrites).  
- Empty `pop` → `None`.  
- Indices are monotonic; slot = `index % 1024`.  
- Release-store of `_tail` after write; acquire-load of `_tail` in `pop`
  establishes happens-before.

**Violations:** two producers, two consumers, or push+pop on the same thread
without a defined ownership model — memory ordering assumptions break.

Put the ring in **stable shared storage** (`static` or heap that outlives both
threads). Do not move it while in use.

## Fixed pool model

- Free-list of indices; `acquire` → `Option[u32]`, `release(at)`.  
- Free list is LIFO (last released is next acquired).  
- **Not concurrent** — one thread, or lock around acquire/release.  
- **Release-once:** double-release corrupts the free list.  
- `get` / `set` on an index you have not acquired is your bug.

For structs larger than `u64`, store a parallel `[MyThing; 1024]` and put
only the index (or a packed handle) in the pool cell.

## Capacity

Both types hard-code **1024**. Need another size: copy the struct and change
the constants until const-generic structs exist.

## Choosing tools

```
Two threads, control messages / sample indices?
  → SpscRingU64

Many short-lived handles, same thread / locked section?
  → FixedPoolU64

Need heap growth / many owners?
  → not rt (channel, Vec, arena, …)

Need OS priority / clocks / page lock?
  → rt_darwin (or future rt_*)
```

## Gotchas

### `count()` on the ring is a snapshot

Under concurrent push/pop it is approximate for a third observer. Exact for
the participant that only uses its side when the other is idle.

### Pool release order

Tests release `0..1024` after acquiring all — that works because every index
was live. Releasing an never-acquired index is undefined for free-list
integrity.

### `#[no_alloc]` / `#[no_block]`

Hot methods are marked for the real-time checker. Prefer them from RT
callbacks; do not call malloc-heavy code from the same path.

## Typical pattern

```cplus
// audio/control thread boundary
static RING: rt::SpscRingU64 = …;  // or initialize once

// UI / control: RING.push(msg_id)
// audio callback: while let Some(id) = RING.pop() { … }
```
