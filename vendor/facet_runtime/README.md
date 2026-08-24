# facet_runtime

The boot facade for facet apps: one import starts the app, and the platform
override picks the backend — app code never names a platform.

```toml
[dependencies]
facet         = "*"
facet_runtime = "*"
# plus the platform backend's closure — `cpc pm add . facet_runtime` writes it
```

```cplus
import "facet_runtime/runtime" as runtime;

fn main() -> i32 {
    var app: runtime::App = runtime::App::new("MyApp");
    app.screen("home", home_factory);
    let _s = app.run("home");
    return 0;
}
```

`import "facet_runtime/runtime"` resolves by filename override:
`runtime_macos.cplus` on a macOS target (installs facet_appkit),
`runtime_linux.cplus` on Linux (facet_gtk), `runtime_ios.cplus` on iOS
(facet_uikit), the neutral `runtime.cplus` anywhere else — which renders
nothing and says so, never some other platform's toolkit. The facade installs
the backend into facet's seams; facet itself knows no backend.

A facade is a COPY of another facade, not a fresh file: ~1100 of
`runtime_linux.cplus`'s lines are `runtime_macos.cplus`'s verbatim, because
almost all of the facade is about facet's own tiers (App, routes, nav, screens,
teardown) and not about a toolkit. Three regions differ and each says so where
it sits: the imports, the lifecycle observers, and the quit seam.

This package exists so every dependency arrow points down (2026-08-17):
apps → facet_runtime → backend → facet. The old facet ↔ facet_appkit cycle
lived exactly here — the one file in facet that named a platform — and
moving it out is what made the family's dependency graph acyclic. The full
runtime surface (App, routes, windows, alerts, menus) is documented in
`vendor/facet/docs/ref.md` under "facet_runtime/runtime"; only the import
path moved.

Tests: `cd vendor/facet_runtime && cpc test`. The suite's load-bearing line
is `test_main.cplus`'s `import "./runtime"` — it compiles the ACTIVE
platform's facade and backend, so a backend that does not build turns this
package red.
