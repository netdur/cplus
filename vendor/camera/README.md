# camera

The device's cameras: enumerate, preview, capture a still. macOS, iOS, Android
and Windows from one import.

```toml
[dependencies]
camera = "*"
```

```cplus
import "camera/camera" as camera;
import "stdlib/result" as result;

match camera::open(camera::Request::new(facing: camera::Facing::Back)) {
    result::Result::Ok(cam) => {
        b.add(cam.preview(key: "live").grow(1.0));   // an ordinary facet node
        let _ = cam.capture(on_photo: got_photo);
    }
    result::Result::Err(why) => { /* Denied, Unsupported, Busy, Failed */ }
}

fn got_photo(jpeg: u8[], ctx: *u8) {
    if jpeg.is_empty() { return; }        // the capture failed
    // JPEG bytes, borrowed for this call only. On the main thread everywhere
    // except Windows — see the guide.
}
```

`Facing` is an Apple and Android idea. **Windows reports no facing at all**, so
name the camera you want in `Request::device` there and read `Facing::External`
back from every device.

`Camera` owns the session — when it drops, the device is released and the
recording light goes out, so keep it somewhere that lives as long as the screen.

Ask for permission first; `open` reports `Denied`, it never prompts:

```cplus
permissions::request(permissions::CAMERA, on_answer, ctx);
```

- [tutorial](docs/tutorial.md) — the five-minute path
- [guide](docs/guide.md) — what each platform does, and the traps
- [ref](docs/ref.md) — signatures

Tests: `cd vendor/camera && cpc test`. On Apple and Android they open nothing —
the platform round-trips live in probes, run on purpose. **The Windows suite
does open the lens**, deliberately and for a reason the guide gives.
