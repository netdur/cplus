# appkit_list_detail — master/detail GUI app

The AppKit milestone app (plan.appkit.md §5): a list+detail window that ties the
v0.0.16 binding surface together.

- **List** — an `NSTableView` in a scroll view, driven by a synthesized
  `NSTableViewDataSource` (`data::create_table_data_source`).
- **Detail** — a label updated on selection by a synthesized
  `NSTableViewDelegate` (`data::create_table_delegate` + `TableView::selected_row`).
- **Quit** — a button (`set_on_click`) and an app menu item, plus an app
  delegate that quits when the window closes.

The callbacks are closure-free: the data-source / selection functions reach the
live table and detail field through `static mut` handles.

## Build & run

```sh
cpc build
./target/debug/appkit_list_detail
```

A window opens with a list of fruit; selecting a row updates the detail label.
Quit from the button or the menu (⌘Q).

## Ownership

Each widget wrapper is held in a local that outlives the `addSubview:` that
installs it, so the parent retains the view before the wrapper's `drop` releases
its `+1` (the "+1 normal form", plan.appkit.md §2). Don't write
`Widget::new().obj` inline — the temporary wrapper would drop immediately and
dangle the pointer.
