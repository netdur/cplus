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

Ordinary containers are Borders with a Canvas document; a ScrollViewer owns
a document Canvas.
Facet/flex_layout computes frames in logical
coordinates; the backend subtracts the parent's origin and applies dimensions.
It does not let WinUI StackPanel/Grid compute a second portable layout.
Native Measure/DesiredSize supplies leaf intrinsic sizes, with explicit native
width/height temporarily cleared during measurement and then restored.

Renderer requests coalesce onto DispatcherQueue. The callback opts into
IAgileObject because the queue requires it, captures no object/context, and
executes only on that queue's UI thread. Ordinary control delegates remain
non-agile. DispatcherQueueTimer implements delayed callbacks and cancellation;
size observers run after layout. Worker-thread dispatch/jobs remain deferred.

The deployment manifest declares PerMonitorV2 DPI awareness. WinUI SizeChanged
supplies logical client dimensions for layout. Testing was performed at the
machine's current scale and after window resizing; cross-monitor DPI changes
are not yet an automated coverage claim.

## Scope

TextBox changes update retained text before invoking the application handler.
Programmatic writes suppress matching native notifications; applying other
properties avoids assigning identical text and resetting selection. Selection
setters use UTF-16 offsets, clamp to the native text length, and read back
selection-only changes. Enter submits fields/search; Ctrl+Enter submits a
multiline area. Secure fields use native PasswordBox controls and have no
native caret/selection API. Keyboard scopes map to native input scopes.

