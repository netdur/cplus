# v0.0.10 — Real-time positioning + GPU binding-layer wedge

Two anchor themes. They're independent (work on either side advances regardless of the other) and they share a worldview: keep the locked design principles intact, ship the smallest set of language additions that lets a new domain land on top.

- **Real-time** — the locked principles (no GC, no closures, no exceptions, no hidden allocator, monomorphization, explicit Drop) already make C+ a soft-real-time-capable language. A `#[no_alloc]` attribute turns "incidentally suitable" into "verifiable contract" — the single highest-leverage piece for audio / embedded / game / HFT consumers.
- **GPU** — **position locked**: C+ is a *consumer* of GPU backend SDKs (the llama.cpp / FAISS pattern), not a *provider* of a unified compute abstraction (the Mojo pattern). We bind to vendor SDKs (Metal + MPS, CUDA + cuBLAS, Vulkan, Accelerate) and dispatch precompiled kernels written by NVIDIA / Apple / Khronos. We do NOT compile kernels ourselves. The [plan.gpu.md](plan.gpu.md) maximalist vision (`#[kernel]` attribute, NVPTX/AMDGPU/SPIR-V codegen, address spaces, GPU intrinsics) is **dropped, not deferred** — wrong shape for this positioning.

What v0.0.10 ships for GPU: three compiler intrinsics (`#selector`, `#msg_send`, `#compile_shader`) that make ObjC FFI dramatically less painful. These pay off for **any** ObjC binding (vendor/metal today, vendor/appkit, future CoreML / AVFoundation / MetalFX / Accelerate), so they're justified independently of GPU. v0.0.11 is the bindings cycle: MPS on top of vendor/metal, new `vendor/cuda` + cuBLAS, `vendor/accelerate` for CPU fallback. Zero language work for GPU after v0.0.10.

The "user with large numbers to crush" story (vector search, image processing, embedding generation, any heavy numerical workload): they write the host orchestration in C+ and dispatch the inner loop to `cublas::sgemv` / `mps::MatrixMultiplication` / `accelerate::cblas_sgemm`. Same shape FAISS uses. The kernels themselves are Apple's and NVIDIA's responsibility; we ride those investments.

---

## v0.0.9 carryover

| Item | Reason for deferral | v0.0.10 disposition |
|---|---|---|
| Phase 1 default-move flip | Wide blast radius — every stdlib + vendor call site that passes a non-Copy value needs audit. | **Phase 6** (this cycle). `borrow` keyword shipped in v0.0.9 as additive; flipping the default is the breaking-change half. |
| metal #10 (real compute kernel proves project) | External / requires a Metal compiler + shader source pipeline. | **Closed by Phase 4C** below — `#compile_shader` makes it feasible to ship without a separate Makefile. |
| Per-field TBAA | Perf-only; raytracer didn't show a win in v0.0.8. | Stays deferred until a workload measures it. |
| `vendor/sqlite` (dropped in v0.0.8) | "Wait for a real consumer." | Still deferred. |

---

## Phase 1 — `#[no_alloc]` attribute (real-time contract) · size S

The locked principles make C+ already real-time-capable. The thing missing is a **verifiable** contract — a way for an audio plugin / kernel module / hot-path author to prove "this function and everything it transitively calls never heap-allocates."

### Surface

```cplus
#[no_alloc]
pub fn render_audio_frame(out: f32x4[], in: f32x4[]) {
    // every fn called from here must be #[no_alloc] OR a known-leaf
    // (extern with no transitive allocator calls).
}
```

### Validation rules (sema)

A `#[no_alloc]` function may NOT:
1. Call `malloc` / `calloc` / `realloc` / `aligned_alloc` / `free` (hardcoded blocklist of extern names).
2. Call any function that isn't itself `#[no_alloc]` AND isn't a known-leaf (small whitelist: `memcpy`, `memset`, `memcmp`, `snprintf`-into-caller-buffer, `printf`-family, libc math).
3. Construct `vec::Vec[T]` / `string` / `box::Box[T]` / `arc::Arc[T]` / `rc::Rc[T]` / `map::HashMap[K, V]` (these all call `malloc` internally; type-level check catches them before transitive analysis).
4. Trigger any compiler-inserted allocation. (Today there are none — every alloc is explicit. Spelled out so the rule survives future feature work.)

