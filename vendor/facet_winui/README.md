# facet_winui

Experimental Windows x64 renderer for retained Facet trees using generated
WinUI bindings. Runs the shared 36-page Facet gallery, with native controls,
text entry, graphics, scrolling, viewport-based lists, and decorations.

Start with [the gallery launcher](../../examples/facet_gallery_winui/README.md).

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
& examples/facet_gallery_winui/run.ps1 -Verify -Release
```

These test real pointer/keyboard/wheel input, text readback and programmatic
updates, length limits and read-only fields, scrolling, resize, control
replacement during its callback, and cleanup. The existing Win32
backend and `facet_runtime` default are unchanged. This package is not yet a
replacement for the full Windows runtime facade.
