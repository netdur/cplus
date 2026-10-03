# Experimental backend coverage

Windows x64, one window and one WinUI Application run per process. The explicit
host is `facet_winui/facet_winui::run`. The shared gallery's 38 pages mount;
this is not a claim that every Facet property is implemented.

| Contract | Implemented scope |
|---|---|
| Containers/layout | Border with Canvas document, Facet/flex_layout frames, native leaf measurement refreshed after initial template load, resize, z-order and display/visibility. |
| Text | TextBlock, font size/weight/italic/color/family, spacing, text scaling, horizontal alignment, underline/strikethrough, wrapping/max lines, selectable text, formatted runs and standalone spans. Unicode display casing preserves source text; line height and inline decorations update live. Label font/color overrides restore native defaults when cleared. |
| Actions | Button, initial toggle-button mode, text buttons, compact icon buttons and click callbacks. Button/icon-button pointer press/release callbacks, including handled native events. Text-button typography, decoration, alignment and optional borders. Button image/symbol placement in all four content layouts, icon-image fit and border resets. Native toggle readback also works through automation. Bundled Material glyphs and gallery SF-name aliases. |
| Inputs | TextBox, PasswordBox, search and multiline entry, max length, read-only, placeholder, native edits before callback, programmatic edit suppression. UTF-16 caret/selection setters and readback, selection-only notifications, Enter submission (Ctrl+Enter for multiline), input font/spacing settings, keyboard scopes including secure numeric PIN entry. |
| Values | CheckBox, ToggleSwitch, RadioButton, Slider, NumberBox, ProgressBar, ProgressRing and corresponding change callbacks. Radio typography and native-default resets. Slider thumb drag-start/completion callbacks and normal-state minimum/maximum track colors with native-default resets. Progress color and frame-driven easing, retargeting and cancellation. |
| Pickers | ComboBox items/selection/alignment and per-item enabled state, CalendarDatePicker date/min/max, TimePicker time, change callbacks. All three map font/spacing/scaling/color with native-default resets. Popup title color maps separately from selected text; calendar date and bounds can be cleared. Date formats map native patterns and eight common numeric forms; time formats select native 12/24-hour display. Popup and calendar open/close events update state and defer user callbacks safely outside native apply. |
| Carousel | Native FlipView over retained child pages; initial/live position and scroll-to, circular programmatic indexes, adjacent-page animation switch, native position/current-item callbacks, scroll observer and scrolling readback, swipe enablement, bars, insertion/removal anchors and remaining-item threshold. |
| Tabs | Native TabView headers; child keys label persistent Facet panes. Initial/live selection, selection readback before callbacks, bar brush/color, text and selected/unselected fills with theme resets. |
| Scrolling | ScrollViewer document extent, axes/bars, initial/live offsets, actual offset observers, input chaining and native wheel scrolling. |
| Refreshable | Native RefreshContainer around scrollable content, refreshing state, indicator color/reset and deferred refreshing callback. Desktop Refresh context menu; finish by setting refreshing false. |
| Split/tree | Draggable Thumb divider, limits/collapse layout, TreeView hierarchy, expansion, single/multiple/none selection, selection clearing and native selection readback. Custom Facet rows, binding, row-kind replacement, live row automation IDs and nested sender model lookup. |
| List/collection/relist | ScrollViewer with a viewport-sized set of owned Facet rows. Count/builder/bind, columns, scroll-to, and cached variable list/collection heights supplied by row_height_of. Collection first/all-item sizing and tallest-item grid rows. List separator color/visibility, list/collection selection and highlights, tap/selection callbacks, scroll observers and bar policies. Deferred list appearance/disappearance callbacks preserve row sender lifetime. Collection item/offset/bottom anchoring follows inserts and removals. Collection threshold callbacks use the visible viewport and permit append/removal from callbacks. Relist observable changes, keyed live rows, inherited sender item identity, deterministic release. |
| Images | BitmapImage and ImageBrush, file/URL source, fit/fill/center modes. Retained image sources preserve animation across style updates; animated bitmap play/stop and source clearing are mapped. Bundled icon font staged with the app. |
| Drawing | Native shapes for recorded rectangles/rounded rectangles, ellipses, lines and move/line/quad/cubic paths; text, fills, gradients, stroke width and opacity. Canvas hover/move, primary pointer press/release, captured drag and cancellation callbacks. Drawn shapes do not intercept hit testing. |
| Decorations | Semantic/theme colors in dark appearance, borders/corners, gradient strokes, DIP dash patterns/phase, caps, joins, miter limits, rounded/per-corner/ellipse stroke outlines, gradients, 2D transforms, frame-synchronized opacity/transform easing, composition rounded/ellipse clips and drop shadows. |
| Menus and actions | Inline menu launchers with native MenuFlyout items; context-menu hosts with retained visible content; toolbar action buttons. Live text, icons, destructive foreground/reset, enablement, accessible toolbar names, click handlers and safe callback removal. Context-item keyboard shortcuts register on the visible host, including before the first menu opening; context actions are scoped. |
| Other gallery hosts | Circle/square page dots, table document container, swipe actions through native context menu, with image/named icons, destructive foreground/reset, stable interleaved insertion order, and clicked/invoked callbacks. WebView2 source/user-agent updates, history/back/forward, reload, script execution, navigation and process-failure callbacks. |
| Services | UI-thread delayed callbacks/cancellation, size observers, renderer dispatch, dark-theme query. |
| Agent integration | Optional lifecycle/apply/release hooks; `facet_agent_winui` connects keyed controls and privacy policies to `agent_winui`. In-app sessions and UI-dispatched MCP requests. |
| Accessibility | Native WinUI peers, stable control AutomationIds, explicit accessibility name/help text with native-default restoration, heading levels 1–9 and attached Unicode tooltips with live updates/removal. |
| Ownership | Native records owned by nodes, event removal before freeing callback contexts, row unrealise before drop, timers cancelled on release, zero view/subscription assertions on shutdown. |

