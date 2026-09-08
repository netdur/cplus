# Guide

What each platform does, why CameraX is not here, and the several ways this can
look like it is working when it is not. Signatures are in [ref.md](ref.md); the
five-minute path is [tutorial.md](tutorial.md).

## What each platform does

| | Apple | Android | Windows |
|---|---|---|---|
| capture | AVFoundation, `AVCaptureSession` | `android.hardware.camera2` | Media Foundation, `IMFSourceReader` |
| preview | `AVCaptureVideoPreviewLayer` in a layer-backed view | `TextureView` | an HWND this package owns, blitting the frames |
| still | `AVCapturePhotoOutput` → JPEG | `ImageReader`, JPEG format | a frame converted to BGR, encoded by WIC |
| device controls | `AVCaptureDevice` | camera2 | `IAMCameraControl` + `IAMVideoProcAmp` |
| Java/ObjC shipped | none | one class, 8,244 bytes of dex | none |
| needs a permission | yes, `camera` | yes, `camera` | no gate to pass; `permissions` reports `Granted` |
| needs a manifest entry | `NSCameraUsageDescription` | `android.permission.CAMERA` | none |
| facing | real | real | **none — see below** |

The preview is a `facet::Node` on all three, built through
`facet::adopt_native_with` — the factory escape hatch, where the platform view
is created at mount and the previous one released first. No new ledger element
exists for camera and `tools/gen_contract.py` was not touched.

**Nothing Windows-only is linked for it.** Media Foundation, WIC and gdi32 are
all bound with `LoadLibrary` at first use. `mfplat`/`mf`/`mfreadwrite` are
absent on N editions and Server Core without the Media Feature Pack, and a
link-time dependency on a DLL that might not be there makes the whole program
REFUSE TO START — so merely *importing* `camera` would kill an application on a
machine that simply has no lens. Binding late means such a machine answers
`count() == 0` and keeps running, and `cpc`'s Windows link line stays clean for
every program that never touches this package.

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

Apple and Android deliver a photo on a background queue, and both hop before
calling you: Apple through `performSelectorOnMainThread:`, Android by posting to
the main looper from Java. The next statement after a photo is almost always a
facet mutation, and a package whose callback lands on a background queue makes
that a crash the caller has to already know about.

**Windows does not hop, and which thread you get depends on what was running.**
With no frame stream open, `capture` reads its own frame and encodes it INLINE:
the handler runs on the caller's own thread, before `capture` returns — which is
the main thread if that is where you called it, so the common case is better
than the platforms that hop. With a stream already running, the capture rides
the pump's next frame and the handler runs on the pump thread, the same one
`on_frame` arrives on. There is no main-thread queue for this backend to post
to: a hop would need a message loop the package has no claim on. If you capture
from a frame handler, treat the result as off-main and cross back yourself.

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

**Windows has no facing at all**, and this is the one place the platform is
poorer than the contract rather than differently shaped. A capture device is
addressed by name or by index and says nothing about which way it points. So
every facing answers the same way there, `actual_facing()` reports `External`
rather than guessing a side, and `switch_to` refuses. An application that wants
a particular camera on Windows names it in `Request::device` and opens that one
— and a name that matches nothing is REFUSED rather than silently substituted,
because handing back a different camera than the one asked for is exactly the
lie this package exists to avoid.

## Verification, and why the suite opens nothing — except on Windows

`cd vendor/camera && cpc test` checks the outcome mapping, the guards and the
factory hooks. On Apple and Android it **never opens a camera**.

That is deliberate, and it is the rule `vendor/securestore` follows for the same
reason: `cpc test` builds an unsigned binary and runs it on your machine. A
suite that opened the lens would light the recording indicator on every run, and
on macOS the first open raises a system permission dialog. A test suite that can
interrupt you with a dialog is one you stop running.

**The Windows suite breaks that rule on purpose**, and it is worth being plain
about rather than quietly inconsistent. It opens the device, streams frames,
encodes a still and sets exposure and white balance — reading each back off the
hardware afterwards, so an `Ok` that did nothing fails. The two halves of the
original argument come apart on this platform: there is no permission dialog to
be interrupted by, and the alternative is a backend whose every real code path
is unexercised. The recording light does come on for a few seconds per run.
If that trade stops being worth it, the opening tests are the ones that begin
`if _c_count() == 0 { return; }` — they are already written to no-op on a
machine with no lens, so gating them behind an environment variable is a small
change.

The platform round-trips live in probes:

| target | how | state |
|---|---|---|
| macOS | `playground/cameraprobe` (CLI), `playground/cameraprobe_mac` (window) | **PASSES** — 1920×1080 JPEG with valid Exif, and the preview confirmed live BY EYE |
| Android emulator | `playground/cameraprobe_android` | **PASSES** — see below |
| Windows | `playground/cam_probe` (CLI), `cam_shot` (still), `cam_view` (raw window), `cam_facet` (preview in a facet tree) | **PASSES** — see below |
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

The Windows run, on an HP Wide Vision HD Camera:

