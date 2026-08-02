# Bug 28 — a value of a zero-length-array-recursive struct overflows the compiler's stack

- Status: OPEN, found 2026-08-02 while landing issue-13 (b)
- Type: compiler hang (stack overflow, `SIGABRT`) — not a miscompile
- Area: `cplus-core` — the zero-value / layout path, NOT the drop rule
- Severity: low reachability, hard failure. No diagnostic, no partial output.

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

## Where to look

The surviving walk runs when a VALUE is needed, not when the type is
declared, which points at the zero-value / layout / field-init path rather
than at classification. Candidates: the `#zero` lowering's per-field
recursion, `layout_of`-style size/align computation (issue-15 (b) is about
consolidating those), and codegen's struct-body emission.

The fix shape is the same one the drop rule just took: a visiting set, with a
revisit answering the identity for that path (zero fields contribute nothing
to size, alignment, or a zero value). Whichever walk it turns out to be is
likely to be one of the copies issue-15 (b) wants to merge, so fixing it
inside that consolidation is probably cheaper than fixing it twice.

## Verification when fixed

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