## Limits

- `facet_runtime` still selects Win32. Multiwindow host services, dialogs,
  clipboard, jobs/worker-thread dispatch, general gesture/key readers and host
  focus commands remain separate work. Agent HTTP serving/inspection is
  available through the optional WinUI agent connector.
  `run_on_main` currently queues only from the UI thread.
- List/collection grouping and reorder are not implemented. Uneven list rows
  and variable-height collections require `row_height_of`; automatic content-driven
  measurement remains work. Nonpositive callback heights use the fallback pitch.
  The row window is rebuilt when its range/data changes;
  it does not yet retain a type-based reuse pool. Relist's current adapter uses
  uniform pitch, with a stated first-row height when supplied.
- Custom tree rows retain one Facet subtree per model node, including collapsed
  nodes; there is no viewport-based subtree reuse pool yet. `row_kind` controls
  replacement of each retained row. Positive row heights
  use native container styles; nonpositive values restore native measurement.
  Selection readback follows native selection notifications; direct
  external edits to WinUI's SelectedNodes vector can bypass those notifications.
- Menu nodes are inline launchers, not an application menu bar. Nested menus,
  menu priority, toolbar priority and primary/secondary placement remain unmapped.
  Flyout actions must be children of a menu, context-menu or swipeable host;
  context-menu hosts can also contain visible children.
- Action icons share the small named-icon mapping used by icon buttons; arbitrary
  platform symbol names are not supported. Destructive actions use the danger
  foreground in the normal state and retain native interaction-state feedback.
- Carousel currently uses native one-page-at-a-time FlipView behavior. Touch/native-arrow looping,
  peek insets, multiple columns, bounce policy, writing scrolling state,
  drag-state readback and header/footer/empty slots remain unmapped. Touch
  swiping has native enablement wired but has not been physically exercised. Positions
  wrap modulo the page count when `wraps` is true (the default), or clamp
  at the ends when false. Native arrows still stop at the ends. `animates_scroll`
  selects native adjacent-page animation for programmatic/mouse/keyboard changes;
  long jumps remain immediate and touch retains native transitions. Swipe
  actions use a desktop context menu, not touch swipe. List-local refresh properties are not wired;
  wrap the list in a `refreshable` host. Native pull gestures require supported
  touch input and a descendant ScrollViewer; physical touch remains untested.
- Clear-button and return-key policies are not mapped. PasswordBox has
  no caret/selection API. Changing secure or toggle control class
  after creation is not supported. Text inputs map horizontal/vertical alignment
  and reset font/color overrides. Several other per-control styling properties
  remain incomplete; TimePicker open/close callbacks are still unmapped. Date-format translation
  is limited to the numeric forms listed in the guide; other formats use WinUI
  grammar. Time-format support controls hour cycle only, not arbitrary layout
  or seconds.
- Symbol fill-axis variation, arbitrary platform icon names, image completion
  notifications and rich-text links are not wired. WebView requires
  the installed WebView2 runtime; hybrid-web resource/message hooks remain work.
- Border stroke outlines do not implicitly clip their content or reshape the
  background; use the separate clip property when clipping is wanted. Nonpositive
  miter limits restore the native default (10); negative dash lengths clamp to
  zero, and an all-zero dash pattern is treated as solid.
- Canvas caches recorded native shapes until explicit redraw, drawable changes
  or size changes. It supports the gallery's drawing subset, not all DrawOps:
  general clip/save/restore/blend/dashes/arc/image commands need more work.
- Toggle track/thumb colors, checkbox color and radio border color/thickness
  support initial and live updates and clearing back to theme defaults. These
  customize normal states, preserving native interaction-state feedback. Checkbox
  foreground and radio corner radius are not mapped.
- Slider track and thumb colors apply to the normal visual state; WinUI retains
  its theme feedback for hover, pressed and disabled states. Thumb color changes
  during a drag wait until release, then rebind the replacement native thumbs.
  Thumb images remain unmapped.
- WinUI uses trailing ellipsis for head/middle truncation and native wrapping
  for both word/character wrap.
- Appearance is dark; OS appearance changes, native system palette tracking,
  3D transforms, per-corner composition clipping and inherited container enable
  state are not complete.
  Shadows use rectangular masks on children with a native panel parent;
  root-window shadows and arbitrary shape masks are not mapped.

Initial unsupported trees/adopted views/children under leaf controls return
E_NOTIMPL. Unsupported kinds added later and failed native HRESULTs produce a
fatal diagnostic. Runtime error recovery is not yet implemented.

## Modifiers

```modifier
carousel.wraps                 changes normalization of initial/live position and nonnegative scroll-to indexes in paging::sync; modulo when true, clamped when false. The paging probe covers initial, negative, extreme, and live changes. Native arrows/touch still stop at the ends.
```
