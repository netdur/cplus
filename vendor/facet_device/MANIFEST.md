# facet_device — the manifest

What this tier answers, what it deliberately does not, and what the platform
would not let it. Read this before trusting an adjective about the package.

Two modules. `permissions` is the gate every device capability shares; `camera`
ENUMERATES capture devices. Neither opens anything: streaming a camera, reading
a microphone, following a location are all absent, and each is its own piece of
work with a very different shape — a device list is a question with an answer,
a video stream is a lifetime.

## What is live

| verb | Linux (XDG portal) | elsewhere |
|---|---|---|
| `supported()` | true when `org.freedesktop.portal.Desktop` answers | false |
| `status(Camera)` | `Unavailable` with no camera, else `Undetermined` | `Unavailable` |
| `status(Microphone)` | `Granted` — the platform has no runtime gate | `Unavailable` |
| `status(Location)` | `Unavailable` with no `Location` portal, else `Undetermined` | `Unavailable` |
| `status(Notifications)` | `Granted` when the interface is offered | `Unavailable` |
| `request(Camera)` | `AccessCamera` + the Request handshake, bounded | `Unavailable` |
| `request(Microphone)` | `Granted` — there is no gate to pass | `Unavailable` |
| `request(Location)` | `CreateSession` + `Start` + the Request handshake | `Unavailable` |
| `request(Notifications)` | `Granted` — there is no gate to pass | `Unavailable` |
| `camera::list()` | V4L2 `VIDIOC_QUERYCAP` over `/dev/video*` | empty |
| `camera::count()` | the same, counted | 0 |

Measured on the session this was written against: the service is present, camera
and location answer `Undetermined`, microphone and notifications `Granted`, and
`camera::list()` finds one device where the machine exposes two `/dev/video`
nodes — see §3.

## 1. Decided absent — the platform has no such thing

- **`status` cannot report a stored decision.** The portal keeps one, in
  `org.freedesktop.impl.portal.PermissionStore`, and that interface is
  `impl.portal` on purpose: it belongs to the backend, not to applications,
  precisely so an app cannot silently probe what the user has allowed. So a
  gated capability reads `Undetermined` even after a grant, and only `request`
  can say more. This is a NARROWING against a platform like macOS, where
  `AVCaptureDevice.authorizationStatus` answers without prompting — and it is
  the portal's design rather than a gap here.
- **There is no permission bit on a plain Linux desktop.** An unsandboxed
  process opens `/dev/video0` with no ceremony. A backend reporting what the
  SYSCALL would do would answer `Granted` to everything, which is true of the
  syscall and false of the user's intent. Where no portal is running this
  answers `Unavailable` — "this machine has no way to ask" — never `Granted`.
- **A MICROPHONE HAS NO RUNTIME GATE, and the portal that looks like one is
  not one.** `org.freedesktop.portal.Device` has `AccessDevice(pid, devices,
  options)` and reads exactly like the microphone's consent call. It is not.
  Calling it answers

      org.freedesktop.portal.Error.NotAllowed
      "This call is not available inside the sandbox"

  because that interface exists for a HOST to grant a device to some OTHER
  process, not for an application to ask on its own behalf. There is no
  app-facing audio-input consent on a Linux desktop at all: a sandboxed app is
  given the audio socket at install time, and an ordinary one opens PipeWire.

  So the answer is `Granted`. This is a REAL DIVERGENCE from macOS and iOS,
  where a microphone is gated at runtime and `request` shows a prompt — an
  application that wants to behave the same everywhere should still call
  `request` and honour what it gets, which is what makes the divergence
  harmless.

  Found by calling it. A reading of the portal documentation gets this wrong,
  which is why the suite pins it.

- **Notifications have no request flow.** `org.freedesktop.portal.Notification`
  has `AddNotification` and nothing to consent to; the desktop shows the
  notification and the user turns it off there. `Granted` is the honest answer
  and `Undetermined` would send an application looking for a prompt that does
  not exist.

## 2. Not built yet — the debt

