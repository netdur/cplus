# Guide

## Why every verb is a no-op somewhere

Haptics is the one capability where "it did nothing" is the *normal* outcome
rather than a failure:

- a person turned them off in Settings, and no platform reports that
- a Mac has no Force Touch trackpad
- an **iPad has no Taptic Engine at all** — the classes exist and play nothing
- an emulator has no motor

So `play` answers whether the platform **took** the request, and there is no
answer available for whether anything was felt. A UI that branches on it is
asking a question the hardware does not take.

`available()` exists for exactly one job: hiding a "vibrate" setting that would
do nothing. Guarding individual taps with it makes the code worse and changes
nothing.

## The vocabulary, and what each platform does with it

`Feel` names the *moment*. The three platforms describe haptics completely
differently, and an app should not have to know which it is talking to.

| `Feel` | iOS | macOS | Android |
|---|---|---|---|
| `Selection` | `UISelectionFeedbackGenerator` | `Alignment` | 10 ms |
| `Light` | impact, style 0 | `LevelChange` | 20 ms |
| `Medium` | impact, style 1 | `LevelChange` | 40 ms |
| `Heavy` | impact, style 2 | `LevelChange` | 60 ms |
| `Success` | notification, type 0 | `Generic` | 30 ms |
| `Warning` | notification, type 1 | `Generic` | 120 ms |
| `Error` | notification, type 2 | `Generic` | 30 ms |

**iOS is the platform the enum was shaped after**, so there the mapping is a
lookup rather than a judgement.

**macOS collapses seven onto three**, and the collapse is a judgement:
`NSHapticFeedbackManager` offers Generic, Alignment and LevelChange, meant for
a trackpad under a finger — an alignment guide snapping, a slider passing a
detent. There is no "error" buzz because a Mac has nowhere to buzz.

**Android has no vocabulary of intent at all** — `VibrationEffect` is a
duration and an amplitude. The numbers above were chosen to read like the Apple
ones, not to match any Android convention. Amplitude is left at
`DEFAULT_AMPLITUDE` rather than inventing a number that means different things
on different motors.

## `prepare`, and why the first tap is late

The Taptic Engine idles. A tap played on a cold engine lands late enough to
feel disconnected from the touch that caused it — Apple's own guidance is to
warm it when a gesture *begins* and play when it ends.

That is what `prepare` is. It is **not required**: skipping it costs latency on
the first tap and nothing else. macOS and Android have nothing to warm and do
nothing.

The iOS backend keeps **one generator per family alive for the process** rather
than building one per tap, because the warmth lives in the generator and a
fresh one would discard what `prepare` bought.

## No Java, no dex

This is the first mobile package here that needs neither, because there is no
interface to implement — haptics is a call out, never a callback in. Compare
`camera`, `location` and `sensors`, which all ship a `.dex` purely to satisfy a
listener interface.

## API levels

`VibrationEffect` arrived in API 26, which is this project's floor, so there is
no legacy `vibrate(long)` path.

API 31 replaced `getSystemService("vibrator")` with a `VibratorManager`. The
old call still works and is deprecated; the backend tries the manager first and
falls back, because the floor is 26 and the ceiling is whatever the person is
holding.

## What was measured

Nothing yet — **a tap cannot be asserted from a test**. The suite covers the
vocabulary, the code mapping and that the verbs are safe where nothing can be
felt. Whether a Heavy feels heavier than a Light needs a finger:
`playground/hapticprobe_*` is a screen of buttons for that.
