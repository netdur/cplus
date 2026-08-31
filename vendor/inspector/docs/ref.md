# Reference

Manual for the `inspector` package. Signatures and behavior only. Recipes are in
[tutorial.md](tutorial.md), judgment in [guide.md](guide.md), the JSON-RPC
namespace in [wire.md](wire.md).

```cplus
import "inspector/widget" as panel;          // the panel, and the host API
import "inspector/remote" as remote;         // the vtable over a wire

import "agent_core/inspect" as insp;         // the neutral surface
import "facet_agent/inspect_tree" as itree;  // the facet walker
import "agent_mcp/inspect" as imcp;          // the verbs
```

WHERE EACH LIVES IS THE ARGUMENT. `agent_mcp` serves the fourteen verbs and must
not depend on a toolkit, so the types those verbs carry cannot either — which is
why the neutral surface is in `agent_core` and the facet walker that fills it is
in `facet_agent`. What is left in this package is the panel.

## Cross-cutting contracts

- Every verb that takes a `Handle` checks liveness first. A handle issued before
  any removal, anywhere, answers `Outcome::Stale`.
- Every write goes through facet's own setters: prop, dirty bit, scheduled sync,
  backend apply. Nothing writes a platform view directly.
- Reads and writes must happen on the UI thread. The serving facade installs
  the hop that lets a socket consumer satisfy that; nothing an application
  writes has to.
- Property names handed to `set` / `reset` / `is_overridden` must be literals
  from `declared_names` or `computed_names`; exchange a caller's name through
  `canonical_prop` first.

---

# `agent_core/inspect`
<!-- was `inspector/inspector` (retired) -->

The neutral surface. Names no backend, no toolkit and no platform — which is
what lets `agent_mcp` serve the verbs that carry these types.

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

# `facet_agent/inspect_tree`
<!-- was `inspector/tree` (retired) -->

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

### `selection` / `selected_handle` / `node_count` / `row_count` / `shown_count`

```cplus
fn selection(this) -> text::Text          // the node id, or empty
fn selected_handle(this) -> insp::Handle
fn node_count(this) -> usize
fn row_count(this) -> usize
fn shown_count(this) -> usize
```

`selection` answers the ID (below); `selected_handle` is the opaque, for the
panel's own machinery. `node_count` and `shown_count` are the same number — how
many rows reached the tree model — and `row_count` is the last snapshot's size,
which is larger when the panel filters its own subtree out.

## The host API

What an application EMBEDDING the panel drives it with. It exists because the
alternative is what a host writes instead: a probe before a connect, a
`Backend::none()` swapped in by hand to disconnect, a bool tracking whether the
panel is remote, and a timer polling `Remote::fault()` to find out something
went wrong. All four are the panel's own state, read from outside because it
had no way to say them.

### A node's ID

`kind#key` — `button#tap`. Not a new spelling: it is what the change ledger
labels a row with, what the journal prints, and what a tree row shows. So an id
from `on_select` matches the `node` on a `Change` from `on_change`, and both
match a key written in the source. That correlation is why a host wants an id.

**An unkeyed node has none** and answers empty. It is not addressable in the
ledger or the journal either, and a positional id would name a different node
after the next insert.

### Transport

```cplus
async fn connect(st: *Inspector, take target: text::Text) -> status::Status
fn disconnect(st: *Inspector)
fn is_connected(st: *Inspector) -> bool
fn target(st: *Inspector) -> text::Text
fn refresh(st: *Inspector)
fn discover(app_id: str) -> vec::Vec[text::Text]
```

`connect` takes four forms and the **scheme picks the transport**:

| `target` | |
|---|---|
| `inapp` | this process. No transport, and `is_connected` stays false — there is nothing to disconnect |
| `/tmp/mcp-app-123.socket` | a Unix socket |
| `9123` | a loopback port. What a phone reports |
| `http://127.0.0.1:9123/` | the same JSON-RPC, POSTed |

| Status | |
|---|---|
| `Ok` | attached, and the tree is read |
| `InvalidInput` | not an address this panel can read |
| `OutOfBounds` | nothing is listening there, **or** something is and it never answered |

`fault()` carries the sentence either way, so a host shows one string whichever
answer it got — and the two `OutOfBounds` cases say which, because they send a
developer to different places. "Nothing is listening there" means the app is not
running or the address is stale; "something accepted the connection there and
never answered" means the door belongs to someone else, which on a device is
the port forward standing up for an app that is not serving.

**Async**, and for two cases: a target that is not there, and a target that
answers the door and then says nothing. `connect(2)` waits for the network stack
to give up; a peer that accepts and stays quiet waits **for good**. Either on
the UI thread is a frozen window, so both are spent on a worker — which is why
`connect` asks the target a question (`initialize`) rather than merely opening a
socket. Everything else answers on the first poll.

**Every call has a deadline**, `remote::CALL_TIMEOUT_SECS` — five seconds,
per `Remote` via `timeout_secs`. Seconds rather than milliseconds because a
device at the end of a cable is legitimately slow, and a deadline that fires on
a slow answer turns a working target into an intermittent one. An expiry takes
the path every other transport failure takes: a fault, and `Stale`.

