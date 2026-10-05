# Facet rendered with WinUI

A retained Facet column, labels, button, text field, and scroll document
rendered through `facet_winui`.
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

The test types into a native TextBox, checks Facet text readback through an echo
label, verifies max-length/read-only behavior and programmatic text updates,
and scrolls with the mouse wheel. It checks initial and live scroll offsets
and preservation across unrelated updates. It also clicks three times, checks
the displayed native label through Windows UI Automation, resizes the window
and checks native geometry, then closes.
Each click replaces its own button through Facet's mount API to exercise
removal during a native callback. The rest of the tree remains retained.
Shutdown must report zero view records and control event subscriptions. Verification
uses real mouse/keyboard input on an unlocked desktop, Pillow screenshots, and a local
PowerShell UI Automation helper (execution policy override for that process
only). Logs and screenshots are written under ignored `out`.

This tests an experimental renderer, not full backend parity. Broader editing
and control properties, services, navigation and the `facet_runtime` selection seam remain
unimplemented; see [coverage](../../vendor/facet_winui/MANIFEST.md).
