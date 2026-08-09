# inspector

A developer-only live inspector for a mounted facet tree. Browser DevTools for
a C+ application: list the nodes, select one, read what it declared against
what layout computed, edit its properties, and watch the running window change.

    inspector/inspector   the neutral surface — Handle, Value, Prop, Backend
    inspector/tree        the facet-tree walker and the typed get/set dispatch
    inspector/appkit      highlight overlay and point picking (macOS)
    inspector/widget      the facet-facing panel an application embeds

Try it: `examples/inspector_probe` puts the panel beside the app it inspects.

## Use

Embed the panel in your own tree and run its two lifecycle hooks:

```cplus
import "inspector/widget" as panel;
import "inspector/appkit" as iplatform;

struct App {
    panel: panel::Inspector,
}

impl App: component::Component {
    fn build(ref this) -> core::Node {
        var row: core::Builder = core::Builder::new();
        row.add(my_ui());
        row.add(this.panel.build().width(560.0f64));
        return ui::row(row, key: "window").grow(1.0f64);
    }
}

impl App: component::Lifecycle {
    fn on_attach(ref this) { panel::attach(#addr_of(this.panel)); return; }
    fn on_detach(ref this) { panel::detach(#addr_of(this.panel)); return; }
}

fn main() -> i32 {
    iplatform::install();                    // highlight + picking
    let app: App = App { panel: panel::embedded() };
    ...
}
```

`attach` and `detach` are the host's job because facet fires `Lifecycle` for
the component the runtime owns, not for one nested inside another component's
tree.

Or drive it without any UI at all:

```cplus
import "inspector/tree" as inspector;

let rows: vec::Vec[insp::Node] = inspector::describe();
let h: insp::Handle = /* a row's handle */;
let r: insp::Outcome = inspector::set(h, "padding", insp::Value::Num(12.0f64));
```

## What it edits

Writable properties fall into three tiers, and only the third has to know what
control it is talking to.

| Tier | What | Needs a key? |
|---|---|---|
| 1 | The common band — `opacity`, `background_color`, `corner_radius`, `visible`, `enabled`, `input_transparent`, `tooltip`, `accessibility_label`, transform | no |
| 2 | Flex style — `width`, `height`, `grow`, `shrink`, `padding`, `margin`, `gap` | no |
| 3 | Control props — `text`, `title`, `on` | **yes** |

Tiers 1 and 2 are uniform across all 38 control kinds and every bare container,
because `CommonProps` is inline on every node and facet's tree *is* flex's
tree. That is most of what an inspector edits, and it needs no generated
dispatch layer at all.

Tier 3 goes through the generated typed handles, which are reachable only by
key. On an unkeyed node it answers `Unsupported` — the name was right, this
build cannot reach it — rather than failing silently. That closes when
`gen_contract.py` grows an inspector dispatch layer.

## The tree pane

`facet/tree` — a recycling `NSOutlineView` on macOS. This package supplies the
model and nothing else: the control indents its own rows, owns expansion, and
reuses its cells.

Each row is `kind#key` — `card#save`, `label#status` — with the kind alone for
an unkeyed node and a `(viewless)` marker for one an ancestor draws.

    ▾ node#root
      ▾ node#card
          label#title
          label#subtitle
      ▸ node#controls
        node (viewless)

A node's identity is its **address**, not its position. A positional id
("row_7") renames every row after any insert, so expansion and selection would
follow the slot instead of the node. Addresses are what let `restore()` put the
user's folds and selection back after a rebuild.

Row shape, row bind and row height are all supplied. The bind is the one that
is not optional — cell reuse is gated on it, and without one the backend
rebuilds every row from scratch. The height is stated because a tree row is a
fixed-height row, which also spares the height query a throwaway layout per
row.

## Three panels, not one list

`inspect` answers in three categories and they must not be flattened:

- **declared** — facet props and flex style. What the application asked for. Writable.
- **computed** — the laid-out frame, attachment, focus. What layout decided. Read-only.
- **native** — platform class, native frame, first responder. Read-only, and empty until a platform module is installed.

Setting `width` writes a style; reading the frame reads what came out the other
side. The browser calls these `style.width` and `offsetWidth` and keeps them in
separate panels for the same reason.

## What it refuses, and why it says so

An inspector whose refusals are silent teaches a developer that a property does
nothing, which is worse than teaching them nothing.

| Outcome | Means |
|---|---|
| `Stale` | the tree changed since the handle was issued — re-describe |
| `NotFound` | the handle addresses nothing |
| `UnknownProperty` | no property of that name on this node |
| `TypeMismatch` | the property exists and does not take a value of that tag |
| `ReadOnly` | real, and not writable — a computed frame, a native class |
| `Unsupported` | writable in principle, not reachable on this node — tier 3 without a key |

## This is not the agent surface

`agent_core` is curated and permissioned: it hides unexposed nodes, limits
actions to declared affordances, and refuses point-addressed operations. Those
rules protect a user-facing automation surface.

A developer inspector needs the opposite powers — every node including the
unkeyed ones, and writes to properties that are not affordances. So it has its own vtable and its own package. Nothing here is
reachable from the agent surface, and `agent_core::Backend` is unchanged.

Treat it as debug-only. Inspector access is arbitrary UI mutation.

## Two things it is careful about

**It never issues a command bit.** `C_FOCUS`, `C_BLUR` and `C_FLUSH` are
commands the backend performs and clears, not state it re-reads. A write that
raised them would take first responder away from whatever the developer was
typing into — on an unrelated node's property edit. Every setter here names the
one bit it changed. There is a test for it.

**It never writes the platform view directly.** A native-only write leaves
facet's declared state stale and is overwritten by the next sync walk. Every
edit goes through the same setters an application calls, so it takes the same
path: prop, dirty bit, scheduled sync, backend apply.

## Persistence is a copied line

Inspector writes are volatile. They die with the process, and they are not
reapplied when a node is rebuilt.

`Copy as C+` turns the current overrides into source:

    core::set_opacity(mount::node("card"), 0.65f64);

The developer pastes that into the file it belongs in. An override ledger that
silently reapplied itself would be a development tool that had become part of
the application.

`reset` restores what the **application** declared, not the type default. An
app that shipped `opacity: 0.8` and an inspector that reset to `1.0` would not
have undone the experiment — it would have started a second one.

## Limits in this version

- macOS only for the highlight overlay. Everything else is portable and needs no platform.
- **No point picking.** Selecting an element by clicking it in the app was built and removed; select from the tree instead. See docs/design.md.
- Tier 3 needs a key (see above).
- No structural editing: no insert, delete or reparent.
- No handler replacement, no expression evaluation, no memory access.
- No source navigation. facet's `Data` carries no source origin; that needs a debug-only origin ID injected during `@ui` lowering.
- Handles are process-local pointers. A transport adapter sends indices into the flat `describe` listing instead — which is why that listing is flat and parent-indexed.

## Tests

    cd vendor/inspector && ../../target/release/cpc test

Unit, end-to-end over a mounted tree, and negative. The negative half is the
larger one on purpose.
