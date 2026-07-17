# Tutorial

Open a DB, run SQL, prepare + bind + step. Details in [guide.md](guide.md);
signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
sqlite = "*"
```

```cplus
import "sqlite/sqlite" as sqlite;
import "stdlib/option" as option;
import "stdlib/result" as result;
import "stdlib/text" as text;
```

## Open

```cplus
// file (READWRITE|CREATE)
guard let result::Result[sqlite::Connection, sqlite::Error]::Ok(db) =
    sqlite::Connection::open("app.db") else {
    return;
};

// or memory
// Connection::open_memory()
```

## Execute (no rows)

```cplus
match db.execute("create table t(id integer primary key, name text)") {
    result::Result[i64, sqlite::Error]::Ok(_) => {}
    result::Result[i64, sqlite::Error]::Err(e) => {
        // e.code, e.message
        return;
    }
};
```

## Prepare, bind, step

Bind indices are **1-based**. Column indices are **0-based**.

```cplus
guard let result::Result[sqlite::Statement, sqlite::Error]::Ok(st) =
    db.prepare("select id, name from t where id = ?1") else {
    return;
};
let _b: result::Result[bool, sqlite::Error] = st.bind_i64(1, 1);
loop {
    match st.step() {
        result::Result[sqlite::Step, sqlite::Error]::Ok(s) => {
            match s {
                sqlite::Step::Row => {
                    let id: i64 = st.column_i64(0);
                    match st.column_text(1) {
                        option::Option[text::Text]::Some(name) => { /* owned */ }
                        option::Option[text::Text]::None => { /* NULL */ }
                    };
                }
                sqlite::Step::Done => { break; }
            }
        }
        result::Result[sqlite::Step, sqlite::Error]::Err(_) => { return; }
    }
}
// Drop finalizes the statement; Drop closes the connection
```

## Day-one rules

- Prefer **`sqlite`**, not `sqlite_ffi`.
- Check every `Result`; errors carry SQLite code + message `Text`.
- Column text/blob are **owned copies** — safe after the next `step`.
- Do not use the connection after it is dropped while statements still live
  (finalize statements first, or let them drop first in reverse order).
