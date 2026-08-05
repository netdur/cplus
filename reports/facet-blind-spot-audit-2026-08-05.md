# facet ↔ facet_appkit — the blind-spot audit, 2026-08-05

What five parallel audits found after `Chrome.zoomable` turned out to be
declared, filled, and never read by the backend.

Every finding below was re-verified by hand before it was written down.
Where an agent's premise was wrong, that is recorded too.

---

## 1. The headline: both gates validate CLAIMS, not implementations

This is the finding that matters, because it explains all the others and
because it means the green numbers cannot be trusted as-is.

facet has two coverage gates. Each has a false-positive mechanism, and
they are different mechanisms:

**`tools/verb_coverage.py` marks a verb LIVE when its bit is NAMED in a
backend function body — including inside a dirty MASK.** A body that
gates on `P_ICON | P_FONT` and then reads neither field scores two live
verbs. That is exactly how `symbol.icon` and `symbol.font` score green
while `symbol(icons::home)` renders nothing.

**Guard 5b (`TIER_ROWS` in `tools/gen_contract.py`) records a STRING that
names a declaration, and never checks that a consumer exists.** The row
`("Page","MenuBarItems"): ("implemented", "screen.Screen.menu_items")`
is green, and `Screen::menu_items()` is discarded by every caller.

So "0 absent / 0 deferred" means *every verb has a sentence written about
it*. It does not mean every verb works.

### What is not counted at all

| Uncounted | Why |
|---|---|
| the 17 shared `C_*` bits | `props.cplus` + `facet.cplus` are in `NOT_CONTROLS` |
| `gestures` (18 handlers + 6 params) | in `NOT_CONTROLS` |
| `theme`, `screen`, `application`, `nav`, `services`, `component` | in `NOT_CONTROLS` — `Chrome.zoomable` lives here |
| `context_menu` | declares no `P_*`, so `if not bits: continue` skips it |

**Correction to my earlier claim.** I told you `verb_coverage` only
enumerates the 38 generated modules and therefore misses the four
facet-origin kinds. That was wrong. It globs `vendor/facet/src/*.cplus`
and covers 41 modules including `symbol`, `tree`, `split`,
`window_chrome` and `canvas`. The blind spot is the hardcoded skip list
above, not "generated vs hand-written".

---

## 2. Fixed

| Fix | Commit |
|---|---|
| Pinch zoom: `Chrome.zoomable` had no consumer | `49c15f6` |
| `C_SAFE_AREA` and `C_GESTURES` were the SAME BIT (2^63) — self-inflicted that morning | `b7dc56f` |
| A label never grew when its text changed | `35408e9` |
| **The gate itself** — LIVE now needs the field READ, not the bit named; new `gated, unread` disposition; `--check` fails on it | `e06b4db` |
| **`symbol` renders** — bundled font registered per-process, codepoint into the label | `e06b4db` |
| **`background` (Brush), `shadow`, `clip`** — the shared band's paint trio, all three previously absent | `248692e` |
| **`Screen::menu_items()` is merged** — was required of every screen and discarded | `df890d8` |
| **The async pump** — `spawn_ui` tasks no longer park forever | `725d642` |

Turning the gate on named exactly two verbs — `symbol.icon` and
`symbol.font` — and both were real. It reports zero again now, this time
meaning it.

### The bit collision, because it is the cautionary one

`C_SAFE_AREA` was added at 2^63 on the reasoning that the shared band is
"the top 16 bits" and fifteen were spoken for. Fifteen were — *in
`props.cplus`*. The sixteenth, `C_GESTURES`, is declared in `facet.cplus`
because it is hand-written. The count was taken over the wrong file and
the two constants came out identical.

The band was not full in the way the comment implied: the highest
per-control bit is 2^22 and **bits 2^23..2^47 are unused**. "The top
sixteen" was a convention, not a limit. `safe_area` now takes 2^47.

The guard that missed it asserted `C_GESTURES` was disjoint from
`C_HANDLERS` and nothing else. The replacement asserts over every shared
constant wherever declared, arithmetically (for single-bit values, OR ==
SUM iff disjoint), and was verified against the bug: restoring 2^63 makes
it fail.

### The label re-measure, because it is the most user-visible

Measured: a label reading `"x"` in a row is 14.5pt wide. After
`set_text("a very much longer string than x")` plus a full tick and
layout pass — still 14.5pt. Now 190.

`set_text` changes no *style*, so flex's cache pruned the subtree and the
frame walk early-returned. `mark_content_changed` is flex's own door for
this and had **one caller in the repo** (`core::relayout`). A column hides
it entirely via `align_items: Stretch`, which is why it survived.

---

## 3. Confirmed gaps — all closed

Every item found by the audit is now fixed, decided, or removed. The gate
reports **0 absent, 0 gated-unread, 0 never-fire**, and it means it: it
was corrected first, so the number is evidence rather than an assertion.

