# Bug 27 — A tuple TYPE naming a type parameter inside a generic body: ICE

- Status: FIXED 2026-08-02, commit 9c36226 — with one sub-case left open, see
  "What is still broken" below
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

## Correction to this report: the family is wider than the two repros

The two repros above are one of FOUR shapes that fail, with three distinct panics. The
report as filed named only the first. All four were reproduced on the pre-fix binary:

1. **Annotated tuple type in a generic body** (both repros above) —
   `codegen reached TypeKind::Tuple` from `ty_from`. Monomorphize's fallthrough.
2. **Tuple in a generic SIGNATURE** — `fn wrap[T](take x: T) -> (T, i32)`,
   `fn snd[T](take p: (T, i32))`, or the same on a generic impl method. Panic:
   `generic_origin names a template not in struct_generic_templates`, from sema's
   `subst_ty_deep`. This one never reaches monomorphize at all, so no amount of work
   on the mono arm would have fixed it.
3. **Tuple literal with an inferred type in a generic FREE-fn body** —
   `fn pick[T](take a: T, take b: T) -> T { let p = (a, b); return p.0; }`. Panic:
   `sema should have synthesized tuple struct __tuple_i32_i32`, from `gen_tuple_lit`.
   There is no `TypeKind::Tuple` node anywhere here — the type is inferred — so the
   AST-walking fix does not see this site either.
4. **Tuple literal with an inferred type in a generic IMPL-METHOD body** — same panic
   as (3). Still open; see below.

All four share one root cause, stated correctly in the original report: sema type-checks
the TEMPLATE, so the tuple struct it synthesizes is keyed on `Ty::Param` element types,
and nothing registers the substituted `("__Tuple", [i32, i32])` that the instantiated
body actually needs. What differs is only where the missing registration surfaces.

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

## Fix (as landed)

Option 1 from the sketch — register the instantiation in sema before mono runs — in the
pass that already exists for exactly this purpose, `propagate_body_instantiations`
(the fixed-point walk of generic template bodies, one walk per concrete enclosing
instantiation). Three parts:

1. `BodySite::TupleType` — `walk_type_sites` now records every `TypeKind::Tuple` it
   passes, not just its elements. Processing substitutes the enclosing instantiation's
   subst through the elements and calls `synthesize_tuple_struct`. Fixes shape (1).
2. `subst_ty_deep`'s `Ty::Struct` arm grew a `TUPLE_TEMPLATE` case: a synthesized tuple
   is re-instantiated by synthesizing the substituted element list, because `"__Tuple"`
   has no `StructDecl` to look up. Fixes shape (2) — the `.expect()` right below it was
   the panic. The pseudo-template name is now the single constant `sema::TUPLE_TEMPLATE`
   rather than five spellings of `"__Tuple"` across sema and monomorphize.
3. `BodySite::TupleLit` + `SemaCx::tuple_lit_elems` — a span-keyed record of what each
   tuple literal's elements checked as, written by `check_tuple_lit`, replayed under the
   enclosing subst by the propagation pass. This is the same trick `BodySite::FnCall`
   already plays with `call_monos`, and for the same reason: the AST alone doesn't carry
   the types. Fixes shape (3).

The sketch's second recommendation — replace the mono fallthroughs with a loud failure —
was deliberately NOT taken, and the reason is worth recording. `subst_type_ast` runs over
non-generic items with an empty subst as well, including bodies that codegen may never
emit; a panic there would turn "an un-lowerable node in dead code" into a hard compiler
failure on programs that build today. The fallthrough is only reachable now when a tuple
instantiation is genuinely missing, and that case still fails loudly at codegen with a
message that names the type.

## What is still broken

Shape (4) — a tuple literal whose type is inferred, inside a generic IMPL-METHOD body:

```cplus
struct H[T] { v: T }
impl H[T] { fn tup(this) -> i32 { let p = (this.v, 1); return p.1; } }
fn main() -> i32 { let h: H[i32] = H[i32] { v: 3 }; return h.tup() - 1; }
```

still panics with `sema should have synthesized tuple struct __tuple_i32_i32`.

Why the fix above doesn't reach it: generic impl-method bodies are never type-checked
(they get `check_generic_method_body_names`, a name-resolution pass only), so there is no
`tuple_lit_elems` record to replay — sema has no idea what `(this.v, 1)` is. Fixing it
needs element types that only a type-check of the body under the subst can produce. That
is the same underlying gap as bug-06/bug-07 (generic impl-method bodies leave no trace in
the tables the later passes read), not a tuple-specific one.

Two notes on its blast radius: adding a type annotation (`let p: (T, i32) = ...`) works,
and so does any other site in the program that instantiates the same tuple shape — the
instantiation is registered program-wide, so this only bites the first and only use.

## Verification

1. All four shapes reproduced on the pre-fix binary; shapes (1)–(3) now compile and exit 0,
   shape (4) is unchanged and documented above.
2. Negative control: the concrete-tuple and non-generic-impl forms still compile.
3. e2e `tuple_types_naming_a_type_parameter_instantiate` covers, in one program: an
   annotated tuple in a generic fn body and in a generic impl-method body, a tuple in a
   generic impl-method return type, a generic free fn's tuple return and tuple parameter,
   and an inferred tuple literal in a generic fn — plus a second instantiation
   (`(bool, i32)`) so a mangling collision between two tuple shapes would fail the test.
4. `cargo test -p cplus-core` (1834) and `cargo test -p cpc` (630) green; `cpc test` in
   `vendor/stdlib` green at 290 in both debug and `--release`; vendor-wide `cpc check`
   diagnostic-count parity against the pre-change binary shows no change in any of the
   54 packages.
