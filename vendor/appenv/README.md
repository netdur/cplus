# appenv

App paths, packaged assets and installed identity for C+ mobile and desktop applications.

```toml
[dependencies]
stdlib = "*"
appenv = "*"
```

```cplus
import "appenv/appenv" as appenv;
import "stdlib/result" as result;
import "stdlib/io" as io;

fn show_data_dir() {
    // iOS and Android already scope this directory to the current app.
    match appenv::data_dir() {
        result::Result::Ok(dir) => { io::println(dir.view()); }
        result::Result::Err(e) => { io::eprintln("App storage is unavailable"); }
    }
}
```

Provides app directories, packaged asset readers (`read_asset`, `read_asset_text`,
`asset_path`) and ID/name/version/build. Paths return `Result[Text, Error]`;
optional metadata returns `Option[Text]`. On desktop, supply
`namespace: "com.example.app"` for storage when no native app ID is available.
Getters leave CWD unchanged and don't create custom folders.

[Tutorial](docs/tutorial.md) · [Guide](docs/guide.md) · [Reference](docs/ref.md)

Unit tests are in `src/`, discovered through `src/test_main.cplus`:
`cd vendor/appenv && cpc test --filter appenv_`.
`tests/run_native.py` checks consuming apps, resources, Unicode, file I/O and
API privacy: `python3 vendor/appenv/tests/run_native.py host` (or `ios` /
`android`). iOS needs Xcode and a booted simulator; Android needs an ARM64
emulator/device, SDK, NDK and JDK. Set `CPC` to override the compiler path.
Append `--asan` to the host command for AddressSanitizer. Desktop source checks
and compiled documentation examples: `python3 vendor/appenv/tests/run_source_checks.py`.
These source checks do not replace native Windows/Linux testing; when Homebrew
GLib is available, the runner also exercises the real GApplication metadata API.
