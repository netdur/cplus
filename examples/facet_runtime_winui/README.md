# A runtime::App on WinUI

Two windows of one `runtime::App`, run on `facet_winui` - the Windows backend -
through the facade. The source is ordinary facade code and names no backend;
the manifest lists facet_runtime's Windows closure.

- **main** is `Bar::Blended` and draws `window_buttons()` in its header, so
  WinUI removes the system caption and those three are the only buttons; the
  header is `.window_drag()`. 640x420 points, minimum 480x320.
- **settings** keeps the native (dark) caption, 420x280, with no minimise or
  maximise button. `Open settings` opens it with
  `runtime::app().open_window("settings")`; `Done` closes it with
  `runtime::window_close(sender)` and the main window keeps running. Its
  `Show alert` and `Alert in 1.5 s` put the sheet over Settings, the window
  that asked, even when main is the active window by then.
- `Show alert`, `Prompt` and `Choose` call `runtime::alert`, `prompt` and
  `choose`. Each is a keyed facet sheet over the window (`alert:title`,
  `alert:primary`, `prompt:value`, `choose:opt:2`, ...), reachable by UI
  Automation and the agent; the answer is shown in `home:answer`.

`main.cplus` declares an app menu - File (Settings... Ctrl+,, About, Quit
Ctrl+Q), Edit (the standard actions) and Window (Close Settings, enabled only
while Settings is open) - shown as a MenuBar in every window.

`CPLUS_RUNTIME_WINUI_AGENT=1` serves the agent surface over loopback HTTP
(`facet_agent_winui`); one connection reaches both windows, as `main` and
`settings`.

Settings carries a gesture pad: a plain container with click, double click,
right click, hover, long press, swipe and Escape handlers, focus/blur
handlers, and `Focus pad` / `Blur pad` buttons; `settings:pad-log` says what
it last heard.

Under it, drag and drop: `card:1` drags its key (`can_drag`, logging
`drag start` / `drag end` to `settings:drag-log`), and `settings:drop` takes
text and files (`allow_drop`, `allow_file_drop`). Its `on_drag_over` refuses a
file whose path contains `refuse`; its `on_drop` writes what arrived and where
to `settings:drop-log`.

`Load in background` runs a `services::run_job` whose `run` sleeps on a worker
thread; its result is applied on the UI thread and shown in `home:loaded`.

Closing the last window ends `app.run`, and the process exits 0.

`tools/test_facet_runtime_winui.py` drives this sample from outside. It sets
`CPLUS_RUNTIME_WINUI_PROBE=1`, which makes `src/probe.cplus` open Settings at
launch and refuse the first `on_should_quit`, and then posts `WM_CLOSE`s.

```powershell
# From the repository root:
& examples/facet_runtime_winui/build.ps1
& examples/facet_runtime_winui/out/runtime/facet_runtime_winui.exe
```

Prerequisites and the `-Packages` cache override are the same as the
[standalone native sample](../winui_standalone/README.md); the build script
stages the Windows App SDK runtime into `out/runtime` and embeds the
activation manifest.
