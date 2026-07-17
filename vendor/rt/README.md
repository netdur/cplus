# rt

Portable **real-time data structures**: allocation-free, non-blocking.

| Module | Type |
|---|---|
| `rt/rt` | `SpscRingU64` — 1024-slot SPSC ring |
| `rt/pool` | `FixedPoolU64` — 1024-slot object pool |

```toml
[dependencies]
rt = "*"
```

```cplus
import "rt/rt" as rt;           // SpscRingU64 (+ pulls pool into the test graph)
import "rt/pool" as pool;       // FixedPoolU64 directly if you prefer

var q: rt::SpscRingU64 = rt::SpscRingU64::new();
assert q.push(42u64);
```

OS knobs (clock, QoS, mlock) live in platform packages such as **`rt_darwin`**,
not here.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — SPSC rules, pool rules, vs platform packages
- [docs/ref.md](docs/ref.md) — API manual

## Tests

Unit tests live in `src/rt.cplus` and `src/pool.cplus` (discovered via `rt`).

```
cd vendor/rt && cpc test
```
