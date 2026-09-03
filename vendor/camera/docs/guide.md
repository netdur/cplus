# Guide

What each platform does, why CameraX is not here, and the several ways this can
look like it is working when it is not. Signatures are in [ref.md](ref.md); the
five-minute path is [tutorial.md](tutorial.md).

## What each platform does

| | Apple | Android |
|---|---|---|
| capture | AVFoundation, `AVCaptureSession` | `android.hardware.camera2` |
| preview | `AVCaptureVideoPreviewLayer` in a layer-backed view | `TextureView` |
| still | `AVCapturePhotoOutput` → JPEG | `ImageReader`, JPEG format |
| Java/ObjC shipped | none | one class, 8,244 bytes of dex |
| needs a permission | yes, `camera` | yes, `camera` |
| needs a manifest entry | `NSCameraUsageDescription` | `android.permission.CAMERA` |

The preview is a `facet::Node` on both, built through `facet::adopt_native_with`
— the factory escape hatch, where the platform view is created at mount and the
previous one released first. No new ledger element exists for camera and
`tools/gen_contract.py` was not touched.

## Why not CameraX

CameraX is `androidx.camera`, an AAR, and it is a real thing to give up: what it
carries is the device-quirk database for the OEM long tail — orientation,
stretched preview, aspect-ratio bugs on specific hardware.

The measurement that decided it (`cpc pm maven price`, plans/aar.md):

| | this package | CameraX |
|---|---|---|
| artifacts resolved | 0 | 35 |
| dex | 8,244 bytes | 6,971,948 bytes |
| method budget | negligible | 40,866 of 65,536 |
| extra runtime | none | Kotlin stdlib + coroutines |

845× the dex, and 62% of the single-dex method budget, for one feature, in every
app. `android.hardware.camera2` is 57 classes already in `android.jar`.

**Linking an AAR without Gradle is possible** and worth being accurate about:
an `.aar` is a zip, `d8` takes jars directly, and the whole CameraX closure was
resolved and dexed successfully while deciding this. It was not a wall. It was a
price, and this is a judgement about the price.

The bet is that facet needs preview-into-a-view and still capture, which is
camera2's well-trodden part, and that a quirk is better fixed against a device
that shows one. Revisit when a device shows one.

## Android needs a dex, and the dex is a build artifact

camera2's callbacks are ABSTRACT CLASSES — `CameraDevice.StateCallback`,
`CameraCaptureSession.StateCallback`. `java.lang.reflect.Proxy` implements
interfaces only, so they cannot be answered from JNI at all. Real subclasses are
required, and real subclasses need a dex.

`camera.dex` is COMMITTED. Editing `java/cplus/camera/CplusCamera.java` changes
nothing until `tools/build_dex.sh` runs. The symptom is a Java method that is
plainly there and never called — `vendor/facet_android` documents the same trap,
and it has cost this project time once already.

The dex is loaded with `InMemoryDexClassLoader` and its native bound with
`RegisterNatives`, so an app using this package ships no Java of its own. An app
that DID merge `camera.dex` into its own `classes.dex` is detected first and its
copy used, because two copies of the class would mean natives registered on one
and instances of the other.

### Gotcha: `FindClass` cannot see a class from an in-memory loader

Every call into `CplusCamera` goes through the global class ref taken at load
(`C_CAMERA`), never through a class NAME. The name-based helpers
(`env.call_object_1` and friends) resolve with JNI's `FindClass`, which searches
the **calling native frame's** loader — the system loader, for our code. A class
that only exists inside an `InMemoryDexClassLoader` is invisible to it.

This is worth stating because of how it failed. An app that MERGES `camera.dex`
has the class in its own dex, so the name-based call works and every test
passes. Remove the merge and `preview` takes down the process inside facet's
mount walk. Both paths have to be run; the merged one alone proves nothing about
the other.

## The photo arrives on the main thread, and the bytes are borrowed

Both platforms deliver a photo on a background queue, and both hop before
calling you: Apple through `performSelectorOnMainThread:`, Android by posting to
the main looper from Java. The next statement after a photo is almost always a
facet mutation, and a package whose callback lands on a background queue makes
that a crash the caller has to already know about.

