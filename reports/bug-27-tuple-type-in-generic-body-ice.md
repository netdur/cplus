# Bug 27 — A tuple TYPE naming a type parameter inside a generic body: ICE

- Status: reproduced 2026-08-01, pre-existing (panics identically on the pre-audit
  binary); found while verifying bug-04/06/07, NOT one of them
- Severity: ICE
- Area: monomorphize (`cplus-core/src/monomorphize.rs`) + sema instantiation recording
- Master report: not in the 2026-08-01 audit; discovered during its fix pass

## Reproduction

Generic free fn:

```cplus
fn pack[T](take x: T) -> i32 {
    let p: (T, i32) = (x, 1);
    return p.1;
}
fn main() -> i32 { return pack::[i32](3) - 1; }
```

Generic impl method (same failure):

```cplus
struct H[T] { v: T }
impl H[T] {
    fn tup(this) -> i32 {
        let p: (T, i32) = (this.v, 1);
        return p.1;
    }
}
fn main() -> i32 { let h: H[i32] = H[i32] { v: 3 }; return h.tup() - 1; }
```

```
thread 'main' panicked at cplus-core/src/codegen.rs:
codegen reached TypeKind::Tuple — sema/monomorphize did not lower this site
```

Expected: both compile and exit 0. Controls that DO work: the same tuple type written
concretely (`let p: (i32, i32)`) in a generic body, and a tuple type in a
non-generic impl.

## Root cause

`subst_type_ast`'s `TypeKind::Tuple` arm substitutes the element types and then looks the
result up in `struct_lookup.by_names` under the `"__Tuple"` key, falling through unchanged
on a miss — the comment there says "sema would have synthesized it on first encounter, so
a miss here means an out-of-band tuple type that won't codegen."

That premise does not hold inside a generic body. Sema type-checks the TEMPLATE, so it
registers `("__Tuple", ["T", "i32"])`; the substituted `("__Tuple", ["i32", "i32"])` is
never registered unless some other site in the program happens to use it. Mono then leaves
`TypeKind::Tuple` in place and codegen panics.

The `TypeKind::Generic` arm has the identical fallthrough with the identical premise, so
`Pair[T]` in a generic body is only safe because sema's struct-instantiation propagation
covers it. Tuples have no equivalent propagation.

## Fix (sketch — not implemented)

Either:

1. Extend the instantiation propagation that already resolves `Pair[T]` → `Pair__i32` to
   tuple types, so the substituted `__Tuple` instantiation is registered before mono runs; or
2. have monomorphize synthesize the tuple struct on demand when the `by_names` lookup
   misses (it has the resolved element types in hand at that point).

Whichever is chosen, replace the fallthrough in BOTH the `Tuple` and `Generic` arms with a
loud failure — a silent fallthrough to an un-lowerable node is what turned a missing
instantiation into a codegen panic with no source location.

## Verification

1. Both repros compile and exit 0.
2. Negative control: the concrete-tuple and non-generic-impl forms still compile.
3. e2e runtime test for a tuple type mentioning a type parameter, in both a generic fn and
   a generic impl method.
