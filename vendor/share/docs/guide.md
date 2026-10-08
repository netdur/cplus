# Guide

What presenting a share means on each platform. Fast start:
[tutorial.md](tutorial.md). Signatures: [ref.md](ref.md).

## What `Ok` means

The sheet was **presented**. That is the only fact any of these platforms
reports honestly.

iOS's `completionWithItemsHandler` reports cancellation reliably and reports
*success* wrongly for several targets — a share to an extension that fails
after the sheet dismisses still says completed. Android's `createChooser`
reports nothing whatsoever: `startActivity` returns and the app never hears
again. macOS needs a delegate to hear anything and still cannot tell "sent"
from "opened the composer and quit".

So this package returns no such answer rather than a wrong one.

## Anchoring, and the iPad crash

Neither Apple platform will show a sheet without being told where to point.

**iOS.** On iPhone the sheet slides up from the bottom and needs nothing. On
**iPad it is a popover**, and UIKit throws `NSGenericException` — *"your
application has presented a UIPopoverPresentationController with a nil
sourceView"* — if nothing set one. That is a crash, not a layout glitch, and it
is the most common way this API is got wrong. The backend always sets the
anchor: the presenting controller's view, centred, with no arrow.

**macOS.** `showRelativeToRect:ofView:preferredEdge:` is the only entry point.
The backend anchors to the key window's content view.

Both are guesses about where the user's eye is. An app with a share *button*
would rather anchor to the button and cannot from here — a `source_rect:`
parameter would mean nothing on Android, and one portable verb is worth a
centred popover.

## `subject`

| | |
|---|---|
| Android | carried as `EXTRA_SUBJECT`; mail clients read it |
| iOS | needs a delegate implementing `activityViewController:subjectForActivityType:` |
| macOS | needs `NSSharingService.subject`, which means choosing a service first — the opposite of showing a picker |

Rather than synthesise a delegate for a field Mail alone reads, both Apple
backends drop it. Pass it anyway: it costs nothing and works where it works.

## A URL is not text

Every Apple platform gives a real `NSURL` a richer treatment than a string that
happens to look like one — a preview card, a different app list, "Copy Link"
instead of "Copy".

**Android has no equivalent.** There is no link intent: `ACTION_SEND` with
`text/plain` is what every app does, and receivers parse the URL out of
`EXTRA_TEXT`. So `url()` and `text()` are the same call on Android and
different calls on Apple, which is exactly why the facade keeps them apart.

## Files on Android: a `content://` URI, from a provider the app declares

`file://` URIs have thrown `FileUriExposedException` since API 24, and
androidx's `FileProvider` is an AAR facet does not ship. So facet_android
carries its own: `cplus.facet.FacetFileProvider`, read-only, serving only
files copied into `<cacheDir>/cplus-share/`. `file(path)` copies the file
there and sends `content://<package>.cplus.files/<name>` through a chooser,
with a read grant that travels to the app the person picks.

A provider has to be declared in the app's manifest, as `FacetActivity`
already is, inside `<application>`:

```xml
<provider android:name="cplus.facet.FacetFileProvider"
          android:authorities="<your package>.cplus.files"
          android:exported="false" android:grantUriPermissions="true"/>
```

Without it, `file()` answers `Unsupported`, and logcat (tag `facet`) prints
that line with your package filled in, once. The provider is created by the
system from the APK, so facet_android's dex has to be merged into the APK,
which is how facet apps are built.

Windows `file()` is still `Unsupported`.

## Windows: WinRT reached as plain COM

`DataTransferManager` is a WinRT class, and WinRT is COM once you stop looking
for a projection: `RoGetActivationFactory` answers an interface pointer and
every call after it is a vtable index. That is how this backend works — no
C++/WinRT, no generated projection, nothing on the link line (combase is bound
with `LoadLibrary` at first use, so a machine without it answers
`available() == false` rather than failing to start).

Two things about it are worth knowing as a caller.

**It needs a window and a running message loop.** The sheet is anchored to an
HWND (`IDataTransferManagerInterop::GetForWindow`), and it asks for the payload
*later*, by raising `DataRequested` on that loop. `share::text` returns as soon
as the sheet has been asked for — which is all `Ok` ever promised — and the data
is handed over afterwards. A process with no window, or one that never pumps,
gets `Unsupported` or a sheet that never asks.

**A title is not optional.** A `DataPackage` with no title makes the shell show
a "nothing to share" pane rather than an error, which is the most confusing way
this can fail, so a share with no `subject` still gets one.

## Linux: no system share sheet

The XDG portals provide `OpenURI`, `OpenFile`, and `ComposeEmail`, but none is a
chooser that hands an item to an application selected by the person. Opening a
URL in the browser or a file in its default editor would be a different verb,
so the Linux backend reports `available() == false` and every share returns
`Unsupported`.

### It is not blocked by package identity, and an earlier note said it was

A previous pass measured the sheet opening and `DataRequested` never firing, and
recorded package identity (MSIX / sparse manifest) as the suspect. That was
wrong. Measured again from an ordinary non-packaged process — with
`GetCurrentPackageFullName` confirming `APPMODEL_ERROR_NO_PACKAGE` — the event
fires and every call inside the handler answers `S_OK`, reproducibly, with and
without an explicit AppUserModelID. The earlier harness had a defect that was
never isolated. It is written down because a false "cannot" in a header is
exactly what stops the next person from trying.

## Always a chooser

`startActivity` on a bare `ACTION_SEND` lets Android remember a default, so a
person who once tapped "always" can never share anywhere else. `createChooser`
forces the sheet every time, which is what the other two platforms do and what
"share" means.

## No Java, no dex

Like `haptics`, this is a call out with no callback in — nothing to implement,
so nothing to compile.

## What was measured

Nothing yet — **a sheet cannot be asserted**. The suite covers the vocabulary,
the empty-input guards and that sharing without a window degrades rather than
traps. Whether the sheet looks right needs eyes.