The buffer is valid FOR THE DURATION OF THE CALL and freed after it returns.
Copy what you keep. Handing back an owned buffer would make every caller
responsible for a free across a seam where forgetting is silent.

`capture` always calls back, including when the capture failed — with a length
of zero. A callback that sometimes never fires is the worst shape this seam
could have: there is nothing for a caller to time out against.

## Gotcha: `has` answers true for both facings on a desktop

macOS reports its built-in camera's position as "unspecified" — there is no
front/back distinction on a machine with one lens above the screen. So
`has(Back)` and `has(Front)` are both true there, and `open` succeeds for
either. Measured on a MacBook: `count() == 1`, both facings true.

The alternative reads worse — `has` saying no on a machine where `open` works.
Android does the same for an external USB camera, which reports `EXTERNAL`:
rather than refuse it, the first camera is the fallback.

## Verification, and why the suite opens nothing

`cd vendor/camera && cpc test` checks the outcome mapping, the guards and the
factory hooks. It **never opens a camera**.

That is deliberate, and it is the rule `vendor/securestore` follows for the same
reason: `cpc test` builds an unsigned binary and runs it on your machine. A
suite that opened the lens would light the recording indicator on every run, and
on macOS the first open raises a system permission dialog. A test suite that can
interrupt you with a dialog is one you stop running.

The platform round-trips live in probes:

| target | how | state |
|---|---|---|
| macOS | `playground/cameraprobe` (CLI), `playground/cameraprobe_mac` (window) | **PASSES** — 1920×1080 JPEG with valid Exif, and the preview confirmed live BY EYE |
| Android emulator | `playground/cameraprobe_android` | **PASSES** — see below |
| **iOS simulator** | — | **nothing to run: it has no camera** |
| iOS device | a real iPad | **NOT YET RUN.** Cross-builds only |
| Android device | `playground/cameraprobe_android` | not yet run |

The Android emulator run, both dex paths, `count=2`:

```
host=true vm=true
count=2 back=true front=true
open ok
preview mounted=true
photo #1: 97417 bytes, jpeg=true
photo #2: 97698 bytes, jpeg=true      (repeat captures, no ImageReader stall)
```

with the synthetic scene live in the `TextureView`, laid out by flex inside the
facet tree. What that does NOT prove is image content, orientation against a
real sensor, focus or torch.

**iOS has been compiled and never run.** The preview there takes the
`+layerClass` branch, which macOS never executes, so it has had no exercise at
all.

**The iOS simulator enumerates zero capture devices**, always. The package
answers `Unsupported` there. That is not a bug to work around and a test that
expects a device on the simulator fails for the wrong reason forever.

**The Android emulator's two cameras are synthetic** —
`vendor.qemu.sf.fake_camera=front`, front and back both present. Enumeration,
open, session configuration and frame delivery are all real code paths worth
running there. Image content, orientation against a real sensor, focus and
torch are not.

## Gotcha: a bare binary never prompts, and a bundle does

Running a probe from a terminal asks for nothing and just works. That is not
macOS being relaxed about the camera, and building on the observation is how a
crash gets shipped.

macOS charges a camera request to the **responsible process** — the app at the
head of the launch chain — not to the binary that made the call. A `cpc build`
executable is ad-hoc, linker-signed, with no bundle and no Team ID (`Identifier=
probe, TeamIdentifier=not set`), so it has no stable code identity for a grant
to attach to. The request lands against Terminal, iTerm or VS Code, which
already holds one, and every binary launched from there inherits it silently.

A real `.app` with its own bundle identifier gets its OWN TCC record and WILL
prompt on first use. It also needs

```xml
<key>NSCameraUsageDescription</key>
<string>To take a photo.</string>
```

and without it macOS **kills the process** rather than denying the call — the
same shape as the `UNUserNotificationCenter` trap `examples/notifications_demo`
records.

So a probe proves the capture pipeline and proves nothing about the permission
flow. The two have to be tested in a bundle.

## Gotcha: `isPreviewing` is false on macOS even when the preview is live

