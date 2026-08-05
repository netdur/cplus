# facet ← iris gap audit — 2026-08-01

> **Resolved 2026-08-03.** Both Stage 2 defects (§4) are fixed and guarded, the
> 11 facet-origin words (§3) are recorded in the ledger with two of them
> emitted, and the rename table (§1) is superseded by the fuller one at the
> end of this file — the naming pass renamed 48 more verbs, so iris's migration
> table is longer than it was when this was written. See `stages/2.md`.


What a real application needs from facet, measured against what Stage 2
generated. iris is the consumer: 16,948 lines over 38 files, **84 distinct
`facet::` symbols**, 44 build modifiers, 25 live verbs.

The question this answers: MAUI seeded the vocabulary, but MAUI is not an app.
What does an app reach for that the seed never had?

---

## 1. Already covered

**The 38 generated widgets.** `label` `button` `text_field` `text_area`
`search_field` `popup` `slider` `box` `bordered` `scroll` `icon_button`
`list` `menu` `image` and the rest. iris's widget vocabulary is a subset of
what Stage 2 emitted.

**Layout, all of it, from flex_layout.** facet's `Node` *is* flex's, so
iris's layout modifiers already work — some under different names:

| iris | flex |
|---|---|
| `align_items` | `align` |
| `justify_content` | `justify` |
| `margin_edge` | `set_margin` |
| `padding_edge` | `set_padding` |
| `position_absolute` | `position_type` |
| `inset` | `set_position` |

`width` `height` `grow` `shrink` `padding` `gap` `frame` `z_index` are
identical. Containers too — `Builder` `column` `row` `hstack` `vstack` `box`
`screen` `card` are flex's, and facet re-exports rather than reimplements
(`8638a0c`).

**`spacer` and `zstack`** were the only container gaps; both were pure layout
and landed in flex_layout (`a91d43e`), not facet.

**Most "missing" live verbs are renames**, because the ledger renamed MAUI's
names and iris uses the old facet's:

| iris | facet now |
|---|---|
| `set_background` | `set_background_color` |
| `set_foreground_color` | `set_text_color` |
| `set_hidden` | `set_is_visible` (inverted) |
| `set_text_spans` | `set_formatted_text` |
| `scroll_offset_x` / `set_scroll_offset` | `scroll_x` / `scroll_y` |
| `show` | `set_is_visible(true)` |
| `size` | `width()` / `height()` |
| `found` | obsolete — `find` returns `Option`, not a null object |

---

## 2. Stage 3 owes these — the tiers (23 symbols)

None of this is widget vocabulary, so MAUI was never going to supply it.

`Component` `Lifecycle` `Screen` `ScreenBox` `screen_box` `Service`
`load_service` `Chrome` `AppMenu` `app_menu` `menu_action_*` (11 of them)
`run_on_main` `on_worker` `after` `Cancellable` `component_at` `raise`
`key_of` `set_theme` `Theme` `is_dark` `observe_size`

---

## 3. facet's own words — no MAUI provenance (11)

INTENT anticipates this: "it grows by what its applications need, and adds
words of its own." These are the words. Each needs declaring deliberately and
recording, not appearing by accident.

| word | what it is | why MAUI has none |
|---|---|---|
| `symbol` | an SF Symbols glyph | Apple-specific |
| `wrap_label` | a label that wraps | MAUI folds it into Label |
| `composer` | a multi-line input with chrome | compound widget |
| `tree` / `TreeNode` | a hierarchical list | MAUI has no tree control |
| `split` | a draggable divider | behaviour, not layout |
| `clickable` | a box that takes a click | needs a handler |
| `window_buttons` / `window_drag` | custom titlebar chrome | platform chrome |
| `TextSpan` | a styled run | `Spans` exists; the builder does not |
| `set_count` | list row count | MAUI binds `ItemsSource`; the ledger dropped it as MVVM, so the imperative replacement is facet's to invent |
| `set_split_position` | divider position | `split` is facet's |
| `tree_select_path` / `tree_deselect` | tree selection | `tree` is facet's |

---

## 4. Two defects in Stage 2, found by this audit

### 4a. 31 ADOPT rows never emitted

The generator skips any row whose facet type is in `SKIP_TYPES`, and says
nothing. Stage 1 exits non-zero on an unbucketed row and on an unclassified
type; Stage 2 has no equivalent guard, so the ledger and the emitted contract
can disagree in silence — the exact failure the discipline was built against.

```
ADOPT rows reaching a control: 376
  emitted : 345
  SKIPPED :  31
```

| n | facet type | examples |
|---|---|---|
| 13 | `Node` | `ItemsView.EmptyView`, `StructuredItemsView.Header`/`Footer` |
| 6 | method | `ProgressBar.ProgressTo`, `ItemsView.ScrollTo`, `ListView.BeginRefresh` |
| 4 | `SwipeItem[]` | `SwipeView.LeftItems`/`RightItems`/`TopItems`/`BottomItems` |
| 3 | `Node[]` | `ScrollView.Children`, `CarouselView.VisibleViews` |
| 2 | `f64[]` | `Border.StrokeDashArray` |
| 1 | `str[]` | `Picker.Items` — iris needs this to fill a popup |
| 1 | `Drawable` | `GraphicsView.Drawable` |
| 1 | `KeyboardAccelerator[]` | `MenuFlyoutItem.KeyboardAccelerators` |

