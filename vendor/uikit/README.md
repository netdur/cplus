# uikit

C+ bindings for UIKit (iOS), mirroring `vendor/appkit`. Raw ObjC-runtime FFI:
`objc_getClass` / `sel_registerName` / typed `objc_msgSend` declarations, with
thin C+ structs (`Window`, `ViewController`, `Label`, `Color`, `Screen`) over
the `id` pointers.

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
