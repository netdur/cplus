# v0.0.11 — Vendor bindings cycle

Position locked in v0.0.10 (see [docs/GPU.md](docs/GPU.md)): **C+ is a consumer of GPU backend SDKs**, not a provider of a unified compute abstraction. The compiler's GPU story is done — the v0.0.10 binding-layer wedge (`#selector`, `#msg_send`, `#compile_shader`) is the only GPU-relevant language work that will ever ship. Everything after this is package work.

v0.0.11 is the cycle that puts that position to the test by building real vendor bindings against real SDKs and validating against a real workload.

Anchors:
1. **MPS bindings** for `vendor/metal` — Apple's pre-tuned matmul/conv/FFT/softmax. The fast follow-up that the GPU wedge enables.
2. **`vendor/cuda`** — Driver API + Runtime API + cuBLAS. The NVIDIA story. Plain C FFI, no ObjC complexity.
3. **`vendor/accelerate`** — Apple's CPU-SIMD numerics (BLAS, LAPACK, vDSP, BNNS). The fallback when no GPU exists.
4. **`proves/vector_search_server/`** — a real consumer benchmarked against FAISS. The proof that the position works in practice.

The cycle is **all package work** with one optional compiler diversion (the intrinsic-spelling migration, §Phase 5). No GPU language additions of any kind.

---

## v0.0.10 carryover

| Item | Status | v0.0.11 disposition |
|---|---|---|
| `vendor/static-arena` full implementation | Stub only in v0.0.10 ([vendor/static-arena/src/README.md](vendor/static-arena/src/README.md) explains the gap). | **Phase 4** below. Needs either const-generic struct params or a fixed-size-enum fallback. |
| Intrinsic-spelling migration to `#` | Not done. v0.0.10 added `#selector` / `#msg_send` / `#compile_shader` as new `#`-prefixed intrinsics, but kept `addr_of(x)`, `include_bytes!("path")`, `env!("NAME")`, `size_of::[T]()`, `align_of::[T]()` at their existing spellings. | **Phase 5** below (optional — only if we want consistency). Pre-1.0, no users — can land cleanly any cycle. |
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

## Phase 2 — `vendor/cuda` skeleton · size M

NVIDIA's CUDA stack is plain C — no ObjC, no `objc_msgSend`, no selector machinery. Bindings are straightforward `extern fn` declarations. The challenge is **scope**: CUDA's surface area is huge (cuBLAS alone has hundreds of functions), and shipping bindings for every API at once is impractical.

Scope decision: ship the **minimum viable subset** that lets `proves/vector_search_server/` work on NVIDIA hardware. Add more as workloads demand.

### Minimum viable subset

| Module | Functions to bind | Why |
|---|---|---|
| `cuda::driver` | `cuInit`, `cuDeviceGet`, `cuDeviceGetCount`, `cuDeviceGetName`, `cuCtxCreate_v2`, `cuCtxDestroy_v2`, `cuMemAlloc_v2`, `cuMemFree_v2`, `cuMemcpyHtoD_v2`, `cuMemcpyDtoH_v2`, `cuStreamCreate`, `cuStreamDestroy_v2`, `cuStreamSynchronize` | Device discovery + memory + sync. The bare minimum for any GPU workload. |
| `cuda::cublas` | `cublasCreate_v2`, `cublasDestroy_v2`, `cublasSetStream_v2`, `cublasSgemv_v2`, `cublasSgemm_v2`, `cublasDgemv_v2`, `cublasDgemm_v2` | The matmul primitives. Vector search needs `sgemv` (single-precision matrix×vector); benchmark prep may need `sgemm` (single-precision matrix×matrix). |
| `cuda::error` | `CUresult` → `CudaError` enum mapping; `cublasStatus_t` → `CublasError` | Discriminated errors mirroring `MetalError`'s shape. |

### Wrappers

