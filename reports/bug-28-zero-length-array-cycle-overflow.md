# Bug 28 — a value of a zero-length-array-recursive struct overflows the compiler's stack

- Status: PARTIAL 2026-08-02 — the HANG is fixed (`issue-15 (b)`, the shared
  `layout_of` cycle guard). What remains is a lowering gap: the emitted LLVM
  type is self-referential and clang rejects it. Found while landing
  issue-13 (b).
- Type: was a compiler hang (stack overflow, `SIGABRT`); now a late,
  wrong-layer error from clang
- Area: `cplus-core/src/codegen.rs` — the struct-body emitter
- Severity: low reachability. The failure is loud either way now.

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

## What is fixed, and what is left

The stack overflow is gone: `layout_of` is now one cycle-guarded walk shared
by sema and codegen (issue-15 (b)), and a revisit answers "zero size, align
1" for that path. Both repros above, and the mutual-recursion variant, now
reach the end of the compiler.

They do not link. Codegen emits the struct body verbatim:

```llvm
%Node = type { [0 x %Node], i32 }
```

and clang rejects it — `identified structure type 'Node' is recursive`. The
diagnostic is real but it arrives from the wrong tool at the wrong layer,
naming a definition the user did write but in terms of an IR type they never
saw.

The fix is in the struct-body emitter, not in layout: a zero-length array
contributes no storage, so `[0 x %Node]` can be emitted as `[0 x i8]` — same
size (0), same alignment contribution (1), no self-reference. Any zero-length
array of a type currently being emitted needs the same treatment, since that
is precisely when the reference would close a cycle.

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

## Verification when the lowering is fixed

- The repro above compiles and runs (`x.n == 0`).
- Add it as an e2e case beside `zero_length_array_recursion_is_clean`, which
  today only pins that the DECLARATION is accepted.
- Mutual recursion through zero-length arrays too — confirmed to overflow the
  same way:

  ```cplus
  struct A { b: [B; 0], n: i32 }
  struct B { a: [A; 0], m: i32 }
  fn main() -> i32 { let x: A = #zero::[A](); return x.n; }
  ```
