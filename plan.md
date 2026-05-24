# v0.0.11 — Vendor bindings cycle

Position locked in v0.0.10 (see [docs/GPU.md](docs/GPU.md)): **C+ is a consumer of GPU backend SDKs**, not a provider of a unified compute abstraction. The compiler's GPU story is done — the v0.0.10 binding-layer wedge (`#selector`, `#msg_send`, `#compile_shader`) is the only GPU-relevant language work that will ever ship. Everything after this is package work.

v0.0.11 is the cycle that puts that position to the test by building real vendor bindings against real SDKs and validating against a real workload.

Anchors:
1. **MPS bindings** for `vendor/metal` — Apple's pre-tuned matmul/conv/FFT/softmax. The fast follow-up that the GPU wedge enables.
2. **`vendor/accelerate`** — Apple's CPU-SIMD numerics (BLAS, LAPACK, vDSP, BNNS). The fallback when no GPU exists.

Plus two compiler-side carryovers from v0.0.10: `vendor/static-arena` full implementation (needs const-generic struct params) and the optional `#`-spelling migration for existing intrinsics.

The cycle is **all package work** with one optional compiler diversion (the intrinsic-spelling migration, §Phase 4). No GPU language additions of any kind. Validation is left to consumers — vendor packages ship with in-package `#[test]` fns (the MPS `test_2x2_matmul_identity_correctness` is the proof shape) and that's enough; no benchmark harness in-tree.

**Note**: `vendor/cuda` was originally planned here but dropped — the host environment is Apple Silicon (no NVIDIA hardware to validate against) and the consumer-of-SDKs position already proves itself via MPS. CUDA bindings can ship as a separate package later when someone has the hardware to test them.

---

## v0.0.10 carryover

| Item | Status | v0.0.11 disposition |
|---|---|---|
| `vendor/static-arena` full implementation | Stub only in v0.0.10 ([vendor/static-arena/src/README.md](vendor/static-arena/src/README.md) explains the gap). | **Phase 3** below. Needs either const-generic struct params or a fixed-size-enum fallback. |
| Intrinsic-spelling migration to `#` | Not done. v0.0.10 added `#selector` / `#msg_send` / `#compile_shader` as new `#`-prefixed intrinsics, but kept `addr_of(x)`, `include_bytes!("path")`, `env!("NAME")`, `size_of::[T]()`, `align_of::[T]()` at their existing spellings. | **Phase 4** below — ✅ shipped. Hard cutover; all six legacy spellings now produce parse/sema errors. |
| Per-field TBAA | Stays open since v0.0.9. Raytracer didn't show a measurable win in v0.0.8. | Stays deferred. Revisit if a tensor / GEMM workload measures it. |
| `vendor/sqlite` (dropped in v0.0.8) | "Wait for a real consumer." | Still deferred. |

---

## Phase 1 — MPS bindings for `vendor/metal` · size S

Apple's Metal Performance Shaders (MPS) ship pre-tuned matmul, convolution, FFT, softmax, reductions, and other primitives for every Apple Silicon generation. Today `vendor/metal` binds the low-level compute infrastructure (Device, Buffer, Library, Pipeline, CommandQueue, etc.) but NOT MPS. Users wanting matmul-on-GPU must write the kernel themselves in MSL — possible but unergonomic, and you're competing with Apple's own optimizations.

This is the fast follow-up the v0.0.10 wedge unlocks. With `#selector` and `#msg_send` now available, the binding code is roughly half the LOC it would have been against the v0.0.9 runtime helpers.

### Deliverables

- `vendor/metal/Cplus.toml` — add `MetalPerformanceShaders` to `[link].frameworks`.
- `vendor/metal/src/mps.cplus` — new file binding:
  - `MPSDataType` enum (`F16`, `F32`, `F64`, `I8`, `U8`, `I32`).
  - `MPSMatrixDescriptor` wrapper — describes shape, stride, dtype of a matrix view into a buffer.
  - `MPSMatrix` wrapper — couples an `MTLBuffer` with an `MPSMatrixDescriptor`.
  - `MPSMatrixMultiplication` wrapper — `init(device, transposeLeft, transposeRight, resultRows, resultColumns, interiorColumns, alpha, beta)`, `encode_to(cmd_buf, lhs, rhs, out)`.
  - `MPSMatrixVectorMultiplication` wrapper — same shape, optimized for matrix×vector.
  - Drop impls on every wrapper (`objc_release`).
