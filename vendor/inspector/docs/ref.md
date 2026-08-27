# Reference

Manual for the `inspector` package. Signatures and behavior only. Recipes are in
[tutorial.md](tutorial.md), judgment in [guide.md](guide.md), the JSON-RPC
namespace in [wire.md](wire.md).

```cplus
import "inspector/inspector" as insp;
import "inspector/tree" as itree;
import "inspector/widget" as panel;
import "inspector/appkit" as iplatform;
import "inspector/mcp" as imcp;
```

## Cross-cutting contracts

- Every verb that takes a `Handle` checks liveness first. A handle issued before
  any removal, anywhere, answers `Outcome::Stale`.
- Every write goes through facet's own setters: prop, dirty bit, scheduled sync,
  backend apply. Nothing writes a platform view directly.
- Reads and writes must happen on the UI thread. `iplatform::install()`
  installs the hop that lets a socket consumer satisfy that.
- Property names handed to `set` / `reset` / `is_overridden` must be literals
  from `declared_names` or `computed_names`; exchange a caller's name through
  `canonical_prop` first.

---

# `inspector/inspector`

The neutral surface. Names no backend, touches no platform.

## `Handle`

```cplus
struct Handle { opaque node: *core::Node, seen: u64, window: usize }
```

Where a mounted node is, and whether that answer is still true. `seen` is
flex's global removal counter at the moment the handle was issued. `window` is
the window root it was reached from, so a keyed re-resolve is scoped to the
right tree.

### `Handle::none`

```cplus
fn none() -> Handle
```

A handle to nothing. Not live.

### `Handle::of`

```cplus
fn of(n: *core::Node, window: usize) -> Handle
```

Issues a handle at the current removal count.

### `is_live`

```cplus
fn is_live(this) -> bool
```

False for a null node, and false once any removal has occurred in any tree since
the handle was issued.

### `is_none`

```cplus
fn is_none(this) -> bool
```

True when the handle addresses no node.

## `Value`

```cplus
enum Value { Nothing, Bool(bool), Int(i64), Num(f64), Str(text::Text),
             Color(vocab::Color), Enum(text::Text) }
```

The tagged vocabulary an inspector can carry. `Enum` holds a member's spelling
rather than its discriminant, so a client never has to know the numbering and a
rename is caught rather than silently accepted.

### `type_name`

```cplus
fn type_name(this) -> str
```

`"nothing"`, `"bool"`, `"int"`, `"number"`, `"text"`, `"color"`, `"enum"`.

### `is_nothing`

```cplus
fn is_nothing(this) -> bool
```

### `clone`

```cplus
fn clone(this) -> Value
```

Duplicates the owned `Text` in the `Str` and `Enum` arms. Needed because a
`Value` inside an owning struct cannot be moved out of it.

## `Spec`

```cplus
struct Spec { maker: text::Text, key: text::Text, label: text::Text }
```

What to build, for `insert`. `maker` is a `facet/elements` function name, not a
control kind — kind 0 covers row, column and spacer alike. `label` is the one
string the makers that take one take; the containers ignore it. An empty `key`
is legal and yields an unkeyed node.

### `Spec::of`

```cplus
fn of(maker: str, key: str = "", label: str = "") -> Spec
```

### `clone`

```cplus
fn clone(this) -> Spec
```

## `Prop`

```cplus
struct Prop { name: str, value: Value, writable: bool, overridden: bool }
```

One row of a property listing. `name` is a `str` because every name comes from a
literal table. `writable` is false for everything in `computed` and `native`,
and for a declared property this build cannot write back. `overridden` is true
when an inspector write is currently masking what the application declared.

### `Prop::read_only`

```cplus
fn read_only(name: str, take v: Value) -> Prop
```

### `Prop::editable`

```cplus
fn editable(name: str, take v: Value, overridden: bool) -> Prop
```

## `Outcome`

```cplus
enum Outcome { Ok, Stale, NotFound, UnknownProperty, TypeMismatch, ReadOnly,
               Unsupported, NotAContainer, Protected, UnknownKind, Cycle,
               NothingToUndo }
```

| Variant | Means |
|---|---|
| `Ok` | |
| `Stale` | the handle's generation is dead — re-describe |
| `NotFound` | the handle addresses nothing, or the node is not in the tree |
| `UnknownProperty` | no property of that name on this node |
| `TypeMismatch` | the property exists and does not take a value of that tag |
| `ReadOnly` | real, and not writable through the inspector |
| `Unsupported` | the name was right; this build needs a key on this node |
| `NotAContainer` | that node does not take inserted children |
| `Protected` | a window root, or the inspector panel's own subtree |
| `UnknownKind` | no maker of that name |
| `Cycle` | that move would put a node inside its own subtree |
| `NothingToUndo` | nothing removed, or its parent has since gone |

