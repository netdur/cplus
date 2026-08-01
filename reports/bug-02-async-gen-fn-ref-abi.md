# Bug 02 — `async fn` / `gen fn` ignore the borrow ABI: def/call prototype mismatch

- Status: reproduced 2026-08-01 with `target/release/cpc` (prints stack garbage, write-back lost)
- Severity: miscompile
- Area: codegen (`cplus-core/src/codegen.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B2)

Context for the fixer: codegen emits LLVM IR as text; clang compiles it. Build with
`cargo build --release`; always use `target/release/cpc`. Tests: `cargo test -p cplus-core`
(codegen's IR-text unit tests live in codegen.rs's `#[cfg(test)]` module) and
`cargo test -p cpc --test e2e`. A `ref x: T` parameter and any non-Copy borrow are passed
by pointer (`param_passes_by_ptr` decides). Line numbers are from 2026-08-01; locate by
symbol name if drifted.

## Summary

There are nine hand-rolled function-definition emitters in codegen. Seven classify
parameters through the pointer-passing rule; `gen_async_function` and `gen_gen_function`
emit every parameter as a plain SSA value (`write!(out, "{} %{}", llvm_ty(pty, types), i)`)
with no `param_passes_by_ptr`, no attributes, and no pointer for `ref` params. The call
side (`gen_named_call`) pointer-passes those arguments, so the definition reads a pointer
bit-pattern as an integer. No diagnostic; silent garbage.

## Reproduction

Project `arp/` — `Cplus.toml`:

```toml
[package]
name = "arp"
version = "0.0.1"
edition = "2026"

[[bin]]
name = "arp"
path = "src/main.cplus"

[dependencies]
stdlib = "*"
```

`src/main.cplus`:

```cplus
import "stdlib/future" as future;
import "stdlib/reactor" as _;

async fn bump(ref n: i64) -> i64 {
    n = n + 1;
    return n;
}

fn main() -> i32 {
    var x: i64 = 5;
    let f: future::Future[i64] = bump(x);
    let r: i64 = #block_on::[i64](f);
    #println(r as i32);
    #println(x as i32);
    return 0;
}
```

```
$ target/release/cpc build && ./target/debug/arp
1827136057
5
```

Expected: `6` and `6` (result 6, write-back visible in `x`). Observed: garbage (the pointer
truncated to i32) and a lost write-back. The exact garbage value varies per run.

## Root cause

- `gen_async_function` (codegen.rs:7371-7379) and `gen_gen_function` (codegen.rs:7226-7233)
  emit parameters with no ABI classification.
- Their method twins do it correctly and are the model to copy: `gen_async_method`
  (codegen.rs:6808-6828) and `gen_gen_method` (codegen.rs:7044-7121) — pointer-pass
  classification, parameter attributes, and the prologue binding that makes `ref`
  write-back work.
- The call side is `gen_named_call` (codegen.rs:13673-13690), which classifies with the
  shared rule; only the two def emitters diverge.

## Fix

1. In `gen_async_function`, replicate the parameter loop from `gen_async_method`: classify
   each param with `param_passes_by_ptr(pty, mv, mu, types)`; emit `ptr` + attributes for
   pointer-passed params; bind them in the prologue the same way the method twin does
   (so writes go through the pointer).
2. Same change in `gen_gen_function` (copy from `gen_gen_method`).
3. Confirm the coroutine frame capture (these emitters build async/gen state machines)
   stores the pointer, not a copy, for `ref` params — mirror whatever the method twins do.

Structural companion: `issue-03-abi-classifier.md` (one classifier all nine emitters
consume). The tactical fix above is correct on its own and should land first.

## Verification

1. The repro prints `6` / `6`.
2. Add a codegen unit test asserting the emitted IR for an `async fn f(ref n: i64)` defines
   `ptr` for the param (copy a neighboring IR-text assertion test in codegen.rs's test
   module).
3. Add a runtime e2e test (cpc/tests/e2e.rs) for async + gen fns with `ref` params
   verifying result and write-back.
4. Full suites: `cargo test -p cplus-core && cargo test -p cpc --test e2e`.

## Notes

- Strict C ABI symmetry is a hard project requirement: def and call classification must
  agree. Do not "fix" this by changing the call side to match the broken defs — the method
  twins define the correct convention.
- `gen fn` (generator) was not run at runtime during the audit; the code shape is identical
  to the async case. Verify it with its own e2e test.
