# Reference

`import "filepicker/filepicker" as fp;`

## Outcome

```cplus
enum Outcome { Ok, Unsupported, Unavailable, InvalidInput, Failed }
```

`Ok` means the picker **opened**. `Unavailable` is "nothing to present from" —
no window, no Activity — and is not `Unsupported`.

## Pick

```cplus
#[repr(C)]
struct Pick { path: str }

fn Pick::chose(this) -> bool     // path is non-empty
```

`path` is **borrowed for the dispatch**. Copy it to keep it.

## Verbs

```cplus
fn available() -> bool
fn open(on_pick: fn(Pick, *u8), ctx: *u8 = 0 as *u8, types: str = "") -> Outcome
fn save(suggested: str, on_pick: fn(Pick, *u8), ctx: *u8 = 0 as *u8) -> Outcome
```

The handler runs **on the main thread on every platform**, exactly once,
including on cancel.

## Coverage

| | macOS | iOS | Android |
|---|---|---|---|
| `open` | ✅ | ✅ | ✅ |
| `save` | ✅ | ❌ | ✅ |
| `types` | ✅ | ignored | one MIME family |
| real path | ✅ | ✅ | ❌ `content://` |
