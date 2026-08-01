# Bug 23 — Extern-import `declare` drops the `ref` rule: declaration lies about the ABI

- Status: IR-verified 2026-08-01 (declare says `i64`, call passes `ptr`)
- Severity: latent ABI (right-by-accident at runtime; poisons LTO and anything trusting declares)
- Area: codegen (`cplus-core/src/codegen.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B23)

Context for the fixer: codegen emits LLVM IR text. For an `extern fn` IMPORT it emits a
`declare`; calls to it are lowered from the C+ signature. A `ref n: i64` param is
pointer-passed (out-param semantics; the C callee receives `int64_t*`). Build
`cargo build --release`; binary `target/release/cpc`. Line numbers from 2026-08-01.

## Reproduction

`extref.cplus`:

```cplus
extern fn frob(ref n: i64);

fn main() -> i32 {
    var x: i64 = 5;
    frob(x);
    #println(x as i32);
    return 0;
}
```

Emit IR (use the project's IR-emission flag; check `target/release/cpc --help` for
`--emit-ll` or equivalent) and observe:

```
declare void @frob(i64)
...
call void @frob(ptr %...)
```

The declare and the call disagree. Runtime behavior is currently correct because the
call-site types govern lowering and the C callee wants the pointer — but the declaration
is false, which breaks LTO signature checking and any tool that trusts declares.

## Root cause

The extern-import declare emitter (codegen.rs:6161-6196) destructures the parameter tuple
as `(pty, _move_flag, _mut_flag, _restrict_flag)` and classifies on the TYPE alone,
skipping the ref/out-param pointer rule. The EXPORT path applies the rule and even
documents it ("Mirrors the native side", codegen.rs:6420-6424); the import path skipped
it.

## Fix

1. In the import-declare emitter, apply the same classification the export path and the
   call site use: a `ref` (mutable, non-move) param declares as `ptr` (with the same
   attrs the call site emits, if any).
2. Audit the same emitter for the other pointer-passing cases (non-Copy borrow structs)
   so declare and call agree for every parameter class, not just `ref`.

Structural companion: `issue-03-abi-classifier.md` — with one classifier, declare, define,
and call all render from the same `PassBy` and cannot disagree.

## Verification

1. Emitted IR: declare and call agree for the repro (`ptr` in both).
2. Add a codegen unit test asserting the declare shape for `extern fn f(ref n: i64)`
   (copy a neighboring declare-assertion test in codegen.rs's test module).
3. Link a real C callee taking `int64_t*` in an e2e test if the harness supports linking
   C (grep e2e.rs for existing extern-C link tests); verify write-back.
4. Full suites.
