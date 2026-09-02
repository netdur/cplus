# Reference

`import "share/share" as sh;`

## Outcome

```cplus
enum Outcome { Ok, Unsupported, InvalidInput, Failed }

fn Outcome::to_code(this) -> i32
fn Outcome::from_code(c: i32) -> Outcome
```

| | meaning |
|---|---|
| `Ok` | the sheet was **presented**. Not that anything was shared. |
| `Unsupported` | no backend, nothing to present from, or `file()` on Android |
| `InvalidInput` | an empty item — a caller bug |
| `Failed` | anything else |

## Verbs

```cplus
fn available() -> bool
fn text(body: str, subject: str = "") -> Outcome
fn url(link: str) -> Outcome
fn file(path: str) -> Outcome
```

**`file`** requires the file to exist and be readable **when the sheet is up**,
not when the call is made — a temporary deleted immediately after this returns
arrives empty.

**`subject`** is carried on Android and dropped on Apple.

## Coverage

| | macOS | iOS | Android |
|---|---|---|---|
| `text` | ✅ | ✅ | ✅ |
| `url` | ✅ `NSURL` | ✅ `NSURL` | ✅ as text |
| `file` | ✅ `fileURLWithPath:` | ✅ | ❌ `Unsupported` |
