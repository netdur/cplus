# Experimental backend coverage

Windows x64, one window and one WinUI Application run per process. The explicit
host is `facet_winui/facet_winui::run`. The shared gallery's 36 pages mount;
this is not a claim that every Facet property is implemented.

| Contract | Implemented scope |
|---|---|
| Containers/layout | Border with Canvas document, Facet/flex_layout frames, native leaf measurement, resize, z-order and display/visibility. |
| Text | TextBlock, font size/weight/italic/color, font family, wrapping/max lines, selectable text, formatted runs and standalone spans. |
| Actions | Button, initial toggle-button mode, text buttons, compact icon buttons and click callbacks. Bundled Material glyphs and gallery SF-name aliases. |
| Inputs | TextBox, PasswordBox, search and multiline entry, max length, read-only, placeholder, native edits before callback, programmatic edit suppression. Caret/selection sampled on text edits. |
| Values | CheckBox, ToggleSwitch, RadioButton, Slider, NumberBox, ProgressBar, ProgressRing and corresponding change callbacks. |
| Pickers | ComboBox string items/selection, CalendarDatePicker date/min/max, TimePicker time, change callbacks. |
| Scrolling | ScrollViewer document extent, axes/bars, initial/live offsets, actual offset observers and native wheel scrolling. |
| Split/tree | Draggable Thumb divider, limits/collapse layout, TreeView hierarchy, expansion and invoked selection. |
| List/collection/relist | ScrollViewer with a viewport-sized set of owned Facet rows. Count/builder/bind, uniform row pitch, columns, scroll-to. Relist observable changes, keyed live rows, inherited sender item identity, deterministic release. |
| Images | BitmapImage and ImageBrush, file/URL source, fit/fill/center modes. Bundled icon font staged with the app. |
| Drawing | Native shapes for recorded rectangles/rounded rectangles, ellipses, lines and move/line/quad/cubic paths; text, fills, gradients, stroke width and opacity. |
| Decorations | Semantic/theme colors in dark appearance, borders/corners, gradients, 2D transforms, frame-synchronized opacity/transform easing, composition rounded/ellipse clips and drop shadows. |
| Other gallery hosts | Page dots, table document container, WebView2 URL surface, swipe actions through native context menu. |
| Services | UI-thread delayed callbacks/cancellation, size observers, renderer dispatch, dark-theme query. |
| Accessibility | Native WinUI peers, stable control AutomationIds, explicit accessibility name/help text. |
| Ownership | Native records owned by nodes, event removal before freeing callback contexts, row unrealise before drop, timers cancelled on release, zero view/subscription assertions on shutdown. |

## Limits

- `facet_runtime` still selects Win32. Multiwindow host services, dialogs,
  clipboard, jobs/worker-thread dispatch, Facet agent/inspection integration,
  general gesture/key readers and focus commands remain separate work.
  `run_on_main` currently queues only from the UI thread.
- List/collection selection, grouping, reorder and variable-height layout are
  not implemented. The row window is rebuilt when its range/data changes;
  it does not yet retain a type-based reuse pool. Relist's current adapter uses
  uniform pitch, with a stated first-row height when supplied.
- Tree custom row factories/binding and multiple-selection semantics are not
  implemented. Selection is delivered on invocation.
- Carousel is a content host only; paging is not implemented. The existing
  gallery carousel sample intentionally supplies no pages. Swipe is a desktop
  context menu, not touch swipe; refresh demo's explicit button works.
- Text submit, selection-only notifications/setters, keyboard mapping and
  clear-button policy are not mapped. Changing secure or toggle control class
  after creation is not supported. Resetting overridden fonts to defaults and
  several per-control styling properties remain incomplete.
- Symbol fill-axis variation, arbitrary platform icon names, image completion
  notifications, rich-text links/decorations and WebView navigation callbacks
  are not wired. WebView requires the installed WebView2 runtime.
- Canvas supports the gallery's recorded drawing subset, not all DrawOps:
  general clip/save/restore/blend/dashes/arc/image commands need more work.
- Appearance is dark; OS appearance changes, native system palette tracking,
  3D transforms, per-corner composition clipping, accessibility headings,
  tooltips and inherited container enable state are not complete.
  Shadows use rectangular masks on children with a native panel parent;
  root-window shadows and arbitrary shape masks are not mapped.

Initial unsupported trees/adopted views/children under leaf controls return
E_NOTIMPL. Unsupported kinds added later and failed native HRESULTs produce a
fatal diagnostic. Runtime error recovery is not yet implemented.
