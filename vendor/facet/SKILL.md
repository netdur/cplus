# SKILL — writing facet UI

Dense reference for an LLM about to write or edit a facet screen. Assumes the C+
language skill (`cpc skill`); this file is only about facet.

facet is a **retained, imperative** UI framework. It is not React, not SwiftUI,
and not reactive in any sense. Almost every mistake an LLM makes here comes from
assuming otherwise — and because facet's API is permissive, **those mistakes
compile**. The compiler cannot catch a design error. This file is the part
`cpc build` will never tell you.

Backends: `facet_appkit` (macOS), `facet_uikit` (iOS), `facet_gtk` (Linux). Your
screens are portable; the entry installs one backend.

---

## 1. The model — build once, then mutate

**`build` runs ONCE, at mount. Nothing re-runs it.** There is no render loop, no
diff, no vdom. What `build` returns is a *live tree*, like the DOM. To change
what is on screen you find the node and set the property. You never rebuild to
show a change.

Three tiers, and every UI task is one of them:

| Tier | What it is | When |
|---|---|---|
| **tree** | `build` returns nodes | once, at mount |
| **cursor** | `label::find(key)` → typed handle → `set_*` | every change after that |
| **store** | a `resource` — verbs in, `Change` out | data shared by more than one component |

The trap, stated plainly: if you find yourself calling `build` again, holding the
tree to "re-render it", or minting a child inside a parent's `build` on every
call — stop. That is the React shape and it is wrong here.

---

## 2. A component, whole

This is the template. Copy this shape.

```cplus
import "flex_layout/flex_layout" as flex;
import "facet/facet" as core;
import "facet/elements" as ui;
import "facet/component" as component;
import "facet/label" as label;
import "facet/screen" as screen;
import "facet/vocabulary" as vocab;
import "stdlib/option" as option;
import "stdlib/text" as text;
import "stdlib/vec" as vec;

struct Counter {
    clicks: i64,                       // state is a FIELD. The component is
}                                      // retained; fields live as long as the tree.

impl Counter {
    fn new() -> Counter { return Counter { clicks: 0 as i64 }; }

    // ---- node helpers: structure `build` would otherwise repeat -------------
    // `ref this` because it binds a handler (see §4).
    fn step(ref this, key: str, title: str) -> core::Node {
        return @ui {
            button(title, key: key, on_click: this.on_step)
        };
    }

    // ---- setters: the live tree, found by key -------------------------------
    fn show_count(this) {
        let n: i64 = this.clicks;
        if let option::Option::Some(l) = label::find("count") {
            let _l: label::Label = l.set_text("${n}");
        }
        return;
    }

    // ---- handlers -----------------------------------------------------------
    fn on_step(ref this, sender: *u8) {
        let key: text::Text = component::key_of(sender);
        if key.view() == "step:up"   { this.clicks = this.clicks + (1 as i64); }
        if key.view() == "step:down" { this.clicks = this.clicks - (1 as i64); }
        this.show_count();
        return;
    }
}

impl Counter: component::Component {
    fn build(ref this) -> core::Node {
        let start: i64 = this.clicks;
        return @ui {
            column {
                label("You have pushed the button this many times:", key: "caption")
                label("${start}", key: "count",
                      font_size: 56.0f64,
                      font_weight: vocab::FontWeight::Bold)
                hstack {
                    this.step("step:down", "-")
                    this.step("step:up", "+")
                }
                    .gap(8.0f64)
            }
                .grow(1.0f64)
                .gap(12.0f64)
                .padding(20.0f64)
                .align(flex::Align::Center)
                .justify(flex::Justify::Center)
        };
    }
}

impl Counter: component::Lifecycle {
    fn on_attach(ref this) { this.show_count(); return; }
    fn on_detach(ref this) { return; }
}

impl Counter: screen::Screen {
    fn chrome(this) -> screen::Chrome {
        return screen::Chrome::new(title: "Counter", width: 380.0f64, height: 420.0f64);
    }
    fn menu_items(this) -> vec::Vec[screen::MenuItem] {
        return vec::new::[screen::MenuItem]();
    }
}

fn boxed() -> screen::ScreenBox { return screen::screen_box(Counter::new()); }
```

