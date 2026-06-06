# appkit

C+ bindings for Apple's Cocoa/AppKit framework.

## Usage

Add `appkit` dependency in your `Cplus.toml`:

```toml
[dependencies]
appkit = "*"
```

Import the compatibility facade in your source file:

```cplus
import "appkit/appkit" as appkit;
```

Or import narrower modules directly:

```cplus
import "appkit/runtime" as appkit;
import "appkit/application" as application;
import "appkit/window" as window;
import "appkit/view" as view;
import "appkit/controls" as controls;
import "appkit/text" as text;
import "appkit/containers" as containers;
import "appkit/data" as data;
import "appkit/graphics" as graphics;
import "appkit/menu" as menu;
import "appkit/dialogs" as dialogs;
import "appkit/panels" as panels;
import "appkit/toolbar" as toolbar;
import "appkit/controllers" as controllers;
import "appkit/pasteboard" as pasteboard;
import "appkit/layout" as layout;
import "appkit/events" as events;
import "appkit/notifications" as notifications;
```

## Modules

- `runtime`: geometry types, NSString helper, ObjC runtime/message helpers, and the ownership primitives (`retain`/`release`/`autorelease`/`retain_count`)
- `application`: `AutoreleasePool`, `Application`, app delegate helper
- `window`: `Window`. Also `create_window_delegate(should_close_imp, will_close_imp)` — synthesizes an `NSWindowDelegate` (`windowShouldClose:` / `windowWillClose:`).
- `view`: `View`, `StackView`, `ScrollView`, `Box`, `Scroller`, `BackgroundExtensionView`. Also `create_custom_view(frame, draw_imp)` — synthesizes an `NSView` subclass whose `drawRect:` is a C+ function (custom drawing).
- `controls`: `TextField`, `Button`, `Slider`, `ProgressIndicator`, `PopUpButton`, `Stepper`, `Switch`, `SegmentedControl`, `ComboButton`, `DatePicker`, `ColorWell`, `LevelIndicator`, `PathControl`
- `text`: `TextView`, `SecureTextField`, `SearchField`, `TokenField`, `ComboBox`, `Form`
- `containers`: `SplitView`, `TabView`, `TabViewItem`, `VisualEffectView`, `GridView`, `Browser`, `Matrix`, `ClipView`, `RulerView`, `Popover`
- `data`: `TableView`, `TableColumn`, `OutlineView`, `TableCellView`, `TableRowView`, `CollectionView`, `CollectionViewItem`, `CollectionViewFlowLayout`, `CollectionViewGridLayout`, `RuleEditor`, `PredicateEditor`. Also `create_table_data_source(row_count_imp, value_imp)` (synthesizes an `NSTableViewDataSource`), `create_table_delegate(selection_changed_imp)` (selection via `tableViewSelectionDidChange:`), and `TableView::selected_row()`. The synthesis helpers are the documented delegate pattern (same shape as `create_app_delegate`).
- `graphics`: `ImageView`, `Image`, `Font`, `Color`, `BezierPath` (move/line/close, append rect/oval, line width, `element_count`, stroke/fill — the path primitive for custom drawing)
- `menu`: `Menu`, `MenuItem`
- `dialogs`: `Alert`
- `panels`: `Panel`, `SavePanel`, `OpenPanel`, `PageLayout`, `PrintPanel`
- `toolbar`: `Toolbar`, `ToolbarItem`, `ToolbarItemGroup`, `StatusBar`, `StatusItem`, `StatusBarButton`, `TouchBar`, `TouchBarItem`
- `controllers`: `ViewController`, `WindowController`, `TabViewController`, `SplitViewController`, `ArrayController`, `ObjectController`
- `pasteboard`: `Pasteboard` — the system clipboard (general pasteboard string read/write). Re-exported from the facade.
- `layout`: Auto Layout. Anchor getters (`leading`/`trailing`/`top`/`bottom`/`width`/`height`/`center_x`/`center_y`/…), constraint builders (`equal`/`equal_offset`/`ge`/`le`, `equal_const`/`ge_const`/`le_const`), `activate`/`deactivate`/`is_active`, constraint priorities (`set_priority`/`priority` + `priority_required`/`priority_high`/`priority_low`), and `add_guide` (NSLayoutGuide). Operates on raw view/anchor/constraint handles; import directly.
- `events`: `NSEvent` introspection. `NSEventType` and `NSEventModifierFlags` constants, `has_modifier`, and accessors (`event_type`, `modifier_flags`, `key_code`, `location_in_window`, `characters_ns`, …). Import directly.
- `notifications`: `NotificationCenter` (post + `add_observer`) and `Observer`. `add_observer` returns an `Observer` that owns the subscription — keep it alive; dropping it unsubscribes (`removeObserver:`) and releases. Re-exported from the facade.
- `drag`: drag-and-drop. Destination: `create_drag_destination_view(frame, entered_imp, perform_imp)` (an `NSView` that accepts drops), `register_for_string_drops`, `dragged_string_ns`. Source: `create_drag_source_view(frame, op_mask_imp)` (an `NSView` declaring its allowed operations). Plus the `NSDragOperation` constants (`drag_op_copy`/`drag_op_move`/…). Import directly. (Initiating a live drag — `beginDraggingSession…` from a mouse handler — is left to the app.)
- `convert`: C+ ↔ Cocoa data bridges. `cplus_{str,string}_to_nsstring` / `nsstring_to_cplus_{string,str_unsafe}`; `nsarray_count` / `nsarray_obj_at` / `nsarray_to_vec_{i32,i64,f32,f64}`; `nsdata_to_vec_u8` / `vec_u8_to_nsdata` / `vec_u8_to_nsdata_view`. Import directly via `import "appkit/convert" as bridge;` — there is no facade re-export.

