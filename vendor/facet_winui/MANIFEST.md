# First-slice coverage

Experimental, Windows x64, one window and one WinUI Application run per process.
The explicit host is `facet_winui/facet_winui::run`.

| Contract | Implemented scope |
|---|---|
| Renderer create/release/apply | Canvas for K_NODE, TextBlock for label, Button for button. Native records owned by Facet nodes. |
| Insert/remove | Ordered IVector child insertion and identity-based removal for Canvas containers. |
| Layout | Facet/flex_layout frames, parent-relative Canvas positioning, intrinsic label/button measurement. |
| Resize | WinUI SizeChanged relays logical dimensions into Facet layout. |
| Dirty synchronization | Coalesced DispatcherQueue callback; clears its pending flag before syncing. |
| Labels | Text and positive font size; native defaults when initially unspecified. |
| Buttons | Title, positive font size, enabled, click callback. |
| Shared properties | Opacity, visibility, input transparency. |
| Sender readers | Key and item pointer for button callbacks. |
| Lifetime | Unsubscribe before freeing record/context; supports removal during a click callback. |
| Shutdown | Unmount, remove window events, disconnect routing, release records before apartment teardown. |

Unsupported initial node kinds, adopted native views, and children under leaf
controls are rejected before starting the UI. Adding an unsupported kind to a
live tree is a fatal diagnostic in this experimental implementation.

Not implemented: TextBox/editing, scrolling, remaining controls, images,
brushes/colors and other decorative properties, button toggle state,
pressed/released events, gesture/key readers, focus commands, menus/dialogs,
clipboard, timers, worker dispatch, size observers, theme hooks, navigation,
multiwindow lifecycle, or the Facet agent/inspection backend. Native WinUI
accessibility is used by the test harness; it does not provide Facet's agent
integration. Top-level HWND interop is also deferred.

Current font setters apply positive values; resetting font size to the portable
default after overriding it is not implemented. The shared enabled property
is implemented for buttons, not inherited enable/disable of container subtrees.

WinMD includes DispatcherQueueTimer for future work, but that is not a claim
that Facet's scheduling services are installed. Only renderer sync is wired.

Native HRESULT failures inside the renderer are fatal with a diagnostic;
recoverable renderer error propagation remains future work. The host returns
an HRESULT Status for startup failure, unsupported initial trees, and repeated
run attempts. It does not change `facet_runtime/runtime_windows.cplus`.
