# Tutorial

Quick path: depend, set a level, log a line. Gotchas in [guide.md](guide.md);
signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
log = "*"
```

```cplus
import "log/log" as log;
```

No other package dependencies — output goes to **stderr** via libc `write`.

## Log a message

```cplus
log::info("ready");
log::warn("disk almost full");
log::error("cannot open config");
```

Or the general form:

```cplus
log::log(log::Level::Debug, "payload bytes");
```

Conveniences: `trace`, `debug`, `info`, `warn`, `error` — each takes
`message: str`.

## Filter by level

Default cutoff is **Info** (Trace and Debug are dropped).

```cplus
log::set_max_level(log::Level::Debug);   // emit Debug and above
log::set_max_level(log::Level::Error);   // only Error
```

Skip expensive work when a level is off:

```cplus
if log::is_enabled(log::Level::Debug) {
    log::debug("details...");
}
```

## Color

On by default. Turn off for files or dumb terminals:

```cplus
log::set_uses_color(false);
```

## Day-one rules

- Output is always **stderr** (fd 2), one line per call.
- Message is a plain `str` — no `printf` format args; build the string first.
- Config (`max_level`, color) is **process-global** and not synchronized —
  set it before threads if you share it.
