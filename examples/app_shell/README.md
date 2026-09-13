# App Shell

A macOS app shell with a Finder-style legacy layout. `screen::Bar::Blended`
and `SafeArea::None` on the screen root let the ordinary split fill the entire
window. The sidebar header contains
native window controls; the detail header contains Back, Forward, the current
section title, flexible space, and New. Action buttons show icons only, with
tooltips and accessibility labels. Both headers support window dragging.
The toolbar shares the detail pane's background. The panes use opaque
backgrounds and `split::PaneRole::Content`.

The sidebar opens sections in the detail slot. Back and Forward navigate its
history and update the selected row. Drag the divider to resize the sidebar
between 180 and 320 points. New is a placeholder action.

From the repository root, using the compiler built from this checkout:

```sh
cd examples/app_shell
ln -s ../../vendor vendor # only if vendor is missing
../../target/release/cpc build
./target/debug/app_shell
```

Compare with [app_shell_glass](../app_shell_glass), which uses the same app
structure with blended chrome and the platform's native glass sidebar.
