# Guide

Darwin OS knobs for soft real-time work. Fast start: [tutorial.md](tutorial.md).
API: [ref.md](ref.md).

## Why a separate package

Real-time **platform** APIs differ by OS (`CLOCK_MONOTONIC` id, QoS vs
`sched_setscheduler`, `mlock` limits). The project keeps them in
`rt_<os>` packages rather than core language or a fake “portable” shim
that lies about constants.

| Package | Role |
|---|---|
| **`rt`** | Portable data structures (SPSC, pool) |
| **`rt_darwin`** | clock, thread QoS, mlock on macOS |
| future `rt_linux` | same *jobs*, Linux APIs |

## Modules

| Import | Purpose |
|---|---|
| `rt_darwin/clock` | monotonic ns timestamps, elapsed |
| `rt_darwin/thread` | `set_current_priority(Priority)` |
| `rt_darwin/mem` | `lock_pages` / `unlock_pages` |
| `rt_darwin/rt_darwin` | umbrella imports for `cpc test` only |

App code should import `clock` / `thread` / `mem` directly.

## Clock

- `now_monotonic_ns()` uses Darwin `CLOCK_MONOTONIC` (**id 6**, not Linux’s 1).  
- Suitable for latency measurement on a hot path (cheap, non-decreasing).  
- Not wall-clock time; not NTP-adjusted.  
- `elapsed_ns(from:, to:)` is wrapping subtract; pass ordered timestamps.

## Thread QoS

Maps intent to Darwin `qos_class_t`:

| `Priority` | Approx role |
|---|---|
| `RealtimeAudio` | user-interactive tier — closest soft-RT / audio-style |
| `Interactive` | user-initiated work |
| `Default` | default class |
| `Background` | may be throttled |

Uses `pthread_set_qos_class_self_np` on the **calling** thread. Interactive
tiers normally need no special privileges. This is **not** hard real-time
or time-critical FIFO priority with root.

## Memory lock

`mlock` / `munlock` on `[at, at+len)` so the range should not page-fault
later. Failures (`EPERM`, `ENOMEM`, rlimits) surface as `Err(rc)` with the
libc return code (errno not yet wrapped into a richer type).

Lock only what the hot path touches; unlock when done. Zero-length and
unlock-without-lock are exercised as non-trapping edge cases in tests.

## Error model

```cplus
Result[i32, i32]   // Ok(0) success; Err(nonzero) libc-style code
```

Never traps on failure. Check results on startup paths that must guarantee
locked memory or elevated QoS.

## Gotchas

### Not portable by import path

Do not `import "rt_darwin/..."` on Linux and expect it to build. Gate by
target or use a facade package later.

### QoS is best-effort soft RT

The kernel still schedules other work. Pair with buffer sizes and
`#[no_alloc]` callbacks; do not assume microsecond guarantees.

### `lock_pages` address

Pass a real pointer into mapped memory (`#addr_of` of a buffer), not a
fabricated integer.

### Tests may see `Err` on mlock

Sandboxes and low `RLIMIT_MEMLOCK` make lock failure acceptable in tests;
production should treat `Err` as a real config problem if RT requires it.

## Typical startup

```cplus
fn prepare_audio_thread() {
    let _q: result::Result[i32, i32] =
        thread::set_current_priority(thread::Priority::RealtimeAudio);
    // optionally mlock DSP state buffers
    return;
}
```
