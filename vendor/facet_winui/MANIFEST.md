# Experimental backend coverage

Windows x64, one window and one WinUI Application run per process. The explicit
host is `facet_winui/facet_winui::run`. The shared gallery's 38 pages mount;
this is not a claim that every Facet property is implemented.

| Contract | Implemented scope |
|---|---|
| Containers/layout | Border with Canvas document, Facet/flex_layout frames, native leaf measurement refreshed after initial template load, resize, z-order and display/visibility. |
| Text | TextBlock, font size/weight/italic/color/family, spacing, text scaling, horizontal alignment, underline/strikethrough, wrapping/max lines, selectable text, formatted runs and standalone spans. Unicode display casing preserves source text; line height and inline decorations update live. Label font/color overrides restore native defaults when cleared. |
| Actions | Button, initial toggle-button mode, text buttons, compact icon buttons and click callbacks. Button/icon-button pointer press/release callbacks, including handled native events. Text-button typography, decoration, alignment and optional borders. Button image/symbol placement in all four content layouts, icon-image fit and border resets. Native toggle readback also works through automation. Bundled Material glyphs and gallery SF-name aliases. |
| Inputs | TextBox, PasswordBox, search and multiline entry, max length, read-only, placeholder, native edits before callback, programmatic edit suppression. UTF-16 caret/selection setters and readback, selection-only notifications, Enter submission (Ctrl+Enter for multiline), input font/spacing settings, keyboard scopes including secure numeric PIN entry. Live upper/lowercase display for plain inputs preserves programmatic source text and uses native casing for edits. |
| Values | CheckBox, ToggleSwitch, RadioButton, Slider, NumberBox, ProgressBar, ProgressRing and corresponding change callbacks. Radio typography and native-default resets. Slider thumb drag-start/completion callbacks and normal-state minimum/maximum track colors with native-default resets. Progress color and frame-driven easing, retargeting and cancellation. |
| Pickers | ComboBox items/selection/alignment and per-item enabled state, CalendarDatePicker date/min/max, TimePicker time, change callbacks. All three map font/spacing/scaling/color with native-default resets. Popup title color maps separately from selected text; calendar date and bounds can be cleared. Date formats map native patterns and eight common numeric forms; time formats select native 12/24-hour display. Popup and calendar open/close events update state and defer user callbacks safely outside native apply. |
| Carousel | Native FlipView over retained child pages; initial/live position and scroll-to, circular programmatic indexes, adjacent-page animation switch, native position/current-item callbacks, scroll observer and scrolling readback, swipe enablement, bars, insertion/removal anchors and remaining-item threshold. |
| Tabs | Native TabView headers; child keys label persistent Facet panes. Initial/live selection, selection readback before callbacks, bar brush/color, text and selected/unselected fills with theme resets. |
| Scrolling | ScrollViewer document extent, axes/bars, initial/live offsets, actual offset observers, input chaining and native wheel scrolling. |
| Refreshable | Native RefreshContainer around scrollable content, refreshing state, indicator color/reset and deferred refreshing callback. Desktop Refresh context menu; finish by setting refreshing false. |
| Split/tree | Draggable Thumb divider, limits/collapse layout, TreeView hierarchy, expansion, single/multiple/none selection, selection clearing and native selection readback. Custom Facet rows, binding, row-kind replacement, live row automation IDs and nested sender model lookup. |
| List/collection/relist | ScrollViewer with a viewport-sized set of owned Facet rows. Count/builder/bind, row-kind-aware retained subtrees, columns, scroll-to, and cached variable list/collection heights supplied by row_height_of. Collection first/all-item sizing and tallest-item grid rows. List separator color/visibility, list/collection selection and highlights, tap/selection callbacks, scroll observers and bar policies. Deferred list appearance/disappearance callbacks preserve row sender lifetime. Collection item/offset/bottom anchoring follows inserts and removals. Collection threshold callbacks use the visible viewport and permit append/removal from callbacks. Relist observable changes, keyed live rows, inherited sender item identity, deterministic release. |
| Images | BitmapImage and ImageBrush, file/URL source, fit/fill/center modes. Retained image sources preserve animation across style updates; animated bitmap play/stop and source clearing are mapped. Bundled icon font staged with the app. |
| Drawing | Native shapes for recorded rectangles/rounded rectangles, ellipses, lines and move/line/quad/cubic paths; text, fills, gradients, stroke width and opacity. Canvas hover/move, primary pointer press/release, captured drag and cancellation callbacks. Drawn shapes do not intercept hit testing. |
| Decorations | Semantic/theme colors in dark appearance, borders/corners, gradient strokes, DIP dash patterns/phase, caps, joins, miter limits, rounded/per-corner/ellipse stroke outlines, gradients, 2D transforms, frame-synchronized opacity/transform easing, composition rounded/ellipse clips and drop shadows. |
| Menus and actions | Inline menu launchers with native MenuFlyout items; context-menu hosts with retained visible content; toolbar action buttons. Live text, icons, destructive foreground/reset, enablement, accessible toolbar names, click handlers and safe callback removal. Context-item keyboard shortcuts register on the visible host, including before the first menu opening; context actions are scoped. |
| Other gallery hosts | Circle/square page dots, table document container, swipe actions through drag reveal and native context menu, with image/named icons, destructive foreground/reset, stable interleaved insertion order, and clicked/invoked callbacks. WebView2 source/user-agent updates, history/back/forward, reload, script execution, navigation and process-failure callbacks. |
| Services | UI-thread delayed callbacks/cancellation, size observers, renderer dispatch, dark-theme query. |
| Agent integration | Optional lifecycle/apply/release hooks; `facet_agent_winui` connects keyed controls and privacy policies to `agent_winui`. In-app sessions and UI-dispatched MCP requests. |
| Accessibility | Native WinUI peers, stable control AutomationIds, explicit accessibility name/help text with native-default restoration, heading levels 1–9 and attached Unicode tooltips with live updates/removal. |
| Ownership | Native records owned by nodes, event removal before freeing callback contexts, row unrealise before drop, timers cancelled on release, zero view/subscription assertions on shutdown. |

