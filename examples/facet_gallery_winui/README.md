# Facet Gallery on WinUI

Open **Launch Gallery.cmd**, or run from the repository root:

```powershell
& examples/facet_gallery_winui/run.ps1 -SkipBuild
```

The launcher uses the staged executable and sets the working directory for
the gallery's image assets. If the executable is missing it builds first.
Close an existing gallery before rebuilding.

This runs the existing `examples/facet_gallery` component and its 37 demo
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
`verify_walk.ps1` uses the existing build, visits all 38 pages, and verifies
clean shutdown with the final page's list rows still mounted.
The interaction check also opens the picker, verifies browser content, resets
an animation, checks the Accessibility page’s native hover tooltip, exercises
timed refresh and navigation during a load, switches the Controls page between
custom colors and theme defaults, tests the Values page slider color switch,
cycles Graphics border outlines through real clicks on their child control,
and closes mid-animation to check
rendering-handler cleanup.
Runtime staging includes WinUI's acrylic texture and WebView2's native SDK DLL;
the installed Edge WebView2 browser runtime supplies the browser engine.

Set `FACET_GALLERY_WALK=1` before launching to visit all 38 pages automatically.
The completion line is in `out/run.log`; the window stays open. Unset the
variable for normal navigation.

This is a testable gallery, not full `facet_runtime` integration. See the
[backend coverage](../../vendor/facet_winui/MANIFEST.md) for precise limits.
Swipe actions use a desktop context menu; touch swipe actions remain work.
The gallery does not start the optional Facet agent service.

The Tabs page demonstrates native header selection and live colors. Desktop
verification checks mouse selection, Ctrl+Tab, and checkbox state retained while
switching panes, then navigates away to exercise tab cleanup.

The Carousel page now supplies four retained child pages. Verification clicks
the native next arrow, sends an arrow key, jumps first/last, and checks that a
checkbox stays checked across page changes. It also exercises the Previous/Next
buttons across the ends with looping enabled/disabled and toggles adjacent-page
animation. To run only this desktop check:

```powershell
./run.ps1 -Verify -VerifyMode carousel -Release -SkipBuild
```

The Menus page includes a native flyout launcher, toolbar action and right-click
context host. It supports live styling, Ctrl+D as a menu command, Ctrl+I while
the context text field is focused, and Ctrl+R to remove the menu and unregister
its shortcuts. Desktop verification checks mouse actions, accessibility names,
shortcuts before first opening, scope and callback removal:

```powershell
./run.ps1 -Verify -VerifyMode menus -Release -SkipBuild
```

The Tree page's **Change row height** button cycles through compact, taller and
native-default rows while keeping selection and expanded branches. Verify it with
`./run.ps1 -Verify -VerifyMode tree-rows -Release -SkipBuild`.

The Tree page also offers **Toggle custom rows** and **Rebind labels**. Edit a
row draft, rebind its label, and press **Use** to check nested action identity.
The tree verification exercises these controls and saves
`out/verified-tree-custom-rows.png`.
