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
    // BOTH take a reason. See §9 — `why` is not decoration, it is the
    // difference between releasing a device and breaking the app.
    fn on_attach(ref this, why: component::Attach) { this.show_count(); return; }
    fn on_detach(ref this, why: component::Detach) { return; }
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

**Shrink says MAY BE SQUEEZED, never MAY FILL.** Both `flex_grow` and
`flex_shrink` default to `0.0`, so a shrink-only node keeps its intrinsic width —
and nothing in layout can widen a node that is not asking. In a recycled row that
intrinsic width is measured *before* the label is realised, where it can be zero
(§6). This is the whole of "labels truncated to slivers in a wide pane", and it
looks so much like a framework fault that one was filed as one; measuring the
frames ended it in a line — 69pt without grow, 739 with. **A label that should own
its row says grow AND shrink.** When a symptom survives a fix whose test is green,
print the frames before reasoning harder.

**Every control needs a `key`.** Keys are how you find a node again (§5) and they
are the agent/test surface. A control without one is unreachable and untestable.
Put the key on the node the gesture is on, not on the content a helper wraps.

**`build` MUST NOT BLOCK.** It runs on the main thread during mount, so anything
slow in it is a window that does not appear. Paint placeholders, start the slow
work off-main (§8), and patch the tree when it lands — which is what the retained
model is for. There is no second render pass to wait for, so a node mounted empty
and filled a moment later costs nothing.

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
immediately after it, so positionally the second handler lands in the first
one's context slot. The compiler stops it (E0824 for a bound method, E0312 for
a free fn), but neither message says "you wanted named arguments":

```cplus
row(on_click: this.open, on_long_press: this.menu)     // correct
row(this.open, this.menu)                              // rejected: E0824
```

**Declaring your own handler parameter?** The `*u8` slot is not optional:

```cplus
fn row(on_click: fn(str, *u8) = props::no_handler,
       on_click_ctx: *u8 = 0 as *u8) -> core::Node { ... }
```

Omit it and callers can never pass a method — **W0824** warns at the declaration
and prints the line to add. Same for a struct field that stores a handler: store
the `*u8` beside it.

**A label names a parameter of the receiver's own method.** Two types may share
a labeled method name; each call resolves against the parameter list of the type
the receiver actually has, so `reload(then:, then_ctx:)` on two stores is fine
and neither one's callers see the other. **E1002** is left for the two callees
that genuinely have no parameter names: a fn-pointer value (its type records
parameter types, not names — a handler field included), and a method reached
through a generic receiver, which is not one type until it is instantiated.
Both take positional arguments.

---

## 5. Changing what is on screen