- `Device` — `Device::default() -> Result[Device, CudaError]`, `Device::count() -> u32`, `Device::name() -> string`.
- `Context` — RAII wrapper around `cuCtxCreate` / `cuCtxDestroy`.
- `Stream` — `Stream::new(device) -> Result[Stream, CudaError]`, `stream.synchronize()`, Drop.
- `DeviceBuffer[T]` — typed RAII wrapper around `cuMemAlloc` / `cuMemFree`. `alloc(n)`, `copy_from_host(slice)`, `copy_to_host(out_slice)`, Drop.
- `cublas::Handle` — `Handle::new(stream)`, `handle.sgemv(...)`, `handle.sgemm(...)`, Drop.

### Out of scope for this cycle

- cuFFT, cuSPARSE, cuDNN, cuRAND, NCCL, NVTX — all separate libraries, each non-trivial. Defer until a workload asks.
- Runtime API (`cudaMalloc` / `cudaMemcpy` — the higher-level wrapper). Driver API is sufficient for our needs and avoids the runtime-vs-driver context-mixing pitfalls.
- Multi-GPU. One device per process for v0.0.11.
- Async memcpy / overlap. Synchronous only — simpler model.

### Size estimate

~400–600 LOC across `vendor/cuda/src/{driver,cublas,error}.cplus`. ~8–12 `#[test]` fns. Two sessions if the host has an NVIDIA GPU for testing; one session if we test against the CUDA runtime stubs.

### Platform note

Most macOS workstations don't have an NVIDIA GPU. The package compiles on macOS (CUDA headers + stubs are installable via the NVIDIA toolchain) but unit tests requiring an actual device skip cleanly. CI / proper testing happens on Linux + NVIDIA hardware. Document the testing story in the package README.

---

## Phase 3 — `vendor/accelerate` · size S

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

## Phase 4 — `vendor/static-arena` full implementation · size S

Carryover from v0.0.10 Phase 2. The package ships as a stub today ([vendor/static-arena/src/README.md](vendor/static-arena/src/README.md)) because the natural API needs const-generic struct parameters that sema doesn't yet support.

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

**Pick A.** Const-generic struct params are likely to be useful beyond this one package (any RAII type with a fixed-size buffer wants it — `RingBuffer[N]`, `FixedString[N]`, `BoundedQueue[T, N]`). Lands once, pays off forever. If A's compiler scope balloons mid-implementation, fall back to B and re-file A as a future open question.

### Deliverables

- Const-generic struct param support in sema + parser + mono.
- `vendor/static-arena/src/static_arena.cplus` — the real implementation, replacing the stub.
- `vendor/static-arena/Cplus.toml` — drop the "stub" disclaimer.
- ≥5 `#[test]` fns: alignment math, overflow detection, OOM behavior, reset, alloc_str.
- Marked `#[no_alloc]` end-to-end so it composes with v0.0.10 Phase 1.

---

## Phase 5 — Intrinsic-spelling migration to `#` · size XS (optional)

v0.0.10 added three `#`-prefixed intrinsics (`#selector`, `#msg_send`, `#compile_shader`) but kept the existing intrinsics at their old spellings (`addr_of(x)`, `include_bytes!("path")`, `env!("NAME")`, `size_of::[T]()`, `align_of::[T]()`). The result is an inconsistent surface: half the intrinsics use `#`, half use `!`, two use bare names.

The plan called for full migration. Skipped in v0.0.10 because the new intrinsics took priority and the cosmetic cleanup wasn't blocking anything.

### Decision point

Land in v0.0.11 if we want the surface consistent before any more intrinsics get added. Skip if the inconsistency doesn't bother anyone in practice.

### If landed

- Lexer: parse `#name(...)` as the canonical intrinsic-call form.
- Sema: rename internal dispatch from `addr_of` / `include_bytes` / `env` / `size_of` / `align_of` to `#addr_of` / `#include_bytes` / `#env` / `#size_of` / `#align_of`. Old spellings removed outright (pre-1.0, no users, no deprecation).
- Migrate every call site across `vendor/` and `docs/examples/`. Probably ~30–50 sites.

### Size estimate

~100 LOC of compiler work + a mass `sed`-style migration of call sites. One session.

---

## Phase 6 — `proves/vector_search_server/` · size M

The canonical "C+ as a consumer of GPU SDKs" demo. A working HTTP server that loads an embedding database, accepts query vectors, and returns top-K matches using MPS (Apple) or cuBLAS (NVIDIA) for the inner loop.