**Everything lives on the struct.** State is fields, handlers and node helpers are
instance methods. A screen has **zero top-level fns** — the only ones are the
`boxed()` factory and, in the entry module, `run()`. A top-level
`fn on_click(sender: *u8, ctx: *u8)` that casts `ctx` back to your type is the
single most common wrong shape; it means you did not know handlers bind (§4).

**A long `build` is the tell.** Say repeated structure once as a node helper and
call it. If the helper grows state or handlers of its own, it is a component in
its own file, not a helper.

---

## 3. The tree — `@ui { ... }`

`@ui` is a contextual builder block (C+ language feature; `ui` is
`facet/elements`). Bare names inside resolve to `ui::*`, so `label`, `button`,
`column` are `ui::label`, `ui::button`, `ui::column`.

```cplus
@ui {
    column {                              // container: bare name + braces, NO `@`
        label("Title", key: "t")          // leaf element
        hstack {
            label("left")
            label("right")
        }
            .gap(8.0f64)                  // modifiers attach to the CLOSING BRACE
            .align(flex::Align::Center)
    }
        .grow(1.0f64)
        .padding(20.0f64)
}
```

**Modifiers are line-leading dots.** A `.x` at the start of a line modifies the
item above it; a `.x` on the same line is ordinary postfix. This is the piece
most often missed — without it you end up building the tree and then hunting
through it with `core::find_in` + `core::set_grow(...)` to state layout that
belonged inline. If you are writing `set_grow`/`set_align`/`set_padding` inside
`build`, you wanted a modifier.

**Layout modifiers come from flex_layout** (`core::Node` *is* `flex::Node`), so
`import "flex_layout/flex_layout" as flex;` is in every screen:

```
.grow(f64)   .shrink(f64)   .width(f64)   .height(f64)
.width_percent(f64)  .height_percent(f64)
.padding(f64)  .padding_edge(flex::Edge::Top, f64)   .margin(f64)  .margin_edge(..)
.gap(f64)      .justify(flex::Justify::…)   .align(flex::Align::…)   .wrap(..)
```

**facet adds** `.gesture(on_click: …)` (`facet/gestures`) and the appearance
setters on the node (`set_background_color`, `set_corner_radius`, `set_shown`,
`set_input_transparent`, …).

Say **`column`** and **`hstack`**. `vstack`/`row` are aliases for the same two
axes; picking a third name for a second thing is how a codebase ends up with
four words for two concepts.

Allowed inside a block: item lines, `.modifier` lines, `let`, `if`/`else`,
`for … in …`, nested containers, and calls to your own node helpers
(`this.step(...)`). Rejected: `while`, `return`, `break`, `defer`, `guard`, and a
nested `@`.

```cplus
@ui {
    let ver: str = this.service.version;      // `let` setup is fine
    column {
        label("Version ${ver}")
        if this.is_admin { label("admin", key: "badge") }   // adds into THIS block
        for i in 0..3 { this.tab(i) }                        // one+ items per pass
    }
}
```

`for` takes a range (`0..n`, element type `i32`), never a `Vec` — and it is for a
small fixed set like tabs, **not** for building list rows (§6).

**Grow needs growing ancestors.** `flex_grow` distributes *free space*, and a
content-sized ancestor has none — `column { column { x.grow(1) } }` fills only if
the inner column also grows. Matches CSS; not a bug.

**Every control needs a `key`.** Keys are how you find a node again (§5) and they
are the agent/test surface. A control without one is unreachable and untestable.
Put the key on the node the gesture is on, not on the content a helper wraps.

---

## 4. Handlers — `this.method` binds

A handler is a **bound method**, passed by name. The compiler synthesizes the
bridge and fills the context slot:

```cplus
fn on_step(ref this, sender: *u8) { ... }        // the method

button("+", key: "step:up", on_click: this.on_step)   // the binding
```

That is the whole mechanism. **Never** write `#addr_of(this) as *u8` and pass it
as `on_click_ctx` yourself — that is the manual form of what the compiler already
did, and it is the tell of code written by guessing.

The method's shape is the handler's parameters **minus the trailing `*u8`**, same
return type. For `on_click: fn(*u8, *u8)` that is `fn(ref this, sender: *u8)`.