A `#[no_alloc]` function MAY:
- Use stack allocations (`let x: [u8; N]`, `#addr_of(x)`).
- Call libc string/math/memory helpers that don't allocate.
- Call other `#[no_alloc]` functions (transitive check).
- Use `static`/`static mut` (storage is fixed at link time).
- Use any SIMD, atomic, or fast-path stdlib helper marked `#[no_alloc]`.

### Implementation

- `attrs.rs` — add `#[no_alloc]` to the attribute table. Target: `Function` only.
- `sema.rs` — new pass `check_no_alloc` after `check_function_bodies`. Builds the call graph for every `#[no_alloc]` function, walks transitively, errors on any blocklisted call or non-`#[no_alloc]` user function. New code `E0901` ("function marked #[no_alloc] calls allocating function `X`") with a label at the offending call site and a note showing the chain.
- `codegen.rs` — no changes. Sema-only contract.
- **Stdlib annotation pass** — mark every demonstrably non-allocating stdlib helper `#[no_alloc]`. Most of `stdlib/option`, much of `stdlib/atomic`, `stdlib/io::write_str`, etc. This is what makes the attribute usable in practice.

### Tests

- Positive: `#[no_alloc] fn pure_arith(x: i32) -> i32 { return x + 1; }` compiles.
- Negative E0901: `#[no_alloc] fn f() { vec::Vec::new::[i32](); }`.
- Transitive: `#[no_alloc] fn a() { b(); } #[no_alloc] fn b() { malloc(...); }` — error on `b` (point at the malloc, note the chain through `a`).
- Stdlib: `#[no_alloc] fn render() { let s = string::new(); }` errors.

### Cost / value

~150 LOC of sema + a few days of stdlib annotation. Opens a new audience (audio, embedded, game hot paths) and is something Rust's embedded WG has been asking for since 2018 and never quite got (closest equivalent: `#![no_std]` + `#[cfg(no_global_oom_handling)]`, still feature-gated nightly).

---

## Phase 2 — `vendor/static-arena` (real-time arena pattern) · size S

`#[no_alloc]` rules out `vendor/arena` (whose `Arena::new(chunk_size)` calls `malloc`). But a fixed-size arena allocated at startup and used during the hot path is real-time-safe. This package gives the audio / embedded crowd a concrete API.

```cplus
pub struct StaticArena[const N: usize] {
    buf: [u8; N],
    used: usize,
}

#[no_alloc] pub fn StaticArena[N]::new() -> StaticArena[N] { ... }
#[no_alloc] pub fn StaticArena[N]::alloc_bytes(mut self, n: usize) -> Option[*u8] { ... }
```

Marked `#[no_alloc]` end-to-end. Backed entirely by stack or `static mut` storage.

**Compiler prereq**: `const N: usize` on struct generic params. Sema today accepts `[T; N]` array types with literal N; extending struct generics is small but real work. If const-generics scope balloons, fall back to a `StaticArena16K` / `StaticArena64K` / `StaticArena256K` enum of fixed sizes and ship without the language work.

~100 LOC of `.cplus` + 5–8 in-package `#[test]` fns.

---

## Phase 3 — `#[bounded_recursion]` attribute (real-time follow-up) · size XS

`#[no_alloc]` catches heap allocation but not stack overflow. A `#[no_alloc]` function could still recurse unboundedly and blow the stack — an audio-thread crash that's just as bad as a malloc.

```cplus
#[no_alloc]
#[bounded_recursion]
pub fn audio_callback(in: f32x4[], out: f32x4[]) { ... }
```

Sema runs the same call-graph walk as `#[no_alloc]` and rejects any back-edge that leads back to the original function. Applies transitively. ~50 LOC of sema — same machinery, different rejection predicate.

