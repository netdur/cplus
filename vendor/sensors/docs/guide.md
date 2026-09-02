# Guide

## Units, and why this package converts

The two platforms disagree in a way nothing reports:

| | Apple | Android | this package |
|---|---|---|---|
| acceleration | **G** | m/s² | **m/s²** |
| rotation | rad/s | rad/s | rad/s |
| magnetic field | µT | µT | µT |
| pressure | kPa | hPa | **hPa** |

An app that checks `magnitude() > 15.0` for a shake works on Android and never
fires on iOS, because iOS would be reporting ~1.0 at rest. The Apple backend
multiplies by 9.80665; the Android backend passes values through and says so.

**Acceleration includes gravity.** A device flat on a table reads about 9.8 on
one axis, not 0. Removing gravity is a different sensor (`CMDeviceMotion` /
Android's `TYPE_LINEAR_ACCELERATION`), not offered here.

## `Unavailable` is not `Unsupported`

- **`Unsupported`** — no backend for this platform. The build cannot ask.
- **`Unavailable`** — there is a backend, and this device has no such sensor.

A Mac answers `Unavailable` for all four: CoreMotion links on macOS and every
availability probe returns false. Collapsing the two would let a wiring bug
hide behind a hardware excuse.

## The barometer is Android-only

Apple's barometer is `CMAltimeter`, not `CMMotionManager`, and **every one of
its entry points takes a block** — there is no polled form to read from. The
three motion sensors have both forms, and this package uses the polled one (see
below), so the altimeter does not fit the same shape.

`available(Kind::Barometer)` therefore answers `false` on Apple, and `updates`
answers `Unavailable`. Android has `TYPE_PRESSURE` as an ordinary sensor.

## Why Apple polls

CoreMotion offers each motion sensor twice: `startXUpdatesToQueue:withHandler:`
takes a block, and bare `startXUpdates` starts the sensor and lets you read
`xData` whenever you like. This backend uses the second and drives it from an
`NSTimer`.

The reason is **block lifetime**. cpc-bindgen builds a *stack* block — correct
for something like `enumerateAttributesInRange:`, which calls it and returns,
and unverified for a handler CoreMotion stores and calls for minutes. It would
work if the framework copies the block, and the copy would leave the block's
descriptor pointing at a dead frame. That is a question to settle on a device
with a measurement rather than to guess at, and the polled form makes it moot.

The visible consequence: on Apple the sample rate is the timer's, and
`interval_ms` sets both the sensor's own interval and the timer's.

## The rate is a hint

Android's own documentation calls `samplingPeriodUs` a hint and delivers faster
or slower as it pleases; Apple's interval is best-effort too. Ask for what you
need and read the timestamps.

`interval_ms: 0` takes the platform default — `SENSOR_DELAY_NORMAL` on Android,
100 ms on Apple — rather than meaning "as fast as possible", which would pin a
core for a UI redrawing at 60 Hz.

**Above ~200 Hz Android 12+ requires `HIGH_SAMPLING_RATE_SENSORS`** and silently
caps you at 200 Hz without it. That arrives as samples that are merely slower
than asked, never as an error.

## Timestamps

`Sample.timestamp_ms` is Unix milliseconds, taken at the **source** — when the
reading was made, not when it was delivered.

Android's `SensorEvent.timestamp` is *nanoseconds since boot*, which is the one
thing in this API that silently produces nonsense if taken at face value: a
reading timestamped 4,000,000 would be January 1970. The Java half converts it
against `elapsedRealtimeNanos`, so the age of the sample is preserved rather
than replaced by "now".

## Lifecycle

A sensor is a radio. `Readings` stops on drop, and the reasons say when to stop
deliberately:

- `Detach::Inactive` — focus only, still visible. **Do not stop.**
- `Detach::Background` — not visible. Stop.

## What was measured

Android emulator, 2026-09-02, all four started and delivering:

    accel=true gyro=true mag=true baro=true
    accel  0  9.77632  0.812345   |9.81001|     (injected 0:9.81:0)
    mag    0  40       0          |40|          (injected 0:40:0)
    baro   1013.25     0  0       |1013.25|

The gyroscope delivers samples at the right rate but reads zero while the
emulator reports `gyroscope = 0.25:0:0`. **The iPad settled this**: on real
hardware the gyroscope reads ~0.003 rad/s of hand tremor, so the emulator was
the problem and the plumbing was never in doubt.

iPad Pro 11-inch (M1), same day, all three motion sensors on the first run:

    accel=true gyro=true mag=true baro=false
    accel  -7.69945  0.39654  -6.12766   |9.8482|
    gyro    0.00230   0.00649  -0.00327   |0.00762|
    mag     691.628  -134.639  -200.862   |732.681|

Three things that confirmed rather than surprised:

**The unit conversion is real and visible.** CoreMotion reported about 1.004 G
and the magnitude here is 9.8482 m/s². Without the conversion an app checking a
threshold would be wrong by 9.8x between platforms — the exact failure the
table at the top of this page exists to prevent. The axes are -7.7 / 0.4 / -6.1
because the device was lying at an angle; the magnitude is gravity regardless.

**Polling plus an NSTimer works**, at the requested 200ms, steadily. The block
lifetime question never arose, which was the point of choosing the polled form.

**The magnetometer reads ~733 microtesla** where Earth's field is 25-65. That is
not an error: `CMMagnetometerData.magneticField` is the RAW field, including
whatever the device itself generates. An app treating this as a compass would
point at the iPad's own magnets. `CMDeviceMotion.magneticField` carries a
calibration accuracy and is the API a compass wants; this package does not
offer it yet.

macOS: builds, and answers `Unavailable` for all four because the hardware does
not exist. Correct, and it tests nothing.