**A node helper that binds a handler takes `ref this`.** Binding needs a writable
receiver place; a helper declared `fn step(this, …)` fails with **E0823**. A
helper that only reads (`fn chip(this, …)`) may stay `this`.

**Which control fired:** one handler can serve many keyed controls. Ask the
sender.

```cplus
fn on_step(ref this, sender: *u8) {
    let key: text::Text = component::key_of(sender);
    if key.view() == "step:up" { ... }
}
```

Also on `sender`: `component::item_index_of` (the row index in a list),
`component::item_of`, `component::dropped_text`, `component::drop_position`.

**Pass more than one handler BY NAME.** Each handler's context slot is the `*u8`
immediately after it, so positional arguments make every handler fill the slot
belonging to the one before it:

```cplus
row(on_click: this.open, on_long_press: this.menu)     // correct
row(this.open, this.menu)                              // silently misaligned
```

**Declaring your own handler parameter?** The `*u8` slot is not optional:

```cplus
fn row(on_click: fn(str, *u8) = props::no_handler,
       on_click_ctx: *u8 = 0 as *u8) -> core::Node { ... }
```

Omit it and callers can never pass a method — **W0824** warns at the declaration
and prints the line to add. Same for a struct field that stores a handler: store
the `*u8` beside it.

**A labeled signature is claimed once, across every type.** A named call resolves
a method NAME to one labeled parameter list. A second type declaring the same
name with labels — even identical labels — turns every already-written named call
ambiguous (**E1002**), in files that did not change. Give the twin a different
name, or a label-free signature.

---

## 5. Changing what is on screen

### The cursor tier — reach a control through its OWN kind

Each control module exports a typed `find`. **The wrong kind answers `None` and
does nothing, silently** — a label is not a field is not a button.

```cplus
import "facet/label" as label;

if let option::Option::Some(l) = label::find("count") {
    let _l: label::Label = l.set_text("${n}").set_text_color(vocab::Color::rgba(0.4f64, 0.4f64, 0.4f64));
}
```

Setters return the cursor, so they chain. Bind the result to `let _x: T` — it is
a value, not a statement.

`label::find` · `text_field::find` · `text_area::find` · `button::find` ·
`text_button::find` · `icon_button::find` · `symbol::find` · `list::find` ·
`table::find` · `tree::find` · `collection::find` · `popup::find` ·
`search_field::find` · `scroll::find` · `split::find` · `toggle::find` … — one
per control module.

### The mount tier — structure, not properties

```cplus
import "facet/mount" as mount;

mount::find(key, within: *core::Node = 0)  -> option::Option[*core::Node]
mount::node(key, within: ...)              -> *core::Node   // 0 if absent
mount::add_child(parent, take child)       -> bool
mount::insert_child(parent, take child, at: usize) -> bool
mount::remove_child(parent, at: usize)     -> option::Option[core::Node]
mount::replace(key, take child)            -> option::Option[core::Node]
mount::set_content(outlet, ref component)  -> bool          // fill a named outlet
```

`within:` scopes a search to a subtree — the way to ask "where is this showing
now" when the same key shape appears in more than one lane.

**Outlets** are how a screen hosts child components: build an empty container with
a key, then fill it in `on_attach`.

```cplus
fn on_attach(ref this) {
    match mount::find("welcome:main") {
        option::Option::Some(n) => { mount::set_content(n, this.launcher); }
        _ => { }
    }
    return;
}
```

### Show/hide, never rebuild

To toggle a thing, mount it always and show/hide it. Hidden frees its space **and
keeps its state** — scroll offset, selection, half-typed text.

```cplus
core::set_shown(n, on);
if on { core::relayout(n); }        // REQUIRED — see below
```

**Showing it again needs `relayout`.** Out of layout the node cached a zero size,
and restoring the display does not invalidate that: it comes back visible and
0×0, drawing its children on top of whatever took its place.

### Show/hide by SIZE — say the rule, not the callback

When the thing being toggled depends on how much room there is, do not observe
the size and toggle by hand. Name a **band** on the node and the layout pass
decides, every pass:

```cplus
cards.add(pane("Detail", ...).hide("tiny").hide("compact"));
sidebar.hide_in("compact");                 // same rule, on a cursor
```

