# Reference

Manual for the idiomatic `sqlite` package.

```cplus
import "sqlite/sqlite" as sqlite;
```

Raw symbols: `import "sqlite_ffi/sqlite" as raw`.

---

## Flag / code helpers

```cplus
fn open_readonly() -> i32
fn open_readwrite() -> i32
fn open_create() -> i32
fn open_uri() -> i32
fn open_memory() -> i32      // OPEN_MEMORY flag value
fn open_default_flags() -> i32  // READWRITE|CREATE
```

Result codes used internally: OK `0`, ROW `100`, DONE `101`.

---

## `Error`

```cplus
struct Error {
    code: i32,
    message: text::Text,
}
```

---

## `Step`

```cplus
enum Step { Row, Done }
```

---

## `ColumnType`

```cplus
enum ColumnType {
    Integer, Float, Text, Blob, Null, Unknown,
}
```

---

## `Connection`

```cplus
struct Connection { /* private db handle */ }

fn open(path: str) -> result::Result[Connection, Error]
fn open_with_flags(path: str, flags: i32) -> result::Result[Connection, Error]
fn open_memory() -> result::Result[Connection, Error]

fn prepare(ref this, sql: str) -> result::Result[Statement, Error]
fn execute(ref this, sql: str) -> result::Result[i64, Error]

fn last_insert_rowid(this) -> i64
fn changes(this) -> i64
fn set_busy_timeout(ref this, ms: i32) -> result::Result[bool, Error]

fn raw_handle(this) -> *u8
fn drop(ref this)   // sqlite3_close_v2
```

`execute` returns `changes()` after the statement completes.

---

## `Statement`

```cplus
struct Statement { /* private stmt handle */ }

fn bind_null(ref this, at: i32) -> result::Result[bool, Error]
fn bind_i32(ref this, at: i32, value: i32) -> result::Result[bool, Error]
fn bind_i64(ref this, at: i32, value: i64) -> result::Result[bool, Error]
fn bind_f64(ref this, at: i32, value: f64) -> result::Result[bool, Error]
fn bind_text(ref this, at: i32, value: str) -> result::Result[bool, Error]
fn bind_blob(ref this, at: i32, data: *u8, n: i32) -> result::Result[bool, Error]

fn step(ref this) -> result::Result[Step, Error]
fn reset(ref this) -> result::Result[bool, Error]
fn clear_bindings(ref this) -> result::Result[bool, Error]

fn column_count(this) -> i32
fn column_type(this, at: i32) -> ColumnType
fn column_is_null(this, at: i32) -> bool
fn column_i64(this, at: i32) -> i64
fn column_i32(this, at: i32) -> i32
fn column_f64(this, at: i32) -> f64
fn column_text(this, at: i32) -> option::Option[text::Text]
fn column_blob(this, at: i32) -> option::Option[vec::Vec[u8]]
fn column_name(this, at: i32) -> option::Option[text::Text]

fn raw_handle(this) -> *u8
fn drop(ref this)   // sqlite3_finalize
```

Bind `at` is **1-based**. Column `at` is **0-based**.  
`bind_text` / `bind_blob` copy into SQLite (TRANSIENT).  
`column_text` / `column_blob` return **owned** data.

---

## Package

| | |
|---|---|
| Package name | `sqlite` |
| Module | `sqlite/sqlite` |
| Dependencies | `stdlib`, `sqlite_ffi` |
| Link | via `sqlite_ffi` → `libsqlite3` |
| Tests | `cpc test` |
