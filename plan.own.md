# C+ Ownership-Drop Plan (`own` marker + auto field-drop)

Status: **design complete, implementation deferred.** This document captures the
design and its rationale so the reasoning survives until a focused
implementation cycle. It should land between llama.cplus port milestones, not
mid-stream, because the core is a global change to drop semantics.

## 1. Problem

C+ runs `drop` on scope exit and auto-frees `string` / `Vec` *bindings*, but it
does **not** recurse into struct fields. From `docs/design/phase3-drop.md §5`,
quoted in sema: "a destructor is responsible for freeing its own fields by hand;
the compiler does not synthesize per-field drops."

Two consequences:

1. A struct holding an owning field (`string`, `Vec[T]`, `Box[T]`, another Drop
   struct) and no `drop` method silently leaks that field. Unlike Rust, nothing
   warns; unlike a binding, nothing auto-frees.
2. The author cannot tell, from a raw `*u8` / `i32` field, whether it is an
   owned resource (needs `free`/`close`), a borrowed view, or a plain integer.
   The compiler cannot tell either, which is why raw pointers already sit behind
   `unsafe`.

The motivating example (tutorial §13): `struct Buf { ptr: *u8, len: usize }`
with a hand-written `drop` calling `free(self.ptr)`. The `free` is genuinely
required, but nothing forces or checks it, and the owning-field case is worse
because it is invisible.

## 2. Design

Two mechanisms, split by what the compiler can know.

### 2.1 Auto-drop of owning fields (type-driven)

A struct's teardown recurses into fields whose type the compiler knows is
owning: `string`, `Vec[T]`, `Box[T]`, a struct/enum with its own `drop`, and
arrays/enums of those. No annotation; the type carries the drop logic.

Order: run the user's `drop(mut self)` body first (so fields are still live and
readable inside it), then drop fields in **reverse declaration order**
(construct forward, tear down backward). This matches the C++/Rust convention
and is the recommended decision for the open question in §6.

### 2.2 `own` field marker (declaration-driven)

For raw resource fields the compiler cannot reason about, the author declares
ownership with a bare field marker, in the same family as the parameter markers
`mut` / `move` / `borrow` / `restrict` (locked principle #3: borrowing is a
marker, not a type):

```cplus
struct Buf {
    own ptr: *u8,
    len: i32,
}
```

`own` is **check-only**: it declares "this raw field is a resource I own" and
the compiler emits **W0003** if the struct has no `drop` that could release it.
It does not auto-release, because for a raw `*u8` the compiler cannot know the
releaser (`free` vs `close` vs `CFRelease` vs a custom allocator). The author
supplies the fact (ownership); the `drop` body supplies the mechanism. This
mirrors `borrow`, which declares "read-only" and is enforced, not executed.

`own` is the imperative-verb mood of the marker family and the antonym of
`borrow`: `own ptr: *u8` / `borrow x: T`.

### 2.3 The escape hatch is marker absence

An unmarked raw field is silent: no auto-drop (the compiler does not know how),
no W0003 (the author did not claim ownership). This is the borrowed-pointer,
parent-backpointer, id-integer, and JNI/FFI-handle case. No attribute needed.

### 2.4 `#[no_drop]` is **not** part of this design

Earlier drafts included a struct-level `#[no_drop]` opt-out. It is dropped:

- For raw fields, the opt-out is already free: omit `own`.
- For owning fields you want to transfer out or leak, the case is already
  governed by the existing E0509 rule ("cannot move a field out of a
  Drop-carrying type; clone or restructure"). Cutting `#[no_drop]` removes no
  capability C+ currently has; it declines to add a new one.

If the E0509 migration (§5) proves painful in practice, `#[no_drop]` returns as
an item-level attribute. It would be principled there: it *parameterizes* drop
codegen (skip auto field-drop, lift the move-out ban), the way `#[repr(C)]`
parameterizes layout. It must never *generate* a call (that would be the
`#[drop_with(free)]` form, which violates locked principle #7: attributes are
pure metadata).

## 3. The resulting model

| Field shape | Mechanism | Behavior |
| :--- | :--- | :--- |
| `string` / `Vec` / `Box` / Drop struct | type-driven | auto-dropped (recursive); E0509 governs moves |
| `own ptr: *u8` (raw resource) | bare marker | W0003 unless a `drop` releases it |
| unmarked raw `*T` / `i32` | (none) | silent — the escape hatch |

One new keyword, one new warning, no new attribute.

## 4. Why marker, not attribute

The split that keeps each piece on the right side of the locked principles:

- **Binding-level intent** (param or field) is a bare marker: `mut`, `move`,
  `borrow`, `restrict`, and now `own`. Markers are grammar; they may be
  load-bearing, the way `move` and `drop` already drive codegen.
- **Item-level flags** are attributes: `#[repr(C)]`, `#[no_alloc]`, a future
  `#[no_drop]`. An attribute may *parameterize* existing codegen but may not
  *introduce* a call (principle #7).

`own` is a property of a field binding, so it is a marker, and being a marker it
never approaches the principle-#7 line that an attribute would.

## 5. Implementation surface

Localized to the drop machinery plus a small grammar addition.

1. **`ty_carries_drop` → recursive needs-drop predicate** (sema.rs). Today it is
   shallow: `Ty::Struct → is_drop`, and `string` is not even included. Make it
   report true for `string`, `Vec`/`Box` shapes, structs with an explicit
   `drop`, structs that transitively contain any of these, and arrays of such.
   This one predicate feeds E0509, the Copy computation, and codegen, so fixing
   it once propagates correctly.
2. **`gen_drop_in_place(Ty::Struct)` → recurse** (codegen.rs). After the
   optional user `drop`, iterate fields in reverse declaration order and call
   `gen_drop_in_place` on each owning field. `Ty::String` is already handled;
   reuse it.
3. **`own` field marker** (lexer/parser/ast/sema). Contextual keyword in the
   field-prefix position only (reserve like `restrict` was). Parser allows an
   optional `own` before a field name, mirroring param-marker parsing. AST adds
   a per-field `is_owned` flag.
4. **W0003 check** (sema, near the W0001 lint). For each struct: if any field is
   `own` and the struct has no `drop` method, `cx.warn("W0003", ...)`. Keep it
   binary (has-`own`-field AND no-`drop`); a body scan ("the `drop` exists but
   never touches `self.ptr`") is noisier and deferred.

### Staging

- **Slice 1 (non-breaking): `own` marker + W0003.** Pure addition: a marker that
  currently only drives a warning, plus the warning. No codegen change, no
  semantics change. Can ship independently.
- **Slice 2 (semantics change): recursive `ty_carries_drop` + auto field-drop.**
  Gated behind the §5 audit. This is the breaking part.
- **Slice 3: migration sweep** of stdlib / vendor / port.

Slice 1 alone is lopsided (it warns about a raw `fd` while staying silent on a
leaked `Vec` field, since auto-drop is not in yet), so the value is in shipping
1+2 together. Slice 1 first only to de-risk the grammar change.

## 6. Open decisions

- **Drop order.** Recommended: user `drop` body first, then fields in reverse
  declaration order (§2.1). Document it explicitly; it is invisible until code
  depends on it.
- **Tagged-enum payloads.** Today E0344 forbids Drop payloads in tagged enums.
  Keep that restriction for the first cut; enum-payload drop recursion is the
  harder case and out of scope here.
- **`own` on an owning C+ type** (`own s: string`). Redundant. Do not add a
  dedicated diagnostic; `own` is simply meaningful only where the compiler is
  blind (raw fields).

## 7. Migration audit (proxy)

A read-only sweep of `vendor/` for the affected patterns:

- **Newly drop-carrying structs** (hold an owning field, gain auto-drop): a
  modest set, mainly `vendor/clap` (`App { args: Vec[Arg] }`,
  `ArgMatches { positionals: Vec[str] }`) and similar. These currently leak
  their owning fields; auto-drop fixes that. This is the intended effect, not a
  cost.
- **Actual E0509 violations** (a method moves an owning field *out* of such a
  struct): appear to be few. Most `return self.<field>;` sites across
  stdlib/vendor return Copy values (SIMD lanes in `simd/vec3`/`vec4`, lengths,
  ints), which are E0509-exempt. The exact count requires the recursive
  `ty_carries_drop` to be implemented to enumerate precisely; the proxy suggests
  single digits.

Conclusion: the migration looks small, which supports shipping auto-drop as a
focused cycle without reintroducing `#[no_drop]`. Confirm with an exact pass
once slice 2's predicate exists.

## 8. Backward-compatibility risks

- **Existing leakers** (owning field, no `drop`) start getting cleaned up.
  Strictly better; no observable behavior change beyond memory.
- **Hand-freeing an owning field** in a `drop` body would become a double-free
  under auto-drop. Almost certainly nonexistent (hand-freeing a `string` field
  is awkward enough that nobody does it). Grep `str_ptr(self.` near `free`
  before landing slice 2.
- **E0509 expansion** is the real source-compat surface (§7). Code that moves an
  owning field out of a now-drop-carrying struct must switch to clone or
  restructure.

## 9. New diagnostics

- **W0003** (warning) — `own` field with no releasing `drop`. Silenced by
  omitting `own` (you are not the resource's owner) or by adding the `drop`.
- **E0509** (existing, scope expands) — moving an owning field out of a
  Drop-carrying struct.

## 10. Timing

Implement as a dedicated cycle, landed between llama.cplus port milestones. The
auto-drop slice changes drop semantics globally; landing it mid-port would make
new port failures ambiguous (real cpc gap vs drop-model fallout), which defeats
the port's purpose as a gap detector.