Six bands are pre-registered — `tiny` (<300pt wide), `compact` (300–599),
`medium` (600–839), `expanded` (840–1199), `large` (1200–1599), `xlarge`
(≥1600) — and `bands::configure(name, max_width: …, max_height: …)` retunes
one or adds your own. Use the names, not raw numbers: a threshold written
where it is used drifts, and two screens end up disagreeing about where a
phone stops being a phone.

The band is measured against the node's nearest ancestor whose size does not
depend on its own contents — **not the window**. In Split View the app has
half the screen, and half the screen is the honest answer. A node never
queries itself, so a pinned 400pt sidebar still asks about the space it was
given.

No `relayout` is needed here and no `Cancellable` has to be kept alive: this
is not a runtime write, it is a rule the pass already re-reads.

### The one exception

**A text field being EDITED cannot be written** — its field editor owns the
string. Swap that small subtree instead of setting its text.

---

## 6. Lists are recycled — you supply a data source

A list of anything is `list`, `table`, `collection` or `tree` with a **data
source**. You never hand-mount, reorder, or swap row nodes.

```cplus
// build: the empty control
@ui { list(key: "panel:list") }.grow(1.0f64)

// after mount: arm it
if let option::Option::Some(l) = m_list::find("panel:list") {
    let _l: m_list::List = l
        .set_row(this.row_at)                          // fn(usize, *u8) -> Node
        .set_row_height_of(this.row_height)            // fn(usize, *u8) -> f64
        .set_selection_mode(vocab::SelectionMode::Single)
        .set_count(this.rows.count());                 // count LAST
}

// to change what it shows: change the model, then say the count
fn reload(ref this) {
    if let option::Option::Some(l) = m_list::find("panel:list") {
        let _l: m_list::List = l.set_count(this.rows.count());
    }
    return;
}
```

Fine-grained: `insert_rows(at, count)` / `remove_rows(at, count)`. Sectioned:
`set_group_count` / `set_group_size` / `set_group_header`.

Four things about lists that compile wrong:

1. **Say appearance AFTER mount.** `selection_mode`, separators and scroll bars
   are skipped on the create pass and nothing dirties them again — a value the
   control was *born with* is never applied. The table keeps `highlight: none`,
   which is not a look, it is a refusal to be selected, and every row is dead.
   Write appearance through the cursor beside `set_row`, never in the
   constructor.

2. **A row's click is the LIST'S SELECTION**, not a gesture you hang on it. The
   table owns the mouse inside its own rows, so `.gesture(on_click:)` on anything
   in a row is a handler nothing will deliver to. Say `selection_mode`, read
   `on_item_selected`, and the index you are handed is already the model's. A
   real control inside a row still gets its own press.

3. **A row is measured BEFORE it is realised**, so anything sized by its own text
   answers zero — a label cannot say how tall it is until it has a view. State
   the height yourself as arithmetic over what the row stacks, and keep that
   arithmetic beside the builder: they are two readers of one number, and in
   separate files they drift. When the height genuinely *is* the text (a wrapped
   bubble), measure the string — with the same font the label will draw and at
   the width the row is really laid out at (a table keeps an inset for itself;
   measuring at the wider number clips the last line off every row).

4. **Decoration must not take the pointer.** A label answers the hit test with
   itself, so text laid over a clickable card is a hole in the card exactly where
   the eye aims. Everything inside a clickable thing that is not itself a control
   wants `set_input_transparent(true)`.

---

## 7. Data — resources

Data shared by more than one component lives in a **resource** (`facet/resource`):
a store whose only doors are REST verbs. Each verb runs the backing **off the
main thread** and installs the result on it; every landed write broadcasts one
typed `Change` to every watcher. **The write IS the notification** — nobody
hand-wires "tell that screen". One component never updates another; both watch
the resource.

```
get(r)                     GET    /notes       refresh the collection
get(r, id: 12)             GET    /notes/12    refresh one row
get(r, q: "auth", then:)   GET    /notes?q=    query — caller-scoped, broadcasts nothing
post(r)                    POST   /notes       create from r's draft
put(r, id: 12)             PUT    /notes/12    update from r's draft
delete(r, id: 12)          DELETE /notes/12
watch(r, this.on_changed)  the channel
```

