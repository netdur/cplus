# AppKit binding gaps

`vendor/appkit` covers a practical core of Cocoa controls, views, windows,
menus, panels, tables, toolbars, layout, drag-and-drop, and basic drawing. It
is not complete AppKit coverage. This document lists the missing work by user
impact, rather than attempting to enumerate every Apple framework symbol.

## Priority 1: make ordinary desktop applications practical

### C+-native API surface

The package is largely a thin Objective-C-shaped binding. Its public API should
be aligned with `naming_guideline.md`:

- Public methods accept and return `str` or `Text`, rather than `*u8` and
  parallel `_ns` variants.
- Types describe their role. A configured `NSTextField` used as a label should
  be `Label`, not `TextField::new_label`.
- Rename Objective-C-shaped methods such as `set_string_value` and
  `index_of_selected_item` to intent-oriented names such as `set_text` and
  `selected_index`.
- Use labeled parameters and defaults where they improve the call site.
- Replace sentinel/null results with `Option`, `Result`, or `Status`.
- Hide Objective-C object handles behind private fields; keep explicit raw
  escape hatches only where interop needs them.

### Delegates, data sources, and callbacks

Only a small number of delegate/data-source synthesis helpers exist. Add typed
helpers for:

- `NSTextView` and text-field editing/validation delegates.
- `NSOutlineView` child/count/value delegate and data-source methods.
- `NSCollectionView` item provisioning, selection, and layout callbacks.
- Rich `NSTableView` provisioning, selection, editing, sorting, row views, and
  drag/reordering.
- `NSSplitView`, `NSTabView`, `NSBrowser`, `NSPopover`, toolbar, menu, and
  panel delegates.

The current raw `set_delegate(*u8)` escape hatch remains useful, but it should
not be the normal path for an application author.

### Application and window lifecycle

- Launch, activation, deactivation, reopen, hide/unhide, and termination
  delegate callbacks.
- First-responder management and window delegate events for resize, move,
  focus, fullscreen, and close.
- Sheets and modal-session APIs, including completion callbacks.
- Minimize, zoom, fullscreen, tab groups, restoration, titlebar accessories,
  screen/backing-scale queries, and window-state getters.
- Document-based application support (`NSDocumentController` and related APIs).

### Events and interaction

`events` currently introspects an event once AppKit has delivered it. Add:

- Local and global event monitors.
- Responder callbacks for key, mouse, scroll, magnify, rotate, and pressure
  events.
- Gesture recognizers, tracking areas, cursor management, and first-responder
  APIs.
- Complete drag-and-drop sessions: pasteboard items, promised files, previews,
  validation, source callbacks, and destination metadata.

## Priority 2: commonly expected platform services

### Foundation and system data

AppKit applications need a small Foundation layer. Add safe wrappers for:

- `NSURL` / file URLs, `NSDate`, `NSError`, `NSBundle`, and process metadata.
- `NSUserDefaults`, `NSFileManager`, and directory/file coordination.
- Dictionaries, sets, ordered sets, attributed strings, and mutable data.
- Timers, run-loop scheduling, undo management, and operation queues.

### Text system

- `NSTextStorage`, `NSLayoutManager`, and `NSTextContainer`.
- Attributed text, ranges/selections, text attachments, find/replace, spelling,
  grammar, and text completion.
- Text-field and text-view delegate helpers with `Option`/`Result` outcomes.

### Dialogs, panels, and sharing

- Async sheet variants for alerts, open/save panels, page layout, and printing.
- Accessory views, allowed content types, directory/filename configuration, and
  selected URLs as typed values.
- `NSSharingServicePicker`, color/font panels, and help panels.

### Menus, commands, and toolbars

- Ergonomic menu-item actions and validation callbacks.
- Contextual menus, menu delegates, alternate items, and key-equivalent
  configuration.
- Modern toolbar item providers, customization, search/group items, and
  Touch Bar delegate/item provisioning.

## Priority 3: richer layout, drawing, and appearance

### Views and Auto Layout

- View frame/bounds getters, subview removal/reordering, alpha, tooltips,
  appearance, and safe-area/layout-margin APIs.
- Bulk constraint activation/deactivation, identifiers, multipliers, visual
  format constraints, content hugging/compression priorities, and layout-guide
  anchors.
- Constraint errors expressed through `Result`/`Status` rather than raw handles
  where ownership or activation can fail.

### Graphics and animation

- `NSGraphicsContext`, gradients, shadows, transforms, compositing, clipping,
  image representations, and PDF export/drawing.
- Core Animation (`CALayer`, transactions, animations) for layer-backed views.
- More complete `NSBezierPath` and `NSImage` APIs, including image loading and
  symbol/image configuration as owned wrappers.

### Controls and containers

- Control-specific APIs currently exposed only as a minimal subset: editable
  combo boxes, segmented-control configuration, level indicators, date pickers,
  path controls, search fields, color wells, and popovers.
- Additional native controls and panels where demand justifies them, including
  scrubbers, sharing pickers, font/color panels, and modern symbol buttons.

## Cross-cutting requirements

### Ownership

Many wrappers deliberately remain `opaque` even when they are created with
`alloc`/`init`. Complete the documented +1 ownership model consistently:

- Every owned wrapper releases exactly once in `drop`.
- Factory/singleton/parent-owned handles stay non-owning.
- Child-to-parent transfers have explicit `into_raw`/`from_raw` or equivalent
  ownership APIs.
- Delegate and observer lifetimes are represented by owned C+ values.

### Accessibility, localization, and appearance

- Accessibility roles, labels, values, actions, notifications, and custom
  elements.
- Localization and right-to-left layout support.
- Appearance/theme observation, high contrast, reduced motion, and dynamic
  system colors.

### Facade consistency

The umbrella facade should make a deliberate choice for every module. At
present, `layout`, `events`, `drag`, and `convert` require direct imports while
most other modules are re-exported. Either re-export the common cross-cutting
modules or document the import boundary consistently in the facade and README.

## Suggested implementation order

1. Finish the C+-native naming and data/ownership surface for existing APIs.
2. Add typed delegate/data-source synthesis for text, table, outline, and
   collection views.
3. Add lifecycle, sheets, event monitoring, and responder APIs.
4. Add Foundation wrappers for URLs, preferences, filesystem access, errors,
   timers, and attributed text.
5. Expand window/layout/graphics APIs, then accessibility and less-common
   controls based on application demand.
