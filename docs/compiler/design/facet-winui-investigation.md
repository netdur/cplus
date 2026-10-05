# Facet WinUI backend investigation

Follow-up: the C++ preview now runs, and a direct C+ alternative has been
[audited against issue #19](facet-winui-issue-19-audit.md). The bridge below is
an implementation option, not a technical requirement imposed by WinUI.
The subsequent [pure C+ probe](../../../playground/winui_probe/README.md) now
opens a themed window, receives real button clicks, and exits cleanly without
an application C++ bridge. This supports pursuing the direct WinMD projection.

Investigated 2026-09-28. Status: source and documentation review, not a built
WinUI prototype. No backend selection or application behavior was changed.

Recommendation: build an opt-in `facet_winui` backend, using a C ABI bridge
implemented in C++/WinRT. Start with one WinUI XAML Island per top-level Win32
window. Keep Facet's tree, state, layout and application API. Validate one
representative screen before committing to contract parity.

## Why this fits

Facet already separates its UI vocabulary from native implementation.
[`mount::Renderer`](../../../vendor/facet/src/mount.cplus) supplies `create`,
`apply`, `insert`, `remove`, `schedule`, `view_release` and `redeclare`.
`Data.view` is opaque: it can carry an owned bridge handle instead of an HWND.
There is no fundamental requirement that every widget be a child window.

WinUI supports embedding a `Microsoft.UI.Xaml` tree into an HWND through
`DesktopWindowXamlSource`. Use the Windows App SDK API family, not old
`Windows.UI.Xaml` UWP Islands examples. Initialize the island with the host's
`WindowId`; resize its `SiteBridge` with the client area. This preserves a
Win32 application shell while replacing the visible content.
[Hosting documentation](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/host-controls-existing-desktop-apps),
[API reference](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.hosting.desktopwindowxamlsource).

This is a new renderer, not a theme for `facet_win32`. Standard WinUI templates
are the intended visual baseline. Explicit application colors, dimensions and
fonts still affect the result; migrating controls will not fix an application's
spacing automatically. Arbitrary Facet decoration also needs individual mapping
and cannot be declared supported just because WinUI is more flexible.

## Proposed boundaries

```text
application written in C+
  -> Facet tree, state and flex_layout
  -> facet_winui (C+: props, events, layout coordination, Facet services)
  -> narrow C ABI
  -> facet_winui_bridge.dll (C++/WinRT: objects, resources, UI-thread lifetime)
  -> WinUI content tree inside a Win32 host window
```

The C+ side should own knowledge of Facet's kinds, dirty bits and callbacks.
The bridge should own WinRT references, event tokens and platform operations.
Avoid transferring raw Facet structs across the boundary: a C++ copy of generated
prop layouts would be a second contract to maintain.

Use opaque handles, fixed-width scalars, pointer-plus-length UTF-8, explicit
out parameters, and integer error codes. Catch C++ exceptions at every exported
entry point. Copy callback strings before their borrowed buffers expire. Keep
allocation and freeing on the same side of the DLL boundary. Verify both call
directions and calling conventions in the first executable probe.

A handle can hold both an outer decorated element and an inner control, plus
its child-host element, event revokers and callback context. Insertion then knows
whether to target a panel's children or a content control's single content slot.
Removing a child detaches it; `view_release` releases the backend's ownership.
Revoke callbacks before freeing node contexts, including during a callback that
causes its own node to be removed.

Direct WinRT ABI calls from C+ are an alternative, but would require activation,
interface discovery, reference counting, HSTRING handling, delegates and generic
collection interfaces. A C++/WinRT bridge contains that work without requiring
a new language projection as a prerequisite for improving Facet's UI.

## What must change in this repository

| Existing surface | Consequence for WinUI |
|---|---|
| [`facet/src/mount.cplus`](../../../vendor/facet/src/mount.cplus) | Its opaque renderer interface is a suitable starting point. It installs one process-wide renderer; do not install both backends or hot-switch live trees. |
| [`facet_win32/src/views.cplus`](../../../vendor/facet_win32/src/views.cplus) | Reuse the create/apply discipline and non-view-kind decisions. Rewrite HWND creation, parenting and destruction. |
| [`facet_win32/src/window.cplus`](../../../vendor/facet_win32/src/window.cplus) | Its host currently assumes HWND-backed roots and children. Retain useful lifecycle logic, but create a WinUI-specific host path rather than importing this module unchanged. |
| [`facet_win32/src/geometry.cplus`](../../../vendor/facet_win32/src/geometry.cplus) | Replace child `SetWindowPos` calls with XAML layout. Preserve viewless-container offsets, scroll extents and virtual row placement. |
| [`facet_win32/src/scheduler.cplus`](../../../vendor/facet_win32/src/scheduler.cplus) | Preserve coalesced sync followed by relayout. Integrate WinUI dispatch and shutdown. This file also has existing local edits. |
| [`facet_runtime/src/runtime_windows.cplus`](../../../vendor/facet_runtime/src/runtime_windows.cplus) and its manifest | Both explicitly select Win32 today. An opt-in facade/probe is required before any default switch. Filename dispatch selects OS, not toolkit. |
| [`facet_agent/src/inspect_platform_windows.cplus`](../../../vendor/facet_agent/src/inspect_platform_windows.cplus) | It passes native views to HWND geometry functions. Add backend-aware bounds/highlight operations before enabling it for WinUI handles. |
| [`facet_agent/src/agent_windows.cplus`](../../../vendor/facet_agent/src/agent_windows.cplus) | Its native reader uses `agent_win32`. HWND walking will not enumerate individual XAML controls; retain the Facet tree surface and separately adapt native inspection. |
| [`camera/src/camera_backend_windows.cplus`](../../../vendor/camera/src/camera_backend_windows.cplus) | Its adopted preview is an HWND. This cannot be inserted into a XAML children collection as if it were a UIElement. |

Window identifiers can remain HWNDs while node view handles become bridge
objects. These two identities must stay distinct throughout the Windows code.
The generic `adopt_native` escape hatch also needs an explicit representation
policy; blindly casting an existing adopted HWND to a WinUI handle is invalid.

## Layout is the first technical gate

Facet should own outer control geometry. WinUI should measure native content and
lay out each control's internal template.

1. Apply props and theme resources before measuring.
2. Map flex measure constraints to WinUI `Measure` and read `DesiredSize`.
3. Run Facet layout to compute the outer frames.
4. Supply those frames to an absolute-layout host panel which arranges its
   children during XAML's layout pass.
5. Remeasure when text, font, theme, text scaling or width changes require it.

`Measure` updates `DesiredSize`; measurement precedes initial arrangement and
can invalidate arrangement. XAML also schedules layout asynchronously, so merely
calling `Arrange` once from Facet and letting another panel arrange later is
not a reliable ownership model.
[WinUI measurement reference](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.uielement.measure).

A Canvas with explicit positions and dimensions is a possible first probe;
a custom panel consuming stored Facet frames is the stronger production design.
Check whether custom panel authoring needs generated IDL/runtime-class support
in the chosen bridge project. Clear stale explicit sizes before intrinsic
measurement, and decide how native minimum sizes interact with small explicit
Facet frames. Avoid a global layout on every `LayoutUpdated` event.

Keep Facet/XAML coordinates logical. Convert only the native host/island boundary
to physical pixels. The current Win32 geometry code multiplies every child frame
by DPI; carrying that operation into XAML children would double-scale them.

## Initial control mapping to validate

These are implementation candidates, not parity claims.

| Facet concept | WinUI candidate | Main behavior to prove |
|---|---|---|
| Label / styled spans | TextBlock / Run | Wrapping, baseline, font metrics |
| Text or icon button | Button and appropriate content | Click, keyboard activation, focus and disabled state |
| Editable field | TextBox; PasswordBox for secure entry | Editing, selection, submit, IME, Unicode index conversion |
| Toggle / checkbox / slider | ToggleSwitch / CheckBox / Slider as appropriate | Match the existing kind's semantics and suppress update feedback loops |
| Progress | ProgressBar / ProgressRing | Determinate versus indeterminate behavior |
| Popup chooser | ComboBox | Selection, item enablement and popup keyboard behavior |
| Scroll | ScrollViewer over a Facet layout host | Explicit content extent, wheel/touch, scroll position restoration |
| List / collection / relist | ListView or ItemsRepeater with a Facet element factory | Virtualization, stable row identity, bind/recycle lifecycle and inspection |
| Menus / contextual actions | MenuBar / MenuFlyout / CommandBar where appropriate | Commands, accelerators and target selection |
| Decorated container | Border plus a layout panel | Insets, clipping, opacity and hit testing |

ItemsRepeater offers virtualization but supplies no default UI or interaction
policy; it is especially plausible for Facet's `relist`, but requires a deliberate
element factory and a virtualization-supporting host. A themed selectable list
may fit ListView better.
[ItemsRepeater reference](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.controls.itemsrepeater).

Keep default control resources and override only explicitly requested values.
Include and validate the WinUI control resource setup in the bridge; creating
objects programmatically does not mean resources and their deployment disappear.
[XamlControlsResources reference](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.controls.xamlcontrolsresources).

## Threading, focus and teardown

The host should initialize the runtime and UI apartment before creating WinUI
objects, retain its XAML manager/islands, and own the UI thread's dispatcher.
Use `Microsoft.UI.Dispatching.DispatcherQueue`; it is distinct from the system
`Windows.System.DispatcherQueue`. A current-thread queue needs a running message
pump. Integrate content message preprocessing and island focus navigation, rather
than assuming the existing TranslateMessage/DispatchMessage loop is sufficient.
[DispatcherQueue documentation](https://learn.microsoft.com/en-us/windows/apps/develop/dispatcherqueue).

Schedule one coalesced Facet sync and layout on the UI thread. Prevent callbacks
from programmatic property updates from reentering the same update indefinitely.
Close islands and disconnect callbacks before shutting down XAML and the queue;
test closing either of two windows and closing while work is queued.
[Island lifetime and focus guidance](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/host-controls-existing-desktop-apps).

For native HWND adoption, start with an explicitly unsupported result in the
probe. Investigate a separately positioned native child with clipping/focus
restrictions, or a WinUI-specific implementation of the capability (for example,
a camera surface). It is not safe to promise arbitrary native child composition,
rounded clipping or opacity across this boundary.

## Build and deployment

[`cplus-core/src/manifest.rs`](../../../cplus-core/src/manifest.rs) provides
`[link] libs`, `search-paths` and `extra-objects`.
[`cpc/src/main.rs`](../../../cpc/src/main.rs) consumes them. `BuildSpec` contains
`prebuild` and `dev`; the reviewed build path has no WinUI/NuGet/C++ project
integration. Linking an external bridge has an existing route, but building and
staging that bridge needs an explicit step.

Proposed first build: restore pinned Windows App SDK and C++/WinRT packages in a
small MSBuild C++ project; build the DLL and import library; link the C+ probe;
stage the DLL and required runtime/resources beside the executable. Prefer the
supported MSBuild package targets initially instead of reproducing SDK resource
generation by hand. Verify the import library with the actual `cpc`/Clang target.

Local inspection found Clang, clang-cl, CMake, VS 2022 Community MSBuild, and
Windows SDK include directories through 10.0.26100.0. This establishes useful
prerequisites, not a verified WinUI build environment. SDK package restore,
C++/WinRT generation, runtime activation and execution remain untested.

For an unpackaged framework-dependent app, deploy the Windows App SDK runtime
and initialize it before XAML. The C+ executable is not an MSBuild application,
so do not assume project auto-initializers in a bridge DLL cover executable
startup. Make bootstrap ownership explicit.
[Unpackaged deployment](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/deploy-unpackaged-apps).

Self-contained deployment is another option, with more files and application-owned
runtime servicing; MSIX is not required for the initial investigation.
[Unpackaged distribution options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/unpackage-winui-app).
Pin a supported stable release at implementation time. The release-channel page
reviewed on this date lists 2.5.1 as latest stable; confirm its exact NuGet package
versions and toolchain requirements when restoring, rather than copying an older
1.4/1.8 sample pin.
[Release channels](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/release-channels).

## Prototype and decision criteria

1. **Bridge and startup:** a C+ executable creates and destroys a WinUI island
   through the DLL, receives a button callback, reports initialization failures,
   and runs outside Visual Studio. Verify a second launch and full cleanup.
2. **Facet screen:** use the real Renderer, with a resizable form containing a
   label, field, button, toggle, scroll area and menu. Check both themes and
   100%, 150% and 200% DPI, including moving between monitors.
3. **Interaction and lifetime:** test Tab/Shift+Tab, Enter/Escape, Unicode/IME,
   text selection, update from a callback, deletion during an event, repeated
   mount/unmount, and two windows. Inspect standard control accessibility and
   map Facet keys to automation IDs where appropriate.
4. **Collections and native integration:** test a 20,000-row list/relist with
   bounded realization, stable identity and correct callbacks after recycling.
   Decide the camera/native-adoption policy and repair inspector bounds before
   claiming the existing Windows application surface works.
5. **Distribution and visual decision:** run on a clean supported machine with
   the chosen runtime model. Compare the same screen against Win32, including
   startup, idle memory, resizing and scrolling. Have the user assess the actual
   visual result; API suitability alone does not establish that it looks right.

Use a dedicated `playground/winui_probe` and explicit backend installation first.
Do not call the existing Windows runtime facade in that probe: it installs Win32.
After the probe passes, add an explicit opt-in runtime route and a documented
contract manifest for `facet_winui`. Keep the current default until essential
controls, lifecycle, distribution and required native integrations are proven.

The recommendation is **go for a bounded prototype**. The strongest reasons are
the existing opaque renderer seam and WinUI's supported HWND hosting. The largest
risks are layout coordination, HWND adoption, Windows inspection assumptions and
runtime distribution. Full parity is a backend project, not a library substitution.
