# camera

The device's cameras: enumerate, preview, capture a still. macOS, iOS and
Android from one import.

```toml
[dependencies]
camera = "*"
```

```cplus
import "camera/camera" as camera;
import "stdlib/result" as result;

match camera::open(facing: camera::Facing::Back) {
    result::Result::Ok(cam) => {
        b.add(cam.preview(key: "live").grow(1.0));   // an ordinary facet node
        let _ = cam.capture(on_photo: got_photo);
    }
    result::Result::Err(why) => { /* Denied, Unsupported, Busy, Failed */ }
}

fn got_photo(jpeg: u8[], ctx: *u8) {
    if jpeg.is_empty() { return; }        // the capture failed
    // JPEG bytes, on the main thread, borrowed for this call only.
}
```

`Camera` owns the session — when it drops, the device is released and the
recording light goes out, so keep it somewhere that lives as long as the screen.

Ask for permission first; `open` reports `Denied`, it never prompts:

```cplus
permissions::request(permissions::CAMERA, on_answer, ctx);
```

- [tutorial](docs/tutorial.md) — the five-minute path
- [guide](docs/guide.md) — what each platform does, and the traps
- [ref](docs/ref.md) — signatures

Tests: `cd vendor/camera && cpc test`. They open nothing — the platform
round-trips live in probes, run on purpose. See the guide.
