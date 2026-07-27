# Bound components

> Status: **spike**. Landed as `facet/bound`, behind no flag, additive: nothing
> in the existing tier changed. See §7 for what is unresolved.
> Reading order: [component-model.md](component-model.md) →
> [updates.md](updates.md) → here.
> Depends on: [`#[watch]`](../../../docs/design/watch-structs.md).

---

## 1. Problem

In the normal tier a handler does two things:

```cplus
fn inc(ref this, sender: *u8) {
    this.n = this.n + 1;                                        // 1. state
    let _u: facet::Handle = facet::find("n").set_text("${this.n}");  // 2. view
}
```

Step 2 is the bug surface. It is easy to omit, easy to write against the wrong
key, and it has to be repeated in every handler that touches `n` — so the number
of places that must agree about how `n` is displayed grows with the number of
ways `n` can change. When one of them is missed the app does not crash: a
mutator on a missing or un-updated element is a silent no-op, so the screen just
goes stale, and stale-by-omission reads as a bug in whatever feature owns the
button.

A bound component states the relationship **once**:

```cplus
fn inc(ref this, sender: *u8) { this.n = this.n + 1; return; }   // that is all
```

## 2. Surface

Four blocks. Only the middle two are new, and `impl T: facet::Component` is
untouched — a component is converted by *adding*, never by editing its tree.

```cplus
#[watch]                                     // (a) the barrier
struct Counter { n: i32, note: text::Text, busy: bool }

impl Counter {
    fn inc(ref this, sender: *u8) { this.n = this.n + 1; return; }
}

impl Counter: facet::Component {             // (b) unchanged
    fn build(ref this) -> facet::Node { /* ... .key("n") ... */ }
}

impl Counter: bound::Bound {                 // (c) the declarations
    fn bindings(ref this, b: bound::Bindings) -> bound::Bindings {
        return b
            .int("n", this.n, prefix: "count: ")
            .text("note", this.note)
            .shown("spinner", this.busy);
    }
    fn on_value(ref this, field: str) { bound::changed(this); return; }
}

impl Counter: facet::Lifecycle {             // (d) the seam
    fn on_attach(ref this) { let _m: i64 = bound::bind(this); return; }
    fn on_detach(ref this) { bound::unbind(this); return; }
}
```

Then `run_component(Counter { ... })` as usual.

### 2.1 What each piece does

| Piece | Job |
|---|---|
| `#[watch]` | Makes every field store call `on_value`. Compiler-emitted; see [watch-structs.md](../../../docs/design/watch-structs.md). |
| `bindings` | Declares field → element links. Runs **once per attachment**, not per update. |
| `on_value` | Always the same line. The barrier's landing point. |
| `bind` | Reads the declarations and paints the initial state. Returns the number of keys that did not resolve. |
| `unbind` | Drops the declarations. Required — see §4.2. |

### 2.2 Why each binding takes the field as a `ref`

A binding has to **re-read** its field at every later push, so what it must keep
is the field's *location*. A by-value parameter would freeze a snapshot at
declaration time and the element would never move again; a getter function per
field would work but means writing one, and naming one, for every bound field.

`ref` is the language's existing way to say "pass me the place, not a copy", so
the declarations take the field itself:

```cplus
fn int(this, key: str, ref src: i32, prefix: str = "", suffix: str = "") -> Bindings
//                     ^^^^^^^^^^^^ the caller writes `this.n`
```

That keeps three properties at once:

- **Nothing unsafe at the call site.** The single `#addr_of` per kind lives
  inside `Bindings`, where the retention it implies is the module's documented
  job. App code names no address and casts nothing.
- **It is checked.** `this.nope` is E0320, and a `bool` field passed to `.int`
  is E0302 — the compiler validates the binding, which a field-*name* string
  could never be.
- **The address it captures is stable.** The component is retained at a fixed
  address for the life of its tree — already a rule of the normal tier
  ([component-model.md](component-model.md) §"A composed child must outlive the
  tree") — so a field address inside it is stable too.

All three verified against the compiler: a `ref` parameter does carry the
caller's place (for scalars and for field projections, not a copy the callee
could take the address of), `.int("a", this.nope)` is E0320, and
`.int("b", this.flag)` on a `bool` field is E0302.

The same reasoning applies to `bind` / `unbind` / `changed` / `unresolved`: each
takes `ref c` and recovers `cp` itself, so a component never spells its own
address. `sync(cp)` and `unbind_at(cp)` are the address forms, for an erased
callback that holds a bare `cp` and not the component.

### 2.3 The binding kinds

