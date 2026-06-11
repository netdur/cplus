# uikit

C+ bindings for UIKit (iOS), mirroring the organization of `vendor/appkit`.
The package uses raw ObjC-runtime FFI (`objc_getClass`, `sel_registerName`,
typed `objc_msgSend` declarations) with thin C+ structs over UIKit/Foundation
`id` pointers.

## Building

UIKit code only makes sense for the iOS targets, which stop at object
emission (Xcode owns the final link):

```
cpc build --target ios-arm64              # device
cpc build --target ios-arm64-simulator    # simulator
```

The consuming package declares `uikit = "*"` under `[dependencies]` and
builds as a `[lib]` staticlib. The `[link]` frameworks declared here (UIKit,
Foundation, libobjc) belong on the *external* link line in the Xcode project.

## Usage

Import the umbrella module:

```cplus
import "uikit/uikit" as ui;
```

Or import narrower modules directly:

```cplus
import "uikit/runtime" as rt;
import "uikit/application" as application;
import "uikit/screen" as screen;
import "uikit/window" as window;
import "uikit/view" as view;
import "uikit/controls" as controls;
import "uikit/text" as text;
import "uikit/containers" as containers;
import "uikit/data" as data;
import "uikit/graphics" as graphics;
import "uikit/dialogs" as dialogs;
import "uikit/toolbar" as toolbar;
import "uikit/pasteboard" as pasteboard;
import "uikit/layout" as layout;
import "uikit/events" as events;
import "uikit/notifications" as notifications;
```

## Modules

- `runtime`: geometry types, NSString helper, ObjC runtime/message helpers, class synthesis, associated-object callback storage, ownership primitives, and UIControl target-action helpers
- `application`: `UIApplicationMain` handoff and app delegate synthesis
- `screen`: `Screen`
- `window`: `Window`
- `view`: `View`, `Label`, `Color`, `StackView`, `ScrollView`, and `create_custom_view(frame, draw_imp)`
- `controls`: `Button`, `Slider`, `Switch`, `SegmentedControl`, `ProgressView`, `ActivityIndicator`, `PageControl`, `DatePicker`
- `text`: `TextField`, `SecureTextField`, `TextView`, `SearchBar`
- `containers`: `VisualEffectView`, `BlurEffect`, `TableViewCell`, plus aliases for stack/scroll and UIKit controller containers
- `data`: `TableView`, `CollectionView`, `CollectionViewCell`, `CollectionViewFlowLayout`, `PickerView`
- `graphics`: `ImageView`, `Image`, `Font`, `Color`, `BezierPath`
- `dialogs`: `AlertController`, `AlertAction`
- `toolbar`: `Toolbar`, `NavigationBar`, `BarButtonItem`, `TabBar`, `TabBarItem`
- `pasteboard`: `Pasteboard`
- `layout`: Auto Layout anchor helpers (`leading`, `top`, `width`, `equal`, `equal_offset`, `activate`, priorities, layout guides, safe area)
- `events`: `UIEvent`/`UITouch` accessors
- `notifications`: `NotificationCenter` and owned `Observer`

## Entry convention

`UIApplicationMain` never returns, so the app's flow is:

1. The C+ app exports `pub extern fn cplus_app_main(argc: i32, argv: *u8) -> i32`
   and tail-calls `application::run(argc, argv, did_finish_imp)`.
2. `did_finish_imp` is the `application:didFinishLaunchingWithOptions:`
   implementation — build the `Window` / root `ViewController` / views there
   and return `1`.
3. The Xcode target's `main.c` is the two-line shim:

```c
extern int cplus_app_main(int argc, char **argv);
int main(int argc, char **argv) { return cplus_app_main(argc, (void *)argv); }
```

## Ownership

The bindings follow the same rule as `vendor/appkit`: wrappers that own a +1
UIKit object release it in `drop`; singleton/factory objects are exposed as
opaque borrowed handles. Keep owned wrappers alive until after adding them to a
parent view/controller that retains them, or use the module's raw-object helpers
where available.