### `is_ok`

```cplus
fn is_ok(this) -> bool
```

### `describe`

```cplus
fn describe(this) -> str
```

A one-line sentence for a status line or a wire `reason`.

## `Node`

```cplus
struct Node {
    handle: Handle, parent: option::Option[usize], depth: usize,
    key: text::Text, slot: text::Text, kind: u32, frame: Frame,
    has_view: bool, is_attached: bool, is_focused: bool, is_visible: bool,
    dirty: u64, child_count: usize, is_inspector: bool,
}
```

One row of a `describe` snapshot. Flat and parent-indexed, so the same answer
serves an in-process consumer (which follows `handle`) and a transport adapter
(which sends indices). `frame` is flex's answer in the node's parent's
coordinates, not the platform's. `is_inspector` marks the panel's own subtree;
the walk descends into it and marks rather than skipping.

## `Detail`

```cplus
struct Detail {
    node: Node,
    declared: vec::Vec[Prop],   // facet props + flex style; writable
    computed: vec::Vec[Prop],   // frame, attachment, focus; read-only
    native: vec::Vec[Prop],     // platform-owned; empty with no platform module
}
```

## `Backend`

```cplus
struct Backend {
    opaque surface: *u8,
    describe: fn(*u8) -> vec::Vec[Node],
    inspect: fn(*u8, Handle) -> option::Option[Detail],
    set: fn(*u8, Handle, str, take Value) -> Outcome,
    reset: fn(*u8, Handle, str) -> Outcome,
    highlight: fn(*u8, Handle),
    clear_highlight: fn(*u8),
    insert: fn(*u8, Handle, usize, take Spec) -> Outcome,
    remove: fn(*u8, Handle) -> Outcome,
    reparent: fn(*u8, Handle, Handle, usize) -> Outcome,
    undo: fn(*u8) -> Outcome,
}
```

`Copy` — all fields are fn-pointers plus one erased receiver. All rows but
`highlight` and `clear_highlight` are portable.

### `Backend::none`

```cplus
fn none() -> Backend
```

The degraded backend: answers nothing, refuses every write, crashes nothing.
`undo` answers `NothingToUndo`; everything else answers `Unsupported`.

### Call-through methods

```cplus
fn describe_now(this) -> vec::Vec[Node]
fn inspect_now(this, h: Handle) -> option::Option[Detail]
fn set_now(this, h: Handle, p: str, take v: Value) -> Outcome
fn reset_now(this, h: Handle, p: str) -> Outcome
fn highlight_now(this, h: Handle)
fn clear_highlight_now(this)
fn insert_now(this, parent: Handle, at: usize, take sp: Spec) -> Outcome
fn remove_now(this, h: Handle) -> Outcome
fn reparent_now(this, h: Handle, parent: Handle, at: usize) -> Outcome
fn undo_now(this) -> Outcome
```

Sugar so a consumer writes `b.describe_now()` rather than spelling the receiver
at every site.

## `kind_name`

```cplus
fn kind_name(kind: u32) -> str
```

What a control kind is called. Kind 0 answers `"node"` — every container facet
did not generate. Hand-written; the test suite pins every constant against it.

---

# `inspector/tree`

The walker, the typed dispatch, and the structural verbs. Portable.

## Reading

### `describe`

```cplus
fn describe() -> vec::Vec[insp::Node]
```

Every mounted node in every window, in window-open order and then document
order — the same order `mount::find` resolves in.

### `get`

```cplus
fn get(h: insp::Handle, prop: str) -> option::Option[insp::Value]
```

`None` for a dead handle, and for a property this node does not answer.

### `inspect`

```cplus
fn inspect(h: insp::Handle) -> option::Option[insp::Detail]
```

`None` for a dead handle.

### `declared_names`

```cplus
fn declared_names() -> vec::Vec[str]
```

Every writable property name, in the order a UI should draw them: what it is,
then how big, then how it looks.

### `computed_names`

```cplus
fn computed_names() -> vec::Vec[str]
```

The read-only names a developer might type. Answering `ReadOnly` rather than
`UnknownProperty` for these is the difference between "layout decided that" and
"you misspelled it".

### `canonical_prop`

```cplus
fn canonical_prop(name: str) -> option::Option[str]
```

Exchanges a caller-supplied name for this package's own literal, searching
`declared_names` then `computed_names`. `None` for a name in neither.

Required for any name that did not come from those tables: the override ledger
stores the name as a borrowed `str`, so recording a view of a request-scoped
`Text` leaves it holding freed bytes — and the symptom is a `reset` that answers
`Ok` and restores nothing.

## Writing

### `set`