| Was | Now |
|---|---|
| `symbol(icons::home)` rendered nothing | bundled font registered per-process, codepoint into the label |
| `background` (Brush) never applied | solid → layer colour, gradient → CAGradientLayer at index 0 |
| `shadow` never applied | layer shadow, y-offset flipped for the flipped view |
| `clip` never applied | CAShapeLayer mask |
| `on_focus` / `on_blur` never fired; `is_focused()` always false | two routes (armed class + text delegate), edge-triggered |
| `spawn_ui` tasks parked forever | dispatch read source on the reactor's kqueue |
| `Screen::menu_items()` discarded | merged per screen in `App::run` and `run_screen` |
| controls measured zero-width in a row | a zero fitting size is floored to the offered space |
| `page_dots` / `window_buttons` measured 0×0 | measured by what they drew |
| `is_enabled` ignored on non-control kinds | dimmed, but only where AppKit has no `setEnabled:` |
| `background_color` could not be cleared | clears when a layer already exists |
| `.window_drag()` was a no-op | `performWindowDragWithEvent:`, and it now counts as a gesture |
| a label with `span` children lost its measure | non-view kinds are `Display::None`; flex counts in-flow children |
| viewless kinds consumed gaps and justify slots | same fix |
| `Split::expand()` never restored | `saved_position`, and the resize write-back stands down while collapsed |
| tree expansion was write-only | `outlineViewItemDidExpand:` / `DidCollapse:`, guarded against self-application |
| `on_drag_start` / `on_drop_completed` never fired | the session's own two callbacks |
| `MenuItem.is_enabled` / `title_of` never read | `validateMenuItem:` on the item's target |
| find-panel actions inert | `performTextFinderAction:` + the tag that selects the operation |
| `min_width` alone did nothing | each axis answered on its own |
| `z_index` never applied | siblings ordered in the frame walk |
| `flow_direction` never applied | flex mirrors the layout; controls mirror their text |
| agent saw only the primary window | pushed screens and presented windows attach |
| `nav::go`/`quit` stalled with a pushed screen | pushed screens close first |
| 5 dead `Window` methods | menu pair wired, three vestigial ones removed |
| base `runtime.cplus` had drifted | ten verbs mirrored; `alert` no longer diverges |
| `observe_size` inert on viewless nodes | observes the nearest backed ancestor |
| menu bar accumulated across navigations | tree menus marked and replaced |
| `present_window` leaked its tree | `presented_closed` |
| `beginSheet:` sent one argument | both, with an explicit nil handler |
| `touch_points` / `swipe_threshold` | **decided** — recorded in the manifest with the reason |

### The two decided, and why

`touch_points` needs a finger count `magnifyWithEvent:` does not carry,
and the API that does is answered by a trackpad and not a mouse — a verb
whose meaning depends on the attached hardware is worse than one that
says it cannot. `swipe_threshold` needs a distance, and
`swipeWithEvent:` delivers a discrete ±1 already recognised and
quantised. The continuous form of that question IS answered, in the
swipeable strip's own drag, because facet tracks that one itself.

They are recorded in the manifest rather than the `cannot-ledger` block,
because that block is machine-checked against CONTROL verbs and these
are on `gestures`, which the tool does not census. That is a remaining
gap in the tool, not in the decision.

### Still open in the tool, stated plainly

The gate now checks that a field is READ, not merely that its bit is
named. Two limits remain, both written into its docstring:

- It asks whether SOME body reaching the struct reads the field, not
  whether the body that GATES the bit does. A reader that exists but is
  never called still counts. Closing that needs a call graph.
- `NOT_CONTROLS` still exempts `gestures`, `theme`, `screen`,
  `application`, `nav`, `services` and `component` from the census, and
  the 17 shared `C_*` bits with them. Everything in those modules was
  audited by hand here; nothing yet stops the next one.

## 4. What was checked and is genuinely fine

Worth recording so it is not re-audited: **theme is fully wired** (all 24
Tier-1 tokens, adaptive colours, 18 roles, live `set_theme` repaint,
appearance flip). **The canvas replay is complete** — all 39 `DrawOp`
variants dispatched with real bodies, no stubs. **All 50 `elements.cplus`
DSL forwards resolve.** The `Scheduler`, `SenderReaders` and `KeyReader`
install structs are fully populated with no null slots. `nav`
push/pop/go/quit are really implemented. **`has_intrinsic_size` is
complete** — no kind is missing; all 42 were checked and the omissions
(containers, surfaces, non-views) are all correct, including `text_area`
(`NSScrollView(NSTextView).fittingSize` is 0×0 anyway).

---

## 5. What to do, in order

1. **Fix the gates first.** Everything else is noise until the numbers
   mean something.
   - `verb_coverage.py`: a verb is live only if the backend READS THE
     FIELD, not merely names the bit. Mask-only mentions must not count.
   - Census the uncounted: the 17 shared bits, `gestures`, `theme`,
     `screen`/`Chrome`, `nav`, `services`.
   - Guard 5b: a row claiming `("implemented", "x.y")` should fail when
     nothing consumes `x.y`.
2. **Tier 1**, roughly in the order listed — `symbol` first, since a
   4,268-constant icon set and a bundled font are inert.
3. **Tier 2**, cheapest-first: `min_width` (one operator), `clip`,
   `shadow`, `background` (Brush) are each small and local.
4. **Decide, don't build**, for `touch_points` and `swipe_threshold` —
   both are plausibly "AppKit cannot", but neither is in the
   cannot-ledger, which is the manifest's own definition of debt.

### A note on the method

Four of the biggest findings — zoom, the async pump, `z_paint_order`,
`Screen::menu_items` — are the same shape: **capability that worked
before the regen and was not ported, where the contract still declares
it.** `git show eb5b1b7:` is worth a systematic diff against the current
backend; this audit found those four incidentally, not by looking.
