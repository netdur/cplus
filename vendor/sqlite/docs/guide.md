# Guide

How the idiomatic layer relates to raw SQLite, and the rules that keep you
safe. Tutorial: [tutorial.md](tutorial.md). API: [ref.md](ref.md).

## Two packages

| Package | Role |
|---|---|
| **`sqlite`** | App API — this package |
| **`sqlite_ffi`** | Auto-generated C ABI (`sqlite3_*` as `*u8` / `*i8`) |

`sqlite` depends on `sqlite_ffi` and links `libsqlite3` through it. Escape
hatch: `raw_handle()` on connection/statement for rare gen APIs.

## Types

| Type | Role |
|---|---|
| `Connection` | open DB; prepare / execute; Drop → `sqlite3_close_v2` |
| `Statement` | bind / step / columns; Drop → `sqlite3_finalize` |
| `Error` | `code: i32` + `message: Text` |
| `Step` | `Row` \| `Done` |
| `ColumnType` | Integer / Float / Text / Blob / Null / Unknown |

## Opening

- `open(path)` → `READWRITE | CREATE`
- `open_with_flags(path, flags)` — use `open_readonly()`, `open_readwrite()`,
  `open_create()`, `open_uri()`, `open_memory()` flag helpers
- `open_memory()` → `":memory:"`

Paths and SQL are C+ `str`; interior NUL is rejected as an error.

## Statements

1. `prepare(sql)`  
2. `bind_*` (1-based indices)  
3. `step` until `Done` (or stop after handling rows)  
4. optional `reset` / `clear_bindings` to reuse  
5. Drop finalizes  

`execute(sql)` is prepare + step-to-done for non-query SQL; returns
`changes()` as `i64`.

## Bind and columns

| Bind | Column read |
|---|---|
| `bind_null` | `column_is_null` |
| `bind_i32` / `bind_i64` | `column_i32` / `column_i64` |
| `bind_f64` | `column_f64` |
| `bind_text` (copied into SQLite) | `column_text` → owned `Text` |
| `bind_blob` | `column_blob` → owned `Vec[u8]` |

Text bind uses SQLite **TRANSIENT** (copy), so the `str` need only live
through the bind call.

## Errors

All fallible ops return `Result[…, Error]`. Message comes from
`sqlite3_errmsg` / `errstr`. No traps on SQLITE_ERROR.

## What is not in v1

- Named parameters beyond `?1` (raw still works if you prepare with names)
- Transaction helpers (`begin`/`commit` as methods) — use `execute("begin")`
- Connection pooling, async, ORM / typed rows
- Full surface of backup, blob streaming, hooks, FTS — use `sqlite_ffi`

## Gotchas

### Statement vs connection order

Finalize statements before destroying the connection (or rely on Drop order:
inner statements drop before outer connection if nested correctly).

### `changes()` after execute

Counts rows modified by the last completed statement on that connection.

### Flags are raw i32

Helpers return SQLite OPEN_* values; combine with `+%` (or whatever bit-or
you use) carefully.

### Not thread-safe by default

Same as SQLite serialized mode defaults; multi-thread needs care and
possibly `sqlite_ffi` config APIs.
