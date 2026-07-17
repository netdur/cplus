# rt_darwin

Darwin (macOS) **real-time platform controls**: monotonic clock, thread QoS,
page locking.

```toml
[dependencies]
rt_darwin = "*"
```

```cplus
import "rt_darwin/clock" as clock;
import "rt_darwin/thread" as thread;
import "rt_darwin/mem" as mem;

let t0: u64 = clock::now_monotonic_ns();
let _r: result::Result[i32, i32] = thread::set_current_priority(thread::Priority::RealtimeAudio);
```

Portable structures (SPSC ring, pool) live in **`rt`**, not here.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — why per-OS packages, contracts
- [docs/ref.md](docs/ref.md) — API manual

## Tests

Unit tests in `src/clock.cplus`, `src/thread.cplus`, `src/mem.cplus`
(umbrella: `src/rt_darwin.cplus`).

```
cd vendor/rt_darwin && cpc test
```
