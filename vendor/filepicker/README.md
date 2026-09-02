# filepicker

The system's own file chooser.

```toml
[dependencies]
filepicker = "*"
```

```cplus
import "filepicker/filepicker" as fp;

fn picked(p: fp::Pick, ctx: *u8) {
    if !p.chose() { return; }        // they cancelled — an answer, not an error
    open_it(p.path);
}

let _o: fp::Outcome = fp::open(picked, types: "png,jpg");
```

## Asynchronous everywhere

Every platform hands the screen to another process and answers later, so there
is no blocking form — not even on macOS, where `runModal` exists. A modal run
loop inside a facet app is a reentrancy problem, not a convenience.

The return value is **whether the picker opened**. The choice arrives on the
handler.

## A cancel is an answer

The handler runs either way, with an empty `path`. `chose()` is the check.

## `path` is not always a path

On Android it is a `content://` URI from the Storage Access Framework — opaque,
provider-owned, and not something `fs::open_read` can take. Read it with
`ContentResolver.openInputStream`. Handing back an invented filesystem path
would be a lie.

## Coverage

| | macOS | iOS | Android |
|---|---|---|---|
| `open` | ✅ | ✅ | ✅ |
| `save` | ✅ | ❌ no such picker | ✅ `CREATE_DOCUMENT` |
| `types` filter | ✅ extensions | ❌ ignored | ✅ one MIME family |
| `path` is a real path | ✅ | ✅ | ❌ `content://` URI |

- [tutorial](docs/tutorial.md) · [guide](docs/guide.md) · [ref](docs/ref.md)

## Tests

    cd vendor/filepicker && cpc test

A picker cannot be asserted.
