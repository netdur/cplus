# A runtime::App on the Win32 opt-out

One window of a `runtime::App` on the deprecated Win32 backend. WinUI is the
Windows default; `main.cplus` opts out with `facet_runtime/win32`'s `select()`,
and the manifest adds `facet_win32` to facet_runtime's Windows closure for it.
It exists to show and test the opt-out until facet_win32 is removed
(netdu/cplus#20).

```powershell
cd examples/facet_runtime_win32
cpc build
target/debug/facet_runtime_win32.exe
```

`tools/test_facet_runtime_winui.py` (`run_win32_opt_out`) closes it from
outside: `on_should_quit` refuses the first close and allows the second.