- ≥4 `#[test]` fns: 2×2 matmul correctness, 3×4 × 4×2 → 3×2 matmul correctness, matvec round-trip, dtype-mismatch error path.

### Optional follow-ups (same phase if cheap, defer if not)

- **MPSGraph bindings** — Apple's newer high-level API (autograd-style graph construction). Bigger surface, more useful for ML. Land if we have appetite; defer if the simpler `MPSMatrixMultiplication` is enough for the vector-search consumer.
- **MPSImage / MPSCNN bindings** — image processing + CNN primitives. Useful for image / video pipeline workloads. Definitely defer unless a workload asks.

### Size estimate

~150–200 LOC of `mps.cplus` (mostly `#msg_send` calls + Drop boilerplate) + manifest + tests. One session.

---

## Phase 2 — `vendor/accelerate` · size S

Apple's Accelerate framework is the CPU-side numerics library — BLAS, LAPACK, vDSP (signal processing), BNNS (neural network primitives). It's already linked into every Apple binary; we just need C+ bindings.

This serves two purposes:
1. **The "no GPU available" fallback path.** On Apple Silicon machines without a discrete GPU or when MPS is overkill (small batch sizes where the host↔device transfer dominates), Accelerate's CPU-SIMD implementation is the right choice.
2. **A reference implementation for correctness testing.** When MPS gives surprising numerical results, `cblas_sgemm` is the source of truth.

### Deliverables

- `vendor/accelerate/Cplus.toml` — `frameworks = ["Accelerate"]`.
- `vendor/accelerate/src/cblas.cplus` — BLAS Level 1/2/3 primitives. Same surface naming as `cblas.h`: `cblas_sgemv`, `cblas_sgemm`, `cblas_sdot`, `cblas_saxpy`, `cblas_sscal`, etc. F32 and F64 variants.
- `vendor/accelerate/src/vdsp.cplus` — vDSP signal processing primitives. Smaller scope: `vDSP_vadd`, `vDSP_vmul`, `vDSP_vsmul`, `vDSP_dotpr`, `vDSP_meanv`, `vDSP_maxv`. Useful for audio / DSP workloads too — composes with the v0.0.10 real-time story.
- Wrappers: typed enums for `CBLAS_ORDER` (RowMajor / ColumnMajor) and `CBLAS_TRANSPOSE` (NoTrans / Trans / ConjTrans).
- ≥6 `#[test]` fns: known sgemm result, sgemv round-trip, dot product, vDSP vector add.

### Size estimate

~200–300 LOC. Pure `extern fn` + thin typed wrappers. One session.

---

## Phase 3 — `vendor/static-arena` full implementation · size S

**Status: ✅ shipped as Option B + bonus compiler feature.**

Carryover from v0.0.10's static-arena stub. The package ships as a stub today ([vendor/static-arena/src/README.md](vendor/static-arena/src/README.md)) because the natural API needs const-generic struct parameters that sema doesn't yet support.

### Two options

**Option A — Add const-generic struct params (compiler work).** Sema accepts `[T; N]` array types with literal `N`; extending struct generic param lists to accept `const N: usize` is the missing piece. Lets us ship the canonical API:

```cplus
pub struct StaticArena[const N: usize] {
    buf: [u8; N],
    used: usize,
}
```

Effort: ~300 LOC of sema + parser, plus mono mangling support. One session of compiler work, then the package is trivial (~80 LOC).

**Option B — Fixed-size-enum API (no compiler work).** Ship `StaticArena16K`, `StaticArena64K`, `StaticArena256K`, `StaticArena1M` as distinct types. Less elegant but works today.