## Ownership

ObjC objects are reference-counted. A wrapper that **owns** its object holds one
strong reference and releases it once in `fn drop` ("+1 normal form"):

- `alloc`/`init`, `new`, `copy` already return +1 — hold it, release in `drop`.
- convenience constructors return an autoreleased +0 object — `rt::retain` it on
  capture to reach +1, then release in `drop`.
- a handle the wrapper does **not** own (a shared singleton like the general
  `Pasteboard`, or a child view its parent retains) stays `opaque` and has no
  `drop` — releasing it would over-release.

`self`/`mut self` method receivers are borrows (they do not consume), so builder
chaining works on owned wrappers. Keep an owned wrapper alive as long as the UI
needs its object; do not drop a top-level object that nothing else retains while
it is still on screen, and don't pass `Widget::new().obj` inline (the temporary
wrapper would drop and dangle the pointer) — hold it in a local across the
`addSubview:`.

**Owned (release in `drop`):** `Alert`, `Button`, `TextField`, `ScrollView`,
`TableView`, `TableColumn`, `Menu`, `MenuItem`, `BezierPath`, `Observer`.

**`opaque` (not owned — managed elsewhere):** top-level windows
(`Window`/`Panel`), shared/factory/pool singletons (`Application`,
`AutoreleasePool`, `NotificationCenter`, `Pasteboard`, the `*Panel` factories),
and the `Color`/`Font`/`Image` namespaces. The remaining child-widget wrappers
are still `opaque`; they follow the same `+1` pattern and gain a `drop` as apps
need them.

## Example

```cplus
import "appkit/appkit" as appkit;

fn on_button_click(sender: *u8) {
    // Handle click
}

fn main() -> i32 {
    let pool = appkit::AutoreleasePool::new();
    let app = appkit::Application::shared();
    
    app.set_activation_policy(0 as i64); // Regular app
    
    let frame = appkit::Rect {
        origin: appkit::Point { x: 0.0, y: 0.0 },
        size: appkit::Size { width: 800.0, height: 600.0 }
    };
    
    let window = appkit::Window::new(frame, 15 as u64, 2 as u64, 0 as i8);
    window.set_title(#str_ptr("C+ App\0"));
    window.center();
    
    let btn = appkit::Button::new(frame);
    btn.set_title(#str_ptr("Click Me\0"));
    btn.set_on_click(on_button_click);
    
    let content = window.content_view();
    // Add subviews, stack views, etc.
    
    window.make_key_and_order_front(0 as *u8);
    app.activate(1 as i8);
    app.run();
    
    pool.drain();
    return 0;
}
```
