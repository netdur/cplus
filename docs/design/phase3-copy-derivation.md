# Phase 3 — `Copy` Derivation for Aggregate Types

> Status: draft
> Scope: the rule that decides which struct and array types are `Copy`. Companion to [phase3-ownership.md](phase3-ownership.md), which uses `Ty::is_copy()` to decide whether a move actually consumes a binding.
> Out of scope: the `Drop` trait (separate note); generic / parametric `Copy` constraints (Phase 7); user-defined `Copy` opt-out via marker traits (deferred until traits exist).

## 1. Problem

Phase-3 slice 3A landed a conservative `Ty::is_copy()`: primitives + plain enums are `Copy`, structs and arrays are not. That was correct for landing the surface syntax but it leaves a real ergonomic gap — `struct Point { x: i32, y: i32 }` cannot be reused after passing it to a function:

```cp
let p: Point = Point { x: 3, y: 4 };
let d: i32 = distance_from_origin(p);
#println(p.x);  // E0335 today — but p only contains two i32s; *why* is this a move?
```

A primitive-only struct is bit-for-bit copyable. Making it non-`Copy` forces users to write awkward code (re-construct, take `mut p` and pass borrows that don't exist yet, etc.) for no safety benefit.

This note picks a derivation rule that decides when aggregates are `Copy`, locks the rule in, and explains how it interacts with the rest of the language.

## 2. Decision

**Structural auto-derivation.** A type is `Copy` iff every component is `Copy`:

| Type | `Copy` iff |
|---|---|
| Primitives (`iN`, `uN`, `isize`, `usize`, `f32`, `f64`, `bool`, `()`) | always |
| Plain enums (Phase 2A — integer-shaped) | always |
| `[T; N]` | `T` is `Copy` |
| `struct S { f1: T1, ..., fn: Tn }` | every `Ti` is `Copy` |
| Tagged unions (Phase 3 later) | every variant payload is `Copy` |
| Future `Drop` types | never (regardless of fields) |
| Pointers, slices, strings, etc. (later phases) | type-by-type rule |

No user-visible marker — neither `derive(Copy)` nor a `copy` keyword. The compiler computes Copy-ness from the type's structure.

## 3. Rationale

Considered alternatives:

**A. Auto-derive (chosen).** Compiler walks fields recursively; `is_copy()` returns true iff all components are.

**B. Explicit opt-in.** User writes a marker (`copy struct Point { ... }` or similar). Aggregates are non-`Copy` by default.

**C. Hybrid.** Some size threshold below which auto-derive applies. Rejected — "small" is a magic number that becomes brittle.

The choice between A and B is the real decision. Arguments for B (the §2.8 "no-magic, locality of reasoning" stance):

- A reader looking at one site cannot tell whether a value is `Copy` without consulting the type declaration.
- Adding a non-`Copy` field silently changes the type's behavior at every call site.

Arguments for A (chosen):

- **Copy-ness is already structural for primitives.** `i32` is `Copy` without a marker; nobody writes `copy i32`. Extending the structural rule to aggregates is consistent, not new magic.
- **The "silent flip" failure mode is compile-time, not runtime.** If a previously-Copy type becomes non-Copy after a field change, every call site that relied on Copy fails with a precise E0335 use-of-moved. The user fixes it the same way they'd fix any compile error — pointed-to, recoverable, no UB risk. This is a *much* tamer hazard than the usual "silent magic" problems §2.8 is guarding against.
- **The locality argument is weak here.** Type declarations always live elsewhere; reading `let b = a; use(a);` requires knowing whether `a`'s type is `Copy` no matter which rule we pick. Auto-derive moves the answer to "look at the type's structure"; explicit moves it to "look at the type's marker." Both are external; neither is local.
- **Smaller spec.** No marker syntax to design or document, no `derive(Copy)` to bikeshed.
- **Rust convergence.** Rust ended up at auto-derive of `Copy` (modulo the historical `#[derive(Copy)]` attribute, which exists to opt *in*, not to express the structural rule itself). Real-world Rust experience: silent-flip pain is rare and self-healing.

**The decision is reversible.** If experience shows users want explicit opt-in, we can add a `copy struct` keyword later: every existing auto-Copy type that wants to keep its status adds the keyword, and the structural rule becomes a constraint (`copy struct` is only valid if all fields are Copy). No call site changes.

## 4. Semantics

- `Ty::is_copy()` is updated from the Phase-3-slice-3A conservative rule (only primitives + enums) to the structural rule above. The change is invisible to existing Phase-3-slice-3A code paths — `Ty::is_copy()` returning `true` on more types just means more bindings stay usable after a move-positioned call.
- A `move` parameter or `move self` receiver still consumes its source iff the source's type is non-Copy. With auto-derive, more types will be Copy, so fewer real moves happen. This is the *desired* behavior: copying a `Point` doesn't need to consume the source.
- E0336 (lint: `move` on Copy-typed parameter is redundant) becomes more applicable. Still deferred — not blocking this slice.
- Recursive types: structs cannot directly contain themselves (no infinite-size types). Once raw pointers / `*T` exist, `struct Node { next: *Node }` becomes a question — but `*T` is non-Copy by default (Rust's rule — copying a raw pointer is "fine" but you lose ownership clarity), so `Node` would be non-Copy via the pointer field. No special cases needed in this phase.

## 5. Implementation

Single change in `cplus-core/src/sema.rs`:

```rust
impl Ty {
    pub fn is_copy(&self) -> bool {
        match self {
            // Atomic Copy types.
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64
            | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
            | Ty::Isize | Ty::Usize
            | Ty::F32 | Ty::F64
            | Ty::Bool | Ty::Unit
            | Ty::Enum(_) => true,
            // Aggregates: derive from components.
            Ty::Array(elem, _) => elem.is_copy(),
            Ty::Struct(_) => {
                // Cannot resolve without a SemaCx context — see note below.
                unreachable!("call Ty::is_copy_in(cx) for struct types")
            }
            Ty::Error => true,
        }
    }
}
```

`is_copy()` on a bare `Ty::Struct(id)` needs the struct table to look up the field types. Two ways to do this:

- **A.** Pass a `SemaCx` reference: `Ty::is_copy_in(&cx) -> bool`. Forces every caller to thread the context.
- **B.** Pre-compute and cache: when collecting struct fields, also compute a `is_copy: bool` field on `StructDef`. Then `Ty::is_copy()` on `Struct(id)` just reads `cx.structs[id].is_copy`. The caller still needs the cx, but only for the lookup, not for recursion.

Picking **B** because (a) it's amortized across all uses, (b) it matches how struct field lookup already works, and (c) it puts the "is this struct Copy?" question into the same place where field types are resolved.

Computation order: after `collect_struct_fields` finishes (so every struct's fields are resolved), do a fixpoint pass that marks structs Copy iff every field is Copy. One pass is enough for the Phase-3 type system because there's no recursion (no pointers yet, struct fields can't be the enclosing struct). Add a `compute_struct_copy_flags` pass between `collect_struct_fields` and `collect_methods`.

For arrays — `Ty::Array(elem, _)` — the rule is `elem.is_copy()`. Since `elem` can be a `Ty::Struct(id)`, the helper that answers Copy-ness must still take a `cx` reference. So the public API becomes:

```rust
impl SemaCx<'_> {
    fn is_copy(&self, ty: &Ty) -> bool { /* uses struct flag table */ }
}
```

And `Ty::is_copy()` (the bare-`Ty` method) is removed or restricted to types that don't need context. Cleaner: always go through `cx.is_copy(&ty)`.

## 6. Sample programs

### 6.1 Must compile and run (newly accepted under auto-derive)

`docs/examples/copy_struct.cplus`:

```cp
struct Point { x: i32, y: i32 }

fn distance_from_origin(p: Point) -> i32 {
    return p.x * p.x + p.y * p.y;
}

fn main() -> i32 {
    let p: Point = Point { x: 3, y: 4 };
    let d: i32 = distance_from_origin(p);
    // Under slice 3A's conservative rule this would be E0335 on `p.x`.
    // Under auto-derive, Point is Copy (both fields are i32), and `p`
    // remains usable.
    #println(d);            // 25
    #println(p.x);          // 3
    #println(p.y);          // 4
    return 0;
}
```

Expected output: `25\n3\n4\n`.

### 6.2 Must still error (mixed-Copy struct)

```cp
struct Buffer { data: [i32; 4] }   // Array of Copy → Buffer is Copy. Not the case we want.

// Once we have a non-Copy type (e.g. String) we can write:
//   struct Mixed { n: i32, name: String }   // non-Copy
// For Phase 3 we use a struct containing a non-Copy type that already exists:
// nothing yet qualifies. Add the test once we have a non-Copy aggregate
// type beyond an empty marker. (String / Vec etc. are later phases.)
```

For Phase 3 specifically, *every* user-definable aggregate composed of currently-available types is going to be Copy under auto-derive. That's fine — the test of non-Copy aggregates lands when Phase-3 tagged unions or Phase-3 `Drop` types arrive.

### 6.3 Existing samples that are unaffected

[ownership.cplus](../examples/ownership.cplus) currently relies on Buffer being non-Copy (so `move self` actually consumes it). Under auto-derive, `Buffer { data: [i32; 4] }` *would* be Copy, which would silently make `move self` a no-op consumption.

This is the only sample affected. Two options:

- **A.** Keep the sample but note that the move tracking is exercising the Copy fast-path (consumption is a no-op). The negative tests (use-after-move) need a genuinely non-Copy struct to fire E0335 — those tests will need updating.
- **B.** Introduce a placeholder non-Copy mechanism for the sample to demonstrate consumption.

Lean: A. The sample's purpose is to *show* the three receiver kinds; under auto-derive, the consumption is invisible because Buffer is Copy. The negative E0335 tests change to use a struct that's known to be non-Copy. Options for "known non-Copy":
- Wait until a non-Copy aggregate exists in the language (tagged unions w/ heap data, Phase-5+ heap types, etc.) — clean.
- Mark Buffer non-Copy with an attribute or `nodrop` marker — premature.

Sub-decision: **wait**. Phase-3 move tracking still works for *any* non-Copy type the language gains later. For now, the E0335 tests need a synthetic non-Copy type, which we don't have. The slice-3A negative tests will need to be reworked when this slice lands — specifically, they'll move to a follow-up slice that has a non-Copy aggregate to point at.

Alternative: the slice-3A tests can keep their assertion *forms* but their **input programs** will start compiling cleanly because Buffer is now Copy. The tests should be deleted or marked `#[ignore]` until non-Copy aggregates exist.

This is a real cost of auto-derive: the slice-3A move-tracking tests need rework. It's worth it because the test rework is a one-time event, while the ergonomic win (using primitive-only structs naturally) pays off forever.

## 7. Interactions

### 7.1 Phase 3 slice 3A (move tracking)

- Existing E0335 use-of-moved tests against `struct B { x: i32 }` will start passing cleanly because `B` becomes Copy. Rework: rewrite tests to target a non-Copy type, or mark `#[ignore]` until one exists (see §6.3 above).
- Move tracking machinery itself doesn't change — `cx.is_copy(&ty)` just returns `true` for more types, and the consumption code skips those.

### 7.2 Phase 2 samples

- All Phase-2 sample programs that pass structs by value (`point.cplus`, `nested.cplus`, `mutable_struct.cplus`, `methods.cplus`, `array_struct.cplus`) continue to compile. Under auto-derive, those structs become Copy, but the code didn't try to use the source after the pass anyway — no behavior change.

### 7.3 Future `Drop`

When `Drop` lands (separate design note), `Drop` types are non-Copy regardless of fields. The rule becomes: *every component is Copy AND the type does not implement Drop*. A simple flag on `StructDef`; same fixpoint pass adds the `Drop` exclusion.

### 7.4 Phase 7 generics

Generic functions / types can require `T: Copy` as a bound. The auto-derive rule gives concrete types a clear answer; the bound test is `cx.is_copy(&concrete_T)`. No surprises.

## 8. Open questions

- [ ] **`Drop` exclusion** — formal rule that `Drop` types are non-Copy, with a coherence error if someone tries to make a Drop type Copy. Belongs in the Drop design note, not here.
- [ ] **Opt-out marker** — if experience shows users want to declare a Copy-component struct as non-Copy (e.g. for invariant-tracking), how is that expressed? Possible answers: a `nocopy` keyword, a marker field of type `PhantomData<NonCopy>`, requiring an explicit `impl Drop`. Defer until a real use case appears.
- [ ] **Slice-3A test rework** — concrete plan for the E0335 tests that target `struct B { x: i32 }`. Listed in §6.3; the rework lands with this slice's implementation.
- [ ] **The `Copy` name itself** — Rust's name; matches user expectation. Alternative names considered: `Plain`, `Trivial`, `BitCopy`, `Value`. None are clearly better. Keep `Copy` for the marker / property name.
- [ ] **Tagged-union Copy-ness** — once tagged unions land (Phase 3 later slice), a tagged union is Copy iff *every* variant's payload is Copy. Recorded here; implementation lives with the tagged-union slice.
