# C+ GPU Compute Plan

This document outlines the architectural boundaries, design specifications, and implementation roadmap for GPU heterogeneous compute (native kernel execution) in the C+ compiler (`cpc`) and its standard library ecosystem.

---

## 1. Architectural Division: Language vs. Package

To maintain C+'s locked principles (zero-overhead, no operator overloading, and explicit, auditable code), GPU compute is strictly divided between compiler-native capabilities and user-space package abstractions.

### Responsibility Matrix

| Feature / Capability | Location | Lowering / Mechanics / FFI | Rationale |
| :--- | :--- | :--- | :--- |
| **Kernel Syntax & Attribute** (`#[kernel]`) | **Language** | AST parsing & multi-target separation | Identifies functions that compile to device instruction sets instead of CPU. |
| **Multi-Target Backend** (NVPTX/AMDGPU/SPIR-V) | **Language** | LLVM GPU backends and target triples | Compiles GPU code into target-specific binaries (e.g., PTX or GCN). |
| **Binary Payload Embedding** | **Language** | Static binary embedding in host `.rodata` | Embeds the compiled GPU kernel binary inside the host executable. |
| **GPU Execution Indices** (e.g., `thread_idx()`) | **Language** | GPU special registers (e.g., `@llvm.nvvm.read.ptx.sreg.tid.x`) | Queries hardware thread and block dimensions directly on-device. |
| **Memory Address Spaces** (Global vs. Shared/Local) | **Language** | LLVM `addrspace(N)` pointer attributes | Maps pointer variables to different physical GPU memory levels (SRAM vs VRAM). |
| **GPU Synchronization** (e.g., group barriers) | **Language** | Hardware barrier instructions | Inserts barriers (e.g., `@llvm.nvvm.barrier0`) for thread-block synchronization. |
| **Sema GPU Constraints** | **Language** | Semantic validation passes | Rejects CPU-only logic inside kernels (e.g., host allocation, host print, I/O, recursion). |
| **Device Memory Allocator & RAII** (`GPUBuffer[T]`) | **Package** | Host driver FFI APIs (`cudaMalloc`, etc.) | Safely manages VRAM allocation lifecycles and transfers from CPU. |
| **Device Discovery & Contexts** | **Package** | Driver C FFI bindings (Metal/CUDA APIs) | Discovers available devices, sets up queues, and handles contexts. |
| **Kernel Launching** (`device.launch(...)`) | **Package** | Driver FFI launch API calls | Sets up grid/block sizes and invokes execution parameters on the device. |
| **GPU Libraries** (GEMM, reductions, transforms) | **Package** | Sequence of kernel execution dispatches | Implements high-level library code (e.g., Matrix Multiply, FFT) on top of GPU kernels. |

---

## 2. Compiler Implementation Specifications (`cpc`)

Implementing GPU compute requires extending `cpc`'s AST, type-checker, and backend pipeline.

### A. Kernel Declarations & Constraints
A kernel is marked by a special attribute. The type checker (`sema.rs`) enforces strict GPU restrictions:
```cplus
#[kernel]
pub fn vec_add_kernel(a: *const f32, b: *const f32, c: *mut f32, n: usize) {
    let i: usize = gpu::thread_idx_x() + gpu::block_idx_x() * gpu::block_dim_x();
    if i < n {
        unsafe {
            *c.offset(i) = *a.offset(i) + *b.offset(i);
        }
    }
}
```
**Sema Constraints (Compile-Time Rejections)**:
1. **No host allocations**: Any call to dynamic memory allocators (e.g., `alloc`, `Vec`, `String`) is rejected.
2. **No Host FFI or I/O**: Calls to OS operations, filesystem, socket FFI, or CPU-side libraries are banned.
3. **No CPU Threads**: Usage of CPU threading APIs (`thread::spawn`) is banned.
4. **No Recursion**: Stack recursion is rejected because GPU stack limits are extremely low.

### B. Address Spaces & Physical Memory Hierarchy
To support GPU memory optimization (like shared SRAM caching), the compiler must recognize different address spaces in pointers:
* **Global Memory**: Standard device pointers mapping to LLVM Address Space `0` (or target-specific global space, e.g. CUDA `1`).
* **Shared/Local Memory**: Declared with attributes like `#[shared]` or specialized wrapper arrays. Lowers to LLVM `addrspace(3)` representing on-chip Shared SRAM (e.g. `__shared__` in CUDA).
* **Private Memory**: Standard local variables lowering to GPU register allocation.

### C. Multi-Target Compilation Pipeline
The compilation flow in `cpc` will split into two paths:
1. **Host Path**: Normal compilation of the program for the host CPU target.
2. **Device Path**:
   * Extracts all `#[kernel]` functions and their transitive dependencies.
   * Compiles them using an LLVM GPU target triple (e.g., `nvptx64-nvidia-cuda` for NVIDIA or `amdgpu-amd-amdhsa` for AMD).
   * Compiles the AST into a GPU binary blob (PTX assembly, AMD GCN bytecode, or SPIR-V).
   * Embeds this GPU binary block as a static byte array in the host CPU executable's `.rodata`.
   * Emits a descriptor mapping the kernel name `vec_add_kernel` to its location in the embedded binary blob.

---

## 3. Package API Design (`vendor/gpu`)

The library ecosystem exposes the runtime API needed to control the GPU from host CPU code.

### A. Memory Management (Host-Device Bridge)
```cplus
pub struct GPUBuffer[T] {
    ptr: *mut T,
    len: usize,
}

impl[T] GPUBuffer[T] {
    pub fn alloc(len: usize) -> GPUBuffer[T] {
        let dev_ptr = unsafe { gpu_ffi::malloc(len * sizeof(T)) };
        return GPUBuffer { ptr: dev_ptr, len: len };
    }

    pub fn copy_to_device(self, host_slice: []T) {
        unsafe { gpu_ffi::memcpy_h2d(self.ptr, host_slice.ptr, self.len * sizeof(T)) };
    }

    pub fn copy_to_host(self, host_slice: []mut T) {
        unsafe { gpu_ffi::memcpy_d2h(host_slice.ptr, self.ptr, self.len * sizeof(T)) };
    }
}

impl[T] Drop for GPUBuffer[T] {
    fn drop(mut self) {
        unsafe { gpu_ffi::free(self.ptr) };
    }
}
```

### B. Kernel Execution Dispatch
```cplus
fn run_compute() {
    let n: usize = 1000000;
    let a_device = GPUBuffer::alloc(n);
    let b_device = GPUBuffer::alloc(n);
    let c_device = GPUBuffer::alloc(n);

    // Initialize data...
    
    // Launch configuration: (grid_size, block_size)
    let grid = Dim3 { x: (n + 255) / 256, y: 1, z: 1 };
    let block = Dim3 { x: 256, y: 1, z: 1 };

    gpu::launch(
        vec_add_kernel, 
        grid, 
        block, 
        [a_device.ptr, b_device.ptr, c_device.ptr, n]
    );

    gpu::synchronize();
}
```
