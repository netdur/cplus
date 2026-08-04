# Handoff — facet bootstrap, next session

Stage 2 is **done**. The two tasks the last handoff left open are closed and
guarded. Stage 3 is unblocked, with three decisions below that want a human
before it builds on them.

Read `INTENT.md` first. Then `stages/1.md`, `stages/2.md`,
`reports/facet-iris-gap-2026-08-01.md`.

---

## Where things stand

Branch `facet-maui-regen`.

```
Stage 1  the ledger          DONE   865 rows, ADOPT 482 / DROP 383
Stage 2  the contract        DONE   38 generated + 4 hand-written controls, 362 verbs
Stage 3  platform-free body  DONE   2026-08-04, per stages/3.md end to end: tier
                                    symbols (six modules) + renames; mount seam
                                    (Renderer, mount/sync walks, M1-M7); the 12
                                    deferred rows emitted (fn+ctx — the events dep
                                    died with decision 5's caveat); set_content +
                                    switch_to (Display::None parking); FontWeight +
                                    is_italic; guard 5; the key band; find(key,
                                    within:) over mounted_root(). facet 430
                                    plain/asan/release. UNDER USER REVIEW.
Stage 4  headless suite
Stage 5  docs + provenance
```

Gates, all green:

```
python3 tools/maui_map.py                  # fails on an unbucketed row
python3 tools/gen_contract.py              # fails on any of four guards; byte-idempotent
python3 tools/gen_icons.py                 # the icon table, from the font
cd vendor/facet      && cpc test           # 430 (DSL, tier, mount seam, composition, keys)
                        cpc test --asan
                        cpc test --release
cd vendor/flex_layout && cpc test          # 280
cd vendor/stdlib      && cpc test          # 290
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
  anywhere. A *command* (`scroll_to`, `begin_refresh`, `focus`) is the same
  shape — a write plus a bit — so the backend seam stays ~37 apply functions
  and never becomes a verb-indexed table.
- **A subtree is a child, not a property.** `header`, `footer`, `empty_view`,
  `content` are **slots**: a named child recorded in `Data.slot`, replaced in
  place by `core::set_slot`. The name is a separate field, not the key, so the
  application's own key survives and `find` still reaches the node.
- **The shared band lives once on `Node`** (facet.cplus), reached through
  generated one-line forwards on each cursor. 36 of the ledger's 41; the 5
  that are not are in `DEFERRED_SHARED` with a reason each.
- **Shared bases are embedded, not restated** — `InputViewProps` is declared
  once and embedded by text_field/text_area/search_field. Composition is the
  workaround for having no inheritance; C+ has none (verified: `interface
  View: Element` and `struct B : A` are both E0100).
- **Cursors carry `{_p, _seen}`.** `_seen` is `flex::removal_count()`; while
  it matches, no node anywhere has been removed, so the pointer is live
  without dereferencing it.
- **Ownership hierarchy (2026-08-04, user-directed): process → app → window
  → node.** One exe may contain several apps (one runs at a time); an app
  owns theme/nav/windows in `app_state.cplus` (`runtime::App` embeds the
  record, `run` points the ONE process door at it); a window owns its root;
  a node owns its presentation in `Data`. Module statics are legal only for
  what the whole process shares: the Renderer, platform hook slots, the UI
  thread, the RUNNING door. This is MAUI's own Application/Windows scaffold
  (the ledger's 13 Application + 30 Window rows) — anything app- or
  window-scoped in a static is drift.
- **The `@` builder DSL is the authoring surface, reconnected 2026-08-04.**
  The regen had orphaned it (controls left the `facet::` namespace, so
  `@facet { button(...) }` resolved nothing). The rendezvous is generated:
  `elements.cplus` forwards every constructor into one module, and DSL.5
  (`name(args) { children }`, Builder first) lets containers take keys and
  config where SwiftUI puts them. `import "facet/elements" as ui;` then
  `@ui { vstack(key: "body") { button("Save", key: "save") } }` — proven by
  four tests in `test_main.cplus`, including `split` taking panes as DSL
  children. Props in the constructor, layout via modifiers, children by
  nesting; there is no second build-time spelling for control props.

---

## What Stage 3 owes

The 23 tier symbols iris reaches for, none of which is widget vocabulary, so
MAUI was never going to supply them:

`Component` `Lifecycle` `Screen` `ScreenBox` `screen_box` `Service`
`load_service` `Chrome` `AppMenu` `app_menu` `menu_action_*` `run_on_main`
`on_worker` `after` `Cancellable` `component_at` `raise` `key_of` `set_theme`
`Theme` `is_dark` `observe_size`

**The 23 tier symbols EXIST (2026-08-04) — the mount seam does not.** Ported into
`nav.cplus` / `services.cplus` / `component.cplus` / `screen.cplus` /
`theme.cplus` / `runtime.cplus` (the neutral facade base platform facades
shadow). Three reseams vs the old monolith, everything else copy-paste:
`observe_size` takes `*Node`; the lifecycle/presentation registry is keyed
by KEY not `Handle`; the Color token model (two tiers + adaptive pairs)
lives in the generated vocabulary and `theme.cplus` holds the roles.
`set_theme`'s router re-stage call is stripped until the outlet router
ports (node-keyed). The old `MenuItem` default-`action` value (2 =
close_window, commented as none) was kept verbatim — flagged, not fixed.

**facet's own words are DONE.** All eleven were worked through on 2026-08-03
and each is recorded in the ledger's *facet's own words* section with what it
became. Four are new hand-written modules, two are modifiers, three folded
into things facet already had, and none of them appeared by accident:

| word | what it became |
|---|---|
| `symbol` | `src/symbol.cplus` + `src/icons.cplus` (4,268 generated constants) |
| `tree` / `TreeNode` | `src/tree.cplus` — generic, `id` and `is_branch`, not `path` and `is_dir` |
| `split` | `src/split.cplus` — `SplitAxis::Columns\|Rows`, panes Leading/Trailing |
| `window_buttons` | `src/window_chrome.cplus` — a control, it draws something |
| `.window_drag()` | a MODIFIER in the same file — draws nothing, so it is not a control |
| `.gesture(…)` | `src/gestures.cplus` — the 21 orphaned gesture rows landed here |
| `clickable` | **does not exist.** `.gesture()` on any node is what it was |
| `TextSpan` / `Spans` | a real owned list of runs; `Spans` had been a stub, so `set_formatted_text` was a verb nothing could feed |
| `set_count` / `set_row` | emitted on `list` and `collection` |
| `wrap_label` | **not a control** — `label(…, line_break: WordWrap)`. Which NSTextField factory to call is the backend's sentence |
| `composer` | **not a control** — text_area + bordered + a background is a COMPONENT (Stage 3's tier), not a new word |

Do not let a twelfth appear by accident: `FACET_ORIGIN` in `tools/maui_map.py`
is where a word gets declared, and it carries the reasoning for every one.

**Two conventions the new modules follow.** Facet-origin controls take kind
tags from **1000 up** (`symbol` 1000, `tree` 1001, `split` 1002,
`window_buttons` 1003), so regenerating the 38 MAUI-derived modules can never
collide. And a behaviour that draws nothing is a **modifier on Node**, not a
control — `.gesture()` and `.window_drag()` both, for the same reason.

Two things unblock as soon as the events package is a dependency:

- the **5 deferred shared-band rows** (`on_focus` `on_blur` `on_attach`
  `on_detach` `observe_size`), and
- the **7 recorded-skip control rows**, all genuinely continuous
  (`observe_scrolled` ×4, `observe_drag_interaction`,
  `observe_move_hover_interaction`, `observe_swipe_changing`).

Both lists are in `tools/gen_contract.py` and both are guarded: move a row out
of one and the run fails until it is emitted or recorded elsewhere.

---

## Invariants — do not break these

- **No silent anything.** Four guards in `gen_contract.py` exit non-zero and
  name what they found: a row nothing carries, a command that writes a field
  its control lacks, a shared-band verb in neither list, and a name
  `naming_guideline.md` rejects. Nothing is written when any of them fires.
  They have been tested by breaking each one on purpose.
- **The MAP is the naming authority.** A verb is renamed in
  `maui_map.RENAME` (keyed by MAUI member — the rule is about the word) or in
  `OVERLAY` (keyed by row, when a second type would read differently). Never
  in the generator, which only transcribes.
- **The generator owns its files whole.** Never hand-edit
  `vendor/facet/src/{vocabulary,props,elements}.cplus` or the 38 control
  modules. `facet.cplus` and `test_main.cplus` are hand-written.
  `elements.cplus` is the DSL namespace: every constructor forwarded into
  one module so `@ui { ... }` (`import "facet/elements" as ui;`) resolves
  bare element names; the four facet-origin controls are forwarded from
  literals in `gen_contract.py`, and drift there breaks the facet build,
  which is the guard.
- **`test_main.cplus` imports all 38 control modules.** It did not before, and
  32 of them were emitted and never compiled — which is how a control could
  carry a verb that collided with the shared band and nothing noticed. If you
  add a control, add its import.
- **Regeneration is byte-idempotent.** If it stops being so, something reads
  a dict in nondeterministic order.
- **facet declares, the backend answers.** A verb AppKit cannot do still
  belongs in the contract; the backend records that it cannot, in its own
  manifest. There is no UNSUPPORTED bucket in facet.
- **flex owns layout.** If a thing is layout-only, it goes in flex_layout, not
  facet — that is how `spacer` and `zstack` were placed.
- **`str` only for strings whose bytes outlive the struct** — literals or
  interned. A `str` field fed from a `Text` dangles; the borrow check catches
  it now (E0514 across a `ref` parameter, E0516 across a raw pointer), but the
  rule is what keeps props writable at all, so props hold `text::Text` and
  never a view. That is why `Data.key`, `Data.slot` and the shortcut key are
  owned. Repro: `examples/str_dangle_repro`.

---

## Open decisions, for the human

1. **DECIDED 2026-08-04: no `Handle`, confirmed.** Stage 3 builds on
   per-control `find` plus a match at each of iris's 125 sites. The shared
   band keeps the universal verbs on every cursor, so it is a `match` per
   site and not a redesign.
2. **Chained build modifiers — OPEN, leaning constructor-only.** iris
   writes `facet::label("x").foreground_color(c).font(13.0)`. facet now
   writes `label::label("x", text_color: c, font_size: 13.0)`. The live
   side chains because every setter returns the cursor. Offering the
   chained form at build time too means a second generated function family
   per verb with different mechanics: a build modifier threads an owned,
   unattached `Node` (`take this -> Node`, no `_seen`, no dirty bit, no
   relayout), a live setter writes through a guarded pointer and marks
   dirty — same name, divergent semantics, and the map would have to name
   both. Named-param constructors already cover build time with one
   authority. Layout chaining on `Node` (`.width()`, `.grow()`) is
   flex_layout's API and stays either way.
3. **DECIDED 2026-08-04: `key` stays owned `text::Text` — which it
   already was.** This item's premise was stale: `Data.key` moved to owned
   Text in the boundary pass (see the `struct Data` comment; iris builds 25
   keys by interpolation and they work). Interning remains an available
   optimization (pointer-compare `find`), deliberately not taken now.

Two smaller ones, worth a glance:

4. **Event names still read as MAUI's** where the default rule was right about
   the band but not about the word: `on_web_view_initializing`,
   `on_raw_message_received`, `on_process_terminated`, `on_close_requested`.
   Role names would be `on_initialize`, `on_message`, `on_crash`, `on_close`.
   Left alone deliberately — renaming an event is a judgement call, not a rule
   the lint can make.
5. **`vocab::Shortcut` narrows MAUI.** `MenuFlyoutItem.KeyboardAccelerators`
   is a list; facet declares one key equivalent, because that is what every
   platform with menus shows. If a second is ever wanted, the row is in
   `OVERLAY` with that reasoning attached.

---

## Known-broken, pre-existing

`vendor/terminal` and `vendor/facet_gtk` declare `facet = "*"` and consume the
**old** facet API (`set_payload`, `role_none`). They have not compiled since
Stage 2 replaced `vendor/facet` in `c4a6d9c`, and this session did not change
that either way. `vendor/facet_appkit` is an empty scaffold and is Stage 3's.

---

## Files

| path | what |
|---|---|
| `INTENT.md` | the mental model both packages are judged against |
| `stages/1.md` `2.md` | what each stage decided and why |
| `stages/skel.txt` | the model sketch that preceded the slice |
| `reports/facet-iris-gap-2026-08-01.md` | the audit, its disposition, and iris's migration table |
| `naming_guideline.md` | the authority the lint enforces |
| `tools/maui_spec.py` | extracts the spec; owns the type-closure guard |
| `tools/maui_map.py` | the curated ledger; **the naming authority** (`RENAME`, `OVERLAY`, `BAND`, `FACET_ORIGIN`) |
| `tools/gen_contract.py` | emits the contract; owns the four output guards |
| `vendor/facet/src/facet.cplus` | hand-written: tree, Data, the shared band, slots |
| `vendor/facet/src/test_main.cplus` | hand-written: 44 tests over all 38 modules |
| `vendor/facet/docs/contract.md` | generated manifest, 362 verbs + what is not emitted and why |
| `plans/facet/maui-map-draft.md` | generated 865-row ledger + facet's own words |
