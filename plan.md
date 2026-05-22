# C+ — Plan

Version 0.0.8 shipped 2026-05-22. See [plan-0.0.8.md](plan-0.0.8.md) for the archived 0.0.8 roadmap and resolved log; [plan-0.0.7.md](plan-0.0.7.md) covers v0.0.7, [plan-0.0.6.md](plan-0.0.6.md) v0.0.6, [plan-0.0.5.md](plan-0.0.5.md) v0.0.5, [plan-0.0.4.md](plan-0.0.4.md) v0.0.4, [plan-0.0.3.md](plan-0.0.3.md) v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.9 — Tighten the safety story, close the long-tail bugs

**Strategy:** v0.0.8 validated the v0.0.7 surface against three real benchmarks and closed the bench-gap punch list. C+ now wins the raytracer outright (0.94 s vs C's 1.16 s) on Apple Silicon, the SIMD surface has a packaged consumer (`vendor/simd`), the native Metal GPU compute package (`vendor/metal`) has been implemented and verified entirely from pure C+ using Objective-C FFI, and the macro-builtin trilogy is settled (`include_bytes!` / `include_str!` / `env!`).

What's left is the long tail: small bugs that surfaced under real workloads (mixed-if-arm panic, lingering musttail edge cases), one safety footgun that contradicts the "safety as default" pitch (`fn echo(x: string) -> string` is a silent double-free without an explicit `move`), and one ergonomic gap that touches every byte-level program (no character literals).

No new principles. The locked twelve from §1.Locked-Principles stand. v0.0.9 is polish + correctness, not language reshape.

Slice sizes follow the same S/M/L assistant-paced framing.

---

### Phase 1 — Safety: default-move for non-Copy value params · size M

**The footgun**, lifted from the v0.0.8 raytracer port and from a user-flagged note in the closed plan:

```cplus
// ❌ Caller's `s` and callee's `x` both run Drop. Double-free under ASan.
fn echo(x: string) -> string { return x; }

// ✅ Today's workaround — manually mark the param `move`.
fn echo(move x: string) -> string { return x; }
```

A safety-first language should not silently emit a double-free when the user writes the "obvious" signature. The current model — value-passed non-Copy = "shared borrow that aliases the caller's heap" — was a v0.0.5 expedient and never stress-tested against a real workload until the raytracer port.

**Goal:** for a non-`Copy` value-typed parameter without `move`, default to **move** semantics (single owner: the callee). The current "borrow" interpretation requires the caller's writer to opt in via `borrow` or stays as the explicit `mut x: T` non-Copy pointer ABI.

**Locked design decisions:**

1. **Backwards compatibility break** — every `fn f(x: NonCopyT)` in the wild gets the new semantics. We accept this; the codebase is small enough to migrate, and the alternative (keep the footgun forever) violates the principle that motivated the borrow checker in the first place.
2. **`mut x: T` pointer-pass ABI is unchanged.** Exclusive borrow keeps the §2.9 shape.
3. **`borrow x: T`** becomes the explicit shared-borrow form (new keyword). Today's `x: T` shape lowers to it for source migration: parser warning E0X12 ("`x: T` for non-Copy T defaulted to `move` in v0.0.9; previously `borrow`. Add `borrow` explicitly to keep the old semantics."), at least for one release cycle.
4. **`echo` worked example from §12 of the tutorial flips** — the move marker becomes default, the comment moves to "explicit borrow if you want sharing."

**Scope:**

- Sema: change the default ownership interpretation. The `move`/`borrow` markers stay; the silent case flips.
- Codegen: existing `move_flag` plumbing is reused; the call-site Drop flag flip + the param-binding Drop registration already work — they just fire by default now.
- Migration warning E0X12 for the breaking-change interpretation.
- Tutorial §12 rewrite (the "Use `move v: T` for non-Copy value parameters" gotcha at §30 disappears).
- E2E sweep: every `fn ... (x: T) ... { return x; }` in stdlib / vendor / docs / proves needs review. Estimate ~30-50 sites; mechanical rewrite (add `move` where the old behavior was relied on, or accept the new default).

**Tests:** unit (the parser warning + sema-pass for both forms) + e2e (ASan run of the `echo` shape — must be clean under default semantics).

**Expected payoff:** the §1 principle "safety as default" stops having a load-bearing exception. The "you must remember `move`" sentence leaves the tutorial.

---

### Phase 2 — Character literals · size S

**The gap:** `'a'` doesn't parse today. ASCII bytes are written as `65u8`, which is fine for codegen but reads like assembly. Every C+ program that touches the byte alphabet (JSON / CSV / network protocol parsers) has `b'{' as u8 = 123` style comments or magic-number bytes scattered through it.

**Locked design decisions:**

1. **Syntax:** `'a'` for a single ASCII byte, type `u8`. Direct lower to the byte value as an `i8` immediate.
2. **No multi-char.** `'ab'` is a parse error (E0X20 "character literal must be exactly one byte").
3. **Escapes:** the same backslash escapes the string literal accepts — `'\n'` `'\t'` `'\\'` `'\''` `'\0'` and `'\xHH'`. UTF-8 multi-byte code points are rejected at parse time (E0X21 "character literal must be a single byte; for UTF-8 use a `str`").
4. **Type:** `u8`. Pattern-matching against `'a'` in a `match arm` matches a `u8`. Not a separate `char` type — C+ doesn't have one and won't grow one.
5. **Why now:** the JSON tokenizer + raytracer + future bytes-level workloads all have this scattered through them. One token cuts the magic-number noise in half.

**Scope:**

- Lexer: tokenize `'...'` into `TokenKind::CharLit(u8)`.
- Parser: route to `ExprKind::IntLit(byte as u64, NumSuffix::U8)`. AST stays minimal — no new variant.
- Sema: untouched (the existing `u8` literal path handles it).
- Tutorial §3 mentions the new literal; §30 "no character literals in v0.0.4" gotcha is removed.

**Tests:** unit (positive: every escape; negative: empty literal, multi-byte, UTF-8 codepoint).

**Expected payoff:** small but symbolic — closes one of the original gotchas in `Tutorial > Gotchas worth memorising`.

---

### Phase 3 — Long-tail codegen bug fixes · size S

bench.md and the v0.0.8 close surfaced two cpc bugs that didn't make it into a slice-shaped fix:

1. **Mixed-if-arm panic** — when one `if` arm is a simple call and the other is a block with internal `let` + tail expr, codegen still panics on some shapes. v0.0.8 finding 3 fixed one shape; another remains. Workaround: `let mut` + conditional assign. The bench.md raytracer has at least one of these in source — a perf tax (lost branch-elimination on the `if`-as-expression form).

2. **musttail predicate edge cases** — v0.0.8 closed the nested-arg-steals-flag bug. Other shapes may still slip through (e.g. recursive tail-call where the recursive call carries a `move`-marked arg, which currently has its own drop-flag-flip code path running between the call and the `ret`). Audit with a fuzzer / property test.

**Scope:**

- Minimum repro for each, added to `cplus-core/src/codegen.rs` test module.
- Tighten the predicates; remove the source-level workarounds in `bench-cplus/raytracer/cplus/main.cplus` once each fix lands.

**Tests:** pin per bug + a regression suite e2e.

**Expected payoff:** removes "workaround tax" lines from real code. Bench-cplus has a few labeled `// workaround for cpc bug` comments — each closure is one line of source that gets to drop.

---

### Phase 4 — Module-level `const` and `static` items · size M

**The gap**, surfaced by two independent consumers:

1. **The C raytracer benchmark** ([bench-cplus/raytracer/c/main.c](/Users/adel/Workspace/bench-cplus/raytracer/c/main.c)) uses `static const sphere_t scene[10]` (immutable global table), `static uint32_t rng_state` (mutable RNG counter), and `static v3 cam_origin, cam_ll, cam_hor, cam_ver` (mutable camera state set once, read everywhere). The C+ port at [bench-cplus/raytracer/cplus/main.cplus](/Users/adel/Workspace/bench-cplus/raytracer/cplus/main.cplus) has the explicit comment "*scene, rng_state, camera live on the heap (C+ has no top-level static)*" — every randf/scatter/cast call threads `restrict state: *u32` through the signature just to keep parity with C. Ergonomic + perf tax.
2. **The llama.cpp port** ([cpc-gaps.md G-015](../llama.cplus/cpc-gaps.md)): `pub const FOO: i32 = N;` at module scope is rejected. Mirroring a C ABI integer enum has no clean form — [scripts/gen-enum-mirror.py](../llama.cplus/scripts/gen-enum-mirror.py) output sits unused.
3. **The stdlib itself**: 33 sites across [vendor/stdlib/](vendor/stdlib/) use `fn off_head() -> usize { return 136 as usize; }` as a workaround for missing module-level named literals (concentrated in `channel.cplus` and `reactor.cplus` for hand-laid struct offsets).

The `const` keyword is already half-wired ([cplus-core/src/lexer.rs:444](cplus-core/src/lexer.rs#L444) recognizes it but nothing consumes it). `static` is not yet a keyword. Both are purely additive — no parser ambiguity, no clash with existing surface.

**Locked design decisions:**

1. **Three forms, two storage models.** `const FOO: T = lit;` (compile-time alias, no storage), `static FOO: T = lit;` (immutable global with address), `static mut FOO: T = lit;` (mutable global with address).
2. **`const` lowering: inline literal substitution.** No LLVM global emitted. Each use-site path-expression that resolves to a const is replaced with the literal value at lowering time. Same shape as `#define` but type-checked.
3. **`static` lowering: LLVM `@FOO = constant <ty> <lit>` in `.rodata`.** Each use-site path-expression lowers to a load from the global.
4. **`static mut` lowering: LLVM `@FOO = global <ty> <lit>` in `.data`.** Each read lowers to a load; each write to a store. Reads and writes must occur inside `unsafe { ... }` — C+'s borrow checker cannot prove absence of data races for module-scope mutable state, so the keyword is available but the unsafety is opted into locally. E0X33 ("read of `static mut` requires `unsafe { ... }`"), E0X34 (write variant).
5. **Initializer form (all three): literal-only for v0.0.9.** Integer, bool, char, float, string. Struct literals over literals (`Sphere { center: V3 { x: 0.0, y: 0.0, z: 0.0 }, radius: 1.0, ... }`) and array literals of literals (`[N]T { lit, lit, ... }`) are accepted — the raytracer's `scene[10]` array needs this and it's a small extension to the literal-only path (no const-eval pass; the parser already builds these AST nodes for `let` bindings). Arithmetic initializers (`const N: i32 = 1 + 2;`) rejected with E0X30. The const-eval pass waits for a real consumer.
6. **Type annotation required.** No inference for any form. E0X31 ("const/static requires explicit type annotation").
7. **Module-level only.** No `const`/`static` inside `fn` bodies (use `let`) or `impl` blocks. Both wait for a real consumer.
8. **Visibility:** `pub` works the same as on `fn` / `struct`. Module-private by default.
9. **Path resolution:** const/static names register in module scope alongside `fn` / `struct` / `enum`. `module::FOO` resolves through the existing path resolver.

**Scope:**

- Lexer: add `static` keyword token in [cplus-core/src/lexer.rs](cplus-core/src/lexer.rs).
- AST: add `ItemKind::Const(ConstDecl { vis, name, ty, value })` and `ItemKind::Static(StaticDecl { vis, mutable, name, ty, value })` in [cplus-core/src/ast.rs](cplus-core/src/ast.rs).
- Parser: extend `parse_item` at [cplus-core/src/parser.rs:213](cplus-core/src/parser.rs#L213) to accept `pub? const NAME: Ty = lit;` and `pub? static mut? NAME: Ty = lit;`. New diagnostics E0X30 / E0X31.
- Resolver: register names in module scope; resolve path-expressions that name a const/static.
- Sema: type-check initializer against declared type. For `static mut` reads/writes, require enclosing `unsafe { ... }` (E0X33 / E0X34).
- Codegen: const → inline literal at path resolution; static → emit `@FOO = constant`; static mut → emit `@FOO = global`. Loads/stores route through existing path-expression codegen.
- Tutorial: §4 grows a "module-level const and static" subsection.
- Knock-on: llama.cplus G-015 closes; stdlib `fn off_*()` workarounds get a follow-up cleanup pass; raytracer bench can drop the `restrict state: *u32` parameter chain for a fairer apples-to-apples comparison with C.

**Tests:** unit (parser positive for every supported initializer kind across const/static/static-mut; negative for arithmetic init, missing type, inside-fn, inside-impl; sema positive for cross-module use; sema negative for `static mut` read outside `unsafe`) + e2e (a `proves/` consumer that exercises all three forms end-to-end, including a struct-literal `static` initializer and a `static mut` counter read/written from a `unsafe { ... }` block).

**Expected payoff:** closes the most-cited C+ language gap across three independent consumers (llama port, stdlib, raytracer bench). The raytracer specifically can drop parameter-threading workarounds and approach C's codegen shape more directly.

---

### Phase 5 — Threaded raytracer · size M (deferred from v0.0.8)

**Goal:** parallel-tiles raytracer. Each thread renders one horizontal band of the image, joins, then `main` writes the assembled buffer. v0.0.5 shipped `thread::spawn` / `JoinHandle::join`; v0.0.8's raytracer ran it single-threaded. This slice exercises the v0.0.5 thread surface against a real workload and gives the bench.md raytracer benchmark a 4-8× headroom on multi-core machines.

**Locked design decisions:**

1. **No work stealing.** Static tile partition (rows 0..N/T per thread for N rows, T threads). v0.1+ design.
2. **Output buffer is pre-allocated; each thread writes its tile.** No shared writes to a `Mutex` — disjoint rows per thread is the invariant the borrow-checker doesn't yet verify, but `restrict` on the pointer + thread-local row ranges keeps it sound by construction.
3. **Thread count:** detect via libc `sysconf(_SC_NPROCESSORS_ONLN)` or hardcode to 4 for the bench. Caller-overridable via `env!("RAYTRACE_THREADS")` (v0.0.8 Phase 4's env! makes this clean).
4. **Same image hash as single-threaded.** The bench-cplus `cplus/main.cplus` uses a deterministic per-pixel RNG seed, so re-tiling work across threads produces identical bytes if and only if each pixel's RNG state is deterministic from the pixel coordinates (not from a serial RNG advance). The single-threaded path already has this property; the multi-threaded path keeps it.

**Tests:** unit (thread join correctness, already in stdlib tests) + e2e (raytracer wall-time + identical image hash to single-threaded).

**Expected payoff:** the raytracer bench gets a multi-core column. The v0.0.5 thread surface validated against a real shared workload.

---

### Phase 6 — Raw-pointer ↔ integer cast in `unsafe` · size S

**The gap**, from the llama.cpp port ([cpc-gaps.md G-016](../llama.cplus/cpc-gaps.md)): `p as usize` (and `as u64` / `as isize`) fails with **E0315** "invalid cast: raw-pointer cannot be cast to usize". This blocks every C idiom that needs the numeric value of a pointer — alignment checks (`(uintptr_t)p % alignment`), pointer-difference arithmetic that doesn't fit C+'s typed `p - q` ratio form, hash-of-pointer keys. The port works around it via a tiny C shim:

```c
size_t cplus_ptr_addr(const void *p) { return (size_t)(uintptr_t)p; }
```

…then `extern fn cplus_ptr_addr(p: *u8) -> usize` from the C+ side. Every consumer pays an FFI call plus a `.o` in the link line.

The reverse direction (`int as *T`) already works inside `unsafe { ... }` — Phase 10 FFI cleared that. The forward direction is missing only because no slice surfaced a real consumer until the port.

**Locked design decisions:**

1. **Direction:** allow `*T as usize`, `*T as u64`, `*T as isize`, `*T as i64`. Smaller-width casts (`*T as u32`, `*T as i32`, etc.) are **rejected** with E0315 — narrowing a 64-bit pointer to 32 bits silently truncates and is almost always a bug. Cast to `usize` first, then narrow if you really mean to.
2. **Safety gate:** requires `unsafe { ... }` context. Pointer-as-integer is a primitive operation that crosses the type system; the borrow checker has no way to reason about whether the resulting integer will be cast back to a pointer and dereferenced. Same gate as the existing integer-to-pointer cast.
3. **Codegen:** lowers to LLVM `ptrtoint <ty> <val> to i64` (with bitcast to `i64` for `usize`/`isize` since C+ models them as 64-bit on 64-bit platforms). No runtime cost.
4. **No `as bool` from pointer.** Null-checks go through explicit `== (0 as *T)` comparison or `usize` round-trip; we don't grow a third null-check spelling.

**Scope:**

- Sema: extend [cplus-core/src/sema.rs](cplus-core/src/sema.rs) cast validation to accept `RawPtr(_) → Usize | U64 | Isize | I64` when `unsafe_depth > 0`. Diagnostic E0315 stays for narrower targets, with a "cast to `usize` first" suggestion.
- Codegen: extend [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs) `gen_cast` to emit `ptrtoint`.
- Tutorial §6.3 grows a one-paragraph subsection on `p as usize` as the alignment-check primitive.

**Tests:** unit (positive: `p as usize`, `p as u64`, `p as isize`, `p as i64` in unsafe; negative: same casts outside unsafe → E0801; negative: `p as u32` → E0315 even in unsafe) + e2e (alignment-check helper that returns `(p as usize) % 64`).

**Expected payoff:** llama.cplus drops the `cplus-shim/c_helpers.c` file from its link line. Every other C-port that hits the `(uintptr_t)p` shape works natively.

---

### Phase 7 — `cpc --emit-obj` walks local imports · size M

**The gap**, from the llama.cpp port ([cpc-gaps.md G-011](../llama.cplus/cpc-gaps.md)): `cpc --emit-obj file.cplus` is the single-file shim path the port uses for swap-and-link (sema runs over each shim, codegen emits a `.o`, the host CMake stitches the result into the existing build). But single-file mode doesn't walk `import "./..."` statements — each shim file has to **hand-duplicate** any constants it imports (today the ggml_op integer values are mirrored in each shim by hand). When the source moves to pure `cpc build` (Phase 7 of the port), the duplication ends — but during the bridge, the workaround tax is real.

**Locked design decisions:**

1. **Behavior change in `--emit-obj` (and `--emit-ir` / `--emit-ll` / `--emit-asm`):** when the input file contains `import "./..."` statements, walk them via the same resolver path `cpc build` uses. The output is still a single `.o` (or `.ll` / `.asm`), but its IR contains the merged program. Cross-file calls resolve to qualified symbols (`src.foo.bar`); the C-side caller links against those names.
2. **`import "package/..."` (non-local) is still rejected** in single-file mode — there's no manifest, no `vendor/` resolution. Local imports `import "./..."` are the only new shape admitted. Diagnostic E0411 ("non-local import in single-file mode; requires `Cplus.toml`") for the rejected case.
3. **Implicit search root** is the directory of the input file. `import "./util"` resolves to `<input-dir>/util.cplus`. No upward search, no implicit `src/` prefix.
4. **Cycle detection** stays unchanged — the resolver already detects cycles via E0404; the single-file path inherits that for free.

**Scope:**

- Driver: factor the import-walking logic in [cpc/src/main.rs](cpc/src/main.rs) so the `--emit-obj` path can call into it. Currently the project-mode `build_project` path constructs a `Loader` that walks imports; the single-file `build_program` path parses one file. The refactor: extract a `load_with_local_imports(entry_path, src) -> LoadedProgram` helper that both paths share.
- Manifest synthesis: in single-file mode, synthesize a minimal in-memory `Cplus.toml`-equivalent (just the entry file path + no dependencies) so the resolver doesn't need a separate code path. Cleaner than dual-pathing the resolver.
- Diagnostic: E0411 for `import "stdlib/..."` (or any non-`./` shape) in single-file mode.

**Tests:** unit (the new `load_with_local_imports` helper across a two-file fixture) + e2e (`cpc --emit-obj` with a two-file project produces a `.o` that links against both files' symbols; same source with `cpc --emit-obj` against just the entry file produces identical IR to `cpc build`).

**Expected payoff:** llama.cplus shim files merge their `gen-enum-mirror.py` constants into one canonical file imported by each shim, instead of hand-duplicating. Every consumer of the swap-and-link pattern (port-an-existing-C-codebase) benefits.

---

### Phase 8 — `[link] extra-objects = [...]` in Cplus.toml · size M

**The gap**, from the llama.cpp port ([cpc-gaps.md G-001](../llama.cplus/cpc-gaps.md)): the reference build embeds Metal shader source via a sed-preprocessed `.metal` file `incbin`'d into an assembly source that produces a `.o` exporting `_ggml_metallib_start` / `_ggml_metallib_end`. The standalone C+ build needs to link that `.o` alongside the C+-emitted ones. Today the workaround is a wrapper script that calls `cpc build`, then re-links with `clang` passing the extra `.o`. Ugly and breaks `cpc build` as the single source of truth.

**Locked design decisions:**

1. **Manifest field:** new top-level `[link]` table in `Cplus.toml` with an `extra-objects` array of strings. Paths are resolved relative to the manifest directory.

   ```toml
   [link]
   extra-objects = ["build/metallib.o", "build/shader_blob.o"]
   ```

2. **Link order:** extra-objects appear on the linker command line **after** the C+-emitted objects but **before** the system libraries (`-lc`, `-lobjc`, etc.). This matches the conventional GCC link order — your `.o` exports symbols the C+ code references; the libraries close any remaining undefined symbols.
3. **No glob support** for the first cut. Globbing adds a portability surface (Windows vs POSIX glob semantics) for marginal benefit; explicit listing is clearer for the small N (~1-5 extra objects per project) we expect. If a workload surfaces with 50+ extra objects, revisit.
4. **No build-script execution.** cpc doesn't run `make` to produce the extra `.o`. The user is responsible for producing them out-of-band (typical pattern: a Makefile / shell script invokes `clang -c metallib.s -o metallib.o` before `cpc build`). cpc just links what's there. If the file is missing, fail with **E0411-LINK** ("manifest [link] extra-objects entry `<path>` not found").
5. **Field name:** `extra-objects` (kebab-case, matching Cargo's `crate-type` / `default-features` style). Not `extra_objects` (snake_case would clash with v0.0.7's `[[bin]] name` style; the manifest is consistently kebab-case for multi-word keys).
6. **Scope:** the field is honored for `[[bin]]` and `[lib]` targets equally. The same `.o` is appended to whatever final-link cpc does.

**Scope:**

- Manifest parser: extend [cplus-core/src/manifest.rs](cplus-core/src/manifest.rs) to admit `[link] extra-objects = [...]` as a `Vec<PathBuf>`. Unknown fields under `[link]` warn but don't error (forward-compat for future link-related knobs like `extra-flags`).
- Driver: extend the link-line construction in [cpc/src/main.rs](cpc/src/main.rs) to append each extra-object path (validated to exist at link time) before the system libs.
- Diagnostic: E0411-LINK for missing files.
- Manifest docs: tutorial §9 and SKILL.md §9 add a "Linking against pre-built C objects" subsection.

**Tests:** unit (manifest parses the new field; missing-file error path) + e2e (a two-target project where the C+ binary calls into a function defined in a hand-written `.c` file → compiled via `clang -c` → listed in `extra-objects`).

**Expected payoff:** llama.cplus drops the wrapper-script around `cpc build`. The standalone `cpc build` becomes the single source of truth for the port's link line. Any other project that needs to link prebuilt C / assembly / shader-blob objects works the same way.

---

### Phase 9 — Generic `HashMap[K, V]` lock-down (cpc-gaps G-002) · size S

**The discovery**, while preparing the implementation: `HashMap[K, V]` already shipped in **v0.0.4 Slice 3B.5**. The cpc-gaps report was stale. The implementation at [vendor/stdlib/src/hash_map.cplus](vendor/stdlib/src/hash_map.cplus) supports any primitive `K` (`i8`/`i16`/`i32`/`i64`/`isize`/`u8`/`u16`/`u32`/`u64`/`usize`/`str`) and any `Copy` `V`, with blessed `Hash` / `Eq` lowering in sema ([is_blessed_hash_receiver](cplus-core/src/sema.rs#L6562) + [is_blessed_eq_receiver](cplus-core/src/sema.rs#L6582)). Open-addressing + linear probing + 0.75 load-factor grow.

The work for this slice collapses from "implement" to **verify + lock down**: prove the surface works across every (K, V) combination the llama port needs, and add a regression-pinning consumer so a future refactor of the blessed paths can't silently break it.

**Existing surface (not re-implemented):**

- `pub struct HashMap[K, V] { keys, vals, occupied, cap, len }` — three parallel arrays, no per-entry padding.
- `pub fn new[K, V]() -> HashMap[K, V]` — empty map; first insert allocates.
- `HashMap[K, V]::insert(mut self, k: K, v: V)`, `get(self, k) -> Result[V, IoError]`, `contains_key(self, k) -> bool`, `len(self) -> usize`, `capacity(self) -> usize`, plus `drop`.
- Blessed `K.hash() -> u64` and `K.eq(other) -> bool` for every primitive `K` (sema short-circuits before the normal method-lookup path).
- `StrIntMap` retained as `pub fn new_str_int_map() -> HashMap[str, i32]` thin alias for backwards compatibility.

**Lock-down work delivered:**

- New consumer project at [docs/examples/projects/hash_map_combos/](docs/examples/projects/hash_map_combos/) exercises six (K, V) shapes end-to-end: `[str, i32]`, `[str, u64]`, `[i32, i32]`, `[u64, u32]`, `[i64, bool]`, plus a 100-entry grow workload that forces ≥ 2 doublings from the initial 16-slot capacity.
- New e2e test in [cpc/tests/e2e.rs](cpc/tests/e2e.rs) runs the project end-to-end against the in-tree stdlib and asserts on its stdout (`hash_map combos: 6/6 ok\n`).
- Existing [docs/examples/projects/stdlib_smoke/](docs/examples/projects/stdlib_smoke/) continues to cover the `[str, i32]` overwrite + miss surface (no changes needed).

**Known scope limit (deferred):** non-Copy `V` (e.g. `HashMap[str, string]`) currently double-frees on `get` because the get-path bit-copies the value out without aliasing tracking. The hash_map.cplus source already documents this as a future slice; the llama port's actual needs (vocab, BPE ranks, tensor-name dispatch) are all `Copy V` and unaffected.

---

## Phase ordering rationale

- **Phase 1 first.** It's a backwards-compat break + a tutorial rewrite; doing it early lets every other v0.0.9 source change land under the new default. Doing it late means rewriting Phase 2/3 sources twice.
- **Phase 2 alongside Phase 1.** Independent code path (lexer + parser); doesn't conflict with Phase 1 sema work. Can ship together.
- **Phase 3 after Phase 1/2.** The bug repros may overlap with the new sema rules; want the new defaults in place first so the repros are clean.
- **Phase 4 standalone.** Purely additive (new AST node + parser rule + lowering substitution); doesn't touch sema defaults or codegen. Can land in any order — slot opportunistically alongside Phase 2 if a session has spare capacity. Priority bump if the llama port reaches its Phase 4 (vocab) and starts blocking on G-015.
- **Phase 5 standalone.** Needs the bench harness updated (multi-thread column in bench.sh / bench.md). Independent of the gap-closure phases (6-9).
- **Phase 6 first of the gap-closure group.** Smallest patch (sema-only, no codegen change beyond `ptrtoint`); unblocks llama.cplus Phase 1 immediately. Sequencing it first means the rest of the gap-closure work can use `p as usize` in tests if it wants to.
- **Phase 7 after 6.** The driver refactor it requires is independent of Phase 6's sema change; ordering here is just smallest-blast-radius first.
- **Phase 8 after 7.** Manifest field is additive; the link-pipeline change touches code that Phase 7 will have just refactored, so it's cheaper to land second.
- **Phase 9 last.** Largest by far — stdlib work plus the new `Hash` / `Eq` interfaces; needs the sema/lower paths from earlier phases to be stable. Slip to v0.0.10 if v0.0.9 runs long.

Estimated effort across all phases: ~9-11 sessions aggregate. v0.0.9 ship target: 5 of those sessions if Phase 5 + Phase 9 slip; full 9 if everything goes clean. The hard cut would be slipping 5 and 9 to v0.0.10 (raytracer threading + generic HashMap), since both can stand alone as standalone releases.

---

## Open questions (do not block phase work)

- **Per-field TBAA tree** — v0.0.7 Slice 1.2 punted with "ship when raytracer perf measures the win." The v0.0.8 raytracer is at 0.94 s, ahead of C; not a clear win available here. The work sits unless a workload (gemm / image processing) makes it measurable.
- **Mask types as a distinct `Ty` variant** — v0.0.7 Slice 2.1 aliased `mask32x4` to `i32x4`. No bug has surfaced from the aliasing in v0.0.8's `vendor/simd` consumer. Stays as-is until one does.
- **Submodule re-export through `appkit/appkit` facade for functions** — re-litigated three cycles in a row. No second bindings package in v0.0.9 (sqlite was dropped); rubric decision waits for one to land.
- **`#[align(N)]` for struct fields** — v0.0.6 cut, v0.0.7 deferred, v0.0.8 Phase 1B didn't surface a need (the SIMD raytracer worked on `f32x4` directly with no alignment trap). Stays cut until a real consumer hits a misalignment.
- **`option_env!()`** — explicitly rejected in Phase 4. If a workload genuinely needs "optional build-time config" and the empty-string sentinel pattern isn't enough, revisit.
- **`borrow` keyword reservation** — Phase 1 introduces it. Lexer already reserves the token (v0.0.5 borrow regions); v0.0.9 widens the use to value-typed params. No new lexer work.
