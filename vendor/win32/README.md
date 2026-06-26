# win32

C+ bindings for the native **Win32 GUI API** — the Windows counterpart to
`vendor/appkit` (macOS) and `vendor/gtk` (Linux), organized the same way.

Unlike GTK, this needs **nothing installed**: `user32`, `gdi32`, `comctl32` and
`comdlg32` ship with every Windows install and sit on the linker's default
search path, so a consuming app builds and runs out of the box on
`x86_64-pc-windows-msvc` — no MSYS2, no pkg-config, no toolkit to fetch.

Like the GTK bindings (and unlike Cocoa's `objc_msgSend`), these are direct
`extern fn` calls to the system DLLs — thin C+ structs over opaque
`HWND`/`HMENU`/`HDC` handles. The **ANSI** (`*A`) entry points are used so a
plain NUL-terminated C+ string (`#str_ptr("...\0")`) is a valid `LPCSTR` with no
UTF-16 conversion.

## Building

```
cpc build      # links user32 + gdi32 + comctl32 + comdlg32 (default SDK paths)
```

## Usage

```toml
[dependencies]
win32 = "*"
```

Import the umbrella facade for the flat type surface, or the narrower modules:

```cplus
import "win32/win32" as win32;        // win32::Window, win32::Button, …
import "win32/controls" as controls;  // or narrower modules directly
import "win32/menu" as menu;
import "win32/graphics" as gfx;
```

```cplus
import "win32/win32" as win32;
import "win32/controls" as controls;

fn on_click(sender: *u8, _user: *u8) {
    // handle the click; reach app state through `user` or a global.
    return;
}

fn main() -> i32 {
    let win: win32::Window = win32::Window::new(#str_ptr("C+ on Win32\0"),
                                                420 as i32, 300 as i32);
    let _lbl: controls::Label = controls::Label::new(win.raw(),
        #str_ptr("Hello from C+ via native Win32\0"), 20 as i32, 16 as i32, 380 as i32, 22 as i32);
    let btn: controls::Button = controls::Button::new(win.raw(),
        #str_ptr("Press me\0"), 20 as i32, 48 as i32, 120 as i32, 30 as i32);
    btn.on_click(on_click, 0 as *u8);
    win.show();
    return win32::run();        // pump messages until the window is closed
}
```

## Modules

- `core`: the engine — the shared window class + the single routing
  window-procedure, the per-window **dispatch table** (turns WM_COMMAND /
  WM_PAINT / WM_CLOSE into calls to plain C+ `fn` handlers, the
  `g_signal_connect` analogue), and the `run()` message loop. Apps rarely touch
  it directly.
- `window`: `Window` — `new`, `show`, `set_title`, `set_menu`, `redraw`,
  `on_close`, `on_paint`, `close_after`, `raw`.
- `controls`: `Button`, `Label`, `Edit` (+ `new_multiline`), `CheckBox`,
  `RadioButton`, `ComboBox`, `ListBox`, `ProgressBar`, `TrackBar` (slider),
  `GroupBox`. Interactive controls expose an `on_*` handler registrar
  (`on_click` / `on_change` / `on_toggle` / `on_select`) plus the relevant
  getters/setters (`set_text`, `is_checked`/`set_checked`, `add_item`/
  `selected_index`/`select(at:)`, `set_range`/`set_value`, …).
  `ComboBox::selected_index` / `ListBox::selected_index` return
  `Option[i32]` — `None` when nothing is selected (the Win32 `CB_ERR`/`LB_ERR`
  −1 sentinel is mapped to absence, never surfaced).
- `menu`: `Menu` — a menu bar (`new`) or submenu (`new_popup`), `add_item`
  (bound to a window's dispatch table), `add_submenu`, `add_separator`. Attach
  with `window.set_menu(bar.raw())`.
- `dialogs`: `message_box` / `message_box_ex` (the AlertDialog analogue, with
  `mb_*` button/icon sets and `id_*` results) and `open_file` / `save_file`
  (the FileDialog analogue, comdlg32).
- `graphics`: `Painter` (wrap the `on_paint` HDC) — `text`, `rectangle`,
  `ellipse`, `line`, `fill_rect`, `use_pen`, text color — plus a `Color`
  (COLORREF) value built with `Color::rgb(r, g, b)`.

## Events & callbacks

Win32 has no per-control signal objects: the parent window procedure receives
everything. This binding hides that — each control is created with a unique
command id, and `control.on_click(handler, user)` registers `handler` in the
parent window's dispatch table under that id. When the control fires a
WM_COMMAND, the routing window-procedure looks the id up and calls your `fn`.
Handlers are plain closure-free C+ functions of shape `fn(sender_hwnd, user)`;
thread app state through the `user` pointer (or a global). Paint handlers are
`fn(window_hwnd, hdc)` — wrap the HDC with `gfx::Painter::from_hdc(hdc)`.

## Ownership

`HWND`/`HMENU`/`HDC` are OS-owned opaque handles, so the wrapper structs hold
them `opaque` with **no `drop`** (a window is torn down by closing it —
WM_CLOSE → DestroyWindow — or at process exit; child controls are owned by
their parent). This is the same conservative default as appkit's child widgets
and gtk's floating-ref widgets. The per-window dispatch table is heap-allocated
and lives for the window's lifetime.

## Naming guideline

This binding follows `naming_guideline.md` to the extent a thin, ~1000-line
Win32 FFI layer should: raw OS handles are private (`_hwnd` / `_parent` /
`_hmenu` / `_hdc`, the COLORREF behind `Color::_value`), absence is a value
(`selected_index -> Option[i32]`), and selection setters read as a phrase
(`select(at:)`). A full Swift-style re-skin of the surface is **deferred** — the
same precedent as `vendor/appkit`. Specifically not done, on purpose:

- **C-string parameters stay `*u8`.** The public `text: *u8` / NUL-terminated
  `#str_ptr("...\0")` contract is the `LPCSTR` the ANSI (`*A`) entry points
  expect; bridging `Text`/`str` through would mean a UTF-8→UTF-16 layer and a
  buffer-ownership story that this thin binding deliberately doesn't carry yet.
- **`x, y, w, h` geometry stays positional.** Labeling every control
  constructor (`new(at:, size:)`) is a surface-wide re-skin, not a cheap win.
- **The `mb_*` / `ofn_*` flag sets stay raw `u32` OR-able constants.** Mapping
  the MessageBox button/icon and OPENFILENAME flag families onto labeled
  defaulted params is a redesign of the dialog surface, deferred.
- **The `#[repr(C)]` FFI-mirror structs (`WndClassExA`, `Dispatch`,
  `CmdEntry`, `OPENFILENAMEA` offsets) keep their header-matching field names**
  — they mirror Win32 layout for auditability and are internal, never public.

## Status & roadmap

Covered: top-level windows, the ten control classes above (incl. the comctl32
common controls), menu bars, message + file dialogs, and GDI painting — all
validated by `cpc/tests/e2e.rs` (`win32_window_opens_and_message_loop_runs`,
`win32_command_dispatch_round_trip`). Natural next layers: keyboard/mouse window
events (WM_KEYDOWN / WM_*BUTTON*), WM_HSCROLL routing for `TrackBar` change
callbacks, finer WM_COMMAND notification-code filtering (EN_CHANGE vs the rest),
and a list/table view over the ListView common control.