---

## Phase 4 — GPU binding-layer wedge

### Notation: `#` prefix for compiler intrinsics

v0.0.10 adopts `#name(...)` as the syntax for compiler intrinsics, replacing the inconsistent mix of `!`-suffix (`include_bytes!`, `env!`) and bare-name (`addr_of`, `size_of`) intrinsics in v0.0.9. The `#` family parallels the existing `#[attribute]` family — both are "compiler reads this name from a fixed table"; the bracket-form attaches to items, the paren-form attaches to expressions.

Migration in this cycle (one PR, mechanical):
- `include_bytes!("path")` → `#include_bytes("path")`
- `include_str!("path")` → `#include_str("path")`
- `env!("NAME")` → `#env("NAME")`
- `addr_of(x)` → `#addr_of(x)`
- `size_of::[T]()` → `#size_of::[T]()`
- `align_of::[T]()` → `#align_of::[T]()`

The old spellings are removed outright in the same PR — pre-1.0, no users, no deprecation cycle needed. Sema dispatches `#name(...)` against a hardcoded intrinsic table; an unknown `#frobnicate` produces `E0905: unknown intrinsic '#frobnicate'` and does NOT fall back to function lookup. A user can freely define `fn addr_of(...)` — the two names live in disjoint namespaces.

### Lang vs package responsibilities

The position locked above (C+ as SDK consumer, not kernel compiler) means the v0.0.10 work is the only language work GPU ever needs. Everything else is packages forever.

| Concern | Where it lives | Why |
|---|---|---|
| `#selector(name)` intrinsic | **Lang** (cpc) | Per-name cached global needs codegen support; literal-only arg needs sema validation. |
| `#msg_send(recv, sel, args...) -> T` intrinsic | **Lang** (cpc) | Per-call C-ABI synthesis can't be expressed as a function (no variadic generics). |
| `#compile_shader(path, target)` intrinsic | **Lang** (cpc) | Invokes external toolchain at sema time; embeds bytes as a private global. Mirrors `#include_bytes`. |
| Typed wrappers for ObjC objects (`Device`, `Buffer`, `Library`, `CommandQueue`, `CommandBuffer`, `ComputeCommandEncoder`, `Function`, `Pipeline`, `MTLSize`, `MetalError`) | **Package** (`vendor/metal`) | Pure C+ types with Drop impls. Already exist; refactor after lang lands. |
| Error discrimination (`MetalError::NoDefaultDevice`, etc.) | **Package** | Domain knowledge of which `objc_msgSend` returns mean what. Belongs in the binding library. |
| Dispatch convenience (`dispatch_1d(threads)`, `dispatch_2d(w, h)`) | **Package** | Composes the existing `dispatch` primitive. Zero language support needed. |
| Buffer / kernel-argument typing (`encoder.set_buffer::[T](i, buf)`) | **Package** | Generic on the buffer element type; no language change needed. |
| Result-returning factories (`device.new_library_with_data() -> Result[Library, MetalError]`) | **Package** | Already shipped in v0.0.9. |
| `GPUBuffer[T]` host-device memory abstraction | **Package** (post-v0.0.10) | Built on top of the wrappers; works today, just nobody's written it. |
| MPS (Metal Performance Shaders) bindings — `MPSMatrixMultiplication`, `MPSMatrixDescriptor`, `MPSGraph`, etc. | **Package** (`vendor/metal/src/mps.cplus` — fast follow-up to v0.0.10, see §"Post-v0.0.10 fast follow-ups") | Apple's pre-optimized matmul/conv/FFT/softmax. The "I have large numbers to crush" answer on Apple Silicon. |
| CUDA driver + runtime + cuBLAS + cuFFT + cuSPARSE + cuDNN | **Package** (`vendor/cuda` — v0.0.11 anchor) | Plain C FFI — simpler than ObjC. The NVIDIA story. |
| Accelerate framework bindings (BLAS, LAPACK, vDSP, BNNS) | **Package** (`vendor/accelerate` — v0.0.11) | Apple's host-CPU SIMD numerics — useful as a fallback when no GPU. |
| Vulkan compute API | **Package** (`vendor/vulkan` — later) | Cross-vendor GPU (Linux, Android, AMD, Intel). Lower priority than CUDA + Metal. |
| Tensor / GEMM / reduction libraries on top of the vendor SDKs | **Package** (eventual) | Compose the precompiled vendor primitives. No language work needed. |

