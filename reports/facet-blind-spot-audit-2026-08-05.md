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

## 2. Fixed during this sweep

| Fix | Commit |
|---|---|
| `C_SAFE_AREA` and `C_GESTURES` were the SAME BIT (2^63) — self-inflicted that morning | `b7dc56f` |
| A label never grew when its text changed | `35408e9` |
| Pinch zoom: `Chrome.zoomable` had no consumer | `49c15f6` |

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

## 3. Confirmed gaps, not yet fixed

Ranked by how likely an app author is to hit them.

### Tier 1 — silently does nothing, common reach

| Gap | Evidence |
|---|---|
| **`symbol(icons::home)` renders nothing** | `apply_symbol` gates on `P_ICON\|P_FONT`, body reads only `(*p).name`, which is `""` for the Bundled tier. No font is ever registered (`CTFontManagerRegister` absent). A 949 KB font + **4,268** icon constants are dead on the only shipping backend. |
| **`background` (Brush) never applied** | `core::background(` → **0** backend hits. `controls.cplus:2452` even claims "`apply_background` makes the same reduction" — but that fn takes a `Color` and is only called with `background_color`. |
| **`shadow` never applied** | `core::shadow(` → 0 hits. `bordered` has no shadow prop of its own, so there is no working path at all. |
| **`on_focus` / `on_blur` never fire** | `fire_focus_handler` / `fire_blur_handler` have **zero callers**. `is_focused()` always answers `false` — `set_focused` is called only by a facet test. |
| **`spawn_ui` tasks park forever** | `pump_async` / reactor / kqueue / dispatch_source → **0** hits in the backend. The old backend had `install_async_pump` (`eb5b1b7:1611`). A task suspending on a timer or fd is never polled again. |
| **`Screen::menu_items()` is discarded** | `AppMenu::extend` has zero non-test callers. The interface *forces* every screen to implement it. Guard 5b is green on it. |
| **Controls measure zero-width in a row** | Probed: `NSSlider`, `NSProgressIndicator`, editable `NSTextField`, `NSSearchField` all report `fitting.width == 0`. `row { label text_field }` — the commonest form layout — gives the field size 0. Works in a column only by accident of `Stretch`. |

### Tier 2 — real, narrower reach

| Gap | Evidence |
|---|---|
| `clip` never applied | `core::clip(` → 0 hits |
| `is_enabled` ignored on non-control kinds | honoured on 12 NSControl kinds; silently ignored on box/label/image/canvas/scroll/list/web… |
| `background_color` cannot be cleared | `apply_background` early-returns on unset, leaving the old layer colour. The image path handles this correctly, so it reads as an oversight |
| `.window_drag()` is a no-op | `drags_window` → 0 backend readers. `Bar::Custom` + `window_buttons()` works; the drag half does not |
| `page_dots` / `window_buttons` measure 0×0 | plain views with hand-placed sublayers, but present in `has_intrinsic_size`, so flex trusts the zero |
| a label with `span` children loses its measure | flex uses the measure callback only when `child_count == 0` |
| viewless kinds are still in flow | `set_display` never applied to `span`/`context_menu`/`swipe_item`; they consume gaps and `justify` slots |
| `Split::expand()` never restores | no field stores the pre-collapse position; the resize write-back then zeroes it |
| tree expansion is write-only | `outlineViewItemDidExpand:`/`DidCollapse:` absent; `Tree::restore()` replays a stale set |
| `on_drag_start` / `on_drop_completed` never fire | zero references; a drag can begin but the app is never told |
| `MenuItem.is_enabled` / `title_of` never read | no `validateMenuItem:` anywhere |
| find-panel menu actions inert | `performFindPanelAction:` dispatches on the sender's tag; `setTag:` appears nowhere |
| `min_width` alone does nothing | `&&` where the max pair correctly uses `||` |
| `z_index` never applied | `core::z_index(` → 0 hits; the old backend had `z_paint_order` |
| `flow_direction` never applied | 0 hits for the verb or `setUserInterfaceLayoutDirection` |
| agent surface sees only the primary window | `agent_attach_window` fires at one site; pushed screens and **alert sheets** are invisible — defeating the stated reason the sheet was rewritten |
| `nav::go`/`quit` stall with a pushed screen open | `close_current` closes only the primary; the loop stops only at `OPEN_WINDOWS <= 0` |
| 5 `Window` interface methods have no caller | incl. `has_app_menu`/`app_menu` — a hand-written Window gets no menu bar |
| base `runtime.cplus` has drifted | 10 macOS-only verbs missing from the neutral base; `alert` diverges in signature |
| `observe_size` inert on viewless nodes | returns an already-cancelled handle — and `@ui { }` roots are viewless |
| menu bar accumulates across navigations | `install_menu_bar` appends per window and never removes |
| `present_window` leaks its tree | `into_raw()` with no matching `from_raw` |

### A correction to my own earlier correction

I told you `Window.ModalPopping` / `PopCanceled` were implemented via
`should_close`. That holds for the **primary** window only.
`push_screen` hardcodes `should_close: default_should_close`
(`runtime_macos.cplus:593`), which always returns true, and neither
`Screen` nor `ScreenBox` carries a `should_close` slot. An app **cannot**
refuse a pushed-screen dismissal — which is the only place
`ModalPopping` means anything. Guard 5b is green on both rows.

---

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
