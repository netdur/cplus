# Facet Gallery on WinUI

Open **Launch Gallery.cmd**, or run from the repository root:

```powershell
& examples/facet_gallery_winui/run.ps1 -SkipBuild
```

The launcher uses the staged executable and sets the working directory for
the gallery's image assets. If the executable is missing it builds first.
Close an existing gallery before rebuilding.

This runs the existing `examples/facet_gallery` component and its 36 demo
pages through `facet_winui`. `build.ps1` stages those sources into the ignored
`src/shared` directory; edits belong in the original gallery. The application
and backend are C+. The WinUI projection is generated from WinMD and remains
usable independently of Facet.

Try buttons, text entry and password masking, pickers, icons, image fitting,
canvas drawing, draggable splits, scrolling lists, the Relist row stars and
“Scroll to 120”, animation controls, gradients, clipping, and shadows.
The shared gallery retains some AppKit-specific explanatory captions.

Rebuild or run desktop regression checks:

```powershell
& examples/facet_gallery_winui/build.ps1 -Release
& examples/facet_gallery_winui/run.ps1 -Verify -Release
& examples/facet_gallery_winui/verify_walk.ps1
```

Checks use real mouse/keyboard input and Windows UI Automation, save screenshots
under `out`, and close the test window. Python with Pillow is needed for these
checks, not for ordinary launching. Runtime staging uses the same pinned SDK
packages as `examples/winui_standalone` and embeds its deployment manifest.
`verify_walk.ps1` uses the existing build, visits all 36 pages, and verifies
clean shutdown with the final page's list rows still mounted.

Set `FACET_GALLERY_WALK=1` before launching to visit all 36 pages automatically.
The completion line is in `out/run.log`; the window stays open. Unset the
variable for normal navigation.

This is a testable gallery, not full `facet_runtime` integration. See the
[backend coverage](../../vendor/facet_winui/MANIFEST.md) for precise limits.
The original carousel demo deliberately contains an empty page host; touch
swipe actions and the Facet agent service are not implemented here.
