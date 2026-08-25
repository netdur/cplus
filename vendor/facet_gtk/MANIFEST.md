# facet_gtk — the manifest

INTENT's rule, the same one `facet_appkit/MANIFEST.md` is held to: for each
thing facet declares, this package either implements it with what GTK offers,
or **states plainly that GTK cannot**. A verb that is neither implemented nor
listed here is a gap, and the gap is a bug.

**Status: nearly all of the write surface, and the read surface exists.**

```
360 declared prop bits     353 answered     98%   (appkit 338, uikit 335)
 68 declared handlers       68 fired       100%   (appkit  68, uikit  67)
 21 shared-band bits         18 named        85%   (appkit  20, uikit  18)
```

All THREE axes are in `--check` now, with separate floors: they fail
differently — a missing prop is a control that ignores you, a missing handler
is a control that never answers, and a missing shared-band bit is a verb every
node has and none of them honour.

**THE THIRD NUMBER IS NEW, AND IT IS WHY THE FIRST TWO WERE FLATTERING.**
`parity.py` matched `P_[A-Z0-9_]+` — per-kind bits — so facet's SHARED band was
outside the measurement entirely. A backend could ignore all of it and still
print 98%. It did: `C_ANIMATE` and `C_TRANSFORM` appeared NOWHERE in this
package, so `set_scale` did nothing and `animate_opacity` armed a channel
nobody read — while the end value still landed, because `C_OPACITY` was
handled, so an animation SNAPPED rather than not happening. Both are answered
now (`transform.cplus`); the seven still unnamed are in §2.

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

**EVERY DECLARED VERB IS NOW EITHER IMPLEMENTED OR RECORDED — on all three
axes.** The three shared-band bits the tool counts as unanswered are `C_FLUSH`
(nothing to do, and no backend names it) and `C_SAFE_AREA` (a GTK window under
a compositor has no such inset). Both are in §1. `C_HANDLERS` used to be a
third and was WRONG — see its struck-through row.

**EVERY DECLARED PROP IS NOW EITHER IMPLEMENTED OR RECORDED,** and the tool
enforces it rather than the prose claiming it: `parity.py` reads §1 for the
names it argues about, so an unanswered verb that is not in either place is
printed as UNRECORDED and is the only line on that report anyone has to act on.
The three the tool counts as unanswered are the two `is_opaque` hints and
`carousel.bounces` — all in §1, all because GTK offers nothing to write them
to. The number is 357 and the remaining three are a list, not a gap.

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
| bordered | ALL EIGHT. The four a CSS border has — stroke colour, width, dash style and shape, with an ellipse as a radius of half the box — and the four it does not: cap, join, miter limit and dash phase, drawn with cairo on a `GtkDrawingArea` over the node, which appears only for a stroke CSS cannot spell and stands the border down when it does |
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

Fourteen rows, and each was looked for before it was written down. The bar is the
one `facet_appkit` records as *a control does not have to be one native
widget*: sixty-odd rows left its ledger once the answer was allowed to be built
from two classes instead of one, and `button`, `page_dots` and `swipeable` here
are all built that way rather than declared impossible.

Four of them are NARROWINGS rather than absences — a verb this backend answers
less of than the contract asks for. They are in the same list on purpose: a
verb half-answered and unrecorded is the same lie as one unanswered and
unrecorded.

ONE IS A STRUCK-THROUGH MISTAKE, kept rather than deleted. This list is what
the next reader trusts instead of re-deriving, so a row that was wrong is worth
more standing with its correction than gone — see `C_HANDLERS` below.

AND ONE ROW LEFT BY BEING BUILT (2026-08-25). `bordered`'s cap, join, miter
limit and dash offset were here on the grounds that "a GtkFixed is not a
canvas". It is not, but it can HOLD one — the row had stopped at the first
mechanism, which is the same shape of mistake `C_HANDLERS` records. Anything
still in this list is worth re-reading with that in mind.

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

- **`image.is_opaque` / `icon_button.is_opaque` — GTK 4 HAS NO SUCH HINT.**
  The verb is a COMPOSITING promise, not an appearance one: AppKit writes it to
  `CALayer.opaque` so the compositor may skip what is behind. A GtkWidget has no
  `opaque` property at all — GSK derives opacity from the render node tree — and
  the only thing named `opaque` in the whole stack is
  `gdk_surface_set_opaque_region`, which is a WINDOW-level region and says
  nothing about one widget. Approximating it with a background colour would
  change what is drawn, which is the one thing a hint must not do.
- **`C_FLUSH` is nothing to do, and NO BACKEND NAMES IT** — not this one, not
  appkit, not uikit. It is raised when a `begin_updates` / `end_updates` batch
  closes, and by then every bit the batch raised is already on the node. The
  sync walk applies those. A backend acting on the flush as well would re-apply
  the same node twice for one edit.
