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

`path` is **borrowed for the dispatch**. Copy it to keep it, or keep
`p.file()`.

```cplus
fn Pick::file(this) -> FileRef   // owned copy, see below
```

## FileRef

A path, or on Android a `content://` URI, that every method reads or writes
either way.

```cplus
struct FileRef { /* owned Text */ }

fn FileRef::of(location: str) -> FileRef
fn location(this) -> str                 // the path or URI, verbatim
fn is_path(this) -> bool                 // false for a URI
fn name(this) -> text::Text              // display name / file name
fn read(this) -> result::Result[vec::Vec[u8], result::IoError]
fn read_text(this) -> result::Result[text::Text, result::IoError]
fn write(this, contents: str) -> result::Result[usize, result::IoError]
fn copy_to(this, dir: str) -> result::Result[text::Text, result::IoError]
```

| Method | Path | Android URI |
|---|---|---|
| `name` | `path::file_name` | The provider's `DISPLAY_NAME`, else the URI's last segment |
| `read` / `read_text` | `stdlib/fs` | `ContentResolver.openInputStream`, through a cache scratch file |
| `write` | `fs::write_string` | `openOutputStream(uri, "wt")`, which replaces the old content |
| `copy_to` | Read, then write `dir/<name>` | Stream copy into `dir/<name>` |

- `read_text` answers `InvalidData` for text that isn't UTF-8.
- An absent URI answers `NotFound`.
- Off Android, a URI answers `InvalidInput`.
- `copy_to` returns the new path and replaces an existing file of that name.
- On Android a URI is readable from the app's entry onward, with no window
  needed. The grant lasts for the process, so don't persist a URI across
  launches.

## Verbs

```cplus
fn available() -> bool
fn open(on_pick: fn(Pick, *u8), ctx: *u8 = 0 as *u8, types: str = "") -> Outcome
fn save(suggested: str, on_pick: fn(Pick, *u8), ctx: *u8 = 0 as *u8) -> Outcome
```

The handler runs exactly once, including on cancel. One chooser at a time: a
second `open` or `save` while one is up answers `Failed`, and the first
handler still runs. (Android and Linux enforce this; Apple and Windows panels
are independent.) Apple, Android, and Windows
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
| `FileRef` read/write | ✅ | ✅ | ✅ through ContentResolver | ✅ | ✅ |
