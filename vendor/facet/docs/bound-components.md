# Bound components

> Additive: nothing in the existing tier changed, and a component that never
> calls a `bind_*` modifier behaves identically.
> Reading order: [component-model.md](component-model.md) →
> [updates.md](updates.md) → here.
> Depends on: [`#[watch]`](../../../docs/design/watch-structs.md).

---

## 1. Problem

In the normal tier a handler does two things:

```cplus
fn inc(ref this, sender: *u8) {
    this.n = this.n + 1;                                             // 1. state
    let _u: facet::Handle = facet::find("n").set_text("${this.n}");  // 2. view
}
```

Step 2 is the bug surface. It is easy to omit, easy to write against the wrong
key, and it has to be repeated in every handler that touches `n` — so the number
of places that must agree about how `n` is displayed grows with the number of
ways `n` can change. When one is missed the app does not crash: a mutator on a
missing element is a silent no-op, so the screen just goes stale, and
stale-by-omission reads as a bug in whatever feature owns the button.

A bound component states the relationship **once, at the node**:

```cplus
fn inc(ref this, sender: *u8) { this.n = this.n + 1; return; }   // that is all
```

## 2. Surface

Three blocks. `impl T: facet::Component` keeps its shape — the tree gains
modifiers, it is not restructured.

```cplus
#[watch]                                     // (a) the barrier
struct Counter { n: i32, note: text::Text, busy: bool }

impl Counter {
    fn inc(ref this, sender: *u8) { this.n = this.n + 1; return; }
    // (b) a thunk: an ordinary method computing ONE property
    fn n_text(ref this, item: *u8) -> text::Text { return "count: ${this.n}"; }
    fn busy_now(ref this, item: *u8) -> bool { return this.busy; }
}

impl Counter: facet::Component {             // (c) the declarations, on the node
    fn build(ref this) -> facet::Node {
        return @facet {
            label("").key("n").bind_text(this.n_text)
            spinner().key("spinner").bind_shown(this.busy_now)
            button("+").on_click(this.inc)
        };
    }
}

impl Counter: bound::Bound {                 // (d) the wiring, always this line
    fn on_value(ref this, field: str) { bound::changed(this); return; }
}
```

Then `run_component(Counter { ... })` as usual. There is no `bind` call, no
binding list, and no `Lifecycle` impl.

### 2.1 What each piece does

| Piece | Job |
|---|---|
| `#[watch]` | Makes every field store call `on_value`. Compiler-emitted; see [watch-structs.md](../../../docs/design/watch-structs.md). |
| `bind_*` modifier | Declares that one property of this element is computed by this thunk. |
| a thunk | An ordinary method, `fn(ref this, item: *u8) -> T`. Runs on demand. |
| `on_value` | Always the same line. The barrier's landing point. |
| `mount` | Links each declaration to the view it just created. |

### 2.2 Why the thunk is a plain method

A binding must **recompute** its property at every later push, so what it has to
keep is a way to run the expression again. C+ already has one: a method, passed
as a bound reference.

```cplus
label("").key("n").bind_text(this.n_text)
```

`this.n_text` in value position is validated by sema against the modifier's
fn-pointer type, lowered to a synthesized erased bridge, and the receiver's
address is filled into the modifier's trailing `cp` slot — the identical
mechanism `on_click(this.inc)` has always used. This is its second, unrelated
consumer, which is the best evidence the primitive belongs where it is.

That buys three things at once:

- **Nothing new in the language.** No intrinsic, no parser change, no sema rule.
  An `#bind(expr)` form capturing an inline expression would be *sugar* over
  exactly this, and sugar does not earn language surface. The day a profile shows
  thunk re-evaluation dominating and the fix needs to know which fields a thunk
  reads — that is genuine introspection no package can do, and it earns a `#`
  then.
- **It is checked.** `this.nope` does not resolve; a thunk with the wrong return
  type is a type error at the modifier.
- **It is testable.** `assert c.n_text(0 as *u8).equals("count: 5")` is a real
  unit test. An inline expression could never be one.

