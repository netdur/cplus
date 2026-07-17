# Guide

How the logger is meant to be used, what “max level” means, and concurrency
limits. Fast start: [tutorial.md](tutorial.md). Signatures: [ref.md](ref.md).

## What you get

Each emitted line looks like:

```text
[2026-07-17 14:30:01] [INFO]  your message
```

| Piece | Source |
|---|---|
| Optional ANSI color | level color escape, then reset after the line |
| Timestamp | local time via `time` / `localtime`, format `[YYYY-MM-DD HH:MM:SS] ` |
| Level tag | fixed width 8, e.g. `[INFO]  `, `[ERROR] ` |
| Message | the `str` you passed, then `\n` |

Destination is always **stderr**. There is no file sink, callback sink, or
structured field API — this is a small diagnostic logger, not a telemetry
pipeline.

## Levels and the cutoff

```cplus
enum Level { Trace, Debug, Info, Warn, Error }
```

Severity ranks ascend with seriousness:

| Level | severity |
|---|---|
| Trace | 0 |
| Debug | 1 |
| Info | 2 |
| Warn | 3 |
| Error | 4 |

A line is emitted when:

```text
level.severity() >= max_level().severity()
```

So `set_max_level(Level::Warn)` means **Warn and Error only**. The name
`max_level` is historical; the comments and behavior mean **minimum severity
that still passes** (the cutoff). Default cutoff is **Info** (`MAX_LEVEL = 2`).

`Level::from_severity(rank)` clamps: below 0 → Trace, above 4 → Error, so a
corrupt stored rank cannot invent a fifth level.

## Config surface

| API | Role |
|---|---|
| `set_max_level(level)` / `max_level()` | cutoff |
| `is_enabled(level)` | would this level print? |
| `set_uses_color(bool)` / `uses_color()` | ANSI on/off |

Use `is_enabled` before building an expensive message string. The conveniences
(`info`, …) already no-op when disabled, but they still need the `str`
argument to exist.

## Color

When `uses_color()` is true, the line is wrapped:

1. level’s ANSI foreground code  
2. timestamp + label + message + newline  
3. reset (`\x1b[0m`)

Colors are fixed per level (trace dim, debug white, info cyan, warn yellow,
error red). There is no theme API. Disable color when redirecting stderr to a
file or when the terminal is not a TTY (the package does not auto-detect TTY).

## Concurrency (important)

`MAX_LEVEL` and `USE_COLORS` are plain mutable module statics. Every `log`
call reads them; `set_*` writes them. **No locks, no atomics.**

Safe patterns:

- Single-threaded app: configure anytime.
- Multi-threaded: call `set_max_level` / `set_uses_color` **once before
  spawning workers**, then only call `log` / conveniences from any thread.

Unsafe: flipping config from one thread while another logs (data race).

Stderr writes themselves are not locked either; concurrent `log` calls can
interleave bytes on the fd. Fine for casual diagnostics; not a guarantee of
atomic lines under heavy parallel logging.

## Performance notes

- Filtering is a severity compare before any I/O.
- Timestamp formatting uses a stack buffer (no malloc per line).
- Message is written as given — no heap copy inside `log`.
- `localtime` is used; that is typically not reentrant. Same concurrency
  caveat as config if many threads log at once on some libcs.

## What this package is not

- Structured key/value logging (JSON lines, spans, trace ids)
- Log rotation, files, syslog, network sinks
- printf-style formatting (`log::info("x=%d", n)` does not exist)
- Per-module or hierarchical loggers
- Async / non-blocking queues

Compose outside: format into a `text::Text`, then `log::info({ t })`, or
route serious events through your own channel.

## Gotchas

### Default hides Trace/Debug

Fresh process: `info` and above only. In tests or local debugging, remember
`set_max_level(Level::Trace)` or you will think `debug` is broken.

### Tests share process-global config

`#[test]` functions that call `set_max_level` leave the cutoff changed for
later tests in the same process. The package tests set levels explicitly;
your suite should too if order matters.

### Message lifetime

`message` is borrowed only for the duration of `log`. Pass a literal or a
live buffer; do not free storage before `log` returns (same as any `str`
borrow).

### No return status

`write` failures are ignored (short write aborts the rest of that line).
Logging is best-effort diagnostics, not a reliable audit trail.

## Typical startup

```cplus
fn init_logging(verbose: bool) {
    if verbose {
        log::set_max_level(log::Level::Debug);
    } else {
        log::set_max_level(log::Level::Info);
    }
    log::set_uses_color(true);   // or false under CI
    log::info("logging ready");
    return;
}
```