## Limits

- `facet_runtime` still selects Win32. Multiwindow host services, dialogs,
  clipboard, jobs/worker-thread dispatch, general gesture/key readers and host
  focus commands remain separate work. Agent HTTP serving/inspection is
  available through the optional WinUI agent connector.
  `run_on_main` currently queues only from the UI thread.
- Collections report native drag/drop moves through flat reorder indexes and
  `on_reorder_completed`; the application updates its data order. Cross-group
  moves require `can_mix_groups`. List reorder and drag-edge auto-scrolling
  remain unimplemented. Uneven list rows
  and variable-height collections require `row_height_of`; automatic content-driven
  measurement remains work. Nonpositive callback heights use the fallback pitch.
  Overlapping rows survive viewport changes. Binding refreshes preserve rows
  with the same factory and kind; changed kinds replace only their rows.
  Offscreen rows are released; there is no cross-item reuse pool yet. Relist's current adapter uses
  uniform pitch, with a stated first-row height when supplied.
- Custom tree rows retain one Facet subtree per model node, including collapsed
  nodes; there is no viewport-based subtree reuse pool yet. `row_kind` controls
  replacement of each retained row. Positive row heights
  use native container styles; nonpositive values restore native measurement.
  Selection readback follows native selection notifications; direct
  external edits to WinUI's SelectedNodes vector can bypass those notifications.
- Menu nodes are inline launchers, not an application menu bar. Nested menus,
  menu priority, toolbar priority and primary/secondary placement remain unmapped.
  Flyout actions must be children of a menu, context-menu or swipeable host;
  context-menu hosts can also contain visible children.
- Action icons share the small named-icon mapping used by icon buttons; arbitrary
  platform symbol names are not supported. Destructive actions use the danger
  foreground in the normal state and retain native interaction-state feedback.
- Carousel currently uses native one-page-at-a-time FlipView behavior. Touch/native-arrow looping,
  peek insets, multiple columns, bounce policy, writing scrolling state,
  drag-state readback and header/footer/empty slots remain unmapped. Touch
  swiping has native enablement wired but has not been physically exercised. Positions
  wrap modulo the page count when `wraps` is true (the default), or clamp
  at the ends when false. Native arrows still stop at the ends. `animates_scroll`
  selects native adjacent-page animation for programmatic/mouse/keyboard changes;
  long jumps remain immediate and touch retains native transitions. Swipe
  actions support desktop drag reveal and a context menu; physical touch is unverified. Lists support refresh
  directly or through a `refreshable` host. Native pull gestures require supported
  touch input and a descendant ScrollViewer; physical touch remains untested.
