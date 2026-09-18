# facet_appkit

The macOS AppKit backend for facet: native mounting, input, layout sync,
services, and window integration.

Use `cpc pm add . facet_appkit --platform macos` so the consuming manifest gets
the full macOS dependency closure. Applications normally import the runtime,
not this backend directly; `App::run` installs it:

```toml
[dependencies]
facet         = "*"
facet_runtime = "*"

[macos.dependencies]
facet_appkit = "*"
```

```cplus
import "facet_runtime/runtime" as runtime;
import "facet/screen" as screen;

fn main() -> i32 {
    let app = runtime::App::new("hello");
    app.window("main", main_screen,
               chrome: screen::Chrome::new(title: "Hello"));
    if app.run("main").is_ok() { return 0; }
    return 1;
}
```

- [Tutorial](docs/tutorial.md) — declare and run a macOS facet app
- [Guide](docs/guide.md) — mounting, sync, ownership, and AppKit constraints
- [Reference](docs/ref.md) — installed seams and backend surface
- [Backend manifest](MANIFEST.md) — disposition of every facet contract verb

Run unit tests from this directory with `cpc test`. The suite includes the
backend seam and leak harness; visual behavior still needs a real app window.
