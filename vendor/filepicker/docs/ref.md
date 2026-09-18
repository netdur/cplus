# Reference

Fast start: [tutorial.md](tutorial.md). Platform behavior and gotchas:
[guide.md](guide.md).

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

The handler runs exactly once, including on cancel. Apple, Android, and Windows
deliver on their UI thread. Linux delivers through the calling thread's default
GLib main context, which is the UI thread in a facet GTK app; a headless caller
must run that context.

## Coverage

| | macOS | iOS | Android | Linux | Windows |
|---|---|---|---|---|---|
| `open` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `save` | ✅ | ❌ | ✅ | ✅ | ✅ |
| `types` | ✅ | ignored | one MIME family | extension globs | **not wired yet** |
| real path | ✅ | ✅ | ❌ `content://` | ✅ decoded from `file://` | ✅ |