- Plain text-field clear-button policies are mapped; password clear-button
  and return-key policies remain unmapped. PasswordBox has
  no caret/selection API. Secure/plain text-field changes and button toggle
  modes support live replacement. Text inputs map horizontal/vertical alignment
  and reset font/color overrides. Several other per-control styling properties
  remain incomplete. TimePicker open/close uses an owned native flyout. Date-format translation
  is limited to the numeric forms listed in the guide; other formats use WinUI
  grammar. Time-format support controls hour cycle only, not arbitrary layout
  or seconds.
- Symbol fill-axis variation, arbitrary platform icon names, image completion
  notifications and rich-text links are not wired. WebView requires
  the installed WebView2 runtime. Hybrid-web local resources, messages and lifecycle
  hooks are mapped; browser storage across explicit root/file reloads and engine
  initialization failure recovery remain unsupported.
- Border stroke outlines do not implicitly clip their content or reshape the
  background; use the separate clip property when clipping is wanted. Nonpositive
  miter limits restore the native default (10); negative dash lengths clamp to
  zero, and an all-zero dash pattern is treated as solid.
- Canvas caches recorded native shapes until explicit redraw, drawable changes
  or size changes. It supports the gallery's drawing subset, not all DrawOps:
  general clip/save/restore/blend/dashes/arc/image commands need more work.
- Toggle track/thumb colors, checkbox color and radio border color/thickness
  support initial and live updates and clearing back to theme defaults. These
  customize normal states, preserving native interaction-state feedback. Checkbox
  foreground and radio corner radius are not mapped.
- Slider track and thumb colors apply to the normal visual state; WinUI retains
  its theme feedback for hover, pressed and disabled states. Thumb color changes
  during a drag wait until release, then rebind the replacement native thumbs.
  Thumb images remain unmapped.
- WinUI uses trailing ellipsis for head/middle truncation and native wrapping
  for both word/character wrap.
- Appearance is dark; OS appearance changes, native system palette tracking,
  3D transforms, per-corner composition clipping and inherited container enable
  state are not complete.
  Shadows use rectangular masks on children with a native panel parent;
  root-window shadows and arbitrary shape masks are not mapped.

Initial unsupported trees/adopted views/children under leaf controls return
E_NOTIMPL. Unsupported kinds added later and failed native HRESULTs produce a
fatal diagnostic. Runtime error recovery is not yet implemented.

## Modifiers

```modifier
swipeable.reveal_threshold     swipe::finish reads the current threshold at release; nonpositive/NaN means half the strip and values above its width clamp. Native swiping probe covers initial/live thresholds, clamp and cancellation.
carousel.wraps                 changes normalization of initial/live position and nonnegative scroll-to indexes in paging::sync; modulo when true, clamped when false. The paging probe covers initial, negative, extreme, and live changes. Native arrows/touch still stop at the ends.
carousel.scroll_anchor         paging::insert/remove preserves the selected item or offset according to the policy; paging.cplus smoke probe exercises structural changes.
```

## Indirect-path audit (2026-10-03)

The baseline scanner reported 261 live verbs, one modifier, and 109 absent.
Of those 109, 63 had recognized reads. All 63 were reviewed below: 44 have
indirect live paths, three are readback/observer policies, one is a structural
modifier, and 15 remain unresolved or partial. The 46 entries without recognized
reads were not covered by this audit. Handler coverage is unchanged.

The subsequent split batch fixes six of those 15: both panes' bounds now share
one layout/drag clamp, collapse hides panes and divider, and divider geometry
uses the requested thickness. The native `splits` probe exercises these together.
The split batch brought the ledger to 50 reviewed-live entries and nine unresolved
recognized-read entries. Live button-mode replacement subsequently accounts for
three more, bringing the ledger to 53 reviewed-live and six unresolved reads. The 44 existing paths found by the audit are not new
implementations; the six split entries include actual fixes.

