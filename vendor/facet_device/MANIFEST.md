# facet_device — the manifest

What this tier answers, what it deliberately does not, and what the platform
would not let it. Read this before trusting an adjective about the package.

`permissions` is ONE SLICE: the gate every device capability shares. Opening a
camera, reading a microphone, streaming a location — none of those are here.

## What is live

| verb | Linux (XDG portal) | elsewhere |
|---|---|---|
| `supported()` | true when `org.freedesktop.portal.Desktop` answers | false |
| `status(Camera)` | `Unavailable` with no camera, else `Undetermined` | `Unavailable` |
| `status(Microphone)` | `Unavailable` with no `Device` portal, else `Undetermined` | `Unavailable` |
| `status(Location)` | `Unavailable` with no `Location` portal, else `Undetermined` | `Unavailable` |
| `status(Notifications)` | `Granted` when the interface is offered | `Unavailable` |
| `request(Camera)` | `AccessCamera` + the Request handshake, bounded | `Unavailable` |
| `request(Notifications)` | `Granted` — there is no gate to pass | `Unavailable` |

Measured on the session this was written against: the service is present, camera
/ microphone / location all answer `Undetermined`, notifications `Granted`.

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
- **Notifications have no request flow.** `org.freedesktop.portal.Notification`
  has `AddNotification` and nothing to consent to; the desktop shows the
  notification and the user turns it off there. `Granted` is the honest answer
  and `Undetermined` would send an application looking for a prompt that does
  not exist.

## 2. Not built yet — the debt

- **`request(Microphone)`.** `Device.AccessDevice(pid, ["microphone"], opts)` is
  one Request, the same shape as the camera's, so this is a small piece of work
  and not a design question. It answers `Undetermined` today.
- **`request(Location)` is a SESSION, not a request.** `CreateSession` then
  `Start`, with positions arriving as `LocationUpdated` signals — a different
  lifetime from a one-shot consent, and it belongs with the capability that
  consumes the stream rather than with the gate. `status` still answers.
- **macOS / iOS / Android have no backend.** They land on the neutral base and
  say so through `supported()`. Each is real work with its own story
  (`AVCaptureDevice` + `CLLocationManager`, `UNUserNotificationCenter`,
  `ActivityCompat.requestPermissions`), not a translation of this one.
- **No capability is actually opened.** This tier gates; it does not stream.

## 3. Works, but does not look like its name

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
