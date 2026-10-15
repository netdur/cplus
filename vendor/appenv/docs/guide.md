# Guide

Use app storage for persistent private files, cache for data you can recreate,
documents for user-oriented files, and temporary storage for short-lived work.
For setup see [tutorial.md](tutorial.md); signatures are in [ref.md](ref.md).

## Platform locations

`<ns>` is an explicit desktop namespace or the native app ID.
Mobile roots already belong to the current app; an explicit namespace adds
one child beneath the data/cache root.

| Lookup | iOS | Android | macOS | Linux | Windows |
|---|---|---|---|---|---|
| `current_dir` | Process CWD | Process CWD | Process CWD | Process CWD | Win32 CWD |
| `app_path` | Main bundle | Base APK | Main `.app`, or executable dir for a CLI | Executable dir | Executable dir |
| `resource_dir` | Main bundle resources | Unavailable | Bundle resources, or executable dir for a CLI | Executable dir | Executable dir |
| `home_dir` | Data container | `Context.getDataDir()` | Foundation home | Absolute `$HOME` | Profile known folder |
| `temp_dir` | Foundation temp | Internal cache | Foundation temp | Absolute `$TMPDIR`, else `/tmp` | `GetTempPathW()` |
| `data_dir` | Application Support | Internal files | Application Support / `<ns>` | XDG data root / `<ns>` | LocalAppData / `<ns>` |
| `cache_dir` | Library/Caches | Internal cache | Library/Caches / `<ns>` | XDG cache root / `<ns>` | LocalAppData / `<ns>` / Cache |
| `documents_dir` | Container Documents | Internal files / Documents | Foundation Documents | xdg-user-dirs Documents, else `$HOME/Documents` | Documents known folder |
| `downloads_dir` | Foundation container Downloads | App-specific external Download | Foundation Downloads | xdg-user-dirs Downloads, else `$HOME/Downloads` | Downloads known folder |
| `library_dir` | Container Library | Unavailable | Foundation Library | Unavailable | Unavailable |
| `external_data_dir` | Unavailable | App-specific external files | Unavailable | Unavailable | Unavailable |
| `external_cache_dir` | Unavailable | App-specific external cache | Unavailable | Unavailable | Unavailable |
| `no_backup_dir` | `data_dir` / NoBackup | `Context.getNoBackupFilesDir()` | `data_dir` / NoBackup | `data_dir` / NoBackup | `data_dir` / NoBackup |

