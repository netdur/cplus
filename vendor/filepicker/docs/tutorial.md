# Tutorial

Quick path: open or save with the system chooser. Platform behavior and
gotchas: [guide.md](guide.md). Signatures: [ref.md](ref.md).

## 1. Depend on it

```toml
[dependencies]
filepicker = "*"
```

Use `cpc pm add . filepicker` to write the platform-specific dependency
closure.

## 2. Open something

```cplus
import "filepicker/filepicker" as fp;

fn picked(p: fp::Pick, ctx: *u8) {
    if !p.chose() { return; }
    show(p.path);
}

let _o: fp::Outcome = fp::open(picked);
```

## 3. Filter, loosely

```cplus
fp::open(picked, types: "png,jpg");
```

Honoured as extensions on macOS and as portal glob filters on Linux, mapped to
one MIME family on Android, and **ignored on iOS and Windows**. See the
[guide](guide.md#the-types-filter-is-lossy-on-purpose).

## 4. Save

```cplus
fp::save("report.pdf", picked);
```

`Unsupported` on iOS, which has no such picker.

## 5. Read or write what came back

```cplus
fn picked(p: fp::Pick, ctx: *u8) {
    if !p.chose() { return; }
    let f: fp::FileRef = p.file();
    match f.read_text() {
        result::Result[text::Text, result::IoError]::Ok(body) => { show(body.view()); }
        result::Result[text::Text, result::IoError]::Err(e) => {}
    }
}

fn save_to(p: fp::Pick, ctx: *u8) {
    if !p.chose() { return; }
    let _w = p.file().write("date,amount\n");
}
```

`p.path` is a real path on macOS, iOS, Linux and Windows, and a `content://`
URI on Android, which `fs` can't open. A `FileRef` reads and writes either.
When an API only takes a path, use `f.copy_to(dir)` to get a real file.

## 6. Keep the FileRef, not the path

`path` is borrowed for the dispatch, like every handler string in this repo.
Storing it is a use-after-free. `p.file()` copies it, so keep the `FileRef`.
