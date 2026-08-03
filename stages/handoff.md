# Handoff — facet bootstrap, next session

Two tasks, both inside Stage 2. **Do not start Stage 3 until these are done.**

1. **Fix the names** — the generated vocabulary carries MAUI's naming through
   in places where naming_guideline.md says otherwise.
2. **Re-audit MAUI against what iris actually needs** — the ledger was built
   from MAUI alone, before we knew what an application reaches for.

Read `INTENT.md` first. Then `stages/1.md`, `stages/2.md`,
`reports/facet-iris-gap-2026-08-01.md`.

---

## Where things stand

Branch `facet-maui-regen`. Stages 1 and 2 are built and committed.

```
Stage 1  the ledger          DONE   865 rows, ADOPT 486 / DROP 379
Stage 2  the contract        DONE   38 controls, 322 verbs, 29 enums, ~10.5k generated lines
Stage 3  platform-free body  NOT STARTED — blocked on the two tasks below
Stage 4  headless suite
Stage 5  docs + provenance
```

```
c646fef  fix(facet): boolean setters drop the assertion prefix
a91d43e  feat(flex_layout): spacer + zstack
8638a0c  fix(facet): containers come from flex, not a second implementation
e5c44d7  fix(facet): shared bases are embedded, not restated
c4a6d9c  feat(facet): Stage 2 — the contract, generated
fc03417  feat(facet): Stage 2 slice — one control, whole
```

Gates, all currently green:

```
cd vendor/facet      && cpc test           # 291
                        cpc test --asan
                        cpc test --release
cd vendor/flex_layout && cpc test          # 280
cd vendor/stdlib      && cpc test          # 290
python3 tools/gen_contract.py              # must be byte-idempotent
```

Use `./target/release/cpc`, never the Homebrew one. Rebuild it first
(`cargo build --release`) — the language moves.

---

## The architecture, so you do not redesign it

Settled by evidence, not preference. Each of these was argued and several were
wrong once already.

- **facet's tree IS flex_layout's tree.** `type Node = flex::Node`. facet's
  per-node `Data` rides in flex's owned attachment slot. Geometry is read from
  flex (`frame()`), never copied. facet holds **zero** layout code.
- **A control is a constructor plus a typed cursor.** `button(title:, key:, …)
  -> Node` builds; `button::find(root, key) -> Option[Button]` addresses.
  There is **no generic `Handle`** and no generic element type: `find` returns
  the control with everything it can do and nothing it cannot. A wrong verb is
  E0324, a wrong kind is `None`.
- **`Data` carries a kind tag plus an owned typed pointer**, not a tagged
  union — an enum payload cannot be moved out through the raw pointer flex's
  attachment hands back (E0509), and a union would size every node by its
  largest variant.
- **A setter writes one field and sets a dirty bit.** No fn-pointer registrar
  anywhere. The backend will read dirty bits in ~37 apply functions.
- **The shared band lives once on `Node`** (facet.cplus), reached through
  generated one-line forwards on each cursor.
- **Shared bases are embedded, not restated** — `InputViewProps` is declared
  once and embedded by text_field/text_area/search_field. Composition is the
  workaround for having no inheritance; C+ has none (verified: `interface
  View: Element` and `struct B : A` are both E0100).
- **Cursors carry `{_p, _seen}`.** `_seen` is `flex::removal_count()`; while
  it matches, no node anywhere has been removed, so the pointer is live
  without dereferencing it.

---

## Task 1 — fix the names

The generator faithfully carries MAUI provenance and never checks its output
against naming_guideline.md. Three rules are being violated. One class was
fixed in `c646fef`; the rest are open.

### Already fixed, keep it that way

Booleans: the reader keeps the assertion, the setter and the parameter drop
it. `set_enabled(false)` / `is_enabled()`, not `set_is_enabled`. Guarded so a
stem that collides with a sibling field or a C+ keyword keeps the full form —
`set_is_opaque` is the one survivor.

### Open: omit needless words

The guideline's words: *"Drop words the type or context already implies.
`index_of_selected_item` becomes `selected_index`; `set_string_value` becomes
`set_text`."*

Worst offenders as emitted today:

```
36  set_horizontal_scroll_bar_visibility
34  set_vertical_scroll_bar_visibility
30  set_items_updating_scroll_mode
29  set_font_auto_scaling_enabled
29  set_horizontal_text_alignment
29  set_remaining_items_threshold
28  set_selected_indicator_color
27  set_clear_button_visibility
```

Candidates: `set_horizontal_scroll_bars(…)`, `set_text_align(…)`,
`set_font_scales(…)`. The `_enabled` suffix is redundant on a bool in 8 names
(`set_grouping_enabled` → `set_grouping`, `set_swipe_enabled` → `set_swipe`).

### Open: name for role, not for the class it wraps

*"Name a type for what it is, not the class it wraps."* MAUI-isms carried
through: `set_line_break_mode` (→ `set_line_break`), `set_return_type`
(→ `set_return_key`), `set_item_sizing_strategy` (→ `set_item_sizing`),
`set_image_source` (→ `set_image`), `set_text_type` (→ `set_text_format`?),
`set_items_updating_scroll_mode` (→ `set_scroll_anchor`).

Note the enum types were already renamed correctly in Stage 1
(`LineBreakMode`→`LineBreak`, `ReturnType`→`ReturnKey`), so the *verb* names
drifted from their own types. Fixing this aligns them.

### How to do it

The rename table belongs in `tools/maui_map.py`'s `OVERLAY`, not in the
generator — the MAP is the naming authority and the generator only transcribes
it. Add an `OVERLAY` row per verb you rename, regenerate, run the gates.