```cplus
fn set(h: insp::Handle, prop: str, take v: insp::Value) -> insp::Outcome
```

Records the application's value on the first write to a given (node, property),
then applies. Checks liveness, then existence, then writability, then type, so
the answer names the first thing that was wrong.

### `reset`

```cplus
fn reset(h: insp::Handle, prop: str) -> insp::Outcome
```

Puts back what the application declared and forgets the record. Resetting a
property that was never overridden is `Ok` and changes nothing.

### `is_overridden`

```cplus
fn is_overridden(n: *core::Node, prop: str) -> bool
```

### `override_count`

```cplus
fn override_count() -> usize
```

### `forget_all_overrides`

```cplus
fn forget_all_overrides()
```

Drops every record **without restoring anything**. For tests and for a session
starting over; not what `reset` does.

## Values

### `parse_as`

```cplus
fn parse_as(current: insp::Value, typed: str) -> option::Option[insp::Value]
```

Reads `typed` as the tag `current` holds. A property whose current value is
`Nothing` is read as a length.

### `parse_for`

```cplus
fn parse_for(h: insp::Handle, prop: str, typed: str) -> option::Option[insp::Value]
```

`parse_as` against a live node's current value. `None` when the node answers no
such property.

### `parse_hex_color`

```cplus
fn parse_hex_color(s: str) -> option::Option[vocab::Color]
```

`#rrggbb`, exactly seven characters. Alpha is 1.0.

## Structure

### `make`

```cplus
fn make(sp: insp::Spec) -> option::Option[core::Node]
```

Builds a node from a spec. `None` for a maker name this build does not know.
A `button` made here has no `on_click` and cannot be given one.

### `maker_names`

```cplus
fn maker_names() -> vec::Vec[str]
```

`label`, `button`, `text_button`, `text_field`, `checkbox`, `row`, `column`,
`spacer`, `box`.

### `can_host_children`

```cplus
fn can_host_children(n: *core::Node) -> bool
```

True for kind 0, `box` and `scroll`. False for a null node and for every leaf
control and self-managing container.

### `within`

```cplus
fn within(root: *core::Node, target: *core::Node) -> bool
```

True when `target` is `root` or anywhere under it.

### `insert`

```cplus
fn insert(parent: insp::Handle, at: usize, take sp: insp::Spec) -> insp::Outcome
```

Builds `sp` and places it at `at`, clamped to the end — so any index past the
last is an append. `NotAContainer` when the parent cannot host children,
`UnknownKind` for an unknown maker, `Protected` inside the panel's own subtree.
Journals the edit on success.

### `remove`

```cplus
fn remove(h: insp::Handle) -> insp::Outcome
```

Takes the node out and **keeps** it, so `undo_remove` can put it back. Views are
detached and handlers notified by `mount::remove_child`. `Protected` for a
window root and for the panel's own subtree.

### `reparent`

```cplus
fn reparent(h: insp::Handle, parent: insp::Handle, at: usize) -> insp::Outcome
```

Moves a live node under a new parent. One verb rather than remove-then-insert,
because between the two halves the subtree is owned by nobody a caller could
name. `Cycle` when the destination is inside the moved subtree, checked before
anything is cut.

### `undo_remove`

```cplus
fn undo_remove() -> insp::Outcome
```

Restores the most recent removal to its original parent and index.
`NothingToUndo` when nothing was removed, or when that parent is no longer in
any open window — and then the subtree stays held rather than being inserted
somewhere unreachable.

### `trash_count`

```cplus
fn trash_count() -> usize
```

### `empty_trash`

```cplus
fn empty_trash()
```

Drops every held subtree without restoring. This is what releases their native
views.

## The journal and the snippet

### `snippet_for`

```cplus
fn snippet_for(h: insp::Handle, prop: str) -> text::Text
```

One property override as a `core::set_*` line. Empty for a node with no key —
a snippet that cannot address its target is not a snippet.

### `journal_count` / `journal_at` / `forget_journal`

```cplus
fn journal_count() -> usize
fn journal_at(i: usize) -> option::Option[text::Text]
fn forget_journal()
```

The session's structural edits as C+ source, in the order they happened. An edit
under an unkeyed node is recorded as a comment saying it cannot be addressed.

## Wiring

### `local_backend`

```cplus
fn local_backend() -> insp::Backend
```

The in-process backend: no transport and none needed.

### `mark_self` / `self_root` / `clear_self`

```cplus
fn mark_self(n: *core::Node)
fn self_root() -> *core::Node
fn clear_self()
```

Marks a subtree as the inspector's own. Sets `Node.is_inspector` on the listing
and makes every structural verb answer `Protected` inside it.

### `install_platform`

