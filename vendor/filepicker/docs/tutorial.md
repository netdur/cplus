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

## 5. Read what came back

```cplus
fn picked(p: fp::Pick, ctx: *u8) {
    if !p.chose() { return; }
    // macOS, iOS, Linux, and Windows: a real path.
    // Android: a content:// URI — use ContentResolver, not fs::open_read.
}
```

## 6. Copy it if you keep it

`path` is borrowed for the dispatch, like every handler string in this repo.
Storing it is a use-after-free; copy it into a `Text` first.
