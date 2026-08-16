# `#[watch]` — struct write barriers

> Status: spike. Landed behind no flag; the surface below is what the compiler
> implements today.
> Depends on: [phase5-attributes.md](phase5-attributes.md) (the declarative-only rule).

---

## 1. Problem

A struct's owner often needs to know when its state changed — to bump a
version counter, mark a cache dirty, append to an undo log, forward to
subscribers, or write an audit trail. Today that requires hand-written setters
for every field, and the discipline breaks the moment someone assigns a field
directly. The struct has no way to state "writes to me are observable."

## 2. Surface

```cp
#[watch]
struct Counter {
    count: i32,
    step: i32,
}

impl Counter {
    fn on_value(ref this, field: str) {
        // `field` is the name of the field just written.
        // The new value is read back off `this`.
    }
}
```

After any store to a field of a `#[watch]` struct, the compiler calls that
struct's `on_value` hook with the written field's name.

```cp
var c = Counter { count: 0, step: 5 };
c.count = 10;      // calls c.on_value("count")
c.step += 2;       // calls c.on_value("step")
c.bump();          // a `this.count = ...` inside bump() also calls on_value("count")
```

### 2.1 The hook has two accepted shapes

```cp
fn on_value(ref this, field: str)                              // plain
fn on_value(ref this, field: str, old: Counter, new: Counter)  // snapshot
```

Anything else is **E0362**, and a `#[watch]` struct with no hook at all is
**E0361**. Parameter *names* are the author's; the compiler checks arity and
types. The snapshot types must be the watched struct's own type.

Constraints common to both, and why:

- **`ref this`** — a handler that cannot write is useless for the accumulate
  and forward cases (bump a version, push to a log).
- **`field: str`** — field types differ within one struct, so the changed
  *value* has no single type a fixed signature could carry. The field name is
  the only type-uniform thing available.
- **no return type** — the barrier sits mid-statement and has nowhere to put a
  returned value.

E0361 exists so the attribute is never a silent no-op. A marker whose whole job
is to make writes observable is at its worst when it quietly observes nothing.

### 2.2 Why the snapshot form exists

At the instant the hook runs, `new` and `this` hold the same values, so `new`
looks redundant. They diverge the moment the handler *keeps* one: `this` is a
live borrow, `new` is a frozen value.

That difference is the whole feature. Consider a device writing at 1 kHz whose
consumer only wants to repaint at 60 Hz. The hook stashes a snapshot; the
consumer reads it 16 ms later:

```cp
static PENDING: Sensor = Sensor { reading: 0, seq: 0 };

impl Sensor {
    fn on_value(ref this, field: str, old: Sensor, new: Sensor) {
        if new.reading - old.reading < 10 { return; }  // deadband: most writes die here
        PENDING = new;                                 // frozen at write time; one slot, O(1)
    }
}
```

Had the handler stashed a reference to `this`, the 16 ms read would see the
*latest* value, not the one it was notified about. `old` earns its place in the
same example: the deadband check discards the majority of writes before they
cost anything downstream.

Note the coalescing slot lives **outside** the struct. A `Sensor` cannot
contain a `Sensor` by value (E0913, infinite size) — which is also why the
snapshots are parameters rather than a compiler-maintained shadow field. A
synthesized `previous: Self` field would make every `#[watch]` struct
uncompilable.

### 2.3 Why the snapshot form is restricted (E0363)

A snapshot is handed out expecting it may be held past the write. That is only
sound when a value copy is flat and self-contained, so the struct must be:

- **`Copy`** — rules out owned heap, since `Copy` and `Drop` are mutually
  exclusive; and
- **pointer-free, transitively** — a raw-pointer field is `Copy`, so the Copy
  rule alone would admit it, but its pointee can be freed while a snapshot
  still names it.

Either failure is **E0363**, which points at the plain form. The same struct
may always use the plain hook: no snapshot is taken, so there is nothing to
outlive the pointee.

### 2.4 Whole-struct assignment and the `"*"` sentinel

A store that replaces the entire struct fires the hook **once**, with `field`
set to `"*"`:

```cp
c.a = 10; c.b = 1;        // fires twice — two independent updates
c = C { a: 20, b: 2 };    // fires ONCE, field == "*"
```

`"*"` cannot collide with a real field name, since identifiers admit no `*`,
so a hook can test for it unambiguously. Under the snapshot form it costs
nothing at all: `old` and `new` describe the change completely, and the name
is redundant.