The thunk's shape is `fn(item, cp) -> T`, mirroring a handler's `(sender, ctx)`.
The receiver comes **last** because that is where the compiler appends it. The
`item` slot is the node's `.item()` pointer — the same per-row channel handlers
use — so one method serves every cell of a grid.

### 2.3 The binding kinds

| Modifier | Thunk returns | Element effect |
|---|---|---|
| `.bind_text(f)` | `text::Text` | `set_text` |
| `.bind_value(f)` | `f64` | `set_value` — slider / stepper / progress / gauge |
| `.bind_on(f)` | `bool` | `set_on` — toggle / checkbox / switch |
| `.bind_hidden(f)` | `bool` | `set_hidden(v)` |
| `.bind_shown(f)` | `bool` | `set_hidden(!v)` |

There is no formatting language and no numeric kind zoo, because the thunk is an
expression: `"count: ${this.n}"`, `"${this.first.view()} ${this.last.view()}"`,
`this.done as f64 / this.total as f64` all just work. A value computed from two
fields needs no derivation concept — it is one method reading two fields.

`bind_hidden` / `bind_shown` are what keep conditional display out of the
structural path: a hidden element is a scalar push, not a rebuild.

## 3. What makes it cheap: comparison, not dispatch

`on_value` reports *which field* was written. The rows deliberately ignore it.
Each row caches the value it last pushed; a sync re-runs the component's thunks
and pushes only on a real difference.

Ignoring the field name buys three things:

1. **`"*"` needs no special case.** A whole-struct assign fires the barrier once
   with `field == "*"`
   ([watch-structs.md](../../../docs/design/watch-structs.md) §2.4); a value
   comparison lands it on exactly the properties that moved.
2. **Idempotence.** `n = n`, or a handler that rewrites three fields of which two
   were already correct, costs zero native calls.
3. **Reentrancy converges.** A push that somehow provoked another write would
   re-enter; the caches make the second pass a no-op.

**No read-set, on purpose.** Knowing which fields a thunk reads would only let
us skip work — it can never change the result, because the cache already decides
what reaches the screen. Tracking it is the first step toward a dependency
graph, so it stays untracked until a measurement demands it.

The cost is one pass over that component's rows per write: for each, running a
thunk and comparing. A `bind_text` thunk allocates a `Text` per sync even when
nothing changed. At five to ten bound properties per component that is nothing;
for a writer touching a field in a loop, `suspend` / `resume` is the answer, and
the caches make `resume` paint exactly the net delta.

## 4. Lifetimes, and why they are structural

### 4.1 A row cannot outlive its view

`mount` registers a row against the view it just created, and the backend's view
release drops it — the same call destroys both. There is no discipline to
follow and no teardown hook to forget.

