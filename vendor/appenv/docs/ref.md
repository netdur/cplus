# Reference

Manual for `appenv`. Signatures and behavior only. Setup is in
[tutorial.md](tutorial.md); ownership and platform contracts are in
[guide.md](guide.md).

```cplus
import "appenv/appenv" as appenv;
```

## Error

```cplus
enum Error {
    Unavailable,
    InvalidInput,
    NotFound,
    ReadFailed,
    InvalidData,
    OutOfMemory,
}
```

| Case | Meaning |
|---|---|
| `Unavailable` | Facet has not started (Android) or installed identity is missing, the lookup is unsupported, or a native query failed. |
| `InvalidInput` | A storage namespace or relative asset name is invalid. |
| `NotFound` | The packaged asset could not be opened because it is absent (Android also uses this for any failed native asset open). |
| `ReadFailed` | The asset could not be read, including access or native I/O failures. |
| `InvalidData` | An asset requested as text is not valid UTF-8. |
| `OutOfMemory` | An asset buffer or native filename allocation failed. |

Native OS error codes aren't exposed. Subsequent filesystem operations report
their own `stdlib/result::IoError`.

## Paths

All successful lookups return owned UTF-8 `Text` with an absolute native path.
Getters leave CWD unchanged and don't create custom directories. A location may
not exist yet; Android's Context APIs may initialize their standard roots.

### `current_dir`

```cplus
fn current_dir() -> result::Result[text::Text, Error]
```

Returns the process working directory at the time of the call. Returns
`Unavailable` if the native query fails, including when CWD has been deleted.

### `executable_path`

```cplus
fn executable_path() -> result::Result[text::Text, Error]
```

Returns the running process's executable filename through `stdlib/bundle`.
On Android this identifies the process executable rather than the APK or your
shared library. Returns `Unavailable` when the executable can't be located.

### `executable_dir`

```cplus
fn executable_dir() -> result::Result[text::Text, Error]
```

Returns the directory containing the process executable. Returns `Unavailable`
when its path can't be located.

### `app_path`

```cplus
fn app_path() -> result::Result[text::Text, Error]
```

Returns the main Apple app bundle directory, the Android base APK filename,
or the executable directory for desktop tools without a bundle. The Android
answer is a file and doesn't enumerate split APKs. Returns `Unavailable` when
the native app location can't be queried, including before Facet starts on Android.

### `resource_dir`

```cplus
fn resource_dir() -> result::Result[text::Text, Error]
```

Returns the Apple bundle's resource directory or a desktop tool's executable
directory. Returns `Unavailable` on Android, where APK assets are streams, and
when a native resource location can't be queried.

### `home_dir`

```cplus
fn home_dir() -> result::Result[text::Text, Error]
```

Returns the native home/profile directory. On iOS it is the app data container;
on Android it is `Context.getDataDir()`. Linux requires an absolute `$HOME`.
Returns `Unavailable` when the location can't be obtained.

### `temp_dir`

```cplus
fn temp_dir() -> result::Result[text::Text, Error]
```

Returns Foundation temporary storage on Apple, the internal cache root on
Android, absolute `$TMPDIR` or `/tmp` on Linux, and `GetTempPathW()` on Windows.
Returns `Unavailable` when the native query fails. OS cleanup can remove contents.

### `documents_dir`

```cplus
fn documents_dir() -> result::Result[text::Text, Error]
```

Returns native Documents on Apple/Windows, configured xdg-user-dirs Documents
or `$HOME/Documents` on Linux, and private internal files/Documents on Android.
Returns `Unavailable` when the location can't be obtained. The returned directory
isn't created by this package; Android shared documents need a content URI.

### `downloads_dir`

```cplus
fn downloads_dir() -> result::Result[text::Text, Error]
```

Returns Foundation Downloads on Apple (inside the app container on iOS), native
user Downloads on Windows, configured xdg-user-dirs Downloads
or `$HOME/Downloads` on Linux, and the app-specific external Download folder on
Android. Returns `Unavailable` when the native location is absent,
including unavailable Android external storage. It is not a shared public Downloads directory on mobile.

### `library_dir`

```cplus
fn library_dir() -> result::Result[text::Text, Error]
```

Returns Foundation's user Library directory on macOS and the app container's
Library directory on iOS. Returns `Unavailable` on other platforms or when the
native query fails. Unlike `data_dir`, this is the Library root with no namespace.

### `external_data_dir`

```cplus
fn external_data_dir() -> result::Result[text::Text, Error]
```

Returns Android's app-specific external files root from `getExternalFilesDir(null)`.
Returns `Unavailable` on other platforms, before Facet starts on Android, or when
external storage is unavailable. The location may be removed when the app is uninstalled.

