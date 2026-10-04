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

`python tools/test_facet_winui_parity.py --grouped-items-only` checks grouped
lists and collections, empty headers, flat item indexes, scroll targets,
full-width grid headers, partial grid lines, live group changes and teardown.

`python tools/test_facet_winui_parity.py --list-refresh-only` checks list-local
begin/end refresh, native menu requests, availability, color/reset, retained
editor drafts, duplicate suppression, callback removal and teardown.

`python tools/test_facet_winui_parity.py --refresh-only` (from the repository root)
checks independent nested list/wrapper refresh, native refresh requests,
desktop menu activation, indicator color/reset,
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

`--row-retention-only` checks list and collection binding refreshes, native
editor identity and draft preservation across overlapping viewport changes,
selective row-kind replacement, factory replacement, nested sender indexes,
list appearance/disappearance counts and shutdown cleanup.

`--input-transform-only` checks Unicode display casing, preserved programmatic
source text, silent live transforms, native UTF-16 selection, default reset,
limited input, unchanged passwords, native edit readback and teardown.

`python tools/test_facet_winui_parity.py --popup-caption-only` checks fixed
popup captions, live Unicode changes, selection readback and placeholder reset.
The popup and date-format probes also exercise native events arriving while
a newer application selection/date/time is waiting for synchronization.

`python tools/test_facet_winui_parity.py --clear-button-only` verifies native
clear-button visibility, clearing/readback, callback counts, live switching,
read-only suppression and teardown.

`python tools/test_facet_winui_parity.py --splits-only` checks initial and live
pane bounds, resizing, conflicting constraints, collapse/restore on either
side, axis changes, native divider size/placement, and zero-thickness hiding.
It exercises the native drag handler's shared movement path and callback
suppression at a bound; it does not synthesize a physical pointer drag.
Shutdown must release every view and subscription.

Validation caveat: desktop runs have intermittently timed out on the first list
click. A combined run also exited with `0xC0000005` after the tree-row
callback-removal assertion, before its teardown marker; the isolated tree probe
then passed. Those failures remain undiagnosed. A successful retry is evidence
for that run, not a fix for either intermittent failure.

`python tools/test_facet_winui_parity.py --button-modes-only` exercises both
directions of live Button/ToggleButton replacement for ordinary, icon and text
buttons, including list rows, carousel pages, custom tree rows and a window
root. It checks stable Facet records, native callbacks, stale-event suppression,
stored checked state, focus, and closing with an unapplied mode change in either
direction. Retained old native objects must no longer call application handlers.

Secure/plain replacement: `python tools/test_facet_winui_parity.py --secure-modes-only`
checks original mixed-case text, native edits, silent mode changes, focus,
selection restoration, detached events and both pending-mode shutdown directions.
The probe releases retained native objects before the WinUI apartment ends.

`python tools/test_facet_winui_parity.py --time-open-only` checks initial/open/close
requests, native dismissal, duplicate suppression, live time changes, removal in
an opened callback, and closing the window with its flyout still open.

`python tools/test_facet_winui_parity.py --reordering-only` checks native drag
availability, flat source/destination readback, group restrictions, live policy
changes, data invalidation and safe removal before/during callbacks. These checks
exercise the move state directly; the gallery also tests actual mouse drags.

`python tools/test_facet_winui_parity.py --hybrid-only` creates local HTML/CSS/JS
fixtures, then checks Unicode and empty-message round trips, sends queued before
loading, live file/root changes, clearing/restoring content, missing-file status,
resource response overrides and removal from each of the four hybrid callbacks.
Every process checks zero remaining native views and subscriptions.

Swipe reveal: `python tools/test_facet_winui_parity.py --swiping-only` runs a
native probe for threshold changes/clamping, drag slop, vertical rejection,
cancellation, disabled state, action invocation, live replacement and removal
from each of the five callbacks. It also checks subscription/view teardown.

Swipe reveal: `python tools/test_facet_winui_parity.py --swiping-only` runs a
native probe for threshold changes/clamping, drag slop, vertical rejection,
cancellation, disabled state, action invocation, live replacement and removal
from each of the five callbacks. It also checks subscription/view teardown.

The swipe probe also checks auxiliary action identities, live private/open agent
policy, identity cleanup, hidden-action compaction and accessible-name resets.
