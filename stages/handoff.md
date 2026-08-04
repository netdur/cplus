# Handoff — facet, next session

Stage 4 is **under way**. Item 1 (the window shell) is DONE: facet_appkit
is a real backend, a keyed tree reaches the screen, and facet's own suite
compiles it. Item 2 (the per-kind bodies) has five of 42 and the shape for
the rest. Items 3–7 are open.

Read `INTENT.md` first, then `stages/4.md` — its "Inspection notes" section
records every decision taken and every question still open, so none of it
is re-derived. `vendor/facet_appkit/MANIFEST.md` is the "AppKit cannot"
record INTENT requires; it also separates *cannot* from *not yet reached*.

## Start here: Stage 4 item 2 — the remaining 37 kinds

`vendor/facet_appkit/src/controls.cplus` holds the bodies, one pair per
kind, and `views::create_for_kind` / `apply_for_kind` are the dispatch. The
pattern is fixed and worth following exactly:

- ONE body per kind. `create` calls the same `apply_<kind>` with an
  all-ones dirty word; `apply` calls it with the real one. A separate
  create-path is how a control ends up right on mount and wrong on its
  first update.
- Read props through `props_of(n, kind)`, which checks `Data.kind` and
  answers 0 on a mismatch. Never trust the cast.
- The per-control dirty bits live in the GENERATED control modules
  (`label::P_TEXT` is not `text_field::P_TEXT`), so import each one.
- Kind-independent work belongs in `paint.cplus`, not repeated per kind.
- A kind with no body still gets a backing view AND a one-time stderr line.
  Silence is the failure INTENT is written against.

`vendor/facet_appkit/MANIFEST.md` is the other half of the job: every
"AppKit cannot" is written there, kept apart from "not yet reached". Three
families are already recorded as cannot (refreshable, swipeable/swipe_item,
and the control tint colours); three more are blocked on a DECISION rather
than effort and are named on `stages/4.md` — web/hybrid_web (needs a WebKit
binding package), date_picker/time_picker (needs a Date↔NSDate bridge, and
which calendar is a decision), and tabs (NSTabView's item model does not fit
a tree whose children mount as subviews).

`context_menu` / `context_menu_item` / `toolbar_item` are now IMPLEMENTABLE
and were not before — they carried no text and no action until the
MenuItem-base fix landed. They are the natural next kinds. They are also
non-view kinds: an NSMenu is not an NSView, so the shape is a zero-size
placeholder view carrying the built NSMenu, with `views::insert` setting it
on the HOST rather than adding a subview.

Then items 3–7 on `stages/4.md`. Item 3 (scheduler) is largely already in
`scheduler.cplus` — run_on_main, after, observe_size and the
CFRunLoopObserver tick are built; what remains is jobs/teardown proving.

---

## Where things stand

Branch `facet-maui-regen`.

```
Stage 1  the ledger          DONE   873 rows, ADOPT 490 / DROP 383
Stage 2  the contract        DONE   38 generated + 4 hand-written controls, 362 verbs
Stage 3  platform-free body  DONE   2026-08-04, reviewed. Tier modules, mount seam
                                    (M1-M7), renames, composition, FontWeight,
                                    key band, DOM find, guards 5 + 5b.
Stage 4  facet_appkit        UNDER WAY
         item 1 window shell DONE   2026-08-04 — a keyed tree on screen
         item 2 the 42 kinds 21/42  label, button, text_field, image, scroll, box,
                                    checkbox, radio, toggle, slider, progress,
                                    spinner, stepper, icon_button, symbol,
                                    text_area, search_field, popup, bordered,
                                    split, page_dots
         items 3-7                  scheduler proving, input, the 58 rows,
                                    the agent surface, examples
Stage 5  docs + provenance
```

Gates, all green:

```
python3 tools/maui_map.py                  # fails on an unbucketed row
python3 tools/gen_contract.py              # SIX guards incl. 5b tier dispositions;
                                           # byte-idempotent; prints the tier tally
python3 tools/gen_icons.py                 # the icon table, from the font
cd vendor/facet && cpc test                # 475  (was 433: +C_LAYOUT, the
                   cpc test --asan         # 475   structural verbs, the window
                   cpc test --release      # 475   accessors, the macOS facade,
                                           #       the MenuItem base fix)
cd vendor/facet_appkit && cpc test         # 380
                          cpc test --asan  # 380
                          cpc test --release # 380
       leaks -atExit -- ./target/debug/facet_appkit_tests   # 0 leaks / 0 bytes
cd vendor/flex_layout && cpc test          # 280
cd vendor/stdlib      && cpc test          # 290
```