- ~~**`C_HANDLERS` is free HERE**~~ — **WRONG, and it cost a real bug. Corrected
  2026-08-25; the bit is acted on now and this row is kept as the record.**

  The claim was that this package reads handlers OFF THE NODE AT FIRE TIME, so a
  live swap is picked up by the next event with nothing re-bound. That half is
  true, and it is why `button_clicked`, `fire_of` and the text delegates need
  nothing. What it missed is that AN EVENT HAS TO ARRIVE FIRST.

  `input::arm` adds the focus controller only `if core::wants_focus_events(n)`,
  and it was re-run on `C_GESTURES | C_INPUT_TRANSPARENT`. A shared-band swap
  raises neither — `C_GESTURES` exists precisely so a backend can tell "the
  gesture set was replaced" from "a focus/blur/attach/detach handler was
  swapped". So a node that mounted with no focus handler and gained one later
  was never re-asked, never got a controller, and could not receive the event
  that fire-time reading depends on. Silent for the life of the node, and it
  reads as an application bug.

  `set_item` rides the same bit, so a swapped item was unreachable the same way.

  The mask is `views::rearm_bits()` now, and the suite pins it — this backend
  builds no widgets in tests, so the mask is the only place a missing bit can be
  caught. THE LESSON IS THE ROW ITSELF: "we do it differently and therefore do
  not need the bit" is a claim about two mechanisms, and it was checked against
  one of them.
- **`C_SAFE_AREA` has nothing to answer.** A safe area is the part of a window
  the system furniture does not cover — a notch, a home indicator, a rounded
  display corner. A GTK window under a desktop compositor has no such inset:
  the window IS its content area, and the frame is outside it. uikit names the
  bit because a phone has one and appkit because a MacBook display can; there
  is no number here to report, and reporting zero would be indistinguishable
  from a backend that forgot.

## 2. Not built yet — the debt

Everything not listed as live above. The large ones, in the order they matter.

READ THESE AS CLAIMS, NOT AS FACTS. Every row here was written from reasoning
rather than from a measurement, and a pass over them on 2026-08-25 found that
of the four largest, one described work that was already done, one pointed at a
fix that would have bought three percent of the wrong half, and one was a live
bug being recorded as a design. Two rows are struck through with what was
actually measured. `FACET_GTK_SYNC=1` is how the timings were taken and is
cheaper than another round of reasoning.

The pattern in every one of them: a row that says "we do it differently here,
so the thing the other backend does is unnecessary" is a claim about two
mechanisms, and it was checked against one. §1's `C_HANDLERS` row is the same
shape and was the same mistake.


