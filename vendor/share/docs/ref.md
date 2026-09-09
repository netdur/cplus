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
| `Unsupported` | no backend, nothing to present from (no window on Windows), or `file()` on Android or Windows |
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

| | macOS | iOS | Android | Windows |
|---|---|---|---|---|
| `text` | ✅ | ✅ | ✅ | ✅ `DataPackage::SetText` |
| `url` | ✅ `NSURL` | ✅ `NSURL` | ✅ as text | ✅ `SetUri`, a real `Uri` object |
| `file` | ✅ `fileURLWithPath:` | ✅ | ❌ `Unsupported` | ❌ `Unsupported` |

Windows goes through WinRT's `DataTransferManager`, reached with
`RoGetActivationFactory` and driven by vtable index — no projection, no WinRT
toolchain, and nothing added to the link line (combase is bound at runtime).
It needs **a window and a running message loop**: the sheet asks for the payload
later, on that loop. A console process with neither answers `Unsupported`.

`file` on Windows is a gap rather than a wall: `SetStorageItems` wants an
`IVectorView<IStorageItem>` and the only route from a path to a `StorageFile` is
the async `GetFileFromPathAsync`. Both are real work and neither is built.
