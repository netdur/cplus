# vendor/static-arena — design study (not yet implementable)

This package is a placeholder for the v0.0.10 Phase 2 design from
[plan.md](../../../plan.md). The design's API surface looks like:

```cplus
pub struct StaticArena16K {
    buf: [u8; 16384],
    used: usize,
}

#[no_alloc] pub fn StaticArena16K::new() -> StaticArena16K { ... }
#[no_alloc] pub fn StaticArena16K::alloc_bytes(mut self, n: usize) -> Option[*u8] { ... }
```

i.e. a fixed-size arena that lives entirely on the stack (or in
`static mut` storage), marked `#[no_alloc]` end-to-end so it
composes with the v0.0.10 Phase 1 real-time contract.

## Why this isn't shipping yet

The implementation needs one of:

1. **Const-generic struct fields**: `struct StaticArena[const N: usize] { buf: [u8; N], used: usize }`.
   Sema accepts `[T; N]` *array types* with a literal `N` but not
   `const N: usize` on struct generic parameter lists. Extending the
   parser + sema is "small but real work" (plan.md §Phase 2).

2. **Fill-array literal**: `[0; 16384]` as a shorthand for
   `[0, 0, ..., 0]`. Today the parser only accepts enumerated array
   literals — `let x: [u8; 16384] = [0; 16384];` doesn't parse.
   This is the smaller language change of the two.

3. **Zero-init static**: `static BUF: [u8; 16384];` (no initializer).
   Sema requires every `static` to declare an initializer expression.
   Zero-init BSS would be the classical no-malloc-needed pattern.

None of these three exist today. The package directory + manifest are
scaffolded so the Cargo-style namespace is reserved; the actual
`.cplus` source lands in a v0.0.10 follow-up slice once one of the
language pieces above is in.

## Workaround for users today

Use `vendor/arena`'s `Arena` with a single up-front `malloc` at startup
and then operate it without further allocator calls in the hot path.
That arena's `alloc_bytes` doesn't malloc until the current chunk is
exhausted, so for workloads whose total allocation is bounded by the
initial chunk size, it behaves as a static arena would. The catch is
that `#[no_alloc]` will reject calls to `Arena::alloc_bytes` because
sema can't prove the chunk-exhaustion branch is unreachable.

Tracking issue: ship one of the three language pieces above, then
land the real `static-arena` source here.