**Explicitly NEVER in C+** (dropped from [plan.gpu.md](plan.gpu.md), not deferred):

| Concern | Why dropped |
|---|---|
| `#[kernel]` attribute | We don't write kernels. NVIDIA/Apple do. |
| Multi-target LLVM backends (NVPTX / AMDGPU / SPIR-V) | Same — kernels come from vendor SDKs as precompiled blobs. |
| GPU execution intrinsics (`#thread_idx_x`, `#block_dim_x`, etc.) | Only useful inside `#[kernel]` — which we don't have. |
| Address-space qualifiers (`addrspace(3)` for shared memory) | Same. |
| GPU barrier intrinsics | Same. |
| Sema rules rejecting host code inside kernels | Same. |
| Tensor as a builtin type | A `Tensor[T]` struct in a vendor package handles this. No language work. |

The split rule: language work is justified only when it lets us **bind to existing C / ObjC SDKs more ergonomically**. Anything kernel-related is forever a package concern (the package's kernels live in `.metal` / `.cu` / `.spv` files compiled by vendor toolchains).

### Impact summary

| Intrinsic | Friction today | After |
|---|---|---|
| **`#selector("name")`** | 14 `runtime::sel(str_ptr("foo\0"))` call sites + the `\0`-termination dance | One token; compiler caches the registered selector in a global |
| **`#msg_send(recv, "sel", args...) -> T`** | 14 `#[link_name = "objc_msgSend"] extern fn objc_msg_<shape>(...)` declarations, one per arity/return-type combo | Zero externs; compiler synthesizes the right per-call C ABI |
| **`#compile_shader("kernel.metal", target: "msl")`** | User runs `xcrun metallib` separately, `#include_bytes` the blob, hopes the file is current | Compiler runs the toolchain at sema time, embeds bytes, shader compile errors become C+ compile errors |

After all three: `vendor/metal/src/runtime.cplus` shrinks from 221 LOC to ~50, and a real `proves/metal_compute/` project becomes feasible (closes v0.0.9's metal #10 gap).

Crucially: **none of these commit us to multi-target codegen**. Kernel source stays in `.metal` / `.cu` / `.spv` files, compiled by vendor toolchains. Consistent with the "C+ is a consumer of GPU SDKs, not a provider of a compute abstraction" position locked at the top of this plan.

### Phase 4A — `#selector("name")` intrinsic · size XS

```cplus
let s: *u8 = #selector("setBuffer:offset:atIndex:");
```

**Lowering**: at sema time the compiler interns the literal into a `__cplus.selector.<mangled>` pair: `{[N x i8] data, *u8 cached}`. At codegen each call expands to:

```llvm
%cached = load ptr, ptr @__cplus.selector.<n>.cached
%is_null = icmp eq ptr %cached, null
br i1 %is_null, label %register, label %done
register:
  %sel = call ptr @sel_registerName(ptr @__cplus.selector.<n>.data)
  store ptr %sel, ptr @__cplus.selector.<n>.cached
  br label %done
done:
  %s = phi ptr [%cached, %entry], [%sel, %register]
```

Per-call cost: one load + one branch (predicted-taken after the first call). The `\0`-termination, `str_ptr` wrapping, and the explicit `sel_registerName` extern all disappear from the source.

**Thread-safety**: the cached-pointer global is racy, but the race is benign (all losers compute the same `*u8`). Document and move on; do NOT add an atomic load/store on the hot path.

**Implementation**:
- `lexer.rs` — `#` prefix recognized; followed by an identifier token (not `[`) dispatches as an intrinsic call.
- `parser.rs` — `#name(...)` parses as an intrinsic-call expression.
- `sema.rs` — handle `#selector` in the intrinsic dispatch table. Single string-literal arg; error `E0903` ("selector name must be a string literal") otherwise. Record into `MonoInfo.selectors: BTreeMap<String, SelectorEntry>`.
- `codegen.rs` — `emit_selector_globals` emits one pair per unique name; call-site lowers to the load+branch+register pattern.

~80 LOC for the intrinsic itself (the `#` parsing is shared infrastructure with the migration above).

### Phase 4B — `#msg_send(recv, "selector", args...) -> T` intrinsic · size S

Today `vendor/metal/src/runtime.cplus` declares 14 `#[link_name = "objc_msgSend"]` externs, one per arity/return-type combo. Any new shape (e.g. a 6-arg selector or one returning `f32`) needs another extern + matching helper.

```cplus
let lib: *u8 = #msg_send(
    device,
    "newLibraryWithData:error:",
    dispatch_data: *u8,
    null_err:      *u8,
) -> *u8;

#msg_send(encoder, "endEncoding");                          // void return
let n: u64 = #msg_send(ns_string, "length") -> u64;         // primitive return
```

The compiler synthesizes the right `objc_msgSend(receiver, selector, args...) -> ret` call signature from the type annotations. Selector is auto-passed via `#selector(...)` from 4A.

**Why an intrinsic and not a function**: `objc_msgSend` is variadic in the C-ABI sense (each arg type matters for codegen) but C+ doesn't have variadic generics. The intrinsic shape lets us synthesize the per-callsite signature in sema without forcing every shape to be declared.

**Implementation**:
- `parser.rs` — parse the special arg-with-type-annotation list and the optional `-> RetTy` after the closing paren.
- `sema.rs` — build a `FnSig` for the call; record an `objc_msgsend_call` entry in `MonoInfo` keyed by per-shape mangled name.
- `codegen.rs` — emit one `declare ptr @objc_msgSend.<mangle>(...)` per unique shape; call site lowers to the typed call.

**Net effect on vendor/metal**: 14 extern declarations + 14 wrapper helpers collapse to zero. Every `msg_*` call site in queue / device / pipeline / buffer rewrites to `#msg_send(self.raw, "selectorName", args...) -> RetTy`. runtime.cplus shrinks 221 → ~50 LOC.

~200 LOC of compiler work.

### Phase 4C — `#compile_shader("path", target: "msl")` intrinsic · size M

Today shader source lives in a `.metal` file; users run `xcrun metallib` out-of-band, `#include_bytes` the blob, then `device.new_library_with_data(blob)`. The compiler can't see the source, can't catch compile errors at build time, can't detect that the file changed.

```cplus
let library_bytes: *[u8; N] = #compile_shader("./shaders/double.metal", target: "msl");
let library: metal::Library = match device.new_library_with_data(
    unsafe { slice_from_raw_parts(library_bytes as *u8, N) }
) {
    result::Result::Ok(lib) => lib,
    result::Result::Err(e) => return /* handle */,
};
```

**Lowering**: at sema time the compiler runs `xcrun -sdk macosx metal -c <path> -o <tmp.air> && xcrun -sdk macosx metallib <tmp.air> -o <tmp.metallib>`, reads the resulting bytes, emits them as a `@.shader.N = private constant [N x i8]` global. Call site resolves to the global's address. Shader compile errors become C+ compile errors (with the `.metal` file's line numbers in the diagnostic).

The `target:` keyword arg selects the toolchain:
- `"msl"` → `xcrun -sdk macosx metal` + `metallib` (ship in v0.0.10).
- `"ptx"` → `nvcc --ptx` (deferred until anyone asks).
- `"spirv"` → `glslc -fshader-stage=compute` (deferred).

**Implementation**:
- `sema.rs` — handle in the intrinsic dispatch table (kin to `#include_bytes`). Resolve path relative to source file. Invoke toolchain via `Command::new`. On non-zero exit, parse stderr for line numbers and emit `E0904` with the shader file as primary span.
- `codegen.rs` — same path as `#include_bytes` — bytes go in `MonoInfo.compile_time_blobs`, emitted as a private constant global, call site resolves to the global address.
- `manifest.rs` — optional `[shader-tools]` table that overrides the default toolchain command (for cross-shader-language projects).

~200 LOC + Command-spawning machinery. Closes metal #10.

**Build-cache question**: `xcrun metallib` takes ~100ms per shader. For tight inner-loop iteration that adds up. v0.0.10 ships without an mtime-based cache; revisit if a real workload measures it.

---

## Phase 5 — Phase 1 default-move flip (carryover from v0.0.9) · size M

The `borrow` keyword landed in v0.0.9 as additive (no-op marker). v0.0.10 flips the default: non-Copy value parameters move by default; `borrow` is the opt-out.

```cplus
// v0.0.9 semantics:
fn echo(s: string) -> string { return s; }   // s is copied (silent double-free risk)

// v0.0.10 semantics:
fn echo(s: string) -> string { return s; }            // s is MOVED
fn echo(borrow s: string) -> string { return s; }     // explicit no-move; old behavior
```

### Migration surface

Every stdlib + vendor function that takes a non-Copy parameter needs audit:
- Consumes the value (final use, returns it, stores it) → no change; new default matches.
- Inspects the value (length check, hash, comparison) → add `borrow` prefix.

Estimated churn (from a grep across `vendor/stdlib/`):
- `vec.cplus`: ~6 methods need `borrow` (`len`, `get`, `iter`).
- `hash_map.cplus`: ~10 methods.
- `string.cplus`: ~5 methods.
- `option.cplus` / `result.cplus`: most methods are generic over `T`, so the new default + `borrow` rule both work generically — minimal explicit annotation.

Total: ~50–80 site annotations across stdlib. Each vendor package needs a similar pass.

### Implementation

- `sema.rs` — flip the default in `Param` construction; require `borrow` for non-consuming uses; error `E0902` ("non-Copy parameter `x` is moved by default; add `borrow` if the caller should retain ownership") with a fix-it suggestion.
- `borrowck.rs` — already treats `move` and non-`move` distinctly; no changes once sema flips the default.
- `monomorphize.rs` — no changes (operates on resolved types).

### Tests

- Positive: `fn consume(s: string) { ... }` followed by `consume(my_string); print(my_string);` errors `E0335` (use of moved value).
- Migration check: every stdlib package's existing test suite passes after the annotation pass.
- New `E0902` test: `fn echo(s: string) { print(s); }` where the body re-reads `s` after return — sema fires.

---

## Suggested ordering

Phases 1, 2, 3 are real-time; 4 is GPU; 5 is migration. They're mostly independent — pick by appetite.

1. **Phase 1 (`#[no_alloc]`)** — smallest piece, biggest user-facing realization, validates the real-time positioning. Do first.
2. **Phase 4A + 4B (`#selector` + `#msg_send`)** — pair these. Land them, then measure how much vendor/metal actually shrinks.
3. **Phase 4C (`#compile_shader`)** — depends on Command-spawning machinery; do after 4A/4B so vendor/metal has its new shape before adding the shader pipeline.
4. **Phase 2 (`vendor/static-arena`)** — depends on Phase 1's `#[no_alloc]` annotation. Either ship the const-generics version or the fixed-size-enum fallback.
5. **Phase 3 (`#[bounded_recursion]`)** — small follow-up to Phase 1. Easy to ship any time.
6. **Phase 5 (default-move flip)** — bigger blast radius. Save for the back half of the cycle when other work is stable.

---

## Post-v0.0.10 fast follow-ups (pre-v0.0.11)

These are **pure package work** (no compiler changes) that should land as fast follow-ups once Phase 4 is in. They validate the "C+ as GPU SDK consumer" position concretely.

### MPS bindings for `vendor/metal` · size S

Apple's Metal Performance Shaders ship pre-tuned matmul, convolution, FFT, softmax, and reductions for every Apple Silicon generation. Today vendor/metal binds the low-level compute infrastructure but **not** MPS, so users wanting matmul-on-GPU must write the kernel themselves in MSL.

Adding MPS:
1. `vendor/metal/Cplus.toml` — add `MetalPerformanceShaders` to `[link].frameworks`.
2. `vendor/metal/src/mps.cplus` — bind `MPSMatrix`, `MPSMatrixDescriptor`, `MPSMatrixMultiplication`, `MPSMatrixVectorMultiplication`, optionally `MPSGraph` (the newer high-level API). `MPSDataType` enum (`F16`, `F32`, `F64`, `I8`, `U8`).
3. Drop impls (same `objc_release` pattern as existing wrappers).
4. ≥2 `#[test]` fns: known 2×2 matmul, larger correctness check vs scalar.

After Phase 4A + 4B land, this becomes ~150-200 LOC of pure `#msg_send` calls. Before Phase 4, it's ~300 LOC against the existing `runtime::msg_*` helpers — still trivially doable; the v0.0.10 wedge just makes it shorter.

### `proves/vector_search_server/` · size M

A real consumer that proves the loop end-to-end. Sketch:
- HTTP server (probably hand-rolled `vendor/net` or a thin libuv binding) on a fixed port.
- Loads N×D embedding database from disk at startup.
- Per-request: take query vector → cosine-sim against database via `MPS::MatrixVectorMultiplication` → top-K reduction → JSON response.
- Benchmark against FAISS on the same data; ship the numbers.

This is the canonical "I have large numbers to crush" workload. If C+ + MPS beats / matches FAISS-CPU on Apple Silicon for plausible N and D, that's the headline.

### `vendor/cuda` skeleton · size M (v0.0.11 anchor)

Plain C FFI bindings for the CUDA Driver API + Runtime API + cuBLAS. No ObjC complexity. Once landed, the same `vector_search_server` example works on NVIDIA hardware by swapping `vendor/metal` → `vendor/cuda` and `MPS::Matmul` → `cublas::sgemv`. Cross-vendor support without a unified abstraction — each backend is its own package, the user picks at startup.

---

## Out-of-scope for v0.0.10

- **Full GPU multi-target codegen** (the [plan.gpu.md](plan.gpu.md) maximalist vision). **Dropped from the roadmap.** C+ binds GPU vendor SDKs; it does not compile kernels itself. plan.gpu.md remains in-tree as a historical record of the rejected direction.
- **`#[interrupt]` / `#[naked]` for embedded** — wait for a real embedded consumer. The architecture supports it (clang has the attributes; we'd plumb them through the attribute table) but designing without a workload is premature.
- **Lock-free queue primitives** — `stdlib/atomic` is enough today; building MPMC / SPSC ring buffers as in-tree stdlib types waits for a consumer.
- **Tensor types** — research.md argues the "`Tensor[T]` as a vendor package using existing primitives" path is fine today. Don't build into the language.
- **Operator overloading for tensor math** — explicitly rejected. The principle stands.

---

## Open questions (do not block phase work)

- **`#[no_alloc]` interaction with generics.** If a `#[no_alloc]` fn calls `vec.get(i)` on a generic `T`, what does the contract become per instantiation? Tentatively: the constraint propagates — the instantiation is rejected if T's Drop impl allocates. Worth a sema pass design before Phase 1 lands.
- **`#selector()` cache pattern thread-safety.** ObjC's `sel_registerName` is thread-safe (well-known idiom); our cached `*u8` global is racy but the race is benign (all losers compute the same value). Document; no atomic on hot path.
- **`#compile_shader()` and incremental builds.** ~100ms per shader compile is fine for small projects, adds up for large ones. Worth measuring before adding mtime-based caching.
- **Per-field TBAA** — stays open from v0.0.9. If a tensor / gemm consumer surfaces under the GPU work, revisit.
