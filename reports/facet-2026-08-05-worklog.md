# facet — 2026-08-05 worklog

One day, on branch `facet-maui-regen`. Start `e38057d`, end `7deef3f`.

Four phases: execute an existing gap report, build the example it asked
for, audit the region the gates never looked at, then fix everything the
audit found. Each phase produced the next one's work.

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
| `e06b4db` | **the gate counted a masked bit as implemented, and symbol proved it** |
| `248692e` | the shared band's background, shadow and clip |
| `df890d8` | a screen's menu items were required, then discarded |
| `725d642` | **spawn_ui tasks parked forever — the reactor had no wake source** |
| `774ba9e` | a zero measurement is not a size, and a span is not a box |
| `558df43` | on_focus, on_blur, and an is_focused that can answer true |
| `07c10f2` | window_drag, z_index, and a measure for views that draw themselves |
| `b460683` | a menu item can grey itself out, rename itself, and find |
| `bf81ed8` | a collapsed pane comes back, a tree reports expansion, a drag has ends |
| `8f6af16` | the runtime tier — agent, nav, menus, a leak, and two decisions |
| `3651198` | is_enabled reaches every kind, and the sheet is sent both its arguments |
| `7cff81b` | RTL mirrors, and the portable base is the full surface again |
| `7deef3f` | the audit is closed |

Gates at end: coverage green (**0 absent, 0 gated-unread, 0 never-fire**),
guard 5b **61/21/0**, contract provenance clean and byte-idempotent.
flex **280**, facet **513**, facet_appkit **657**, all passing.
`examples/hello_appkit` builds and drives correctly over the agent socket.

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

See the audit report for the ~30 findings, all now closed. The two fixed
on the spot, before the rest:

- **`C_SAFE_AREA` == `C_GESTURES`**, both 2^63 — self-inflicted that
  morning by counting "the top sixteen bits" in `props.cplus` alone, when
  the sixteenth is declared in `facet.cplus`. Bits 2^23..2^47 were free
  the whole time.
- **A label never grew when its text changed.** `"x"` measured 14.5pt and
  stayed 14.5pt after a 32-character `set_text` plus a full tick. Now 190.

---

## What is left

**Nothing from the audit.** Every finding is fixed, decided, or removed.
Final state: **flex 280 / facet 513 / facet_appkit 657** passing,
coverage gate green (0 absent, 0 gated-unread, 0 never-fire), guard 5b
61/21/0, contract byte-idempotent and provenance clean, and
`examples/hello_appkit` builds and drives correctly over the agent
socket.

Two things are deliberately still open, both in the TOOL rather than the
backend, and both are written into its docstring:

1. The read check asks whether SOME body reaching a control's struct
   reads the field, not whether the body that GATES the bit does. A
   reader that exists but is never called still counts. Closing it needs
   a call graph.
2. `NOT_CONTROLS` still exempts `gestures`, `theme`, `screen`,
   `application`, `nav`, `services` and `component`, and the 17 shared
   `C_*` bits with them. Everything in those modules was audited by hand
   in this pass; nothing yet stops the next one drifting.

Censusing those is the highest-value next tooling change, because it is
where `Chrome.zoomable` hid and where the shared band's seven dead verbs
hid.

### The pattern worth acting on

Zoom, the async pump, `z_paint_order` and `Screen::menu_items` are the
same shape: **worked before the regen, never ported, contract still
declares it.** All four were found incidentally. A systematic
`git show eb5b1b7:` diff against the current backend would likely surface
more, and is the highest-yield next search.