`reviewed-live` is a manual source audit, separate from the scanner's automatic
`live` count. It does not mean every value or transition has a native test.
The scanner requires a recognized read and rejects nonexistent names, but its
read heuristic cannot verify the evidence or prevent a later semantic regression.
These rows must be reviewed when their implementation changes.

Paths below are relative to `src/`. Existing smoke probes live in
`examples/facet_agent_winui_smoke/src/` at the repository root. The `borders`,
`texts`, `styles`, `paging`, and `menus` probes provide regression evidence for
parts of the corresponding paths; they do not exhaustively validate every row.
In particular, all page-dot combinations, symbol setters, selection-mode
transitions, scroll-axis changes, and the three input length-limit policies need
stronger dedicated native coverage. Relist has gallery coverage for observable
updates, appending, grid layout and scrolling; that is not a complete builder
replacement/observable lifetime test.

```reviewed-live
refreshable.is_refreshable     refreshing::enabled/visible are shared with lists; apply updates action availability and visualizer visibility, and native requests obey the policy; refresh probe.
refreshable.refresh_color      refreshing::color feeds the shared visualizer foreground/reset path; refresh and list_refresh probes verify custom color and native default restoration.
text_field.is_secure           views::reclassify_leaf replaces TextBox/PasswordBox; editing stores the actual native class, reapplies all editor properties and suppresses stale events; secure_modes tests both directions and pending-mode teardown.
button.toggles                 views::reclassify_leaf replaces the native Button/ToggleButton while keeping the View and Facet node; native parent slots are rebound and events use stored subscription mode; button_modes probe.
icon_button.toggles            same replacement path with media callback and pointer subscriptions; button_modes probe checks both directions, silent programmatic changes and stale native events.
text_button.toggles            same replacement path with rebuilt title content and choices callback; button_modes probe checks both directions and pending-mode teardown.
bordered.stroke_dash           borders::apply converts the dash pattern and sets the native shape stroke; borders probe covers changes and reset.
bordered.stroke_shape          borders::apply/place updates the outline geometry; borders probe exercises shapes.
bordered.stroke_width          borders::apply/place updates stroke thickness and inset geometry; borders probe.
button.border_width            buttons::apply updates border thickness, including default reset; texts probe.
button.bordered                buttons::apply switches native background/border presentation; texts probe.
button.symbol                  buttons::apply rebuilds icon/title content; texts probe exercises content replacement.
checkbox.on                    choices::apply boxes the branch-local boolean for IToggleButton.set_is_checked; changed writes native state back.
collection.selection_mode      items::selected/tapped uses the current policy for highlighting and selection changes; source reviewed, transition matrix not exhaustively tested.
collection.scroll_to           items::apply consumes the command; items::place computes row offset and calls change_view.
context_menu_item.shortcut     actions::apply delegates to shortcuts::apply, which updates/removes the native accelerator; menus probe and gallery shortcut checks.
icon_button.border_width       media::apply updates native thickness or clears its dependency property; source reviewed.
icon_button.bordered           media::apply switches background and border, restoring theme defaults when enabled; source reviewed.
icon_button.symbol             media::apply builds glyph content or image content from the current properties; source reviewed.
label.line_height              views::configure delegates to typography::line_height; texts probe checks height and reset.
label.formatted_text           views::configure delegates nonempty spans to typography::runs, which clears and rebuilds native inlines; texts probe checks run decoration, not all span transitions.
list.selection_mode            items::selected/tapped uses the current policy for highlighting and selection changes; source reviewed, transition matrix not exhaustively tested.
list.separator                 items::apply refreshes existing row borders through separator; styles probe checks removal.
list.scroll_to                 items::apply consumes the command; items::place computes row offset and calls change_view.
page_dots.count                media::apply rebuilds the native shape collection from current count; source reviewed.
page_dots.hides_single         media::apply suppresses the single-dot collection; source reviewed.
page_dots.dot_color            media::apply uses the current normal brush or its default when rebuilding dots; paging probe covers colors/reset.
page_dots.dot_size             media::apply sets each shape's size and spacing, defaulting to eight points; source reviewed.
page_dots.dot_shape            media::apply creates rectangle or ellipse shapes from the current policy; source reviewed.
page_dots.max_dots             media::apply caps the shape count for positive limits; source reviewed.
page_dots.selected_dot_color   media::apply selects the current selected brush or default; paging probe covers colors/reset.
radio.text                     choices::apply transforms and boxes the current text into native content; texts probe checks rendered content.
radio.on                       choices::apply boxes the branch-local boolean for IToggleButton.set_is_checked; changed writes native state back.
radio.text_transform           choices::apply delegates current text and transform to typography::transformed; texts probe checks Unicode content.
relist.rows                    items::apply invalidates; items::place reads count_of(rows), rebuilds visible rows and uses the relist release protocol; gallery observable-update coverage.
relist.builder                 items::place invokes the current builder/context after invalidation; source reviewed, builder replacement needs dedicated coverage.
relist.columns                 items::place uses current columns for widths, row positions and visible range; gallery grid coverage.
relist.scroll_to               items::apply consumes the index; items::place computes the target and calls change_view; gallery scrolling coverage.
scroll.axis                    views::configure_scroll and size_scroll update native scroll modes and content sizing from the current axis; source reviewed.
search_field.max_length        editing::apply reads InputViewProps through the owning-kind helper and sets native MaxLength; source reviewed, dedicated boundary tests needed.
span.line_height               media::apply delegates to typography::line_height; texts probe checks live height and reset.
split.axis                     views::prepare_layout chooses flex row/column; divider::place/dragged uses the same axis; splits probe changes axes and checks geometry.
split.min_leading              divider::constrained supplies layout and drag bounds; splits probe covers live changes, contradictory bounds and host-size clamping.
split.min_trailing             divider::constrained subtracts divider thickness before enforcing the trailing minimum; splits probe checks actual pane frames.
split.max_leading              divider::constrained enforces the leading maximum in layout and drag; splits probe checks changes, reset and conflict precedence.
split.max_trailing             divider::constrained raises the leading floor using available extent minus trailing maximum; splits probe checks initial layout, live resize and reset.
split.collapsed                views::prepare_layout hides each collapsed pane and removes the gap; divider::place hides the thumb; splits probe covers either/both panes and restoration.
split.divider                  divider::thickness supplies the layout gap and native line dimension; a transparent thumb keeps at least a six-point hit area; splits probe covers live sizes, axis changes, zero thickness and rendered pixel rounding.
symbol.icon                    media::apply delegates the current nonzero code point to icon, which sets native glyph text; source reviewed.
symbol.size                    media::icon sets current font size or the 24-point default; source reviewed.
symbol.color                   media::icon sets current brush or default ink; source reviewed.
symbol.font                    media::icon sets current FontFamily or the bundled symbol font; source reviewed.
text_area.max_length           editing::apply reads InputViewProps through the owning-kind helper and sets native MaxLength; source reviewed, dedicated boundary tests needed.
text_button.bordered           choices::apply computes native border thickness from current bordered/width values; source reviewed.
text_button.border_width       choices::apply sets thickness from the current width when bordered; source reviewed.
text_field.max_length          editing::apply maps the common limit to TextBox or PasswordBox MaxLength, with native-range clamping; source reviewed, dedicated boundary tests needed.
```