- **A tree's flat index is rebuilt whole on every structural change.** NOT a
  per-bind walk — this row used to read as if it were, and the cache it
  describes has been in `recycler.cplus` all along (`reindex_tree` into a boxed
  vector of (node, depth), which is NSOutlineView's row cache). What remains is
  narrower: the rebuild is O(visible) and runs for any structural bit, so
  expanding one branch re-walks the whole expanded model rather than splicing.
  A model that changes every frame would pay for the walk every frame; nothing
  does that yet.
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
  one — where AppKit's notification centre is process-wide.

  THE SECOND HALF OF THIS ROW WAS A BUG, fixed 2026-08-25. It used to end "an
  app that observes before opening a window gets an inert handle, which the
  suite pins" — and calling that pinned behaviour was the mistake, because
  BEFORE THE WINDOW OPENS IS WHERE AN APP NATURALLY REGISTERS, beside every
  other service it wires in setup. The same line works on AppKit. So a
  registration with no window is PENDING now rather than refused: the slot is
  claimed, the token is real and cancellable, and `arm_pending_observers`
  connects it when the first window opens — after `present`, since a resize
  observer needs a GdkSurface that does not exist before realise.

  What remains is the genuine half: an observer follows the window it was armed
  on and not a later one.
- **`scroll` has no kinetic tuning and no `edge_reached`.** GTK offers both;
  facet declares neither, so nothing is missing from the contract — noted only
  so the next reader does not go looking.
- ~~**`relayout_all` re-lays every window on every sync**~~ — **MEASURED
  2026-08-25, and the row was aimed at the wrong half. Do not implement the fix
  it suggested.**

  It said to prune on `layout_changed()` the way AppKit does. Here is what a
  sync actually costs, from `FACET_GTK_SYNC=1` over a gallery walk:

  | | typical | page switch |
  |---|---|---|
  | `mount::sync` | 2–90us | 190us |
  | `calculate_layout` | 565–1200us | 4000–9000us |
  | `reposition_children` | 15–40us | 76us |
  | `refresh_menu_state` | 2–5us | 23us |

  `layout_changed()` gates the REPOSITION WALK, which is 15–40us — three to
  five percent of the relayout, and under half a percent of a page switch.
  Pruning it buys nothing and costs the two correctness traps
  `facet_appkit/src/geometry.cplus` documents (new views at birth size; a
  two-pass `place_row`).

  The cost is the SOLVE, and flex_layout already prunes that itself: every
  sync where `layout_changed()` comes back false measured 16–24us total,
  against 565us+ when something really moved. An unchanged window — including
  every window a multi-window app is not touching — is already ~20us, so "re-lays
  every window" was never the shape of the problem either.

  What is left is the solve on a window that DID change, and that is flex's
  number rather than this package's. Nothing to do here; the row stays so the
  next reader does not re-derive it.
- **A gesture band is never REMOVED once armed.** Handlers are read off the
  node at fire time, so a detached set makes every one of them `no_gesture` and
  the controller fires nothing — dead weight, not wrong behaviour.

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
- **THE THREE BACKGROUND VERBS ARE ALL ANSWERED NOW, and two of them were
  not.** `background_color` is a Color and was applied; `background` is a
  BRUSH — a solid or a linear gradient — and `background_image` is a source
  string, and neither appeared anywhere in this backend (`core::background(`
  had zero hits, the same hole `facet_appkit/paint.cplus` records finding on
  its own side). In CSS each is one declaration, so what AppKit needs a
  CAGradientLayer and a sublayer-ordering rule for, this needs a string.
  Precedence runs most-specific-last: colour, then brush, then an explicit
  image. A gradient BORDER goes the same way, through `border-image-source`
  with `border-image-slice: 1` — with one CSS rule to know about, which is that
  a border image is not clipped by `border-radius`, so a gradient border on a
  rounded `stroke_shape` draws square.
- **A GRADIENT ANGLE IS A QUARTER TURN OFF CSS's, and the three facet
  implementations do not all agree.** facet's is the CANVAS convention — zero
  runs LEFT TO RIGHT — which `drawing::gradient_for` here and
  `facet_appkit/drawing.cplus`'s `gradient_axis` both implement. CSS's zero
  points TO THE TOP and increases clockwise, so `to right` is 90deg and the two
  differ by +90. Worth naming: `facet_appkit/paint.cplus`'s `apply_brush` — the
  VIEW background rather than the canvas — uses zero as top-to-bottom, which is
  neither. Two of the three agree and the canvas is the one facet's own
  `DrawCommand` documents, so this package follows the canvas.
- **A CLIP IS `gtk_widget_set_overflow`, NOT CSS.** GTK 4 does not clip a
  widget to its allocation by default — a child drawing past its box is drawn
  past its box — so a clip is TWO writes: the overflow flag, and the SHAPE as a
  `border-radius` in the CSS document, because GTK clips an overflow-hidden
  widget to its rounded border box. That is also the whole limit: rectangle,
  rounded rectangle, or ellipse (`border-radius: 50%`). An ARBITRARY path has
  no spelling in CSS and no widget-level door in GTK — a node that wants one
  wants a `canvas`, which clips properly because cairo does. A RECTANGULAR clip
  writes no radius at all, deliberately: `overflow: hidden` already clips to the
  box, and a `border-radius: 0` would fight a `corner_radius` the node also set.
- **A HEADING'S LEVEL IS SAID; ITS ROLE IS NOT.** GTK 4's accessibility is
  ARIA-shaped, and a ROLE is set on the widget CLASS at create time — so a node
  that becomes a heading after its widget exists cannot be told so. The LEVEL is
  an ordinary property and is written. `gtk_accessible_update_property` is
  variadic and unreachable from here; `..._property_value` is the same call with
  an explicit count and arrays, which is why it exists and what this uses.
- **A BLUR IS A WINDOW-LEVEL STATEMENT.** There is no `ungrab_focus` on a
  widget, and asking one to resign would only move focus elsewhere inside the
  window — so "nothing is focused" goes to the root, `gtk_root_set_focus(root,
  NULL)`. `facet_appkit` reaches the same shape from the other side and for the
  same reason.
- **A TRANSFORM IS A CHILD-IN-ITS-HOST, not a widget property.** GTK 4 has no
  `transform` on a widget; what it has is a transform applied to a child at
  ALLOCATION, and `gtk_fixed_set_child_transform` is the public door — which
  lands exactly right, because this package already places every child into a
  GtkFixed. The seam the layout needed is the seam the transform needed.
  GSK has no ANCHOR (a rotation is about the origin), so facet's `anchor_x` /
  `anchor_y` fractions are spelled out as the sandwich every toolkit without
  one writes: translate to the anchor, transform, translate back, with the
  node's frame turning the fractions into points. An anchor ALONE is not a
  transform — counting it would put two translations on every node in the tree.
- **AN ANIMATION IS A TIMEOUT, and ANIMATE RUNS BEFORE THE BAND.** GTK 4 has no
  property animation — `anim.cplus` says so and reaches the same answer for
  `animate_progress` — so the shared band's two channels are one GLib timeout
  over a table, not a source per node: a staggered entrance has a dozen running
  at once. The ORDER is the whole difference between a fade and a jump: an
  animation starts from what the view SHOWS, and `paint::band` answers
  `C_OPACITY` by writing the end value straight onto the widget. Measured with
  the dispatch after the band, scale interpolated 100 → 106 → 122 → ... → 200
  while opacity went 0 → 100 in one frame, because scale has no snap branch in
  the band and opacity does. `facet_appkit` records the same order in one line.
  The transform's start cannot be read back off the widget — a GskTransform
  does not decompose into facet's nine numbers unambiguously (a rotation of 180
  and a scale of -1 are one matrix) — so this module remembers what it last
  applied, per widget, and `view_release` drops it before the unref: an
  animation writing to a released widget is a use-after-free with a timer on it.
