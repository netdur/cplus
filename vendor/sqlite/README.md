# sqlite

Idiomatic SQLite for C+: `Connection` / `Statement`, `Result` errors, owned
column text, Drop closes and finalizes.

```toml
[dependencies]
sqlite = "*"
```

```cplus
import "sqlite/sqlite" as sqlite;
import "stdlib/result" as result;

guard let result::Result[sqlite::Connection, sqlite::Error]::Ok(db) =
    sqlite::Connection::open_memory() else {
    return;
};
let _n: result::Result[i64, sqlite::Error] =
    db.execute("create table t(id integer primary key, name text)");
```

Raw C ABI (bindgen): package **`sqlite_ffi`** — use only for advanced calls
via `Connection.raw_handle()` / `Statement.raw_handle()`.

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — model, indices, lifetimes
- [docs/ref.md](docs/ref.md) — API manual

## Tests

```
cd vendor/sqlite && cpc test
```
