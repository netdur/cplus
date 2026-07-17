# Reference

Manual for the `rt_darwin` package. Signatures and behavior only.

```cplus
import "rt_darwin/clock" as clock;
import "rt_darwin/thread" as thread;
import "rt_darwin/mem" as mem;
```

---

## Module `clock`

### `now_monotonic_ns`

```cplus
fn now_monotonic_ns() -> u64
```

Current Darwin `CLOCK_MONOTONIC` time in nanoseconds (boot-relative epoch).

### `elapsed_ns`

```cplus
fn elapsed_ns(from: u64, to: u64) -> u64
```

`to -% from`. Labeled params free order. Caller ensures ordering.

---

## Module `thread`

### `Priority`

```cplus
enum Priority {
    RealtimeAudio,   // QOS_CLASS_USER_INTERACTIVE (0x21)
    Interactive,     // QOS_CLASS_USER_INITIATED (0x19)
    Default,         // QOS_CLASS_DEFAULT (0x15)
    Background,      // QOS_CLASS_BACKGROUND (0x09)
}
```

### `set_current_priority`

```cplus
fn set_current_priority(p: Priority) -> result::Result[i32, i32]
```

Set calling thread’s QoS via `pthread_set_qos_class_self_np`. `Ok(0)` on
success; `Err(rc)` on nonzero libc return.

---

## Module `mem`

### `lock_pages`

```cplus
fn lock_pages(at: *u8, len: usize) -> result::Result[i32, i32]
```

`mlock(at, len)`. `Ok(0)` / `Err(rc)`.

### `unlock_pages`

```cplus
fn unlock_pages(at: *u8, len: usize) -> result::Result[i32, i32]
```

`munlock(at, len)`. `Ok(0)` / `Err(rc)`.

---

## Module `rt_darwin`

Umbrella file that imports `clock`, `thread`, and `mem` for package test
discovery. Not required for app imports.

---

## Package

| | |
|---|---|
| Package name | `rt_darwin` |
| Platform | macOS / Darwin |
| Dependencies | `stdlib` (`result`) |
| Tests | `cpc test` |
| Portable sibling | `rt` (SPSC / pool) |
