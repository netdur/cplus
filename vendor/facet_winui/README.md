# facet_winui

Experimental Windows x64 renderer for retained Facet trees using generated
WinUI bindings. This first slice supports containers, labels, and buttons.

```toml
[dependencies]
stdlib = "*"
events = "*"
flex_layout = "*"
facet = "*"
winrt = "*"
winui = "*"
facet_winui = "*"
```

```cplus
import "facet_winui/facet_winui" as host;
// host::run(tree, "My application") owns the tree through native shutdown.
```

[Tutorial](docs/tutorial.md) · [Guide](docs/guide.md) · [Reference](docs/ref.md)
· [Coverage](MANIFEST.md)

Run the integration checks from the repository root:

```powershell
& examples/facet_winui_smoke/run.ps1 -Verify
& examples/facet_winui_smoke/run.ps1 -Verify -Release
```

These test real pointer input, native label updates, intrinsic sizing, resize,
control replacement during its callback, and cleanup. The existing Win32
backend and `facet_runtime` default are unchanged. This package is not yet a
replacement for the full Windows runtime facade.