The store lives as a module `static`, and the three interface methods split by
thread:

```cplus
struct Notes {
    st: resource::State,
    rows: vec::Vec[Note],        // live — main-thread truth
    staged: vec::Vec[Note],      // staging — run writes, apply installs
    d_title: text::Text,         // draft — what post/put mean
    a_title: text::Text,         // armed — prepare's copy, owned by the flight
}

static NOTES: Notes = #zero::[Notes]();

impl Notes: resource::Resource {
    fn state(ref this) -> *resource::State { return #addr_of(this.st); }
    fn prepare(ref this) { this.a_title = this.d_title.clone(); return; }  // main
    fn run(ref this, req: *resource::Request) { ... }                     // OFF main
    fn apply(ref this, req: *resource::Request) { ... }                   // main
}
```

`run` hits the backing and writes **staging fields only**; `apply` installs them
into the live fields. At most one flight per resource — a verb called while one
is up queues in call order, so exactly one worker ever touches staging and a
later query can never overtake an earlier one.

Wrap the verbs so a handler calls one thing and stops, and expose plain
synchronous accessors for reads:

```cplus
fn load(then: fn(*u8) = 0 as fn(*u8), then_ctx: *u8 = 0 as *u8) {
    resource::get(NOTES, then: then, then_ctx: then_ctx);
    return;
}
fn add(title: str, then: fn(*u8) = 0 as fn(*u8), then_ctx: *u8 = 0 as *u8) {
    NOTES.d_title = title.to_text();
    resource::post(NOTES, then: then, then_ctx: then_ctx);
    return;
}
fn watch(f: fn(resource::Change, *u8), ctx: *u8 = 0 as *u8)
    -> events::SignalSubscription[resource::Change] {
    return resource::watch(NOTES, f, ctx: ctx);
}
fn count() -> usize { return NOTES.rows.count(); }
fn at(i: usize) -> option::Option[*Note] { return NOTES.rows.at_ptr(i); }
```

### The component discipline — the whole of it

```cplus
impl Panel: component::Lifecycle {
    fn on_attach(ref this) {
        this.sub = notes::watch(this.on_notes_changed);   // watch FIRST
        notes::load();                                     // then ask
        return;
    }
    fn on_detach(ref this) { return; }   // the subscription is an owning handle
}

impl Panel {
    // ALL screen updating lives here.
    fn on_notes_changed(ref this, c: resource::Change) {
        match c.verb {
            resource::Verb::Loaded  => { this.reload(); }
            resource::Verb::Created => { this.reload(); }
            resource::Verb::Updated => { this.repaint(c.id); }
            resource::Verb::Deleted => { this.reload(); }
        }
        return;
    }
}
```

- **`on_attach`**: watch the resources you show, then `get` them.
- **`build`**: render immediately from whatever the store holds.
- **handlers**: fill the draft, call the verb, **stop**. No UI code at the call site.
- **the watch handler**: reconcile the one change, by key, reading the store's
  sync accessors.

Five ways this goes wrong:

- **Never keep your own copy.** A snapshot parked in a field is a second truth,
  and it is the one that goes stale. Read the store.
- **If it broadcasts, do not also do the work.** The watch handler runs when the
  write lands; doing the update at the call site too mounts everything twice.
- **Never call the backing (sqlite/fs/net) from a handler** — that blocks the main
  thread. Backing code lives in `run`. A failed write broadcasts nothing; handle
  failure in `then`.
- **When one verb carries several writes, ask WHICH one landed, positively.**
  Three writes sharing `put` all broadcast `Updated`; asking "not a backup" makes
  every write added later raise a banner that belonged to exactly one of them.
- **The queue is faithful, so the caller collapses bursts.** Every queued verb
  runs. A file watcher firing once per file turns a hundred-file build into a
  hundred queued walks unless the surface holds a guard that drops calls while
  one is up, cleared at completion so the next ask re-reads.

`events::emit` (the bus) stays for UI-only facts that touch no store. When a bus
fact does feed a store, the component that owns the store translates it into a
verb and stops.

---

## 8. Screens, chrome, nav

A screen is a component that also implements `screen::Screen`:

```cplus
impl Welcome: screen::Screen {
    fn chrome(this) -> screen::Chrome {
        return screen::Chrome::new(
            title: "Iris",
            width: 800.0f64, height: 500.0f64,
            min_width: 640.0f64, min_height: 400.0f64,
            bar: screen::Bar::Blended,
            maximizable: false, minimizable: false, zoomable: true,
        );
    }
    fn menu_items(this) -> vec::Vec[screen::MenuItem] {
        return vec::new::[screen::MenuItem]();
    }
}

fn boxed() -> screen::ScreenBox { return screen::screen_box(Welcome::new()); }
```

The app registers screens by route and runs one:

```cplus
fn run() -> i32 {
    var app: runtime::App = runtime::App::new("iris");
    app.screen("welcome", welcome::boxed);
    app.screen("workspace", workspace::boxed);
    match app.run("welcome") {
        status::Status::Ok => { return 0 as i32; }
        _other => { return 1 as i32; }
    }
}
```

Navigation is `nav::go(route, arg)` / `nav::push` / `nav::pop` / `nav::quit`,
read back with `nav::arg()` and `nav::param(key)`. A screen that navigates away
is **parked, not destroyed** — attach/detach only notify (`on_attach`/
`on_detach`); the views and state survive, so coming back restores scroll
position and half-typed input for free.

The entry module installs a backend and calls `run`:

```cplus
import "./app" as app;
import "facet_appkit/facet_appkit" as backend;

fn main() -> i32 {
    backend::install();
    return app::run();
}
```

---

## 9. Traps that compile

Ranked by how often they are written, all of them clean builds:

1. **A top-level handler with a hand-cast `ctx`.** Handlers are methods; pass
   `this.method` (§4).
2. **Building the tree, then hunting it with `find_in` + `set_grow`/`set_align`
   to state layout.** Those are leading-dot modifiers (§3).
3. **Rebuilding to show a change.** Find the node and set the property (§5).
4. **Rebuilding to toggle.** Mount always, show/hide — and `relayout` on the way
   back (§5).
5. **Hand-mounting rows.** Lists recycle; supply a data source (§6).
6. **Setting list appearance in the constructor.** Say it after mount (§6).
7. **`.gesture(on_click:)` on something inside a row.** That is the list's
   selection (§6).
8. **A component reaching into another component.** Both watch the resource (§7).
9. **A snapshot of the store parked in a field.** Read the store (§7).
10. **Doing the update at the call site *and* in the watch handler.** Everything
    mounts twice (§7).
11. **A control with no `key`.** Unreachable and untestable (§3).
12. **Decoration that eats the pointer.** `set_input_transparent(true)` (§6).

Two rules about believing your own work:

- **`click` proves an action fires; a pointer proves it can be reached.** The
  agent surface's `click` sends `performClick:`, which skips hit testing and the
  responder chain *on purpose* — so it can drive a control the pointer could
  never get to, and says nothing about whether a hand could. Drive the app to a
  state and measure that. For "can this be pressed", ask a person.
- **Typing is not `set_text`.** They land on different halves of the backend, so
  a screen driven only through the socket does not test what typing does.
- **A write onto the live tree is a claim until you count it.** A call that did
  nothing looks exactly like a value that arrived and was ignored. Count the
  calls before reasoning harder.

---

## 10. Finding the rest

Elements take many named parameters — `label` has 18, `button` 25 — and this file
does not enumerate them. Ask the graph:

```bash
cpc query def facet.src.elements::label      # the signature, with defaults
cpc query members facet.src.label::Label     # every setter on the cursor
cpc query callers facet.src.mount::set_content
```

Modules: `elements` (every constructor) · `facet` (Node) · `component` ·
`mount` · `screen` · `nav` · `resource` · `gestures` · `vocabulary` (colors,
enums, spans) · `theme` · `services` · plus one module per control
(`label`, `button`, `text_field`, `list`, `table`, `tree`, `collection`,
`popup`, `scroll`, `split`, `toggle`, `slider`, `web`, `canvas`, …).

`vendor/facet/src/*.cplus` carry long header comments explaining *why* each tier
is shaped the way it is — `resource.cplus` and `component.cplus` especially.
Read those before proposing a change to facet itself.

**Before adding anything to facet, ask whether the app can write it itself.** If
it can, write it in the app.
