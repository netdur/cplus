# facet_appkit — the manifest

INTENT.md's rule: for each thing facet declares, this package either implements
it with what AppKit offers, or **states plainly that AppKit cannot**. This file
is where every "cannot" is written down, so anyone can answer "how does that
happen on a Mac?" by reading it.

A row here is a commitment, not a note. Nothing is left implicit: a verb that
is neither implemented nor listed below is a gap, and the gap is a bug.

Status: **the contract is fully accounted for below the kind level.**

Read that precisely. It does not say every verb works on a Mac — it says every
verb has an ANSWER, and the answer is checkable:

```
314 declared prop/command bits    188 live, 18 host-rendered, 4 create-only,
                                   84 decided, 20 no carrier, 0 absent
 76 declared handlers              55 wired, 21 decided, 0 never fire
```

`python3 tools/verb_coverage.py --check` is the gate and it fails on a verb that
is neither implemented nor recorded, and on a ledger row naming a verb that does
not exist. That is this file's oldest claim — "a verb that is neither
implemented nor listed below is a gap, and the gap is a bug" — finally enforced
rather than asserted.

The six dispositions, and the difference between them is the point:

| | what it means |
|---|---|
| **live** | gated on the dirty bit; a later write lands |
| **host-rendered** | the node has no view; its HOST re-applies (a `span` is a run in its label's string) |
| **create-only** | read when the view is built, never after — four of them, each because a live flip needs a different OBJECT |
| **decided** | AppKit cannot; the reason is in the ledger below |
| **no carrier** | AppKit CAN; facet declares the verb and gives the backend nothing to apply it to |
| **absent** | neither. This is the debt, and it is zero. |

**Where this started.** The stage page read "42/42 kinds have a body", which was
true and was being read as much stronger than it is. The first verb-level count
said 99 live / 51 create-only / 164 absent, and that count was itself wrong
twice over: it matched module names literally, so `box::P_COLOR` was not found
in a backend that imports `facet/box` as `box_view`; and it searched the whole
concatenated backend for `(*p).<field>`, so `label`'s body reading `(*p).text`
marked `span.text`, `menu.text` and three more create-only along with it. The
correction moved ~34 verbs OUT of create-only and INTO absent — worse, not
better: `button.text_color` did not work on the first frame either.

And a whole dimension was uncounted. Handlers ride the shared `C_HANDLERS` bit
rather than a verb bit, so a verb-level count cannot see them — **52 of 76 never
fired**, including `text_field.on_text_changed` and `list.on_item_selected`.

The tier ledger reads 57 implemented, 20 decided, 5 deferred; item 7 (examples)
waits on the docs pass.

---

## AppKit cannot

The doctrine is the same one an UNSUPPORTED control verb follows: the contract
still declares it, because the contract is readable as a whole and a backend's
gaps are not the vocabulary's business.

### The ledger

The prose below is the REASON. This block is the INDEX, and it exists because
prose is not checkable: `tools/verb_coverage.py` reads it and separates
"decided" from "nobody built it yet", so the debt number means something. A
verb listed here is a commitment; a verb neither implemented nor listed is a
bug, which is exactly what this file has always claimed and could not enforce.

Every row is `module.verb` — the same names the coverage tool prints. Run
`python3 tools/verb_coverage.py --check` and it fails on a row that names
nothing real, so a stale row cannot sit here reading true; it fails on an
unrecorded verb too, which is this file's oldest claim finally enforced.

```cannot-ledger
toggle.on_color                 system-drawn: NSSwitch paints from the accent colour
toggle.off_color                system-drawn
toggle.thumb_color              system-drawn
slider.maximum_track_color      system-drawn
slider.thumb_color              system-drawn
progress.progress_color         system-drawn: NSProgressIndicator uses the accent colour
checkbox.color                  system-drawn: the mark is system-drawn
spinner.color                   system-drawn: the spinner is system-drawn
tabs.bar_background             NSTabView draws its own strip, no colour API
tabs.bar_background_color       NSTabView draws its own strip
tabs.bar_text_color             NSTabView draws its own strip
tabs.selected_tab_color         NSTabView draws its own strip
tabs.unselected_tab_color       NSTabView draws its own strip
toolbar_item.is_destructive     NSToolbarItem has no destructive style
refreshable.is_refreshable      pull-to-refresh is a touch idiom; use a Refresh command
refreshable.is_refreshing       as is_refreshable
refreshable.refresh_color       as is_refreshable
refreshable.on_refreshing       as is_refreshable
swipeable.reveal_threshold      swipe-to-reveal is a touch idiom; use a context menu
swipeable.on_open_requested     as reveal_threshold
swipeable.on_close_requested    as reveal_threshold
swipeable.on_swipe_started      as reveal_threshold
swipeable.on_swipe_ended        as reveal_threshold
swipeable.observe_swipe_changing  as reveal_threshold
swipe_item.text                 swipe items are unreachable on this backend
swipe_item.icon                 as text
swipe_item.is_destructive       as text
swipe_item.on_clicked           as text
swipe_item.on_invoked           as text
text_field.keyboard             soft-keyboard layout; a hardware keyboard has one
text_field.predicts_text        QuickType; macOS predicts in the input method
text_field.font_scales          Dynamic Type; macOS scales at the display
text_field.return_key           the LABEL on a soft keyboard's return key
text_area.keyboard              as text_field.keyboard
text_area.predicts_text         as text_field.predicts_text
text_area.font_scales           as text_field.font_scales
search_field.keyboard           as text_field.keyboard
search_field.predicts_text      as text_field.predicts_text
search_field.font_scales        as text_field.font_scales
search_field.return_key         as text_field.return_key
label.font_scales               Dynamic Type; macOS scales at the display
span.font_scales                as label.font_scales
button.font_scales              as label.font_scales
radio.font_scales               as label.font_scales
popup.font_scales               as label.font_scales
date_picker.font_scales         as label.font_scales
time_picker.font_scales         as label.font_scales
date_picker.character_spacing   NSDatePicker draws its own fields; no attributed value
time_picker.character_spacing   as date_picker.character_spacing
date_picker.is_open             a field-with-stepper has no popover to open
time_picker.is_open             as date_picker.is_open
date_picker.on_opened           as date_picker.is_open
date_picker.on_closed           as date_picker.is_open
time_picker.on_opened           as date_picker.is_open
time_picker.on_closed           as date_picker.is_open
list.is_refreshable             pull-to-refresh is a touch idiom; use a Refresh command
list.is_refreshing              as list.is_refreshable
list.can_refresh                as list.is_refreshable
list.refresh_color              as list.is_refreshable
list.begin_refresh              as list.is_refreshable
list.end_refresh                as list.is_refreshable
list.on_refreshing              as list.is_refreshable
carousel.is_swipeable           swipe between pages is a touch idiom
carousel.wraps                  an infinite carousel needs the touch paging it wraps
collection.can_reorder_items    drag-reorder needs NSCollectionView's own drag session
collection.on_reorder_completed  as collection.can_reorder_items
collection.can_mix_groups       NSCollectionView sections do not interleave
slider.thumb_image              NSSlider's knob is cell-drawn; there is no image slot
hybrid_web.on_web_resource_requested  intercepting needs a WKURLSchemeHandler per scheme
```

### The no-carrier ledger

A third disposition, and the one that is NOT this backend's to fix. AppKit can
do these. facet declares the verb and gives the backend nothing to apply it to,
so there is no code to write until the contract grows a carrier — the same
shape as the five tier rows stage 4 ended on, and the same shape as `item_of`
and the canvas vocabulary before they were closed.

Recorded here rather than left absent because "AppKit cannot" and "facet did
not say" are different facts, and a backend that files the second under the
first is claiming a platform limit that does not exist.

```no-carrier
radio.text_color                RadioButton.Content was not adopted; a radio has no text
radio.text_transform            as radio.text_color
radio.character_spacing         as radio.text_color
carousel.scroll_anchor          ItemsUpdatingScrollMode needs an update event facet has none of
collection.scroll_anchor        as carousel.scroll_anchor
carousel.item_sizing            facet has no measure hook for a grid item
collection.item_sizing          as carousel.item_sizing
collection.is_grouped           facet declares no group carrier on the item sequence
list.is_grouped                 as collection.is_grouped
table.style                     `TableStyle` has no carrier the backend can read
table.has_uneven_rows           a table's rows are its CHILDREN; flex measures them
table.row_height                as table.has_uneven_rows
collection.selection_mode       a materialised collection is a scroll of children, not a table
collection.on_selection_changed  as collection.selection_mode
collection.bind                 collection does not recycle by design; nothing to bind INTO
window_chrome.style             `window_buttons` has no style beyond the traffic lights
```

### What a scroll implies

Four verbs and three handlers were recorded as needing a carrier facet does not
have. That was wrong in the same way the rest of this pass was wrong: nothing
was missing from the contract, the backend just had no place that saw a scroll.
Once `scrolling.cplus` existed it did, and these are derived there.

- **`position` / `on_position_changed` / `on_current_item_changed`.** A
  carousel's position is an INDEX; a scroll view's truth is an OFFSET, and
  AppKit converts neither to the other. The index is the child whose frame
  contains the offset — read off flex, which already laid them out, so this
  measures nothing of its own. A position change is always an item change here,
  because the items ARE the children and the position indexes them.
- **`is_scrolling`.** There is no "stopped scrolling" event: a scroll simply
  stops arriving. The flag goes up on any scroll and is cleared by a timer each
  scroll pushes forward, so a scroll in progress cancels the pending clear
  rather than stacking one per frame.
- **`remaining_threshold` / `on_remaining_items_threshold_reached`.** Fires once
  on the way IN and re-arms only after the scroll leaves the tail. A handler
  that loads the next page must not be asked for it on every scrolled pixel.
- **`bounces`** is `scrollElasticity`, which IS per scroll view. **`peek_insets`**
  is `contentInsets` — a page narrower than its viewport by the peek is what
  shows its neighbours at the edges. **`scroll.safe_area`** is
  `automaticallyAdjustsContentInsets`, which insets for the title bar and any
  accessory, so every value but `None` lets AppKit do it.

The one that stays: **`carousel.wraps`**. An infinite carousel needs the content
duplicated at both ends and the offset teleported across the seam mid-scroll,
which is a paging model rather than a property — recorded rather than half-built.

### The derived ledger

A fifth disposition, and the smallest. These props do not flow from the
description to the screen at all, so no apply body gates them:

- `carousel.is_scrolling` is a contract READ — the backend WRITES it, the
  application reads it, and the direction is the opposite of every other verb.
- `remaining_threshold` is consulted by the scroll observer rather than applied
  to anything. It configures a comparison, not a view.

Listed so the bucket is accountable, exactly as the create-only one is: a
derived prop that nobody wrote down counts as debt.

```derived
carousel.is_scrolling           written BACK by the scroll observer
carousel.remaining_threshold    read by the observer; configures a comparison
collection.remaining_threshold  as carousel.remaining_threshold
```

### The host-rendered ledger

A second block, and a different fact. These verbs are NOT gated on a dirty bit
of their own, because their node has no view: a `span` is a run inside its
label's attributed string, a `menu_item` is a row in its menu's NSMenu. The
coverage tool calls that shape create-only, which is the honest reading of the
code and was the honest reading of the behaviour too — until `mount::sync_from`
learned to route a viewless node's change to the ancestor that draws it.

They are live. What makes them live is the HOST re-applying, so they are listed
apart from the verbs that are gated directly, and a reader can tell which
mechanism answers a given verb.

```host-rendered
span.text                       a run in its label's attributed string
span.text_color                 as span.text
span.text_transform             as span.text
span.font_size                  as span.text
span.font_weight                as span.text
span.font_family                as span.text
span.is_italic                  as span.text
span.character_spacing          as span.text
span.line_height                as span.text
span.text_decoration            as span.text
menu.text                       the NSMenu its parent builds
menu_item.text                  a row in its menu's NSMenu
menu_item.icon                  as menu_item.text
context_menu_item.text          a row in its view's NSMenu
context_menu_item.icon          as context_menu_item.text
toolbar_item.text               an item in the window's NSToolbar
toolbar_item.icon               as toolbar_item.text
context_menu_item.shortcut      the key equivalent on its NSMenuItem
menu_item.is_destructive        a red title on its NSMenuItem
context_menu_item.is_destructive  as menu_item.is_destructive
```

### The create-only ledger

The last disposition, and the smallest. These verbs are read when the view is
BUILT and a later write does not reach the screen — which is exactly what the
coverage tool means by create-only, and exactly the bucket the handoff said to
start with, because it looks implemented from the outside.

Each is here because the platform gives no other answer: a live flip would need
a different OBJECT, not a different property. They are listed so the bucket is
accountable — an unlisted create-only verb is debt, the same rule the cannot
ledger follows, and the tool counts it as such.

```create-only
text_field.is_secure            NSSecureTextField is a different CLASS, not a property
list.row_height                 the table's row height is read when its source is built
tree.row_height                 as list.row_height
window_chrome.spacing           the traffic lights are laid out once, by the window
toolbar_item.placement          an NSToolbarItem is built once, when the window opens
toolbar_item.priority           as toolbar_item.placement — and the ORDER is the bar's
menu.priority                   an NSMenu is built once, when the window opens
```

### The modifier ledger

A verb with no write of its own. It changes what ANOTHER write does, so gating
it would mean acting on a change that has nothing to act on — writing
`animates_scroll` would re-scroll the carousel to the page it is already
showing, which is a visible bug in the name of a tidy bucket.

```modifier
carousel.animates_scroll        decides whether `position` and `scroll_to` jump or slide
```

### The control tint colours

| Verb | Why not |
|---|---|
| `toggle(on_color:)` `off_color:` `thumb_color:` | NSSwitch paints from the system accent colour and exposes no per-instance tint. |
| `slider(minimum_track_color:)` `maximum_track_color:` `thumb_color:` | NSSlider's track and knob are drawn by the cell; there is no colour API. |
| `progress(progress_color:)` | NSProgressIndicator's bar is the system accent colour. |
| `checkbox(color:)` `spinner(color:)` | Same: the mark and the spinner are system-drawn. |

A tinted layer under each was considered and rejected. It would stop tracking
the system appearance (the whole point of these being platform controls), and
it would drift the moment Apple changes the control's shape. An application
that must have a specific colour draws it itself with `box` and `gesture`,
which is a visible choice rather than a lie.

### `radio(group:)` IS honoured, by name

AppKit groups NSButtons of type Radio **by superview**: radios sharing a
superview deselect each other automatically, and there is no group name.

This was recorded as "AppKit cannot" and that was wrong. facet's `group` is
the portable model, and a portable model the backend declines to honour is the
description being rendered wrong — not a platform limit. AppKit not having the
API is a reason to implement the rule, not a reason to drop it.

So the rule AppKit's own grouping implements is implemented here: turning a
radio ON turns off every other radio naming the same group, in the props and on
the control both. The search is over the mounted roots rather than the node's
siblings, because a facet group is a NAME and its members need not share a
parent — which is the whole reason facet names it instead of inferring it from
the tree. `a_radio_group_is_honoured_by_NAME_not_by_superview` puts its two
radios in different containers on purpose.

### One corner radius per layer

`corner_radius` carries four corners (`Corners { top_leading, top_trailing,
bottom_leading, bottom_trailing }`). Core Animation has one `cornerRadius`
per layer, so the **largest** of the four is used. Per-corner radii would
need a mask layer per view, which is real cost for a case no consumer has
asked for yet.

### The tab strip's colours

`tabs(bar_background:)` `bar_background_color:` `bar_text_color:`
`selected_tab_color:` `unselected_tab_color:` are not honoured. NSTabView
draws its own strip and offers no colour API — the same reason as the other
system-drawn controls above.

### `destructive:` is a red title

NSMenuItem has no destructive style, and this file used to conclude from that
that the flag should be carried and ignored — a red attributed title "would
look like a system convention that does not exist".

That was the wrong conclusion. The API not having the property is not the
platform being unable: a red title is what every macOS app that marks a
destructive action does, and a contract verb the backend silently drops is
worse than one it answers plainly. `menu_item` and `context_menu_item` both
mark it now. `toolbar_item` does not — an NSToolbarItem's label is drawn by the
toolbar and takes no attributed string — and that row stays in the ledger.

### The `bordered` stroke family IS honoured, on a path

A layer border is a solid rectangle of one width and one colour: it has no
dash, no cap, no join and no shape. This was recorded as not honoured, with
the cost — a second layer per bordered node, sized and re-pathed on every
layout pass — given as the reason. The cost is real and it is the price of the
verb, so it is paid, but only by the nodes that ask.

`wants_shape` is the gate. A node asking for a dash, a non-default cap or join,
or a `stroke_shape` gets a CAShapeLayer; one asking for none keeps the cheap
layer border and pays for no second layer. Asking and then not asking TAKES THE
LAYER AWAY — without that the shape outlives the request and keeps drawing the
old outline under the new plain border.

Two things worth knowing:

- **The path arrives with the SIZE, not at create.** The frame walk sizes a
  view after its body runs, so a path built at create is empty and a path built
  once is wrong at the next size. The view is observed and re-pathed per resize.
- **The path is INSET by half the stroke width.** A stroked path is centred on
  the line, so a path on the bounds paints half of itself outside the view and
  is clipped to a half-width border.

`stroke_shape`'s rounded case keeps the one deviation `corner_radius` makes:
Core Graphics has one corner radius per rounded rect and facet carries four, so
the largest wins. A gradient `Brush` still uses its start colour — a gradient
stroke needs a CAGradientLayer masked by this shape, which no consumer has
asked for.

### The touch-gesture controls

| Control | Why not, and what to use instead |
|---|---|
| `refreshable` | Pull-to-refresh does not exist on macOS. There is no scroll-past-the-top gesture to hang it on, and inventing one would fight every other scroll view on the machine. A desktop app puts a Refresh command on a menu or a toolbar button (⌘R by convention). |
| `swipeable` / `swipe_item` | Swipe-to-reveal-actions is a touch idiom. macOS reveals per-row actions through a **context menu** (right-click) or a hover affordance. |

Both are recorded rather than approximated because a half-working gesture is
worse than an absent one: it teaches an application a shape that will not
survive on the platform its users are actually on.

They still get a BODY, and it matters that they do: a plain backing view, so
their content renders exactly as it would have (a `swipeable` shows its
content and simply reveals nothing), and SILENTLY — a decided kind must not
take the unimplemented path, because that path warns, and a warning for
something already answered would train a reader to ignore the warnings that
mean something.

This is also where the standing rule bites — **an agent has no hands**. A
gesture-only affordance must have a click path in the UI, never a new agent
verb. On this backend that click path is the substitute above, not a
simulated swipe.

### The toolbar's colours and height

| Verb | Why not |
|---|---|
| `Toolbar.BarBackground` `BarTextColor` | NSToolbar draws its own strip and offers no colour API — the same reason as the tab strip. |
| `Toolbar.BarHeight` | NSToolbar sizes itself to its items and the window's style. |
| `Toolbar.IconColor` | NSToolbarItem images are template-tinted by the system. |
| `Toolbar.DynamicOverflowEnabled` | NSToolbar overflows into a chevron by itself; there is no switch to turn that off. |

### `format:` names components, not a layout

NSDatePicker has no format STRING — it has an element mask — and that was
recorded as the reason `format:` is not honoured. It is the reason it cannot be
honoured WHOLE, which is a different claim.

A format string says two things: which components appear, and how they are
ordered and separated. A date picker can answer the first exactly, and the
second is the LOCALE's — which is most of the point of using a date picker
rather than a text field. So the format is read for the components it names and
the element mask follows: a format naming a day shows year-month-day, one
naming only a month or year shows the pair, a time format naming seconds shows
hour-minute-second.

What is dropped is the ordering and the separators, and dropping them is the
correct behaviour rather than a gap.

### Mobile concepts, on a desktop

| Verb | Why not |
|---|---|
| `Toolbar.DrawerToggleVisible` | A drawer toggle has no desktop equivalent; a sidebar is a `split` pane. |
| `Toolbar.BackButtonEnabled` `BackButtonTitle` `BackButtonVisible` | A nav-bar back button is a phone idiom. A desktop app navigates with its own controls and ⌘[. |
| `ContentPage.HideSoftInputOnTapped` | There is no soft keyboard to hide. |
| `Page.IsBusy` | macOS has no app-wide busy indicator. A spinner is a control the application places where the waiting is. |

The `InputView` band carries four of the same shape, and they apply to all
three of `text_field`, `text_area` and `search_field` because all three embed
that band:

| Verb | Why not |
|---|---|
| `keyboard:` | `vocab::Keyboard` picks a SOFT keyboard layout — numeric, email, url. A hardware keyboard has one layout and the application does not choose it. |
| `predicts_text:` | QuickType's inline prediction bar. macOS predicts inside the input method, not per text field, and offers no per-control switch. |
| `font_scales:` | Dynamic Type. macOS scales at the display, not per font, and has no per-control opt-in. |
| `return_key:` | The LABEL on a soft keyboard's return key ("Go", "Search", "Done"). A hardware Return key has one label, engraved. |

These four were counted as unrecorded debt until now, which is why they are
written down here rather than quietly left absent — the doctrine is that a
verb is either implemented or recorded, and "nobody looked at it yet" is
neither.

These are DECIDED, not deferred, and the tier ledger says so: guard 5b now
carries three dispositions rather than two, and prints them apart. "The
platform cannot" and "nobody has built it yet" are different facts and a
tracker that conflates them is not a tracker.

## Implemented, with a deviation worth knowing

### `canvas` replays a recording; it does not call back per verb

facet's drawing vocabulary is RECORDED: a `Drawable` is handed a
`vocab::Canvas` and appends commands to it, and the backend replays the list.
So this package registers no per-verb hooks — `drawing.cplus` has one loop
over `Canvas::at(index:)`, executing into the CGContext AppKit hands
`drawRect:`.

`canvas` is the one control with a **view class of its own**
(`FacetCanvasView`, an NSView with `isFlipped` and `drawRect:`), and the one
control with **no layer** — `drawRect:` paints straight into the window's
backing store, and a layer would hold a second copy of the same pixels.

Two things about the replay are worth knowing:

**No text measurement at record time.** `ICanvas.GetStringSize` is not adopted
and could not be honoured if it were: measuring text needs a live platform
context, and while a Drawable is recording there is none. Text is placed by
giving it a box and an alignment. The one verb that still needs a width —
`draw_text(at:)`, which aligns a line ABOUT a point — measures at REPLAY time,
where a context does exist.

**`draw_text(at:)` places the top-left, not the baseline.** MAUI's
`DrawString(value, x, y, alignment)` treats `y` as the baseline. facet's box
model is top-left everywhere else, and a point that means something different
from every other point in the contract is worse than a small deviation from
MAUI. The block form has a box, so it does not arise there.

Everything else in ICanvas is honoured: the state setters, the state stack,
the transform, the clip (including `subtract_from_clip`, which CG has no call
for and which is done with an even-odd clip over the current clip bounds), the
shapes, paths with both winding rules, gradients via `set_fill_brush`, images,
and the three text verbs. `Blend` maps to `CGBlendMode` by ordinal, because
MAUI's enum was modelled on CG and matches it value for value.

**A shadow's offset is negated in y.** Core Graphics does not carry a shadow
offset through the CTM, so the flip that makes everything else top-left does
not reach it, and a positive `Shadow.offset.y` would throw the shadow UP.
MAUI does the same thing in the same place (`SetShadow`, `#if MONOMAC`).
`a_shadow_falls_DOWN_from_a_positive_y_offset` is the guard.

**An arc's `clockwise` reads backwards, and is right.** 0 degrees is 3 o'clock
and 90 is 12 o'clock, so `clockwise: true` from 0 to 90 takes the LONG way,
down through 6 and 9. The angles are measured the ordinary way while the
screen's y runs down, and MAUI resolves it identically — negated angles into
`AddArc(..., !clockwise)`. Pinned by `an_arc_sweeps_the_way_clockwise_says`
so nobody "corrects" it by eye. The transform that turns CG's circular arc
into an elliptical one is written out at `add_arc_path`.

`examples/canvas_probe` draws every verb once, including both sweeps side by
side, because an agent cannot see whether an arc went the right way.

### A text control's truth lives in AppKit, and is written back

`text_field`, `text_area` and `search_field` are the three kinds where the
USER changes the value, not the application. Everything else in facet flows one
way — the app writes a prop, the bit goes up, the backend paints it — and these
three have to flow both.

They did not. Nothing wired any of them back, with two consequences:

- `on_text_changed` and `on_submit` never fired, on any of the three.
- `field.text()` reads the props (`text_field.cplus`), and the props still said
  whatever the application last SET. So an app that read a field after the user
  typed in it got the old string, silently, with nothing logged and no error.

The second is the one that mattered: an invisible wrong answer from the
contract's own reader. `text_input.cplus` closes it. The native control is
where editing happens, so every edit is written back into the props BEFORE the
handler runs — a handler that asks `text()` gets what is on screen.

Three deviations worth knowing:

- **The write-back does not raise the dirty bit.** `core::touch` would schedule
  a write of the string back into the control the user is typing in, and
  writing `stringValue` moves the insertion point. The props are being made to
  agree with the screen, not the other way round.
- **`on_submit` is a different moment per kind.** A field and a search field
  submit on Return, which is the control's own action. An editor submits on
  losing focus, because Return inserts a newline there and MAUI's
  `Editor.Completed` has no other moment to mean. Wiring both on a field would
  fire submit twice.
- **`cursor_position` / `selection_length` are answered by the field editor**,
  which exists only while the control has focus. An unfocused field keeps what
  the props last said rather than resetting to zero — there is no selection to
  report, and reporting zero would be a claim rather than an absence.

The tests type through the real path — `keyDown:` on the field editor, which is
what AppKit itself calls, running interpretKeyEvents → insertText → didChangeText
→ the notification. Posting the notification by hand would prove the imp is
reachable and nothing about whether AppKit ever reaches it. All six fail if
`text_input::install` is ablated.

### A span is not a view either

`span` is a styled RUN inside a label, so it has nothing to apply onto. A
label with span children renders one attributed string built from them and
IGNORES its own `text` — a formatted label's text is its runs, and its own
`text` would be a second, contradictory answer to the same question. A label
with no spans renders `text` exactly as before.

### The menu kinds are not views

`context_menu` and `context_menu_item` answer `wants_view` FALSE: an NSMenu
is not an NSView, and a context menu decorates the node it sits under rather
than occupying a place of its own. The mount walk creates nothing for them
and they take no place in the native tree — the menu is read from the NODES
when the PARENT's view is built, and hung on that view with `setMenu:`.

Consequence: a `context_menu` node has no frame, no `native()`, and no
geometry. It is a description that resolves into its parent, which is the
only reading that matches what a context menu IS.

### Every backing view is flipped

facet's tree is top-left origin, because flex is. AppKit's NSView is not, by
default. Rather than carry a flip through the frame walk — the pre-regen
backend threaded a `parent_flipped` flag through every level and computed
`parent.height - (child.top - parent.top) - child.height` for the bottom-up
case — every view this backend creates has `isFlipped` true, and the window's
content view is flipped too.

Consequence for an application that drops beneath facet: a native view you
add yourself lands in a **flipped** superview, so its frame is read top-left.
That is the opposite of a hand-built AppKit window and it is deliberate.

### `blur` resigns first responder to the window

There is no null first responder on AppKit: passing nil makes the window
refuse the change rather than clear it. `blur` therefore makes the WINDOW the
first responder, which is what "nothing in the tree is focused" means on this
platform.

### The sync tick is a CFRunLoopObserver at before-waiting

facet has no loop of its own; M5 says the backend coalesces sync requests on
its run loop. This uses `kCFRunLoopBeforeWaiting` — the moment the loop has
finished everything it had and is about to sleep, which is the same flush
point Core Animation commits on. A hundred writes in one event cost one tick.

### A background needs a layer

`background_color` on a plain view has no AppKit equivalent without a backing
layer, so setting one turns `wantsLayer` on for that view. A node that sets no
background stays layer-free, so a tree that themes nothing pays for no layers.

### Intrinsic size comes from the platform

flex sizes a LEAF from a measure callback, and `views::create` installs one:
the view's own `fittingSize`. That is what makes a button as wide as its title
and a label as tall as its text without facet knowing anything about type
setting. A bounded width is handed over first through
`preferredMaxLayoutWidth`, or a wrapping label measures as one long line and
gets clipped.

It goes on the kinds whose size is genuinely intrinsic —
`views::has_intrinsic_size` names them one by one. A scroll view, a web view
and a canvas have a fitting size too and it means something else: a surface is
sized by its container, and asking WKWebView how big it would like to be is
not a question with a useful answer.

Until 2026-08-04 nothing installed a measure at all, so every control without
an explicit width and height laid out 0x0 — present, correct and invisible.
It survived because the tests asserted PROPS and the examples sized everything
by hand; the guard now asserts a FRAME.

### The titlebar slots are read from the tree

`TitleBar.LeadingContent` / `TrailingContent` are subtrees under a reserved `@`
key — `window_chrome::titlebar_leading` / `_trailing` — and the window lifts
them into an `NSTitlebarAccessoryViewController`. Carried as a key rather than
a field on `Chrome` because Chrome is a VALUE struct, copied into every window
that opens, and a subtree is not a value.

Lifted OUT of the content: the slot's node gets `display: none`, which is
flex's own word for "not in this layout" and needs no backend concept. A node
laid out in two places is laid out wrong in one.

Portability, since it decides the shape: **GTK supports this best** —
`GtkHeaderBar` is exactly leading and trailing widget slots — AppKit has the
accessory controller, and Windows has neither: an app there extends the client
area and draws its own bar. So this is the GTK-shaped extra, and `Bar::Custom`
plus `window_buttons()` and `.window_drag()` remains the portable answer.

### `alert` is a facet tree in a sheet, and non-blocking

It used to be an NSAlert and `runModal`, which was two problems at once. It
BLOCKED — the caller did not return until the user answered — and nothing in an
NSAlert has a key, so AN AGENT COULD NOT REACH IT. A dialog an agent cannot
answer is a dialog an agent cannot get past.

So the sheet is an ordinary facet tree in an ordinary window, attached with
`beginSheet:`, and every part of it is addressable the way everything else is:
`alert:title`, `alert:message`, `alert:primary`, `alert:secondary`. The agent
clicks the button; there is no second path and no simulated one.

Non-blocking is the load-bearing half: a blocking call cannot be driven over
MCP, because the agent's own request is what would have to return first.

WINDOW-modal, not application-modal — it blocks its parent, which is what an
alert means to a person, while the rest of the app and the agent channel keep
running.

`alert_blocking` is kept for the one case a sheet cannot serve: a decision that
must be made before the process can go on, with no window to attach to. An
agent cannot reach it, which is exactly why it is not the default.

**The file pickers stay native, and that is a hole.** `choose_file` and
`choose_directory` are NSOpenPanel, which an agent cannot drive and which
cannot be reimplemented — the panel IS the sandbox door, and a facet-drawn
imitation would grant nothing. An application that needs an agent to choose a
file has to offer a path some other way.

### The window tier: what a native window is asked

Sixteen ledger rows landed together, and they share a shape — none of them can
be answered without a real window, so none could exist before this stage.

**`close_button_only` became `maximizable` + `minimizable`.** The old field
said exactly `maximizable: false, minimizable: false` in one word and could
say nothing else; the ledger asks for the two rows apart because a window may
well want one and not the other. The buttons are HIDDEN rather than disabled:
AppKit's own answer is to drop the style-mask bit, but the mask is what chose
the bar's style, and re-deriving it in one expression would put two decisions
in one place.

**Zero is unbounded for a maximum size.** An unset `max_width` must not become
a maximum of nothing. The minimum pair already reads that way, so the maximum
has to agree or the two fields mean different things by the same value.

**`is_window_active()` is `isKeyWindow`, not main.** A window can be main
while a panel holds key, and "is the user typing into this" is the question an
application means.

**The appearance override is undoable.** `Appearance::Unspecified` is not a
third colour scheme — it is "give the system back", and without it an override
could never be taken off. Setting one takes the same repaint path a system
light/dark flip already goes through, because colours resolved at paint time
do not re-resolve themselves.

**The app menu was built and never installed.** `App::build_menu()` produced
an AppMenu and nothing read it. facet's AppMenu is FLAT — each item names its
menu — and AppKit wants a tree, so the grouping happens in the backend, in
FIRST-MENTION order, which is the only order a flat list can have expressed.

A platform verb (Copy, Quit, Close) gets the platform's selector and **no
target**, so the responder chain decides. That is what greys Copy out with
nothing focused, without facet knowing anything about focus. An app command
carries a real target and wins over a token action, because a row with both
meant the callback.

**The toolbar is read from the tree.** A `toolbar_item` answers `wants_view`
false, so the items are collected from the NODES when the window opens — from
anywhere in the tree, because a screen describes its toolbar where the content
that owns it lives, which is rarely the top. A tree with no items asks for no
NSToolbar at all rather than an empty one.

### `web` and `hybrid_web` are one WKWebView with two wirings

`vendor/webkit` landed 2026-08-04 and both kinds are implemented. They share a
file and a delegate class, because WKNavigationDelegate and
WKScriptMessageHandler are both "the page told us something" and splitting them
would pin two objects to one view.

**The two reads are WRITTEN BACK.** `can_go_back()` and `can_go_forward()` are
contract reads whose truth lives in WKWebView's back-forward list, so the
delegate writes them into the props on every navigation. The cursor then
answers from the props like every other read, and an application never learns
that one answer came from the platform and another from the description.

**A `source` is read three ways**, because MAUI's `Source` is a WebViewSource
(URL or HTML) and facet reduced it to a `str`, so the string has to say which:
starts with `<` is markup, contains `://` is an absolute URL, anything else is
a file path — loaded with read access to its FOLDER, or its stylesheet is
refused and the page looks broken rather than blocked.

**`on_navigating` is `didStartProvisionalNavigation:`, not a policy decision.**
MAUI's `Navigating` is cancellable; facet's handler returns nothing, so it
could not cancel even if this were wired to
`decidePolicyForNavigationAction:`. Reporting the START of a navigation is the
honest thing the signature supports.

**The hybrid channel is named here**, because facet declares a raw message in
each direction and nothing about how it is carried:

| direction | how |
|---|---|
| page → facet | `window.webkit.messageHandlers.facet.postMessage(body)` |
| facet → page | `window.facet.onmessage(body)`, if the page defined it |

One word, `facet`, both ways. The outgoing body is escaped before it is
spliced into script — a quote or a newline in a message would otherwise end
the literal and run whatever followed.

**`on_web_resource_requested` is not honoured.** It needs a
`WKURLSchemeHandler` and a custom scheme, which is a second loading path
alongside `loadFileURL:`; nothing has asked for it yet, and a resource hook
that fires for some requests and not others would be worse than none.

### The gesture band moves the view's class

A gesture handler answers `bool`, and `false` means the event must carry on
exactly as if facet had never looked at it. Nothing on AppKit continues an
event except `[super mouseDown:]`, so the handlers have to BE the view's event
methods — and gestures are a modifier on ANY node, whose backing view is
whatever its kind needs.

So the class is made at runtime, per backing class, and the view's isa is moved
onto it. This is what KVO does, for the same reason. An NSButton with gestures
is still an NSButton; it gains overrides that call super when the app declines.
One subclass per backing class, cached on the class itself.

Consequence for an application that drops beneath facet: `object_getClass` on
a gesture-bearing view answers `FacetInput<n>`, not the class you expect.
`isKindOfClass:` is unaffected, which is the check that matters.

The per-view gate is never a method, and that is load-bearing: the class is
SHARED by every view of its kind, so adding `performDragOperation:` for one
drop zone would answer for every node of that kind. Drop selectors live on the
class from the start and `registerForDraggedTypes:` — AppKit's own gate — is
what a view opts in with.

### `on_pinch` is `magnifyWithEvent:`, not the window zoom

The gesture reports a trackpad magnify on the node. Window-level
pinch-to-zoom is a different feature with a different rule (scale only, never
reflow) and is not what this verb does.

### Scroll is not a gesture

facet routes the wheel through the `scroll` control, where the nested-axis rule
lives (`FacetScrollView` locks an axis per gesture). A `scrollWheel:` override
in the gesture band would fight it.

## Not yet reached

These are unimplemented because their stage item has not landed, NOT because
AppKit cannot. They are listed so the difference is never ambiguous.

| What | Stage 4 item | Size |
|---|---|---|
| 5 tier rows with no facet verb to carry them | 5 | decisions, not tasks |
| Examples revived on the new API | 7 | medium |

Everything else in items 1-5 has landed: all 42 kinds have bodies, the window
tier answers, the app menu and toolbar install, `web` and `hybrid_web` run on
`vendor/webkit`, and nothing takes the unimplemented-kind warning path.

### `list` and `tree` recycle; `collection` does not, and that is deliberate

`list` is an NSTableView with a data source. Rows are AppKit's: it asks for one
when it needs it, so a list of three and a list of ten thousand cost the same
at create. Each row's subtree is realised through `mount::realise` — the second
mount path, named in the seam rather than re-implemented here — into the cell
that owns it. The CELL owns the node, because in facet a node owns its views:
letting the node drop after building would empty the cell the moment it was
filled.

`tree` is an NSOutlineView, and it is a BETTER fit than the list: the data
source is item-based and facet's tree is already a model of items, so the item
IS the `*TreeNode`. Nothing maps an index onto a node and nothing can drift
when the model changes shape. Indentation is AppKit's here — the materialising
version made it a leading pad on the row because it had no other way, and
drawing a second gutter inside an outline view's own indent would double it.

facet's `expanded` set is the MODEL; the outline view's per-item state follows
it on reload. The walk stops at a closed branch, because an outline view has no
rows below one to expand.

`collection` still materialises every item as an ordinary child. That is not
an oversight: a collection is a grid whose items flex against each other, and
an NSTableView row is not that. It gets the recycling treatment when a
consumer has a collection long enough to need it.

**Two things make it fast, and one still does not.**

`row_height` skips measurement ENTIRELY — no build, no layout, no cache. When
an application knows its rows are uniform that is the cheapest a list can be.

The height cache exists because NSTableView asks `heightOfRow:` for EVERY row
whenever it recomputes geometry, not only the visible ones, and answering means
building the row and laying it out. Without it a scroll re-measures the whole
list on every pass, felt as a periodic hitch. The cache is keyed to the width
it was measured at, because a different width re-wraps every row.

**The bind step is what makes reuse real.** Without one, a recycled cell was
stripped and its subtree REBUILT — the cell was reused and the work was not, so
scrolling paid a full build per row. `set_row_bind` splits the two questions:
`row` describes the SHAPE once per cell, `bind` writes one row's data into it.
Scrolling then costs a handful of property writes.

It is OPTIONAL and its absence is not an error — a list that names no bind
rebuilds each row, which is what every backend did before the contract could
say otherwise. Setting one is a promise that every row has the same shape,
which recycling assumes anyway, said out loud.

Two things about it are worth knowing. A row is bound on its FIRST use too, so
the builder is responsible for shape only and a row can never show the data of
whichever row happened to build its cell. And MEASURING binds as well — heights
are asked for every row, so without a bind, measuring a thousand rows built a
thousand subtrees; with one it builds one.

A bind writes PROPS, which marks nodes dirty and touches nothing native, and a
realised row is outside the window walk that carries dirty bits across. So the
backend applies it immediately through `mount::sync_from` — the third verb this
path needed, after `realise` and `unrealise`.

**A resize coalesces into one reload.** A live drag changes the width on every
frame, and every width change invalidates every measurement, so reloading on
each one re-measures the whole list dozens of times a second. A reload is
scheduled instead and the next frame cancels the one before it: the list
re-measures once, when the drag settles.

Whether the viewport was at the bottom is read BEFORE anything moves, and
re-pinned after. A narrower width re-wraps rows taller, so the content grows
under the viewport and "at the bottom" has stopped being true by the time the
reload has run — which is why the question cannot be asked afterwards. It is
the chat behaviour, and the one that is invisible until it is missing.

The width a cell gets is the COLUMN's, not the table's frame, and that is a bug
rather than a detail: modern NSTableView styles inset cells horizontally, so
measuring wrapped rows at frame width under-measures them and clips the last
lines at certain widths. It presents as "scrolling down does not show the rest
of the message", which is nothing like the mistake that causes it.

### The collection group builds every row, not just the visible ones

`list` and `collection` MATERIALISE their rows as ordinary children of
themselves: `row(ctx, i)` is called for each i and the result is added with
the same structural verb anything else uses. From there facet's own machinery
does everything — mount creates the views, insert puts them in the scroll's
document view, the frame walk places them, teardown releases them. There is
no second lifetime model and no path a row can take that a child cannot.

The cost is real and is the one thing to know: a list builds every row up
front rather than the visible ones. AppKit's answer is NSTableView with a
data source, which recycles — and the pre-regen backend's recycling
NSTableView (`git show eb5b1b7:vendor/facet_appkit/src/ui.cplus`) is the
upgrade when a consumer actually has ten thousand rows. It was not worth
inventing a second, parallel mount path before then.

`tree` walks only through OPEN branches, which is what keeps a deep tree
cheap under that rule: a closed branch is not descended at all.

The warning path still exists and is still the rule — a control kind with no
body renders as an empty backing view **and says so on stderr**, once per
kind. Nothing takes it today, and `every_kind_now_has_an_answer` is what keeps
that true.