```cplus
pub struct StaticArena16K { buf: [u8; 16384], used: usize }
pub struct StaticArena64K { buf: [u8; 65536], used: usize }
// ...
```

Effort: ~150 LOC of `.cplus` (mostly repeated by macro-like pattern) + tests.

### Decision

**Picked A; fell back to B** mid-implementation — const-generic struct params turned out to need invasive surgery across `Ty::Array` (48 sites) plus parser/sema/mono machinery for parsing const expressions as bracket arguments at use sites. The plan's documented fallback rule (~"if scope balloons, fall back to B") kicked in.

What actually shipped instead:

- **`[EXPR; N]` fill-array literal** (small compiler feature ~120 LOC): a previously-missing prereq for both options. Enumerating 16384 zeros is impractical, so before this landed neither A nor B could initialize a `[u8; 16384]` field. Lowered to `llvm.memset.p0.i64` for byte-zero fills (the common case), or a per-element store loop otherwise. AST: `ExprKind::ArrayFill { fill, count }`. Disambiguated from `ArrayLit` by `;` after the first element.

- **`vendor/static-arena`** as Option B with two sizes (16K + 64K). Each is a hand-rolled fixed-size shape with `new`, `capacity`, `used_bytes`, `remaining`, `reset`, `alloc_bytes`, `alloc_bytes_aligned`, `alloc_zeroed_bytes`, `alloc_str`. 7 in-package tests covering the common code paths. Marked-up-able with `#[no_alloc]` from v0.0.10 Phase 1 — composing the contract is now possible.

- **256K + larger sizes dropped** from this cycle. By-value returns of large structs from `new()` trigger 3-4× stack copies (~1MB for a 256K arena) which overflows the default 8MB stack in practice. The proper fix is `sret` for large struct returns — a separate compiler improvement. Documented in the package header.

Option A (full const-generics) remains a future flagship feature. The fill-array literal landed during this work removes one of the blockers; the rest is the per-site mono substitution machinery.

### Deliverables (as actually shipped)

- ✅ `[EXPR; N]` fill-array literal in the compiler (`ast.rs` + `parser.rs` + `sema.rs` + `codegen.rs` + walker arms in `borrowck.rs`, `lower.rs`, `resolver.rs`, `monomorphize.rs`).
- ✅ `vendor/static-arena/src/static-arena.cplus` — `StaticArena16K` + `StaticArena64K`.
- ✅ `vendor/static-arena/Cplus.toml` — stub disclaimer dropped.
- ✅ 7 in-package `#[test]` fns covering construct / alloc / overflow / reset / zeroed / str / 64K shape.

---

## Phase 4 — Intrinsic-spelling migration to `#` · ✅ shipped

v0.0.10 added three `#`-prefixed intrinsics (`#selector`, `#msg_send`, `#compile_shader`) but kept the existing intrinsics at their old spellings. Phase 4 finished the migration. The surface is now uniform: every compiler-known builtin uses `#name(...)`.

### What landed

- Parser: `#name(args)` (with optional turbofish `::[T...]` and `-> RetTy`) routes universally into `ExprKind::Intrinsic`. The legacy `include_bytes!("path")` / `include_str!("path")` / `env!("NAME")` macro-suffix forms removed outright.
- Sema: `check_intrinsic` dispatch gained arms for `addr_of`, `include_bytes`, `include_str`, `env`, `size_of`, `align_of`. The old bare-name `addr_of`/`size_of`/`align_of` special-cases in `check_named_call` deleted — those names now produce E0300 (undefined function) when called without `#`.
- Codegen: `gen_intrinsic` extended with parallel arms; lookup tables (`compile_time_blobs`, `env_var_globals`) are keyed off the Intrinsic node span.
- Monomorphization: added Intrinsic + ArrayFill walker arms in `rewrite_expr` / `rewrite_expr_self` / `visit_ident_calls` / `rewrite_alias_expr`. Critical for `#size_of::[T]()` inside generic bodies (T must get substituted).
- Hard cutover: ~150 .cplus call sites migrated across `vendor/`, `docs/examples/`, `proves/`, and Rust test fixtures. No deprecation path (pre-1.0, no users — per locked principle).

