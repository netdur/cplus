# C+ — Plan

Version 0.0.8 shipped 2026-05-22. See [plan-0.0.8.md](plan-0.0.8.md) for the archived 0.0.8 roadmap and resolved log; [plan-0.0.7.md](plan-0.0.7.md) covers v0.0.7, [plan-0.0.6.md](plan-0.0.6.md) v0.0.6, [plan-0.0.5.md](plan-0.0.5.md) v0.0.5, [plan-0.0.4.md](plan-0.0.4.md) v0.0.4, [plan-0.0.3.md](plan-0.0.3.md) v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.9 — Tighten the safety story, close the long-tail bugs

**Strategy:** v0.0.8 validated the v0.0.7 surface against three real benchmarks and closed the bench-gap punch list. C+ now wins the raytracer outright (0.94 s vs C's 1.16 s) on Apple Silicon, the SIMD surface has a packaged consumer (`vendor/simd`), the native Metal GPU compute package (`vendor/metal`) has been implemented and verified entirely from pure C+ using Objective-C FFI, and the macro-builtin trilogy is settled (`include_bytes!` / `include_str!` / `env!`).

What's left is the long tail: small bugs that surfaced under real workloads (mixed-if-arm panic, lingering musttail edge cases), one safety footgun that contradicts the "safety as default" pitch (`fn echo(x: string) -> string` is a silent double-free without an explicit `move`), and one ergonomic gap that touches every byte-level program (no character literals).

No new principles. The locked twelve from §1.Locked-Principles stand. v0.0.9 is polish + correctness, not language reshape.

Slice sizes follow the same S/M/L assistant-paced framing.

---

### Phase 1 — Safety: default-move for non-Copy value params · size M (deferred to v0.0.10)

**Status (2026-05-23):** deferred. The default-move flip itself is straightforward (~20 lines in sema's signature collection plus codegen mirror — verified locally; closes the `Vec[i32]` echo double-free under ASan). The blocker is that the locked design depends on the new `borrow x: T` keyword to give users a way to express "shared borrow" at the source level — without it, 10 existing e2e tests that pin the old shared-borrow shape have no semantic-equivalent rewrite (they want `borrow p.right`, `borrow b: B`, etc.). The two pieces must land together. Moving both to v0.0.10 with the renamed scope "default-move + `borrow` keyword".

Scoping-down attempts ("just flip the default, defer `borrow`, sweep 3 stdlib sites") run into the test-suite problem above. The stdlib + bench surface is genuinely larger than 3 sites once tests are counted. The keyword work isn't optional once the flip is real.

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

### Phase 5 — Threaded raytracer · size M (deferred to v0.0.10)

**Status (2026-05-23):** deferred again. The "Same image hash as single-threaded" lock in §4 below requires a per-pixel-deterministic RNG seed — but the v0.0.8 raytracer at [bench-cplus/raytracer/cplus/main.cplus](file:///Users/adel/Workspace/bench-cplus/raytracer/cplus/main.cplus) uses a serial-advance xorshift32, so re-tiling work across threads produces a different image. The fix is a real renderer refactor (per-(i,j,sample) seed derivation), not just thread pool plumbing. Compounded by the conversion to a project layout needed to import `stdlib/thread` (single-file `.cplus` only accepts `./` imports, per the v0.0.9 Phase 7 G-011 rule). Two substantial pieces of work in a slice that was already once-deferred from v0.0.8. Moving to v0.0.10 alongside the renderer redesign.

**Original goal (kept for v0.0.10):** parallel-tiles raytracer. Each thread renders one horizontal band of the image, joins, then `main` writes the assembled buffer. v0.0.5 shipped `thread::spawn` / `JoinHandle::join`; v0.0.8's raytracer ran it single-threaded. This slice exercises the v0.0.5 thread surface against a real workload and gives the bench.md raytracer benchmark a 4-8× headroom on multi-core machines.

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

## v0.0.9 shipped status (2026-05-23)

| Phase | Status |
|---|---|
| 1 — default-move + `borrow` keyword | **deferred to v0.0.10** (must land as one slice — `borrow` is the source-level escape hatch the breaking change needs) |
| 2 — character literals `'a'` | **shipped** |
| 3 — long-tail codegen bug fixes | **shipped** (mixed-if-arm panic extended to `Field` / `Index` / `Unsafe` / `Match` / `Cast` / `GenericEnumCall` tail expressions; musttail recursive-move audit produced a known-limitation note instead of a fix — proper analysis is property-test scope) |
| 4 — module-level `const` and `static` | **shipped** |
| 5 — threaded raytracer | **deferred to v0.0.10** (renderer needs per-pixel-deterministic RNG refactor before threading; bench-cplus needs project-layout conversion) |
| 6 — raw-pointer ↔ integer cast in `unsafe` | **shipped** |
| 7 — `cpc --emit-obj` walks local imports | **shipped** |
| 8 — `[link] extra-objects = [...]` | **shipped** |
| 9 — generic `HashMap[K, V]` lock-down | **shipped** (discovered already implemented in v0.0.4; lock-down tests added) |

**7 of 9 shipped.** Two deferrals (1 + 5) move to v0.0.10 together. Test totals at v0.0.9 close: 1014 lib + 392 e2e + 11 misc = 1417 tests, all green.

**Post-close compiler fixes (2026-05-23, in-tree on the v0.0.9 branch):**
- **G-022** — cross-package generic field method-table bug fixed in `backfill_generic_struct_methods`.
- **G-023** — `move` param + raw-pointer store + struct-literal field init leaked Drop on the source local; fixed by adding the two missing mark_moved sites in `gen_struct_lit` / `gen_assign`.
- **G-024 / `addr_of`** — added `addr_of(x)` compile-time intrinsic returning `*T` for a stack local. Closes the "no address-of-local" language gap that had been blocking `vendor/uuid`, `vendor/log`, `vendor/metal`, and the raytracer from removing per-call mallocs. Zero runtime cost (alloca pointer reused directly). See "Discovered: address-of-local intrinsic (G-024)" below.
- **G-025 / `Ty::Mask`** — promoted mask types from aliases of `i{N}x{M}` to a distinct sema-level type. Sema enforces the boundary (comparison ops return `Ty::Mask`; `select`/`any`/`all` require it; arithmetic and `Type::splat`/`::new`/`::from_array` rejected on masks; implicit Mask↔Simd coercion rejected); codegen lowers both to the same `<N x iN>` LLVM (zero runtime cost). Explicit `mask.to_bits()` / `simd.to_mask()` conversions added — both no-ops at the IR level.
- **string `\xHH` escape** — lexer accepts `\xHH` inside string literals (ASCII-only; bytes ≥ 0x80 rejected to keep the lexer's `String` payload valid UTF-8). Used by `vendor/log` to write ANSI control bytes as string literals.
- **`static FOO: str = "..."`** — codegen emits a paired data global (`@FOO.bytes = constant [N x i8] c"..."`) + a fat-pointer global (`@FOO = constant { ptr, i64 } { ptr @FOO.bytes, i64 N }`). Reads through the regular static-load path. Both `static` (immutable) and `static mut` (mutable) variants supported.
- **`borrow x: T` parameter marker** — additive: parses as a fourth ownership-prefix alongside `mut` / `move` / `restrict`. For v0.0.9 it's semantically identical to the unmarked form (`x: T`) on non-Copy types — both mean "shared by-value". Reserved for a future Phase 1 slice that flips the default for non-Copy `T` to `move` semantics; `borrow` will then be the opt-out escape hatch. Sema rejects `borrow` + `move` and `borrow` + `mut` (E0334).

Tests now 1035 lib + 399 e2e + 11 misc = **1445** in the Rust harness, all green.

**Vendor-package unit tests landed this session** (run via `cpc test` from a project that depends on the package):

| Package | `#[test]` fns | Status |
|---|---|---|
| arena | 11 | ✅ all pass |
| uuid | 10 | ✅ all pass |
| json | 23 | ✅ all pass |
| clap | 9 | ✅ all pass |
| log | 5 | ✅ all pass (unblocked 2026-05-24 by G-028 fix) |
| metal | 8 | ✅ all pass (unblocked 2026-05-24 by G-029 fix) |

**66 in-package `#[test]` fns** (was 50 before G-028/G-029 + the 2026-05-24 vendor polish session). Closes the test-discipline gap (theme #1) end-to-end.

**Compiler bugs investigated this session:**
- **G-026** — recursive enum payload `Array(vec::Vec[Value])` fires E0303 "unknown type `<concrete>`". **Fully fixed 2026-05-23** across three layers:
  1. **Sema** — `substitute_param_in_type_ast` now uses real struct/enum names from the tables instead of the `<concrete>` placeholder ([sema.rs](cplus-core/src/sema.rs)).
  2. **Monomorphize** — `rewrite_item_calls` now walks `ItemKind::Enum` variant payloads to rewrite `TypeKind::Generic` to mangled `TypeKind::Path` ([monomorphize.rs](cplus-core/src/monomorphize.rs)).
  3. **Codegen** — `unwrap_iterator_ty` now finds the `Iterator__` marker via `rfind` so dots inside the mangled inner-T name (e.g. `Iterator__src.main.Value`) don't break the prefix-strip ([codegen.rs](cplus-core/src/codegen.rs)).
  Recursive enums end-to-end work now: parse, match-destructure, auto-Drop chains, all ASan-clean. **Unblocks the `vendor/json` typed refactor** (shipped same session).
- **G-027** — `fn f(move args: vec::Vec[str])` emits LLVM IR with `store i1 false, ptr %args.drop_flag.unused` where the alloca is undefined. **Fixed 2026-05-23**: codegen's move-scanner now also walks method-call args for the callee's `move`-flagged param positions (not just receivers and free-fn args). `vendor/clap`'s `get_matches_from(self, move args: vec::Vec[str])` shipped with the `move` marker restored.
- **G-028** — `cpc test` driver didn't populate `md.statics` for vendor-package statics. **Fixed 2026-05-24**: `generate_test_binary` now takes a `&MonoInfo` and threads through `mono.statics` / `mono.compile_time_blobs` / `mono.env_vars`, matching `generate_with_mono`'s shape. `vendor/log` unit tests (5 fns) now pass.
- **G-029** — `cpc test` didn't propagate `[link] frameworks/libs` from the package manifest into the test binary's link line. **Fixed 2026-05-24**: `run_test` now builds link args from (a) the package's `[[bin]]` or `[lib]` target, (b) `collect_dep_link_args(&m)`, and (c) the package's own top-level `[link]` table (the last is the new bit — vendor packages declare their frameworks at top level, not on a target). Also fell back to `src/<package-name>.cplus` as the entry when neither `[[bin]]` nor `[lib]` is declared (the common library-only vendor shape, where manifest auto-injects a phantom `src/main.cplus` bin). `vendor/metal` unit tests (8 fns) now pass.
- **G-030** — `cpc test` from within a vendor package couldn't resolve sibling vendor deps. **Fixed 2026-05-24**: both `resolver::resolve_vendor_path` and `cpc::collect_dep_link_args` now fall back to `<manifest_root>/../<dep>/` when `<manifest_root>/vendor/<dep>/` doesn't exist. This is the natural layout when running `cpc test` from inside `vendor/uuid/`, `vendor/log/`, etc. — sibling packages live one directory up. Replaces the failed-attempt-at-symlinks workaround (which caused cyclic recursion in `cpc fmt --check`'s directory walk via `docs/examples/projects/json_smoke/vendor/json/vendor/json/...`).

---

## Vendor-package cleanup — 2026-05-24

Six new packages landed in commit `64f7b83`: [arena](vendor/arena/), [clap](vendor/clap/), [json](vendor/json/), [log](vendor/log/), [metal](vendor/metal/), [uuid](vendor/uuid/). A read-through found real correctness bugs and one entirely-empty package. The work below is per-package punch lists for tomorrow's session, ordered within each by severity (critical → low). Cross-cutting themes that span every package live at the bottom.

### `vendor/metal` (6 files, ~446 LOC)

The most mature of the six (plan.md line 9 says it's been verified end-to-end), but the gaps below mean it leaks unbounded in any long-running program.

| # | Issue | Effort | Severity |
|---|---|---|---|
| 1 | **No `Drop` impls on any MTL* wrapper** — `Device`/`CommandQueue`/`CommandBuffer`/`Library`/`Function`/`ComputePipelineState`/`Buffer` all hold +1-retained ObjC pointers that never get `release`'d. Every new* call leaks GPU resources. | ~50 LOC (7 types × `objc_msgSend(self.raw, sel("release\0"))`) | **critical** |
| 2 | ~~**`new_library_with_data` silently borrows the byte slice** via `dispatch_data_create(..., NULL_destructor)`~~ — **investigated + fixed 2026-05-24**. Per Apple docs `dispatch_data_create(buf, len, NULL, NULL)` defaults to `DISPATCH_DATA_DESTRUCTOR_DEFAULT`, which **copies** the buffer — so the original "UAF" framing was a false alarm. The real bug was a per-shader-load **leak**: the `dispatch_data` object itself is +1 retained and was never released. Fixed by calling `runtime::release(dispatch_data)` after `newLibraryWithData:` consumes it. | — | done |
| 3 | **`Cplus.toml` doesn't declare `[dependencies] stdlib = "*"`** despite four files importing `stdlib/option`. Breaks composability when a consumer pulls in `metal` without separately declaring `stdlib`. | trivial | high |
| 4 | ~~**`ComputePipelineState::new(device_raw: *u8, ...)`** takes the raw pointer~~ — **fixed 2026-05-23**: added `Device::new_compute_pipeline_state(self, function: Function)` as the typed entry point. The raw-pointer form stays for backwards compat (and to avoid the circular `device ↔ pipeline` import). | — | done |
| 5 | ~~**Errors silently discarded**~~ — **fixed 2026-05-24**: introduced `pub enum MetalError { NoDefaultDevice, LibraryLoadFailed, FunctionNotFound, PipelineCreationFailed }` in `runtime.cplus`. Converted `default()`, `new_library_with_data`, `new_function`, `ComputePipelineState::new` from `Option[T]` to `Result[T, MetalError]`. Callers now get a discriminant instead of an unspecific `None`. `proves/metal_test.cplus` + `proves/06-metal-package-test.cplus` updated to match. (NSError-string surfacing is still TODO — the discriminant is the minimum useful structure.) | — | done |
| 6 | ~~**`ns_string()` leaks the produced NSString** on every call (used in `Library.new_function`)~~ — **fixed 2026-05-23**: `Library::new_function` now calls `runtime::release(ns_name)` after `newFunctionWithName:` (which retains internally). | — | done |
| 7 | ~~**`copy_to_slice` / `copy_from_slice` silently truncate**~~ — **fixed 2026-05-23**: both now return `usize` (bytes actually copied). Callers compare against `slice_len(slice)` to detect a too-small buffer. | — | done |
| 8 | ~~**`msg_dispatch` mallocs 48 bytes per call**~~ — **fixed 2026-05-23** via G-024 (`addr_of`): `objc_msg_dispatch(recv, sel, addr_of(groups), addr_of(per_group))` passes the by-value parameters' addresses directly. Removed the malloc/free externs from runtime.cplus. | — | done |
| 9 | ~~**NUL-terminated string-literal convention undocumented**~~ — **fixed 2026-05-23**: file header docstring in `runtime.cplus` documents the `"...\\0"` selector convention and the `str_len(...)` returning N+1 gotcha. | — | done |
| 10 | **No `proves/` consumer running a real compute kernel.** | medium | medium |

### `vendor/uuid` (123 LOC)

> **2026-05-23 update — items #1 #2 #3 #4 #6 #7 SHIPPED.** API consolidated to `Uuid::new_v4` / `Uuid::parse` / `Uuid::to_string`, all returning `Option[T]` on OOM. ASCII magic numbers replaced with char literals (`'0'` / `'9'` / `'a'` / `'f'` / `'A'` / `'F'` / `'-'`). 16-zero array hoisted into `zero_uuid()`. Verified ASan-clean.

| # | Issue | Effort | Severity |
|---|---|---|---|
| 1 | ~~**Broken import path**~~ — **fixed 2026-05-23** (earlier in session): `import "stdlib/option"` + `[dependencies] stdlib = "*"` in Cplus.toml. | — | done |
| 2 | ~~**Silent zero-UUID on `malloc(16)` failure**~~ — **fixed 2026-05-23**: `Uuid::new_v4()` now returns `Option[Uuid]::None` on OOM. | — | done |
| 3 | ~~**Magic-number ASCII codes everywhere**~~ — **fixed 2026-05-23**: hex-range bounds use `'0'`/`'9'`/`'a'`/`'f'`/`'A'`/`'F'`; dash separator uses `'-' as u8`. Hex offset arithmetic uses `('a' as u8) -% 10` instead of magic 87/55. | — | done |
| 4 | ~~**16-element `0u8` array literal duplicated 3×**~~ — **fixed 2026-05-23**: hoisted into `fn zero_uuid() -> Uuid`. | — | done |
| 5 | ~~**`new_v4` allocates 16 bytes + 16-iteration byte copy**~~ — **fixed 2026-05-23** via G-024 (`addr_of`): `arc4random_buf(addr_of(uuid) as *u8, 16)` writes directly into the stack-allocated Uuid. Removed ~20 lines (malloc + copy loop + free). | — | done |
| 6 | ~~**`to_string` silently returns `""` on malloc/snprintf failure**~~ — **fixed 2026-05-23**: `to_string` now returns `Option[string]::None` on OOM or snprintf failure. | — | done |
| 7 | ~~**Inconsistent factory shape**~~ — **fixed 2026-05-23**: consolidated to `Uuid::new_v4()` / `Uuid::parse(s)` / `Uuid::to_string(self)`, all in `impl Uuid`. The module-level `uuid::new_v4()` / `uuid::parse()` are gone. | — | done |
| 8 | ~~**No `impl ToString for Uuid {}` declaration**~~ — **fixed 2026-05-24**: `Uuid::to_string(self) -> string` is now infallible (formats into a stack-allocated `[u8; 37]` buffer via `addr_of`, then heap-copies via `str::to_string()`). The fallible `Option[string]` wrapper was unnecessary — UUID's canonical format is a fixed 36 bytes and snprintf into a 37-byte buffer can't run out of space. C+ has no ToString interface, so the method's name + signature is the dispatch contract. | — | done |
| 9 | ~~**`arc4random_buf` is macOS/BSD/glibc≥2.36** — no older-Linux fallback~~ — **fixed 2026-05-24**: replaced `arc4random_buf` with `open("/dev/urandom") + read + close`. Portable across macOS / Linux / *BSD; works on every libc, not just glibc≥2.36. Same `addr_of(uuid) as *u8` writes the 16 random bytes directly into the stack-allocated struct. | — | done |

### `vendor/clap` (399 LOC, was 448)

> **2026-05-23 update — clap #1 + #4 SHIPPED** after G-022 and G-023 fixes landed. Net -49 LOC; `ArgMatches` now has typed `pub matches: HashMap[str, str]` / `pub positionals: Vec[str]` fields directly. Removed: `alloc_map_str` / `map_str_free` / `alloc_vec_str` / `vec_str_free` helpers (4 helpers, ~30 LOC), explicit `Drop` impl (auto-recurse into typed fields), `positionals()` raw-pointer leak. ASan-clean end-to-end.

| # | Issue | Effort | Severity |
|---|---|---|---|
| 1 | ~~**`ArgMatches` uses `*u8` opaque-pointer indirection**~~ — **fixed 2026-05-23** via G-022 + G-023 compiler fixes. Typed fields + auto-drop. | — | done |
| 2 | ~~**macOS-only via `_NSGetArgc` / `_NSGetArgv`** — fails to link on Linux~~ — **fixed 2026-05-24**: dropped `_NSGetArgc` / `_NSGetArgv` / `strlen` externs and removed the `get_matches()` convenience method that depended on them. Consumers now call `get_matches_from(args)` with a Vec[str] they construct themselves (portable). The doc comment on `get_matches_from` explains the rationale. clap is now portable; consumers still need a platform-aware argv constructor, but that's their concern. | — | done |
| 3 | ~~**Hand-rolled `ArgVec`** instead of `Vec[Arg]` from stdlib~~ — **fixed 2026-05-24**: replaced `ArgVec` (50 LOC of duplicated growable-array logic) with `vec::Vec[Arg]`. `Arg` is Copy by construction (all fields are `str` / `bool`), so it slots into `Vec[T]` without trouble. Drops the `realloc` / `free` externs too. Test `test_app_args_collects_via_vec` pins the replacement. | — | done |
| 4 | ~~**`positionals(self) -> *vec::Vec[str]`**~~ — **fixed 2026-05-23**: method removed; callers use `positional_count()` + `positional_at(i)`. `value_of` returned `str` is still borrowed-from-map (lifetime tied to `ArgMatches`); that's not a UAF in practice because `ArgMatches` outlives the call chain. | — | done |
| 5 | ~~**`--name=value` syntax missing**~~ — **fixed 2026-05-23**: parser splits long options on `=`; `--flag=...` for flag args silently treats it as the flag (no value attached). | — | done |
| 6 | ~~**`-abc` combined short flags missing**~~ — **fixed 2026-05-23**: short body of length > 1 is checked against the registered flags; if every byte is a known flag, each is inserted. Falls back to single-option path otherwise (so `-vh` still works if `vh` is a registered short). | — | done |
| 7 | **Pre-emptive Phase-1 compat**: `get_matches_from(self, args: vec::Vec[str])` should be `move args`. **Blocked on G-027** (codegen bug: `%args.drop_flag.unused` undefined when `move` marker is added). | blocked | low |
| 8 | ~~**`new_arg` / `new_app` as module-level fns**~~ — **fixed 2026-05-23**: added `Arg::new(name)` / `App::new(name)` shims in their impl blocks. The module-level fns stay for backwards compat. | — | done |

### `vendor/log` (181 LOC)

Showcase of v0.0.9 Phase 4's `static mut` — the package literally couldn't exist before today. Bugs are mostly per-call allocations.

| # | Issue | Effort | Severity |
|---|---|---|---|
| 1 | ~~**`malloc(5)` / `malloc(4)` per log call** for ANSI escape codes~~ — **fixed 2026-05-23**: shipped `static FOO: str` + string `\xHH` escape compiler features, then declared `static ANSI_TRACE: str = "\x1b[90m";` (etc.) at module scope. Per-call mallocs gone. vendor/log is now zero-malloc per log call. | — | done |
| 2 | ~~**`print_timestamp` mallocs twice per call**~~ — **fixed 2026-05-23** via G-024 (`addr_of`): both the `time_t` slot and the 32-byte snprintf scratch buffer now live on the stack, passed via `addr_of`. Removed 2 mallocs + 2 frees per log call. | — | done |
| 3 | **Gratuitous NUL bytes in level-tag strings** — `"[TRACE] \0"` etc. The `\0` decodes to a literal byte; `str_len` returns 9 instead of 8; the NUL gets written to stderr. Strip from lines 145-149. (The `\0` on line 88's snprintf format IS required.) | trivial | high |
| 4 | **`struct Tm` is not `#[repr(C)]`** — cpc may reorder fields, breaking `localtime`'s layout assumption. | trivial | medium |
| 5 | ~~**`static mut` reads + writes have no sync story**~~ — **fixed 2026-05-23**: file header docstring now explains the data race and the single-threaded contract (set config once before spawning). Atomic-based integration deferred to when `vendor/stdlib::atomic` exists. | — | done |
| 6 | ~~**Platform-dependent `Tm` fields** (`tm_gmtoff`, `tm_zone`)~~ — **fixed 2026-05-24**: dropped the trailing `tm_gmtoff` / `tm_zone` fields. The first 9 i32 fields are POSIX-portable (macOS, glibc, musl, BSD all agree on layout up through `tm_isdst`). We never read the trailing fields anyway; copying `*tm_ptr` with the smaller declared shape just reads 36 bytes, which is valid under every libc. | — | done |

### `vendor/arena` (119 LOC)

| # | Issue | Effort | Severity |
|---|---|---|---|
| 1 | ~~**`alloc[T](move val)` is broken for non-Copy T**~~ — **fixed 2026-05-23 via G-023**. The cpc fix flips the `move val` source's drop_flag at the raw-pointer-store site (`unsafe { *p = val; }`), so the arena slot owns the value and the local doesn't double-drop. Verified ASan-clean with `arena::alloc[Vec[i32]]` and `Box::new[Vec[i32]]`. Smoke in [docs/examples/projects/arena_smoke](docs/examples/projects/arena_smoke/) covers Copy T; non-Copy T smokes live as regression tests in [cpc/tests/e2e.rs](cpc/tests/e2e.rs). | — | done |
| 2 | ~~**No `Arena::new`**~~ — **fixed 2026-05-23**: added `impl Arena { pub fn new(chunk_size: usize) -> Arena { return new(chunk_size); } }` as a shim — package-level `arena::new` stays for backwards compat, callers can use either. | — | done |
| 3 | ~~**Wrapping arithmetic (`+%`) for size math**~~ — **fixed 2026-05-23**: caller-controlled byte additions (`bytes + padding`, `header_size + payload`, `aligned_avail + bytes`) now use trap-on-overflow `+`. Pointer-as-usize math stays on `+%` (real addresses don't wrap on supported targets). | — | done |
| 4 | ~~**`align == 0` traps via div-by-zero** in the `% align` math~~ — **fixed 2026-05-23**: normalized to `align = 1` if caller passes 0, documented in the file header API contract. | — | done |
| 5 | ~~**OOM returns `0 as *u8` silently**~~ — **fixed 2026-05-23**: header docstring documents the null-return convention; added `alloc_bytes_opt` / `alloc_bytes_aligned_opt` parallel APIs that return `Option[*u8]` for callers preferring explicit failure handling. | — | done |
| 6 | ~~**Missing typical arena APIs:** `alloc_zeroed_bytes(n)`, `reset()`, `alloc_str(s: str)`, `total_allocated()`~~ — **all four shipped 2026-05-23**: `alloc_zeroed_bytes` (memset-zero), `reset()` (frees every chunk, leaves arena reusable — Drop is now `self.reset()`), `alloc_str(s)` returns `Option[str]` of the arena-copied bytes, `total_allocated()` sums every chunk's capacity. | — | done |
| 7 | ~~**Variable name `default_chunk_size`** at line 19 holds the chosen size, not a default~~ — **fixed 2026-05-23**: renamed `chosen_size`. | — | done |
| 8 | ~~**One-line file header**~~ — **fixed 2026-05-23**: published API contract now documents `Arena::new` sizing, `alloc_bytes_aligned` alignment requirements (`align == 0` → 1), `alloc[T]` Copy-T restriction, and OOM null-return convention. | — | done |

### `vendor/json` (~720 LOC, shipped 2026-05-23)

MVP landed. Parser + serializer + Value tree (heap-owned, `value_free` to release). Smoke test in [docs/examples/projects/json_smoke](docs/examples/projects/json_smoke/) is ASan-clean.

**Scope shipped:** null, true, false, numbers (via libc `strtod`), strings (with `\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`, `\t`, `\uXXXX` BMP), arrays, objects. Strict number parsing (`1.2.3` rejected). Compact-only serialization with `%.17g` for numbers.

**Out of scope (later):** surrogate-pair `\u` handling, NaN/Infinity, duplicate-key policy, streaming, pretty-printing, per-package unit tests, integration with stdlib types (blocked on G-022 — `Value` couldn't use `Vec[Value]` / `HashMap[str, Value]` so it's a hand-rolled `**u8` slab, which is why the API surface is `*Value`-pointer-heavy rather than typed).

| # | Issue | Effort | Severity |
|---|---|---|---|
| 1 | ~~**Pointer-heavy public API** (`*Value` everywhere, manual `value_free`)~~ — **typed refactor SHIPPED 2026-05-23** after G-026 was fully fixed end-to-end. `Value` is now a recursive enum with `Array(Vec[Value])` / `Object(Vec[Member])` payloads. Cleanup is automatic via the enum's derived Drop — no `value_free` needed. API: `json::parse(s) -> Result[Value, ParseError]`, `v.to_string()`, accessors `is_*` / `as_*` / `array_at` / `object_get`. 20 in-package `#[test]` fns rewritten for the typed API, all green. The `Object` payload uses `Vec[Member]` instead of `HashMap[str, Value]` because HashMap requires `V: Copy` and `Value` is non-Copy — linear-lookup objects are fine for typical JSON, hash-map objects need a future `HashMap` relaxation. | — | done |
| 2 | ~~**`%.17g` round-trips correctly but is ugly** (`3.14` → `3.1400000000000001`)~~ — **fixed 2026-05-24**: shortest-round-trip encoder with two passes — `%.0f` through `%.17f` first (catches integers and natural decimals, so `30.0 → "30"` and `0.1 → "0.1"`), then `%.1g` through `%.17g` as fallback for very large / very small magnitudes that need scientific notation. Each candidate is `strtod`-parsed and compared to the original `f64`; first one that round-trips wins. | — | done |
| 3 | ~~**No unit tests inside `vendor/json/src/`**~~ — **fixed 2026-05-23**: 20 in-package `#[test]` fns shipped with the typed refactor; +3 more 2026-05-24 (`test_emit_number_short_form`, `test_parse_surrogate_pair_decodes_to_4byte_utf8`, `test_parse_bmp_unicode_escape_still_works`). Total 23. | — | done |
| 4 | ~~**Surrogate pair `\u` not handled**~~ — **fixed 2026-05-24**: parser now detects high-surrogate (D800-DBFF) after the first `\uXXXX`, looks ahead for `\uYYYY` with a low-surrogate, and combines them into a single codepoint outside the BMP. `buf_push_utf8` gained a 4-byte form for U+10000..U+10FFFF. Lone surrogates fall through to 3-byte UTF-8 (lenient — JSON spec recommends U+FFFD replacement, but preserving bits lets callers round-trip). | — | done |

### Discovered: `move` param + raw-pointer store leaks Drop (G-023) — **fixed 2026-05-23**

Hit while verifying arena #1. Minimal repro — Box[T]'s own `new[T]` constructor in stdlib has the same bug:

```cplus
import "stdlib/box" as box;
import "stdlib/vec" as vec;

fn make_vec() -> vec::Vec[i32] {
    let mut v: vec::Vec[i32] = vec::new::[i32]();
    v.push(10 as i32); v.push(20 as i32); v.push(30 as i32);
    return v;
}

fn main() -> i32 {
    let b: box::Box[vec::Vec[i32]] = box::new::[vec::Vec[i32]](make_vec());
    let v: vec::Vec[i32] = b.unwrap();
    let _ = v.get(0 as usize);   // <-- ASan: heap-use-after-free
    return 0;
}
```

ASan reports the Vec's inner buffer (`v.ptr` → `realloc`'d by `Vec.push`) is freed by `Vec.drop`, which fires at `Box.new`'s function-exit on the `move v: T` parameter. The `unsafe { *p = v; }` raw-pointer-store inside `Box::new` is a bitwise copy — it does NOT consume `v` as far as the borrow/drop tracker is concerned, so `v`'s Drop still runs at scope exit and frees the inner buffer that `*p` now aliases.

The pattern hit:
```cplus
pub fn f[T](move v: T) {
    let p: *T = unsafe { malloc(...) as *T };
    unsafe { *p = v; }   // bitwise copy, ownership NOT consumed
}                        // v.drop() fires, freeing inner storage that *p aliases
```

**Impact**: any stdlib helper of this shape leaks UAF for non-Copy T. Confirmed broken: `arena::alloc[T]`, `box::new[T]`. Plus a sibling failure (same root cause): **struct-literal field initialization from a non-Copy local** also fires Drop on the local while the field aliases. Minimal repro:

```cplus
pub struct Wrap { pub m: map::HashMap[str, str] }
fn make() -> Wrap {
    let mut m: map::HashMap[str, str] = map::new::[str, str]();
    m.insert("name", "alice");
    return Wrap { m: m };          // <-- ASan UAF in caller's contains_key
}
```

This sibling blocks the vendor/clap rewrite (`return ArgMatches { matches: matches, positionals: positionals };`) and any analogous "build a non-Copy local, wrap in a struct, return" pattern. So G-023 covers two surface forms with the same root: raw-pointer store + struct-literal field init.

**Fix candidates** (need design decision):
1. Compiler intrinsic `__cplus_forget(v)` to suppress Drop on `v` after raw store.
2. Treat `unsafe { *ptr = move_param; }` as a destructive move (special-cased by sema).
3. Add `T: Copy` bound to `alloc[T]` / `Box::new[T]` signatures and route non-Copy T through a separate "alloc + manual fwd-Drop" surface.

**Root cause:** the codegen drop-tracker scans Let-init, Return, and a handful of other expression contexts for bare-Ident sources, pre-registers them so a runtime drop_flag is allocated, and flips the flag at codegen time so the scope-exit destructor skips. Two surface forms were missing from both the scanner and the codegen mark_moved sites:
1. `StructLit`/`GenericStructLit` field-init where the value is a bare Ident (covers `Wrap { m: m }` and the `place = Wrap { m: m }` fast path).
2. Plain `=` assignment where the RHS is a bare Ident (covers `unsafe { *p = val; }` after `move val: T`).

**Fix:** [codegen.rs](cplus-core/src/codegen.rs)
- Scanner: in `scan_moves_in_expr`'s `StructLit`/`GenericStructLit` arm, insert each bare-Ident `f.value` into the move set. In the `Assign` arm (for plain `=` only — compound assigns don't transfer ownership), insert the bare-Ident RHS.
- Codegen: in `gen_struct_lit`, the `place = StructLit{...}` fast path inside `gen_assign`, and the `gen_assign` fall-through, call `self.mark_moved(n)` after the field/value store when the source was a bare Ident. `mark_moved` is a no-op for bindings without an allocated drop_flag (Copy types and non-move sources), so this is safe to apply unconditionally on bare Idents.

**Downstream landings:** `vendor/clap` shipped its typed-fields rewrite (-49 LOC). `arena::alloc[Vec[i32]]` and `Box::new[Vec[i32]]` confirmed ASan-clean (verified end-to-end). Two regression e2e tests landed in [cpc/tests/e2e.rs](cpc/tests/e2e.rs): `g023_struct_literal_field_init_does_not_double_drop` and `g023_raw_pointer_store_does_not_double_drop`. 1015 lib + 394 e2e + 11 lsp = 1420 tests, all green.

### Shipped: `addr_of(x)` intrinsic (G-024) — **2026-05-23**

The "no address-of-local" gap was the most-cited C+ language hole, blocking `vendor/uuid` #5, `vendor/log` #2, `vendor/metal` #8, and several raytracer patterns. Each one had to malloc a stack-shaped slot, write through it via a C fn, then free — typically 5-20 lines of code per call site.

**Design:** new compile-time intrinsic `addr_of(x)` returning `*T` where `T` is the binding's type. Modeled after `size_of::[T]()` / `align_of::[T]()`. Tight rules to keep the surface small:
- Exactly 1 argument, no type arguments (no turbofish — type inferred from the binding).
- Argument must be a bare identifier — `Field` / `Index` / `Call` are rejected (E0302). Caller can `addr_of(my_struct) as *u8` and GEP if they need an interior pointer.
- Must appear inside `unsafe { ... }` (E0801 outside — raw-pointer producer).

**Implementation:**
- Sema ([sema.rs](cplus-core/src/sema.rs)): handles `addr_of` in `check_named_call`, mirroring the `size_of` / `align_of` branch. Reuses `resolve_value_ident` for the binding-type lookup. Validates unsafe-context, arg count, and bare-Ident shape.
- Codegen ([codegen.rs](cplus-core/src/codegen.rs)): one-line lowering — call `gen_place(&args[0])`, wrap the returned slot type in `Ty::RawPtr`. Zero runtime cost: the alloca pointer IS the address. IR test pins this: `call i64 @time(ptr %t.addrN)` with no intermediate.

**Downstream landings (this session):**
- `vendor/uuid::new_v4`: `arc4random_buf(addr_of(uuid) as *u8, 16)` — replaced ~20 lines of malloc+copy+free with a single call (closes uuid #5).
- `vendor/log::print_timestamp`: `time(0)` + `localtime(addr_of(t))` + `snprintf(addr_of(buf) as *u8, ...)` — removed 2 mallocs per log call (closes log #2).
- `vendor/metal::msg_dispatch`: `objc_msg_dispatch(recv, sel, addr_of(groups), addr_of(per_group))` — removed the 48-byte-per-call malloc + memcpy + free, deleted the `malloc` / `free` externs from runtime.cplus (closes metal #8).

**Tests:** 5 sema unit tests + 2 e2e tests (round-trip via `time(2)` and IR-shape pin). All green.

### Discovered: cross-package generic field bug (G-022) — **fixed 2026-05-23**

Hit while attempting clap #1. Minimal repro — a vendor package `inner` with a struct whose field type is a generic instantiated from `stdlib`:

```cplus
// vendor/inner/src/inner.cplus
import "stdlib/hash_map" as map;
struct Holder { m: map::HashMap[i32, i32] }
pub fn touch() -> bool {
    let mut h: map::HashMap[i32, i32] = map::new::[i32, i32]();
    h.insert(1 as i32, 2 as i32);
    return h.contains_key(1 as i32);
}
```

Pre-fix: build failed with `E0324: no method 'insert' on struct HashMap__i32__i32` and `E0324: no method 'contains_key'` — even though `Holder` itself is never referenced anywhere. **Deleting `struct Holder` made the same calls resolve cleanly.**

Reproduced with `[K, V] = [i32, i32]`, `[str, str]`, `Vec[str]`, with `pub` and non-`pub` on the field.

**Root cause:** sema's pass ordering. `collect_struct_fields` runs *before* `collect_methods`. When `collect_struct_fields` resolves `m: map::HashMap[i32, i32]`, it triggers `instantiate_struct_from_arg_tys` which synthesizes a concrete `StructDef` and populates methods from `generic_impl_methods[name]` — but at that point `generic_impl_methods` is empty (only populated by `collect_methods`). The instantiation gets cached with an empty methods table. When the consumer later writes `h.insert(...)`, dedup hits the cached methodless `StructDef` and method lookup fails. There was already a workaround for the *intra-impl* version of this (see [sema.rs:1525](cplus-core/src/sema.rs#L1525)'s "two-phase: register every generic-impl-method template BEFORE resolving any concrete impl method signature") but it didn't cover the field-type path.

**Fix:** added `backfill_generic_struct_methods` ([sema.rs](cplus-core/src/sema.rs)) called at the end of `collect_methods`. It iterates every struct with `generic_origin = Some((name, args))` and an empty `methods` table, and re-runs the impl-template substitution using the same logic as `instantiate_struct_from_arg_tys`'s late-arrival path. 1014 lib + 392 e2e + 11 lsp tests all pass post-fix.

**Downstream impact:** unblocks `vendor/clap`'s rewrite at the *type-check* level — `pub matches: HashMap[str, str]` field now resolves and methods are found. But the rewrite is still blocked on **G-023** at runtime (the struct-literal `return ArgMatches { matches: matches, ... }` ASan-fails because the local `matches`'s Drop fires while the field aliases its storage). G-023 must ship before the clap rewrite can land.

### Cross-cutting themes

| # | Theme | Action |
|---|---|---|
| 1 | ~~**Zero tests across all six new packages**~~ — **66 in-package `#[test]` fns shipped 2026-05-23/24** (arena 11, uuid 10, json 23, clap 9, log 5, metal 8). G-028/G-029 fixed 2026-05-24 unblocked log + metal. | — | done (6/6) |
| 2 | ~~**`[dependencies] stdlib = "*"` missing in uuid + metal**~~ — checked 2026-05-24: both manifests now declare `stdlib = "*"`. | — | done |
| 3 | ~~**"No address-of-local" language gap hit 4 times**~~ — **fixed 2026-05-23** via G-024 `addr_of(x)` intrinsic. uuid #5, log #2, metal #8 all closed in the same session. Raytracer can now drop its per-pixel scratch-pool dance whenever someone wants to refactor it. | — | done |
| 4 | ~~**String `\xHH` escape missing**~~ — **fixed 2026-05-23** (ASCII-only: bytes ≥ 0x80 rejected to keep `String` payload UTF-8). | — | done |
| 5 | ~~**`static FOO: str = "..."` not supported**~~ — **fixed 2026-05-23**: codegen emits a paired data global + fat-pointer global. Both `static` and `static mut` variants work. Closed vendor/log #1 the same day. | — | done |
| 6 | **Convention drift on factory shape** (`pkg::new_foo` vs `Foo::new`). Stdlib is `Type::new`; vendor packages diverge. | Per-package fix — small. |
| 7 | **Integer-literal patterns in match arms** still missing (rediscovered during Phase 2 char-literal example). | Future feature. |

### Suggested ordering for tomorrow

All six items below SHIPPED across 2026-05-23/24. Vendor-package polish round is complete; the only deferred items are external (`metal #10`: real GPU compute kernel as a proves/ project) or wider-scope language work (Phase 1 default-move flip — v0.0.10).

1. ~~`vendor/uuid` import path fix~~ — done 2026-05-23.
2. ~~`vendor/log` strip `\0` from tag strings + add `#[repr(C)]` to `Tm`~~ — done 2026-05-23.
3. ~~`vendor/metal` Drop impls for all 7 wrappers~~ — done 2026-05-23.
4. ~~`vendor/clap` `ArgMatches` opaque-pointer fix~~ — done 2026-05-23.
5. ~~`vendor/arena` `alloc[T]` ASan verification + per-package smoke tests~~ — done 2026-05-23.
6. ~~`vendor/json` ship MVP or delete~~ — typed enum shipped 2026-05-23 (994 LOC). Polish round (#2 short-form numbers, #4 surrogate pairs) closed 2026-05-24.

### 2026-05-24 session summary

| Track | Status |
|---|---|
| **G-028** (statics in `cpc test`) | fixed; `vendor/log` unblocked, 5 unit tests landed |
| **G-029** (link args in `cpc test`) | fixed; `vendor/metal` unblocked, 8 unit tests landed |
| **G-030** (vendor parent fallback) | fixed; `cpc test` works from within any vendor package without symlinks |
| `vendor/uuid` #8 (`ToString`) | shipped — `to_string()` now infallible via stack buffer + `addr_of` |
| `vendor/uuid` #9 (Linux getrandom) | shipped — replaced `arc4random_buf` with `/dev/urandom` open+read+close |
| `vendor/log` #6 (Linux Tm gating) | shipped — dropped `tm_gmtoff`/`tm_zone`; first 9 i32 fields are POSIX-portable |
| `vendor/metal` #2 (dispatch_data UAF) | investigated — was actually a per-shader leak; fixed via `release(dispatch_data)` after `newLibraryWithData:` |
| `vendor/metal` #5 (Result errors) | shipped — `pub enum MetalError { ... }`; converted `default`/`new_library_with_data`/`new_function`/`ComputePipelineState::new` from Option to Result; proves callers updated |
| `vendor/clap` #2 (Linux argv) | shipped — dropped `_NSGetArgc/Argv` externs and the macOS-only `get_matches()` shortcut |
| `vendor/clap` #3 (`Vec[Arg]`) | shipped — replaced bespoke `ArgVec` (~50 LOC) with `vec::Vec[Arg]` |
| `vendor/json` #2 (short numbers) | shipped — shortest-round-trip via `%.*f` then `%.*g` fallback; `30.0 → "30"`, `0.1 → "0.1"` |
| `vendor/json` #4 (surrogate pairs) | shipped — combine `\uD8XX\uDCXX` pairs into U+10000.. codepoints; `buf_push_utf8` gained 4-byte form |

**Tests after session:** 1035 lib + 399 e2e + 11 lsp = 1445 Rust tests, all green. **66 in-package vendor `#[test]` fns** (arena 11, uuid 10, json 23, clap 9, log 5, metal 8), all green.

---

## Open questions (do not block phase work)

- **Per-field TBAA tree** — v0.0.7 Slice 1.2 punted with "ship when raytracer perf measures the win." The v0.0.8 raytracer is at 0.94 s, ahead of C; not a clear win available here. The work sits unless a workload (gemm / image processing) makes it measurable.
- ~~**Mask types as a distinct `Ty` variant**~~ — **shipped 2026-05-23** as `Ty::Mask` (distinct from `Ty::Simd`; same `<N x iN>` LLVM lowering for ABI compatibility, zero runtime cost). Sema rules: comparison ops on numeric SIMD now return `Ty::Mask`; `select`/`any`/`all` require a mask receiver; arithmetic on masks rejected (E0324); Mask ↔ Simd assignment rejected (E0302); `mask{N}x{M}::splat`/`new`/`from_array` rejected — masks are produced by comparisons or `simd.to_mask()`. Explicit conversions: `mask.to_bits() -> Ty::Simd`, `simd.to_mask() -> Ty::Mask`. 8 new sema tests + 1 e2e roundtrip; total 1028 lib + 397 e2e + 11 lsp = 1436 green.
- **Submodule re-export through `appkit/appkit` facade for functions** — re-litigated three cycles in a row. No second bindings package in v0.0.9 (sqlite was dropped); rubric decision waits for one to land.
- **`#[align(N)]` for struct fields** — v0.0.6 cut, v0.0.7 deferred, v0.0.8 Phase 1B didn't surface a need (the SIMD raytracer worked on `f32x4` directly with no alignment trap). Stays cut until a real consumer hits a misalignment.
- **`option_env!()`** — explicitly rejected in Phase 4. If a workload genuinely needs "optional build-time config" and the empty-string sentinel pattern isn't enough, revisit.
- **`borrow` keyword reservation** — Phase 1 introduces it. Lexer already reserves the token (v0.0.5 borrow regions); v0.0.9 widens the use to value-typed params. No new lexer work.
