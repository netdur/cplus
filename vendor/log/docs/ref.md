# Reference

Manual for the `log` package. Signatures and behavior only.

```cplus
import "log/log" as log;
```

Process-global leveled logger writing to stderr. No package dependencies.

---

## `Level`

```cplus
enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
```

### `severity`

```cplus
fn severity(this) -> i32
```

Numeric rank: Trace `0`, Debug `1`, Info `2`, Warn `3`, Error `4`. Higher
means more severe. Emission requires `severity() >=` configured cutoff.

### `from_severity`

```cplus
fn from_severity(rank: i32) -> Level
```

Map a rank to a level. Ranks `< 0` clamp to `Trace`; ranks `> 4` clamp to
`Error`. Always returns a valid level.

### `label`

```cplus
fn label(this) -> str
```

Fixed-width tag written before the message (length 8), e.g. `"[INFO]  "`,
`"[ERROR] "`.

### `color`

```cplus
fn color(this) -> str
```

ANSI SGR foreground escape for this level (starts with ESC `0x1b`). Used
internally when color is enabled; safe to call for inspection.

---

## Configuration

Stored in module statics. Not synchronized across threads.

### `set_max_level`

```cplus
fn set_max_level(level: Level)
```

Set the emission cutoff: only levels with `severity() >= level.severity()`
are printed. Default at process start is `Info`.

### `max_level`

```cplus
fn max_level() -> Level
```

Current cutoff (reconstructed from the stored severity rank).

### `is_enabled`

```cplus
fn is_enabled(level: Level) -> bool
```

`true` if a message at `level` would be emitted under the current cutoff.

### `set_uses_color`

```cplus
fn set_uses_color(enabled: bool)
```

Enable or disable ANSI color around each log line. Default is `true`.

### `uses_color`

```cplus
fn uses_color() -> bool
```

Whether color escapes are currently enabled.

---

## Emit

### `log`

```cplus
fn log(level: Level, message: str)
```

If `is_enabled(level)`, write to stderr (fd `2`):

1. optional color escape  
2. local timestamp `[YYYY-MM-DD HH:MM:SS] `  
3. `level.label()`  
4. `message`  
5. newline  
6. optional ANSI reset  

If disabled, returns immediately with no I/O. `message` is borrowed for the
call only. Write errors are ignored.

### Conveniences

Each forwards to `log` with a fixed level:

```cplus
fn trace(message: str)
fn debug(message: str)
fn info(message: str)
fn warn(message: str)
fn error(message: str)
```

---

## Package

| | |
|---|---|
| Package name | `log` |
| Module path | `log/log` |
| Dependencies | none (libc: `write`, `time`, `localtime`, `snprintf`) |
| Output | stderr only |
| Default cutoff | `Level::Info` |
| Default color | on |
| Tests | `cpc test` (in `src/log.cplus`) |
