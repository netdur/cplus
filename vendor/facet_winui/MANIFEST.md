# Experimental backend coverage

Experimental, Windows x64, one window and one WinUI Application run per process.
The explicit host is `facet_winui/facet_winui::run`.

| Contract | Implemented scope |
|---|---|
| Renderer create/release/apply | Canvas, TextBlock, Button, TextBox for text_field, ScrollViewer for scroll. Native records owned by Facet nodes. |
| Insert/remove | Ordered IVector child insertion and identity-based removal for Canvas containers and the scroll document Canvas. |
| Layout | Facet/flex_layout frames, parent-relative Canvas positioning, intrinsic label/button/text-field measurement. Scroll extent includes direct child frames. |
| Resize | WinUI SizeChanged relays logical dimensions into Facet layout. |
| Dirty synchronization | Coalesced DispatcherQueue callback; clears its pending flag before syncing. |
| Labels | Text and positive font size; native defaults when initially unspecified. |
| Buttons | Title, positive font size, enabled, click callback. |
| Text fields | Text, placeholder, read-only, max length, spelling/prediction flags, positive font size, enabled, on_text_changed. Native edits update Facet props before callback. Equal text is not reassigned, preserving the caret on unrelated updates. |
| Scroll | Axes, scrollbar visibility, initial/live offsets, content_size, native scrolling, observe_scrolled. The native viewport clips its document. |
| Shared properties | Opacity, visibility, input transparency. |
| Sender readers | Key and item pointer for control callbacks. |
| Lifetime | Unsubscribe before freeing record/context; supports removal during a click callback. |
| Shutdown | Unmount, remove window events, disconnect routing, release records before apartment teardown. |

Unsupported initial node kinds, adopted native views, and children under leaf
controls are rejected before starting the UI. Adding an unsupported kind to a
live tree is a fatal diagnostic in this experimental implementation.

Not implemented: secure/password fields (rejected), text_area, text submit,
selection/caret setters and selection-only readback, text alignment, keyboard
mapping, clear-button policy, remaining controls, images,
brushes/colors and other decorative properties, button toggle state,
pressed/released events, gesture/key readers, focus commands, menus/dialogs,
clipboard, timers, worker dispatch, size observers, theme hooks, navigation,
multiwindow lifecycle, or the Facet agent/inspection backend. Native WinUI
accessibility is used by the test harness; it does not provide Facet's agent
integration. Top-level HWND interop is also deferred.

Current font setters apply positive values; resetting font size to the portable
default after overriding it is not implemented. The shared enabled property
is implemented for buttons and text fields, not inherited enable/disable of container subtrees.

Scroll content must have a Facet layout extent larger than the viewport; use
an explicit/content-sized child with shrink(0) when appropriate. The adapter
does not impose new flex rules or traverse overflow outside a direct child's
frame. Nested scroll chaining and cascades_input are not mapped. Desktop tests
cover vertical wheel input and offsets; horizontal/touch input has not been
separately exercised. Programmatic text updates do not invoke on_text_changed;
scroll observers report actual native offset changes.

WinMD includes DispatcherQueueTimer for future work, but that is not a claim
that Facet's scheduling services are installed. Only renderer sync is wired.

Native HRESULT failures inside the renderer are fatal with a diagnostic;
recoverable renderer error propagation remains future work. The host returns
an HRESULT Status for startup failure, unsupported initial trees, and repeated
run attempts. It does not change `facet_runtime/runtime_windows.cplus`.
