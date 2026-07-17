# Tutorial

Quick path: SPSC ring and fixed pool. Gotchas in [guide.md](guide.md);
signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
rt = "*"
```

```cplus
import "rt/rt" as rt;
import "rt/pool" as pool;
import "stdlib/option" as option;
```

## SPSC ring (`SpscRingU64`)

One producer, one consumer; 1024 `u64` slots; no malloc, no block.

```cplus
var q: rt::SpscRingU64 = rt::SpscRingU64::new();

// producer
if !q.push(7u64) {
    // full — drop, coalesce, or try later
}

// consumer
match q.pop() {
    option::Option[u64]::Some(v) => { /* v */ }
    option::Option[u64]::None => { /* empty */ }
}
```

## Fixed pool (`FixedPoolU64`)

Recycle up to 1024 slots of `u64` (or indices into your own arrays).

```cplus
var p: pool::FixedPoolU64 = pool::FixedPoolU64::new();

guard let option::Option[u32]::Some(slot) = p.acquire() else {
    return;   // pool full
};
p.set(100u64, at: slot);
let v: u64 = p.get(at: slot);
p.release(slot);   // release once; do not double-release
```

## Day-one rules

- Ring: **exactly one** pusher thread and **one** popper thread.
- Pool: **single owner** (not atomic) unless you add external locking.
- Capacity is **1024** until const-generic sizes exist.
- Payloads are `u64` — use as handles/indices for richer types.
