# The `@facet` DSL, widgets, and layout

> Entry path: [tutorial.md](tutorial.md) · [guide.md](guide.md) · [ref.md](ref.md)

A screen is a `Node` tree. You build it with the `@facet { }` contextual-builder
DSL, or with the `Builder` API directly.

## The DSL

```cplus
@facet {
    vstack {
        label("Title", size: 20.0f64, bold: true)
        wrap_label("A longer line that wraps to the width.", secondary: true)
        hstack {
            button("New", primary: true).on_click(this.on_new)
            button("Open")
        }
    }
    .padding(12.0f64)
    .gap(8.0f64)
}
```

Rules:

- A bare **leaf** name resolves against this package: `label(...)` is
  `facet::label(...)`.
- A bare **container** (`vstack { ... }`) calls the container function with the
  block's accumulated children.
- **Modifiers** follow the `flex_layout` line rule: on the **same line** as the
  item they are a fluent chain (`take this -> Node`); on their **own
  leading-dot line** they are in-place `set_*`. Both return a `Node`.

`@facet { ... }` desugars to `Builder::new()` + `.add(item)` + `.finish()`, so
an `@facet` root wraps its children in a column.

## The Builder API

The imperative equivalent, useful when you need an exact shape or a computed
list:

```cplus
var b: facet::Builder = facet::Builder::new();
b.add(facet::label("row-a"));
b.add(facet::label("row-b"));
return facet::vstack(b);        // or column/hstack/row/zstack/grid/card/scroll/...
```

## Leaves

| constructor | widget |
|---|---|
| `label(s, size?, bold?, secondary?)` | single-line text |
| `wrap_label(s, size?, secondary?)` | wrapping text |
| `button(s, primary?)` | push button |
| `text_field(placeholder?, value?)` | single-line input |
| `secure_field(placeholder?, value?)` | password input |
| `toggle(...)` | checkbox / switch |
| `slider(...)` | continuous value |
| `stepper(...)` | discrete +/- |
| `progress(value?, indeterminate?)` | progress bar |
| `gauge(value?, min?, max?)` | level indicator |
| `segmented(...)` | segmented control |
| `popup(...)` | pop-up menu |
| `color_picker(on_change?, ctx?)` | color well |
| `date_picker(on_change?, ctx?)` | date picker |
| `image(path)` | image |
| `symbol(name, size?, secondary?)` | SF Symbol (provisional; Apple-specific) |
| `divider()` | separator line |
| `spacer()` | flexible gap |
| `box()` | an empty backing view (drop target / drag source) |
| `path(width, height)` | vector path (`.move_to`/`.line_to`/`.close_path`) |
| `native(handle)` | adopt an app-owned native view (the escape hatch) |
| `list(count, row, ctx?)` | recycling list — `row(i, ctx) -> Node` built lazily |
| `text_area(value?, on_change?, ctx?, editable?, ...)` | multi-line editor (see below) |
| `composer(on_submit, on_change?, ctx?, value?, ...)` | chat input — Enter submits, Shift+Enter newline |

A code editor is a `text_area` configured, not a separate kind:

```cplus
text_area(value: src, editable: true, show_border: false,
          wrap: false, smart_substitutions: false, line_numbers: true)
    .monospaced().font(13.0f64)
    .foreground_color(...)          // text and caret
    .background(...)
```

`wrap: false` disables soft wrap and adds a horizontal scroller;
`smart_substitutions: false` turns off automatic quotes/dashes/replacement;
`line_numbers: true` adds a line-number gutter. `.monospaced()` / `.font(size)`
/ `.foreground_color(...)` reach the inner text view, and the caret follows the
foreground. An editable area supports undo.

## Containers

| constructor | layout |
|---|---|
| `vstack {}` / `column(b)` | vertical stack |
| `hstack {}` / `row(b)` | horizontal stack |
| `zstack {}` | overlay (children stacked front-to-back) |
| `grid {}` / `grid(b, columns)` | grid flow |
| `card {}` | padded, bordered container |
| `scroll {}` | scrollable content |
| `split {}` / `split(b, vertical?, position?)` | draggable split panes |
| `bordered {}` / `bordered(b, radius?)` | a backing view with a border |
| `clickable {}` | a container that takes gestures (click/hover/drag/...) |
| `material {}` | a translucent material background |

## Modifiers

Layout (consumed by `flex_layout`, applied write-once at mount). facet uses
`flex_layout`'s CSS-flexbox vocabulary directly:

| modifier | effect |
|---|---|
| `.grow(v)` / `.shrink(v)` / `.flex_basis(v)` | flex grow / shrink / basis |
| `.width(v)` / `.height(v)` / `.frame(w, h)` | fixed size |
| `.width_pct(v)` / `.height_pct(v)` | percentage size |
| `.min_width(v)` / `.max_width(v)` / `.min_height(v)` / `.max_height(v)` | bounds |
| `.gap(v)` / `.gap_row(v)` / `.gap_col(v)` | spacing between children |
| `.padding(v)` / `.margin(v)` / `.inset(edge, v)` | inner / outer / per-edge space |
| `.align_items(a)` / `.align_self(a)` / `.justify_content(j)` | flexbox alignment (`flex::Align` / `flex::Justify`) |
| `.flex_direction(d)` / `.flex_wrap(w)` / `.direction(d)` | axis / wrapping / writing direction |
| `.position_absolute(...)` / `.position_relative()` | positioning |
| `.grid_pos(...)` / `.grid_span(...)` | grid placement |
| `.aspect_ratio(r)` / `.z_index(z)` | ratio / paint order within a `zstack` |

Content, style, and identity:

| modifier | effect |
|---|---|
| `.key(id)` | the keyed-direct address (also the agent id / accessibility identifier) |
| `.agent_id(id)` | set the agent id without making it a keyed-direct target |
| `.on_click(cb, ctx?)` / `.on_drop(cb, ctx?)` / `.draggable(text)` | wire an interaction |
| `.context_menu(...)` | attach a context menu |
| `.font(size)` / `.weight(w)` / `.strong()` / `.italic()` / `.monospaced()` | text style |
| `.line_limit(n)` / `.truncate()` / `.text_align(a)` / `.underline()` / `.strikethrough()` | text layout |
| `.background(color)` / `.foreground_color(color)` / `.border(w, color)` / `.corner_radius(r)` | paint |
| `.gradient(...)` / `.shadow(...)` / `.opacity(v)` / `.clip()` | paint |
| `.rotation(deg)` / `.scale(v)` / `.fade_in(duration)` | transform / entrance animation |
| `.hidden()` | mount hidden |
| `.tooltip(s)` / `.accessibility_label(s)` / `.accessibility_hint(s)` | help / VoiceOver |
| `.keyboard_shortcut(key)` | a control's key equivalent |
| `.toolbar(items)` | attach a window toolbar to the root node |

Handlers wired through container/leaf **constructors** (not modifiers) — for
example `on_submit` / `on_change` on a composer, or `on: handler` on a `toggle`
— are passed as constructor arguments; see each constructor's signature.

## Handlers

The handler primitive is `fn(sender: *u8, ctx: *u8)` — `sender` is the control
that fired, `ctx` is an opaque pointer facet passes through untouched.

**Bind a component method** — the default form. In `build(ref this)`:

```cplus
button("+1").on_click(this.inc)   // inc is `fn inc(ref this, sender: *u8)`
```

The bound method's receiver fills `ctx`, so `inc` declares only `sender`. Inside,
use `this` for state and `facet::find(key)` to address elements — by id, global,
the same handle an agent uses. See [component-model.md](component-model.md) and
[updates.md](updates.md).

**Raw form**, for a handler with no component: a free `fn(sender, ctx)` wired
with an explicit `.on_click(handler, ctx: data)`. Pass data as `ctx` (a row
index, say); the handler reads it and facet never interprets it.

## Colors and style

`facet::Color` carries a semantic token (which a backend maps to a native
system color) or an explicit RGBA. Style props (`background`, `border`,
`corner`, `font`, `weight`) layer over the widget the constructor produced.

Semantic tokens are adaptive — they resolve correctly in both light and dark
appearance without any app-side branching:

| token | maps to (AppKit) |
|---|---|
| `Color::text()` / `Color::text_secondary()` / `Color::text_tertiary()` | label tiers |
| `Color::placeholder()` / `Color::link()` | placeholder / link text |
| `Color::accent()` | the accent color |
| `Color::separator()` | hairline separators |
| `Color::window_background()` / `Color::under_page_background()` / `Color::control_background()` | surface tiers |
| `Color::fill()` / `Color::fill_secondary()` | neutral control fills |
| `Color::selected_content_background()` / `Color::selected_text_background()` | selection |
| `Color::system_red()` / `green` / `blue` / `orange` / `yellow` / `purple` / `pink` / `teal` / `indigo` / `gray` | system palette |
| `Color::primary()` / `on_primary()` / `secondary()` / `on_secondary()` | theme brand roles ([theme.md](theme.md)) |
| `Color::ink(a)` / `surface()` / `raised()` / `sunken()` / `outline()` / `success()` / `warning()` / `danger()` | theme roles ([theme.md](theme.md)) |
| `Color::adaptive(light:, dark:)` | a light/dark pair, resolved at paint |

`Color::rgba(...)` is a fixed color: it does not adapt. An app that themes with
explicit RGBA branches on `facet::is_dark()` and re-themes from its
`facet::on_appearance_change` handler (see updates.md).
