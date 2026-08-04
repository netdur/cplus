# Codex review — Stage 3

Reviewed: **2026-08-04**  
Target: the current on-disk implementation described by `stages/3.md`  
Method: read-only source audit, generator/idempotence checks in a temporary
copy, and the documented facet test matrix. No source code was changed during
the review.

## Verdict

Stage 3 is **partially implemented and does not yet meet its plan**.

Most of the named API surface exists: the platform-free tier modules, the
mount seam and its M1-M7 decisions, the requested renames, `Chrome.bar`,
`FontWeight` plus the italic axis, keyboard readers, typed global `find`,
themes, and guard 5. However, important parts are only symbol-level or
structural implementations. The 82 tier ledger rows were not implemented,
the test gate is red, the agent surface is absent, and several mounted-tree and
background-job paths have correctness problems.

## Findings

### 1. The 82 tier ledger rows were not cleared

Severity: **high — direct plan failure**

Guard 5 does not verify that tier verbs are implemented. `TIER_TYPES` assigns
each non-control type to a module (`tools/gen_contract.py:1597-1607`), and the
guard accepts every ADOPT row as soon as its type appears in that table
(`tools/gen_contract.py:1632-1646`). It never searches or otherwise proves
that the row's facet verb exists in the assigned module.

This lets all 82 rows pass while much of their surface is absent. Examples
from the generated ledger include:

- Window: `set_is_maximizable`, `display_density`,
  `on_display_density_changed`, `observe_window_size`.
- Application: `set_accent_color`, `set_main_page`, `requested_theme`,
  `on_requested_theme_changed`.
- Page: `set_background_image_source`, `set_container_area`, `set_is_busy`,
  `on_layout_changed`, and the navigation observers.
- ContentPage: `set_hide_soft_input_on_tapped` and `set_safe_area`.
- Toolbar: `set_back_button_enabled`, `set_bar_background`,
  `set_dynamic_overflow_enabled`, and `set_title_view`.
- TitleBar: `set_leading_content`, `set_subtitle`, and
  `passthrough_elements`.

None of those names exists in the Stage 3 implementation modules. The current
`Screen` interface has only `chrome()` and `menu_items()`
(`vendor/facet/src/screen.cplus:273-283`), and `Chrome` holds only title,
width/height, minimum size, bar style, close-button policy, and zoom fields
(`vendor/facet/src/screen.cplus:230-267`). Those are useful tier symbols, but
they do not discharge the ledger rows assigned to the tier.

### 2. `run_job` permits a queued main-thread use-after-free

Severity: **high — lifetime safety**

The worker decrements `JOBS_IN_FLIGHT` immediately after `run` returns, then
boxes and queues `finish_job_flight` on the main thread
(`vendor/facet/src/services.cplus:381-390`). `wait_for_jobs()` only waits for
that counter to reach zero (`services.cplus:342-353`). It can therefore return
while `finish_job_flight` is still queued.

If teardown drops the job owner after the wait, the queued callback later
dereferences `job_addr` to call `apply` (`services.cplus:394-399`). That is the
same raw address whose lifetime the counter is intended to protect.

The comments say the runtime calls `wait_for_jobs()` before dropping screens,
but there is no production call to it anywhere in the current tree; its only
calls are in the services tests. Even if such a runtime call were added, the
early decrement would leave the queued-apply race intact.

### 3. Mounted content replacement does not remove the old native child

Severity: **high — native tree diverges from facet tree**

`Renderer.remove` is part of the seam (`vendor/facet/src/mount.cplus:59-67`),
but no implementation path calls it. `set_content` evicts the component
record and removes each old flex child directly
(`mount.cplus:586-593`). Dropping those nodes runs `view_release`, but it does
not ask the native parent to remove the view.

On native toolkits where a superview retains its children, releasing facet's
own reference leaves the old view retained by and attached to its superview.
The facet tree contains only the replacement while the native tree may still
display or retain the old content. The unused `remove` hook is the seam meant
to prevent this.

### 4. `switch_to` updates only direct-child lifecycle state

Severity: **medium-high — lifecycle and state preservation**

`switch_to` parks or restores each direct child and changes only that child's
`is_attached` flag (`vendor/facet/src/mount.cplus:690-736`). It does not walk
the child's descendants. Consequently:

- descendants of a parked subtree remain marked attached;
- descendant `on_detach`/`on_attach` handlers do not fire;
- a detached outlet can still cause a child to be marked attached;
- a child originally using `Display::Grid` is restored as `Display::Flex`.

The Display::None parking mechanism preserves node addresses and native state,
but the lifecycle state no longer describes the whole visible subtree.

### 5. The documented test gate does not compile

Severity: **high — release gate blocked**

Using the available `cpc 0.0.26`, all three documented facet commands failed
before running any tests:

```text
cpc test
cpc test --asan
cpc test --release
```

All report the same parser error at
`vendor/facet/src/test_main.cplus:2262`, on the second child in the `@ui`
block:

```text
error[E01XX]: expected `;` or `}`, found identifier
```

This may expose a compiler/DSL syntax regression or a stale test expectation,
but either way the exact gate inherited by Stage 3 is currently red.

### 6. The planned `agent.cplus` surface is absent

Severity: **medium — incomplete deliverable**

There is no `vendor/facet/src/agent.cplus`. The Stage 3 imports in
`vendor/facet/src/test_main.cplus:61-69` compile `nav`, `services`,
`component`, `screen`, `theme`, `runtime`, `mount`, and `application`, but no
agent module.

`runtime.cplus` contains agent attach/serve hook slots, but no current facet
module installs them. The former bridge still exists only at
`vendor/facet.old/src/agent.cplus`.

### 7. Only eleven of the twelve deferred rows are fn+ctx pairs

Severity: **medium — status claim does not match implementation**

The four shared handlers (`on_focus`, `on_blur`, `on_attach`, `on_detach`) and
the seven continuous control events are represented as function/context
pairs. `observe_size`, however, is a separate backend registration returning
`services::Cancellable` (`vendor/facet/src/services.cplus:125-146`). It is not
an emitted fn+ctx pair on each node/control as the Stage 3 status states.

The bus-coupled `events::Subscription` was correctly avoided, but replacing it
with a different subscription handle still differs from the recorded
"12 fn+ctx pairs" decision.

### 8. `Cancellable::pending()` cannot observe a fired callback

Severity: **medium — API contract mismatch**

`Cancellable::pending()` returns whether its handle-local `_id` is nonzero
(`vendor/facet/src/services.cplus:83-116`). Cancellation clears that field,
but firing a backend callback has no reference to the returned Cancellable and
cannot clear it. After a one-shot timer fires, `pending()` therefore remains
true unless the caller explicitly cancels it, contrary to the comment saying
it is false after firing.

### 9. The Renderer shape differs from the recorded plan and available backend

Severity: **medium — integration mismatch**

The plan describes per-control apply hooks plus the shared band. The current
`mount::Renderer` instead exposes one generic `create` and one generic `apply`,
with kind dispatch delegated inside the backend
(`vendor/facet/src/mount.cplus:51-67`). That can be a valid alternative design,
but it is a deliberate departure from the reviewed plan and should be
recorded as such.

The available GTK backend still constructs the older per-kind renderer shape
(`vendor/facet_gtk/src/facet_gtk.cplus:421-458`), while the current AppKit
backend is an empty scaffold. Thus the new seam is presently proven only by
the fake renderer, not by a current native backend.

## Confirmed implementations

The following Stage 3 work is present and substantially matches the plan:

- Six platform-free tier modules plus the later `application.cplus` ownership
  record are present and imported by the facet test root.
- M1: `Data` contains the native `(view, view_release)` ownership pair.
- M2: one renderer is installed process-wide.
- M3: the mount walk creates top-down.
- M4: node lifecycle handlers are queued until the mount walk completes.
- M5: `touch` schedules sync, with `C_FLUSH` closing batches.
- M6: tree mutation checks the installing UI thread.
- M7: mounted roots are app-owned, with bare `find` using the newest window.
- `Service`/`load_service`/`on_worker` were renamed to
  `Job`/`run_job`/`run_on_worker`.
- `Chrome` uses `bar: Bar` with Native, Blended, Hidden, and Custom.
- `present` was replaced by `set_content`.
- Display::None parking exists and avoids flex child removal.
- `FontWeight` is a scale and italic is a separate axis on the ten generated
  font-carrying controls.
- The keyboard gesture band and portable named-key readers exist.
- Per-control `find(key, within:)` defaults through `mounted_root()`.
- Key storage is owned `Text`; tree row identity remains `id`.
- Theme roles are app-scoped and `set_theme` invokes a live repaint hook.
- Guard 5 exists and catches ADOPT types with no declared owner, although it
  needs a row-level implementation check to clear the ledger reliably.

## Verification results

The generators were run in a temporary copy to avoid rewriting the workspace.
All succeeded and were byte-idempotent:

```text
python3 tools/maui_map.py
  873 rows — ADOPT 490, DROP 383

python3 tools/gen_contract.py
  29 enums, 38 controls

python3 tools/gen_icons.py
  4268 icons, 70 names remapped
```

The facet test matrix failed at the shared parse error described above, so no
plain, ASan, or release tests executed.

## Review boundary

The worktree already contained unrelated or user-owned modifications in
documentation, `vendor/events`, `vendor/agent_appkit`, and the untracked
`tools/gen_icons.py`. They were preserved. This review added no implementation
changes and evaluated the current on-disk state.