```derived
carousel.is_scrolling          paging::scrolled writes native ViewChanged.is_intermediate back; this is readback, not support for programmatically starting a scroll. Physical touch remains untested.
carousel.remaining_threshold   paging::remaining observes the current threshold and schedules a deferred callback, suppressing duplicate notifications; paging probe.
collection.remaining_threshold items::remaining observes the current threshold and visible range, scheduling a deferred callback with re-entry suppression.
collection.reorder             item_reorder::deliver writes flat source/destination indexes before on_reorder_completed; the application owns data order. Native cancellation/removal probe and real gallery mouse-drag checks.
```

The remaining five recognized-read entries stay in debt:

| Verb | Audit result / required work |
| --- | --- |
| `button.line_break` | Live mapping exists, but head/middle truncation becomes trailing ellipsis; word/character wrapping share a native mode. |
| `label.line_break` | Same partial typography mapping. |
| `text_button.line_break` | Same partial typography mapping. |
| `radio.corner_radius` | Unmapped; the broad read heuristic borrows a sibling control's field read in choices::apply. |
| `symbol.name` | A small alias table plus a help-glyph fallback; arbitrary platform symbol names are unsupported. |

Live `text_field.is_secure` adds a 54th reviewed-live entry outside that initial
audit. TimePicker presentation and open/close events now use an owned native
TimePickerFlyout. List reorder
and the other no-read entries remain separate
feature work. No unsupported behavior was classified as a platform impossibility.

