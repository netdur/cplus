# Reference

Compact API map for `facet`. Behavior and recipes live in the topical docs
and [guide.md](guide.md). Tutorial: [tutorial.md](tutorial.md).

```cplus
import "facet/facet" as facet;
import "facet/runtime" as runtime;
```

---

## Modules

| Path | Contents |
|---|---|
| `facet/facet` | Node, DSL, widgets, Component, find/Handle, lifecycle, Color/Style |
| `facet/runtime` | run, run_component, Window, alert, dialogs, menus (host + backend select) |

---

## `Component`

```cplus
interface Component {
    fn build(ref this) -> Node;
}
```

`build` once; bind handlers with `.on_click(this.method)`.

---

## Addressing

```cplus
fn find(key: str, cp: *u8 = 0) -> Handle
```

Global by default; `cp` scopes to a component subtree. Miss → empty Handle
(no-op mutators). `found()`, `view() -> *u8`.

### Handle leaf mutators (chainable)

| method | effect |
|---|---|
| `set_text(s)` | label / button / field |
| `set_value(v: f64)` | slider / stepper / progress |
| `set_on(on: bool)` | toggle |
| `set_hidden` / `show` / `hide` | visibility |

### Handle structural verbs (keyed containers)

| method | effect |
|---|---|
| `add_child(take Node)` | append |
| `insert_child(take Node, at: usize)` | insert |
| `replace_child(key, take Node) -> bool` | swap |
| `remove_child(key) -> bool` | remove |
| `set_child(take Node)` | replace sole child |

Retained live subtrees: lifecycle attach (see lifecycle.md), not these alone.

Full narrative: [updates.md](updates.md).

---

## Lifecycle

```cplus
// stage / Staged / attach / detach / is_attached / Lifecycle
// see lifecycle.md for signatures and router pattern
```

---

## Leaves (constructors)

| constructor | widget |
|---|---|
| `label` / `wrap_label` | text |
| `button` | push button |
| `text_field` / `secure_field` | single-line input |
| `text_area` / `composer` | multi-line / chat input |
| `toggle` / `slider` / `stepper` / `progress` / `gauge` | value controls |
| `segmented` / `popup` | choice |
| `color_picker` / `date_picker` | platform pickers |
| `image` / `symbol` | media (symbol often Apple-specific) |
| `divider` / `spacer` / `box` | chrome / layout |
| `path` | vector path |
| `list` | recycling list (`row` builder) |
| `native` | adopt app-owned view |

Details and options: [widgets.md](widgets.md).

---

## Containers

`vstack`/`column`, `hstack`/`row`, `zstack`, `grid`, `card`, `scroll`,
`split`, `bordered`, `clickable`, `material`, …

---

## Common modifiers

**Identity / interaction:** `.key`, `.agent_id`, `.on_click`, `.on_drop`,
`.draggable`, `.context_menu`, …

**Layout (flex_layout):** `.grow`, `.shrink`, `.width`/`.height`,
`.width_pct`/`.height_pct`, min/max, `.gap`, `.padding`/`.margin`,
`.align_items`/`.justify_content`, `.flex_direction`/`.flex_wrap`, absolute
position, grid placement, `.aspect_ratio`, `.z_index`, …

**Style:** `.font`, `.monospaced`, `.foreground_color`, `.background`, …

Full tables: [widgets.md](widgets.md).

---

## `Color` / `Style`

Semantic tokens (`primary`, `secondary`, `accent`, `system_*`,
`window_background`, …) and `Color::rgba`. `Style` holds font/color fields
used by leaves. See source `facet.cplus` and widgets.md for the complete
token list.

---

## Runtime host

```cplus
fn run[W: Window](take window: W)
fn run_component[C: Component](take component: C, title, width, height, ...)
fn present_window(take root: Node, title, width, height)
fn alert(title, message, primary, secondary?) -> i32
fn choose_file() / choose_directory() -> Option[Text]
// Window interface: root, title, size, chrome flags, menus, close hooks
```

Backend selection and porting: [backends.md](backends.md).

---

## Package

| | |
|---|---|
| Name | `facet` |
| Dependencies | `stdlib`, `flex_layout` |
| Tests | `cpc test` (`src/test_main.cplus`) |
| Backends | `facet_appkit` (primary), `facet_gtk` (stub/partial) |
