# facet — 2026-08-05 worklog

One day, on branch `facet-maui-regen`. Start `e38057d`, end `8630cd4`.

Three phases: execute an existing gap report, build the example it asked
for, then audit the region the gates never looked at. Each phase found
the work for the next one.

Companion documents, both committed:

- [`facet-appkit-contract-gap-2026-08-05.md`](facet-appkit-contract-gap-2026-08-05.md) — the contract ↔ backend gap report, verified and then executed. Its P1/P2/P3 sections are annotated with the commit that closed each.
- [`facet-blind-spot-audit-2026-08-05.md`](facet-blind-spot-audit-2026-08-05.md) — the five-way audit of what neither coverage gate censuses. **The substantive findings live here.**

---

## Commits

| | |
|---|---|
| `6725d8c` | the touch-gesture controls are live, and the list has one body |
| `287ab8e` | `is_secure` flips live, because a different object is a cost not a wall |
| `585f643` | the sequence limits, and two tier rows that were closed and still read as debt |
| `6cf5fca` | **a DSL-authored screen reached the window empty** |
| `199f814` | stage 4 item 7 closed, and what building the example found |
| `f365603` | the last five tier rows — two carriers on the shared band |
| `49c15f6` | **pinch zoom, which Chrome had asked for and nothing answered** |
| `b7dc56f` | **`C_SAFE_AREA` and `C_GESTURES` were the same bit** |
| `35408e9` | **a label never grew when its text changed** |
| `8630cd4` | the blind-spot audit |

Gates at end: coverage green, contract provenance clean, contract
byte-idempotent. facet **513 passed / 0 failed**; facet_appkit **636
passed / 0 failed**. `examples/hello_appkit` builds and runs.

---

## Phase 1 — the gap report, verified then executed

Verified every claim against the code before acting. The headline numbers
reproduced exactly; three numbers did not (the MAUI map row was wrong on
all three: 865/486/379 → **873/496/377**).

Closed P1, P2 and P3 in full:

- **P3** — MANIFEST said `refreshable`/`swipeable` were unimplemented and
  got "a plain backing view". Both had been live for a while. Deleted the
  dead `create_list`/`materialise_list`/`apply_list` trio (zero callers).
- **P2** — `text_field.is_secure` made live via `views::reclass`
  (coverage 284→285 live, 8→7 create-only). `collection` documented
  rather than rebuilt, **with the real reason corrected**: it is
  `CanReorderItems`, not the grid argument the manifest gave — reorder
  ends in `mount::remove_child`/`insert_child` on real children, and a
  recycling collection has none. `table` recorded as a contract decision.
- **P1** — the tier ledger's deferred column emptied: `safe_area` and
  `background_image` onto the shared band, `Page.IconImageSource` recorded
  as `cannot`. **61 implemented / 21 decided / 0 deferred.**

Also corrected `stages/4.md`, which was stale on two rows: the titlebar
slots were built in `ce765c5`, and `ModalPushing`/`NavigatingFrom` are
recorded `cannot` rather than owed. **Guard 5b (`TIER_ROWS` in
`tools/gen_contract.py`) is the authority, not the stage prose.**

## Phase 2 — the example, which paid for itself immediately

`examples/hello_appkit` — one screen, one file, @ui-authored, themed,
keyed, static-free, with the agent surface wired so it can be driven
without a person at the keyboard. (`examples/` is gitignored; the fixes
and their tests are in the tree, the app is not.)

It found three bugs the whole suite was green through:

1. **A DSL-authored screen reached the window EMPTY.** `@ui { }` is a
   keyless container; `wants_view` gives such a node no view by design;
   `open_window` inserted only the root's own view. So a pass-through
   root left its children inserted nowhere — **every app authoring its
   screen the way the contract asks opened a blank window.** The
   framework's own `open_window` test uses a *keyed* column, which is
   backed, which is why it never showed.
2. A focused field lost its text across an `is_secure` flip.
3. A reclassed view lost its accessibility identifier and its
   intrinsic-size measure.

None of the three is visible in a screenshot — which is the argument for
an example that carries an agent surface rather than only eyes.

## Phase 3 — zoom, and the audit it triggered

Reported from the running app: zoom does not work. It did not, twice over
— the example never set `zoomable` (it defaults false), and **nothing in
the backend read the field anyway.** Pinch zoom worked before the regen
and was never ported.

It needed *both* gates to miss it, for different reasons: `verb_coverage`
counts per-**control** bits and `zoomable` is on `Chrome`; guard 5b counts
**MAUI-seeded** rows and `zoomable` is facet's own word. That question —
what else is facet-original *and* not on a control? — is what the
five-way audit answered.

See the audit report for the ~30 findings. The two fixed on the spot:

- **`C_SAFE_AREA` == `C_GESTURES`**, both 2^63 — self-inflicted that
  morning by counting "the top sixteen bits" in `props.cplus` alone, when
  the sixteenth is declared in `facet.cplus`. Bits 2^23..2^47 were free
  the whole time.
- **A label never grew when its text changed.** `"x"` measured 14.5pt and
  stayed 14.5pt after a 32-character `set_text` plus a full tick. Now 190.

---

## What is left, in order

1. **Fix the gates.** Both validate claims rather than implementations —
   `verb_coverage` counts a bit named in a *mask* as live, guard 5b checks
   that a string names a declaration and never that a consumer exists.
   Until this is done the green board is not evidence.
2. **Tier 1 gaps**, `symbol` first — a 949 KB bundled font and 4,268 icon
   constants are inert on the only shipping backend.
3. **Tier 2 gaps**, cheapest first: `min_width` is one operator.
4. **Decide, don't build**, for `touch_points` / `swipe_threshold`.

### The pattern worth acting on

Zoom, the async pump, `z_paint_order` and `Screen::menu_items` are the
same shape: **worked before the regen, never ported, contract still
declares it.** All four were found incidentally. A systematic
`git show eb5b1b7:` diff against the current backend would likely surface
more, and is the highest-yield next search.