### `external_cache_dir`

```cplus
fn external_cache_dir() -> result::Result[text::Text, Error]
```

Returns Android's app-specific external cache root from `getExternalCacheDir()`.
Returns `Unavailable` on other platforms, before Facet starts on Android, or when
external storage is unavailable. The OS may remove cached contents.

### `data_dir`

```cplus
fn data_dir(namespace: str = "") -> result::Result[text::Text, Error]
```

Returns persistent storage: Application Support on Apple, internal files on
Android, the XDG data root on Linux, and LocalAppData on Windows. Desktop adds
the supplied namespace or native installed ID; without either it returns
`Unavailable`. Mobile's root already belongs to the app, and a nonempty namespace
adds a child. Invalid namespaces return `InvalidInput` before a native lookup.

### `cache_dir`

```cplus
fn cache_dir(namespace: str = "") -> result::Result[text::Text, Error]
```

Returns storage for data that can be recreated: Library/Caches on Apple,
internal cache on Android, the XDG cache root on Linux, and LocalAppData/app/Cache
on Windows. Namespace selection, defaults and errors match `data_dir`; the
package doesn't create the namespace directory. OS cleanup can remove contents.

## Packaged assets

### `read_asset`

```cplus
fn read_asset(name: str) -> result::Result[vec::Vec[u8], Error]
```

Reads the entire named asset into owned bytes, preserving empty files and arbitrary
binary contents. Apple reads from main bundle resources, desktop tools from the
executable directory, and Android from APK assets, including compressed assets.
Names use forward slashes relative to that root; absolute paths, empty/dot components,
backslashes, NUL/control characters, Windows reserved characters/device basenames,
and components ending in dot/space return `InvalidInput` before native access.
Returns `Unavailable` without a resource host, `NotFound` for absent assets,
`ReadFailed` for read/access failures, or `OutOfMemory` for buffer failures.

### `read_asset_text`

```cplus
fn read_asset_text(name: str) -> result::Result[text::Text, Error]
```

Reads an asset as owned UTF-8 text with the same names, roots and read errors as
`read_asset`. Empty text and embedded NUL are preserved. Returns `InvalidData`
for malformed UTF-8; it neither replaces invalid bytes nor removes a BOM.

## Installed identity

Each present value is owned UTF-8 `Text`. Missing/empty values and failed native
queries return `None`. Android returns `None` before Facet starts.
Linux consults an already-loaded GIO library and live default GApplication.
Windows uses MSIX package identity or explicit process identity and VERSIONINFO.

### `id`

```cplus
fn id() -> option::Option[text::Text]
```

Returns the Apple bundle identifier, Android package name, Linux default
GApplication ID, or Windows MSIX package name (fallback: explicit process
AppUserModelID). The compiler manifest isn't a fallback for runtime identity.

### `name`

```cplus
fn name() -> option::Option[text::Text]
```

Returns localized `CFBundleDisplayName`, falling back to `CFBundleName`, on
Apple; returns the localized PackageManager application label on Android,
GLib's application name for a live default GApplication on Linux, or the
executable VERSIONINFO ProductName on Windows. GLib may fall back to its program
name when no application name was set; Windows needs a ProductName resource.

### `version`

```cplus
fn version() -> option::Option[text::Text]
```

Returns the installed user-facing version: `CFBundleShortVersionString` on
Apple or `PackageInfo.versionName` on Android; the GApplication version on Linux
when GIO 2.80+ provides it; or the four-part MSIX package version on Windows
(fallback: executable fixed file version). The value remains a string.

### `build`

```cplus
fn build() -> option::Option[text::Text]
```

Returns `CFBundleVersion` on Apple or the decimal Android version code. Android
API 28+ includes the full long version code; API 24–27 uses the legacy code.
Windows returns the decimal MSIX revision (fallback: fixed file-version revision).
Linux has no standard build field and returns `None`.

## Package metadata

`text`, `vec`, `result` and `option` in the signatures refer to the corresponding
`stdlib` modules; `Error` is `appenv::Error` at a consumer call site.

Import: `appenv/appenv`. Common dependency: `stdlib`. Apple consumers also
declare `objc`; Android consumers declare `jni`, `facet`, `flex_layout` and
`events` in their flat manifest. Supported platforms: macOS, iOS, Android,
Linux and Windows. Android lookups answer from the app's entry onward, on any
thread, once Facet has published the VM and Application context. All functions
are synchronous.

Unit tests live in `src/`, discovered through `src/test_main.cplus`:
`cd vendor/appenv && cpc test --filter appenv_`. Native integration fixtures
are in `tests/`; their runner and prerequisites are documented in [README.md](../README.md).
