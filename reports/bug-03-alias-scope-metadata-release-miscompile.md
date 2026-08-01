# Bug 03 — `!alias.scope`/`!noalias` metadata on `ref` params: release-only miscompile

- Status: FIXED 2026-08-01, commit cd226d9 — param↔param disjointness no longer published; locals kept
- Status (original): reproduced 2026-08-01 with `target/release/cpc` (debug prints 23, `--release` prints 20)
- Severity: miscompile (release builds only)
- Area: codegen (`cplus-core/src/codegen.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B3)

Context for the fixer: codegen emits LLVM IR as text. Build with `cargo build --release`;
always use `target/release/cpc`. Tests: `cargo test -p cplus-core` (IR-text unit tests in
codegen.rs's test module), `cargo test -p cpc --test e2e`. This project has a recorded
bug family of unsound IR attributes (TBAA on aggregates, `noalias` on borrows,
`preserve_nonecc`); the recorded lesson is to distrust any attribute asserting something
the front end merely hopes is true. This bug is the same family, in metadata form.
Line numbers are from 2026-08-01; locate by symbol name if drifted.

## Summary

On 2026-07-27 the `noalias` *attribute* was removed from borrow parameters because the
borrow checker deliberately does not cover the raw-pointer and statics seams
(`param_attrs`, codegen.rs:2932-2976, documents this: "a promise the borrow checker cannot
keep"). The scoped-alias *metadata* pass makes the identical promise and was left in:
every pointer-passed `ref`/mut param gets an `!alias.scope`, and each scope's `!noalias`
list contains all sibling param scopes. Two `ref` params that legally alias (reached
through the raw seam) are then "known" disjoint to LLVM, and `-O3` reorders loads across
stores.

## Reproduction

`alias_probe.cplus`:

```cplus
struct S {
    a: i64,
}

impl S {
    fn drop(ref this) {
    }
}

fn cross(ref x: S, ref y: S) -> i64 {
    x.a = x.a + 1;
    let v: i64 = y.a;
    x.a = x.a + 1;
    let w: i64 = y.a;
    return v + w;
}

fn main() -> i32 {
    var s: S = S { a: 10 };
    let p: *S = #addr_of(s);
    var r: i64 = 0;
    r = cross(*p, *p);
    #println(r as i32);
    return 0;
}
```

```
$ target/release/cpc alias_probe.cplus -o a && ./a
23
$ target/release/cpc alias_probe.cplus --release -o a && ./a
20
```

23 is correct (11 + 12). 20 means both reads of `y.a` were hoisted as if `x` and `y`
cannot alias. The IR carries `!alias.scope !N, !noalias !M` pairs on the accesses through
the two params.

## Root cause

- Scope assignment: in the fn emitter (codegen.rs:6652-6694) and the method emitter
  (codegen.rs:8272-8323), `(param_passes_by_ptr(pty, *mv, *mu, types) && (*mv || *mu)).then_some(i)`
  enrolls every pointer-passed ref/mut param in a scope; sibling scopes are mutually
  `!noalias`.
- Application: the textual rewrite `annotate_alias_scope_metadata` / `annotate_one_line`
  (codegen.rs:3535-3618) stamps the metadata onto loads/stores through those pointers.
- The language guarantee backing param↔param disjointness does not exist across the
  deny-by-design seams (statics, raw pointers) — exactly what the 2026-07-27 attribute fix
  concluded.

## Fix

1. Stop emitting param↔param disjointness: a param's `!noalias` list must not contain
   sibling param scopes.
2. KEEP local↔local and local↔param pairs: locals are fresh allocas, disjoint from
   everything by construction — that part is sound and carries the optimization value.
3. Fix the adjacent seed bug while here: in `gen_method`, `noalias_ssas.push(0)`
   (codegen.rs:8285) ignores the sret offset — when the function uses sret, the receiver
   is `%1` and `%0` is the sret slot, so the sret slot gets the receiver's scope. Push the
   receiver's real index.
4. Also align the twins: `gen_method`'s scope set omits `noalias_local_slots` while
   `gen_function` includes them; make them identical.

Structural companion: `issue-08-emit-time-metadata.md` (attach metadata at emission instead
of re-parsing IR text). The metadata-content fix above is independent and urgent.

## Verification

1. DONE: repro prints 23 in debug AND `--release`. Pinned as the e2e test
   `aliasing_ref_params_are_not_promised_disjoint`, which runs the probe in both modes.
2. DONE: `cd vendor/stdlib && cpc test --release` — 290 passed.
3. DONE: `param_scopes_claim_disjointness_from_locals_only` resolves the metadata nodes
   and asserts a param's `!noalias` list holds the LOCAL scope and nothing else, while
   the local's list holds both params.
4. DONE, and the update was substantive — six codegen tests pinned the unsound shape.
   Five used a params-only function (which now, correctly, publishes nothing) and were
   reshaped to include a non-Copy local so they still exercise the GEP dataflow;
   `two_mut_noncopy_params_emit_domain_and_scopes` became
   `two_mut_noncopy_params_alone_publish_nothing`, asserting the opposite of what it
   used to. `alias_scope_propagates_through_gep_chain` was counting annotated lines
   across the whole module, so `main`'s own locals were satisfying it — now scoped to
   the function under test. New: `sret_method_scopes_the_receiver_not_the_sret_slot`
   for step 3.
5. DONE: full suites green.

## Notes

- Be conservative: if in doubt between dropping too much metadata and keeping an unsound
  promise, drop the metadata. Optimization can be recovered later; miscompiles cannot.