This is also the answer to "how do I get one notification for one logical
update" — group the writes into a single struct assignment.

Only the innermost watched struct is told. `o.leaf = Leaf { .. }` is
syntactically a field store, but semantically it replaces `leaf` wholesale, so
it reports `"*"` on `leaf` rather than `"leaf"` on `o` — matching
`o.leaf.v = 5`, which also tells `leaf` and not `o`.

A `let`/`var` initializer does **not** fire: there is no previous state to
have changed.

> **Corrected 2026-07-26.** This originally did not fire at all, on the stated
> grounds that a whole-struct assign "replaces the observer along with the
> state". That reasoning was wrong: the hook is a method on the *type*, so
> there is no per-instance observer state to replace, and the hook demonstrably
> keeps firing on the same binding afterwards. The real obstacle was only that
> there is no single field name to report — which the sentinel answers. Leaving
> it silent was the worst of the options: a whole-struct assign is plainly a
> state change, and letting it bypass the barrier is the same silent no-op that
> E0361 exists to prevent.

## 3. What fires the barrier, and what does not

Fires:

| Shape | Example |
|---|---|
| Direct field write | `c.count = 10` |
| Compound assign | `c.step += 2` |
| Write from inside a method | `this.count = this.count + 1` |
| Nested watched struct | `outer.leaf.v = 5` (fires on `leaf`) |
| Through a `ref` parameter | `fn f(ref l: Leaf) { l.v = 9 }` |
| Through an array element | `arr[1].v = 7` |
| Whole-struct assign | `c = Counter { .. }` — once, with `field == "*"` (§2.4) |
| Every generic instantiation | `Cell[i32]` and `Cell[f64]` each observe their own |

Does not fire:

| Shape | Why |
|---|---|
| Element write into an array *field* — `d.subs[i] = f` | The barrier covers whole-field stores, not writes *within* a field's interior. |
| Raw-pointer path — `(*p).v = 1` | A raw pointer may not point at a live object; the barrier deliberately stops at the safe-place boundary. |
| Writes inside `on_value` itself | Reentrancy suppression — see §4. |
| A struct without `#[watch]` | No barrier is emitted at all; zero cost. |

The safe-place rule is `place_is_safe_owned`: the receiver must be an
Ident/Field/Index chain. That restriction is also what makes it sound for
codegen to re-lower the receiver for the hook call, since those shapes are
free of side effects when evaluated twice.

## 4. Reentrancy

A hook that writes its own fields must not call itself. The suppression is
**static**: while lowering the body of `on_value`, the barrier is not emitted.
No runtime guard bit exists.

```cp
fn on_value(ref this, field: str) {
    this.version = this.version + 1;   // does NOT re-enter on_value
}
```

This is the same shape as `in_destructor` for `drop`: a compiler-inserted call
must not re-trigger the thing that inserted it.

**Limitation.** The suppression covers the hook's own body only. A hook that
calls a helper which writes an watched field *will* re-enter, and that stays
the author's responsibility.

## 5. Fan-out is library code

The compiler delivers exactly one notification to one place. Multiple
subscribers, queues, and change streams are ordinary code written inside the
hook:

```cp
impl Doc {
    fn on_value(ref this, field: str) {
        this.version = this.version + 1;
        var i: usize = 0;
        while i < this.sub_count {
            let f = this.subs[i];
            f(field, this.version);
            i = i + 1;
        }
    }
}
```

This keeps the compiler's surface at one hook and leaves policy — ordering,
filtering, batching, unsubscribe — entirely in the author's hands. Note that
bookkeeping fields are watched like any other: `subscribe` writing
`this.sub_count` fires the barrier too.

## 6. Relationship to the declarative-only rule

[phase5-attributes.md](phase5-attributes.md) §1 forbids attributes that
generate code, transform the AST, or run user logic at compile time.
`#[watch]` complies: it is a *marker*. Codegen reads it and inserts a call,
exactly as it reads a struct's `drop` method and inserts teardown calls. No
user source is expanded, no method body is synthesized, and the hook is written
by hand.

## 7. Implementation

