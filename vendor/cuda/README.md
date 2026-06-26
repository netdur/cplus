# vendor/cuda

CUDA Runtime API + cuBLAS bindings for C+. The NVIDIA half of the GPU
story described in [docs/GPU.md](../../docs/GPU.md): C+ is a **consumer of
GPU backend SDKs**, not a kernel language. These are plain `extern fn`
bindings — NVIDIA's libraries do the math; C+ allocates, copies,
dispatches, and synchronizes.

## Requirements

- An NVIDIA GPU and the **CUDA Toolkit** (provides `libcudart` + `libcublas`).
- The toolkit's `lib64` directory. The package's `Cplus.toml` points
  `[link] search-paths` at `/usr/local/cuda/lib64` — **edit it if your
  toolkit lives elsewhere** (e.g. `/opt/cuda/lib64`, a versioned path, or
  `$CUDA_HOME/lib64`). `search-paths` emits `-L<dir>` at link time and
  `-Wl,-rpath,<dir>` so the binary also finds the `.so` at runtime — no
  `LD_LIBRARY_PATH` needed.

## Modules

| Module | Surface |
|---|---|
| `cuda/runtime` | `device_count()`, `set_device()`, `synchronize()`, `CudaError` (`.message()` → `Text`, `.code()`) |
| `cuda/buffer`  | `alloc(bytes:)`, `DeviceBuffer` (Drop = `cudaFree`), `.write(from:, bytes:)` / `.read(to:, bytes:)`, `.device_ptr()`, `.byte_count()` |
| `cuda/cublas`  | `handle()`, `Handle` (Drop = `cublasDestroy`), `sgemm`, `sgemv`, `Op` |
| `cuda/cuda`    | facade re-exporting the types |

`DeviceBuffer` and `Handle` are owned handles with `Drop`, so device memory
and the cuBLAS context are released at scope exit. cuBLAS is **column-major**
— lay out matrices accordingly, or transpose via `Op::T`.

## Example — single-precision GEMM on the GPU

`C = A * B` for column-major 2×2 matrices (`A=[[1,2],[3,4]]`,
`B=[[5,6],[7,8]]` → `C=[[19,22],[43,50]]`, stored column-major as
`[19,43,22,50]`):

```cplus
import "cuda/runtime" as rt;
import "cuda/buffer"  as buf;
import "cuda/cublas"  as blas;
import "stdlib/result" as result;

// ... host arrays hostA = [1,3,2,4], hostB = [5,7,6,8] (column-major) ...

guard let result::Result[buf::DeviceBuffer, rt::CudaError]::Ok(da) = buf::alloc(bytes: 16 as usize) else { return 1 as i32; };
let dA: buf::DeviceBuffer = da;            // owned; freed on scope exit
// (same for dB, dC)

let _ = dA.write(from: hostA, bytes: 16 as usize);
let _ = dB.write(from: hostB, bytes: 16 as usize);

guard let result::Result[blas::Handle, rt::CudaError]::Ok(hh) = blas::handle() else { return 2 as i32; };
let h: blas::Handle = hh;

// Each device op returns Option[CudaError]: None on success, Some(err) keeps
// the CUDA code (recover via `err.message()` / `err.code()`).
let _ = h.sgemm(
    blas::Op::N, blas::Op::N,
    2 as i32, 2 as i32, 2 as i32,
    1.0f32, dA.device_ptr(), 2 as i32, dB.device_ptr(), 2 as i32,
    0.0f32, dC.device_ptr(), 2 as i32);
let _ = rt::synchronize();
let _ = dC.read(to: hostC, bytes: 16 as usize); // hostC == [19, 43, 22, 50]
```

Pass `DeviceBuffer.device_ptr()` (a device pointer) to `sgemm`/`sgemv` — that keeps
ownership of the buffer with the caller (the method takes the pointer, not
the buffer). `alpha`/`beta` are host scalars; the binding stages them for
cuBLAS's default host-pointer mode.

For matrix-vector work (e.g. vector search: `scores = database @ query`),
use `Handle::sgemv`.
