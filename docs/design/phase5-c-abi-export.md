# Phase 5 — C-ABI export: building a library C can link against

**Status:** ✅ shipped 2026-05-15 (slices 5.A–5.F).

**Motivation.** Phase 4 (`cpc-bindgen`) covers the C→C+ direction: consuming
existing C headers. Phase 5 covers the inverse: emitting `.a` / `.dylib` /
`.so` artifacts a C, C++, Swift, Python-cffi, Lua-FFI, Java-JNA, or
Ruby-FFI consumer can link against.

Before Phase 5, a hand-test at the end of Phase 1 closeout showed scalar
fns worked but value-passed aggregates corrupted across the boundary:
calling `square({3,4})` from C against a C+ `square(Point) -> i32`
returned `-1454817015` instead of `25`. The cause was that cpc emitted
LLVM "first-class aggregate" parameter passing, which does NOT match
the platform C ABI's register-coerced equivalents. Phase 5 closes that.

## Locked decisions

1. **Library target lives in the manifest.** A new `[lib]` section in
   `Cplus.toml` with `name`, `path`, `crate-type ∈ {staticlib, cdylib, both}`,
   plus the same `frameworks` / `libs` link surface as `[[bin]]`.
   `[[bin]]` and `[lib]` are mutually exclusive in a single manifest
   (E0408) — a crate is either a binary or a library. Same shape as
   Cargo's `[lib]` singleton.

2. **Surface syntax for "this is a C-callable export":**
   ```cplus
   pub extern fn NAME(...) -> T { body }
   ```
   The existing parser rejected `extern fn` with a body
   (`extern_fn_with_body_rejected` test). Slice 5.C lifted that
   restriction *only* when `pub` precedes `extern` — a clean syntactic
   split:
   - `extern fn name(...);` → import (declaration, no body, no `pub`).
   - `pub extern fn name(...) { ... }` → export (definition, body required).

   `extern` already means "C ABI" in C+ — no `extern "C"` string needed
   (C is the only ABI).