| Stage | File | What it does |
|---|---|---|
| Attribute spec | `attrs.rs` `KNOWN_ATTRS` | Registers `watch`: no args, struct-only, no duplicates (E0355/E0356/E0357). |
| Flag | `sema.rs` `StructDef::is_watched` | Presence-read at struct collection. |
| Validation | `sema.rs` `check_watch_hooks` | E0361 (no hook) / E0362 (bad signature), after `collect_methods`. |
| Mono | `monomorphize.rs` | Carries the template's attributes onto each instantiation — see §8. |
| Flag mirror | `codegen.rs` `StructInfo::is_watched` | Re-derived from the post-mono AST, never imported from sema (the id-universe rule). |
| Field-name globals | `codegen.rs` `collect_and_emit_str_lits` | Seeds one `@.str.N` per field of each watched struct; the names appear in no source literal. |
| Barrier | `codegen.rs` `gen_assign` / `watched_barrier_for` / `gen_watched_notify` | Decides before lowering, emits after the store, routed through `gen_method_call` so cc and ABI stay in lockstep. |

## 8. A bug this surfaced

`run_monomorphize` rebuilt instantiated `StructDecl`s with
`attributes: Vec::new()`. Any codegen stage that re-derives a type-level flag
from the post-mono AST — `#[lang("string")]` as well as `#[watch]` — saw
*every generic instantiation as unmarked*, while sema had it marked. A flag
drift with no diagnostic.

`StructInstantiationInfo` now carries the template's attribute list and
monomorphize applies it. `watched_generic_struct_keeps_barrier_after_mono`
in `cpc/tests/e2e.rs` is the regression guard.

## 9. Open questions

1. **Bounded history — `#[watch(10)]` — REJECTED 2026-07-26.** The attribute
   would have kept a fixed ring of the last N snapshots. A compile-time N does
   make it cheap in the ways that first suggested it: `[Snap; N]` rather than a
   `Vec`, so no allocation per write, no `Drop`, and the struct stays `Copy`
   (verified).

   It was dropped on measured cost. The ring is N copies of the whole struct,
   **per instance**:

   | struct | bare | with N=10 | growth |
   |---|---|---|---|
   | 2×i32 | 8 B | 92 B | 11.5× |
   | 4×f64 | 32 B | 360 B | 11.2× |
   | 8×i64 | 64 B | 712 B | 11.1× |

   Across 1000 instances that is 92 KB against 8 KB. And it is the wrong shape
   for the case that motivated it: coalescing a high-rate writer down to a
   repaint tick needs **one slot, globally** (8 KB + 8 B — see §2.2), not ten
   per object, because you repaint once. Where per-instance history genuinely
   is wanted, a hand-declared buffer costs 52 KB rather than 92 KB — it stores
   only the field the author cares about instead of whole-struct copies — and
   lets them choose element type, depth, and overflow policy.

   On top of that, the compiler would have to synthesize hidden fields, which
   ripples into struct-literal construction, field indices, generated headers,
   and `#[repr(C)]` layout. The syntax is deliberately left unreserved:
   `#[watch(10)]` reports `E0355: attribute #[watch] takes no arguments`.

2. **Per-field opt-out.** Bookkeeping fields (`version`, `sub_count`) fire the
   barrier like any other, so a hook that writes them must be written with
   care. An unwatched-field marker would remove that hazard.
3. **Interior writes.** `d.items[i] = x` does not fire. Covering it means a
   barrier on `Index` targets whose receiver chain reaches a watched struct.
4. **Backends.** The barrier lives in the LLVM backend only; `wasm_emit.rs`
   does not emit it.
5. **Mutual recursion.** The static reentrancy suppression does not cover a
   hook that re-enters through a helper it calls.
6. **Snapshots on heap-owning structs.** E0363 rules out `#[watch] struct Doc
   { body: Text }` using the snapshot form. Supporting it needs a deep-copy
   story and is a much larger piece of work.
7. **`fields: str[]` instead of `field: str` — DEFERRED.** Reporting a list of
   written field names would subsume the `"*"` sentinel at identical ABI cost
   (`str` and `str[]` are both `{ptr, len}` = 16 B), backed by a static
   constant emitted once per struct, so no per-write construction. It is
   blocked on slice ergonomics rather than on anything watch-specific: slices
   today have no `.len()`, no indexing, and no array→slice coercion, so every
   hook body would need `#slice_len` / `#slice_ptr` / raw-pointer indexing for
   a list that has one element in the overwhelming majority of fires. Worth
   revisiting if slice indexing and `.len()` land — a change that would
   benefit every slice user (`fs`, `net`, `vec::append_slice` all take `T[]`
   and hit the same wall).
