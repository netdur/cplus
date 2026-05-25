# v0.0.12 — open

## Bugs

> **Numbering note.** llama.cplus is the canonical gap log; cpc-internal IDs occasionally diverge when we picked the next-free slot before seeing their entry. Current alignment:
>
> | cpc-side ID | llama.cplus ID | What |
> |---|---|---|
> | G-022..G-025 | G-022..G-025 | aligned |
> | G-026 | (no equivalent) | unit-type-in-turbofish — cpc-internal |
> | G-027 | G-027 | aligned (sret) |
> | G-028 | G-026 | zero-fill — collided on slot |
> | G-029 | G-028 | `--emit-obj` reads `Cplus.toml` |
> | G-030 | G-029 | `atomic_thread_fence` + E0405 diagnostic |
> | G-031 | G-030 | `#cpu_relax()` spin-loop hint |
> | (none) | G-031 | declare-only `pub static` — open, downgraded to ergonomics by llama.cplus after ownership-flip survey |
> | G-033 | G-032 | `#zero::[T]()` accepted in static/const init |
> | G-034 | G-033 | extern-call struct-by-value param ABI coercion |

- **G-022** — ✅ `4067546` — E0333 diagnostic suggests `};` when the function returns `()` and the tail is unit-typed; `return ...;` only when an actual value is being abandoned.
- **G-023** — ✅ `4067546` — `let x: i64 = -100;` works the same as `let x: i64 = 100;`. Expected type propagates through unary-minus; codegen const-folds `-LIT` so it flows as a textual constant at any width.
- **G-024** — ✅ `621633a` — `*T.is_null()` / `*T.is_not_null()` builtin methods on raw pointers. Single `icmp eq/ne ptr p, null` lowering; safe (no memory access).
- **G-025** — ✅ `621633a` — `#addr_of` accepts any place expression — `Ident`, `Field`, `Index`, `Deref`, and chains. Unblocks the llama.cplus gallocr port. Codegen rides existing `gen_place`.
- **G-026** (cpc-internal — collides with llama.cplus G-026 but unrelated) — ✅ `1745cb2` — `()` parses as the unit type wherever `parse_type` runs (turbofish, fn-pointer return, fn return), resolves to `Ty::Unit`, mangles to `unit`, and round-trips through monomorphization. Parse errors on the entry file now render with a real span instead of `1:1`. Unblocks `spawn_with::[I, ()]` for unit-returning workers.
- **G-027** — ✅ `extern fn` returning a >16-byte aggregate now emits sret on both the import declaration AND every call site, matching the AArch64-Darwin (and x86_64-sysv) C ABI. Pre-fix: cpc declared `declare %T @f(...)` and called direct, while clang on the C side emitted `void @f(ptr sret(%T), ...)` → silent ABI mismatch → SIGSEGV on first call. Cross-language e2e test exercises the case end-to-end.
- **G-028** (= llama.cplus G-026) — ✅ `5d23203` — Two complementary explicit-zero primitives, closing the C99 partial-init silent-garbage gap that caught a real bug in `ggml_dyn_tallocr_new`:
  - `#zero::[T]() -> T` — a value of type `T` with every byte zeroed. Safe; alloca + memset + load.
  - `*T.write_zeroed()` — zero the T-many bytes a raw pointer refers to. Unsafe (raw-pointer write), gated by E0801.
- **G-029** (= llama.cplus G-028) — ✅ `6ef23a8` — `cpc --emit-obj FILE` (and the rest of the single-file driver paths) now walks up from `FILE`'s directory looking for `Cplus.toml`. If found, the project's `[dependencies]` flow to the resolver so `import "stdlib/atomic"` resolves correctly under the CMake `add_custom_command` shape. Unblocks per-file invocations from external build systems. No new flag; existing `cpc --emit-obj` invocations without a reachable manifest still behave the same.
- **G-030** (= llama.cplus G-029) — ✅ Two-part:
  - `__cplus_atomic_fence_<ord>()` intrinsic + `pub fn atomic_thread_fence(ord: Ordering)` in `stdlib/atomic`. Lowers to LLVM `fence <ord>`; `Relaxed` is a no-op (LLVM rejects `fence monotonic`). Unblocks `ggml_barrier`-style publish-without-load patterns.
  - Bonus diagnostic: new **E0405** "no item named X in module Y" for cross-file references to genuinely-missing names. Pre-fix the resolver lumped these into E0403 ("function X is private — mark it pub"), which was actively misleading.
- **G-031** (= llama.cplus G-030) — ✅ `#cpu_relax()` spin-loop hint. Per-arch lowering: aarch64 → `llvm.aarch64.hint(i32 1)` (YIELD); x86_64 → `llvm.x86.sse2.pause()`; other → no instruction emitted. Safe; correctness-irrelevant (the C convention treats unknown targets as a no-op). Picked option (a) from the gap report — smallest surface, defers inline-asm to a future flagship cycle.
- **llama.cplus G-031** (no cpc-side ID) — declare-only `pub static NAME: T;` for aliasing C-defined globals. Open, but **downgraded to ergonomics** by llama.cplus after their port survey: of 17 ggml globals, 0 require staying in C, so ownership-flip (cpc defines, C consumes via plain `extern`) closes the case without language work. Two fix options stay open for the eventual third-party-library case: (a) `pub static NAME: T;` no-init, implicit extern; (b) `#[link_name = "..."]` on statics.
- **G-034** (= llama.cplus G-033) — ✅ Call-site mirror of G-027 on the param side. `extern fn` taking a struct-by-value param now applies AArch64-Darwin / x86_64-sysv coercion at the call site to match the import-declaration shape: ≤8B → coerce to `i64`, ≤16B → coerce to `[2 x i64]` (aarch64) / `{ i64, i64 }` (x86_64), >16B → indirect (alloca on caller frame, pass `ptr`). Pre-fix the declaration was correct but the call site emitted the raw `%T` aggregate → silent ABI mismatch → SIGSEGV on the first call (caught porting `ggml_init(struct ggml_init_params)`). Cross-language e2e test exercises all three size buckets end-to-end.
- **G-033** (= llama.cplus G-032) — ✅ `#zero::[T]()` accepted in `const` / `static` / `static mut` initializer position. Lowers to LLVM `zeroinitializer`, lands in `.rodata` / `.data` / BSS depending on mutability. Closes the BSS-zero global case (lookup tables + zero-init structs) the llama port hit when porting `ggml_cpu_init`-owned globals into cpc. Also fixed a latent codegen ordering bug: struct type declarations now precede static emission so `@T = global %S zeroinitializer` lands in a context where `%S` is known (pre-fix clang rejected with "invalid type for null constant"). Other non-literal initializer shapes (array literals, fill-arrays) still rejected — minimum-surface fix per llama.cplus's option (a).
