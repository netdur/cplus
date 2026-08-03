# bug-29 — a bound method reference inside a generic impl body ICEs codegen

- Status: FIXED 2026-08-03 (`5f511e7`) — rejected at check time with E0822.
- Severity: ICE (`.expect("sema validated")`), reachable from ordinary source
- Area: `cplus-core/src/sema.rs` (generic-impl template checking) →
  `cplus-core/src/codegen.rs:2233` (where it landed)
- Found by: probing the issue-07 step 5 port; unrelated to that port, and
  present identically before it (verified against a binary built at `3a7601d`).

## Repro

```
struct Cell[T] { v: T }
impl Cell[T] {
    fn tap(ref this) { return; }
    fn build(ref this) -> i32 { return take_handler(this.tap); }
}
fn take_handler(f: fn(*u8), ctx: *u8 = 0 as *u8) -> i32 { return 1; }
fn main() -> i32 { var c: Cell[i32] = Cell[i32] { v: 0 }; return c.build(); }
```

```
thread 'main' panicked at cplus-core/src/codegen.rs:2233:14:
sema validated
```

The receiver does not have to be generic — a bound reference to a CONCRETE
`static`'s method inside a generic impl body fails the same way:

```
impl Cell[T] { fn go(ref this) -> i32 { return take_handler(POINT.eat); } }
```

The body has to be instantiated for the ICE to fire; an uninstantiated
template is never handed to codegen.

## Cause

A bound method reference is not one value. It is a PAIR: a bridge function
synthesized for one concrete type, plus the receiver's address. Sema records
`BoundMethodRefInfo` against the ARGUMENT's span
(`try_bound_method_refs`), monomorphize rewrites that span into
`bridge` + `#addr_of(recv) as *u8` (`monomorphize.rs:1995`), and codegen
emits the pair.

A generic-impl method body is checked by `check_methods` with name
resolution only — `check_generic_method_body_names` — and never typed:
typing happens per instantiation, and there is no sema pass after
monomorphize. So `try_bound_method_refs` never runs over the template,
nothing is recorded, monomorphize finds no entry for the span, and codegen
reaches `this.tap` as an ordinary field read of a struct with no such
field: `StructInfo::field_index(...).expect("sema validated")`.

The deeper reason the record could not simply be moved earlier: a template
has ONE span per argument and N instantiations, each needing its own bridge
for its own concrete type. `bound_method_refs` is keyed by span, so the
shape has nowhere to live. Supporting it is a feature (per-instantiation
bridge synthesis), not a bug fix.

This is the same failure the name-resolution pass was written to stop — that
one closed undefined NAMES in templates reaching codegen; this is the next
shape along the same seam.

## Fix

`SemaCx::reject_bound_refs_in_generic_body`, run over every generic-impl
method body beside the name-resolution pass. For each named call whose
signature is known, an argument in an `fn`-typed parameter position that is
a field expression is refused with E0822 — the code the concrete path
already uses for bound references it cannot lower.

A name that IS a fn-pointer field somewhere in the program is left alone
(`is_fnptr_field_name`, the pre-filter sema already keeps for this
distinction): that one is an ordinary field read and lowers fine. The
regression test pins both halves.

```
error[E0822]: cannot take a bound reference to `tap` inside a generic impl
body: a bound reference pairs the receiver's address with a bridge function
synthesized for one concrete type, and this body is compiled once per
instantiation. Move the handler onto a concrete `impl`, or pass a plain `fn`
```

## Verification

- `cpc/tests/e2e.rs:a_bound_method_reference_in_a_generic_impl_body_is_rejected_not_an_ice`
  — both receiver shapes rejected with E0822 and no panic, plus the
  fn-pointer-field control that must still compile.
- `cargo test -p cplus-core` 1869 + 8, `cargo test -p cpc` 617 + 16 + 5 + 6.
- Diagnostic parity against a binary built at `3a7601d` over all 274
  `vendor/*` and `examples/*` sources: byte-identical, so nothing real was
  using the shape.

## Not done

Actually supporting the pattern — synthesizing one bridge per instantiation
and keying the record by (span, instantiation) rather than span alone. E0822
now says the restriction out loud, which is the honest state until then.
