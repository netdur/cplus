# Reference

`import "camera/camera" as cam;`

Behaviour and reasoning are in [guide.md](guide.md).

## Types

### `enum Facing`

| variant | |
|---|---|
| `Back` | away from the user |
| `Front` | toward the user |

A desktop camera that reports no position satisfies both — see the guide.

### `enum Outcome`

| variant | meaning |
|---|---|
| `Ok` | the call succeeded |
| `Denied` | camera permission is not granted; ask with `permissions::request` |
| `Unsupported` | no camera on this device. The iOS simulator is always this |
| `Busy` | a camera is here but another client holds it, or the session is not configured yet |
| `Failed` | anything else |

```cplus
fn Outcome::to_code(this) -> i32
fn Outcome::from_code(c: i32) -> Outcome
fn Facing::to_code(this) -> i32
```

The seam forms, paired onto their own enums. An unrecognised code maps to
`Failed`, never `Ok`.

### `struct Camera`

An owned session. `drop` closes it.

### `struct Frame`

One video frame, borrowed for the duration of the callback.

```cplus
fn width(this) -> i32
fn height(this) -> i32
fn stride(this) -> i32      // bytes per row, always >= width
fn is_empty(this) -> bool
fn luma(this) -> u8[]       // the Y plane: stride * height bytes
fn native(this) -> *u8      // the platform's own buffer, or 0
```

**LUMA BY DEFAULT.** Both platforms deliver 4:2:0 planar YUV natively and the Y
plane has an identical layout on each, so this needs no conversion anywhere. A
session opened with `pixel: PixelFormat::Bgra32` delivers PACKED frames instead:
`luma()` is empty and the image is `native()` — on Apple the `CVPixelBufferRef`
itself. `is_empty()` is false for a packed frame; only a frame with neither a
plane nor a native buffer is empty. See the guide.

**`stride` is not `width`.** Rows are padded; index with `y * stride + x`.

## Free functions

```cplus
fn count() -> usize
```

Capture devices this process can see, across both facings. Needs no permission.

```cplus
fn has(facing: Facing) -> bool
```

Whether a camera faces this way. Needs no permission.

```cplus
fn open(facing: Facing = Facing::Back) -> result::Result[Camera, Outcome]
```

Open a session. Synchronous. Does not prompt — reports `Denied`.

## `Camera`

```cplus
fn is_open(this) -> bool
```

False after `close`.

```cplus
fn preview(this, key: str = "") -> core::Node
```

The preview surface as a facet node. A leaf with no intrinsic size: it fills
the box it is given. The platform view is built at MOUNT, so this may be called
before the tree is mounted, and the node may be mounted more than once.

```cplus
fn capture(this, on_photo: fn(u8[], *u8), on_photo_ctx: *u8 = 0 as *u8) -> Outcome
```

Request one still. `Ok` means the request was accepted, not that a photo exists.

`on_photo(jpeg, ctx)` runs on the MAIN THREAD. The slice is valid only for the
duration of the call — copy what you keep. Reach the bytes with `jpeg.count()`
and `#slice_ptr(jpeg)`. A failed capture calls back with an EMPTY slice rather
than not calling back.

The context is LAST, which is the shape that can receive a bound method
(`capture(on_photo: this.got_photo, on_photo_ctx: #addr_of(this))`); a
context-first handler cannot, and warns W0824/W0825.

At most **four** sessions may have a handler registered at once; a fifth
`capture` answers `Failed`.

```cplus
fn on_frame(this, on_frame: fn(Frame, *u8), on_frame_ctx: *u8 = 0 as *u8) -> Outcome
fn stop_frames(this) -> Outcome
```

Start and stop live frame delivery. `on_frame` is a STANDING request: the camera
opens asynchronously, so arming it immediately after `open` is fine and it takes
effect when the device is ready.

`on_frame(frame, ctx)` runs **on a background thread** — the opposite of
`capture`, and the single most important thing on this page. It must not touch a
facet node; hand results to the UI with `services::run_on_main`.

Frames are DROPPED, not queued, when the handler is slower than the camera.

```cplus
fn pixel_format(this) -> PixelFormat
```

What the frames ACTUALLY hold, read back from the platform — never an echo of
the request. `Any` before `on_frame` has armed the stream, and on a backend
that will not say. `Luma8` means read `Frame::luma()`; `Bgra32` means `luma()`
is empty and the image rides in `Frame::native()`.

```cplus
fn close(ref this)
```

Stop the session and release the device. Idempotent; `drop` calls it.

**Receivers**: `close` is the only verb needing a mutable binding. `is_open`,
`preview` and `capture` take `this`, which borrows — so they work directly on a
`match` binding, which is frozen.

## Constants

None on the public surface.

## Platform notes

| | macOS | iOS | iOS simulator | Android |
|---|---|---|---|---|
| `count` / `has` | yes | yes | always 0 / false | yes |
| `open` | yes | yes | `Unsupported` | yes |
| `preview` | layer-hosting NSView | UIView, `+layerClass` | — | `TextureView` |
| `capture` | yes | yes | — | yes |

Requires `NSCameraUsageDescription` in the bundle on Apple and
`android.permission.CAMERA` in the manifest on Android.
