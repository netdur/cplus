# facet_winui

Experimental Windows x64 renderer for retained Facet trees using generated
WinUI bindings. Runs the shared 38-page Facet gallery, with native controls,
text entry, graphics, scrolling, viewport-based lists, and decorations.

Start with [the gallery launcher](../../examples/facet_gallery_winui/README.md).

```toml
[dependencies]
stdlib = "*"
events = "*"
flex_layout = "*"
facet = "*"
winrt = "*"
winui = "*"
facet_winui = "*"
```

```cplus
import "facet_winui/facet_winui" as host;
// host::run(tree, "My application") owns the tree through native shutdown.
```

Any number of windows run in the one WinUI Application: `host::start` and
`host::open_window` open them, each honours `screen::Chrome` (size, limits,
bar, buttons), and closing the last ends the run. A `runtime::App` runs here
unchanged after `facet_runtime/winui`'s `select()` - windows, routes,
`open_window` and keyed alert/choose/prompt sheets included; see
[examples/facet_runtime_winui](../../examples/facet_runtime_winui/README.md).

[Tutorial](docs/tutorial.md) · [Guide](docs/guide.md) · [Reference](docs/ref.md)
· [Coverage](MANIFEST.md)

Run the integration checks from the repository root:

```powershell
& examples/facet_winui_smoke/run.ps1 -Verify
& examples/facet_winui_smoke/run.ps1 -Verify -Release
& examples/facet_gallery_winui/run.ps1 -Verify -Release
& examples/facet_agent_winui_smoke/build.ps1
python tools/test_facet_winui_parity.py
python tools/test_facet_winui_pointer.py
python tools/verb_coverage.py winui --list --check
```

These test real pointer/keyboard/wheel input, text readback and programmatic
updates, length limits and read-only fields, scrolling, resize, control
replacement during its callback, and cleanup. `facet_runtime` still
defaults to Win32 on Windows; an app opts into this backend with
`facet_runtime/winui`. The app menu, a close-time `on_should_quit` and
density-change observation are not served through the facade yet
([MANIFEST](MANIFEST.md)).

The parity probe checks UTF-16 selection and selection-only events, Enter and
Ctrl+Enter submission, WebView history/reload/script execution and user-agent
reset, list single-selection, collection multiple-selection and scroll-to,
row-binding context, and teardown. It uses real desktop input and local HTML
fixtures; it does not require an external website.

It also checks live label/picker/input typography, restoration of native font
defaults, input alignment, popup/calendar open-close readback, and removal of a
popup from its own opened callback. Run only these checks with
`python tools/test_facet_winui_parity.py --styles-only`.

The styling checks also cover radio/text-button typography and native-default
reset, plus text-button decorations, alignment and optional borders. The
pointer probe checks button/icon-button presses and releases, canvas hover,
movement, dragging beyond its bounds, capture cancellation, and removal from
inside a pointer callback. It verifies that all subscriptions are released.

The verb-coverage gate currently fails: the gallery is not full Facet parity.
The report includes both missing implementations and indirect apply paths the
scanner cannot prove. Grouping/reordering, list-local refresh, advanced text styling,
carousel/swipe behavior, and several control events still need work. No
unsupported feature is waived just to make the gate pass.

The expanded probes cover Unicode display casing, line height, inline decoration,
button images and all four content layouts, icon-image fit, progress animation
and retargeting, keyboard scopes, scroll chaining, disabled picker items, tree
selection modes, list separators and cached variable row heights, and collection
threshold callbacks. The pointer probe also checks slider drags and toggle
activation through both physical input and MCP.
Run `python tools/test_facet_winui_parity.py --text-only` for text/content,
animated-image playback and progress-animation checks; `--styles-only` includes
these checks as well.

The refresh probe (`--refresh-only`) covers native requests and menu activation,
refreshing state, completion, indicator colors, duplicate suppression, initial
refresh, removal from a callback, queued-callback cancellation and shutdown.
The gallery Refresh page wraps its list in a refreshable host and simulates a
short asynchronous load; leaving the page cancels the sample timer.

