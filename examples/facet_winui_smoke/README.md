# Facet rendered with WinUI

A retained Facet column, labels, and button rendered through `facet_winui`.
The app source imports Facet and the explicit host, without native control
construction, raw COM calls, or an application C++ bridge.

```powershell
# From the repository root:
& examples/facet_winui_smoke/run.ps1
& examples/facet_winui_smoke/run.ps1 -Verify
& examples/facet_winui_smoke/run.ps1 -Verify -Release
```

Prerequisites and `-Packages` cache override are the same as the
[standalone native sample](../winui_standalone/README.md). Runtime staging is
shared with that sample; the Facet example has its own `out/runtime` directory.
The activation manifest includes PerMonitorV2 DPI awareness. No C++ preview
build is required.

The test clicks three times, checks the displayed native label through Windows
UI Automation, resizes the window and checks native geometry, then closes.
Each click replaces its own button through Facet's mount API to exercise
removal during a native callback. The rest of the tree remains retained.
Shutdown must report zero view records and click subscriptions. Verification
uses real mouse input on an unlocked desktop, Pillow screenshots, and a local
PowerShell UI Automation helper (execution policy override for that process
only). Logs and screenshots are written under ignored `out`.

This tests the first renderer slice, not full backend parity. Text input,
scrolling, services, navigation and the `facet_runtime` selection seam remain
unimplemented; see [coverage](../../vendor/facet_winui/MANIFEST.md).