**Try the targeted write first; fall back to a full rebuild only if it fails.**
A rebuild is always available and is almost never the right first move — it
throws away scroll offset, selection and half-typed text to change one string.

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
fn on_attach(ref this, why: component::Attach) {
    if why != component::Attach::Mount { return; }   // outlets are filled ONCE
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

Five things about lists that compile wrong:

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

5. **The reader decides where the scroll goes.** Ask whether the view was AT THE
   END *before* the write and honour that answer after it. Pinned to the bottom,
   an arriving row reads as text arriving; anywhere else, following it drags the
   paragraph being read off the screen ten times a second. Scrolling back down
   re-arms it, so the rule needs no state of its own. And when one row REPLACES
   another, drop the old only once the new has landed — a count that dips for a
   single beat is the whole list flashing down and back.

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
into the live fields — and that install is a **copy**, not a move, for the reason
spelled out in §8: a resource owns heap fields, so moving one out of it is E0509. At most one flight per resource — a verb called while one
is up queues in call order, so exactly one worker ever touches staging and a
later query can never overtake an earlier one.

Wrap the verbs so a handler calls one thing and stops, and expose plain
synchronous accessors for reads:

**THE APP SURFACE IS THE MODULE, NOT THE STRUCT.** Components never name the
static and never call `resource::` themselves — the file that owns the store owns
its vocabulary, and a verb spelled once there is a verb every screen spells the
same way. Each wrapper is one line over `resource::get/post/put/delete/watch`:

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
    fn on_attach(ref this, why: component::Attach) {
        if why != component::Attach::Mount { return; }    // ONCE — see §9
        this.sub = notes::watch(this.on_notes_changed);   // watch FIRST
        notes::load();                                     // then ask
        return;
    }
    fn on_detach(ref this, why: component::Detach) { return; }   // the subscription is an owning handle
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

## 8. Jobs — the other tier, and how to pick

Two tiers ship, and choosing wrong is the first mistake. **The question is who
else has to hear the answer.**

- A **job** (`facet/services`) answers the ONE screen that asked. The caller
  learns through a `then:` or by reading a counter the job bumps.
- A **resource** (§7) is a shared store, and every landed write BROADCASTS a
  typed `Change` to everyone watching.

It goes wrong in both directions. A job whose result you find yourself
hand-delivering to a second screen wanted to be a resource. A resource with one
watcher is a change channel paying for a reader that does not exist.

**The interface IS the threading contract, and for a job it is two methods:**

```cplus
import "facet/services" as services;

struct Search {
    query: text::Text,          // main-thread input
    staged: vec::Vec[i64],      // run writes ONLY here
    hits:   vec::Vec[i64],      // main-thread truth — what the UI paints from
    generation: i64,
}

// THE INSTALL IS A COPY, NOT A MOVE. `Search` owns heap fields so it has a
// destructor, and moving an owning field out of a Drop type is E0509 — the
// destructor would free it twice. `Vec` has no `clone` either, so the swap
// every author writes first (`this.hits = this.staged;`) does not compile.
// Copy the elements through a pointer; this is what facet's own resource does.
fn take_rows(src: *vec::Vec[i64]) -> vec::Vec[i64] {
    var out: vec::Vec[i64] = vec::new::[i64]();
    var i: usize = 0 as usize;
    while i < { (*src).count() } {
        match { (*src).at(i) } {
            option::Option[i64]::Some(v) => { let _a: status::Status = out.append(v); }
            option::Option[i64]::None => { }
        }
        i = i +% (1 as usize);
    }
    return out;
}

impl Search: services::Job {
    fn run(ref this) { this.staged = grep_the_disk(this.query.view()); return; }   // OFF main
    fn apply(ref this) {                                                          // ON main
        this.hits = take_rows({ #addr_of(this.staged) });
        this.generation = this.generation + (1 as i64);
        return;
    }
}

let started: bool = services::run_job(this.search, then: this.on_hits);
```

`run` goes on a worker thread — db, fs, net, a child process — and **must touch
nothing the UI reads**. `apply` comes back on the main thread and installs what
`run` prepared, so the UI never observes a half-written result. Conform and
`run_job` hands you the whole thing: no per-job struct, no boxing, no thread
code. A resource is the same contract plus `state()` (one line — the address of
its embedded `resource::State`) and a `run` that dispatches on `{ (*req).kind }`.

**THE STAGED FIELDS ARE THE HALF OF THAT CONTRACT NO SIGNATURE STATES.** `run`
writes `staged_*` only; the plain field beside it is what the main thread paints
from; `apply` is the one place the two meet. The discipline is invisible in the
types, so it is the first thing to check in a review — **a `run` that assigns the
live field compiles and usually looks fine.** What it costs is a `Text` freed
under a main thread that is cloning it.

**What `run` must not read, `prepare` snapshots** (resources only). It is the
main-thread moment before each flight, taken when the request LAUNCHES and not
when it queued — for a path off a main-only static, or the current selection.
Most resources need nothing, which is the default.

**A SECOND CALL QUEUES ON A RESOURCE AND IS DROPPED ON A JOB.** Both tiers refuse
to put two workers inside one object, and they refuse *oppositely*:

|  | second call while one is up |
|---|---|
| resource | queues in call order, drains after each `apply` — every ask is eventually honoured |
| job | `run_job` returns **false** and the ask is simply gone |

So a job's caller has to decide which ask WINS, and say why. A definition lookup
should drop the second word while one is up, because the answer coming back would
be for the older word and jumping to the wrong place is worse than not jumping. A
`running` / `in_flight` flag on your service is that POLICY, not a safety net —
the safety net is framework-side (`claim_job`), and it exists because two
services without a flag crashed.

And because the queue is faithful, **the caller collapses bursts.** A file
watcher firing once per file turns a hundred-file build into a hundred queued
walks unless the surface holds a guard that drops calls while one is up, cleared
at completion so the next ask re-reads.

**HOW THE ANSWER COMES BACK IS THREE SHAPES; PICK ONE.**

| Shape | For | Runs |
|---|---|---|
| `then:` | "I asked, tell me when it landed" | after `apply`, as a bound method |
| `watch` | anything a *second* component reads | main thread, after the store already holds the new truth |
| generation | a caller that already ticks | a counter `apply` bumps; the caller reads it |

Generation is not a fallback for the other two. A bound `then:` into the same
object needs a mutable borrow of the field AND of `this` in one expression, which
is exactly when a poller reading a counter is the shape that compiles.

**A generation must move even when the answer is NOTHING FOUND.** "No definition
for that word" is an answer the caller has to be able to report, and silence
after a menu command reads as a broken command.

**A resource may be a field rather than a `static`.** The static is the default
and is what makes "every component reaches the same store" true — but a store
that is genuinely one panel's data is right as a field on that panel, because a
second watcher would be a feature nobody asked for. The condition is the lifetime
one, the same as a job's: **it must outlive its own flight**, since the pipeline
reaches it through its address.

**What a test can reach is the pure half and `apply`.** Both tiers are plain
methods, so a fixture stages by hand, calls `apply()`, and asserts the generation
moved and staging came back empty. Do that — it is the half where the threading
discipline actually shows. The verbs are a portable no-op with no backend
installed: nothing runs and nothing queues, so no unit test drives a real flight.

---

## 9. Lifecycle — and `why` is not decoration

```cplus
interface Lifecycle {
    fn on_attach(ref this, why: component::Attach);
    fn on_detach(ref this, why: component::Detach);
}
```

**Both take a reason, and the reason splits FOCUS from VISIBILITY.** That split
is the whole point of the two levels, and collapsing it is how an app breaks in
front of the person using it.

| `Attach` | When |
|---|---|
| `Mount` | entered the live tree. Fires ONCE per attachment, and is the only moment `mount::find` / `mount::add_child` mean anything — **this is where views are built** |
| `Foreground` | visible again after being fully gone. Many times per Mount, and **never at launch** |
| `Active` | frontmost and taking input. At launch, and every time focus returns |

| `Detach` | When |
|---|---|
| `Inactive` | lost FOCUS, possibly still fully visible — a dialog, a notification shade, the other pane of a split screen |
| `Background` | no longer frontmost. Still attached, views still live; what is gone is permission to hold exclusive devices |
| `Unmount` | leaving the tree. Views are alive as this runs and will not be afterwards |
| `Terminate` | the app is closing. Flush anything unsaved |

Four facts that each cost something to learn:

- **`Inactive` is NEVER a release signal.** On a desktop it fires whenever the
  user clicks another window; in split screen it fires while the app is fully on
  screen. An app that released its camera here would stop working in front of the
  person watching it. `Background` is the release signal.
- **`Mount` is not where you acquire a device.** On Android it runs inside
  `onCreate`, where the process may still be BACKGROUND as far as the camera
  service is concerned, and `openCamera` throws CAMERA_DISABLED. `Active` always
  follows; devices belong there.
- **`Active` can arrive with no preceding `Inactive`** — a dismissed dialog is one
  way — so never write a handler that assumes the pairs alternate.
- **`Terminate` is not guaranteed**, and is never the only place work is saved. A
  process killed for memory, or swiped out of the task switcher, reaches none of
  them.

**`on_attach` fires on every un-park, not only at mount.** So one-time setup —
subscribing to a resource, filling an outlet — must be guarded, or a re-show is a
second subscription and a doubled handler:

```cplus
fn on_attach(ref this, why: component::Attach) {
    if why != component::Attach::Mount { return; }
    this.sub = notes::watch(this.on_notes_changed);
    notes::load();
    return;
}
```

And a re-show is not a mount: it should ask again only if the first answer
already landed. On a first mount the load from `Mount` is still in the air, and
asking twice is two flights for one answer.

---

## 10. Screens, chrome, nav

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

`push` means **show this next, with whatever room the platform has**: a peer
window on a desktop, a stack entry on a phone. One intent, rendered as each
platform renders it — so an app that says nothing gets the right shape on both.
Say `show: nav::Show::Screen` to narrow it to a drill-down in the current
window, which is the same everywhere.

```cplus
nav::push("settings");                              // window on desktop, stack on phone
nav::push("camera", arg: "front");                  // addressable as "camera:front"
nav::push("step2", show: nav::Show::Screen);        // in place, with a back path
nav::pop();                                         // the stack only — never closes a window
```

A pushed window is keyed by its route, or `route:arg` when one was given, so two
cameras get two addresses without you inventing them. Pushing a key already open
**activates** it rather than opening a second.

`pop` is the stack and nothing else. A window is closed by name:

```cplus
match window::find("camera:front") {
    option::Option[window::Window]::Some(w) => { let _c: bool = w.close(); }
    option::Option::None => { }
}
```

Which is the rule the two tiers follow: **a key always names a screen; it names
a window only where the platform gave that screen one.** `screen::find` answers
on a phone where `window::find` does not, and neither lies about the other.

The app itself is a handle to an instance the runtime owns, so it survives
`main` returning — which is what a platform whose loop belongs to the OS needs.
It carries the app's own environment: facts about the app, written by C+, by
facet, and by you.

```cplus
let a: runtime::App = runtime::app();     // the running app, always answers
a.env("@platform");                       // "macos" / "ios" / "android" / "linux"
a.env("@backend");                        // "facet_appkit", "facet_android", ...
a.set_env("last_project", path);
a.set_env_flag("licensed", true);
a.windows();  a.screens();
```

`@` keys are C+'s and facet's and `set_env` refuses them, so an app cannot claim
a platform or version it is not running on. There is deliberately **no change
channel** — if the UI must update when a value changes, that value belongs in a
`resource`. And it is not `stdlib::env`: one is the OS's environment, the other
is the app's.

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

## 11. Traps that compile

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
13. **Unguarded setup in `on_attach`.** It fires on every un-park, so the
    subscription is made twice and every handler runs twice (§9).
14. **Releasing a device on `Detach::Inactive`.** That is a focus change, not a
    visibility change — the app breaks while fully on screen (§9).
15. **A `run` that assigns the live field instead of staging.** Compiles, looks
    fine, frees a `Text` under a main thread that is reading it (§8).
16. **A job used where a resource was wanted** — the tell is hand-delivering one
    job's result to a second screen (§8).
17. **A shrink-only label in a wide pane.** Nothing widens a node that is not
    asking; say grow AND shrink (§3).

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
- **A MECHANISM FOUND IN SOURCE IS A HYPOTHESIS, NOT A VERDICT.** Reading vendor
  code and finding something that would explain the symptom proves the code
  COULD do it, never that it DID. One such theory quoted real code, explained
  every screenshot, and was filed as verified — and was wrong. A cause earns the
  word *verified* when it moves a number you can measure; until then file it as a
  suspect and say so. The tell is reasoning that only ever confirms itself,
  because nothing in the loop can contradict it.

---

## 12. Finding the rest

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
