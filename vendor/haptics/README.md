# haptics

The small physical taps a UI makes.

```toml
[dependencies]
haptics = "*"
```

```cplus
import "haptics/haptics" as hap;

hap::play(hap::Feel::Selection);    // a value passed under the finger
hap::play(hap::Feel::Success);      // an outcome
```

## Fire and forget

Every verb answers whether the platform **accepted** the request — never
whether anything was felt. A person can turn haptics off system-wide, a Mac
without a Force Touch trackpad has nowhere to play one, and an iPad has no
Taptic Engine at all. None of that reports back.

So `play` is **safe to call anywhere**, including where nothing can happen. Use
`available()` only to hide a "vibrate" setting that would do nothing, never to
guard a tap.

## `Feel` names the moment, not the sensation

| | for |
|---|---|
| `Selection` | a value passing under the finger — a picker, per row |
| `Light` `Medium` `Heavy` | something landed, with weight |
| `Success` `Warning` `Error` | an outcome |

"Medium impact" is an Apple word and "40 ms at default amplitude" is an Android
one. An app should be spelling neither.

## Coverage

| | macOS | iOS | Android |
|---|---|---|---|
| plays | Force Touch trackpad only, 3 patterns | ✅ 1:1 | ✅ durations |
| `prepare` | no-op | ✅ warms the Taptic Engine | no-op |

- [tutorial](docs/tutorial.md) · [guide](docs/guide.md) · [ref](docs/ref.md)

## Tests

    cd vendor/haptics && cpc test

A tap cannot be asserted — `playground/hapticprobe_*` is a screen of buttons to
feel.
