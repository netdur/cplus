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