`AVCaptureVideoPreviewLayer.isPreviewing` reads false on macOS while the preview
is visibly rendering. Verified by eye against a probe where every other signal
was correct:

```
layer=AVCaptureVideoPreviewLayer_Tundra  previewing=false
view.frame=528x574   layer.bounds=528x574
session=true running=true connection=true enabled=true active=true
```

Do not use it as a health check. `session.isRunning` plus a connection that is
enabled and active is the combination that means something.

## Gotcha: the preview view has no size at mount

The factory builds the view during the mount walk, BEFORE flex has laid the
tree out, so anything reading geometry there sees zeros — the same probe
reported `view.frame=0x0` immediately after `mount::add_child` and `528x574` two
seconds later. Harmless for the preview itself, which fills whatever it is
given, but a caller that sizes something off the view at mount gets nothing.

## Gotcha: a benign objc warning on macOS

Running the probe prints:

```
objc[...]: class `NSKVONotifying_AVCapturePhotoOutput' not linked into application
```

AVFoundation observes its own photo output with KVO and the runtime notes that
the dynamic subclass has no static counterpart. It is printed once, changes
nothing, and there is no way to suppress it from this side.

## Live frames: luma, off-main, and dropped

`on_frame` streams; `capture` takes one still. They differ in three ways that
are all deliberate.

**Luma by default.** `Frame::luma()` is the Y plane — one byte per pixel,
`stride` bytes per row. Apple delivers `420YpCbCr8BiPlanar` and Android
`YUV_420_888`, and plane 0 of each has exactly this layout, so the surface needs
no conversion and no per-platform branch. Colour would mean a full conversion
per frame on at least one platform, at 30Hz, for callers that mostly want
luminance anyway — motion, exposure, barcodes and faces all read it.

**Colour on request.** `Request::new(pixel: PixelFormat::Bgra32)` asks the
capture pipeline itself for packed BGRA, so the conversion happens wherever
AVFoundation does it rather than per frame on your thread. Honoured on macOS
(measured, preview mounted, end to end); read what you actually got with
`Camera::pixel_format()`. A packed frame has NO luma plane: `luma()` is empty
and the image is `native()` — the `CVPixelBufferRef` itself, which is what
CoreML and Vision want anyway. `Frame::is_empty()` is false for it; only a
frame with neither a plane nor a native buffer is empty. Android ignores the
request and stays `Luma8`, and `pixel_format()` says so.

**Off the main thread.** `capture` hops to main because the next statement is
almost always a facet mutation. `on_frame` does the opposite: at 30Hz, hopping
every frame would spend the UI thread on work the UI is not waiting for. Cross
back deliberately and rarely — `services::run_on_main` — as
`playground/cameraprobe_mac` does every tenth frame.

**Dropped, not queued.** A handler slower than the camera misses frames. That is
the right failure for live video; a queue would grow without bound and deliver
moments that have passed.

### Gotcha: macOS defaults the video output to a PACKED format

An `AVCaptureVideoDataOutput` with no `videoSettings` hands back `2vuy` on
macOS — packed 4:2:2, with no plane 0 to read. Measured: 162 frames delivered,
162 skipped by the planar guard, and the stream looked simply dead from outside.
The backend now requests `420v` explicitly. The key for that request,
`kCVPixelBufferPixelFormatTypeKey`, is an exported DATA symbol needing a deref
after `dlsym` — the same trap securestore records for the keychain constants,
and it fails the same silent way: an unknown dictionary key is ignored and the
packed default comes back.

CoreVideo and CoreMedia are reached by `dlopen` because the manifest has no
`[<platform>.link]` section, so a cross-platform package cannot name an
Apple-only framework. AVFoundation does not re-export them — verified, not
assumed.

## What this is not

**Not a video recorder in v1.** Movie capture is a second output, a file
lifetime and a microphone permission that stills do not need. It is a verb to
add, not a reason to widen this one.

**Two facings, not a device list.** A phone with three back lenses presents them
as one logical device that switches for you. Enumerating physical lenses would
make every caller carry a device picker it did not want.
