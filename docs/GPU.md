# C+ and the GPU question

A context document. Captures where the GPU discussion landed in v0.0.10 and why, so a future conversation can pick up the thread without re-litigating from scratch. References to the canonical artifacts: [plan.md](../plan.md), [plan.gpu.md](../plan.gpu.md) (historical / rejected vision), [vendor/metal](../vendor/metal/).

---

## TL;DR

**Position locked**: C+ is a **consumer of GPU backend SDKs** (the llama.cpp / FAISS pattern). It is **NOT** a provider of a unified compute abstraction (the Mojo pattern). The kernel source lives outside C+ — in `.metal` / `.cu` / `.spv` / `.comp` files compiled by Apple / NVIDIA / Khronos toolchains. C+ orchestrates; it does not compile kernels.

What this means concretely:
- The only language-level GPU work we ever ship is the v0.0.10 binding-layer wedge (`#selector`, `#msg_send`, `#compile_shader`) — and those are justified by ObjC FFI broadly, not GPU specifically.
- Everything else is **package work forever**: `vendor/metal` + MPS, `vendor/cuda` + cuBLAS, `vendor/accelerate`.
- The maximalist vision from [plan.gpu.md](../plan.gpu.md) — `#[kernel]` attribute, NVPTX/AMDGPU/SPIR-V codegen, address spaces, GPU intrinsics — is **dropped, not deferred**. Wrong shape for this position.

The "user with large numbers to crush" answer: write the host orchestration in C+, dispatch the heavy work to `cublas::sgemv` / `mps::MatrixMultiplication` / `accelerate::cblas_sgemm`. Same shape FAISS uses.

---

## How we got here

The conversation started after the v0.0.9 close. With vendor/metal landed (typed bindings to MTLDevice / Buffer / Library / Pipeline / CommandQueue etc., all with Drop, MetalError, Result-returning factories), the natural next question was: "what should land in the language to make the GPU story possible?"

Two artifacts were on the table:

### plan.gpu.md (pre-existing, ambitious)

The maximalist vision: extend the compiler with a `#[kernel]` attribute, build a multi-target codegen pipeline that compiles kernel functions to NVPTX (NVIDIA) / AMDGPU / SPIR-V / MSL, add address-space qualifiers (`*global`, `*shared`, `*private`) on raw pointers, ship GPU execution intrinsics (`#thread_idx_x`, `#barrier`, etc.), enforce sema rules that reject host-only code inside kernel bodies. Bundle the resulting GPU binary in the host executable's `.rodata`, ship `vendor/gpu` as the host-side runtime abstraction.

Effort: 3-6 months of compiler work plus the package layer. This is approximately what Mojo did.

### research.md (existing, neutral)