3. **Non-C-representable types at the boundary are rejected at sema
   time (E0410), not silently mis-emitted.** The rejection set:
   - `string` (24-byte heap-backed; cross-boundary Drop is undefined)
   - `str` (16-byte fat pointer; no C counterpart)
   - `slice T[]` (16-byte fat pointer; same)
   - tagged enums (no C ABI for sum types)
   - structs without `#[repr(C)]` (layout unspecified)
   - structs with `Drop` (no destructor runs on the C side)
   - generic `Ty::Param` (caller couldn't write a concrete signature)

   Each diagnostic includes the conventional workaround (e.g., "pass
   `*u8` and a `usize` length instead of `str`").

4. **Targets in v1:** macOS arm64 (primary) + Linux x86-64.
   Windows / aarch64-Linux deferred. Windows-x86 needs `inalloca`
   which Slice 1H Tier-3 already rejected; aarch64-Linux differs from
   aarch64-darwin in HFA / vararg edge cases.

5. **`pub` matters at codegen.** In library builds, non-`pub` items
   emit with `internal` LLVM linkage so LTO + `-fvisibility=hidden`
   strips unused implementation details from the shipped artifact.
   Executable builds keep external linkage everywhere (matches
   pre-5.B behavior and avoids touching 34 substring-pinned tests).
   `main` is always external; `drop` is always internal in lib mode.

## What lands by slice

| Slice | Title | Key emitted artifact |
|---|---|---|
| 5.A | Library target + object emission | `[lib]` parsing; `cpc --emit-obj`; `cpc build` → `.a` / `.dylib` / `.so`; resolver skips path-mangling for lib entry items so `pub fn` symbols are bare-C-callable |
| 5.B | `pub` ↔ linkage | non-`pub` items get `internal` linkage in lib mode |
| 5.C | `pub extern fn body` surface | parser accepts body; sema E0410 rejects non-C types in signature; codegen routes export defs to `define` |
| 5.D | C-ABI aggregate coercion | aarch64-darwin: ≤8B → `i64`; 9-16B → `[2 x i64]`; >16B → indirect ptr (param) / `sret` (return); coerced returns stage through alloca + reload-as-coerced |
| 5.E | C header generation | `cpc --emit-header FILE`; `cpc build` for `[lib]` auto-emits `target/<mode>/<libname>.h`; clean C99 with `#pragma once` + stdint/stdbool includes + `extern "C"` guard |
| 5.F | Reference example + this note | `docs/examples/c_consumer/` with mathlib + C consumer + Makefile; this design note |

## The ABI classification rule

For each value-passed parameter and return type in a `pub extern fn`,
slice 5.D's `classify_c_abi` predicate maps the C+ type to one of:

- **Direct** — pass unchanged. Primitives, raw `*T`, fn pointers, plain
  (untagged) enums. The C and C+ ABIs agree.
- **Coerce(ty, size, align)** — rewrite the LLVM type. ≤8-byte aggregates
  become `i64` (one GPR); 9–16-byte aggregates become `[2 x i64]` (two
  GPRs). The callee allocates a slot sized for the coerced type and reads
  original fields through opaque-pointer GEPs; the wider tail is harmless.
- **Indirect** — pass via a pointer. >16-byte aggregates. For params
  on aarch64-darwin: bare `ptr` (no `byval`); the caller owns the slot.
  For returns: `ptr sret(<ty>)` first parameter, void return.

### Worked example — `square(Point) -> i32`

`Point` is `#[repr(C)] { i32, i32 }`, size 8, align 4. The classifier
yields `Coerce { llvm_ty: "i64", size: 8, align: 8 }`. The emitted LLVM:

```llvm
define i32 @square(i64 %0) {
entry:
  %p.addr1 = alloca i64, align 8         ; coerced-size slot
  store i64 %0, ptr %p.addr1             ; pack into 8 bytes
  ; subsequent GEPs use %Point type for offsets:
  %t2 = getelementptr %Point, ptr %p.addr1, i32 0, i32 0  ; x at offset 0
  %t3 = load i32, ptr %t2
  ; ... same for y at offset 4
  %r = mul i32 %t3, %t3
  ; ...
  ret i32 %r
}
```

This byte-matches what `clang -O0 -emit-llvm` produces for the equivalent
C function on aarch64-darwin.

### Worked example — `make_triple() -> Triple`

`Triple` is `#[repr(C)] { i64, i64, i64 }`, size 24. Classifier yields
`Indirect`. The emitted signature:

```llvm
define void @make_triple(ptr sret(%Triple) noalias nonnull noundef writable
                         dereferenceable(24) align 8 %0, i64 %1, i64 %2, i64 %3) {
entry:
  ; build the struct directly into %0 (caller's slot):
  %a_ptr = getelementptr %Triple, ptr %0, i32 0, i32 0
  store i64 %1, ptr %a_ptr
  %b_ptr = getelementptr %Triple, ptr %0, i32 0, i32 1
  store i64 %2, ptr %b_ptr
  %c_ptr = getelementptr %Triple, ptr %0, i32 0, i32 2
  store i64 %3, ptr %c_ptr
  ret void
}
```

This reuses Slice 1D's `sret` infrastructure, generalized from `Ty::String`
only to any indirect-class return.

## The reference example

[docs/examples/c_consumer/](.) is the canonical full workflow:

- `mathlib/Cplus.toml` — `[lib] crate-type = "both"`.
- `mathlib/src/lib.cplus` — one `pub extern fn` per ABI class: scalar,
  8-byte struct, 16-byte struct, 24-byte struct (indirect), plain enum,
  raw-pointer out-param, function-pointer callback, internal-helper
  delegation.
- `c_user/c_user.c` — calls every export from C; asserts each result.
- `c_user/Makefile` — drives `cpc build --release`, links the C consumer
  statically and dynamically, runs the smoke test.

```bash
$ cd docs/examples/c_consumer/c_user
$ make check
./c_user
add(20, 22) = 42 (expected 42)
square({3, 4}) = 25 (expected 25)     # ← the canonical 5.D fix
make_triple = {100, 200, 300}
sum_triple = 600 (expected 600)
...
0 failure(s)
OK
```

## Workflow summary

Library author:

```bash
$ cpc build --release
# Produces:
#   target/release/libmathlib.a       (static)
#   target/release/libmathlib.dylib   (dynamic, macOS)
#   target/release/mathlib.h          (auto-generated header)
```

Consumer:

```bash
$ clang myapp.c -L./target/release -lmathlib \
    -I./target/release -o myapp
$ ./myapp
```

That's it. No `bindgen` step, no header maintenance.

## Non-goals (deferred)

- **Windows ABI** (x86 nor x86_64). Defer until Windows is a tier-1 target.
  The Slice 1H Tier-3 plan already rejected `inalloca` (the x86-Windows
  hack), so this is consistent.
- **HFA optimization** on aarch64. Aggregates of floats currently go
  through integer-class coercion in v1 — correct but suboptimal for
  SIMD-heavy code. Defer to v2.
- **C++ name mangling.** C++ consumers must `extern "C"` the headers
  themselves (standard practice; the generated header already wraps
  declarations in `extern "C"`).
- **Generic exports.** `pub extern fn foo[T](...)` is rejected by
  sema (E0410); users monomorphize manually.
- **Cross-language `Drop`.** Types with destructors cannot cross the
  boundary by value. Workaround: opaque-pointer pattern with a paired
  `*_free(*T)` export.
- **C++ inheritance / virtual / templates.** Out of scope; this is a C
  ABI, not C++.
- **x86_64-sysv** ABI shape. Same shape as aarch64-darwin for integer-
  class aggregates (≤8 → `i64`, 9-16 → `{i64, i64}` instead of `[2 x i64]`,
  >16 → byval). ~1-day follow-up slice.
- **C+ internal calls to its own `pub extern fn`.** The define-site
  applies C-ABI coercion; call sites don't. An internal call to a
  `pub extern fn` taking an aggregate would mismatch. Library authors
  should route through a private helper and let the `pub extern fn`
  be a thin C-ABI wrapper. Documented limitation.

## Cross-references

- Implementation: [cplus-core/src/codegen.rs](../../cplus-core/src/codegen.rs)
  — `classify_c_abi`, `CAbiClass`, the param + return coercion paths in
  `gen_function`, the `coerce_ret` field on `FnState`.
- Sema gate: [cplus-core/src/sema.rs](../../cplus-core/src/sema.rs) —
  `c_exportable_diagnosis`, `check_extern_export_signature`.
- Header generation: [cpc/src/main.rs](../../cpc/src/main.rs) —
  `render_c_header`, `type_to_c`, `render_param_decl`.
- Manifest: [cplus-core/src/manifest.rs](../../cplus-core/src/manifest.rs)
  — `LibTarget`, `CrateType`, the E0408/E0412 paths.
- Tests: [cpc/tests/e2e.rs](../../cpc/tests/e2e.rs) — every 5.* slice has
  at least one round-trip e2e test that goes C → C+ library → C and
  checks the runtime answer.
