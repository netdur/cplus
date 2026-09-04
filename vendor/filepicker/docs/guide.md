# Guide

## Why there is no blocking form

`NSOpenPanel.runModal` is the obvious call and it spins a **nested run loop**:
timers fire, the app's own handlers run, and a facet tree can be mutated from
inside a frame that is still inside the caller's. That is a reentrancy problem,
not a convenience.

`beginWithCompletionHandler:` answers on the main run loop with the stack
unwound, which is what iOS and Android do anyway. One shape, three platforms.

## A cancel is an answer

Every backend calls the handler with an empty path when the person dismisses
the picker. That is deliberate: a handler that only fires on success leaves an
app waiting forever for something that already happened.

On iOS this needs **two** delegate methods — `didPickDocumentsAtURLs:` and
`documentPickerWasCancelled:`. A picker that implements only the first hangs on
Cancel, and it is the most common way this API is got wrong.

## `path` is a `content://` URI on Android

`ACTION_OPEN_DOCUMENT` answers a provider URI, not a filesystem path. It is
handed through as `path` because it is what the platform gives:

```cplus
// Android: this will NOT work
let f = fs::open_read(p.path);

// Android: read it through the resolver
// ContentResolver.openInputStream(Uri.parse(p.path))
```

The permission it carries is **per-process** and dies with the app unless
`takePersistableUriPermission` is called — a decision about long-term access
that a picker should not make silently.

## The types filter is lossy, on purpose

`types` is a comma-separated list of extensions without dots — `"png,jpg"`.

| | |
|---|---|
| macOS | `setAllowedFileTypes:`, which takes exactly these strings |
| Android | mapped to **one** MIME type; anything unrecognised becomes `*/*` |
| iOS | **ignored** — the picker is opened for `public.item` |

iOS wants `UTType` objects, and building one from three letters is a guess for
anything unusual. Filtering wrongly hides a person's own files from them, which
is worse than not filtering — so the iOS backend does not pretend.

## `save` does not exist on iOS

iOS has no "choose where to write" picker. The nearest shape is
`initForExportingURLs:`, which takes a file that **already exists** and asks
where to copy it. An app wanting the macOS flow writes into its own container
first and exports that.

So `save()` answers `Unsupported` on iOS rather than approximating.

## Android answers through `E_NATIVE_RESULT`

This is the only mobile backend here with **no Java class and no dex**, because
facet already owns the door: `FacetActivity.onActivityResult` fires
`app_events::E_NATIVE_RESULT`, and this package subscribes and matches its own
request code.

That kind had been **declared and documented in app_events since the sign-in
work and never fired** — the override did not exist. It does now, which is why
`facet_android.dex` changed in the same commit.

Another package's `startActivityForResult` lands in the same fan-out, which is
exactly why the request code is carried rather than swallowed.

## iOS returns a security-scoped URL

A file chosen outside the app's container comes back scoped, and reading it
without `startAccessingSecurityScopedResource` fails with an error that names
nothing useful. The picker is opened **as a copy** (`inMode:` import), which
sidesteps it — the alternative hands back a URL into another app's container
that stops working when that app suspends.

## What was measured

Nothing yet — **a picker cannot be asserted**. The suite covers the vocabulary,
the empty-input guards, and that a cancelled pick is an empty path rather than
an error.
