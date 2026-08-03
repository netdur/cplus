# facet ← iris gap audit — 2026-08-01

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

## Disposition

| finding | owner | status |
|---|---|---|
| containers, layout modifiers | flex_layout | DONE — re-exported, renames documented |
| `spacer` / `zstack` | flex_layout | DONE (`a91d43e`) |
| live-verb renames | none | naming table above; migration only |
| the 23 tier symbols | Stage 3 | to build |
| the 11 facet-original words | Stage 3 | to declare deliberately |
| 31 skipped ADOPT rows | **Stage 2** | fix before Stage 3 |
| shared band 15/41 forwarded | **Stage 2** | fix before Stage 3 |
| `Handle` → typed cursors | decision | confirm |
