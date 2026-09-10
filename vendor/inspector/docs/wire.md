# The inspector's verbs on the wire

The inspector over a socket, as a second namespace inside the **agent's**
JSON-RPC server. There is no separate inspector server and no second port.

**The verbs carry no prefix.** They used to — `inspector.describe` (retired) and
the rest — and the prefix went when both halves walked the same tree: one flat
`tools/list`, one namespace. `describe_tree` is the one name that is not a plain
de-prefixing, deliberately, so it does not collide with the agent surface's
`describe_ui`.

The design notes for why it is shaped this way are
[design.md](design.md#one-server-two-capability-models); this file is the
protocol.

## Getting them

Nothing. **Serving is the whole opt-in.**

```cplus
import "facet_runtime/runtime" as runtime;
import "facet_agent/agent" as agent;

fn main() -> i32 {
    agent::enable();
    runtime::agent_mcp("iris");     // 25 verbs: 11 core, and the 14 below
    ...
}
```

There used to be a second call — `inspect::arm()` (retired) — and forgetting it
answered `-32601 method not found`, which is the wrong sentence: `-32601` means
THE VERB DOES NOT EXIST, and what was meant was that this endpoint had no tree
installed. A client cannot tell those apart, and three packages grew machinery
to cope with it.

The serving facade installs the walker and the platform half (the overlay, the
native rows, and the synchronous UI-thread hop `core::touch` requires — without
it a write arriving on the server's thread aborts the process). An endpoint with
no walker still SERVES these verbs and refuses them with a policy answer naming
what is missing.

**What gates them is the grant**, which is where it belongs. The eight that
rewrite the tree cost `cap_edit_tree`; an app that wants a reader hands out
`auth::operator()` from its own `set_policy`.

## Addressing

A `Handle` is a node pointer and a pointer does not survive a socket, so
**`describe_tree` issues the addresses** and every other method takes an
index into that listing.

Every answer carries `revision` — flex's global removal counter, the same number
node-handle liveness is one comparison against. **Any removal anywhere
invalidates every index.** A client that sees `revision` move, or gets
`"outcome": "stale"`, re-describes. Inserts do not move it.

## Methods

| Method | Params | Answers |
|---|---|---|
| `describe_tree` | `prefix?` | `{ revision, nodes: [...] }` |
| `inspect` | `node` | `{ revision, node, declared, computed, native }` |
| `set` | `node`, `property`, `value` | outcome |
| `set_many` | `node`, `properties` | outcome per property |
| `reset` | `node`, `property` | outcome |
| `nudge` | `node`, `property`, `by` | outcome, and the value it landed on |
| `insert` | `node`, `element`, `key?`, `text?`, `at?` | outcome |
| `remove` | `node` | outcome |
| `reparent` | `node`, `parent`, `at?` | outcome |
| `undo` | — | outcome |
| `highlight` | `node` | outcome |
| `clear_highlight` | — | outcome |
| `vocabulary` | `element?` | `{ elements: [...], properties: [...] }` |
| `journal` | — | `{ lines: [...] }` |

Fourteen. `set_many` and `nudge` are the two a client dragging an element wants:
placing one usually takes three properties, and each on its own is a round trip.

`describe_tree` takes an optional `prefix` and you should pass it. Without one it
answers the WHOLE developer tree — measured at 80,477 characters against a real
app, which an agent harness refuses outright.

A required parameter that is absent is `-32602`, naming the method and the
parameter. It is not defaulted: every reader here falls back, so an absent
`node` would silently act on index 0 — the window root — and this namespace can
delete what it hits.

### A node in the listing

```json
{ "index": 7, "parent": 3, "depth": 2, "kind": "label", "key": "subtitle",
  "children": 0, "has_view": true, "attached": true, "focused": false,
  "visible": true, "frame": { "x": 0, "y": 24, "w": 220, "h": 17 } }
```

`has_view: false` is a node a walk of the platform's view hierarchy cannot see
at all — a pure-layout container, a span inside its label's string, a menu item
inside its menu. Listing them is the whole argument for walking facet's tree.

`is_inspector: true` marks the panel's own subtree when one is embedded. It is
marked rather than hidden: a remote client is a different consumer and may
legitimately want to see it. Editing it is refused independently.

### `inspect`

Three lists, never flattened into one. Setting `width` writes a style; reading
the frame reads what layout decided. A single `width` row would be a number the
client cannot write back and cannot explain.

- `declared` — facet props and flex style. Writable.
- `computed` — the laid-out frame, attachment, focus. Read-only.
- `native` — the platform's own, and empty with no platform module installed.

Each row is `{ name, type, value, writable, overridden }`. `type` is the tag a
write to that name will take, so a client never has to infer it from the value
the property happens to be holding.

### Values

`value` is a bare JSON scalar, read as the tag the property already holds — the
same rule the embedded panel's text fields use, from the same code.

| Property tag | Send |
|---|---|
| number | `40`, `0.65` |
| bool | `true` |
| text | `"hello"` |
| color | `"#2277ff"` |

That is what makes `"#2277ff"` a colour on a colour property and seven literal
characters on a text one. A value of any other JSON type is `-32602`.

Colours come **back** by what they are, never by inventing channels:

```json
{ "token": 255, "rgba": [0.13, 0.47, 1, 1] }       // a literal
{ "token": 254, "light": [...], "dark": [...] }    // an adaptive pair
{ "token": 7 }                                     // a theme ROLE — no channels yet
```

A property that is unset reads as `null`, not `0`. `auto` has no number to hand
back unchanged, and `0` is a value a client would write back as a real zero.

### `insert`

`element` is one of `vocabulary`'s names, spelled as `facet/elements`
spells the function. `text` is the one string the makers that take one take — a
label's text, a button's title, a field's initial text. `at` defaults to append.

A button made this way has **no** `on_click` and cannot be given one: a function
pointer is not a value any inspector can carry. That is where structural editing
stops and code reloading would begin.

### Outcomes

Every non-read answers `{ outcome, reason?, revision }`. The name is for
matching, the sentence is for reading a transcript.

| `outcome` | Means |
|---|---|
| `ok` | |
| `stale` | the tree changed since the listing — re-describe |
| `not_found` | no such index, or no such node |
| `unknown_property` | no property of that name here |
| `type_mismatch` | that property does not take that tag |
| `read_only` | real, and not writable (a computed frame) |
| `unsupported` | the name was right; this build needs a key on this node |
| `not_a_container` | that node does not take inserted children |
| `protected` | a window root, or the inspector panel's own subtree |
| `unknown_kind` | no element of that name can be made |
| `cycle` | that move would put a node inside itself |
| `nothing_to_undo` | nothing removed, or its parent has since gone |

`unknown_property` and `unsupported` are different answers on purpose: "you
misspelled it" against "the name was right and this build cannot reach it here".
So are `read_only` and `unknown_property`: "layout decided that" is not "no such
thing".

### `journal`

The session's structural edits, as the C+ that would make them:

```
mount::add_child(mount::node("card"), ui::label("hello", key: "made"));
mount::remove("made");
```

An edit under an unkeyed node comes back as a comment saying it cannot be
addressed. Inventing a key would produce a line that compiles and edits a
different node.

The journal is volatile and is not replayed. An inspector that reapplied its own
structural edits at startup would have become the part of the application that
builds the UI.

## Consent

The agent's `AuthGate` runs **first** and covers this namespace too. Arming opens
a door, not a bypass — a denied gate answers `-32001` and the handler never runs.
The socket's filesystem permissions remain the outer boundary.
