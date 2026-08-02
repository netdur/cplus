# Bug 28 — a value of a zero-length-array-recursive struct overflows the compiler's stack

- Status: FIXED 2026-08-02 — the hang in `b818a1b` (issue-15 (b)'s shared
  `layout_of` cycle guard), the lowering in `<this commit>`. Found while
  landing issue-13 (b).
- Type: was a compiler hang (stack overflow, `SIGABRT`), then a late,
  wrong-layer clang error
- Area: `cplus-core/src/sema.rs` (the shared layout walk),
  `cplus-core/src/codegen.rs` (the struct-body emitter)
- Severity: low reachability, hard failure. Both halves were loud.

## Repro

```cplus
struct Node { kids: [Node; 0], n: i32 }
fn main() -> i32 { let x: Node = #zero::[Node](); return x.n; }
```

```
$ cpc check t.cplus
thread 'main' has overflowed its stack
fatal runtime error: stack overflow, aborting
```

DECLARING the type is fine — `cpc check` on the struct alone exits 0, and
sema has a test pinning that (`zero_length_array_recursion_is_clean`). It is
constructing a VALUE that hangs. `#zero::[Node]()` is the only constructor
available: an empty array literal is rejected (E0332, "empty array literals
not supported in Phase 2"), so a plain struct literal cannot build one.

## The two halves

**The hang.** `layout_of` is now one cycle-guarded walk shared by sema and
codegen (issue-15 (b)); a revisit answers "zero size, align 1" for that path.
That took the repro from an abort to a clang error.

**The lowering.** Codegen used to emit the struct body verbatim:

```llvm
%Node = type { [0 x %Node], i32 }
```

which clang rejects — `identified structure type 'Node' is recursive`. A real
diagnostic, from the wrong tool, naming an IR type the user never saw.

The element type of an EMPTY array is unobservable: nothing can index it and
its size is zero. So the cycle is cut with `i8`, and only where it is a cycle:
`llvm_field_ty` renders a field of a named-struct definition, and replaces
`[0 x T]` with `[0 x i8]` exactly when `T` reaches back to the struct being
defined. That is the same place `layout_of`'s guard cuts, which is what keeps
the emitted type and the computed layout the same shape — the guard reports
"zero size, align 1" for the revisited struct, and `[0 x i8]` is a field of
zero size and alignment 1.

A zero-length array that closes NO cycle keeps its element type, and with it
the alignment `layout_of` reports for the field: `struct Pad0 { xs: [i64; 0],
n: i32 }` stays `%Pad0 = type { [0 x i64], i32 }`, size 8 align 8 on both
sides. Cutting those to `i8` would have silently changed their layout.

## Why the type is legal

E0913 rejects value-recursive types because they have no finite size.
`[Node; 0]` embeds nothing, so `Node` is finite and the occurs-check
correctly accepts it. The containment GRAPH still has a cycle
(`Node → [Node; 0] → Node`); it is the size that is finite, not the graph
that is acyclic.

## What it is not

Not the drop rule. `sema::carries_drop` had the same blind spot — both
hand-written copies carried a comment claiming cycle-safety "because by-value
containment is acyclic" — and enumerating every declared type through it
overflowed on sema's own corpus while issue-13 (b) was being landed. That
one is fixed (`f7704c8`): the shared rule now carries a visiting set. The
repro above still overflows with that fix in place, and overflows identically
on the pre-change binary, so a second recursion has the same blind spot.

## How it was found, and where the hang was

Enumerating every declared type through the drop rule (issue-13 (b)'s
classification walk) overflowed on sema's own
`zero_length_array_recursion_is_clean` corpus — both hand-written copies of
that rule claimed cycle-safety "because by-value containment is acyclic",
which is true of SIZE and false of the containment graph. Guarding the drop
rule fixed that walk and left this repro overflowing, which located the
second copy of the same blind spot in the layout walk. Guarding that one — as
part of merging sema's `layout_of` with codegen's `static_layout` — is what
took the repro from a hang to a clang error.

## Verification (as run)

- e2e `a_value_of_a_zero_length_array_recursive_struct_builds_and_runs`
  compiles AND runs four shapes: the direct cycle, the mutual pair, one
  closed through a by-value field (`S { f: Inner }`, `Inner { xs: [S; 0] }`),
  and one with a destructor. `zero_length_array_recursion_is_clean` in sema
  only ever pinned that the declaration is accepted.
- codegen `a_zero_length_array_cycle_is_cut_but_a_plain_one_is_not` pins the
  emitted IR for all four, including the control that must NOT be cut.
- `--emit-ll` byte-identical over the 40 `docs/examples` programs, and the
  vendor-wide `cpc check` sweep unchanged: no existing program contains a
  zero-length array of an aggregate, so nothing else moved.
