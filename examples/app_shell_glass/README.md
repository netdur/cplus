# App Shell Glass

The former `sidebar_demo`: a macOS app shell using `screen::Bar::Blended`
and `split::PaneRole::Sidebar`. AppKit supplies the native sidebar appearance,
including Liquid Glass on macOS 26. New sits in the sidebar toolbar; Back and
Forward sit above the detail pane.

The sidebar opens sections in the detail slot. Back and Forward navigate its
history and update the selected row. Drag the divider to resize the sidebar
between 180 and 320 points. New is a placeholder action.

From the repository root, using the compiler built from this checkout:

```sh
cd examples/app_shell_glass
ln -s ../../vendor vendor # only if vendor is missing
../../target/release/cpc build
./target/debug/app_shell_glass
```

Compare with [app_shell](../app_shell), which uses the same app structure with
legacy window chrome and an ordinary split pane.