### New surface

| Old spelling | New spelling |
|---|---|
| `addr_of(x)` | `#addr_of(x)` |
| `include_bytes!("path")` | `#include_bytes("path")` |
| `include_str!("path")` | `#include_str("path")` |
| `env!("NAME")` | `#env("NAME")` |
| `size_of::[T]()` | `#size_of::[T]()` |
| `align_of::[T]()` | `#align_of::[T]()` |

---

## Suggested ordering

All four phases are independent.

1. **Phase 1 (MPS)** — ✅ shipped in `361920d`. Fastest path to validating the v0.0.10 wedge in a real binding.
2. **Phase 2 (Accelerate)** — ✅ shipped in `59634e8`. 17 in-package tests (CBLAS Level 1/2/3 + vDSP element-wise + reductions).
3. **Phase 3 (static-arena full)** — ✅ shipped via Option B + `[EXPR; N]` fill-array literal as a bonus compiler feature. Const-generic struct params (Option A) deferred to a future flagship cycle.
4. **Phase 4 (intrinsic migration)** — ✅ shipped. Hard cutover; uniform `#name(...)` surface for every compiler builtin.

---

## Out-of-scope for v0.0.11

- **`vendor/cuda` + cuBLAS** — dropped from v0.0.11 (no NVIDIA hardware on the dev host to validate). Can ship as a separate package later when someone has the testing capability. The CBLAS surface in `vendor/accelerate` is the API shape it should mirror.
- **`vendor/vulkan`** — slips to v0.0.12+. Lower priority than Metal for any plausible v0.0.11 user.
- **`vendor/opencl`** — only if asked. Mostly legacy.
- **`vendor/webgpu`** — browser/wasm compute. Niche, defer.
- **cuFFT, cuSPARSE, cuDNN, NCCL** — separate CUDA libraries. Defer with the rest of the CUDA stack.
- **MPSGraph / MPSCNN / MPSImage** — broader MPS coverage. Defer beyond the minimum-viable matmul/matvec.
- **Tensor-as-builtin-type** — never. A `vendor/tensor` package can ship if needed; the language stays out.
- **Operator overloading on tensors** — explicitly rejected by principle.
- **`#[kernel]` / multi-target codegen / GPU intrinsics in the language** — explicitly dropped. See [docs/GPU.md](docs/GPU.md).
- **`#[interrupt]` / `#[naked]` for embedded** — wait for a real embedded consumer.
- **Lock-free queue primitives in stdlib** — `stdlib/atomic` is enough today.

---

## Open questions (do not block phase work)

- **`vendor/tensor` shape, if/when it lands.** Hide backend choice (`Tensor::new()` picks at runtime) or expose it (`Tensor::metal(...)` / `Tensor::cuda(...)`)? Tilts toward exposing it — more honest, avoids the Mojo-shape failure mode. Decision deferred until a real consumer surfaces.
- **GGUF / SafeTensors readers.** If the inference engine angle gets pursued, model format readers are useful. Probably a separate `vendor/gguf` / `vendor/safetensors` package. Wait for demand.
- **Quantization helpers.** Int8 / FP16 / GPTQ / AWQ — probably belongs in `vendor/quant` on top of the BLAS / MPS / cuBLAS primitives. Open whether C+ has any structural advantage vs llama.cpp's existing impls.
- **Real-time + GPU interaction.** `#[no_alloc]` checks host heap. GPU memory allocation (`cuMemAlloc`, `[device newBufferWithLength:]`) is a different path — does the attribute need a GPU-aware extension (`#[no_gpu_alloc]`?)? Probably wait until both real-time and GPU bindings have real consumers, then design from the workload.
- **Per-field TBAA** — stays open from v0.0.9. Tensor / GEMM workloads may eventually surface the measurement that justifies it.
- **`#compile_shader` incremental builds.** ~100ms per shader compile is fine for small projects, adds up for large ones. Revisit if a real workload measures it.
