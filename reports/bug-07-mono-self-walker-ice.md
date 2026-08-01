# Bug 07 — `Self` rewrite walker is a partial clone: `Self { .. }` inside `loop` ICEs

- Status: FIXED 2026-08-01, commit f212467 — took the PREFERRED fix: `Self` is now
  resolved by the main walker (binding carried on `StructLookup`), and the partial-clone
  `rewrite_block_self` / `rewrite_stmt_self` / `rewrite_expr_self` / `rewrite_for_self`
  are deleted
- Status (original): reproduced 2026-08-01 with `target/release/cpc` (panic at codegen.rs:4379 `codegen reached Ty::Error`; control without `loop` compiles and runs)
- Severity: ICE
- Area: monomorphize (`cplus-core/src/monomorphize.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B7)

Context for the fixer: when monomorphize expands a generic impl's methods for a concrete
instantiation, it must substitute `Self` with the concrete instance type throughout the
body. That substitution runs in a SECOND walker pair (`rewrite_stmt_self` /
`rewrite_expr_self`) which is a partial clone of the main rewrite walker with fewer match
arms. Statements the clone does not recurse into keep `Self` unresolved → `Ty::Error` in
codegen. Build `cargo build --release`; binary `target/release/cpc`; tests
`cargo test -p cplus-core`, `cargo test -p cpc --test e2e`. Line numbers from 2026-08-01.

## Reproduction

`selfloop.cplus` (panics):

```cplus
struct Holder[T] { v: T }

impl Holder[T] {
    fn spin(this) -> i32 {
        loop {
            let h: Self = Self { v: this.v };
            return h.v;
        }
    }
}

fn main() -> i32 {
    let b: Holder[i32] = Holder[i32] { v: 3 };
    return b.spin() - 3;
}
```

`selfok.cplus` (identical body without the `loop {}` wrapper) compiles and exits 0.

```
$ target/release/cpc check selfloop.cplus
thread 'main' panicked at cplus-core/src/codegen.rs:4379: codegen reached Ty::Error
```

## Root cause

- `rewrite_stmt_self` (monomorphize.rs:909-944) handles only Let/Expr/Return/While/For;
  `Loop`, `Defer`, `Assert`, `LetDestructure` fall through `other => other.clone()` at
  :938 — the `Self` inside the loop body is never substituted.
- `rewrite_expr_self` (monomorphize.rs:972-1102) additionally misses GenericEnumCall,
  Await/Yield, Range, InterpStr, FnRef, TupleLit.
- The pair is driven by `rewrite_block_with_self` (monomorphize.rs:870-892), a second pass
  over the same tree the main walker already traverses.

## Fix

Preferred (small-to-medium, removes the clone permanently):

1. Make `Self` an ordinary substitution key in the MAIN rewrite walker: it is a name→type
   mapping like any other type-param binding. Thread `self_target: Option<&str>` (the
   concrete mangled instance name) into the main rewrite context, substitute `Self` in
   type positions and `Self { .. }` / `Self::` expression positions there.
2. Delete `rewrite_block_with_self`, `rewrite_stmt_self`, `rewrite_expr_self`.

Tactical fallback (if the preferred change is too wide right now): add the missing arms —
Loop/Defer/Assert/LetDestructure to `rewrite_stmt_self`; GenericEnumCall/Await/Yield/
Range/InterpStr/FnRef/TupleLit to `rewrite_expr_self` — and leave a comment that the
walker must mirror the main one arm-for-arm.

Structural companions: `issue-01-generic-ast-walker.md` (default-recursion walker),
`issue-10-merge-method-mono-paths.md` (Self-as-subst-key is part of merging the twin
expansion paths).

## Note on how the preferred fix was made small

Threading `self_target: Option<&str>` as an 8th parameter through
`rewrite_expr`/`rewrite_stmt`/`rewrite_block`/`subst_type_ast` would have been a ~40-site
mechanical diff. Instead the binding rides on `StructLookup`, the context object the
walker ALREADY threads through every recursive call. `StructLookup` now borrows its two
name tables instead of owning them, so `with_self(name)` is a free copy rather than a
per-method clone of two program-wide HashMaps. `Self` is then resolved in the three
positions it can appear: `TypeKind::Path` inside `subst_type_ast`, `StructLit`'s name, and
the leading segment of a `Path` callee (`Self::assoc()`).

## Verification

1. `selfloop.cplus` compiles and exits 0; `selfok.cplus` still does.
2. DONE: `self_is_substituted_in_every_statement_position` in monomorphize.rs covers
   `loop`, `defer`, `assert`, and `Self::assoc()`, asserting no `"Self"` survives anywhere
   in the expanded program.
3. DONE: `mono_rewrites_generic_calls_and_self_in_every_position` in cpc/tests/e2e.rs.
4. DONE: full suites green.

NOT covered, and NOT this bug: `let p: (Self, i32)` — any tuple TYPE naming a type
parameter inside a generic body ICEs with "codegen reached TypeKind::Tuple", identically
on the pre-fix binary. Written up separately as `bug-27-tuple-type-in-generic-body-ice.md`.
