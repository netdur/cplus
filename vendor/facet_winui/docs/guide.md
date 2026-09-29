# Guide

See [tutorial.md](tutorial.md) for setup and [ref.md](ref.md) for signatures.

## Ownership

The host consumes the tree, mounts it, and owns it until the native loop ends.
Each Facet node owns a native View record through `Data.view_release`. The
record owns a COM control reference and its event token. WinUI's visual parent
also retains a child; those two ownership relationships are separate.

Removing a child detaches it from the native collection. Dropping the returned
Facet node releases its record and unsubscribes its handler. An event handler
may destroy its own button: dispatch copies the callback/context before calling
user code and never reads the freed record afterward. The sample exercises
this with three successive replacement buttons.

Component state must outlive its callbacks. Use a retained component/Box, not
a local whose address is captured and escapes. The host's apartment outlives
the tree and every owned native reference. A process-global routing pointer
borrows the active host but owns no application state; it is cleared before
the host drops. Queued renderer callbacks capture no host pointer.

## Layout and dispatch

Every container is a Canvas. Facet/flex_layout computes frames in logical
coordinates; the backend subtracts the parent's origin and applies dimensions.
It does not let WinUI StackPanel/Grid compute a second portable layout.
Native Measure/DesiredSize supplies leaf intrinsic sizes, with explicit native
width/height temporarily cleared during measurement and then restored.

Renderer requests coalesce onto DispatcherQueue. The callback opts into
IAgileObject because the queue requires it, captures no object/context, and
executes only on that queue's UI thread. Ordinary control delegates remain
non-agile. This does not implement `services::run_on_main`, timers, or jobs.

The deployment manifest declares PerMonitorV2 DPI awareness. WinUI SizeChanged
supplies logical client dimensions for layout. Testing was performed at the
machine's current scale and after window resizing; cross-monitor DPI changes
are not yet an automated coverage claim.

## Scope

Use the explicit host for this first slice. `facet_runtime` still selects
Win32 on Windows. Before selecting this backend through that facade, implement
its host/window lifecycle, services, dialogs, clipboard and inspection seams.
Only one backend and one WinUI Application run are supported per process.

Initial unsupported trees return E_NOTIMPL. Internal native failures abort with
an HRESULT diagnostic instead of continuing with invalid handles. Full error
recovery and broader property/control support are listed in
[MANIFEST.md](../MANIFEST.md).