### 4b. The shared band is partially forwarded

`CommonProps` carries 19 fields and the ledger's shared band is 41 verbs, but
`SHARED_FORWARDS` emits 15. Absent from every cursor: `native()` — which the
ledger explicitly ADOPTed as the escape hatch INTENT blesses, and which iris
uses as `view()` — plus `bounds()` `is_focused()` `is_attached()` `focus()`
`blur()` `relayout()` `measure()` `children()` `begin_updates()`
`end_updates()` and the transform band beyond `rotation`/`scale`.

---

## 5. The one migration that is a decision, not a rename

**`facet::Handle` is gone by design** — 125 uses in iris.

Old: one `Handle` for every widget, so `set_value` on a button compiled and
silently did nothing. New: `find` is per-control and returns
`Option[<Control>]`, so a wrong verb is E0324 and a wrong kind is `None`.

Every `facet::find(cp, k).set_text(...)` becomes a per-control `find` plus a
match. That is iris's largest single change and it follows from a Stage 2
decision, so it is worth re-confirming before Stage 3 builds on top of it.

---

## What the 11 became

| word | outcome |
|---|---|
| `symbol` | **built** — `src/symbol.cplus`, over a 927 KB Material Symbols Outlined and 4,268 generated constants, so an unknown icon is a compile error. Three tiers: bundled (checked), `system_symbol` (the OS's set, deliberately platform-specific), an app's own font |
| `tree` / `TreeNode` | **built** — generic: `id` and `is_branch`, not `path` and `is_dir`. Owns its model as `Vec[Box[TreeNode]]`; the predecessor leaked every node |
| `split` / `set_split_position` | **built** — `SplitAxis::Columns\|Rows`, panes Leading/Trailing, position clamped where it is stored |
| `window_buttons` | **built** — a control, because it draws something |
| `window_drag` | **a modifier** — draws nothing, so it says what pointer input means and lives in the gesture set |
| `clickable` | **deleted** — `.gesture(on_click:)` on any node is what it was |
| `TextSpan` | **built** — `Spans` had been `struct Spans { count: i64 }`, so `set_formatted_text` was a verb nothing could feed |
| `set_count` | **emitted** on `list` and `collection`, with a row builder |
| `tree_select_path` / `tree_deselect` | **built** as `select(id:)` / `deselect()`, plus `restore(expanded:, selected:)` — one verb, because expansion must precede selection |
| `wrap_label` | **not a control** — `label(…, line_break: WordWrap)`. It was a separate kind only because AppKit has a second NSTextField factory |
| `composer` | **not a control** — text_area + bordered + a background is a component |

Two rules fell out, and a twelfth word should follow them: **a behaviour that
draws nothing is a modifier, not a control**, and **an arrangement of existing
words is a component, not a word**.

## Disposition

| finding | owner | status |
|---|---|---|
| containers, layout modifiers | flex_layout | DONE — re-exported, renames documented |
| `spacer` / `zstack` | flex_layout | DONE (`a91d43e`) |
| live-verb renames | none | DONE — full table below; migration only |
| the 23 tier symbols | Stage 3 | to build |
| the 11 facet-original words | **DONE 2026-08-03** | four modules (`symbol` `tree` `split` `window_chrome`), two modifiers (`.gesture()` `.window_drag()`), `Spans` made real, `set_count`/`set_row` emitted — and three that turned out not to be words at all |
| 31 skipped ADOPT rows | **Stage 2** | DONE — 24 emitted, 4 moved to DROP with a reason, 3 carried by the shared band; a guard now fails the run on a row nothing carries |
| shared band 15/41 forwarded | **Stage 2** | DONE — 36 forwarded, 5 deferred with reasons, guarded |
| `Handle` → typed cursors | decision | STANDS — see below |

---

## What the 31 became

| n | facet type | disposition |
|---|---|---|
| 12 | `Node` | **emitted as slots** — a named child under `Data.slot`, replaced in place. `set_header(take n)` / `header() -> Option[*Node]` |
| 4 | `SwipeItem[]` | **emitted as slots** — the same shape one level up |
| 6 | method | **emitted as commands** — a write plus a dirty bit, like any setter |
| 1 | `str[]` | **emitted** as `vocab::TextList`: `set_items(take)`, `item_count()`, `item(at:)`. This is what iris needs to fill a popup |
| 1 | `f64[]` | **emitted** as `vocab::Dashes` — 8 fixed runs, so it stays Copy |
| 1 | `KeyboardAccelerator[]` | **emitted** as `vocab::Shortcut` — one key equivalent, which is what every platform with menus shows |
| 1 | `Drawable` | **emitted** — the type already existed; it was in `SKIP_TYPES` by mistake |
| 3 | `Node[]` / `*u8` | carried by the shared band: `child_count()` + `child(at:)`, and `native()` |
| 1 | `CarouselView.CurrentItem` | **DROP** MODEL — the current item is a position, and `set_position` carries it. Same rule Stage 1 already applied to `Picker.SelectedItem` |
| 1 | `CarouselView.VisibleViews` | **DROP** ENGINE — MAUI's realized-view bookkeeping |
| 1 | `ScrollView.LayoutAreaOverride` | **DROP** LAYOUT — overriding a layout area is layout, whoever asks |
| 1 | `Border.StrokeDashPattern` | **DROP** ENGINE — the handler-facing mirror of `StrokeDashArray` |

## The migration table iris needs

The rename table in §1 was written before the naming pass. This is the whole
of it. Nothing here is a capability change; every row is the same verb under
the name `naming_guideline.md` asks for.

The left column is what the name was: iris's word where §1 recorded one, and
otherwise what Stage 2 emitted before the naming pass.

| before | facet now |
|---|---|
| `set_background` | `set_background_color` |
| `set_foreground_color` | `set_text_color` |
| `set_hidden` | `set_visible` (inverted) |
| `show` | `set_visible(true)` |
| `set_text_spans` | `set_formatted_text` |
| `scroll_offset_x` / `set_scroll_offset` | `scroll_x` / `scroll_y` |
| `size` | `width()` / `height()` |
| `view()` | `native()` |
| `found` | obsolete — `find` returns `Option`, not a null object |
| `set_is_visible` / `set_is_enabled` | `set_visible` / `set_enabled` |
| `set_line_break_mode` | `set_line_break` |
| `set_return_type` | `set_return_key` |
| `set_text_type` | `set_text_format` |
| `set_font_attributes` | `set_font_style` |
| `set_horizontal_text_alignment` | `set_text_align` |
| `set_vertical_text_alignment` | `set_vertical_align` |
| `set_horizontal_scroll_bar_visibility` | `set_horizontal_scroll_bars` |
| `set_vertical_scroll_bar_visibility` | `set_vertical_scroll_bars` |
| `set_clear_button_visibility` | `set_clear_button` |
| `set_separator_visibility` | `set_separator` |
| `set_items_updating_scroll_mode` | `set_scroll_anchor` |
| `set_item_sizing_strategy` | `set_item_sizing` |
| `set_image_source` / `set_icon_image_source` | `set_image` / `set_icon` |
| `set_thumb_image_source` | `set_thumb_image` |
| `set_font_auto_scaling_enabled` | `set_font_scales` |
| `set_is_spell_check_enabled` | `set_checks_spelling` |
| `set_is_text_prediction_enabled` | `set_predicts_text` |
| `set_is_grouping_enabled` / `set_is_grouped` | `set_grouped` |
| `set_is_pull_to_refresh_enabled` / `set_is_refresh_enabled` | `set_refreshable` |
| `set_is_bounce_enabled` | `set_bounces` |
| `set_is_swipe_enabled` | `set_swipeable` |
| `set_aspect` | `set_fit` |
| `set_orientation` (scroll) | `set_axis` |
| `set_intent` (table) | `set_style` |
| `set_order` (toolbar) | `set_placement` |
| `set_indicator_color` / `_size` / `set_indicators_shape` | `set_dot_color` / `set_dot_size` / `set_dot_shape` |
| `set_selected_indicator_color` | `set_selected_dot_color` |
| `set_maximum_visible` (page dots) | `set_max_dots` |
| `set_refresh_control_color` | `set_refresh_color` |
| `set_stroke_thickness` | `set_stroke_width` |
| `set_stroke_line_cap` / `_join` | `set_stroke_cap` / `set_stroke_join` |
| `set_stroke_dash_array` | `set_stroke_dash` |
| `set_group_name` | `set_group` |
| `set_peek_area_insets` | `set_peek_insets` |
| `set_remaining_items_threshold` | `set_remaining_threshold` |
| `set_cascade_input_transparent` | `set_cascades_input` |
| `set_hide_single` | `set_hides_single` |
| `refresh_allowed()` | `can_refresh()` |
| `set_is_scroll_animated` | `set_animates_scroll` |
| `animate_current_item_changes()` | `animates_item()` — a read; MAUI declares no setter |
| `animate_position_changes()` | `animates_position()` — a read |

## `Handle` stands

125 uses in iris, and the decision holds. One `Handle` meant `set_value` on a
button compiled and silently did nothing; a per-control `find` means a wrong
verb is E0324 and a wrong kind is `None`. That is the property Stage 2 was
built to get, and the shared band means the verbs every element carries —
`set_opacity`, `focus`, `native`, `bounds` — are still on every cursor, so the
migration is a `match` at each site and not a redesign.

Worth confirming with a human before Stage 3 builds on it, because it is the
largest single change iris will see.
