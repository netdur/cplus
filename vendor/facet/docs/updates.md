# Keyed-direct updates

facet updates the screen by addressing one element and mutating it in place. A
handler mutates a struct field, then pushes that value to the element that shows
it, found by its `key`. There is no re-render and no diff.

## Keys

Give an element a stable `key` in `build`:

```cplus
label(t.view()).key("count")
button("+1").key("inc")
vstack { ... }.key("list")           // containers can be keyed too
```

Keys are **component-scoped**, not a global namespace: `find(cp, key)` resolves
`key` inside the subtree of the component at address `cp`. (A key is also the
element's `agent_id` / accessibility identifier, shared with the agent surface.)

## `find` and the `Handle`

```cplus
fn find(cp: *u8, key: str) -> Handle
```

`cp` is the component's own address. A handler receives it as its `ctx`, so
`find(ctx, key)` addresses the handler's own component. A miss returns an empty
`Handle` whose mutators no-op — the `getElementById`-null pattern, so a `find`
on a torn-down element is safe.

Under `facet::run`, the window's root mount slot needs a component address the
app can name. Supply one on the `Window` before running it:

```cplus
static WORKSPACE: Workspace = #zero::[Workspace]();

win.component_ctx(#addr_of(WORKSPACE) as *u8);
runtime::run(win);
```

Every `find(cp, key)` and structural verb then resolves against that address,
from any handler. Unset, the runtime keys the slot internally and `find` is not
reachable from app code.

`Handle` is a non-owning, `Copy`, chainable view onto a mounted element:

```cplus
fn found(this) -> bool           // did the key resolve to a live element?
fn view(this) -> *u8             // the native view pointer (the escape hatch)
```

## Leaf mutators

Each sets one element in place and returns the `Handle` (chainable):

| method | effect |
|---|---|
| `set_text(s: str)` | label text, button title, or field value |
| `set_value(v: f64)` | a value control (slider / stepper / progress) |
| `set_on(on: bool)` | a toggle / checkbox / switch |
| `set_hidden(hidden: bool)` | show or hide the element |
| `show()` / `hide()` | the readable pair over `set_hidden` |

```cplus
facet::find(ctx, "count").set_text("count 3");
facet::find(ctx, "vol").set_value(0.5f64);
facet::find(ctx, "flag").set_on(true);
facet::find(ctx, "panel").hide();
```

These four setters are the **portable** update path: they work on every backend
because they surface setters each backend already has.

## Structural verbs

On a **keyed container**, address children in place:

| method | effect |
|---|---|
| `add_child(take Node)` | append a freshly built child; reflow |
| `insert_child(take Node, at: usize)` | insert at index `at` (past the end appends) |
| `replace_child(key: str, take Node)` | swap the child at `key` for a fresh one, same position; returns `bool` |
| `remove_child(key: str)` | remove the direct child at `key`; returns `bool` |
| `set_child(take Node)` | clear the container and mount exactly one fresh child |

```cplus
// append a row
facet::find(ctx, "list").add_child(facet::label("row-c").key("c"));

// insert at the top
facet::find(ctx, "list").insert_child(facet::label("row-0").key("z"), 0 as usize);

// swap one row for another
let ok: bool = facet::find(ctx, "list").replace_child("c", facet::label("row-c2").key("c2"));

// remove one row
let gone: bool = facet::find(ctx, "list").remove_child("a");

// single-slot swap: make the container hold exactly one thing
facet::find(ctx, "outlet").set_child(some_screen_node);
```

`add_child` / `insert_child` / `set_child` mount a **fresh** `Node` and take
ownership of it. To move a **retained, live** subtree in and out (keeping its
views and state), use the component lifecycle instead — see
[lifecycle.md](lifecycle.md).

## The canonical handler

```cplus
fn increment(sender: *u8, ctx: *u8) {
    COUNTER.n = COUNTER.n + 1;                                  // 1. state
    let msg: text::Text = "count ${COUNTER.n}";
    let _u: facet::Handle = facet::find(ctx, "count").set_text(msg.view());  // 2. push by key
    return;
}
```

Two steps, always: write the field, push it to the keyed element. The element's
view survives; nothing is rebuilt.

## Which item fired: `key_of`

```cplus
fn key_of(sender: *u8) -> text::Text
```

A per-item handler bound as an instance method spends its `ctx` binding the
receiver, so it recovers which item fired from `sender`. `key_of` returns the
sender's key (its agent id) as an owned `Text`; the AppKit backend walks up to
the first tagged ancestor view, so a click on an untagged inner view still
reads its row's key. Empty when nothing up the chain is tagged. With namespaced
keys, parse the id segment back out:

```cplus
fn on_tab_click(sender: *u8, ctx: *u8) {
    let k: text::Text = facet::key_of(sender);   // e.g. "tabs:tab:42"
    // parse the trailing segment, look the tab up, act on it
}
```

A `clickable`'s handler fires from a gesture recognizer, so its `sender` is the
recognizer, not a view. `key_of` and `raise` both normalize such senders to the
view they are attached to before any view walk.

## What was dropped: `dropped_text`

```cplus
fn dropped_text(sender: *u8) -> text::Text
```

An `.on_drop` handler's sender is the drop-zone view; `dropped_text(sender)`
returns the text payload the drag carried (a `.draggable(text)` source's
string), empty when nothing has been dropped. Pair with `key_of(sender)` for
where it landed:

```cplus
fn on_card_drop(sender: *u8, ctx: *u8) {
    let zone: text::Text = facet::key_of(sender);        // "board:col:2:drop"
    let card: text::Text = facet::dropped_text(sender);  // "card:42"
    // parse both, move the card, update by key
}
```

## Light and dark: `is_dark` and the appearance signal

```cplus
fn is_dark() -> bool
fn on_appearance_change(handler: fn(*u8), ctx: *u8 = 0)
```

Semantic `Color` tokens adapt to the system appearance by themselves (see
widgets.md). An app that themes with explicit RGBA reads `is_dark()` when
choosing colors, and registers one `on_appearance_change` handler: the backend
fires it on every system light/dark flip. Colors are flattened at build time,
so the handler re-applies them — re-theme keyed elements in place, or swap a
root outlet's child.

## The native escape hatch

When the portable mutators are not enough, `Handle::view()` returns the raw
native view. Wrap it in its backend type for the full native API:

```cplus
// AppKit
if btn_h.found() {
    let b: ak::Button = ak::Button::from_raw(btn_h.view());
    b.set_title("native!");            // anything the native control exposes
}
```

A generic `find_as[T]` returning the typed widget is not expressible (the core
cannot name a backend type, and there is no generic method dispatch to call
`T::from_raw`), so `view()` is the hatch. Guard with `found()` first — on a miss
`view()` is `0`.
