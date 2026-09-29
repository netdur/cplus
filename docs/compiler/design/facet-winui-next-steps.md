# From standalone WinUI to facet_winui

**Progress:** the first `facet_winui` renderer slice is now implemented through
an explicit single-window host. Containers, labels, buttons, intrinsic sizing,
resize, coalesced updates, sender identity, and view/event teardown are covered
by `examples/facet_winui_smoke`. The test replaces its button while the click
callback is executing and checks native text through UI Automation. See
`vendor/facet_winui/MANIFEST.md` for the exact supported surface. The Windows
default in `facet_runtime` has not changed.

The standalone generated projection now establishes the native foundation:
WinMD-derived interfaces and delegates, native control construction, composed
Application startup, properties, generic child collections, real button input,
and shutdown work in debug and release without Facet or an application C++ bridge.
The package is a selected Windows x64 subset, not full SDK coverage.

## Package boundary

```text
standalone application -> winui -> winrt -> stdlib

cross-platform application -> facet + facet_runtime
                              -> facet_winui -> winui + winrt
```

Keep native bindings independent of Facet, like AppKit and objc. The generated
file belongs to `winui`; its handwritten Application owner stays separate.
`facet_winui` should only implement Facet's existing contracts. Windows host
interop may still need Win32 APIs; WinUI controls are COM objects and must
never be treated as HWND child controls.

## Existing integration points

`vendor/facet_win32/src/facet_win32.cplus` installs `mount::Renderer`, the
scheduler, input readers, and theme hooks. Its renderer includes create,
view_release, apply, insert, remove, schedule, and redeclare. Implement those
contracts in a new package using owned control records that also hold event
tokens and callback state. Teardown removes handlers before freeing contexts.

`vendor/facet_runtime/src/runtime_windows.cplus` currently imports four Win32
backend surfaces: backend installation, host/window operations, dialogs, and
clipboard. It also owns lifecycle/teardown wiring. A WinUI integration requires
adapting these together, rather than replacing only the renderer import.
`facet_runtime/Cplus.toml` has a Windows dependency closure naming
`facet_win32` and `win32`; the alternative needs the matching `facet_winui`,
`winui`, and `winrt` closure.

Keep the existing Win32 backend working while a separate explicit WinUI host
is tested. Platform filename selection distinguishes operating systems, not
two backends on the same OS; do not assume an unimplemented manifest feature
can choose between them. Decide the public selection mechanism after the
first adapter slice proves the required host behavior.

## First adapter slice

1. Host one WinUI Window through `winui/application`, establish HWND interop
   for host-only services, and implement close/resize/DPI lifecycle handling.
2. Implement Canvas containers, TextBlock labels, Button, TextBox, and basic
   scrolling. Facet/flex_layout remains the source of layout. Apply its frames
   in WinUI device-independent coordinates; do not introduce a second portable
   layout engine through StackPanel/Grid semantics.
3. Map events into existing Facet sender readers. Track tokens per native
   record, with exact removal and context disposal on view release.
4. Add dispatcher scheduling and delayed callbacks, including pending-work
   teardown. Extend the metadata selection for dispatcher APIs as needed.
5. Adapt inspection/automation geometry to native WinUI objects. Existing
   Windows view inspection assumptions about HWNDs need an explicit audit;
   only the top-level host has that representation.
6. Reproduce the Qwen UI through Facet, compare it to the direct WinUI preview,
   and exercise updates, keyboard focus, scrolling, resizing, and close.

Dialogs and clipboard can use shared Windows facilities where appropriate,
but their window ownership and callback contracts still need verification.
Record unsupported vocabulary honestly in a backend coverage manifest.

## Work still required in the native projection

The current selection has 326 metadata entries, 2,539 emitted methods, and
90 skipped methods. TypeName and XmlnsDefinition lack owning-struct
projection. Arrays/additional out parameters remain skipped, and raw async
interfaces have no Future adapter. These are explicit coverage limits in
`vendor/winui/MANIFEST.json`, not completed capabilities.

Extend the generator when adapter needs encounter these limits; keep new ABI
facts generated. Add isolated handwritten helpers for non-WinMD host interop
or lifecycle contracts. Test multiwindow ownership, cancellation, dispatch,
and failures separately: the existing one-window click test does not establish
those properties.

The first Facet label/button renderer milestone and basic text-field/scroll
adapter are implemented. Text edits write back to Facet; programmatic text
updates suppress duplicate edit callbacks. ScrollViewer owns a Canvas document,
tracks native offsets, and applies Facet offset writes after layout. The desktop
sample covers typing, max length, read-only input, wheel scrolling, offset
preservation, and programmatic text/offset updates alongside the original checks.
Remaining
steps are broader native control/property coverage, host HWND interop, service
scheduling and cancellation, inspection, and `facet_runtime` integration before
changing the default. The generated package remains independently usable.

## Gallery checkpoint (2026-09-29)

The shared 36-page gallery now has a separate WinUI launcher in
`examples/facet_gallery_winui`. Native controls, viewport lists/relist,
TreeView navigation, drawing, decorations and UI-thread scheduling are wired.
The generator now projects non-aggregated parameterized composition factory
calls (including FontFamily), releasing the additional inner reference.
The selection has 735 metadata entries, 5,015 emitted methods and 135 skips.
See `vendor/facet_winui/MANIFEST.md` for current limits; earlier slice counts
above are historical. Full runtime-facade integration remains separate.
