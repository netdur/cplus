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

## Ownership-soundness audit (see [tutorial-gaps.md](tutorial-gaps.md))

An external read of the tutorial predicted six ownership "sharp edges" from places where the docs implied hidden machinery. Audited all six against the compiler; two were live double-free unsoundness, two were unenforced borrow semantics, two were by-design (now documented). New error codes: **E0509** (move out of Drop type), **E0511** (undeclared return region), **E0512** (return-region mismatch), **E0513** (return borrow of a local).

- **GAP-1 / forward-move double-free** — ✅ The v0.0.10 "non-Copy moves by default" rule was enforced by the borrow checker (E0335) but never wired into codegen: a bare `x: T` non-Copy struct param was lowered as a shared borrow (`ptr readonly`, caller keeps the drop). Forwarding the value back out — `fn f(x: T) -> T { return x; }` then `let c = f(b);` — dropped the one heap allocation twice. Fix: `effective_move` in `codegen.rs` collapses the bare case onto the existing, sound `move x: T` lowering (value-pass + caller drop-flag flip + callee `register_drop`). `borrow x` / `mut x` keep their by-pointer borrow ABI. Confirmed under ASan; the tutorial's "`move x` is redundant on non-Copy params" claim is now actually true at the codegen level.
- **GAP-5 / partial move from a Drop type** — ✅ Moving a non-Copy field out of a value whose type implements `drop` (`let q = pair.a;` / `return pair.a;`) double-freed: the moved-to binding drops the field and the owner's hand-written destructor frees it again (C+ does not synthesize per-field drops — `phase3-drop.md §5`). Now **E0509** rejects it, in both `let`-init and `return` positions (mirrors Rust E0509). Partial moves out of non-Drop aggregates still compile (no destructor → no double-free).
- **GAP-2 / `borrow REGION` returns unenforced** — ✅ Region annotations were completely inert (even an undeclared region compiled). Now enforced callee-side: **E0511** when a return type names a region no parameter declares; **E0512** when a `return` borrows a different region than the signature declares. Same-region returns still compile. (Regions are unused in real code → zero risk.)
- **GAP-3 / `str` & `T[]` views escaping** — ✅ (partial) A `str`/`T[]` view rooted at a function-**local non-Copy owned value** (a `string`/`Vec[T]`/Drop type — directly or via `as_str`/`as_slice`) dangles once the local drops. Now **E0513**. Borrows of params/`self` (caller-tied, elision-governed) and `'static` literal-backed `str` are left alone — verified no false positives across stdlib + clap/json/vec. *Conservative misses (documented):* views into a moved-in owned param or `move self`, and indirect reborrows through a Copy `str` local — closing these needs full lifetime inference.
- **GAP-1 (self vs param) / GAP-4 (`mut` Copy vs non-Copy) / GAP-6 (raw-pointer escape)** — documented, not code bugs. The receiver/param ownership matrix and the `mut`-propagation rule are now spelled out in the tutorial; raw-pointer escape is an intentional `unsafe` escape hatch (caller's validity obligation).

## Real-time roadmap (see [realtime.md](realtime.md))

Turned C+'s systems-language strengths into compiler-enforced soft-real-time contracts. Eight phases; all now have at least their core shipped. New error codes: **E0901** (no_alloc), **E0906** (bounded_recursion), **E0907** (no_block), **E0908** (max_stack), plus **E0502** reuse for `Send`/`Sync`.

- **Phase 1 — `#[no_alloc]`** ✅ (hardened) — call-graph walk rejects libc allocators + unmarked user callees + unknown externs (E0901). Now also rejects **string interpolation** (lowers to a `__string_concat` malloc). The shared body walker became an extensible `BodyEffects { calls, interps, let_tys }`. *Open:* instance method-dispatch (`recv.method()`) and drop-glue are still skipped.
- **Phase 2 — stdlib annotation** 🟡 — `u64` atomics + `atomic_thread_fence` marked `#[no_alloc]`/`#[no_block]`. Rest of the no-alloc surface (option/result/io) is follow-up.
- **Phase 3 — `#[no_block]`** ✅ — parallel pass; blocklist of blocking primitives (mutex/cond/barrier/join, sleep, blocking file+socket I/O, poll/select) → E0907; non-blocking leaf whitelist (math/mem/try-locks).
- **Phase 4 — bounded stack `#[max_stack(N)]`** ✅ — E0908 when the estimated frame (params + typed locals across all nested blocks, conservative all-live sum) exceeds N. Sema-native `stack_size_of`/`layout_of` mirrors codegen's `static_layout` ABI.
- **`#[realtime]`** ✅ — bundle of `#[no_alloc]` + `#[no_block]` + `#[bounded_recursion]`.
- **Phase 5 — `vendor/rt`** ✅ — lock-free `SpscRingU64` (release/acquire atomics) + `FixedPoolU64` (intrusive free-list); hot methods `#[no_alloc]`/`#[no_block]`. 13 tests.
- **Phase 6 — `Send`/`Sync` (core)** 🟡 — `is_send`/`is_sync` recognize stdlib markers structurally: `Rc[T]` is `!Send`+`!Sync`, `MutexGuard[T]` is `!Send`; `Arc[T]` stays `Send`+`Sync`. E0502 at `Send`/`Sync`-bounded sites (esp. `thread::spawn`). *Deferred:* broad "raw-ptr structs `!Send`" rule — blocked on a missing `unsafe impl Send` opt-in (would otherwise reject ObjC/channel/mutex FFI).
- **Phase 7 — platform packages** ✅ (Darwin) — `vendor/rt_darwin`: `clock` (monotonic ns via `clock_gettime`), `thread` (QoS priority → `Result`), `mem` (`mlock`/`munlock` → `Result`). 8 tests. `rt_linux`/`rt_posix` mirror it later.
- **Phase 8 — profiles + tooling** ✅ — `[profile.realtime]` (`deny_alloc`/`deny_block`/`deny_unknown_extern`/`stack_limit`) synthesizes the contract attributes onto *local* functions (deps exempt). `cpc check` (no FILE) = whole-project front-end gate, no codegen; JSON via `--diagnostics=json`.
- **Demo** ✅ — `proves/realtime_audio`: a `#[realtime]`+`#[max_stack]` audio callback over an SPSC control channel, raising thread QoS and recording per-frame latency; E0901/E0907/E0908 verified to fire in-context.

## Benchmark gaps (bench-cplus handoff, triaged 2026-05-30)

From `/Users/adel/Workspace/bench-cplus/handoff.md` (C+ vs C / Rust / Swift / Node / Bun). Each item was re-verified against the current build before recording; the handoff was written against a slightly older cpc, so several items no longer reproduce.

**Confirmed open:**

- **B-1 / no `<2 x float>` struct-field loads** (P0, medium): struct loads emit a scalar GEP + scalar `load` per field; LLVM's SLP-vectorizer does not re-fuse them on the raytracer hot path, so a `V3` dot stays scalar (0 vector ops in `--emit-ll-opt`). clang emits `load <2 x float>` for the leading pair. Direction: at struct-load-by-value emission, when a struct has ≥2 consecutive same-width fields at a naturally-aligned offset, emit `load <2 x T>` for the run plus a scalar tail load. This is the real raytracer perf gap; the FMA path already matches C, vectorized struct loads would push below it.
- **B-5 / by-value `self` (and value params) still alloca+store at entry** (P1, low-med): `define %V3 @V3.add(%V3 %0, ...)` is correct, but the body still does `%self.addr = alloca %V3; store %V3 %0, ptr %self.addr` and reads through the pointer. mem2reg/SROA recovers it, so the runtime cost is marginal; the win is smaller pre-opt IR (cpc emits 2-3× clang's line count) and easier inliner heuristics. Direction: when a by-value param's address is never taken (note: `#addr_of(param)` *can* take it, so this must be checked, not assumed) and it is not `mut`, bind the SSA value directly and use `extractvalue` for field reads instead of alloca+GEP+load.

**Needs design (not a quick fix):**

- **B-2 / auto-`noalias` from borrow-checker proofs — AUDITED, no safe win.** Re-checked `param_attrs`: `mut`/`move` pointer-passed struct params *already* emit `noalias` (exactly the disjointness the BC proves); shared borrows get `readonly` and *cannot* be `noalias` (two shared borrows may alias the same object); raw `*T` is the only remaining shape, and the BC deliberately doesn't track it — that's what `restrict` is for. So the handoff's premise ("the BC proves disjointness but it doesn't reach the IR") is already false; auto-`noalias`-ing anything beyond what's emitted today would be a **soundness bug**. A real improvement needs a BC extension to track `*T` disjointness — a research project, not an annotation tweak.

**Not reproducing on current build (verified fixed, candidates to close):**

- **B-3 / `musttail` over-marking — CLOSED.** The handoff's repros (`return dot(..) > 0.0f32;`, `return sub(v, scale(..))`) emit a plain `call fastcc`, not `musttail`; the detector already only marks literal `return CALL(args);` with matching return type. Regression test added (`musttail_wrong_return_type_and_nested_expr_compile`).
- **B-4 / `let X: STRUCT = if { call } else { block-with-lets }` codegen panic — CLOSED.** Fixed (`3230bbb`); the handoff's exact struct-`V` repro is now pinned by `let_struct_eq_if_else_with_block_arm_compiles` (alongside the `str`-arm test from the original fix).

**Not a bug (do not "fix"):**

- **B-6 / f32 literal "double-rounding"**: NOT a defect. The lexer already parses f32-suffixed literals directly to f32 (single rounding) and widens losslessly to f64 for AST storage. `0.4f32` emits `store float 0x3FD99999A0000000` = f32 `0x3ECCCCCD`, the correctly-rounded value, and **clang emits byte-identical IR for `0.4f`** (verified). The handoff's "C produces `0x3ECCCCCC`" is incorrect. Any MD5 divergence has another source; changing this path would make cpc *wrong*.

**Deferred (high effort or library, not language codegen):**

- **B-7 / no SROA on `malloc`/arena `*T`** (P3, high): tree/graph workloads (binary_trees 8-32× vs Perry). Needs cross-function escape analysis; the pointer escapes opaquely from the allocator. Incremental angles: an `#[inline(always)]`-equivalent so `arena.alloc[T]` inlines and the bump math becomes visible; a compiler-recognized `Box[T]` form; or document "use arrays + indices for pointer-tree-shaped hot code".
- **B-8 / `vendor/json` slower than JS parsers — 2.24× FIXED** (library, no API change). The number path was the cost: `parse_number` did 2 malloc/free + memcpy + `strtod` per number, and `encode_number` did 3 malloc/free + an snprintf-format-build + snprintf + a `strtod` round-trip-probe per number. Added an **integer fast-path on both sides**: a canonical `-?[0-9]+` of ≤15 digits is exact in f64 (|v| < 2^53), so it parses by i64 accumulation and emits by manual base-10 — no malloc, no strtod, no snprintf. Non-canonical / float / big-int forms fall back to the prior strtod/snprintf path (values bit-identical; the fast-path only fires where i64↔f64 is provably exact, and `-0`/signed-zero is deliberately routed to the slow path to preserve output). The fallback paths also moved their scratch buffers + strtod end-pointer to the stack via `#addr_of`, so only pathologically long literals touch the heap. Measured on a 140 KB number-heavy payload (200× parse+stringify): **1950ms → 871ms**, identical checksum; all 23 in-package tests green. Remaining headroom (not done): 8-byte-chunk whitespace skip, per-string buffer pre-sizing — smaller, string-shaped wins.

- **B-9 / `size_of` → `#size_of`**: already shipped (v0.0.11 intrinsic-sigil cutover). Changelog/migration note is the only follow-up.

### Re-validated at -O2 (bench agent, 2026-05-30)

The handoff was re-run with `--release` / `cpc --release --emit-ll-opt`, confirming the `-O0` contamination. Final state:

- **B-1 / scalar Vec3 arithmetic — CONFIRMED real at -O2** (the lone surviving codegen item). In `ray_color`, post-opt: C emits 52 `<2 x float>` fmul/fadd/fsub (asm: 27 `fmul.2s` + 13 `fadd.2s` + 6 `fsub.2s`); C+ emits 0 of any `.2s` op. So the vectorization gap is real at the actual codegen level, not an `-O0` artifact. *But* C+ already beats C on wall-time via its FMA path, so the perf urgency is low; this would widen the lead, not erase a loss. Direction unchanged: emit `<2 x T>` loads/arith for runs of consecutive same-width struct fields.
- **B-5, B-6, B-7 — CLOSED, not bugs.** B-5: raytracer has 182 allocas pre-opt, **0 post-opt** (mem2reg/SROA). B-6: confirmed cpc == clang (`store float 0x3FD99999A0000000` for `0.4`); the original "C produces `0x3ECCCCCC`" was a misread. B-7: LLVM eliminates non-escaping mallocs at -O2; the `binary_trees` gap is escaping tree nodes (fundamental AOT-no-GC vs JIT-with-GC tradeoff), not a cpc gap.
- **B-3 — CONFIRMED fixed** (no residual found; emits a plain `call`, not `musttail`).
- **B-4 — had a residual, now fixed** (`3230bbb`). The struct shape was already covered, but `let v: str = if {…} else {…}` (and `string` interpolation arms) still panicked codegen — `expr_value_ty` didn't infer string-literal types, so `gen_if` allocated no result slot. Fixed by adding `StrLit`/`InterpStr` to `expr_value_ty`; codegen + e2e regression tests added. My earlier "B-4 fixed" triage was wrong (I tested only a covered struct shape).
- **E0513 indirect reborrow — closed** (`56b02b7`): `let v = s.as_str(); return v;` now fires E0513 (was a documented conservative miss). Verified sound, no false positives.
- **IR line count — not meaningful.** Post-opt cpc 871 vs clang 769 (13%), not the "2280 vs 769" the pre-opt comparison suggested.

**B-10 / fp-contract default differs from C — FIXED** (`--fp-contract=off` flag). cpc allowed fp-contraction by default (raytracer asm: 60 `fmadd`/`fmsub`); the reference C build used `-fno-fast-math -ffp-contract=off` (0 fused ops). Output bits diverged from FMA fusion, not literal precision (not a correctness bug — FMA is more accurate — but it blocked bit-identical-to-C output). Shipped as a build-level CLI knob, not language surface: `--fp-contract=off|on|fast` (default `on`). `off` suppresses the `a*b+c → llvm.fmuladd` peephole and drops the `contract` fast-math flag from scalar/SIMD float `fadd`/`fsub`/`fmul`/`fdiv`, so float output is bit-identical to a C build compiled with `-ffp-contract=off`. The flag rides alongside `--release` and must precede `--emit-ll`/`--emit-asm`/`--emit-obj FILE` (same arg-order rule as `--release`). Chose the CLI knob over a `#[fp_contract(off)]` attribute: the consumer (bit-reproducible-vs-C) is whole-program, and a build flag adds no language surface (vs. the "no several ways to do the same thing" principle). Internals: `ModuleMetadata.fp_contract` (set once in `generate_inner`), read via `FnState::fmf()`. Codegen unit test + 2 e2e tests (on/off IR diff, build-and-run parity, invalid-value rejection).

**Lesson recorded in the handoff:** `cpc --emit-ll` is pre-opt and not comparable to clang's optimized IR; `cpc --release --emit-ll-opt` is the apples-to-apples surface. Most of the original P0/P1 deflated once measured there.

Net open: **B-1** (real codegen — needs a mini-SLP pass to emit `<2 x T>` loads for runs of consecutive same-width struct fields; **low urgency**, C+ already wins wall-time so this widens the lead on non-Apple hardware rather than fixing a loss; high regression risk for a speculative win → not shipped speculatively). **B-2** audited: already covered for the shapes the BC proves; the rest needs a `*T`-disjointness BC project (unsound otherwise). Shipped this round: **B-3**/**B-4** regression tests, **B-8** number fast-paths (2.24×), **B-10** (fp-contract knob).