Slider minimum/maximum track colors map to the native foreground/background
brushes. Clearing a color restores the WinUI theme. Native hover, pressed and
disabled feedback remains theme-controlled; thumb styling is still unsupported.
Date-picker formats accept WinUI's
[DateTimeFormatter patterns](https://learn.microsoft.com/en-us/uwp/api/windows.globalization.datetimeformatting.datetimeformatter).
The adapter also translates these common numeric forms: `yyyy-MM-dd`,
`dd/MM/yyyy`, `MM/dd/yyyy`, `d/M/yyyy`, `M/d/yyyy`, `dd.MM.yyyy`, `yyyy/MM/dd`,
and `MM/dd/yy`. An empty format restores the native default. Formats rejected
by the native setter fall back to the default; this is not a general .NET/ICU
format-string parser.

Time-picker formats select the native hour cycle: `H:mm`, `HH:mm`, or
`24HourClock` select 24-hour display; `h:mm tt`, `hh:mm tt`, `h:mm a`,
`hh:mm a`, or `12HourClock` select 12-hour display. Empty or unsupported
formats restore the native hour cycle. WinUI still determines padding,
separators and period placement; custom layouts and seconds are not supported.

Popup title color affects its empty-selection placeholder, independently of the
selected-item text color. Clearing calendar dates and limits restores an empty
selection and the native date range.

Scroll document size is the union of its direct children's frames and the
viewport. Offset writes are applied after native layout; initial writes wait
until a viewport exists. User scrolling updates retained offsets before
notifying observers. Unrelated dirty updates preserve the current offset.
The application must give its content a scrollable layout extent, as shown by
the sample's fixed-height document with shrink(0).

Lists use uniform row height by default. Set `uneven_rows: true` and provide
`row_height_of` to supply different heights by index. Prefix offsets are cached
until the row data, height settings or available width change; scrolling locates
the visible range without calling the height provider again. Automatic
content-driven height measurement is not implemented. Collection height providers
support first-item and all-item sizing; each grid row uses its tallest item.
Relist still uses uniform pitch.

Tree `row_height` sets the native item-container height in DIP, including
existing, newly expanded and replacement rows. The theme minimum is cleared
for compact rows. Nonpositive values remove the override and restore native
measurement. Changing height preserves node identity, selection and expansion;
Windows may round the rendered height to the display's pixel grid.

List appearing/disappearing events describe realised rows, including the small
overscan range. Callbacks run outside layout and receive the row sender, so
`component::item_index_of` identifies the row. A disappearing row remains alive
until its callback returns. Removing the host cancels its remaining callbacks;
shutdown does not emit application callbacks for every removed row.

Collection scroll anchoring supports preserving the pixel offset, keeping the
same item after inserts/removals above it, and keeping a bottom-pinned viewport
at the bottom as items arrive. Use `insert_rows`/`remove_rows` to identify splices;
a plain count reset cannot identify which records moved. An explicit `scroll_to`
takes priority over anchoring. Scroll observers run after the native event,
allowing them to replace the collection safely.

Wrap scrollable content in `ui::refreshable(refreshable: true,
on_refreshing: handler)`. The WinUI host uses a native RefreshContainer with
its template-created indicator. Desktop users can invoke its Refresh context
menu, and applications can start a cycle with `set_refreshing(true)`.
Both paths update the model and queue `on_refreshing` on the UI thread. The
indicator stays active until `set_refreshing(false)`; repeating true during an
active cycle does not dispatch another callback. Initial true waits for native
Loaded. Clear `refresh_color` to restore the native foreground.

Disabling refresh blocks user requests while leaving child controls enabled.
Programmatic refreshing remains available when user refresh is disabled.
Callbacks may finish the refresh or remove the host. Removal cancels a queued
application callback and completes the native deferral. List-local refresh
properties remain unsupported: use the wrapper, as the gallery does. Native
pull gestures require supported touch input and scrollable content; the desktop
probe verifies requests and lifecycle but does not simulate physical touch.
See Microsoft's [pull-to-refresh guidance](https://learn.microsoft.com/en-us/windows/apps/develop/ui/controls/pull-to-refresh).

Swipe action items appear in a native context menu. Both `on_clicked` and
`on_invoked` run in that order. If `on_clicked` removes its own action or host,
the pending `on_invoked` is cancelled; unrelated node replacement does not
cancel it. Icons accept image sources or the backend's existing small named
symbol mapping, including `archivebox` and `trash`. Clearing an icon removes it;
clearing the destructive flag restores the native foreground.

Accessibility names and hints update native automation properties. Clearing them
removes the local override so the control can recover its normal accessible name.
Heading levels 1–9 map to WinUI; zero and out-of-range values mean no heading.
Tooltips use native ToolTipService with Unicode content, update live and disappear
when cleared. Unrelated property changes preserve the existing tooltip object.

Carousel pages are ordinary retained children, each filling the native FlipView
viewport. Native arrow/keyboard selection writes `position` before firing
`on_position_changed`, followed by `on_current_item_changed`. Removing the host
from the first callback cancels the second; unrelated mutation does not.
`set_position` and nonnegative `scroll_to` remain silent. With `wraps` (the
default), indexes wrap modulo the page count, including negative `set_position`
indexes. With `wraps: false`, positions clamp to available pages. An empty
carousel reports position zero. Insertions honor item, numeric-offset,
and pinned-last anchors. Replacing a selected page keeps its index.

Scroll notifications and remaining-item threshold callbacks are deferred outside
native layout. Threshold delivery is suppressed while already in the same band
and re-evaluated after count/position/threshold changes. Pending delivery is
cancelled when the host is removed. The native pager supports one page at a
time. Native arrows and touch stop at the ends; applications can implement
circular Previous/Next buttons by setting position minus/plus one. Peek,
multi-column and bounce controls remain work.

`animates_scroll` controls native adjacent-page transitions for programmatic,
mouse and keyboard navigation. Longer jumps are immediate. Disabling it leaves
touch transitions native, as documented by WinUI's
[UseTouchAnimationsForAllNavigation](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.controls.flipview.usetouchanimationsforallnavigation).
The native regression probe measures intermediate offset and page geometry
with animation enabled, and checks an immediate settled offset when disabled.

Use the explicit host for this backend. `facet_runtime` still selects
Win32 on Windows. Before selecting this backend through that facade, implement
its host/window lifecycle, services, dialogs, clipboard and inspection seams.
Only one backend and one WinUI Application run are supported per process.

Initial unsupported trees return E_NOTIMPL. Internal native failures abort with
an HRESULT diagnostic instead of continuing with invalid handles. Full error
recovery and broader property/control support are listed in
[MANIFEST.md](../MANIFEST.md).


Menu construction uses retained children: add `menu_item` or `context_menu_item`
to a `menu` launcher. A `context_menu` host accepts both visible children and
flyout actions, in any insertion order. The action list keeps its own filtered
order, while visible children use normal Facet layout. Menu `count` tracks
native actions and updates when items are inserted or removed. Toolbar items
are ordinary native buttons suitable for a row; priority/placement and nested
menus are not implemented yet.

All action kinds support text, named/bitmap icons, destructive foreground and
restoration to theme defaults, enablement, and `on_clicked`. An explicit
accessibility label takes precedence over the toolbar's text-derived name.

A context-menu item's shortcut supports letters, digits, F1–F24, Enter/Return,
Escape/Esc, Space, Tab, Backspace, Delete, Insert, Home, End, arrows, PageUp and
PageDown. Empty or unsupported names remove the registration. Facet currently
represents a single modifier: Alt, Ctrl, Shift, Windows/Cmd, or None. Cmd maps
to the Windows key, matching the Win32 backend. Shortcut updates are live and
the displayed menu hint follows the key/modifier. Registrations are removed
when the action detaches, before its callback context is freed.

Ordinary menu shortcuts are global; a context-menu or swipeable host restricts
its shortcuts to focus inside that host. This uses native
[KeyboardAccelerator scope](https://learn.microsoft.com/en-us/windows/apps/develop/input/keyboard-accelerators).
The registration lives on the visible host to work before its flyout is first
opened. Invocation follows the normal action delivery path and marks the native
key handled before application code runs. Disabled or hidden actions/ancestors
do not dispatch the callback.


Tree custom `row` factories produce retained Facet subtrees. A binding refresh
with the same factory and `row_kind` invokes the binder on the existing row,
preserving controls and local edits. A changed kind or factory rebuilds that
row. Without a binder, a row refresh calls the factory again. Replacing the
root model rebuilds all rows. The legacy binder signature is
`fn(model: *TreeNode, ctx: *u8, row: *Node)`; use a free adapter when forwarding
to a component method.

Nested control callbacks use `component::item_of(sender)` to resolve the
current model by stable node ID, including when a model has been replaced
before the renderer updates its native rows. A removed ID resolves to null.
Rows are retained eagerly for every model node, even collapsed branches;
native container virtualization does not imply a Facet subtree reuse pool.

Tree `row_id` names the native TreeViewItem automation peer for either plain
or custom content. Names refresh on `set_row_id` and root replacement; native
layout reapplies cached names to newly realised or recycled containers. The
application callback runs during apply, not during native layout. Resetting it
to `tree::no_row_id` clears names without rebuilding retained row content.