What `connect` does **not** make async is the rest of the session: every verb
after it is a synchronous connect-write-read on the calling thread. That is fine
for a socket on the same machine, and it is now bounded rather than open-ended
for one that is not — a target that goes quiet mid-session costs five seconds
and then reports a fault, instead of taking the window with it. Making the
session itself async is the thing to look at if a panel is ever pointed across a
network.

**An owned target, not a `str`**, because an `async fn` cannot take a borrow: a
coroutine frame outlives the call that made it (E0900).

`discover` answers where a named app is listening, newest first, and every entry
is something `connect` takes — a live socket's path, and the address a
descriptor states, which is how a phone's loopback port gets here. It is
re-exported from `agent_mcp`, whose convention it is; a host writing its own
`/tmp` globber gets `mcp-<id>-<pid>` wrong for an id containing a dash and
attaches to a dead socket.

`disconnect` drops the selection, empties the panes and clears the target.

### Being told

```cplus
fn on_fault(st: *Inspector, f: fn(str, *u8), ctx: *u8 = 0 as *u8)
fn on_change(st: *Inspector, f: fn(insp::Change, *u8), ctx: *u8 = 0 as *u8)
fn on_select(st: *Inspector, f: fn(str, *u8), ctx: *u8 = 0 as *u8)
```

The context is LAST, which is the shape a bound method binds to: a host writes
`panel::on_select(st, this.jump_to_key)` and the compiler pairs the method with
its receiver. Zero means nobody is listening.

- **`on_fault`** — a transport failure or a refusal, with the sentence. This is
  the one that matters most: a refused write shows up as a field that snaps back
  to its old value, and the sentence saying why reaches nobody.
- **`on_change`** — an edit LANDED, with the value the tree actually holds, not
  the one asked for. `Change` is `{node, prop, now}`, so a host can write it
  back into source.
- **`on_select`** — the selection changed, by id. Announced only when it
  actually changed, so `connect` (which disconnects first) does not report a
  selection clearing that never happened.

### Reads

```cplus
fn fault(st: *Inspector) -> text::Text
impl Inspector { fn changes(this) -> vec::Vec[insp::Change] }
```

For a host that would rather pull than subscribe. `changes` is read through the
vtable, so it is the ledger of whatever the panel is ATTACHED to rather than of
the process the panel is drawn in — which is what the Copy button got wrong.

### Actions

```cplus
fn select(st: *Inspector, id: str) -> bool
fn highlight(st: *Inspector, id: str) -> bool
fn clear_highlight(st: *Inspector)
fn reset_all(st: *Inspector)
```

`select` is the other half of `on_select`, so host and panel stay in step both
ways. An id nothing answers to is `false` **and a fault with a sentence**, not a
silent no-op; the selection it had is kept.

`highlight` draws the box on the running app WITHOUT moving the selection — for
showing a person which node is under discussion.

`reset_all` puts every property on the selected node back to what the
application built it with. It has been a button since the panel had one; this is
the same thing with a name.

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

# `facet_agent/inspect_platform`
<!-- was `inspector/appkit` (retired) -->

THE PLATFORM HALF: the two things facet's tree cannot answer, plus the thread
hop. Resolved per platform — `inspect_platform.cplus` on macOS,
`inspect_platform_ios.cplus`, `inspect_platform_android.cplus`.

**An application does not call this.** `facet_agent`'s serving facade installs
it beside the walker, so `runtime::agent_mcp(id)` is the whole opt-in. It is
documented here because a reader tracing an overlay ends up in it.

### `install`

```cplus
fn install()
```

Installs the highlight overlay, the native property rows, the clipboard, and the
synchronous UI-thread hop the verbs need.

### `run_on_main_sync`

```cplus
fn run_on_main_sync(ctx: *u8, work: fn(*u8))
```

Runs `work(ctx)` on the main thread and returns when it is done — directly when
already there, otherwise through `dispatch_sync_f` on the main queue.

---

# `agent_mcp/inspect`
<!-- was `inspector/mcp` (retired) -->

The fourteen verbs. Protocol in [wire.md](wire.md).

**Published unconditionally** by `agent_mcp` when it starts serving, so an app
that calls `runtime::agent_mcp(id)` is inspectable with no second call. What
`set_backend` supplies is the WALKER the verbs read through — the serving facade
does that too. A surface with no walker answers a policy refusal naming what is
missing, never `-32601`.

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

How many rows the last `describe_tree` issued addresses for.

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
| Modules | `inspector/widget`, `inspector/remote`. The surface, verbs, walker and platform halves live in `agent_core`, `agent_mcp` and `facet_agent` — see the header |
| Dependencies | `stdlib`, `flex_layout`, `facet`, `objc`, `appkit`, `quartzcore`, `json`, `agent_mcp`, `agent_core` |
| Platform notes | The walker is portable; `facet_agent/inspect_platform` is the platform half and resolves three ways — macOS, iOS, Android. Where the process LISTENS is `agent_mcp`'s business and differs — a Unix socket at `/tmp/mcp-<id>-<pid>.socket` on a desktop, a loopback port `9000 + pid % 1000` on iOS and Android, because a socket inside the sandbox is unreachable from the development machine. Both are derived from the pid, so a launcher can compute the address from the process it spawned. On Android the app also needs `android.permission.INTERNET`, without which the bind fails and nothing listens |
| Tests | `src/test_main.cplus` — `cd vendor/inspector && cpc test` |
