# Grok review — Stage 3

Reviewed: **2026-08-04**  
Against: `stages/3.md` (status: built, under review) and the on-disk
`vendor/facet` tree on branch `facet-maui-regen`.  
Method: read-only source audit of the mount seam, tier modules, renames,
guards, and late finds; generators run for idempotence; full facet gate with
the **project** compiler (`./target/release/cpc`, not Homebrew). **No
implementation code was changed.**

## Verdict

Stage 3 delivers the **platform-free body it was asked for** at the API and
seam level: mount/sync walks with M1–M7 answered in code, the tier modules,
the renames, Chrome/`Bar`, composition verbs, FontWeight + italic, the key
band, typed `find(key, within:)`, theme, and guard 5. The documented test
gate is **green** when the right `cpc` is used: **430 / 430** plain, ASan,
and release.

It is **not** a complete discharge of every paragraph on the stage page.

The biggest gap is the **82 tier ledger rows**: they are *owned* (guard 5
passes) but largely *unimplemented* as verbs. The planned **agent surface
is missing**. A few seam and lifetime bugs will bite the first real backend
or a teardown path that trusts the comments. Treat the page status
("built … end to end") as true for the **seam and tier skeleton**, not for
"every ADOPT row now has a home that does the work."

---

## Gate results

| Command | Result |
|---|---|
| `python3 tools/maui_map.py` | green — 873 rows, ADOPT 490 / DROP 383 |
| `python3 tools/gen_contract.py` | green — 29 enums, 38 controls, byte-idempotent |
| `python3 tools/gen_icons.py` | green — 4268 icons (70 remapped) |
| `cd vendor/facet && ../../target/release/cpc test` | **430 passed** |
| `… cpc test --asan` | **430 passed** |
| `… cpc test --release` | **430 passed** |

Of the 430, **144 are facet's**, 124 flex_layout, 162 stdlib (the package
pulls its deps into one run). That matches the handoff number.

**Compiler note.** `which cpc` is Homebrew `cplus 0.0.26`
(`/opt/homebrew/bin/cpc`). That binary **fails to parse** the `@ui` block in
`test_main.cplus:2262` (`expected ';' or '}', found identifier` on the second
child). `stages/handoff.md` already says to use `./target/release/cpc` only —
that is required, not optional. Anyone reviewing Stage 3 with PATH `cpc` will
wrongly report a red suite.

Stage 3's gate comment still says gen_contract has "**four** guards"; the
file now has **five** (guard 5 is real). Cosmetic doc drift only.

---

## What is solid

### Mount seam (the thing everything waited on)

`vendor/facet/src/mount.cplus` is the heart of the stage and it is real, not
a stub.

| Decision | Answer in code | Evidence |
|---|---|---|
| **M1** view ownership | `Data.view` + `Data.view_release`; teardown calls the release | `facet.cplus` `Data` / `release_data`; test `view_release_fires_on_node_teardown` |
| **M2** where Renderer lives | one process static, `mount::install` | `RENDERER` + `INSTALLED` |
| **M3** creation order | top-down, insert-as-created | `mount_walk`; test `mount_creates_topdown_…` |
| **M4** re-entrancy | walk is not re-entrant; attach/detach handlers drain **after** the walk | `drain_attach_queue` / `drain_detach_queue`; test `mount_fires_on_attach_after_the_whole_walk` |
| **M5** sync scheduling | `touch` → installed `schedule`; batch silent until `C_FLUSH` at depth 0 | `touch` + `batched_writes_request_once_at_flush` |
| **M6** thread discipline | `install` records UI thread; `touch` asserts on it | `set_main_thread_id` + `assert on_ui_thread()` |
| **M7** "the mounted tree" | newest window root of the **running app** | `application::newest_window_root` / `mounted_root` |

