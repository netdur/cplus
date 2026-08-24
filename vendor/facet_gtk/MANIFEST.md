# facet_gtk — the manifest

INTENT's rule, the same one `facet_appkit/MANIFEST.md` is held to: for each
thing facet declares, this package either implements it with what GTK offers,
or **states plainly that GTK cannot**. A verb that is neither implemented nor
listed here is a gap, and the gap is a bug.

**Status: nearly all of the write surface, and the read surface exists.**

```
360 declared prop bits     353 answered     98%   (appkit 338, uikit 335)
 68 declared handlers       68 fired       100%   (appkit  68, uikit  67)
```

Both axes are in `--check` now, with separate floors: the two surfaces fail
differently — a missing prop is a control that ignores you, a missing handler
is a control that never answers.

**WHERE THE TOOL AND THIS FILE DISAGREE, THIS FILE IS RIGHT.** `parity.py`
credits a prop when the backend names its bit OR touches its field, and a field
name is not unique across Props structs. `menu_item.is_destructive` and
`context_menu_item.is_destructive` are counted for that reason — something
touches an `is_destructive` field, for a `toolbar_item` and a `swipe_item`,
which are different kinds that really do answer it. The menu rows do not; see
their entry below. The tool is an upper bound and this is what the slack looks
like.

`python3 tools/parity.py` prints it and `--check` fails if it drops. Run it
before believing any adjective in this file.

## TWO NUMBERS, and the one above is only the first

`facet_uikit/MANIFEST.md` §"TWO NUMBERS" records why: **props are WRITES and
handlers are READS**, and a backend at 100% on props can still be a control
that is tapped and calls nothing. That is not hypothetical — it shipped.

The second number has no tool, so it is stated in words. As of 2026-08-24 the
read half covers:

| | |
|---|---|
| the gesture band | click · double-click · right-click · long press · press · release · hover · unhover · pointer move · pan · pinch · swipe — through GtkEventControllers, with the decline chain intact |
| the key band | `on_key` on any focusable node, plus the four `KeyReader` readers (code / chars / modifiers / named) |
| the control actions | button · text_button · icon_button · toggle · checkbox · radio · slider · stepper · text_field · search_field · text_area |
| drag and drop | `on_drag_start` · `on_drag_over` · `on_drag_leave` · `on_drop` · `on_drop_completed`, through a GtkDragSource and a GtkDropTarget; the payload is the source node's key, and the drag icon is a live paintable of the node being dragged |
| the sender readers | all six: `key_of` · `item_of` · `raise` · `dropped_text` · `drop_position` · `drag_targeted` |
| scroll | `observe_scrolled`, from both adjustments, with the position written back into the props before the handler runs |
| the window tier | `close-request` → `should_close` / `will_close`; the app menu's items; the lifecycle observers, each a `notify::` on the window |
| split | `on_move`, from the divider's own GtkGestureDrag, with the position written back into the props before the handler runs |
| tree | `on_select` and expand/collapse, from two nested gestures — the disclosure claims the click and the row's never sees it |
| list · collection | `on_item_tapped` / `on_item_selected` / `on_selection_changed`, from one handler over every row, told apart by the index the row carries |
| popup · tabs | `on_selected_index_changed` from `notify::selected`, and `on_tab_changed` from the strip's own buttons |
| date_picker · time_picker | `on_date_selected` / `on_time_selected`, and `on_opened` / `on_closed` from one `notify::active` |
| carousel | `on_position_changed` and `on_current_item_changed`, from the adjustment, with the page under the offset read out of the frames flex already computed |
| toolbar_item | `on_clicked`, from the GtkButton this package builds for it in the header bar |
| swipeable · swipe_item | all five of the swipeable's — `on_swipe_started` · `observe_swipe_changing` · `on_swipe_ended` · `on_open_requested` · `on_close_requested` — and the item's `on_clicked` and `on_invoked`, in that order |
| window_buttons | minimise / maximise-or-restore / close, each acting on the real GtkWindow; close goes through `close-request`, so `should_close` still gets its veto |
| web · hybrid_web | `on_load_started` / `on_load_finished` / `on_load_failed` from one `load-changed`, `on_process_crashed` from `web-process-terminated`, `on_web_resource_requested` from `resource-load-started`, and `on_raw_message_received` from the page's own `window.webkit.messageHandlers.facet` — every one of them through a symbol resolved out of the opened engine, so a machine with no WebKit gets an empty container rather than a link error |

