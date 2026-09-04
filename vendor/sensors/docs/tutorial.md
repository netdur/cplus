# Tutorial

## 1. Depend on it

```toml
[dependencies]
sensors = "*"
```

## 2. Check before you ask

```cplus
if !sens::has(sens::Kind::Barometer) {
    // Most phones have no barometer, and no Mac has any of these.
}
```

## 3. Start a stream

```cplus
static READINGS: sens::Readings = #zero::[sens::Readings]();

fn sample(s: sens::Sample, ctx: *u8) {
    show("${s.x}, ${s.y}, ${s.z}");
}

match sens::updates(sens::Kind::Accelerometer, on_sample: sample,
                    request: sens::Request::new(interval_ms: 100 as i64)) {
    result::Result::Ok(r) => { READINGS = r; }
    result::Result::Err(e) => { show("outcome ${e.to_code()}"); }
}
```

`Readings` **stops on drop**. Park it in a `static` or a field, or the stream
ends at the close of the block.

## 4. Stop when you are not on screen

```cplus
fn on_detach(ref this, why: component::Detach) {
    match why {
        component::Detach::Inactive => { return; }   // focus only — keep going
        _ => { }
    }
    READINGS.stop();
}
```

## 5. Detect a shake

```cplus
fn sample(s: sens::Sample, ctx: *u8) {
    // ~9.8 at rest, because acceleration includes gravity.
    if s.magnitude() > 15.0f64 { shaken(); }
}
```

## 6. Run it

    cd playground/sensorprobe_android && ANDROID_SERIAL=emulator-5554 ./build.sh
    adb emu sensor set acceleration 0:9.81:0
    adb emu sensor set pressure 1013