**Add an output check.** Every naming bug so far (`set_label` vs `set_title`,
the duplicated containers, `set_is_visible`) was invisible to tests. A lint
over the emitted verbs — name length, `_enabled`/`_mode`/`_type` suffixes,
`set_is_` — would have caught all three. Put it in the generator so it fails
the run, matching how Stage 1 guards rows and types.

---

## Task 2 — re-audit MAUI against iris's needs

Stage 1 read MAUI's PublicAPI in isolation. iris is 16,948 lines of real
application and it wants things MAUI never had, and does not want things MAUI
has. The ledger should be revisited with that evidence in hand.

Full findings: `reports/facet-iris-gap-2026-08-01.md`. The four that change
the ledger:

### 2a. 31 ADOPT rows never emitted

The generator skips a row whose facet type is in `SKIP_TYPES` and says
nothing — 345 of 376 emitted. Stage 1 exits non-zero on an unbucketed row and
on an unclassified type; **Stage 2 has no equivalent guard**, so the ledger
and the emitted contract disagree in silence. This is the same class of bug
Stage 1 was built to eliminate.

```
13  Node        ItemsView.EmptyView, StructuredItemsView.Header/Footer
 6  method      ProgressBar.ProgressTo, ItemsView.ScrollTo, ListView.BeginRefresh
 4  SwipeItem[] SwipeView.Left/Right/Top/BottomItems
 3  Node[]      ScrollView.Children, CarouselView.VisibleViews
 2  f64[]       Border.StrokeDashArray
 1  str[]       Picker.Items      <- iris needs this to fill a popup
 1  Drawable    GraphicsView.Drawable
 1  KeyboardAccelerator[]
```

Either emit them (decide how a `Node`-typed prop and a collection prop are
carried) or move them to DROP with a reason. Not silence. Then add the guard.

### 2b. The shared band is 15 of 41 forwarded

Missing from every cursor: **`native()`** — which the ledger explicitly
ADOPTed as the escape hatch INTENT blesses, and which iris uses 12 times as
`view()` — plus `bounds()` `is_focused()` `is_attached()` `focus()` `blur()`
`relayout()` `measure()` `children()` `begin_updates()` `end_updates()` and
the transform band beyond rotation/scale.

### 2c. 11 words are facet's own, with no MAUI provenance

INTENT: *"it grows by what its applications need, and adds words of its own."*
These are the words. They need declaring deliberately and recording in the
ledger as facet-origin rows, not appearing by accident.

`symbol` (SF Symbols glyph) · `wrap_label` · `composer` · `tree`/`TreeNode` ·
`split` + `set_split_position` · `clickable` · `window_buttons`/`window_drag` ·
`TextSpan` builder · `set_count`

`set_count` is the interesting one: MAUI binds `ItemsSource`, Stage 1 dropped
it as MVVM, so the imperative replacement — a row count plus a row builder —
is facet's to invent. There is currently no way to fill a list.

### 2d. Renames, not gaps

Most "missing" live verbs are the ledger's names differing from old facet's.
Table in the report. `set_background`→`set_background_color`,
`set_hidden`→`set_is_visible`, `scroll_offset_x`→`scroll_x`, `found`→obsolete
(`find` returns `Option`, not a null object).

---

## Invariants — do not break these

- **No silent anything.** Stage 1's row guard and type guard both exit
  non-zero and name what they found. Stage 2 needs the same and does not have
  it yet.
- **The generator owns its files whole.** Never hand-edit
  `vendor/facet/src/{vocabulary,props}.cplus` or the 38 control modules.
  `facet.cplus` and `test_main.cplus` are hand-written.
- **Regeneration is byte-idempotent.** If it stops being so, something reads
  a dict in nondeterministic order.
- **facet declares, the backend answers.** A verb AppKit cannot do still
  belongs in the contract; the backend records that it cannot, in its own
  manifest. There is no UNSUPPORTED bucket in facet.
- **flex owns layout.** If a thing is layout-only, it goes in flex_layout, not
  facet — that is how `spacer` and `zstack` were placed.
- **`str` only for strings whose bytes outlive the struct** — literals or
  interned. A `str` field fed from a `Text` dangles and the borrow check does
  not catch it across a call boundary. Repro:
  `examples/str_dangle_repro`.

---

## Open decisions, for the human

1. **`Handle` is gone by design** — 125 uses in iris. Every
   `facet::find(cp, k).set_text(…)` becomes a per-control `find` plus a match.
   Biggest single change iris will see. Confirm before Stage 3 builds on it.
2. **Chained build modifiers.** iris writes
   `facet::label("x").foreground_color(c).font(13.0)`. facet now writes
   `label::label("x", text_color: c, font_size: 13.0)`. Same rows, different
   form. Does facet also offer the chained form, or is the constructor the
   only way in?
3. **`key` storage.** `Data.key` is `str` today, sound only for literals.
   Interning makes it sound for computed keys and turns `find`'s comparison
   into a pointer compare. Not yet built.

---

## Files

| path | what |
|---|---|
| `INTENT.md` | the mental model both packages are judged against |
| `stages/1.md` `2.md` | what each stage decided and why |
| `stages/skel.txt` | the model sketch that preceded the slice |
| `reports/facet-iris-gap-2026-08-01.md` | the audit behind Task 2 |
| `naming_guideline.md` | the authority for Task 1 |
| `tools/maui_spec.py` | extracts the spec; owns the type-closure guard |
| `tools/maui_map.py` | the curated ledger; **the naming authority** |
| `tools/gen_contract.py` | emits the contract; needs an output lint |
| `vendor/facet/src/facet.cplus` | hand-written: tree, Data, shared band |
| `vendor/facet/docs/contract.md` | generated manifest, 322 verbs |
| `plans/facet/maui-map-draft.md` | generated 865-row ledger |