The Renderer shape is a deliberate, documented shrink of the stage draft:
**five verbs** (`create`, `view_release`, `apply`, `insert`, `remove`, plus
`schedule`) with per-kind apply living **inside** the backend's `create`/
`apply`. That matches Stage 2's "no per-verb registrar" rule better than a
vtable of ~37 apply slots. The stage page still describes the larger shape;
the code comments own the smaller one. That is fine if the page is updated —
today the page and the code disagree on form, not on purpose.

Fake renderer tests cover mount, sync, batching, unmount detach, view
release, post-walk attach, `set_content` (headless), and `switch_to`.

### Tier modules

Present and imported from `test_main.cplus`:

- `component` — `Component`, `Lifecycle`, `component_at`, `raise`, `key_of`,
  `item_of`, **`dropped_text` / `drag_targeted`** (late find #2 closed)
- `screen` — `Chrome`, `Bar`, `Screen`, `ScreenBox`, app menu
- `services` — `Job` / `run_job`, `run_on_main`, `run_on_worker`, `after`,
  `Cancellable`, `observe_size`
- `nav` — `go` / `push` / `pop` / `quit` intent surface
- `theme` — roles, `set_theme`, live-repaint hook, `is_dark`
- `runtime` — neutral facade (loud no-backend), `App`, agent **hook slots**
- `mount` — the seam
- `application` — process → app → window ownership (`Application.Current`,
  open/close window)

Status line says "**six** modules"; on disk there are **eight** once mount and
application are counted. Naming only.

### Renames and composition

- `Service` / `load_service` / `on_worker` → `Job` / `run_job` / `run_on_worker`
  (no old names left in `vendor/facet/src`)
- `present` → `set_content` (component form in `mount.cplus`; Node-taking
  `set_content` already on scroll/bordered/table/radio — same word, two arities,
  as the stage asked to reconcile)
- `Chrome.bar: Bar` with `Native | Blended | Hidden | Custom`;
  `close_button_only` kept separate
- `switch_to` parks under `Display::None` (no remove/reinsert router)

### Late finds and other settled work

- **FontWeight** scale (UltraLight…Black) + **`is_italic`** on the font-carrying
  controls; `FontStyle` is gone from vocabulary/props
- **Key band**: `.gesture(on_key:)`, `key_named` / `key_escape` / arrows / mods,
  `consume_key`
- **Global find**: every control's `find(key, within: = mounted_root())`;
  `in:` avoided (keyword)
- **Key storage**: owned `Text` on `Data`; tree row identity remains `id`
- **Parking / identity suite tests** still present in `test_main.cplus`
- **No `events` package dependency** — intentional; `Cancellable` is local,
  not `events::Subscription` (decision 5)

### Guard 5

Exists and runs. It fails the generator if an ADOPT type reaches neither a
control module nor `TIER_TYPES`. That closes the "unclaimed type" hole the
stage described. It does **not** prove the row's verb is implemented (see
findings).

---

## Findings

### 1. The 82 tier ADOPT rows are assigned, not implemented

**Severity: high — stage goal under-delivered**

Stage 3 says this is where the ledger debt gets homes:

```
30 Window · 14 Page · 14 Toolbar · 13 Application · 8 TitleBar · 3 ContentPage
```

Guard 5 only checks that the **type** is listed in `TIER_TYPES` (or a control
module). It never checks that `set_is_maximizable`, `display_density`,
`set_accent_color`, `set_background_image_source`, `set_back_button_enabled`,
`set_leading_content`, etc. exist in the owner module.

A scan of the owner modules against those 82 rows finds on the order of
**~15** with a plausible field/verb and **~67** with no implementation surface
at all. `Screen` is still just `chrome()` + `menu_items()`. `Chrome` holds
title, size, min size, `bar`, `close_button_only`, zoom — useful, not the
Window/Page/Toolbar/TitleBar ledger.

So: **guard 5 is green, the debt is not cleared.** The stage text oversells
this section. Either implement the rows, DROP them with reasons, or change the
guard to require a verb (or an explicit per-row exception).

### 2. `Renderer.remove` is never called — content replace can orphan native views

**Severity: high — first real backend**

