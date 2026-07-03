# hello_win32

The Windows counterpart to [`hello_appkit`](../hello_appkit): a native Win32
window with a label and a **Close** button, built on the [`vendor/win32`](../../../../vendor/win32)
bindings. Nothing to install — `user32`/`gdi32` ship with every Windows and sit
on the linker's default search path.

## Build & run

```
cpc build
./target/debug/hello_win32          # (hello_win32.exe on a normal checkout)
```

The window pumps messages until you click **Close** (which posts `WM_CLOSE`).

## Modern look

Two things separate a bare Win32 app from a modern-looking one:

- **Font** — the controls are given the **Segoe UI** font in `src/main.cplus`
  via `WM_SETFONT`; without it they fall back to the blocky bitmap `SYSTEM_FONT`.
- **Theming** — for the rounded, hover-highlighting ComCtl32 **v6** button
  (instead of the flat gray v5 one), copy `hello_win32.exe.manifest` next to the
  built executable. Windows only enables v6 when the app declares a dependency
  on it via that manifest; `InitCommonControlsEx` (which `win32` already calls)
  is necessary but not sufficient.
