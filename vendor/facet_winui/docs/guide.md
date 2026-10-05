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
the tree and every owned native reference. Each window has a host record,
owned by its window and freed on Window.Closed; the process keeps only the
list of open ones. Window events carry their host record as context and check
it is still listed; queued renderer callbacks capture no host pointer.

## Windows

One WinUI Application runs per process, with any number of windows in it.
Windows can only be made once Application.Start has launched, so
`host::start(on_launch)` opens the first ones from its callback and
`host::open_window` opens more at any later point on the UI thread. One
renderer serves every window: a renderer request relays out all of them on
the next queue turn. Closing a window unmounts only its tree; closing the last
also retires the timers, the size observers and the composition retirement
queue, and ends the run. `run(tree, title)` is the one-window spelling.

Chrome follows `screen::Chrome` (table in the [reference](ref.md#chrome)). A
`Bar::Blended` window whose tree draws `window_buttons()` loses the system
caption entirely, as on GTK, so the app's three buttons are the only set;
its `.window_drag()` regions move the window. The client size asked for is
the client size delivered, measured after the caption is gone.

## Through facet_runtime

An app written against `facet_runtime` keeps `runtime::App` and opts in:

```cplus
import "facet_runtime/winui" as winui;
winui::select();          // before app.run(...)
```

`App::run` then opens one WinUI window per `open_window`, queuing the ones
opened before launch, and `runtime::alert`, `choose`, `prompt` and
`alert_blocking` become facet sheets on the active window's modal layer, keyed
as on every backend (`alert:title`, `alert:primary`, `prompt:value`, ...), so
an agent answers them the same way. The app lists `facet_winui`, `winui` and
`winrt` itself; Win32-only apps never compile them. See
[facet_runtime](../../facet_runtime/README.md#windows-win32-or-winui) and
`examples/facet_runtime_winui`.

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
application callback and completes the native deferral. Lists also support
`refreshable`, `refreshing`, `refresh_color`, `begin_refresh()` and
`end_refresh()` directly. `set_refresh(false)` disables user requests without
disabling row controls or programmatic refresh. The List gallery page exercises
both commands and the native Refresh menu while preserving edited rows. Native
pull gestures require supported touch input and scrollable content; the desktop
probe verifies requests and lifecycle but does not simulate physical touch.
Refreshable owns a scroll source so it can contain a list with its own refresh
container. This prevents WinUI's duplicate interaction-source ownership error;
the nested controls keep independent refresh state and deferrals.
See Microsoft's [pull-to-refresh guidance](https://learn.microsoft.com/en-us/windows/apps/develop/ui/controls/pull-to-refresh).

Swipe action items appear as native buttons when the row is dragged left, and in a native context menu. Both `on_clicked` and
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

Only one backend and one WinUI Application run are supported per process.
Through `facet_runtime` the clipboard and file pickers stay the shared Win32
calls, which need no window of their own; the app menu (`App::menu`) is not
rendered on WinUI yet.

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


List and collection viewport changes retain overlapping native rows. An explicit
binding refresh reuses rows with the same factory/context and `row_kind`, so
binders can update labels while leaving an editor's local state intact. Changing
a row's kind rebuilds only that row; changing the factory/context rebuilds the
visible content. Without a binder, data invalidation calls the factory again.
Retained list rows do not emit another appearing/disappearing pair. Rows leaving
the retained viewport still receive deferred disappearance and are released;
there is no cross-item reuse pool yet. Relist keeps its observable-row protocol.

Lists and collections support live grouping through `set_grouped`,
`set_group_count`, `set_group_size`, and `set_group_header`. Each group has a
header, including empty groups; data indexes remain flat across groups.
Selection, row binding, height callbacks and scrolling use data indexes and
exclude headers. Collection headers span every column, and each group's cells
start a new grid line. Headers use the list row height or the collection's
80-DIP default pitch. Header nodes are virtualized with the viewport and are
rebuilt when the group description changes. Group size/header callbacks have
their own contexts. Reordering remains separate, unfinished work.


Text fields, search fields and multiline editors map `text_transform` to a
Unicode display transform plus native character casing. A programmatic
transform changes the displayed copy and preserves the text the application
set; `Default` or `None` restores that source spelling. Real edits write the
native text back before `on_text_changed`. Apply caches the actual native
display so delayed TextChanged events, including native newline/length
normalization, do not turn a programmatic casing update into a user callback.
Selection remains expressed in native UTF-16 positions. Password contents
are left unchanged by display casing.

Popup fixed captions use `popup::Popup.set_label(...)`. A nonempty caption stays
on the closed native dropdown while selection and item callbacks retain their
normal meaning. Clear the caption to show the selected item or the placeholder
again. The Pickers gallery page includes a live toggle for this behavior.
Picker readback preserves pending application writes: popup selection, date and
time events from an older native value are ignored until that write is applied.

Plain text fields support live `clear_button` changes. `Never` removes the native
clear button from layout and hit testing; `WhileEditing` restores WinUI
behavior for focused, editable, nonempty fields. Clearing uses the native
TextBox action and normal text readback/callback. Secure PasswordBox fields
do not yet provide a clear button.

Split panes share one constraint calculation for layout and dragging. Trailing
minimum/maximum sizes exclude the divider thickness. When bounds conflict, the
leading floor wins, capped to the available host size. Collapse hides the pane
and divider without overwriting the saved position; expanding restores the
position subject to the current bounds. Divider thickness controls the visible
native line and layout gap; a transparent drag target remains at least six
points wide for thin lines. Zero thickness hides both line and drag target. Programmatic
layout changes are silent, and dragging against an unchanged bound does not
send duplicate move callbacks.

Ordinary buttons, icon buttons and text buttons support live `set_toggles`
changes. A mode change replaces the native Button/ToggleButton while keeping
the Facet node, sender identity, stored checked state, and callbacks. Styling
and accessibility metadata are reapplied. Native handles retained by application
code still refer to the old control; reacquire them after changing mode.
Closing before synchronization safely removes the subscriptions for the actual
native class.

Text fields support live `set_secure` changes. The backend replaces the native
TextBox/PasswordBox while retaining the Facet node, parent position, text,
styling and focus. Returning to plain mode restores the model's selection;
PasswordBox does not expose its caret/selection. Password contents bypass display
casing. Read-only secure fields remain disabled because PasswordBox has no native
read-only mode. Mode changes do not emit text-change callbacks.

Time pickers support `set_open`, `on_opened`, and `on_closed` through a native
TimePickerFlyout owned by the backend. Opening waits for the control to load;
dismissal updates `is_open`. Open/close callbacks are deferred so they may remove
the picker. The visible trigger retains the pinned SDK style, locale-aware time
display, and accessible caption. Alt+Up/Down opens it; Escape dismisses it.

Regenerate the bindings with `tools/generate_winui.ps1` and the adapted template
with `python tools/generate_time_template.py`. Check reproducibility with
`python tools/test_winui_generation.py` and
`python tools/generate_time_template.py --check`.

Collections support native drag/drop reordering when `reorder_items` is true.
Read `reorder_from()` and `reorder_to()` in `on_reorder_completed`, move that
item in the application's data, then refresh the rows through `set_row`,
`set_count` or binding invalidation. Both indexes are flat data indexes, excluding
group headers. The destination is the final index after removing the source.
The backend reports a move; it does not modify the application's data source.

Grouped collections keep moves inside their group unless `mix_groups` is true.
Self-drops and drops on headers or empty space are ignored. Disabling dragging,
changing the data/group/column policy, or removing the host cancels pending
moves. A completion callback may safely remove its collection. Escape cancels
the native drag. Edge auto-scrolling, keyboard reordering and list reordering
remain unimplemented; touch dragging has not been physically tested.

`hybrid_web` loads `default_file` relative to `hybrid_root` using WebView2.
The root may be absolute or relative to the process working directory. Both
properties update live; clearing either blanks the view. Use an application-owned
asset directory. Paths containing parent segments, backslashes, drive/stream
separators, or NUL bytes are rejected. The directory is not a filesystem sandbox
against junctions or symlinks deliberately placed inside it.

The page sends strings through `window.facet.postMessage(body)` and receives
them through `window.facet.onmessage = body => { ... }`. The native
`send_message` command uses WebView2's string-message API, preserving quotes,
newlines, Unicode and empty strings. A pending send waits for document loading;
multiple unsynchronized sends share Facet's one outgoing-message property, so
the latest value wins. In `on_raw_message_received`, read the borrowed text with
`facet_winui/hybrid::message(sender)` and copy it if it must outlive the callback.
Direct JavaScript object messages are exposed as JSON text.

`on_web_view_initializing` runs before the explicit engine initialization request;
`on_web_view_initialized` runs after the engine and document bridge are ready,
before loading the initial file. These lifecycle senders are Facet view records.
All four application callbacks are deferred and may remove the view.

Local GET/HEAD requests run through `on_web_resource_requested`, including
stylesheets and scripts. Its sender is a borrowed native
`ICoreWebView2WebResourceRequestedEventArgs` object: retain it with
`winrt/runtime::Object::retain(sender)` to query generated `winui` interfaces.
The default local-file response is already assigned; the callback may replace or
modify it before the request deferral completes. Missing files return 404,
invalid relative paths 403, and other HTTP methods 405. Common web asset MIME
types are mapped, with an octet-stream fallback.

Each explicit root/file load gets a fresh synthetic HTTPS origin. This rejects
queued messages from the previous document, but also means browser storage is
not preserved across those loads. Top-level navigation stays within the current
local origin; ordinary external browsing belongs in `web`. The local resource
callback does not intercept remote assets. Serving uses WebView2's
[resource-request mechanism](https://learn.microsoft.com/microsoft-edge/webview2/concepts/working-with-local-content),
so no local HTTP server is needed. Initialization failure recovery remains work,
as it does for the ordinary web host.

### Swipe reveal

Drag a swipeable row left to reveal its actions, then click an action or drag
right to close it. Escape cancels an active drag or closes an open strip. Native
context-menu and shortcut actions remain available. Horizontal motion must pass
8 DIPs before capture; vertical motion yields before capture. The content panel
moves independently of the application's transforms, with clipping at the row.

`reveal_threshold` is read live when the pointer is released. Zero, negative or
NaN selects half the action strip; larger-than-strip values clamp to its width.
Action slots are 88 DIPs, compressed to fit narrow rows. The native reveal buttons
follow live text, icon, destructive, enabled and visibility changes. Hidden actions
are omitted from the strip width. Accessible names, hints and tooltips are shared
with the action node, with its text as the default accessible name.

Callbacks run outside native pointer dispatch and may remove the row. Position
updates can coalesce. `on_swipe_started` and `on_swipe_ended` delimit an actual
drag, not a stationary click. Open/close-requested report its settled destination,
matching Facet's desktop backend contract; they are not MAUI's programmatic
Open/Close notifications. Cancellation restores the prior state and reports only
ended. Disabling a host or changing its action children closes the strip.

Settling is immediate. This implementation reveals right-side actions with a
leftward desktop drag; directional named slots and inertial animation are not
implemented. Physical touch and pen input have not been validated.

Revealed action buttons register distinct `@swipe:<key>` agent identities with
the action node's current policy; live privacy changes apply before native content
updates, and releasing the action unpins the auxiliary button.
