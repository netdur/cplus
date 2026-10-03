# Standalone generated WinUI

A native WinUI window and button written in C+, using `winui` and `winrt`.
There is no Facet dependency, C++ source, or application C++ bridge. Clicking
the button changes its label through a generated delegate.

## Run

From the repository root, on Windows x64:

```powershell
& examples/winui_standalone/run.ps1
# Automated real pointer input, screenshots, and clean shutdown:
& examples/winui_standalone/run.ps1 -Verify
& examples/winui_standalone/run.ps1 -Verify -Release
```

Prerequisites: built `target/release/cpc.exe`, Windows SDK 10.0.26100.0 tools
and import libraries, LLVM/MSVC build prerequisites for C+, Python 3.10+,
and the unpacked `Microsoft.WindowsAppSDK.Runtime.2.5.1` NuGet package.
`-Verify` additionally needs Pillow and an unlocked interactive desktop.
Microsoft's runtime DLLs require the x64 Visual C++ runtime on the machine.

The default package cache is the existing
`playground/qwen_winui_preview/packages` directory. This is only a cache:
no preview executable, C++ source, or C++ headers are used. Supply another
cache with `-Packages C:\path\to\packages`. To obtain it independently:

```powershell
nuget install Microsoft.WindowsAppSDK.Runtime -Version 2.5.1 -OutputDirectory C:\path\to\packages -Source https://api.nuget.org/v3/index.json -NonInteractive
& examples/winui_standalone/run.ps1 -Packages C:\path\to\packages
```

The scripts extract the runtime into ignored `out/runtime`, derive activation
registrations directly from its MSIX manifest, copy WinUI theme resources,
compile C+, and embed the application activation manifest with `mt.exe`.
They do not install an MSIX or write registry registrations. The resource
setup is for this code-built sample; an application with its own XAML/PRI
needs a proper resource merge. Existing staged files are reused when the
package hash and deployment revision match; use a fresh output directory if
you modify its contents manually.

Logs and before/after screenshots live in `out`. The generated binary is
`out/runtime/winui_standalone.exe`. The build creates a local `vendor`
junction to the repository packages if needed.

## Source structure

`src/main.cplus` uses generated factories, constructors, interface queries,
properties, a generic collection, and event handlers. It also checks Size
and Rect ABI calls and boolean property roundtrips. The callback context is
borrowed; keep it alive until the handler is removed.

For routed `AddHandler` subscriptions, pass the generated delegate's
`boxed()` result and retain that box for `RemoveHandler`. A delegate's
`object()` is for typed event subscriptions; it is not an `IInspectable`.
Delegate boxing is generated and uses only `winrt`, without Facet.

The caller holds the STA apartment until after `application::run` returns,
event handlers are removed, and window/control references are released.
The sample's error helpers terminate on failure; a reusable application
should propagate `Result`/`Status` and perform its own cleanup.

## Regenerate

```powershell
cargo build --release -p cpc-bindgen
& tools/generate_winui.ps1
cargo test -p cpc-bindgen winmd::
python tools/test_winui_generation.py
```

The generator takes WinMD directly. Its selection is
`vendor/winui/selection.json`; provenance and unsupported APIs are in
`vendor/winui/MANIFEST.json`. Regeneration does not replace the handwritten
`vendor/winui/src/application.cplus` startup helper.

This is a verified first projection subset, not a complete Windows App SDK
binding. Arrays, extra out parameters, and owning-string structs remain
unsupported. Async interfaces are raw operations, without a Future adapter.
Returned object references use `winrt::Object`; query the appropriate
generated interface before calling its methods. The ABI currently targets
Windows x64 only. See the [WinMD generator documentation](../../cpc-bindgen/README.md#winmd--winmd).