- **No camera is OPENED.** `camera` answers which devices exist; delivering a
  frame is a streaming stack (PipeWire, or V4L2 buffer queues) and is the larger
  half by a wide margin. The enumeration is useful on its own — it is what a
  settings screen needs, and it answers the question `permissions` cannot: the
  gate says who MAY, this says what is plugged in.

- **Location's STREAM.** The gate is built (below); what is absent is following
  `LocationUpdated(o, a{sv})` for as long as a session lives. THAT is the part
  with a different lifetime — a position feed is not a question with an answer —
  and it belongs with the capability rather than with the gate.

  This row previously said the whole of location "is a SESSION, not a request …
  belongs with the capability rather than the gate", and that was wrong: the
  session is one extra call and the consent is the very same Request handshake
  the camera uses. Corrected by reading the interface instead of reasoning about
  it.
- **macOS / iOS / Android have no backend.** They land on the neutral base and
  say so through `supported()`. Each is real work with its own story
  (`AVCaptureDevice` + `CLLocationManager`, `UNUserNotificationCenter`,
  `ActivityCompat.requestPermissions`), not a translation of this one.
- **No capability is actually opened.** This tier gates; it does not stream.

## 3. Works, but does not look like its name

- **`request(Location)` opens a session and closes it again.** `CreateSession`
  answers a session path DIRECTLY — not a Request, which is worth stating
  because several other portal interfaces spell `CreateSession` the other way —
  and `Start(session, parent, options)` is the call that asks. So the gate is
  the camera's handshake with one call in front of it.

  The session is closed once the answer is in, because this module has no reader
  for it: the stream is not built, so a session left open is a resource nobody
  holds, leaked once per call. The portal records the user's decision in its
  permission store (the `Lookup` it performs before asking is against exactly
  that), so closing does not normally discard a grant; if a backend chose not to
  persist one, the user is asked again when positions are first read, which is
  correct for an "allow once" answer rather than a bug.

- **A failed call answers `Undetermined`, not `Unavailable`.** The portal
  answered the proxy, so the service is plainly there; a method erroring or
  timing out means nothing was SETTLED. `Unavailable` is reserved for the portal
  not being reachable at all — using it for a slow call would tell an
  application to stop offering a feature because one round trip was late.

- **`request` CAN answer `Undetermined`.** It is bounded — two minutes — and a
  timeout is reported as `Undetermined` rather than `Denied`, because nobody
  refused and nobody was even asked. The next ask is fair.

  THE BOUND IS NOT THEORETICAL. On the GNOME session this was developed
  against, `AccessCamera` returns its Request path immediately and **no
  `Response` is ever emitted**: `xdg-desktop-portal` logs `Failed to associate
  portal window with parent window` and the consent dialog never appears —
  with a `parent_window` and without one. An unbounded wait therefore froze the
  caller outright, on a machine where nothing was wrong with this code. That is
  the desktop's backend, not this package, and it is exactly why a permission
  request may not block forever.

- **`request` takes a `parent` and the caller must supply it.** It is the
  portal's own `parent_window` — `x11:<hex>` or `wayland:<handle>` — and the
  consent dialog is parented to it. It is a PARAMETER rather than something
  this module discovers because `facet_device` depends on no UI backend: that
  boundary is why the tier is its own package, and a permissions module that
  imported facet_gtk to find a window would give it back.

- **`/dev/video*` IS NOT A LIST OF CAMERAS, and the difference is measured.**
  This machine exposes `/dev/video0` and `/dev/video1`, both from `uvcvideo`,
  both reporting the same card name — and only video0 captures. video1 is the
  UVC METADATA node. A scan of the device paths reports two identical cameras,
  one of which produces no picture.

  `VIDIOC_QUERYCAP` separates them, and reading the right field of its answer is
  the second half: `capabilities` describes the DRIVER across all its nodes
  (0x84a00001 here — capture set, on both nodes), while `device_caps` describes
  THIS node (0x04200001 on video0, 0x04a00000 on video1). Filtering on the
  driver word accepts the metadata node. `device_caps` is only meaningful when
  the driver sets `V4L2_CAP_DEVICE_CAPS`, and a driver too old to set it has
  only the one word — both cases are handled.
