# Tutorial

Quick path: time a span, set QoS, lock pages. Gotchas in
[guide.md](guide.md); signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
rt_darwin = "*"
stdlib = "*"
```

```cplus
import "rt_darwin/clock" as clock;
import "rt_darwin/thread" as thread;
import "rt_darwin/mem" as mem;
import "stdlib/result" as result;
```

## Clock

```cplus
let start: u64 = clock::now_monotonic_ns();
// … work …
let end: u64 = clock::now_monotonic_ns();
let dt: u64 = clock::elapsed_ns(from: start, to: end);
```

Monotonic nanoseconds since an arbitrary boot epoch (not wall clock).

## Thread priority (QoS)

```cplus
match thread::set_current_priority(thread::Priority::RealtimeAudio) {
    result::Result[i32, i32]::Ok(_) => {}
    result::Result[i32, i32]::Err(rc) => { /* libc return code */ }
}
```

Tiers: `RealtimeAudio`, `Interactive`, `Default`, `Background`.

## Page lock

```cplus
var buf: [u8; 256] = [0u8; 256];
match mem::lock_pages(at: #addr_of(buf[0]), len: 256usize) {
    result::Result[i32, i32]::Ok(_) => {
        let _u: result::Result[i32, i32] =
            mem::unlock_pages(at: #addr_of(buf[0]), len: 256usize);
    }
    result::Result[i32, i32]::Err(_) => { /* rlimit / EPERM / … */ }
}
```

## Day-one rules

- All fallible ops return **`Result`** — no traps.  
- macOS-only constants/syscalls; Linux needs a sibling package.  
- Pair with **`rt`** for SPSC/pool, not for clocks/QoS.  
- `mlock` may fail under sandbox or `RLIMIT_MEMLOCK`.