```cplus
fn install_platform(highlight: fn(*u8, insp::Handle), clear_highlight: fn(*u8))
```

Zero keeps the portable no-ops, so a build with no platform degrades to "cannot
highlight" rather than to a crash.

### `set_native_fn`

```cplus
fn set_native_fn(f: fn(*core::Node) -> vec::Vec[insp::Prop])
```

Supplies the `native` rows of `inspect`. Zero lists none.

---

# `inspector/widget`

The panel, as a facet `Component`.

## `Inspector`

```cplus
struct Inspector {
    backend: insp::Backend, rows: vec::Vec[insp::Node],
    selected: insp::Handle, shown: usize, hide_self: bool,
}
```

### `new`

```cplus
fn new(take backend: insp::Backend, hide_self: bool = true) -> Inspector
```

### `embedded`

```cplus
fn embedded() -> Inspector
```

`new(tree::local_backend())` — drives the tree in this very process.

### `selection` / `row_count` / `shown_count`

```cplus
fn selection(this) -> insp::Handle
fn row_count(this) -> usize
fn shown_count(this) -> usize
```

`row_count` is the last snapshot's size; `shown_count` is how many rows reached
the tree model, which is smaller when the panel filters its own subtree.

### `build`

```cplus
fn build(ref this) -> core::Node
```

The `Component` impl. Does **not** grow — the host sizes the panel.

### `attach` / `detach`

```cplus
fn attach(st: *Inspector)
fn detach(st: *Inspector)
```

The panel's two lifecycle hooks, exposed as functions because facet fires
`Lifecycle` for the component the runtime owns, not for one nested inside
another's tree. A host that embeds the panel calls both itself. `Inspector` also
implements `component::Lifecycle` for the case where the runtime owns it
directly.

---

# `inspector/appkit`

THE PLATFORM HALF, and it is not macOS only despite the name. The module
resolves per platform — `appkit.cplus` on macOS, `serve_ios.cplus` on iOS,
`serve_android.cplus` on Android — and all three answer the same six entry
points: the two things facet's tree cannot answer, plus the thread hop. The
name predates the second and third backends and is kept because it is what the
docs and the probe already say.

`inspector/serve` is the door an application should use: one `serve_if_asked()`
that resolves to the same three files, so a shared entry does not name a
toolkit.

### `install`

```cplus
fn install()
```

Installs the highlight overlay, the native property rows, and the synchronous
UI-thread hop used by `inspector/mcp`. Call it before serving over a socket.

### `run_on_main_sync`

```cplus
fn run_on_main_sync(ctx: *u8, work: fn(*u8))
```

Runs `work(ctx)` on the main thread and returns when it is done — directly when
already there, otherwise through `dispatch_sync_f` on the main queue.

---

# `inspector/mcp`

The `inspector.` JSON-RPC namespace. Protocol in [wire.md](wire.md).

### `arm`

```cplus
fn arm(take b: insp::Backend)
```

Publishes `inspector.` into `agent_mcp`'s extension hook. Explicit by design: a
process that never calls this answers "method not found" to every method in the
namespace.

### `disarm`

```cplus
fn disarm()
```

Closes the namespace, clears the backend and drops the snapshot.

### `armed`

```cplus
fn armed() -> bool
```

### `snapshot_count`

```cplus
fn snapshot_count() -> usize
```

How many rows the last `inspector.describe` issued addresses for.

### `set_marshal`

```cplus
fn set_marshal(f: fn(*u8, fn(*u8)))
```

Installs the synchronous UI-thread hop. Context first. Unset calls straight
through, which is right for headless tests and for an in-process caller.
`iplatform::install()` sets this.

### `handle`

```cplus
fn handle(method: str, params: json::Value, id: f64) -> json::Value
```

The registered handler. `agent_mcp` has already run the consent gate and matched
the prefix.

---

## Package

| | |
|---|---|
| Name | `inspector` |
| Modules | `inspector/inspector`, `inspector/tree`, `inspector/widget`, `inspector/appkit`, `inspector/serve`, `inspector/mcp` |
| Dependencies | `stdlib`, `flex_layout`, `facet`, `objc`, `appkit`, `quartzcore`, `json`, `agent_mcp`, `agent_core` |
| Platform notes | `inspector/tree` is portable. `inspector/appkit` is the platform half and resolves three ways — macOS, iOS, Android. On iOS and Android `serve_if_asked` takes a PORT rather than a socket path (a Unix socket inside the sandbox is unreachable from the development machine), and on Android the port arrives as the system property `debug.facet.inspect` because an Activity has no environment for a launcher to set; the app also needs `android.permission.INTERNET`, without which the bind fails and nothing listens |
| Tests | `src/test_main.cplus` — `cd vendor/inspector && cpc test` |