All take the field as `ref src`, so the call is `.int("n", this.n)`.

| Method | Field type | Element effect |
|---|---|---|
| `.text(key, src, prefix:, suffix:)` | `text::Text` | `set_text` |
| `.int(key, src, prefix:, suffix:)` | `i32` | `set_text`, decimal |
| `.long(key, src, prefix:, suffix:)` | `i64` | `set_text`, decimal |
| `.value(key, src)` | `f64` | `set_value` — slider / stepper / progress / gauge |
| `.on(key, src)` | `bool` | `set_on` — toggle / checkbox / switch |
| `.hidden(key, src)` | `bool` | `set_hidden(v)` |
| `.shown(key, src)` | `bool` | `set_hidden(!v)` |

`prefix` / `suffix` cover "count: 12 items" without inventing a format
language. Anything richer goes through a `Text` field the component formats
itself and binds with `.text` — writing that field fires the barrier, so the
label still updates itself. That is also the answer for an `f64` shown as text:
a float has no single right rendering, so `.value` is numeric-controls-only by
design.

Two bindings may share one key (`.hidden("note", ...)` alongside
`.text("note", ...)`) and one field may drive several keys. The registry is a
flat list; neither side is a map.

## 3. What makes it cheap: comparison, not dispatch

`on_value` reports *which field* was written. The bindings deliberately ignore
it. Each binding caches the value it last pushed, and a sync re-reads the source
and pushes only on a real difference.

Ignoring the field name buys four things:

1. **No unchecked strings.** A field name in a binding table would be spelled by
   hand and validated by nothing — the same silent-miss class that `find(key,
   cp)` was removed for (see the `find` comment in `facet.cplus`). Passing the
   field as a `ref` instead (§2.2) makes the compiler check it.
2. **`"*"` needs no special case.** A whole-struct assign fires the barrier once
   with `field == "*"` ([watch-structs.md](../../../docs/design/watch-structs.md)
   §2.4); a value comparison lands it on exactly the members that moved.
3. **Idempotence.** `n = n`, or a handler that rewrites three fields of which two
   were already correct, costs zero native calls.
4. **Reentrancy converges.** The static reentrancy suppression covers only
   `on_value`'s own body, not helpers it calls, so a push that somehow provoked
   another write would re-enter. The caches make the second pass a no-op, so it
   terminates.

The cost is one pass over that component's bindings per write: a handful of word
compares, no tree walk, no allocation once the text caches have warmed (they are
truncate-and-append, not replace). A component with five bindings pays five
compares to discover that one label needs a new string.

## 4. Why the wiring sits where it does

### 4.1 `bind` is called from `on_attach`, not by a runner

`on_attach` is fired by **every** path that puts a component on screen —
`run_component`, `run_screen`, `present`, `switch_to`, `stage`/`attach`, and an
`App` route ([lifecycle.md](lifecycle.md)). Binding at that seam therefore needs:

- no change to `run_component` or `run_screen` (no second copy of twenty window
  parameters),
- no `Bound` bound threaded through `App`, `present`, or the router,
- no change to `facet.cplus` at all.

The whole tier is one new module. The price is the two lifecycle lines, and a
component that comes on and off screen was implementing `Lifecycle` anyway.

The alternative — a `run_bound_component` plus a bound-aware `present` plus a
bound-aware route — would duplicate the entire runner surface to save two lines
per component, and every future runner would have to be written twice.

### 4.2 `unbind` is not hygiene

The registry holds the component's **field addresses**. A binding left behind
after its component dies is a dangling read the next time anything syncs that
address. `unbind` in `on_detach` is what makes the tier safe, and it is the
reason `bind` is idempotent: re-attaching a parked component re-declares from
scratch, which also re-primes the caches so the freshly mounted tree is painted
from current state rather than inheriting a cache describing views that no
longer exist.

### 4.3 `on_value` lives in the conformance block

`#[watch]` looks the hook up by name on the type and does not care which `impl`
block supplies it. Putting it in `impl T: bound::Bound` makes the interlock
two-sided — omitting it reports both:

```
error[E0361]: `#[watch] struct NoHook` has no `on_value` hook
error[E0503]: `impl NoHook: Bound` is missing method `on_value` required by interface
```

Only the plain hook shape is part of the contract; the snapshot shape is
rejected here (`E0505`). Snapshots exist to coalesce a high-rate writer down to
a repaint tick, which is a different job, and they carry a `Copy` +
pointer-free restriction (`E0363`) that a component holding a `Text` cannot meet
anyway.

## 5. What this is not

**Not reactive.** No dependency graph, no recompute, no vdom, no diff. `build`
still runs exactly once, the tree is still retained, and the update is still
`find(key).set_*()` — the identical keyed-direct write the handler would have
made by hand. All that moved is *who* makes it: one declaration instead of a
line in every handler. Nothing here reintroduces the re-render loop that
[../README.md](../README.md) rejects.

**Not two-way.** State → view only. A control's own edits arrive as handlers, the
same as always; a bound field does not read them back. A handler that wants the
field to track an input writes the field, and the binding closes the loop.

**Not a replacement for the normal tier.** Anything a binding kind does not
cover — `set_style`, `set_text_spans`, structural verbs, a native reach-through
— is still a direct `find(key)` call in a handler. The two mix freely in one
component.

## 6. Coverage

`vendor/facet/src/bound.cplus` — 15 tests against a recording stub backend:
initial paint, field write, handler-writes-state-only, push counting (one write
= one push), same-value suppression, whole-struct assign, `Text` binding, `long`
/ `value`, all three bool polarities, unresolved-key reporting, `unbind` stops
every push, rebinding does not double-register, two instances stay independent,
manual `sync` for interior writes, and no-backend-installed.

`vendor/facet_appkit/src/facet_appkit.cplus` — 3 end-to-end tests against real
NSViews:

- a field write moves a real `NSTextField`'s string (read back through
  `Handle::text()`, which asks the located view, not a facet-side cache) and a
  real view's `hidden` flag;
- a detach/re-attach cycle repaints from current state;
- **the whole chain with nothing simulated but the click** — an `NSButton`'s
  wired target/action is fired directly, which runs the bound-method handler,
  whose field store trips the compiler's barrier, which reaches `on_value` →
  `changed` → `find(key).set_text` → the real label.

`examples/bound_counter` — a runnable window. Its `on_attach` asserts
`bound::bind(this) == 0`, so launching it is itself the check that the lifecycle
seam fires late enough for every key to resolve under the real `run_component`.

`leaks --atExit` over the `facet_appkit` test root reports no allocation
originating in this module (the remaining roots are Apple `NSXPCConnection`
cycles, present before this work and varying run to run).

Negative cases verified by hand and recorded here rather than as tests, because
they are compile failures: missing `on_value` (E0361 + E0503, §4.3) and the
snapshot hook shape (E0505, §4.3).

## 7. Open questions

1. **`impl T: Bound` without `#[watch]` compiles clean and silently never
   updates.** The one real hole. The binding declarations are valid, the initial
   paint works, and every later write goes nowhere, because nothing calls
   `on_value`. Verified: it produces no diagnostic.

   The fix belongs in the compiler, not here — a library cannot see its own
   consumer's attributes. The shape is an inverse of E0361: a struct that
   supplies `on_value` but carries no `#[watch]` is *currently legal on purpose*
   (`unwatched_struct_may_define_on_value_freely` in `sema.rs` pins it), because
   without the attribute `on_value` is an ordinary method. Making it a warning
   when the struct also conforms to an interface that declares `on_value` would
   catch this without special-casing facet. Until then: a one-line test —
   write a field, assert the element moved — catches it immediately, and the
   tests in `bound.cplus` are the template.

2. **Interior writes need a manual `changed`.** `this.rows[i] = x` and
   `(*p).v = 1` do not fire the barrier (that is `#[watch]`'s own open question
   3), so a component doing either calls `changed(this)` itself. Fixing it
   upstream fixes it here.

3. **`unresolved` is advisory.** `bind` returns the count of keys that did not
   resolve, but nothing forces the caller to look. It cannot be a hard failure:
   a key inside a conditionally built branch may legitimately not exist yet,
   which is also why the count stays re-readable afterwards. A `#[test]`
   asserting `bind(...) == 0` is the practical guard.

4. **Declaring bindings in `build` instead.** `.key("n").bind_int(#addr_of(this.n))`
   at the node would put the two halves next to each other and remove the
   `bindings` method. It needs `@facet` DSL work and a place to stash the
   declaration during mount; deferred, not rejected.

5. **No `usize` / `f64`-as-text kinds.** `usize` is common in component state
   (`vec` counts) and currently needs an `i64` field or a cast into one. `f64`
   as text needs a decimals policy. Both are additive.

6. **Registry is a flat `Vec` scanned per sync.** Linear in *all* bindings of all
   live components, filtered by `cp`. At the scale a UI has (tens), that is
   faster than any index. If a screen ever holds thousands of bound components,
   sort by `cp` or keep a per-`cp` offset — but measure first.