- **RTL is TWO HALVES, and facet already does the harder one.**
  `core::set_flow_direction` hands flex its own direction, and flex mirrors
  row layout, justification and edge resolution from there. What this package
  adds is the WIDGET — `gtk_widget_set_direction`, which GTK propagates to
  every descendant that has not set its own, so text alignment, an entry's base
  direction and a scale's fill all follow — and the CORNER RADIUS, because
  facet's corners are LOGICAL (leading/trailing) and CSS's are PHYSICAL (left/
  right). CSS has logical corner properties and GTK's engine does not implement
  them, so the pairs are swapped here. `MatchParent` is GTK's `NONE`, which is
  its own word for "take the parent's" rather than a third direction; where the
  radius needs a concrete answer for it, the question goes to GTK's default
  direction — the locale's — which the two agree on by construction, because
  the widget direction is set from the node's.
- **`observe_size` IS THE LAYOUT WALK, not a widget signal.** A GTK4 widget
  has no `size-allocate` signal and no width property to `notify::` on, so
  there is nothing to connect — which is why this answered 0 ("nothing
  registered") for as long as it did. But this package owns the layout: every
  frame is computed here and written onto a GtkFixed by
  `geometry::reposition_children`, so the moment a size changes is a moment
  this code is already standing in. The observer fires from there, and the
  table lives in `observers`, a leaf, because `scheduler` (which registers)
  imports `window` imports `geometry` (which fires). It is seeded with the
  node's current size at registration, so the first report is a CHANGE rather
  than an echo — which is what AppKit's KVO on `frame` does, and the two
  backends would otherwise disagree about how many times a handler was called.
  Firing is DRIVEN BY THE WALK, which is the lifetime answer too: an entry is
  read only when the walk hands over the node it belongs to, so a node torn out
  of the tree is simply never visited again.
- **A SHADOW IS BUILT, because cairo has neither one nor a blur.** Core
  Graphics has `CGContextSetShadow`; cairo has nothing. So the path is copied
  out of the context, replayed into a scratch image surface under the same
  matrix, blurred with three box passes — the standard gaussian approximation,
  and correct on premultiplied ARGB precisely because every channel is already
  colour×alpha — and composited under the shape at the recorded offset. Copying
  the PATH rather than rebuilding the shape is what makes one implementation
  serve rects, ellipses, arcs, lines and an arbitrary `Path`, and what makes a
  rotated canvas cast a rotated shadow. The pen is copied with it; the DASH is
  not, so a dashed stroke's shadow is solid. A shadow whose scratch surface
  would exceed 4096 on a side is dropped rather than drawn at the wrong size.
- **A CANVAS IMAGE IS STRETCHED INTO ITS BOX.** `draw_image(source, box)`
  carries a rect and no content mode, so "fit" and "fill" are not choices the
  recording offers and stretching is the literal reading of the one argument
  there is — an application that wants an aspect preserved passes a box with
  that aspect, which it can compute and this cannot. The source string is read
  exactly as an `image` control reads one (a path has a separator or an
  extension, a theme icon has neither), and the predicate lives in `paint` so
  the two cannot drift. Decoded pixbufs are cached by source and MISSES ARE
  CACHED TOO, because a replay runs at 60Hz and re-opening a file that is not
  there is the expensive case. A theme icon that is not file-backed draws
  nothing; the cairo source is `EXTEND_PAD`, without which a stretched image
  gets a translucent fringe where the filter kernel falls off the edge.
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
