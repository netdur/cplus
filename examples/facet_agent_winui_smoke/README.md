# Facet WinUI agent verification

Exercises the optional connector through `facet_agent::in_app()`: real native
callbacks, retained text readback, privacy inheritance and control replacement.
The app closes itself and verifies native resource cleanup.

From the repository root: `./tools/test_agent_winui.ps1` (or `-Release`).
Logs are in `out/run.log` and `out/run.err`.

Additional parity probes use the same executable:

```powershell
./examples/facet_agent_winui_smoke/build.ps1 -Release
python tools/test_facet_winui_parity.py
python tools/test_facet_winui_pointer.py
```

These cover native input selection/submission, local WebView navigation,
list/collection selection, live styling and reset, picker events, button and
canvas pointer events, callback-time removal, and resource cleanup. They
require an unlocked desktop and temporarily move the mouse. Use
`--styles-only` on the parity script for the automatic styling/picker checks.

The expanded probes cover Unicode display casing, line height, inline decoration,
rendered slider track and popup title colors, calendar date/limit clearing,
button images and all four content layouts, icon-image fit, progress animation
and retargeting, keyboard scopes, scroll chaining, disabled picker items, tree
selection modes, list separator updates, animated-image playback, and collection threshold callbacks. The pointer probe also
checks slider drags and toggle activation through both physical input and MCP.
Run `python tools/test_facet_winui_parity.py --text-only` for the text/content
and progress-animation probe; `--styles-only` includes that probe as well.

`python tools/test_facet_winui_parity.py --lifecycle-only` checks deferred list
row events, row sender identity, callback-time host removal, collection height
providers, measurement caching, mixed-height grid geometry, threshold callbacks,
and scroll anchoring across insert/delete/append operations. It runs by default
with the full parity script.

`python tools/test_facet_winui_parity.py --actions-only` checks native menu
icons and destructive reset, interleaved content/action ordering, replacement,
and clicked/invoked delivery through the native automation peer. It includes
removal from either callback and runs with the full parity script.

`python tools/test_facet_winui_parity.py --date-formats-only` checks the rendered
calendar text for numeric/native formats, live date changes, and default reset,
plus rendered 12/24-hour time changes without spurious selection callbacks.
It runs with the full parity script.

`python tools/test_facet_winui_parity.py --refresh-only` (from the repository root)
checks native refresh requests, desktop menu activation, indicator color/reset,
duplicate suppression, completion and removal from the callback, initial refresh,
cancellation before delivery, and closing while refresh is active. The style
probe also checks accessibility name/help clearing, heading levels and Unicode
tooltip update/reset, including native accessible-name fallback.

Run `python tools/test_facet_winui_parity.py --choice-colors-only` from the repo
root to verify rendered toggle, checkbox and radio colors, radio border width,
live updates, native theme restoration, toggle callbacks and teardown.

`--canvas-redraw-only` checks explicit redraw, retained native shapes across
unrelated layout updates, resize invalidation, drawable clearing and cleanup.
The pointer suite also drags a recolored slider twice and removes it from the
second completion callback; text/content checks verify thumb colors and reset.

`--borders-only` verifies rendered native outline geometry, dash-unit conversion,
cap/join/miter properties, width changes, child replacement, reset and cleanup.

`python tools/test_facet_winui_parity.py --tabs-only` (from the repository root)
checks native tab selection, silent model writes and child replacement, invalid
indices, gradient precedence, rendered header colors and theme reset, focus
preservation during recoloring, and removal from the selection callback.

`--paging-only` checks native carousel position/current-item notifications,
programmatic suppression, replacement and all three insertion anchors,
scroll-to and clamping, swipe/bar settings, scrolling readback, remaining-item
thresholds, empty/refilled pages, and removal from either selection callback.

The paging probe also verifies initial and live circular indexes (including
negative and extreme values), switching wrapping off, wrapped scroll-to, and
animation on/off. It measures intermediate native offset and page geometry for
an adjacent animated change, then immediate arrival for a nonanimated change.

`--menus-only` verifies menu launchers, context-menu hosts and toolbar actions:
live text/icons/destructive colors and reset, automatic measurement, mixed
content/action order, counts, all three callbacks and removal of actions or
hosts during delivery. It also checks native keyboard accelerator key/modifier
updates, context scope, displayed hints, clearing and invalid-key rejection.
The final close leaves a scoped shortcut attached to exercise window teardown.

`--tree-rows-only` verifies initial and live native row heights, compact child
rows, selection/expansion and node-identity retention, default-height reset,
replacement-node styling, custom-row binding and kind changes, retained editor
state, initial/live/cleared automation row IDs, nested sender lookup after model replacement, callback removal and clean
teardown. Rendered sizes allow pixel rounding.