Catalogued the options: SIMD primitives (already feasible — partially shipped in v0.0.8's `vendor/simd`), GPU compute as "feasible but massive" requiring multi-target codegen, tensor types as fine in userland today.

Argued the locked principles (no operator overloading, no comptime) are an *asset* not a liability for AI workloads, under the "AI generates code, human audits" framing.

### Where we diverged

The maximalist plan was the obvious next move on paper. But it had problems:

1. **Solves the wrong friction.** A GPU-curious user trying to ship a Metal compute workload in C+ today hits the **FFI wall** (14 ObjC `objc_msgSend` extern declarations, manual selector registration, no way to embed shader source), not the kernel-language wall (Metal Shading Language is fine — every Apple GPU programmer knows it).

2. **Locks us into a kernel-language design before validating demand.** Mojo went this route because they had Modular's resources and a clear ML-research audience. C+ has no users yet; building a kernel-language ahead of consumers is feature-cheering.

3. **Wrong-shape for the position the language was drifting into.** Earlier in the same session we'd noticed C+ had accidentally become a real-time-capable language (no GC, no exceptions, no closures, no hidden allocator, explicit Drop). A real-time positioning doesn't need an in-language kernel compiler; it needs the binding layer to be solid.

The first proposed redirect was a **smaller GPU wedge**: three `!`-suffix macro-shaped intrinsics that fixed the FFI wall without committing to multi-target codegen. That was an improvement but still framed in terms of "stepping stone toward in-language kernels later."

Then the user pushed back harder:

> *"llama.cpp supports a wide range of backends and it does them correctly — the support is USER of BACKEND SDKs, not implementation. This is the correct way and I want to follow that lead. My thinking, looking at Mojo, was NOT to be some sort of unified driver abstract. All I wanted: I have large numbers to crush, should I use GPU? Would C+ allow that, for example if I have a vector search server?"*

That reframe was decisive. The maximalist vision wasn't deferred — it was **wrong-shape**. We don't want it later either. The whole point is to be llama.cpp, not Mojo.

---

## The llama.cpp model

llama.cpp is the proof point. ~73k GitHub stars, started as one person's hobby project, took over the LLM inference space. It supports CUDA, Metal, Vulkan, OpenCL, SYCL, BLAS, and CPU SIMD. Each backend is its own implementation file (`ggml-cuda.cu`, `ggml-metal.m`, `ggml-vulkan.cpp`, etc.). The host code is a thin orchestration layer.

Crucially: **llama.cpp does not implement its own GPU compute infrastructure**. It uses:
- NVIDIA's cuBLAS / cuBLASLt for matmul on CUDA.
- Apple's Metal Performance Shaders (MPS) for matmul on Metal.
- Hand-written shaders for ops MPS / cuBLAS don't cover, compiled with the vendor toolchains (nvcc / xcrun metallib / glslc).

The C++ host code's job: load the right backend at startup, allocate buffers via the vendor's allocator, copy tensors host↔device, dispatch precompiled kernels, synchronize. The kernels themselves are written by NVIDIA / Apple engineers (in the case of MPS / cuBLAS) or by the llama.cpp team (in the case of custom shaders), but always **outside** the host language.

This is the shape C+ adopts. C+'s role is "the host language for inference / vector search / numerical workloads"; the kernels' role is "what the GPU vendors ship in their SDKs."

---

## The vector search example (the actual user workload)

The concrete question the user asked: "I have a vector search server. Should I use the GPU? Would C+ allow that?"

Vector search workload: N vectors of D dimensions (typical: N=1M+, D=384–1536), incoming query, find top-K most similar by cosine similarity or L2 distance. Inner loop is a matrix-vector multiply (`query · database^T`), which is exactly what `cublas::sgemv` and `MPS::MatrixVectorMultiplication` are tuned for.

After the v0.0.10 wedge + the MPS fast-follow-up, the C+ code looks roughly like:

```cplus
import "metal/metal" as metal;
import "metal/mps"   as mps;
import "stdlib/io"   as io;

fn search_top_k(
    query:    f32[],
    database: f32[],
    n:        usize,
    d:        usize,
    k:        usize,
) -> Vec[usize] {
    // 1. Device setup (one-time per server, cached)
    let device = metal::default_device()?;
    let queue  = device.new_command_queue();

    // 2. Upload to device
    let buf_query    = device.new_buffer_from_slice(as_bytes(query));
    let buf_database = device.new_buffer_from_slice(as_bytes(database));
    let buf_scores   = device.new_buffer(n *% 4);

    let desc_q = mps::MatrixDescriptor::new(rows: 1, columns: d,    row_bytes: d *% 4, dtype: mps::DataType::F32);
    let desc_d = mps::MatrixDescriptor::new(rows: n, columns: d,    row_bytes: d *% 4, dtype: mps::DataType::F32);
    let desc_s = mps::MatrixDescriptor::new(rows: n, columns: 1,    row_bytes: 1 *% 4, dtype: mps::DataType::F32);

    let m_query    = mps::Matrix::new(buf_query,    desc_q);
    let m_database = mps::Matrix::new(buf_database, desc_d);
    let m_scores   = mps::Matrix::new(buf_scores,   desc_s);

    // 3. Dispatch matmul: scores = database @ query.T — Apple's optimized impl
    let mm = mps::MatrixMultiplication::new(
        device,
        transpose_left:  false,
        transpose_right: true,
        rows: n, cols: 1, interior: d,
        alpha: 1.0f64, beta: 0.0f64,
    );
    let cmd_buf = queue.new_command_buffer();
    mm.encode_to(cmd_buf, m_database, m_query, m_scores);
    cmd_buf.commit();
    cmd_buf.wait_until_completed();

    // 4. Top-K on host (or via MPSMatrixSoftMax + sort on device)
    let mut host_scores: Vec[f32] = vec::with_capacity::[f32](n);
    buf_scores.copy_to_slice(as_mut_bytes(host_scores));
    return top_k_indices(host_scores, k);
}
```

No `#[kernel]`. No shader source. No address spaces. No multi-target codegen. Every GPU operation is a call into Apple's MPS — Apple's matmul implementation, hand-tuned for every Apple Silicon generation.

The equivalent on NVIDIA (after `vendor/cuda` lands in v0.0.11):

```cplus
import "cuda/cuda"   as cuda;
import "cuda/cublas" as cublas;

fn search_top_k(query: f32[], database: f32[], n: usize, d: usize, k: usize) -> Vec[usize] {
    let device = cuda::Device::default()?;
    let stream = cuda::Stream::new(device)?;
    let handle = cublas::Handle::new(stream)?;

    let d_query    = cuda::DeviceBuffer::alloc::[f32](d)?;
    let d_database = cuda::DeviceBuffer::alloc::[f32](n *% d)?;
    let d_scores   = cuda::DeviceBuffer::alloc::[f32](n)?;
    d_query.copy_from_host(query)?;
    d_database.copy_from_host(database)?;

    handle.sgemv(cublas::Op::T, d, n, 1.0f32, d_database, d, d_query, 1, 0.0f32, d_scores, 1)?;

    let mut host_scores: Vec[f32] = vec::with_capacity::[f32](n);
    d_scores.copy_to_host(&mut host_scores)?;
    return top_k_indices(host_scores, k);
}
```

Same shape. Different SDK. C+ is the host language; cuBLAS does the math.

---

## What's in the language vs in packages

### LANG (one cycle of work, then done)

| Item | Status | Why lang |
|---|---|---|
| `#selector("name")` | v0.0.10 Phase 4A | Per-name cached global needs codegen support; literal-only arg needs sema validation. |
| `#msg_send(recv, "sel", args...) -> T` | v0.0.10 Phase 4B | Per-call C ABI synthesis can't be expressed as a function (no variadic generics). |
| `#compile_shader("path", target: "msl")` | v0.0.10 Phase 4C | Invokes external toolchain at sema time, embeds bytes. Mirrors `#include_bytes`. |
| `#` sigil convention for compiler intrinsics | v0.0.10 (replaces `!` suffix) | Visual signal that a name is dispatched from the compiler's hardcoded table, not normal function resolution. Parallels `#[attribute]`. |

Total: roughly one cycle of compiler work. After this, **the language is done with GPU**. No further lang additions are planned, ever.

### PACKAGES (the actual GPU story)

| Package | Status | What it binds |
|---|---|---|
| `vendor/metal` | exists since v0.0.9 | MTLDevice, Buffer, Library, Function, Pipeline, CommandQueue, CommandBuffer, ComputeCommandEncoder. |
| `vendor/metal/mps` | fast follow-up to v0.0.10 | Apple's Metal Performance Shaders: MPSMatrix, MPSMatrixMultiplication, MPSMatrixVectorMultiplication, MPSGraph. The "I have large numbers to crush" answer on Apple Silicon. |
| `vendor/cuda` | exists | CUDA Runtime API + cuBLAS (sgemm/sgemv), DeviceBuffer with Drop. Plain C FFI. (Driver API + cuFFT/cuSPARSE/cuDNN can extend it.) |
| `vendor/accelerate` | v0.0.11 | Apple's Accelerate framework — BLAS, LAPACK, vDSP, BNNS. Host-CPU SIMD numerics. The fallback when no GPU. |
| `vendor/cblas` | exists | Reference CBLAS bindings (OpenBLAS / Netlib / MKL). The cross-platform CPU fallback on systems without Accelerate. Self-tested on CPU. |

Each is pure `extern fn` declarations + thin typed wrappers + Drop impls. **No compiler work**.

**Dropped from the roadmap (not the language — these are packages):**
`vendor/vulkan` and `vendor/opencl`. CUDA + Metal already cover the GPU
targets that matter (NVIDIA + Apple Silicon), and `vendor/accelerate` /
`vendor/cblas` cover the CPU fallback; Vulkan-compute's surface is large
and its multi-backend portability is a *runtime* concern we don't need to
bind ahead of a concrete workload, and OpenCL is legacy. Neither is
"never" — the SDK-consumer model would welcome them — but they're off the
plan until a real consumer needs one. (Vulkan still appears below as the
sharpest *illustration* of the "are we Mojo?" discriminator; that's
reasoning, not a commitment to ship it.)

### Explicitly dropped (never going to ship)

| | Why |
|---|---|
| `#[kernel]` attribute | We don't write kernels. NVIDIA/Apple do. |
| Multi-target LLVM backends (NVPTX / AMDGPU / SPIR-V codegen) | Same — kernels come from vendor SDKs as precompiled blobs. |
| GPU execution intrinsics (`#thread_idx_x`, `#block_idx_x`, etc.) | Only useful inside `#[kernel]` — which we don't have. |
| Address-space qualifiers (`addrspace(3)` for shared memory) | Same. |
| GPU barrier intrinsics | Same. |
| Sema rules rejecting host code inside kernels | Same. |
| Tensor as a builtin type | A `Tensor[T]` struct in a vendor package handles it. No language work. |
| Operator overloading for tensor math | Explicitly rejected by principle. `a.add(b)` / `tensor::add(a, b)` instead. |
| Unified compute abstraction across backends | This is what makes Mojo Mojo. Not us. |

---

## The Mojo question (subtle but important)

A natural follow-up (Vulkan is off the roadmap — see above — but it's the cleanest case to reason through): if we *were* to ship a `vendor/vulkan`, wouldn't that effectively make C+ into Mojo? Vulkan abstracts over multiple GPU backends — NVIDIA, AMD, Intel, Apple-via-MoltenVK, Android. One SPIR-V kernel runs everywhere. Isn't that the Mojo promise?

**No, and the discriminator is sharp: where does the kernel source live?**

- **Mojo**: you write the kernel in **Mojo source**, the Mojo compiler emits per-target binaries. One language, one compiler, multi-target output. The kernel IS Mojo code.
- **C+ + vulkan**: you write the kernel in **GLSL / HLSL / SPIR-V** (Khronos's portable IR), compile it with **glslc** or **DXC** (Khronos's tools), and dispatch it from C+. The portability comes from **Vulkan as a runtime**, not from C+ as a language.

Analogy: binding to ffmpeg from C++ doesn't make C++ a video codec. ffmpeg is the codec; C++ orchestrates. Same here — Vulkan's runtime does the multi-target dispatch; C+ is just the host language that calls into Vulkan.

The cleanest test: **if you find yourself writing the kernel in `cplus` source, you've drifted into Mojo territory.** If you're writing it in GLSL / MSL / CUDA / HLSL / SPIR-V, you're still firmly in SDK-consumer territory regardless of how many backends the SDK abstracts over.

### The real failure mode to watch for

The risk isn't a multi-backend binding itself. The risk is a future hypothetical `vendor/tensor` package that wraps cuBLAS + MPS (+ any future backend) behind a single `Tensor[T]::matmul(other)` API. From the user's perspective, that would look like "C+ has cross-vendor GPU compute built in." Under the hood it's still calling vendor SDKs, but the abstraction would hide which one.

The honest read: at the **package** level, yes, you can build something Mojo-shaped on top of C+'s vendor bindings. Python is in this same situation — Python isn't Mojo even though PyTorch / NumPy / JAX exist. The language stays uncommitted; specific compute stories are libraries.

Decision: we **may** eventually ship a `vendor/tensor` package, but it would be a package users can opt into or skip. The language never bakes in the abstraction. Anyone who wants to see what's actually happening can drop down to the raw SDK call.

---

## What v0.0.11 actually becomes

The maximalist plan.gpu.md sketch put `#[kernel]` + multi-target codegen as the v0.0.11 anchor. **Replaced** with:

**v0.0.11 = the vendor-bindings cycle.** Zero compiler work for GPU. Three deliverables:

1. **MPS bindings** added to `vendor/metal` (fast follow-up; should land soon after v0.0.10's wedge).
2. **`vendor/cuda`** — Driver API + Runtime API + cuBLAS. The NVIDIA story.
3. **`vendor/accelerate`** — Apple's CPU-SIMD numerics for the no-GPU fallback.
4. **`proves/vector_search_server/`** — a real consumer that benchmarks against FAISS on the same data. The proof that the position works in practice.

Tensor / GEMM / FFT libraries can land as third-party packages whenever someone needs them.

---

## What this position buys us

1. **Honest scope.** Multi-target compiler work is months. Vendor bindings are weeks. We get a working GPU story in v0.0.11 instead of v0.1.x.

2. **No moat against NVIDIA / Apple.** They invest billions in MPS / cuBLAS. Trying to beat them at writing kernels is doomed. Ride their investments instead.

3. **No moat against PyTorch / Mojo.** We're not trying to win the ML research market — they own it. We're trying to be the auditable host language for inference, vector search, image / video pipelines, and other "ship a binary that crunches numbers" workloads.

4. **Preserves the locked principles.** No closures, no exceptions, no operator overloading, no hidden allocator — all intact. A `vendor/cuda` package doesn't require any of them.

5. **Composes with real-time positioning.** Edge AI is real-time AI. The combination of `#[no_alloc]` + GPU-via-vendor-SDKs is a real and currently underserved niche (Coral, ANE, mobile NPUs, autonomous vehicle perception).

6. **Clear stopping condition.** "Anything kernel-related is forever a package concern" gives us a bright line. Future feature proposals that violate it can be rejected cleanly.

---

## What this position costs us

1. **No "C+ for ML researchers" story.** That space belongs to PyTorch / JAX / Mojo. Their ergonomics + ecosystems are unmatched. We can't compete and we shouldn't try.

2. **Backend portability is the user's problem.** A C+ program that wants to run on both NVIDIA and Apple needs to be written against both `vendor/cuda` and `vendor/metal`, with startup-time backend selection. (Same as llama.cpp does.)

3. **CUDA bindings are a maintenance burden.** NVIDIA ships new APIs constantly. Keeping `vendor/cuda` current requires ongoing FFI work.

4. **No fancy demos.** Mojo can show "look, GPU matmul in 5 lines of Mojo source." We can't. Our demos are "look, here's how to wire MPS / cuBLAS from C+" — less impressive on social media, more honest about what's actually happening.

---

## Open questions

Worth tracking but not blocking:

- **`vendor/tensor` shape, if/when it lands.** Should it expose backend choice explicitly (`Tensor::cuda(...)` vs `Tensor::metal(...)`) or hide it via runtime dispatch (`Tensor::new(...)` picks based on what's available)? The first is more honest, the second is more ergonomic. No decision needed until someone has a workload.

- **GGUF / SafeTensors readers in `vendor/`.** If we're serious about the inference engine story, model format readers are useful. Worth considering once `vendor/cuda` exists.

- **Quantization helpers.** Int8, FP16, GPTQ, AWQ, etc. Probably belongs in a `vendor/quant` package built on top of the BLAS / MPS / cuBLAS primitives. Open question whether C+ has any structural advantage here vs llama.cpp's existing implementations.

- **WebGPU.** Browser-based compute is growing. WebGPU is shipping in Chrome / Safari / Firefox. A `vendor/webgpu` would let C+ compile to wasm + run on the browser GPU. Niche, but a clean fit for the SDK-consumer model. Wait for demand.

- **Real-time guarantees crossing the GPU boundary.** `#[no_alloc]` checks heap allocation on the CPU side. GPU memory allocation via `cuMemAlloc` / `[device newBufferWithLength:]` is a different path — does the attribute know about those? Probably needs an extension (`#[no_gpu_alloc]`?) or a transitive marker. Worth thinking about once both Phase 1 (real-time) and v0.0.11 (vendor bindings) are in.

---

## Reference points

- **[plan.md](../plan.md)** — current cycle plan. §"GPU" intro + Phase 4 + Post-v0.0.10 follow-ups capture the position.
- **[plan.gpu.md](../plan.gpu.md)** — the maximalist vision. Kept in-tree as historical record of the rejected direction. Do not implement.
- **[vendor/metal](../vendor/metal/)** — current Metal bindings (v0.0.9). 6 files, ~620 LOC. Drop-clean, Result-returning. Lacks MPS bindings — that's the fast follow-up.
- **[docs/COMPILER.md](COMPILER.md)** — compiler-internals reference. §11 explains the codegen / IR-text contract that `#compile_shader` plugs into.
- **research.md** — the original "what would it take" survey. Predates the position decision; its framing of GPU as "feasible but massive" is correct but the conclusion (build it anyway) is what we rejected.
- **[tutorial.md](../tutorial.md)** — language tutorial. §27 "Beyond stdlib" lists the existing vendor packages.

---

## How to continue the conversation

If you (Claude, future session) are picking this up:

The conversation is **not** about "should we build GPU support" — that's decided. It's about how to execute the consumer-of-SDKs position well. Likely next-conversation topics:

- **MPS binding details.** What's the minimal viable surface? Just MatrixMultiplication, or also MPSGraph (Apple's higher-level API)? How do we handle the `MPSDataType` enum cleanly?
- **`vendor/cuda` scope.** Driver API only? Driver + Runtime + cuBLAS? Do we include cuDNN (huge) or skip it? Versioning — CUDA 12 vs 11?
- **`proves/vector_search_server/` design.** HTTP server library — hand-roll or use something? Benchmarking methodology against FAISS.
- **`vendor/tensor` shape.** Should it exist at all? If yes, what backend-selection model?
- **Real-time + GPU interaction.** Does `#[no_alloc]` need a GPU-aware extension?

Things to NOT discuss without strong reason:
- Adding `#[kernel]` to the language.
- Multi-target LLVM codegen.
- GPU execution intrinsics in the language.
- Operator overloading for tensor math.
- Tensor as a builtin type.

Any of those would be reverting the decision documented in this file. If a future workload makes a strong case for revisiting one, that's a separate conversation that should explicitly cite this document and explain what changed.
