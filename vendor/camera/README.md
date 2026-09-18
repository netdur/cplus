# camera

The device's cameras: enumerate, preview, capture a still. macOS, iOS, Android,
Linux and Windows from one import.

```toml
[dependencies]
camera      = "*"
facet       = "*"
flex_layout = "*"
permissions = "*"
stdlib      = "*"
```

Use `cpc pm add . camera` to write the platform-specific dependency closure.

```cplus
import "camera/camera" as camera;
import "permissions/permissions" as permissions;
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
    // JPEG bytes, borrowed for this call only. Apple and Android hop to the
    // main thread; Linux calls inline and Windows depends on stream state.
}
```

`Facing` is an Apple and Android idea. **Linux and Windows report no facing at
all**, so name the camera you want in `Request::device` there and read
`Facing::External` back from every device.

`Camera` owns the session — when it drops, the device is released and the
recording light goes out, so keep it somewhere that lives as long as the screen.

Ask for permission first; `open` reports `Denied`, it never prompts:

```cplus
permissions::request(permissions::CAMERA, on_answer, ctx);
```

- [tutorial](docs/tutorial.md) — the five-minute path
- [guide](docs/guide.md) — what each platform does, and the traps
- [ref](docs/ref.md) — signatures

Tests: `cd vendor/camera && cpc test`. Apple and Android open nothing. Linux and
Windows exercise a real camera when one is present, and skip those checks on a
machine without one; the guide explains why.
