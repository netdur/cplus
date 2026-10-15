# Tutorial

Quick path: add `appenv`, resolve a storage location, and write a file.
Platform choices and gotchas are in [guide.md](guide.md); exact signatures
are in [ref.md](ref.md).

Add the package and its active platform dependencies to your app's
`Cplus.toml`. Existing Facet apps usually already declare the Android closure.

```toml
[dependencies]
stdlib = "*"
appenv = "*"

[macos.dependencies]
objc = "*"

[ios.dependencies]
objc = "*"

[android.dependencies]
jni = "*"
facet = "*"
flex_layout = "*"
events = "*"
```

Save a small file to app storage. Getters return locations; create a directory
before writing if it isn't present yet.

```cplus
import "appenv/appenv" as appenv;
import "stdlib/result" as result;
import "stdlib/text" as text;
import "stdlib/fs" as fs;
import "stdlib/path" as path;

fn save_settings() -> bool {
    match appenv::data_dir() {
        result::Result::Ok(dir) => {
            match fs::create_dir_all(dir.view()) {
                result::Result::Ok(created) => {
                    let file: text::Text = path::join(base: dir.view(), child: "settings.json");
                    return match fs::write_string(file.view(), "{}") {
                        result::Result::Ok(n) => true,
                        result::Result::Err(e) => false,
                    };
                }
                result::Result::Err(e) => { return false; }
            }
        }
        result::Result::Err(e) => { return false; }
    }
}
```

For a desktop tool without an installed app ID, use
`appenv::data_dir(namespace: "com.example.notes")` and the same namespace
for `cache_dir`. On mobile, omit it to use the native app root; a supplied
namespace adds a child inside that root.

Read a packaged UTF-8 resource by its relative name. Place this example file in
the Apple resource bundle, Android APK `assets/defaults/`, or a desktop
`defaults/` directory beside the executable.

```cplus
import "appenv/appenv" as appenv;
import "stdlib/result" as result;
import "stdlib/io" as io;

fn show_defaults() {
    match appenv::read_asset_text("defaults/settings.json") {
        result::Result::Ok(value) => { io::println(value.view()); }
        result::Result::Err(e) => { io::eprintln("Could not read packaged settings"); }
    }
}
```

Use `read_asset` for images or other small binary data. Both readers return
owned values and preserve empty files.

When a library (such as a GGUF or ONNX model loader) requires a real filesystem
path on disk rather than an in-memory buffer:

1. Put the file in your project's `assets/` directory (e.g. `assets/models/model.gguf`).
2. Package the app (`cpc package` on macOS copies `assets/*` into `Contents/Resources/`; on iOS copy into the `.app` bundle; on Android pass `-A assets` to `aapt2 link`; on desktop place beside the executable).
3. Resolve the path at runtime with `appenv::asset_path("models/model.gguf")` (returns the direct bundle path on Apple/desktop; streams once to `no_backup_dir()` on Android and reuses it on future launches).
4. Pass `path.view()` to the library.

```cplus
import "appenv/appenv" as appenv;
import "stdlib/result" as result;
import "stdlib/io" as io;

fn resolve_model() {
    match appenv::asset_path("models/model.gguf") {
        result::Result::Ok(path) => { io::println(path.view()); }
        result::Result::Err(e) => { io::eprintln("Could not resolve packaged model"); }
    }
}
```

To show an installed version, match `appenv::version()` as
`option::Option[text::Text]`. `None` means the platform has no value for this
process. For OS, architecture, simulator status and OS version, use the existing
`stdlib/platform` API.

On Android every call works from the first line of `run()` and from any thread.
`read_asset` and `asset_path` work with APK assets even though `resource_dir()`
is unavailable; `app_path()` is the APK filename. See [guide.md](guide.md) for
platform behavior and [ref.md](ref.md) for every signature.