```
cameras visible: 1
  device: HP Wide Vision HD Camera
  format: 640x360 @ 30fps
frames delivered: 62 in 2s      luma bytes 640*360 = 230400, mean 143
still: 39724 bytes, jpeg magic true, real scene, correct colour
preview: adopted HWND at 536x368 inside a facet column, image live
```

`cam_facet` is the one that proves the most, because it is the whole road:
`camera` makes the preview window, facet adopts it, `facet_win32` turns a
parentless popup into a real child, flex gives it a frame, and the pump's frames
land in it. Checked with `PrintWindow` and `PW_RENDERFULLCONTENT` rather than a
screen grab — a screen capture samples whatever is in front of the window.

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

## Windows: the device controls ask, they do not assume

`set_exposure`, `set_white_balance`, `set_focus` and `set_zoom` go through
`IAMCameraControl` and `IAMVideoProcAmp` — DirectShow interfaces that a Media
Foundation capture source still answers `QueryInterface` for, and the only road
to these on Windows.

**Every verb calls `GetRange` first and reports the device's own answer**, which
is the difference between a refusal that is true and one that is written down. A
typical laptop webcam supports Exposure and WhiteBalance and refuses Pan, Tilt,
Roll, Zoom, Iris and Focus with `ERROR_NOT_SUPPORTED` — so `set_focus` returns
`Unsupported` there *because the device said so*, and the identical code answers
`Ok` on a camera with a focus motor. Ask for what you want and read the outcome;
do not infer from the platform.

`Mode::Locked` reads the current value back before switching the property to
manual — manual with a stale value jumps the image rather than freezing it.

**`Mode::Once` is refused on Windows.** It means "adjust once, then hold", and
neither interface has a one-shot: implementing it would be switching to auto,
waiting some invented interval, and latching whatever happened to be there.
Answering `Locked` to a request for `Once` is a quiet substitution, so the
honest answer is that it cannot.

**Zoom is a focal length, not a factor.** DirectShow specifies the Zoom property
in millimetres, so this backend reads the range's maximum over its minimum as
the real optical zoom and `min * factor` as the focal length for a given factor.
That is the only defensible mapping; a device-unit scale invented here would
mean nothing on the next camera. A request above the maximum is clamped rather
than refused — a caller asking for more zoom than the lens has wants the most it
can give — and `zoom()` reports what it actually got.

**`has_torch` is a flat no on Windows**, and unlike the others it is not a
query: neither interface has a lamp property at all, so there is nothing to ask.
A camera with a lamp exposes it through `Windows.Devices.Lights.Lamp` or a
KSPROPERTY extended control set, which is a different pipeline.

## Gotcha: `capture` while streaming costs a frame, not the stream

On Windows a capture asked for while `on_frame` is running rides the pump's next
frame rather than opening a second read — two `ReadSample` calls on one source
from two threads is not something Media Foundation promises. The encode takes a
few milliseconds during which the pump delivers nothing, so one photo drops
roughly one frame. That is the trade every platform's still capture makes, and
it only happens when asked.

The reverse interaction is the one worth knowing: the preview and `on_frame` are
**two independent consumers of one reader**. `preview()` starts the reader
itself, because it is independent of `on_frame` in the contract and a preview
with no handler must still show something. So `stop_frames` ends delivery to
your handler and stops the reader only if nothing else still wants frames — it
will not blank a preview you never mentioned. `close` ends both.

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
`stride` bytes per row. Apple delivers `420YpCbCr8BiPlanar`, Android
`YUV_420_888` and Windows usually NV12, and plane 0 of each has exactly this
layout, so the surface needs no conversion and no per-platform branch. Colour
would mean a full conversion per frame on at least one platform, at 30Hz, for
callers that mostly want luminance anyway — motion, exposure, barcodes and faces
all read it.

Windows will also hand you YUY2 from some devices, which is packed 4:2:2 with
luma at every *other* byte — so `stride` is `width * 2` there, not `width`, and
that is exactly why `stride` exists rather than being assumed equal to width.
`pixel_format()` still reads `Luma8` for it. A device settling on RGB32 reports
`Bgra32` and has no luma plane at all.

**Colour on request.** `Request::new(pixel: PixelFormat::Bgra32)` asks the
capture pipeline itself for packed BGRA, so the conversion happens wherever
AVFoundation does it rather than per frame on your thread. Honoured on macOS
(measured, preview mounted, end to end); read what you actually got with
`Camera::pixel_format()`. A packed frame has NO luma plane: `luma()` is empty
and the image is `native()` — the `CVPixelBufferRef` itself, which is what
CoreML and Vision want anyway. `Frame::is_empty()` is false for it; only a
frame with neither a plane nor a native buffer is empty. Android ignores the
request and stays `Luma8`, and `pixel_format()` says so.

**Off the main thread.** `capture` hops to main on Apple and Android because the
next statement is almost always a facet mutation. `on_frame` does the opposite
everywhere: at 30Hz, hopping every frame would spend the UI thread on work the
UI is not waiting for. Cross back deliberately and rarely —
`services::run_on_main` — as `playground/cameraprobe_mac` does every tenth
frame. On Windows the frames arrive on the reader thread for the same reason,
and `capture` does not hop at all (see above).

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