`--layout-only` verifies automatic button/input heights after native templates
load, including dynamically replaced controls, preservation of explicit
heights and untrimmed automatic button captions at fractional display scaling. Each measured leaf invalidates its initial flex measurement once on
Loaded, then removes that event subscription.

`--choice-colors-only` checks actual native template brushes for toggle track/thumb
colors, checkbox color and radio border color/thickness, including initial
properties, live updates, theme reset, callbacks and cleanup. The gallery Controls
page has a button to switch between purple styling and the native theme.
These overrides apply to the normal state; native hover/pressed/disabled
feedback remains themed. Checkbox foreground and radio corner radius remain unmapped.

Slider thumb colors support initial properties, live updates and theme reset.
The text/content probe checks rendered brushes, and the pointer probe recolors
during a drag, drags again after the template update, and removes the slider
from its completion callback. The Values gallery page includes a color switch.

`--canvas-redraw-only` verifies that unrelated layout updates retain canvas
shapes, explicit `redraw()` refreshes the recorded drawing, size changes redraw,
and replacing the drawable with `Drawable::none()` clears the native children.

Slider pointer steps and keyboard increments both use one hundredth of the
configured range. The pointer probe uses a normalized 0–1 slider and verifies
that dragging produces intermediate values rather than snapping to endpoints.

`--borders-only` checks native border dash patterns and phase (in DIPs), line
caps, joins, miter limits, rounded and elliptical geometry, live stroke-width
changes, child replacement and default reset. Outlines are inset to keep the
stroke inside the allocated frame and do not intercept pointer input. The
Graphics gallery page cycles solid, rounded/dashed and elliptical/dotted borders.

Slider template replacement and live removal explicitly close the thumb value
tooltip. The native content probe keeps that tooltip open while recoloring;
the gallery checks that it does not survive navigation to another page.

`--tabs-only` checks native selection, persistent pane visibility, programmatic
callback suppression, invalid indices, child replacement, rendered tab colors
and theme reset, keyboard focus preservation, and removal from a selection callback. The Tabs gallery page
exercises native headers and live styling. Child keys supply header titles.
Tabs reserve 48 DIPs of top padding for the native strip; an invalid selected
index hides all panes. Programmatic selection and child changes emit no
`on_tab_changed` callback. Header hover/pressed feedback retains native colors.

`--paging-only` exercises native carousel selection, full-size child pages,
programmatic callback suppression, scroll-to, page replacement and insertion
anchors, native swipe/bars, scrolling readback and remaining-item thresholds.
Position/current-item handlers receive the model after native selection.
Scroll and threshold callbacks run outside native layout; removing the host
cancels pending delivery. Carousel indexes wrap by default; `wraps: false`
clamps them to the available pages. Native arrows still stop at the ends.
The gallery Carousel page now has four retained pages, native navigation,
page dots, first/last and Previous/Next commands, and live loop/animation switches.
Adjacent-page animation can be disabled for programmatic/mouse/keyboard input;
long jumps are immediate and touch retains native transitions. Touch/native-arrow
looping and peek insets remain gaps; see MANIFEST.md for the complete limits.

Page indicators use the theme primary color for the selected dot and translucent ink for other dots when no colors are specified. Clearing an explicit color restores these defaults. The paging probe checks both defaults and color reset.

Menu nodes now launch a native flyout containing `menu_item` and
`context_menu_item` children. `context_menu` hosts keep visible content separate
from their actions, and `toolbar_item` renders as a native action button.
Text, icons, destructive color, enablement and handlers update live; removing
an action or its host from a callback is safe. `--menus-only` exercises these
paths, including shortcut key/modifier changes, clearing and scope.

Context-item shortcuts register on their visible host so they are available
before the first flyout opening. Ordinary menu shortcuts are global; context
host shortcuts require focus inside that host. The gallery Menus page exercises
mouse actions, live styling, both keyboard scopes and removal from a shortcut.
Menu priority, nested menus and toolbar priority/placement remain work.

Tree row heights now use native item-container styles. Initial/live changes
preserve selection and expansion; clearing the height restores native sizing.
The gallery Tree page can switch between compact, taller and default rows.