**facet's suite now compiles facet_appkit** — `test_main.cplus` imports
`./runtime`, which on macOS resolves to `runtime_macos.cplus`, which imports
the backend. A backend that does not build turns facet red. That is how the
pre-regen tree had it too.

**Use `./target/release/cpc` ONLY, and rebuild it first** (`cargo build
--release`). Homebrew's `cpc 0.0.26` predates the builder-DSL work and
fails to parse `@ui` blocks — it reports a red suite that is not red.

## Start here: Stage 4, item 1 — the window shell

Port the pre-regen macOS facade onto the new seam. The copy-paste rule
applies: unchanged parts are copied, only the seams move.

```
git show eb5b1b7:vendor/facet/src/runtime_macos.cplus   # 748 lines, the reference
git show eb5b1b7:vendor/facet/src/agent.cplus           # for work item 6
```

`runtime_macos.cplus` shadows `runtime.cplus` by filename (the resolver's
platform override). Adapt: Chrome's six booleans are now `bar: Bar` +
`close_button_only`; the App loop wires `app::make_current` /
`open_window` / `close_window`; the backend install is the contract below,
not loose `set_*_fn` calls. Exit criterion: an empty keyed column on
screen. Then the six-kind create/apply skeleton (stages/4.md item 2).

---

## The backend-install contract (what facet_appkit fills)

Five structs, two slots, nothing else. Zero fields keep the portable
no-op. A hook family appearing outside these is drift by definition.

```
mount::install(Renderer { ctx, create, view_release, apply, insert, remove, schedule })
services::install_scheduler(Scheduler { run_on_main, after, cancel_after,
                                        observe_size, cancel_observe_size })
gestures::install_key_reader(KeyReader { code, chars, modifiers, named })
component::install_sender_readers(SenderReaders { raise, key_of, item_of,
                                                  dropped_text, drag_targeted })
runtime::install_agent(AgentHooks { attach_window, serve_once })
theme::set_is_dark_fn            # the system appearance read — the ONE single slot
theme::set_theme_changed_fn      # writes the running app's record
```

- `mount::install` also arms M5 (`touch` → schedule) and M6 (the UI
  thread; every tree write asserts it).
- Per-kind create/apply dispatch lives INSIDE the backend on `Data.kind`
  — facet never grows a verb table (Stage 2's rule).
- The fake renderer + fake scheduler in the suite are the conformance
  tests: fill the same structs, inherit the proven pipeline — mount,
  sync, batching, lifecycle, set_content (with native removal through
  `Renderer.remove`), switch_to parking, job settling.
- Teardown contract: drain the loop until `services::jobs_settled()`
  (both counters — workers AND queued applies) before dropping job
  owners. `wait_for_jobs()` alone covers only the worker half.

Reality: `vendor/facet_appkit` is an empty scaffold; `vendor/facet_gtk`
still speaks the OLD per-kind renderer and will not type-check against
this seam (out of scope for Stage 4). The seam is proven headless only.

## Stage 4 decisions — already resolved with the user

1. **Sync tick**: AppKit's own run loop, via a CFRunLoopObserver at
   before-waiting — the flush point Core Animation itself commits on.
2. **The 58 deferred tier rows**: implement everything AppKit can do
   during the sweep; the mobile-only handful (soft-input policy, nav-bar
   back button) are recorded "AppKit cannot (mobile concept)" in the
   backend manifest — INTENT's doctrine, same as an UNSUPPORTED control
   verb. Guard 5b's printed tally is the tracker (today: 24 carried, 58
   deferred).
3. **`find` is the DOM rule** (already implemented): bare `find(key)`
   searches the running app's windows in OPEN order, first match wins;
   `within:` scopes or disambiguates. `mounted_root()` remains for
   facades that want "the active window" — find does not use it.

---

## The architecture, so you do not redesign it

Settled by evidence; several were wrong once already.

- **facet's tree IS flex_layout's tree.** `type Node = flex::Node`; facet's
  per-node `Data` rides flex's owned attachment slot; geometry is read
  from flex (`frame()`). facet holds zero layout code.
- **A control is a constructor plus a typed cursor.** `button(title:,
  key:, …) -> Node` builds; `button::find(key)` (DOM rule) or
  `button::from(node)` addresses. No generic `Handle` — a wrong verb is
  E0324, a wrong kind is `None`.
