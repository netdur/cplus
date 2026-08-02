# Bug 23 — Extern-import `declare` drops the `ref` rule: declaration lies about the ABI

- Status: FIXED 2026-08-02, commit 987ab01 — the import declare applies the same
  pointer-passing rule as the export path and the call site
- Status (original): IR-verified 2026-08-01 (declare says `i64`, call passes `ptr`)
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

1. DONE: `declare void @frob(ptr nonnull noundef dereferenceable(8) align 8)` — attribute
   for attribute identical to the call.
2. DONE: `an_extern_declare_pointer_passes_ref_params_like_the_call_site` in codegen.rs
   compares the two parameter lists rather than asserting a literal string, so the
   property tested is the agreement itself.
3. DONE: `extern_ref_param_is_a_c_out_parameter` in cpc/tests/e2e.rs links a C function
   taking `long long *`, and checks the write-back reaches the C+ caller.
4. DONE: full suites green.

Step 2's audit of the other pointer-passing classes: the fix is written as
`param_passes_by_ptr`, the same predicate the export path and the call site use, so every
parameter class it covers — `ref`, and non-Copy borrows — is covered here too, not just
`ref`.
