# A runtime::App on WinUI

Two windows of one `runtime::App`, run on `facet_winui` through the facade.
The source is ordinary facade code; `winui::select()` in `main.cplus`
(`import "facet_runtime/winui"`) is the only WinUI-specific line, and the
manifest lists `facet_winui`, `winui` and `winrt` because of it.

- **main** is `Bar::Blended` and draws `window_buttons()` in its header, so
  WinUI removes the system caption and those three are the only buttons; the
  header is `.window_drag()`. 640x420 points, minimum 480x320.
- **settings** keeps the native (dark) caption, 420x280, with no minimise or
  maximise button. `Open settings` opens it with
  `runtime::app().open_window("settings")`; `Done` closes it with
  `runtime::window_close(sender)` and the main window keeps running.
- `Show alert`, `Prompt` and `Choose` call `runtime::alert`, `prompt` and
  `choose`. Each is a keyed facet sheet over the window (`alert:title`,
  `alert:primary`, `prompt:value`, `choose:opt:2`, ...), reachable by UI
  Automation and the agent; the answer is shown in `home:answer`.

Closing the last window ends `app.run`, and the process exits 0.

```powershell
# From the repository root:
& examples/facet_runtime_winui/build.ps1
& examples/facet_runtime_winui/out/runtime/facet_runtime_winui.exe
```

Prerequisites and the `-Packages` cache override are the same as the
[standalone native sample](../winui_standalone/README.md); the build script
stages the Windows App SDK runtime into `out/runtime` and embeds the
activation manifest.
