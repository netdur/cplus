# Bug 09 — `-g` silently loses all debug info when any tail call exists

- Status: reproduced 2026-08-01 with `target/release/cpc` (clang: "ignoring invalid debug info")
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

1. Compile the repro with `-g`: no clang debug-info warnings; `dsymutil mt && dwarfdump
   --debug-line mt.dSYM | head` (or `lldb ./mt -o "b helper" -o run`) shows line info and
   symbols.
2. Add a codegen unit test: IR for a self-recursive fn under `-g` has `!dbg` on the
   `musttail call` line (grep the test module for an existing `-g`/`!dbg` assertion to
   copy the harness).
3. Full suites.

## Notes

- The failure is silent in normal builds (warnings scroll by); consider promoting the
  clang "ignoring invalid debug info" warning to a hard cpc error in `-g` builds so any
  future gap fails loudly instead of shipping symbol-less binaries.
