# Bug 06 — Generic call inside string interpolation: discovered but never rewritten (ICE)

- Status: FIXED 2026-08-01, commit f212467 — InterpStr arm in `rewrite_expr`
- Status (original): reproduced 2026-08-01 with `target/release/cpc` (panic: missing mangled fn)
- Severity: ICE
- Area: monomorphize (`cplus-core/src/monomorphize.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B6)

Context for the fixer: monomorphize synthesizes concrete copies of generic fns, rewrites
call sites to mangled names, deletes templates. It has TWO walkers that must agree: the
discovery walker `visit_ident_calls` (collects instantiations) and the rewrite walker
`rewrite_expr` (renames call sites). A construct visited by one but not the other leaves a
call to a deleted template. Build `cargo build --release`; binary `target/release/cpc`;
tests `cargo test -p cplus-core`, `cargo test -p cpc --test e2e`. Line numbers from
2026-08-01.

## Reproduction

Project (interpolation `"${...}"` requires stdlib/text) — `Cplus.toml`:

```toml
[package]
name    = "monoaudit"
version = "0.0.1"
edition = "2026"

[[bin]]
name = "monoaudit"
path = "src/main.cplus"

[dependencies]
stdlib = "*"
```

`src/main.cplus`:

```cplus
import "stdlib/io" as io;
import "stdlib/text" as text;

fn double_it[T](take x: T) -> T { return x; }

fn main() -> i32 {
    let t: text::Text = "v=${double_it::[i32](7)}";
    io::println(t.view());
    return 0;
}
```

```
$ target/release/cpc build
thread 'main' panicked ... missing `monoaudit.src.main.double_it`
```

Expected: compiles; prints `v=7`.

## Root cause

- `visit_ident_calls` WALKS interpolation parts (monomorphize.rs:1314-1320), so
  `double_it__i32` is synthesized.
- `rewrite_expr` (monomorphize.rs:2426-3246) has no `InterpStr` arm — the call site inside
  the interpolation keeps the bare template name; the template is deleted; codegen panics.
- This discovery/rewrite asymmetry is a known family: the Await/Yield arms carry a comment
  (monomorphize.rs:2826-2830) describing the identical historical bug.

## Fix

1. Add an `InterpStr` arm to `rewrite_expr` that rewrites every part expression.
2. Sweep both walkers for remaining asymmetries in one sitting: `ExprKind::Asm` operands
   are absent from BOTH (see bug-04, which owns the Asm/TupleLit additions). The correct
   end state: the set of expression kinds traversed by `visit_ident_calls` and
   `rewrite_expr` is identical.

Structural companion: `issue-01-generic-ast-walker.md` — a shared default-recursion walker
makes divergence between the two impossible. Siblings: bug-04 (TupleLit), bug-07 (Self
walker).

## Verification

1. The repro project builds and prints `v=7`.
2. Covered by the e2e test rather than a mono unit test: interpolation needs stdlib's
   `Text`, which the mono test harness (`run_with_driver_names`) cannot supply — it lexes
   and sema-checks one unvendored snippet. The sibling positions (tuple literal, `#asm`
   operand) ARE unit-tested there.
3. DONE: `generic_call_in_interpolation_monomorphizes` in cpc/tests/e2e.rs asserts the
   built program prints `v=7`.
4. DONE: full suites green.
