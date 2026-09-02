# Guide

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

## Files on Android are `Unsupported`

`file://` URIs have thrown `FileUriExposedException` since API 24. A real file
share needs a `FileProvider` declared in the **app's** manifest with an
authority and an XML path list — an app-level arrangement this package cannot
make on the caller's behalf.

So `file()` answers `Unsupported` on Android rather than handing the system a
URI it will refuse. Said rather than half-done.

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