The other pointer a row holds is the component (the thunk's receiver). A `build`
that hands out `this.method` is receiver-capturing, and sema already refuses to
let such a method be called on a local, so that half is checked by the compiler.

### 4.2 A binding cannot fail to resolve

The key never enters the update path. A row holds its view directly, because the
view was in hand at the moment the link was made. So there is no lookup to miss,
no `unresolved` count to check, no mistyped-key class, and no window in which an
element shows a placeholder.

Keys remain exactly what they are for: `find`, agents, MCP.

### 4.3 The initial paint waits for the backend

Registration happens during the tree walk; the first push does **not**. A push
runs the backend's mutator, and a mutator reflows the owning tree — which does
not exist yet while the walk is still running. Rows stay unprimed until the
backend calls `facet::paint_new_bindings()`, which it does once the mounted tree
is stored and reachable (`ui::set_tree_mounted_fn`).

This was found the hard way: painting at the end of facet's own recursion is
still too early, because the backend has not stored the tree at that point.

### 4.4 A parked component keeps painting

Facet retains a parked component's views, so its rows stay valid and keep
updating off-screen. Re-attaching shows current state with no re-registration
and no repaint pass.

## 5. What this is not

**Not reactive.** No dependency graph, no recompute of anything larger than one
property, no vdom, no diff of a tree. `build` still runs exactly once and the
update is the same in-place native write a handler would have made by hand. All
that moved is *who* makes it.

The one honest concession: there *is* a diff — at the leaf, over scalars,
against a slot list fixed before the write happens. Diffing a handful of cached
values is a different object from reconciling an unbounded tree.

**Depth 1, forever.** A thunk reads state and returns a value. It never reads
another thunk's output. That rule, and not the absence of a vdom, is what keeps
this from being a graph — and it is not negotiable.

**A thunk computes a property, never children.** Re-evaluating structure from a
data comparison is reconciliation. Collections stay `ui::list`, `add_child`,
`switch_to`. If `@facet` ever grows `for row in this.rows` with re-evaluation,
this tier has become React from the other end.

**Not two-way.** State → view only. The element is a *projection* of the field;
nothing reads it back. A control's own edits arrive as handlers, as always.

**Not a replacement for the normal tier.** Anything a binding kind does not
cover — `set_text_spans`, structural verbs, a native reach-through — is still a
direct `find(key)` call. The two mix freely, subject to §6.

## 6. What a bound property costs you

**Opting in is per struct, not per field.** `#[watch]` is a struct attribute, so
once it is on, every field write in that component runs one pass over its rows,
including fields nothing is bound to.

**A bound property belongs to its binding.** Every *other* property of the same
element stays hand-driven: `find("n").set_style(...)` alongside a bound `text`
is fine, because the two never touch the same thing. What breaks is
`find("n").set_text(...)` on a bound `text` — the element changes without
telling the row, and the row's belief about the screen is now false in the
"already correct" direction, so it suppresses its own correction.

The cache models the *screen*, not the state. The alternative — comparing
against the element's real value via `Handle::text()` — self-heals, at the price
of a native read per row per sync. That is a bad trade at UI write rates, so the
escape is explicit: `bound::invalidate(this)`.

## 7. Coverage

`vendor/facet/src/bound.cplus` — 16 tests against a recording stub backend:
initial paint, field write, handler-writes-state-only, a thunk spanning two
fields, push counting (one write = one push), same-value suppression,
whole-struct assign, value and all three bool polarities, a per-item thunk
reading `.item()`, view-release stops every push, two instances painting
*different* elements, manual `changed` for interior writes, `invalidate` after
an out-of-band write, `suspend`/`resume` coalescing a 100-iteration loop, a node
that declares nothing registering nothing, and no-backend-installed.

`vendor/facet_appkit/src/facet_appkit.cplus` — 4 end-to-end tests against real
NSViews:

- a field write moves a real `NSTextField`'s string (read back through
  `Handle::text()`, which asks the located view, not a facet-side cache) and a
  real view's `hidden` flag; then `unmount_all` drops the rows with the views;
- unmount and remount repaints the fresh tree from current state;
- **the whole chain with nothing simulated but the click** — an `NSButton`'s
  wired target/action is fired directly, which runs the bound-method handler,
  whose field store trips the compiler's barrier, which reaches `on_value` →
  `changed` → the row's thunk → the real label;
- a direct `set_text` on a bound property goes stale until `invalidate`, while a
  `set_background` on the same element coexists with the binding.

`examples/bound_counter` — a runnable window, rebuilt against this surface.

## 8. Open

1. **Interior writes need a manual `changed`.** `this.rows[i] = x` and
   `(*p).v = 1` do not fire the barrier (that is `#[watch]`'s own open
   question), so a component doing either calls `bound::changed(this)`. Fixing
   it upstream fixes it here.
2. **No read-set.** §3. Additive, gated on a measurement.
3. **`bind_*` covers five properties.** Style bindings (background, foreground,
   corner) are the obvious next set and cost one row kind each. Nothing in the
   registry is property-specific beyond the push switch.
4. **A slotless `mount_into` paints at its own tail.** Two call sites in the
   backend rather than one; a third mount path would need the same line. A
   backend that forgot it would mount blank elements, and nothing would say so.
5. **Only the AppKit backend is wired.** A backend owes two calls:
   `facet::paint_new_bindings()` once a mounted tree is stored, and
   `facet::forget_view_bindings(view)` when it releases a view. `facet_gtk` has
   neither — it does not currently build, for reasons that predate this work
   (`gobject_gir` trips E0917), so it was not wired.
