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
```

## Modules

- `runtime`: geometry types, NSString helper, and ObjC runtime/message helpers
- `application`: `AutoreleasePool`, `Application`, app delegate helper
- `window`: `Window`
- `view`: `View`, `StackView`, `ScrollView`, `Box`, `Scroller`, `BackgroundExtensionView`
- `controls`: `TextField`, `Button`, `Slider`, `ProgressIndicator`, `PopUpButton`, `Stepper`, `Switch`, `SegmentedControl`, `ComboButton`, `DatePicker`, `ColorWell`, `LevelIndicator`, `PathControl`
- `text`: `TextView`, `SecureTextField`, `SearchField`, `TokenField`, `ComboBox`, `Form`
- `containers`: `SplitView`, `TabView`, `TabViewItem`, `VisualEffectView`, `GridView`, `Browser`, `Matrix`, `ClipView`, `RulerView`, `Popover`
- `data`: `TableView`, `TableColumn`, `OutlineView`, `TableCellView`, `TableRowView`, `CollectionView`, `CollectionViewItem`, `CollectionViewFlowLayout`, `CollectionViewGridLayout`, `RuleEditor`, `PredicateEditor`
- `graphics`: `ImageView`, `Image`, `Font`, `Color`
- `menu`: `Menu`, `MenuItem`
- `dialogs`: `Alert`
- `panels`: `Panel`, `SavePanel`, `OpenPanel`, `PageLayout`, `PrintPanel`
- `toolbar`: `Toolbar`, `ToolbarItem`, `ToolbarItemGroup`, `StatusBar`, `StatusItem`, `StatusBarButton`, `TouchBar`, `TouchBarItem`
- `controllers`: `ViewController`, `WindowController`, `TabViewController`, `SplitViewController`, `ArrayController`, `ObjectController`
- `convert`: C+ ↔ Cocoa data bridges. `cplus_{str,string}_to_nsstring` / `nsstring_to_cplus_{string,str_unsafe}`; `nsarray_count` / `nsarray_obj_at` / `nsarray_to_vec_{i32,i64,f32,f64}`; `nsdata_to_vec_u8` / `vec_u8_to_nsdata` / `vec_u8_to_nsdata_view`. Import directly via `import "appkit/convert" as bridge;` — there is no facade re-export.

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
    window.set_title(str_ptr("C+ App\0"));
    window.center();
    
    let btn = appkit::Button::new(frame);
    btn.set_title(str_ptr("Click Me\0"));
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