Popup fixed captions override the native selection presenter, preserving native
items, selection and dropdown behavior. Clearing the caption restores the
selected text or the template placeholder. Layout notifications reconcile the
presenter after native selection and template updates.

Plain text-field clear buttons support Never/WhileEditing live changes, native
clearing, and read-only suppression. Layout notifications reapply the policy
after native template/focus changes. PasswordBox clear buttons remain unmapped.

TimePicker opening waits until its visible trigger is loaded. Native Opened/Closed
notifications update `is_open` and deliver application callbacks outside WinUI's
native event stack; removing the control cancels pending delivery. Explicit open,
explicit close, dismissal, repeated requests, live time changes, callback removal,
and window close with an open flyout have a native probe in `time_open.cplus`.
The gallery adds real mouse/keyboard selection and dismissal checks.

`tools/generate_time_template.py` reproduces the adapted TimePicker template from
the pinned SDK. The hidden native trigger preserves WinUI's locale-aware display
updates; the visible trigger opens the owned flyout. The backend forwards the
native accessible caption and handles Alt+Up/Down before the native trigger.
`TimePickerFlyout` itself remains generated and independently usable in `winui`.


Lists now share the native RefreshContainer lifecycle with Refreshable while
keeping their ScrollViewer, rows and scroll observers intact. Begin/end commands,
refresh availability, indicator color/reset, retained edits and removal during
refresh have a dedicated native probe and desktop gallery check.
Refreshable owns a separate native ScrollViewer so nested list refresh cannot
claim the same interaction source twice. The refresh probe covers independent
nested refresh state; the gallery's `refresh-host` check covers navigation and
completion with a nested list.

Grouped lists/collections use virtual header slots separate from flat item
indexes. Empty groups retain a header; collection headers span the grid and each
group starts a new line. Live group count/size/header changes, disabling grouping,
scrolling and zero-count groups have a dedicated `grouped_items.cplus` probe.
Header heights use the control's fallback row pitch; automatic header measurement
remains work.

Collection rows use native CanDrag and DragStarting/DropCompleted subscriptions;
the document receives native DragOver/Drop. Self-drops, headers, gaps and invalid
indexes do not emit moves. Data/group/layout-policy changes cancel pending moves.
Callbacks run outside native dispatch and may replace the collection. The probe
checks group restrictions, live enablement, data mutation and cancellation; the
gallery checks real mouse moves, displayed order, disabled dragging, Escape,
header exclusion and both cross-group policies. The native probe also forces
drop delivery before pending data changes are synchronized.

Hybrid-web uses generated WebView2 resource/message APIs. A synthetic HTTPS
origin serves local HTML and assets through WebResourceRequested; callbacks may
override responses before the native deferral completes. JavaScript string
messages preserve Unicode, newlines and empty values. Initialization and all
application callbacks are deferred, with state retained across callback removal.
Native probes cover live file/root changes, blank/reset, queued sends, missing
files, response overrides and removal from all four callbacks. Local origins
change on explicit reloads; storage persistence and initialization recovery
remain limitations documented in the guide.

Swipeable uses native action buttons under a separately translated content panel.
Horizontal drag slop is 8 DIPs; vertical movement yields before capture. Reveal is
leftward, limited to an 88-DIP slot per action (compressed for narrow rows).
The five callbacks are deferred and safe when any callback removes the host;
consecutive position notifications may coalesce. As in Facet's existing desktop
backends, open/close-requested describe the settled destination, rather than the
upstream MAUI programmatic-request event. Cancellation restores the prior state
and emits ended without an open/close request. Disablement and structural action
changes close the strip. Settling is immediate; inertial/animated settling,
directional action slots and physical touch validation remain outside this path.

Revealed action buttons register distinct `@swipe:<key>` agent identities with
the action node's current policy; live privacy changes apply before native content
updates, and releasing the action unpins the auxiliary button.
