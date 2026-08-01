# Bug 09 — `-g` silently loses all debug info when any tail call exists

- Status: FIXED 2026-08-02, commit 5d0fcaa — matcher covers every tail marker, AND the
  clang warning is now fatal under `-g`
- Status (original): reproduced 2026-08-01 with `target/release/cpc` (clang: "ignoring invalid debug info")
- Severity: tooling (debug builds ship without symbols, silently)
- Area: codegen (`cplus-core/src/codegen.rs`)
- Master report: `core-drift-audit-2026-08-01.md` (B9)

Context for the fixer: codegen emits LLVM IR text, then a DWARF post-pass re-parses that
text line by line to attach `!dbg` locations before clang compiles it. Build
`cargo build --release`; binary `target/release/cpc`; tests `cargo test -p cplus-core`
(IR-text unit tests in codegen.rs test module), `cargo test -p cpc --test e2e`. Line
numbers from 2026-08-01.

## Reproduction

`mt.cplus`:

```cplus
fn helper(n: i32, acc: i32) -> i32 {
    if n == 0 { return acc; }
    return helper(n - 1, acc + n);
}

fn main() -> i32 {
    let r: i32 = helper(10, 0);
    if r == 55 { return 0; }
    return 1;
}
```

```
$ target/release/cpc mt.cplus -g -o mt
warning: inlinable function call in a function with debug info must have a !dbg location
  %tNN = musttail call fastcc i32 @helper(...)
warning: ignoring invalid debug info ...
```

The binary builds but clang has dropped the ENTIRE debug-info module: no symbols, no line
info. Expected: `-g` produces a binary with working debug info regardless of tail calls.

## Root cause

Self-recursive `return f(...)` emits `musttail call`. The DWARF post-pass line matcher
(codegen.rs:1992-1998) recognizes call instructions with `starts_with("call ")` /
`contains("= call ")` and never matches `musttail call` (or `tail call`), so those lines
get no `!dbg`. LLVM requires every inlinable call in a debug-info function to carry a
location; one violation makes clang discard the whole module's debug info with only a
warning.

## Fix

Tactical (small, do now):

1. Extend the matcher to also recognize `musttail call` and `tail call` lines (both the
   `= musttail call` value form and the bare statement form).

Structural (companion `issue-08-emit-time-metadata.md`):

2. Attach `!dbg` at emission time — the emitting code knows the current function and
   source span — and delete the text re-parse. All three text post-passes (dbg, sanitizer
   attrs, alias scopes) fail open like this; the emission-time funnel closes the class.

## Verification

1. DONE: the repro compiles with `-g` and clang emits no debug-info warnings.
   `dsymutil` cannot be used to confirm on macOS — cpc deletes the temp object, and DWARF
   lives in the object until dsymutil links it, so `dsymutil` reports "unable to open
   object file" for ANY cpc `-g` build, fixed or not. The absence of clang's rejection is
   the signal, and the unit test below pins the IR-level cause directly.
2. DONE: `every_call_form_gets_a_dbg_location_under_g` in codegen.rs — asserts the probe
   really does emit `musttail call`, then walks every call/invoke inside every function
   the pass stamped with a DISubprogram and requires a `!dbg`. There was no existing `-g`
   assertion to copy; the test builds through `generate_with_debug` against a temp file.
   (Synthesized glue — coro helpers, trampolines — gets no DISubprogram and correctly
   needs no locations; the test scopes to functions that carry one.)
3. DONE: full suites green.

## Notes

- The report's suggestion to promote clang's "ignoring invalid debug info" warning to a
  hard error WAS taken, in `run_clang` (cpc/src/main.rs), scoped to `-g` builds so
  ordinary builds keep streaming clang's stderr live. Verified by re-narrowing the
  matcher and confirming the build then fails with a pointed message instead of silently
  producing a symbol-less binary.
