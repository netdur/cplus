# Tutorial

A screen that shows the camera and takes a photo. Signatures are in
[ref.md](ref.md); the reasons behind the choices are in [guide.md](guide.md).

## 1. Depend on it

```toml
[dependencies]
camera = "*"
```

The package pulls in `permissions` itself. On Apple you also need the usage
description in your bundle's `Info.plist` — without it the system kills the
process the moment you open a camera:

```xml
<key>NSCameraUsageDescription</key>
<string>To take a photo.</string>
```

On Android, add the permission to your manifest:

```xml
<uses-permission android:name="android.permission.CAMERA"/>
```

## 2. Ask first

`open` reports `Denied`; it does not prompt. Prompting is asynchronous, and
making the one synchronous verb able to put a dialog on screen would make every
caller asynchronous for the sake of the first run.

```cplus
import "permissions/permissions" as permissions;

permissions::request(permissions::CAMERA, on_answer, 0 as *u8);

fn on_answer(name: str, s: permissions::State, ctx: *u8) {
    if s == permissions::State::Granted { start(); }
}
```

macOS is the exception worth knowing: there, the first `open` is itself what
raises the system dialog, so an app that has never asked can go straight to
`open`. The package passes an `Unknown` permission state through for exactly
that reason.

## 3. Open

```cplus
import "camera/camera" as camera;
import "stdlib/result" as result;

static SESSION: camera::Camera = #zero::[camera::Camera]();

fn start() {
    match camera::open(facing: camera::Facing::Back) {
        result::Result::Ok(c) => { SESSION = c; }
        result::Result::Err(why) => {
            // Unsupported = no camera here. Denied = ask. Busy = someone else has it.
        }
    }
}
```

The match arms need no type arguments — `result::Result::Ok(c)` infers them
from the call.

`Camera` owns the session. When it drops, the session closes and the recording
indicator goes out — so keep it somewhere that lives as long as the screen, not
in a local that falls out of scope.

## 4. Show it

`preview` gives you a `facet::Node`. It is a leaf that fills the box you give
it, so lay it out like any other node:

```cplus
let cam_node: core::Node = SESSION.preview(key: "live");
// ... place it in your tree, e.g. with .grow(1.0) inside a column
```

The platform view is built when the node MOUNTS, not when you call `preview`,
so calling it before the tree is mounted is fine and mounting the node twice is
fine.

## 5. Take a photo

```cplus
let o: camera::Outcome = SESSION.capture(on_photo: got_photo);
// Ok here means "request accepted", not "a photo exists".

fn got_photo(jpeg: u8[], ctx: *u8) {
    if jpeg.is_empty() { return; }       // the capture failed
    let n: usize = jpeg.count();
    let bytes: *u8 = { #slice_ptr(jpeg) };   // for handing to C
    // JPEG, on the main thread, freed when this returns. Copy what you keep.
}
```

The bytes arrive as a SLICE rather than a pointer and a length, so the two
cannot be passed apart. The handler takes its context LAST, which is the shape
that can receive a bound method:

```cplus
SESSION.capture(on_photo: this.got_photo, on_photo_ctx: #addr_of(this));
```

## 6. Close

```cplus
SESSION.close();          // idempotent; `drop` calls it too
```

`close` is the only verb that needs a mutable binding. `is_open`, `preview` and
`capture` borrow, so they work directly on the `c` a `match` arm hands you.

## Running it

`cpc test` in this package proves the portable half and opens nothing. To see
the camera actually work, `playground/cameraprobe` opens the lens on a Mac,
takes one photo and writes it to `/tmp/cameraprobe.jpg`.

Which targets can prove what is in [guide.md](guide.md) — the short version is
that **the iOS simulator has no camera at all** and the Android emulator's are
synthetic.