Linux honors absolute `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `XDG_CONFIG_HOME`;
empty or relative values fall back to the XDG defaults. Documents and Downloads honor the
quoted absolute or `$HOME` form in `user-dirs.dirs`; the file is parsed as data,
never executed. Windows known folders honor folder redirection and return UTF-8.
The Windows storage backend uses known folders for both packaged and unpackaged
apps; it doesn't expose MSIX `ApplicationData`. Android external directories are
app-specific, require no storage permission on the supported API 24+ targets,
and can be absent when storage is unavailable. They do not grant access to shared
public Downloads; use a picker or MediaStore for shared files.

## Keeping files out of backups

`data_dir` is backed up: iCloud/device backup on iOS, Android Auto Backup,
Time Machine on macOS. Keep what must persist but not travel — a downloaded
model, a device-bound token, a large local index — in `no_backup_dir`:

```cplus
fn model_dir() -> option::Option[text::Text] {
    match appenv::no_backup_dir() {
        result::Result::Ok(dir) => {
            match fs::create_dir_all(dir.view()) {
                result::Result::Ok(made) => {}
                result::Result::Err(e) => { return option::Option[text::Text]::None; }
            }
            match appenv::exclude_from_backup(dir.view()) {
                result::Result::Ok(done) => { return option::Option[text::Text]::Some(dir); }
                result::Result::Err(e) => { return option::Option[text::Text]::None; }
            }
        }
        result::Result::Err(e) => { return option::Option[text::Text]::None; }
    }
}
```

Android's no-backup directory is excluded by the OS, so `exclude_from_backup`
only confirms it. On Apple the directory is a plain child of `data_dir` until
`exclude_from_backup` sets `NSURLIsExcludedFromBackupKey` on it, which covers
everything inside. Linux and Windows have no OS backup of app storage, so the
call answers `Ok`. Android can't exclude an arbitrary path at run time; its
backup rules live in the manifest, so a path outside the no-backup and cache
directories answers `Unavailable`.

## Pointing an app at throwaway storage

`APPENV_ROOT=/abs/dir` in the environment, or `appenv::set_root("/abs/dir")`,
moves every place the app writes to `<root>/<kind>`: `data`, `cache`, `temp`,
`home`, `documents`, `downloads`, `no_backup`, and `library` /
`external_data` / `external_cache` where the platform has one. A namespace
still adds its child. Use it for tests, CI, and an agent driving the app
without touching real data.

`set_root` outranks the environment, and `set_root("")` lifts it. Call it
before another thread asks for a location. A relative root is refused:
`set_root` returns `InvalidInput`, and a relative `APPENV_ROOT` makes every
storage lookup return `InvalidInput` rather than fall back to the real
location. The override doesn't move where the app is (`current_dir`,
executable, `app_path`, `resource_dir`, assets) or change its identity.

On the iOS simulator, pass the variable as `SIMCTL_CHILD_APPENV_ROOT` to
`xcrun simctl launch`. An Android app gets no environment from `am start`,
so call `set_root` there.

## Path lifetime and filesystem behavior

Each result owns its `Text`. No result borrows an Objective-C autorelease pool,
Java local reference or temporary native buffer. Getters never call `chdir` and
never create a namespace or Documents child. Android's native Context APIs may
initialize their standard storage roots. A returned path need not exist yet;
use `stdlib/fs::create_dir_all` before writing and handle its `IoError`.

### Gotcha: the working directory isn't app storage

Relative filenames resolve against the process CWD, which the launcher controls.
Resolve persistent files from `data_dir()` and return or pass their full paths;
don't change the process CWD to make relative paths work.

### Gotcha: app resources and executables have different locations

Join a filename onto a returned directory with `stdlib/path::join`, which
writes the target's separator and never doubles one.
Paths are plain filenames rather than URLs. `executable_path()` is the process
executable; on Android this is typically `app_process`, not your APK or shared
library. `app_path()` answers the installed app location instead. A base APK
path doesn't enumerate split APKs and isn't a directory to append filenames to.

### Gotcha: container paths and temporary files aren't stable

Apple container paths can change across app installs. Resolve them at runtime
and store relative names, rather than persisting an absolute container path.
Getters don't promise a path is writable; app bundles are normally read-only,
and cache/temp contents can be removed by the OS. Android documents here are
private app files; shared user documents require a picker and content URI.

## Packaged assets

Use `read_asset` for bytes, `read_asset_text` for UTF-8 text, and `asset_path`
when a consumer (such as a GGUF or ONNX runtime) requires a real filesystem path
to `mmap` or open directly.

The end-to-end workflow for bundled assets is:

1. **Create `assets/` in the project root** (e.g. `assets/defaults/settings.json` or `assets/models/model.gguf`).
2. **Package the app for the target platform:**
   - **macOS (`.app`):** `cpc package` automatically copies `./assets/*` into `<Name>.app/Contents/Resources/` before signing.
   - **iOS (`.app`):** Copy `./assets/*` into the `.app` bundle root (or add `assets` as a folder reference in Xcode).
   - **Android (`.apk`):** Pass `-A assets` to `aapt2 link` when assembling the APK.
   - **Linux / Windows / desktop CLI:** Place the files beside the executable.
3. **Resolve or read by relative name** (`"defaults/settings.json"`, `"models/model.gguf"`). Names are case-sensitive on Android; use exact casing for portability.
4. **Pass the returned value or path to the consumer.**

`read_asset` and `read_asset_text` own their output and close native handles
before returning. Compressed APK entries work through `AAsset_read`; no asset
path is fabricated and no copy into writable storage is required. Because
`read_asset` holds the whole asset in memory, use `asset_path` for large models
and media: on Apple and desktop `asset_path` returns the in-bundle file path
directly without copying, and on Android it streams the APK asset once in 64 KiB
chunks into `no_backup_dir()/assets/<name>` (atomic write-then-rename) and
reuses the file on disk while its size matches the APK entry and its mtime is
at least as new as the installed APK. Empty assets succeed, binary bytes are
unchanged, and the text reader rejects malformed UTF-8. Error cases are in
[ref.md](ref.md#error).

### Gotcha: relative asset names aren't arbitrary filesystem paths

Names use forward slashes and reject absolute paths, dot components, empty
components, backslashes, control/NUL bytes and Windows filename/device aliases.
Unicode and internal spaces are accepted; trailing dots/spaces aren't. Desktop
resource files are trusted application resources: a symlink placed there can
point outside the resource root. This reader is not a sandbox for untrusted
filesystem trees.

## Identity and availability

Apple reads localized `CFBundleDisplayName` (fallback `CFBundleName`),
`CFBundleIdentifier`, `CFBundleShortVersionString` and `CFBundleVersion`.
Android uses its Context and PackageManager, with a localized application label
and the full long version code on API 28+ (legacy version code on API 24–27).
Labels are copied as standard UTF-8, including supplementary Unicode characters.
Windows uses `GetCurrentPackageId` for MSIX package name, four-part version and
revision/build. Unpackaged apps can supply an explicit process AppUserModelID;
name comes from the executable's VERSIONINFO ProductName, and fixed file version
and revision are fallbacks for version/build. These fields require the application
to package the corresponding native identity/resources.

Linux consults an existing default GApplication in an already-loaded GIO library,
without loading a UI stack. It reads its ID, GLib application name and, on GIO
2.80+, GApplication version. GLib's name may fall back to its program name; Linux
has no standard build field. Call on the application thread while that object is
alive. Non-GApplication programs have no value here.

Missing/empty metadata returns `None`. A compiler manifest isn't runtime installed
identity. Ordinary command-line tools can therefore still have no ID.

Without native desktop identity, data/cache lookups with no namespace answer
`Error::Unavailable`. Supply a stable name such as `com.example.notes`.
Namespaces are one component of 1–255 ASCII letters, digits, dots, underscores
or hyphens. Leading/trailing dots and Windows reserved device basenames are
rejected with `Error::InvalidInput` on every platform, including before a host
exists. This keeps the component portable and prevents traversal.

### Android: from the app's first line, on any thread

Android reads the JavaVM and the Application context that Facet publishes
through `facet/app_events` before the app's entry runs. So a lookup answers
from the first line of `run()`, before `App::new` and before any window, and
from any thread: a thread the JVM never called into is attached for the call
and detached after. The Activity (`app_events::host()`) is only a fallback for
a host that publishes no Application context. Code that runs before Facet has
started (no VM published) gets `Error::Unavailable`, as do an unsupported
resource-directory lookup and a failed native query; metadata yields `None`.
JNI references are local to each call. Existing pending JNI exceptions are
preserved, and exceptions raised by a lookup are cleared before returning. No
storage permission is needed for these internal or app-specific external
paths. Asset reads follow the same rules.

This package supports macOS, iOS, Android, Linux and Windows. ESP32 and browser
Wasm don't supply this application filesystem/identity model.

Native contracts: [Apple filesystem guidance](https://developer.apple.com/documentation/foundation/using-the-file-system-effectively),
[Android Context](https://developer.android.com/reference/android/content/Context),
[XDG base directories](https://specifications.freedesktop.org/basedir/latest/),
[Windows known folders](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath),
[Android native assets](https://developer.android.com/ndk/reference/group/asset),
[Windows package identity](https://learn.microsoft.com/en-us/windows/win32/api/appmodel/nf-appmodel-getcurrentpackageid),
[Windows VERSIONINFO](https://learn.microsoft.com/en-us/windows/win32/api/winver/nf-winver-verqueryvaluew),
[GApplication identity](https://docs.gtk.org/gio/method.Application.get_application_id.html),
[GApplication version](https://docs.gtk.org/gio/method.Application.get_version.html).
