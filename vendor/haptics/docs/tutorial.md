# Tutorial

## 1. Depend on it

```toml
[dependencies]
haptics = "*"
```

## 2. Tap

```cplus
import "haptics/haptics" as hap;

fn toggled(sender: *u8, ctx: *u8) {
    hap::play(hap::Feel::Light);
}
```

No guard, no check. On a device that cannot play one this does nothing.

## 3. Match the moment

```cplus
hap::play(hap::Feel::Selection);   // a picker moved a row
hap::play(hap::Feel::Heavy);       // a card snapped into place
hap::play(hap::Feel::Error);       // the form would not submit
```

## 4. Warm it for a gesture

The Taptic Engine idles, and the first tap after a pause lands late enough to
feel disconnected from the touch that caused it. Warm it when the gesture
*begins*:

```cplus
fn drag_began(sender: *u8, ctx: *u8) { hap::prepare(hap::Feel::Medium); }
fn drag_ended(sender: *u8, ctx: *u8) { hap::play(hap::Feel::Medium); }
```

Skipping this costs latency on the first tap and nothing else. On macOS and
Android it does nothing at all.

## 5. Hide a setting that would do nothing

```cplus
if hap::available() { show_vibrate_toggle(); }
```

This is the **only** thing `available()` is for.