The seam declares `remove(ctx, host, child)`. **No call site** uses it
(only the fake renderer's no-op `fr_remove`).

`set_content` rebuilds by:

1. `evict_presented`
2. `remove_child` on every flex child (facet tree)
3. `mount_walk` on the new tree into `nearest_host`

Dropping a node runs `view_release` (M1). It does **not** tell the native
parent to detach the old view. On toolkits where the superview retains
children, you free facet's reference and leave a live subview in the host.
The new mount then `insert`s at slot 0 into the same host — ordering and
ownership diverge from the facet tree.

There is also **no test** that runs `set_content` with a renderer installed;
the composition test is headless (`uninstall()` first). The fake would not
catch a missing `remove` even if the path were exercised, unless the test
asserted `FR_REMOVE` counts.

### 3. `run_job` / `wait_for_jobs` leave a main-thread use-after-free window

**Severity: high — lifetime**

In `run_job_flight`:

1. `run` on the worker  
2. **`jobs_inflight_sub` immediately**  
3. box the flight and `run_on_main(finish_job_flight, …)`  
4. later, on main: `apply(job_addr)` and `then(ctx)`

`wait_for_jobs` only waits for the counter. It can return while
`finish_job_flight` is still queued. Teardown that waits then drops the job
(or its owner) races the queued `apply`.

Comments claim the runtime calls `wait_for_jobs` before dropping screens.
**There is no production call** in the tree — only the services tests. Even
after such a call is added, the early decrement keeps the race.

The early-decrement rationale (avoid deadlock waiting for main while main is
blocked in `wait_for_jobs`) is real; the fix has to be a second phase
("apply still pending") or ownership that outlives both worker and queue,
not a comment.

### 4. `switch_to` only flips lifecycle on the direct child

**Severity: medium–high**

Parking uses `Display::None` (good — cursors and native state stay). But
attach/detach and `is_attached` are updated **only on the outlet's direct
child**, not a walk of that subtree.

Effects:

- Descendants of a parked pane stay `is_attached == true`
- Nested `on_detach` / `on_attach` never fire on the swap
- A child that was `Display::Grid` is restored as `Display::Flex`

Mount/unmount walks the full tree; `switch_to` does not. For multi-level
panes that is a behavioral lie relative to "lifecycle fires on the swap."

### 5. `agent.cplus` is not ported

**Severity: medium — listed deliverable**

Stage order item 9: "The agent surface. `agent.cplus`."  
There is no `vendor/facet/src/agent.cplus`. The old module still lives under
`vendor/facet.old/src/agent.cplus`. Runtime has `agent_mcp`, attach/serve
hook slots, and comments that nothing registers them. Agent attach/serve
from the App path is a no-op until that module (or equivalent) returns.

### 6. Status line overclaims the "12 deferred rows as fn+ctx pairs"

**Severity: low–medium — documentation**

What landed:

- Shared: `on_focus`, `on_blur`, `on_attach`, `on_detach` as **fn+ctx** on
  every node (good)
- Continuous: `observe_scrolled` (on the list/scroll/items surfaces),
  `observe_drag_interaction`, `observe_move_hover_interaction`,
  `observe_swipe_changing` as **fn+ctx** (good)
- **`observe_size`**: backend registration returning
  `services::Cancellable` — recorded in `DEFERRED_SHARED` with that reason

So 11 rows are fn+ctx; size is intentionally a cancellable subscription.
Rejecting bus-coupled `events::Subscription` was the right call. The stage
status banner should not say all twelve are fn+ctx.

Also: the stage text says `observe_scrolled ×4`; the ledger has **three**
Scrolled ADOPT rows (ListView, ScrollView, ItemsView). Four *controls* may
expose the pair via embedding; the ledger count is three.

### 7. `Cancellable::pending()` cannot observe "already fired"

**Severity: medium — API contract**

Comments and `pending()` claim false after cancel **and after fire**.
Cancellation clears `_id` on the handle. Firing a one-shot timer has no
back-pointer to the caller's `Cancellable`, so `_id` stays nonzero until
`cancel`/`drop`. After a successful `after` callback, `pending()` remains
true. Callers that branch on "still pending" will be wrong.

### 8. Backends are not on this seam yet

**Severity: medium — integration, not Stage 3 core**

- `vendor/facet_appkit` is an empty scaffold  
- `vendor/facet_gtk` still builds the **old** per-control `facet::Renderer`
  shape and will not type-check against today's `mount::Renderer`  
- `runtime.cplus` is correctly a loud no-backend facade  

Stage 3 said backends do not come back here. Fair. The seam is only proven
by the fake renderer. Worth stating so nobody thinks GTK/AppKit already
walk this tree.

### 9. `set_content` always inserts the new subtree at native slot 0

**Severity: medium (when the outlet has no view of its own)**

When the outlet is a pure-layout node, `nearest_host` walks up to an
ancestor view and `mount_walk(..., slot: 0)` inserts there. Sibling native
children of that host are ignored; slot 0 may already be occupied. Correct
only when the outlet **is** the host (keyed/backed) or is the host's sole
content. Needs either "replace children of this host that belong to this
outlet" or always backing outlets that accept `set_content`.

### 10. Small consistency nits

- `application.cplus` still comments **`AppState`** while the type is
  `Application` (handoff naming rule: no `State` words). Comments only, but
  they reintroduce the word the redesign removed.
- `Cplus.toml` package blurb still says "Stage 2 slice: `button` only."
- Stage 3 gate blurb: "four guards" → five.
- Handoff top says Stage 3 DONE with green gates; a lower section still
  says "tier symbols EXIST — the mount seam does not." Stale; mount exists.

---

## Claim-by-claim (stage status banner)

| Claim | Assessment |
|---|---|
| Tier symbols (six modules) | **Mostly** — modules exist; agent missing; count is really 7–8 |
| Mount seam, M1–M7 answered | **Yes** — code + tests; M7 is interim (newest window) |
| Renames Job / run_job / run_on_worker | **Yes** |
| Chrome `bar: Bar`, set_content | **Yes** |
| switch_to over Display::None | **Yes** (lifecycle depth incomplete — finding 4) |
| 12 deferred rows as fn+ctx | **Partial** — observe_size is Cancellable |
| FontWeight + is_italic | **Yes** |
| Guard 5 | **Yes** (type ownership only — finding 1) |
| Key band | **Yes** |
| find(key, within:) over mounted_root | **Yes** |
| facet suite 430 plain/asan/release | **Yes**, with `./target/release/cpc` |
| Ledger debt (82 rows) cleared | **No** — owned on paper, not implemented |
| agent.cplus | **No** |

---

## What I would do next (priority)

1. **Decide the 82-row story** — implement the iris-needed subset, DROP the
   rest with reasons, or strengthen guard 5 so "owner module" means "verb
   present." Right now green guard + empty surface is the worst of both.
2. **Wire `Renderer.remove` into `set_content` (and any other rebuild path)**
   and add a fake-renderer test that asserts remove + insert counts on
   replace.
3. **Fix job lifetime** — do not treat "worker finished `run`" as "safe to
   free the job"; cover apply/then or transfer ownership.
4. **Deepen `switch_to`** — walk attached flags and handlers for the whole
   parked/restored subtree; preserve prior `Display` when restoring.
5. **Port or explicitly defer `agent.cplus`** — if deferred, say so on the
   stage page; runtime hooks are already waiting.
6. **Doc pass** — status banner (12 rows, six modules, ledger cleared),
   gate "five guards", handoff stale paragraph, `AppState` comments,
   Renderer shape vs stage draft.
7. Keep reviewing with **`./target/release/cpc` only**.

---

## Review boundary

- Read-only with respect to product code. Generators were run; outputs were
  byte-idempotent (no facet source churn from this review).
- Pre-existing worktree dirt (docs, `vendor/events`, `vendor/agent_appkit`,
  untracked `tools/gen_icons.py`, etc.) was left alone.
- This review does not implement fixes.