**Every handler facet declares is now fired.** That is 68 of 68 and the gate
holds it there. It does not mean every one is EXACT: two of the web verbs are
narrowed by facet's own contract — the resource verb reports THAT and not
WHICH, and the message verb carries its payload in the sender — and both are
written down in §3. What the number means is that no declared handler is
silently dead.

## What is live

| | |
|---|---|
| the shared band | `background_color`, `corner_radius`, `opacity`, `is_enabled`, `is_visible`, `tooltip`, `input_transparent` (with `scroll`'s cascade switch) — every backed node, whatever kind |
| layout | facet's frames placed into `GtkFixed`; intrinsic size via `gtk_widget_measure`, height measured against the offered width; `insert` honours the slot |
| the text band | font family/size/weight/italic, `font_scales`, character spacing, text transform, text decoration, colour — one CSS builder, shared by label · button · text_button · radio · text_field · search_field · text_area |
| label · span | the text band, alignment both ways, max lines, wrap, ellipsize, `TextFormat::Html`, and RUNS — span children, a `formatted_text` value, and a line height — all four through Pango markup |
| button · text_button · icon_button | title, image, `content_layout`, border colour/width/radius, `bordered`, `toggles`/`on`, line break |
| text_field · search_field | text, placeholder + its colour, secure, read-only, max length, alignment, caret position, selection, `keyboard` → GtkInputPurpose, `checks_spelling`/`predicts_text` → GtkInputHints, clear button, and the search entry's two icon colours |
| text_area | a GtkTextView over its buffer: text, read-only, alignment, caret, selection, a truncating max length, purpose and hints |
| toggle · checkbox · radio | state, group membership, and the accent/track/thumb colours through the control's own CSS subnodes |
| slider · progress · stepper · spinner | value, range, increment, running, and the track/thumb/progress colours |
| image · symbol · page_dots · box | source and fit; a glyph or a themed icon; a built row of dots; colour and radius |
| scroll | a GtkScrolledWindow over a GtkFixed document: axis, both bar policies, scroll offsets, and the content extent the scrollable range is read from — a stated `content_size` wins over the measured one, per axis |
| split | position, axis, both minimums, collapse and the divider — written as STYLE on the two panes, with a draggable divider this package owns |
| the four recycler-shaped `collection` verbs | `item_sizing` (MeasureFirstItem lays every row against the sample rather than measuring each), `remaining_threshold` + its handler (fired once per crossing, from the bind), `scroll_anchor` (the offset is read and restored around the model replace; `KeepLastItemInView` scrolls to the end), and reordering — a per-row drag source and drop target that report the move through `reorder_from` / `reorder_to` and honour `can_mix_groups` |
| tree | RECYCLED too, over the same GtkListView: the visible rows are a cached pre-order walk of the expanded model, and a row is built as it scrolls in. Expansion, selection, row height, the row builder and the row's agent id |
| list · collection | RECYCLED, over a GtkListView: the model is a length, the position is the index, and a row is built (or rebound through `props.bind`) as it scrolls in. Count, the row builder, per-row heights, grouping with headers, selection, columns, separators, scroll bars and `scroll_to` — plus `invalidate_rows` and `insert_rows`/`remove_rows` as ONE `items-changed` over the range they name, so appending to a 5000-row list rebinds nothing rather than everything |
| table | the same verbs, NOT recycled — a table's rows are its facet children, so there is nothing to build lazily. Row height, `has_uneven_rows`, and the four styles |
| popup | a GtkDropDown over a GtkStringList, with BOTH factories used: the items, the selection, the text band, and the two verbs a combo box cannot express — `label` on the button and `item_enabled` per row |
| tabs | the panes shown one at a time through `Display::None`, and a tab strip of the panes' own keys that this package builds, places and colours |
| date_picker · time_picker | a GtkMenuButton over a popover — a GtkCalendar for one, a pair of spinners for the other — with the whole text band, `is_open` both ways, and the bounds enforced at the pick |
| carousel | pages side by side in a horizontal scroll host: a circular position, peek insets, swipeability as a scroll policy, and an eased slide where GTK offers only a jump |
| bordered | the four verbs a CSS border has — stroke colour, width, dash style and shape, with an ellipse as a radius of half the box |
| pull to refresh | ONE mechanism for two rows: `refreshable` whole, and `list`'s six bits — a spinner this package owns and a downward pull past the top of a scroll |
| web · hybrid_web | a WebKitGTK view opened at RUNTIME rather than linked: source, user agent, eval, the three navigation commands, both history reads, a local page with its root, and a message sent into it |
| canvas | the recording replayed against cairo: the whole state band, the transform stack, clipping, every shape, recorded paths, linear gradients, and all three text commands through Pango — a string at a baseline, a wrapped and aligned block, and per-run styled spans |
| the window | `Chrome` whole but for the two rows below — title, subtitle, size, minimum size, the button policy through a GtkHeaderBar's decoration layout, and all four `Bar` modes |
| the app menu | a GMenu tree, one GSimpleAction per item, a GtkPopoverMenuBar in the window's shell — rebuilt per screen, so a screen's own items leave with it. Shortcuts are BOUND (a GtkShortcutController per window, global scope) rather than only drawn; `is_enabled` rides the GSimpleAction and `title_of` rebuilds the bar when a title actually moves — both re-asked once per sync |
| menu · menu_item | menus declared IN the tree, merged into the same bar and ordered by priority, with an icon per item |
| context_menu · its items | a GtkPopoverMenu parented on the node the menu decorates, opened by a right-click gesture of its own, with a shown accelerator per item |
| toolbar_item | read out of the tree when a window opens and packed into a GtkHeaderBar the backend builds for their sake: text, icon, destructive marking, and `placement` + `priority` as the bar's order |
| the two things that move on their own | `progress.animate_progress` as a tween on the bar's fraction with the application's own duration and curve (eleven easings, computed rather than tabulated); `image.is_animation_playing` as a GIF's own frames through GdkPixbufAnimation, each frame a GdkTexture and the iterator deciding how long it stays up |
| symbol.fill | the outline-to-filled axis as a CSS `font-variation-settings`, which GTK carries straight through to Pango — so the bundled variable font's FILL axis is reachable without a PangoFontDescription |
| text_area, the last three | a `placeholder` as a label INSIDE the view through `gtk_text_view_add_overlay`, driven by the buffer rather than by `apply`; `style_runs` as named-and-reused GtkTextTags with every property written including the ones a run does not use; `auto_size` as a per-NODE measure callback, re-measured on each keystroke |
| swipeable · swipe_item | assembled, because GTK has no swipeable row: a strip of action buttons under the content, the content translating with the pointer, and `reveal_threshold` deciding where it lands |
| window_buttons · `.window_drag()` | the close/minimise/maximise group as three themed GtkButtons with `style` and `spacing`; a drag region that hands the move to the window manager through `gdk_toplevel_begin_move` |
| the modal verbs | `alert` / `choose` / `prompt` as facet TREES in modal windows, keyed the same as AppKit's (`alert:primary`, `choose:opt:2`, `prompt:value`); `choose_file` / `choose_directory` over GtkFileDialog |
| appearance | `is_dark` and the text-scaling factor from GtkSettings; a flip re-applies through `C_RESTYLE`, not through a registry of painted widgets |

Every other kind still gets a `GtkFixed` that honours the shared band and holds
its children. That is more than nothing and less than a claim: the tree lays out
and the boxes are in the right places, and the control's own verbs are unwritten.

## 1. Decided absent — GTK has no such thing

Eleven rows, and each was looked for before it was written down. The bar is the
one `facet_appkit` records as *a control does not have to be one native
widget*: sixty-odd rows left its ledger once the answer was allowed to be built
from two classes instead of one, and `button`, `page_dots` and `swipeable` here
are all built that way rather than declared impossible.

Four of the eleven are NARROWINGS rather than absences — a verb this backend
answers less of than the contract asks for. They are in the same list on
purpose: a verb half-answered and unrecorded is the same lie as one unanswered
and unrecorded.

- **`text_field.return_key` / `search_field.return_key`** — NARROWED, not
  absent, and this row was rewritten on 2026-08-24 when the answerable half was
  found. FIVE of the six variants name a LABEL on a virtual keyboard's return
  key (Default / Done / Go / Search / Send), and GTK has no soft keyboard:
  neither GtkInputPurpose nor GtkInputHints carries anything about the return
  key, and the on-screen keyboards that exist on a GTK desktop (Caribou,
  Squeekboard) read the purpose and nothing finer. There is no string to set
  and nothing to set it on.

  The sixth names a BEHAVIOUR. `Next` moving to the field the application put
  after it is the only thing "Next" can mean without a keyboard to draw it on
  — which is word for word the reading `facet_appkit/MANIFEST.md` records for
  the same verb — and GTK answers it directly: `child_focus` forward from the
  widget's root is what Tab does. Both fields now advance focus on Return when
  they carry `Next`, AFTER the submit handler has run, so an application that
  submits on Return sees the field it was called about rather than the next
  one. The five labels stay unanswerable; only they belong in this list.
- **`image.is_opaque` / `icon_button.is_opaque`** — a *drawing* hint that the
  image has no alpha, so the compositor may skip what is behind it. GTK's
  render tree derives that from the texture itself; there is no widget-level
  override, and inventing one out of a CSS background would change what is
  DRAWN rather than how it is composited, which is a different verb.
- **`window_frame`'s POSITION.** GTK 4 removed `gtk_window_move` and
  `gtk_window_get_position` outright, because Wayland has no global coordinate
  space to express them in and a toolkit that answered anyway would be
  answering about X11 and lying everywhere else. So `window_frame()` reports
  the size and reads x/y as zero, and `set_window_frame` sets the size and
  ignores the origin. The size half is real; the origin half is not
  implementable, not unwritten.
- **`observe_stopped`.** AppKit posts NSApplicationWillTerminateNotification;
  GTK has no application-lifecycle broadcast outside GtkApplication, and this
  backend deliberately does not adopt that (it would take over the process's
  main loop, its lifecycle and its D-Bus name for the sake of a menu). The last
  window closing is the same moment and `App::run` already reaches `on_quit`
  there, so the observer answers an INERT handle rather than a filter that
  never fires.
- **`popup.is_open`, and its `on_opened` / `on_closed` pair.** A GtkDropDown
  owns its popover and exposes neither a way to open it nor a signal when it
  opens: there is no property, no method, and the popover is a private child.
  It is reachable by walking to `get_last_child` and hoping, which is reaching
  into an implementation detail rather than answering the verb — so this is a
  row rather than a trick that breaks on a GTK point release.
- **`carousel.bounces`.** Rubber-band overscroll is a per-scroll-view property
  on AppKit (`elasticity`) and not a property at all in GTK: a GtkScrolledWindow
  draws an overshoot at the end of a kinetic scroll and offers no switch for it.
  `set_kinetic_scrolling` is the nearest call and is a different verb — it turns
  off the flick, not the bounce — so this is a row rather than a switch that
  says something else.
- **`is_destructive` ON A MENU ROW** — `menu_item` and `context_menu_item`. It
  is a red row on AppKit; a GMenu is a MODEL and carries no appearance at all,
  and GTK's own menus mark nothing either. There is no attribute for it, and
  putting the word in the label would be inventing a convention rather than
  answering the verb. Note the SPLIT: the same verb on a `toolbar_item` and on a
  `swipe_item` IS answered, because both are GtkButtons in this backend and GTK
  ships `destructive-action` as a style class. The kind decides, not the word.
- **`toolbar_item.priority` orders the bar and nothing else.** On AppKit it is
  two things — the sort AND `setVisibilityPriority:`, which decides who
  survives into the overflow menu when the bar runs out of room. A GtkHeaderBar
  does not overflow: it clips, and there is no overflow menu to be demoted into.
  So the sort half is real and the survival half has nothing to mean.
  `placement`'s three values likewise land in the header's TWO slots — `Primary`
  leads, `Default` and `Secondary` trail in that order — because two slots is
  what a header bar has.
- **`window_buttons(style: OnHover)` hovers over the GROUP.** The verb says
  "only when the pointer is over the titlebar", and under `Bar::Custom` there is
  no titlebar as far as GTK is concerned — only whatever the application drew,
  which this control has no way to name. So the reveal is the group's own hover.
  Opacity does not affect hit testing, so the invisible group still catches the
  pointer that goes looking for it, which is how the same style behaves on a Mac
  in full screen — but it is a narrowing and not the whole verb.
- **A swipeable LANDS rather than glides.** `scheduler` has a tween and the
  settle could use it; what stopped it is that the animation would then be
  writing the row's position while the next layout pass writes the same
  position, and two writers of one frame is a bug this package has already paid
  for once. The open/closed states and both handlers are exact; the 200ms
  between them is not there.
- **Two progress bars cannot animate at once.** The tween's state is one static
  pair, so a second `animate_progress` while the first is in flight takes the
  tween — which is the RIGHT answer for one bar animated twice (the application
  wants the newer target) and the wrong one for two bars. A per-widget tween is a
  timeout and a state struct per bar, which nothing has needed; written down so
  the next reader finds it here rather than in a bar that jumps.
- **`bordered.stroke_cap` / `stroke_join` / `stroke_miter_limit` /
  `stroke_dash_offset`** — CSS borders have a style (`solid`, `dashed`,
  `dotted`) and nothing else. There is no cap, no join, no miter limit and no
  dash phase in GTK's CSS border model, and a GtkFixed is not a canvas. A
  `bordered` that needed them would have to be drawn, which is the `canvas`
  kind's job and is listed as debt below rather than smuggled in here.

## 2. Not built yet — the debt

Everything not listed as live above. The large ones, in the order they matter:

- **A tree's flat index is a full walk.** Answering "what is the Nth visible
  row" means walking the expanded model, so the walk runs once per change into a
  cached vector of (node, depth) rather than once per bind. That is
  NSOutlineView's row cache and it is what the materialiser did anyway on its
  way to building widgets — what recycling removed is the BUILDING. A model that
  changes every frame pays for the walk every frame; nothing does that yet.
- **A recycling list keeps more cells than it needs.** MEASURED, on a 5000-row
  list in a 160pt viewport that holds seven rows: GtkListView creates 205 cells
  in the one `items-changed` that fills the model, and never tears them down.
  The number does NOT grow with the model — 500, 5000 and 20000 rows all give
  205, which is the property that makes this a recycler — and it does not move
  when the viewport height, the row-height estimate or the width request
  change, so it is GTK's item manager rather than anything this module does.
  What DID move it was getting out of the size-allocate (410 → 205, and every
  `gtk_widget_size_allocate` warning with it). Chasing the rest needs a GTK
  reader, not another guess.
- **An observation is per-WINDOW.** Every lifecycle observer is a `notify::` on
  the window that was up when it was registered, so it does not follow a later
  one — where AppKit's notification centre is process-wide. An app that
  observes before opening a window gets an inert handle, which the suite pins.
- **`scroll` has no kinetic tuning and no `edge_reached`.** GTK offers both;
  facet declares neither, so nothing is missing from the contract — noted only
  so the next reader does not go looking.
- **`observe_size` answers 0** (a cancellation token meaning "nothing
  registered") rather than half-wiring a size observer.
- **`after` holds ONE outstanding timer.** A second call before the first fires
  replaces it. facet's own uses are one-at-a-time; a caller needing more will
  find this written down instead of discovering it.
- **`relayout_all` re-lays every window on every sync**, at the window's
  current size. AppKit prunes on `layout_changed()`, and its `geometry.cplus`
  names two callers where that prune is WRONG — read that before copying it.
- **A gesture band is never REMOVED once armed.** Handlers are read off the
  node at fire time, so a detached set makes every one of them `no_gesture` and
  the controller fires nothing — dead weight, not wrong behaviour.
- **RTL.** `corner_radius` maps facet's leading/trailing onto CSS's
  left/right, which coincide only in a left-to-right flow. `C_FLOW_DIRECTION`
  is unanswered.

- **The canvas's SHADOW and IMAGE.** A shadow is a group, a blur and a
  composite, and cairo has no blur of its own; an image needs its source decoded
  to a cairo surface and this package has no image cache. Both are recorded
  commands that currently draw nothing.
- **A gradient BORDER.** `bordered.stroke` is a Brush and a CSS border takes a
  solid; a gradient is `border-image`, which is a different mechanism with a
  different box model. Today a gradient stroke writes no border colour at all
  rather than flattening it to its first stop, which would put a flat colour
  where the application asked for a blend and say nothing.
- **A splice is fine-grained only where a SLOT IS A ROW.** A grouped list has
  headers between its rows and a grid has `columns` rows per slot, so a data
  range is not a slot range in either — and computing one from the other is the
  map's inverse over the whole sequence, which costs more than the rebuild it
  would save. Both fall back to the replacement, which is what they had.
- **A menu accelerator is ⌘ = SUPER, and a desktop may have taken it first.**
  The mapping is the key band's and the context menus', so an application does
  not learn two answers to one question — but GNOME binds a good many Super
  combinations for itself, and a chord the window manager grabbed never reaches
  the window. That is the platform's, not this code's, and it is the reason the
  accel is SHOWN beside the item as well as bound.
- **`is_enabled` and `title_of` are answered once per SYNC, not when the menu
  opens.** The ledger says "when the menu opens" and a GtkPopoverMenuBar offers
  no such moment — there is no per-menu about-to-open signal. A sync is more
  often than a menu opens, so nothing is stale; what it costs is one call per
  item per frame, and a title that MOVED costs a bar rebuild, because a GMenu is
  a model whose labels cannot be edited in place.
- **A reorder is a DROP ON A ROW, not an insertion point.** GTK draws no
  between-rows caret and GtkListView has no reorder mode, so the gesture is
  "drag card 3 onto card 7" and the move reported is 3 → 7. That is what the
  ledger's `ReorderCompleted` says (a from and a to) and it is unambiguous; what
  it is not is the drop-between-two-rows affordance a file manager has. A
  GtkDropTarget on a one-pixel strip between cells would be the way in, and the
  cells are GTK's.

## 3. Works, but does not look like its name

- **`TextFormat::Html` is Pango markup.** It shares the inline element
  vocabulary — `<b>`, `<i>`, `<u>`, `<span>`, `<a href>` — and none of HTML's
  block layout. Inline styling renders; a `<div>` does not.
- **`mod_command` is the Super key.** facet's four modifier names are macOS's.
  X11 and Wayland call that physical key Super ("the Windows key"), so Command
  maps there and Control maps to Control. Mapping Command onto Control instead
  would make both true for one press, and an app branching on the pair would
  take the wrong arm.
- **A button is two widgets.** `gtk_button_set_label` gives a button one string
  and no room for an image, so the child is a GtkBox holding a GtkImage and a
  GtkLabel and `content_layout` is the box's orientation plus which leads.
  Likewise **page_dots is a GtkBox of styled labels** — GTK has no page
  indicator, and a row of dots is what every GTK app that wants one builds.
- **Every button kind is a GtkToggleButton**, whether or not it toggles. The
  two are different classes, so choosing between them at create would freeze
  `toggles` for the life of the control; a toggle button that is never left
  active is indistinguishable from a plain one.
- **A quit is a window closing, and there is no seam to intercept.** The macOS
  facade catches `terminate:` because it never returns to the run loop; GTK has
  no such call, so `on_should_quit` is asked through the primary window's own
  `should_close` — the same question at the same moment, through the only path
  there is.
- **A grid is EXPLICIT LINES, not `flex-wrap`.** Wrap was the obvious spelling
  and it does not hold: six 100-point items in a 300-point container came out
  two to a line, not three, and a grid whose column count is a suggestion is not
  a grid. One row node per line costs a node per `cols` items and has the
  property wrap does not — the lines align, because each is a container of its
  own.
- **`web` and `hybrid_web` are TWO PROPS BLOCKS, not one with extras.**
  `hybrid_web` declares its own — a body reading one through the other would be
  reading the wrong offsets. What they share is the widget and nothing else.
- **`on_web_resource_requested` reports THAT, not WHICH.** WebKit's
  `resource-load-started` hands over both the WebKitWebResource and its
  WebKitURIRequest, and facet's handler has nowhere to put either: the shape is
  `fn(sender, ctx)` and `HybridWebProps` declares no field for a URL. So the
  verb fires per sub-resource with the view as the sender. The narrowing is
  facet's, not GTK's — the URL is sitting in the signal, unread, and the day
  the contract grows a field it is one line away.
- **`on_raw_message_received` is a NAMED channel, and the name is `facet`.** A
  page reaches the host with
  `window.webkit.messageHandlers.facet.postMessage(x)`. WebKit's channel is
  registered per name on the view's user-content manager, so the name is part
  of the contract between an application and its page rather than an internal
  detail — generating it would leave a page author with nothing to write down.
  **The payload arrives AS THE SENDER**, which is facet's convention for this
  verb (`facet_appkit` does the same with `WKScriptMessage.body`) and again a
  contract with no field of its own to read a message out of. The string is
  transfer-full and freed after the handler returns: a handler that keeps it
  copies it.
- **The message channel needs a SECOND library.** `jsc_value_to_string` is
  JavaScriptCore's, not WebKit's, so reading a message means opening
  `libjavascriptcoregtk` beside the engine — the generation that opened decides
  which. And the two engine generations hand the signal different things:
  webkitgtk-6.0 passes a JSCValue, webkit2gtk-4.1 a WebKitJavascriptResult that
  must be unwrapped first. The unwrap is resolved OPTIONALLY and used when it is
  there, which is measured rather than assumed — 6.0 does not export it at all —
  and is what makes one body correct on both.
- **`hybrid_web.send_message` is an EVALUATED call.** WebKitGTK's own message
  channel runs page-to-host; sending the other way is dispatching a
  `MessageEvent` into the page, with the payload as a JSON literal so that a
  message containing a quote does not end the call.
- **A STYLED RUN is Pango markup, and its colours lose their alpha.** Pango's
  markup takes a colour as `#rrggbb` and has no alpha channel, so a translucent
  run draws opaque — in a label and on a canvas alike, because both go through
  the one builder in `paint`. The alternative is a PangoAttrList built run by
  run, which says the same thing in ten times the code and would carry the
  alpha: worth doing the day a translucent run matters, and not before.
- **A formatted label does not show its own `text`.** Its text IS the runs.
  Showing both would print the plain string and the styled one one after the
  other, which is what "a formatted label's text is the runs" means and is
  worth stating because the plain text is still sitting in the props.
- **The web engine is OPENED, not linked.** WebKitGTK is ~80MB of browser, and
  linking it would make every facet application on Linux depend on it and fail
  to build where it is absent — for a kind most applications never use. The
  AppKit backend does not face the choice, because WebKit is part of macOS. So
  the engine is `g_module_open`ed when the first `web` node is created and every
  call goes through a resolved symbol; an application that uses no `web` never
  touches it, and one that does gets a real view where the engine is present and
  an empty container that SAYS SO where it is not.
- **The canvas is a REPLAY, not a drawing API.** A `Drawable` records commands
  into a `vocab::Canvas` and this module walks them once, in order, against
  cairo — which is why the backend has one loop instead of forty registered
  callbacks. `facet_appkit/drawing.cplus` is the same loop against Core
  Graphics and was read as the shape, not ported: cairo keeps its path IN the
  context where CG builds one separately, and cairo is already top-left so
  there is no flip arithmetic anywhere.
- **A pull-to-refresh is ASSEMBLED, not adopted.** GTK has no such control
  anywhere: what exists is a scrolled window that reports its position and a
  spinner. The one thing that stops the gesture fighting the scroll is that it
  only listens while the scroll is ALREADY AT THE TOP — a scrolled window with
  room above it owns the drag, and taking it would break scrolling to add a
  refresh. The spinner holds the strip above the content rather than pushing it
  down and back, which is a tween on the padding and is not written.
- **`carousel.is_scrolling` is written and never read.** The backend sets it
  around a scroll, which is what makes `carousel.is_scrolling()` answer; nothing
  an application writes to it means anything, so its dirty bit is deliberately
  NOT named. Naming it to ignore it is exactly what `tools/parity.py` warns the
  number can be inflated by.
- **A carousel SLIDES on a timeout.** A GtkAdjustment has no animation — setting
  `value` jumps — so `animates_scroll` is a fifteen-step eased tween at 16ms.
  ONE outstanding tween: a second call replaces the first, which is the right
  answer for a carousel (the user swiped again) and is the same limitation
  `scheduler::after` records.
- **A `context_menu` DECORATES the node it sits under**, so right-clicking the
  PARENT is what opens it — which is exactly what `gtk_popover_set_parent`
  wants: a popover belongs to a widget without being its child. Its right-click
  gesture is a SECOND one, separate from the gesture band's, because a node may
  have both and the menu is not a handler an application declined.
- **A context menu's SHORTCUT is shown, not bound.** A GMenu carries an `accel`
  attribute that a GtkPopoverMenu displays beside the row; binding it would need
  a shortcut controller on a window this code does not have. The label is the
  half that was missing.
- **A `span` has no widget and never will.** It is a run inside its label's
  markup, read as a NODE by the label that holds it — so its eleven verbs are
  answered by `apply_label` re-reading its children, and `mount` is what makes
  that work: a viewless child changing raises `touch_all` on its host, because
  it cannot say which of the host's own verbs it affected.
- **ALL THREE canvas text commands go through ONE Pango layout.** cairo's toy
  text API takes one string at one point, knows nothing about wrapping or
  alignment, and has two font weights where facet names ten — so it was dropped
  entirely rather than kept for the simple case. `draw_text` is a layout with no
  width, `draw_text_block` one with a width and an alignment, `draw_spans` one
  filled from markup, and a font is described once for all three.
- **A recorded text point is a BASELINE.** Pango draws a layout from its
  top-left, so the baseline is subtracted on the way in — without that every
  recorded string sits one line lower than it was asked for.
- **A time picker is TWO SPIN BUTTONS.** GTK has no time widget at all, and a
  pair of spinners in a popover is what every GTK app that needs one builds —
  the same rule as the button and the tab strip, one more row that did not
  reach the cannot-ledger.
- **`format` is read for its COMPONENTS, not used as a layout.** facet's format
  is MAUI's, which is .NET's pattern language; strftime is not, and handing "D"
  to `g_date_time_format` prints "D". So the string says WHICH parts to show and
  the LOCALE supplies the order and the separators — `%x`, `%X`, or both. That
  is the AppKit backend's own reading, reached because an NSDatePicker has an
  element mask and no format string.
- **A date picker's BOUNDS are enforced at the pick.** A GtkCalendar has no
  minimum or maximum, so an out-of-range day is put straight back and the
  application never sees the refused pick — where AppKit hands NSDatePicker the
  range and lets it refuse.
- **`is_open` is answerable for the PICKERS and not for `popup`**, and the
  difference is only which widget GTK chose to expose: a GtkMenuButton's popover
  is a property, so opening and closing are one `notify::active`; a
  GtkDropDown's is a private child. Same three verbs, two controls, opposite
  answers.
- **A popup is a DROPDOWN WITH TWO FACTORIES**, not a combo box. facet's
  `label` is "what the button reads whatever is selected" and `item_enabled`
  asks whether a row may be picked at all; a GtkComboBoxText is one call and can
  express neither. `set_factory` builds the button and `set_list_factory` the
  rows, which is also the protocol the recycler will need.
- **A tab strip is this package's, not GTK's.** A GtkNotebook owns its pages'
  geometry and their labels, and facet's `tabs` states neither — its panes are
  ordinary children laid out by flex and their titles are the panes' KEYS. So
  the strip is a row of buttons this backend builds, and it takes its height out
  of the node's own top PADDING, which is how flex is told it is there.
- **A tree is MATERIALISED, not recycled**, and its rows are ordinary facet
  children — so an agent walks them, `find` reaches them, and the frame walk
  places them with no special case. The disclosure is a node inside the row with
  a gesture of its own, which works only because declining is the default here:
  the triangle claims the click and the row's handler never runs.
- **A scroll host declares `flex-shrink: 1` on its own node.** `flex_layout`'s
  default shrink is ZERO, not CSS's 1, so a node whose children are taller than
  the space keeps their height and overflows. For an ordinary box that is right
  — nothing should silently squash. For a scroll it is exactly backwards, and
  it is what the kind is FOR: the gallery catalog laid out at 1148 points inside
  a 619-point pane before this.
- **A split's position is written as flex STYLE, not applied at placement.**
  facet says a divider is a control and not layout, and AppKit agrees by giving
  it an NSSplitView that owns its panes' geometry. GTK's GtkPaned would own
  them too — and this backend places every widget itself, so a split is the
  wrong place to stop. The leading pane gets the position as a fixed extent,
  both panes get their minimum, and flex does the arithmetic. The LEADING pane
  `min_trailing` is a CLAMP on the position rather than a shrink on the pane —
  making the leading pane shrinkable was tried and quietly pulled the gallery's
  240-point sidebar down to 193 whenever the other pane held something wide.
  The clamp reads the split's own frame and is therefore a layout behind; the
  drag path clamps immediately, because it has the live widget and is how a
  person actually moves a divider.
- **The app menu is a POPOVER bar inside the window**, not a desktop menu bar.
  GTK 4 has no other kind: GNOME's global bar belongs to GtkApplication and the
  shell, and a GtkPopoverMenuBar is what every GTK 4 app that wants a menu
  builds.
- **A drop's POSITION is answered only during that drop's own handler.** One
  record, not one per widget: `drop_position(sender)` says Some for the widget
  just dropped on and None everywhere else, which is the correct answer at
  every other moment. Keeping a stale point per widget would let a handler read
  where the LAST drop landed and place a card there. `dropped_text` and
  `drag_targeted` DO persist per widget, as they do on AppKit.
- **A draggable node is also clickable.** A GtkDragSource is a
  GtkGestureSingle, so it shares the primary button with the click band; GTK
  settles it by threshold — past a few pixels the drag claims the sequence, and
  short of it the click does. That is the same arrangement AppKit reaches by
  recording the press origin and starting the session from `mouseDragged:`.
- **`hover` / `unhover` / `pointer_move` cannot decline.** They come from a
  GtkEventControllerMotion, which has no sequence to claim, so the handler's
  `bool` is read and discarded. Hover is observational on every toolkit —
  AppKit's tracking areas do not consume the event either.
- **The CSS subnode rules are unverified on screen.** `slider`, `trough`,
  `highlight`, `progress`, `placeholder` and the switch's `slider` are the CSS
  node names GTK documents, and the rules are scoped to this widget's own node
  by a class. They are pinned as STRINGS in the suite; that a GTK theme does not
  override them at higher specificity is a thing only a human with a running
  window can confirm — which is what `examples/facet_gtk_probe` is for.
- **Every unimplemented kind is a bare container.** It is in the right place at
  the right size and honours the band; it does not look like a carousel.