- **Ownership: process → app → window → node, in MAUI's words.**
  `application.cplus` holds `Application` (theme, `Navigation`,
  `Window`s), reached ONLY through `app::current()` (=
  `Application.Current`; `Application.host` points back at the embedding
  `runtime::App`). One exe may contain several apps; one runs at a time;
  an app has many windows shown at once; a node owns its presentation in
  `Data` (dies with it — there is no registry).
- **The statics rule** (user-directed): module statics are legal ONLY for
  what the whole process shares. Today that is exactly: the `RUNNING`
  door + `DEFAULT` (the app-less record), the five install structs,
  `IS_DARK_FN`, core's `SYNC_REQUEST_FN`/`MAIN_THREAD_ID` (internal
  mount wiring), `JOBS_IN_FLIGHT`/`APPLIES_PENDING`, and flex's
  `REMOVAL_COUNT`. Test fixtures are exempt (fn-ptrs cannot capture; the
  language lacks a test-only strip). Anything app- or window-scoped in a
  static is drift.
- **The naming rule** (user-directed): NO `State`-suffixed types. An LLM
  reading `AppState` beside `Component` reasons its way to React, which
  facet is not. When a concept needs a name, MAUI's ledger is the
  vocabulary authority (`Application`, `Window`, `Navigation`).
- **facet is NOT reactive.** `build` runs once; you `find` and mutate in
  place; a setter writes one field and sets one dirty bit. The one-way
  rule holds everywhere (the key band's bool return killed
  `consume_key`; constructor-only for control props — no build-time
  chaining; layout chains are flex's).
- **The `@` builder DSL is the authoring surface.** `import
  "facet/elements" as ui;` then `@ui { vstack(key: "body") {
  button("Save", key: "save") } }` — containers take constructor args
  (Builder first), `elements.cplus` is the generated namespace. Props in
  the constructor, layout via modifiers, children by nesting.
- **Composition**: `set_content(outlet, component)` fills a slot
  (rebuilds; outlet should be KEYED so the host is its own view);
  `switch_to(outlet, key)` parks siblings under `Display::None`
  (full-subtree lifecycle, prior Display restored); `nav::go/push/pop/
  quit` move whole screens by route.

## Invariants — do not break these

- **The generator owns its files whole**: never hand-edit
  `vendor/facet/src/{vocabulary,props,elements}.cplus` or the 38 control
  modules. `facet.cplus`, `test_main.cplus`, the tier modules and
  `mount.cplus` are hand-written. Regeneration is byte-idempotent.
- **The MAP is the naming authority** (`maui_map.RENAME` / `OVERLAY`);
  the generator only transcribes. Six guards; nothing silent.
- **`test_main.cplus` imports every module** — importing is what compiles
  a module into the gate.
- **Keys are owned `Text`** on Data (`key` = address, `id` = identity —
  do not swap them); a `str` field never outlives its bytes (the borrow
  checker now catches the erasure route too).
- **An agent has no hands**: a gesture-only affordance gets a click path
  in the UI, never a new agent verb.
- **Examples are outside test coverage** (gitignored): build them after
  interface changes; feel items end with "run it and try".
- Test discipline: full unit + negative coverage per module; commit each
  increment; fix-don't-park.

## Known-broken, pre-existing

- `iris` and the old examples import `facet/runtime`'s OLD API — they
  migrate AFTER the backend lands (not Stage 4).
- `vendor/terminal` and `vendor/facet_gtk` speak pre-regen facet.
- `examples/hello_facet` vendors an old-facet snapshot (reference only).

## The session trail (2026-08-04, for review)

```
2faa3a9  Stage 3 tier ported (copy-paste, three reseams)
43a8c84  the spec's renames applied (Job/run_job, Chrome bar)
080a7d3  the mount seam — Renderer, walks, M1-M7
435b674  Stage 3 completed to the page (12 rows, set_content/switch_to,
         FontWeight, guard 5, key band, find(within:))
1d59aab  ownership hierarchy: process → app → window → node
e0b3c98  Application, not AppState — MAUI's words
80fc206  one door + four install vtables
e4dbd5f  three more statics die on inspection
0c4358f  external reviews' findings fixed (five bugs + guard 5b)
bf13cf4  Stage 4 planned (facet_appkit)          [handoff briefly lost here,
4c42704  handoff restored                          nothing else affected]
e580b1b  find is the DOM rule
```

Compiler work this session, already on the branch: DSL.5 container
arguments (`name(args) { }`), the resolver's alias fix for named
arguments, and the borrowck erased-`*u8` closure — all with e2e tests.