### Shape

```
proves/vector_search_server/
├── Cplus.toml          # depends on metal, cuda (optional), accelerate, stdlib
├── README.md           # how to run, benchmark methodology
├── src/
│   ├── main.cplus      # HTTP server entry point
│   ├── backend.cplus   # backend trait, MPS/CUDA/Accelerate impls behind cfg
│   ├── search.cplus    # core matmul + top-K logic
│   └── server.cplus    # request parsing, response formatting
├── data/
│   └── embeddings_1m_768.bin   # 1M × 768 float32 — pre-generated
└── bench/
    ├── run_faiss.py    # FAISS comparison harness (Python — they don't have to port)
    └── results.md      # numbers
```

### Backend selection

Startup-time, not runtime. The server's main reads `--backend=metal|cuda|accelerate` (defaulted by host platform) and instantiates the corresponding pipeline. No unified abstraction — each backend has its own `BackendMps` / `BackendCuda` / `BackendAccelerate` struct with the same method names, dispatched via match in main.

This is the pattern llama.cpp uses (each `ggml-<backend>.c` is independent; the host selects at startup). Avoids the `vendor/tensor` failure mode discussed in [docs/GPU.md](docs/GPU.md).

### Benchmarks

Baseline: FAISS (Python bindings, both CPU and GPU modes) on the same data. Measure:
- Cold-start latency (load + first query)
- Steady-state query throughput (queries/sec at p50, p99)
- Memory residency

If C+ + MPS is within 20% of FAISS-GPU on Apple Silicon, the position is validated. If C+ + Accelerate beats FAISS-CPU by any margin on small N, that's a real differentiator (no Python overhead, no GC, single binary).

### Size estimate

~800–1200 LOC across the project. The HTTP server is the wildcard — either a thin libuv binding or hand-rolled epoll/kqueue. Probably 2–3 sessions. Worth doing — this is the artifact that explains the language to a new user.

---

## Suggested ordering

The dependencies:
- Phase 1 (MPS) and Phase 3 (Accelerate) are independent of everything else. Land first.
- Phase 2 (CUDA) is independent of MPS / Accelerate but requires NVIDIA hardware to validate. Land in parallel with Phase 1 if testing access exists.
- Phase 4 (static-arena full) is independent. Land any time.
- Phase 5 (intrinsic migration) is independent. Land any time.
- Phase 6 (vector_search_server) depends on at least one backend being ready. Don't start until Phase 1 ships.

Recommended order:

1. **Phase 1 (MPS)** — fastest path to validating the v0.0.10 wedge in a real binding. Land first.
2. **Phase 3 (Accelerate)** — easy, useful, low risk. Pair with Phase 1.
3. **Phase 6 (vector_search_server)** — start as soon as Phase 1 lands. Even with only MPS backend initially, having the harness exists drives the rest.
4. **Phase 2 (CUDA)** — start once you have NVIDIA testing access. Slot it into vector_search_server's backend matrix once the wrappers are ready.
5. **Phase 4 (static-arena full)** — back-half of the cycle. The const-generic compiler work is worth doing carefully.
6. **Phase 5 (intrinsic migration)** — last, only if you want it. Skipping is fine.

---

## Out-of-scope for v0.0.11

- **`vendor/vulkan`** — slips to v0.0.12+. Lower priority than CUDA + Metal for any plausible v0.0.11 user.
- **`vendor/opencl`** — only if asked. Mostly legacy.
- **`vendor/webgpu`** — browser/wasm compute. Niche, defer.
- **cuFFT, cuSPARSE, cuDNN, NCCL** — separate CUDA libraries. Defer until a workload asks.
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
- **HTTP server library choice for Phase 6.** Hand-rolled epoll/kqueue, or thin libuv binding, or some other vendor package? Decide when Phase 6 starts.
- **Per-field TBAA** — stays open from v0.0.9. Tensor / GEMM workloads under Phase 6 may surface the measurement that justifies it.
- **`#compile_shader` incremental builds.** ~100ms per shader compile is fine for small projects, adds up for large ones. Revisit if Phase 6 measures it.
