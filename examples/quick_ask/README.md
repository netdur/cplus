# quick_ask

A menu bar application with a summoned prompt bar. It exists to show three
facet modules working together:

- `facet/status_item`: the bolt icon in the menu bar, its menu, and the count
  beside it.
- `facet/hotkey`: ⌃⌥Space, delivered by the system whichever application is in
  front.
- `screen::Chrome(panel: true, floating: true)`: the bar takes the keyboard
  without this application's other windows coming forward.

## Run

```
cpc build && ./target/debug/quick_ask
```

## Try

1. Switch to another application and press ⌃⌥Space. The bar appears on the
   display the pointer is on.
2. Type something and press Return. The bar goes away, the main window shows
   what was asked, and the count beside the menu bar icon goes up.
3. Summon it again and press Escape, press the chord again, or click another
   window. Each puts it away.
4. Close the main window. The icon stays and the chord still works. "Show
   Window" in the icon's menu brings the window back.
5. Quit from the icon's menu.

macOS only for now. The other backends answer `Unsupported` for the chord and
give a dead status item.
