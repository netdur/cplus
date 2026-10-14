# tasks_agent_loop

A small facet app built to be driven by an agent from outside, and the check that
drives it. It exists for one experiment: can an agent take a behavioural
requirement, build the app, launch it, find its controls, act on them, check what
happened on screen, find a defect, fix it, rebuild, and show the fix — with no
human in the loop?

The app is a task list: a title field and **Add**, a status line, a count, a list
of tasks, a detail screen that opens when a row is tapped, and **Sync**, which
runs in the background and reports when it finishes.

## Run the loop

```
cd examples/tasks_agent_loop
cpc build
./target/debug/tasks_agent_loop &      # prints: facet: agent surface on http://127.0.0.1:<port>/
python3 agent_check.py                 # finds the app by its descriptor
```

`agent_check.py` reads the port from `/tmp/cplus-<uid>/mcp-tasks_agent_loop-<pid>.json`,
checks it reached the right app (`initialize` answers `serverInfo.name`), and then
checks each requirement by reading the result back from the live UI. A tool
answering `allowed` is never taken as evidence on its own. The script's exit status
is the number of failed checks.

## The requirement it checks

1. Adding "Buy milk" adds a row, says "Added: Buy milk", counts "1 task", and
   clears the field.
2. An empty title is refused ("Title required") and nothing is added.
3. A title of only spaces is refused the same way.
4. "  Call Bob  " is stored and reported trimmed, as "Call Bob".
5. Sync is visibly pending ("Syncing…", button disabled), then finishes within
   3 s ("Synced 2 tasks", button enabled again). The script waits on the label,
   not on a sleep.
6. Tapping a row opens the detail screen showing that task; Back returns, and the
   rows are intact.
7. While the detail screen is showing, the list screen's controls cannot be used.
8. `activity` records what the run did, in order.

## What the experiment found (2026-10-14, macOS)

| Run | Passed | What failed |
|---|---|---|
| 1 | 18/27 | requirements 3–4 (the seeded defect), 6 (the row tap), 7 (hidden controls) |
| 2: after fixing agent_appkit | 21/27 | requirements 3–4, 7 |
| 3: after fixing the app | 26/27 | requirement 7 |

The seeded defect was `text::from_str(f.text())` without `.trim()` in `on_add`, so
whitespace-only titles were added as blank tasks. The checks caught it, and the fix
is the trim in `on_add`. Put the untrimmed line back to rerun the loop from the
start.

The loop also found defects in C+ itself:

- **A click on a list row whose content is a `label` never reached `on_item_tapped`
  on macOS.** It answered `allowed` and did nothing. Fixed in agent_appkit
  (bugs/closed/an-agent-click-on-a-label-row-never-taps-it.md).
- **A hidden screen's controls accept agent actions.** With the detail screen
  showing, `set_text` and `click` on the parked list screen ran its handlers off
  screen. Open, and needs a decision
  (bugs/a-hidden-screens-controls-take-agent-actions.md). This is requirement 7,
  which still fails.
- **Over HTTP, `activity` records every action as `unidentified`**, because each
  request is its own connection. Open (bugs/http-activity-never-names-its-client.md).
- **facet's documented async example did not compile.** `spawn_ui(this.refresh())`
  is E0515, and the `attached(this)` it used doesn't exist. The docs in
  `facet/services` now show what works: owned values into the task, or `after`
  with a held `Cancellable` for screen state. This app uses the latter for Sync.
- **`cpc init --kind gui` without `--platform` left an empty `<name>/src/`
  behind.** Fixed.

The agent surface itself worked well. Every control was found by the key the
app gave it, `read_text` supplied the version stamp `set_text` wanted, the
pending state of the async sync was observable, and the detail screen and Back
were reachable by key.

Not covered: other backends. iOS and Android need a running simulator or
emulator, which a person has to start.
