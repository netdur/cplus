# WinUI generator checkpoint — 2026-09-29

Resumed after the battery pause. The first generated standalone WinUI subset
and first explicit Facet renderer slice are implemented. All work is saved locally; no commit or GitHub
post was made. The unrelated user file `windows.todo.me` remains untouched.

## Implemented

- `cpc-bindgen --winmd`: direct metadata projection, provenance and coverage.
- `vendor/winrt`: shared COM/HSTRING/HRESULT/apartment/delegate support.
- `vendor/winui`: generated native bindings plus separate handwritten
  `src/application.cplus` composition/startup helper. No Facet dependency.
- `examples/winui_standalone`: native window, button, click callback and cleanup.
  Runtime files and activation registrations are staged from the SDK MSIX;
  no C++ preview build or application C++ bridge is used.
- `tools/generate_winui.ps1` and `tools/test_winui_generation.py`.
- Package/runtime docs, sample instructions, and `facet-winui-next-steps.md`.
- `vendor/facet_winui`: single-window host, Canvas containers, TextBlock labels,
  buttons, native measurement, renderer dispatcher, sender readers, teardown.
- `examples/facet_winui_smoke`: retained counter with real mouse/UIA testing,
  resize, and replacement of the emitting button during its callback.
- Explicit opt-in agile delegates for DispatcherQueue; PerMonitorV2 declaration
  in the shared deployment manifest fixes scaled-window pointer coordinates.

The generated source is current, including reserved-word escaping, public-only
plain composition constructors, and 16-byte-aligned aggregate temporaries.

## Verification

- `cargo test -p cpc-bindgen`: 175 passed.
- `cpc test --filter winrt_` in `vendor/winrt`: 4 passed, including COM
  clone/QI ownership checks, unsupported-IID output clearing, and agile opt-in.
- `python tools/test_winui_generation.py`: deterministic output, matching
  checked-in generation, provenance hashes, rejected invalid selection,
  and 130 ABI slots / 10 interface IIDs matched to independent SDK headers.
- Standalone debug and release: real mouse click delivered to C+, button
  label changed, Application.Start returned, event cleanup completed, exit 0.
- Native checks include Size/Rect inputs, Size output, bool/f64 properties,
  computed generic collection IID, theme resources, and generated delegates.
- Facet debug and release checks both passed: three real clicks, native UIA
  label updates, resize, repeated control replacement inside its callback,
  and shutdown with zero owned view records / click subscriptions. The original
  standalone sample also passed after the shared delegate/DPI changes.

## Run or regenerate

From the repository root in PowerShell:

```powershell
& examples/winui_standalone/run.ps1
& examples/winui_standalone/run.ps1 -Verify
& examples/winui_standalone/run.ps1 -Verify -Release
& examples/facet_winui_smoke/run.ps1 -Verify
& examples/facet_winui_smoke/run.ps1 -Verify -Release
cargo build --release -p cpc-bindgen
& tools/generate_winui.ps1
python tools/test_winui_generation.py
```

The sample defaults to the existing package cache at
`playground/qwen_winui_preview/packages`; pass `-Packages` to use another.
Its own deployment is in ignored `examples/winui_standalone/out/runtime`.
Verification logs/screenshots are in `examples/winui_standalone/out`.
Desktop screenshot/mouse verification requires an interactive unlocked desktop.
Build prerequisites and NuGet restoration are documented in the example README.

## Scope and next step

Current selection: 326 metadata entries, 2,539 emitted methods, 90 skipped
methods, 2 skipped owning structs. Windows x64 only. Arrays/additional out
parameters and richer delegate signatures need further projection work;
raw async operations have no Future adapter. Object-valued methods use
`winrt::Object` and typed interface queries. This is not full WinUI coverage.

The first `facet_winui` slice uses an explicit host and is not selected by
facet_runtime. Its exact limits are in `vendor/facet_winui/MANIFEST.md`.
Broader controls, services/timers, host interop, inspection and runtime facade
integration remain future milestones. Keep the generated package independently
usable. GUI evidence is in each example's ignored `out` folder.

Do not commit, publish, or post GitHub comments without authorization.
