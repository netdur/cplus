# C+ SIMD Plan

This document outlines the architectural boundaries, current implementation status (as of v0.0.8), and future roadmap for SIMD (Single Instruction, Multiple Data) in the C+ compiler (`cpc`) and its standard library ecosystem.

---

## 1. Architectural Division: Language vs. Package

To maintain C+'s locked principles (such as zero-overhead abstractions, no operator overloading, and explicit, auditable code), SIMD capabilities are strictly divided between compiler-native features and user-space package wrappers.

### Responsibility Matrix

| Feature / Capability | Location | LLVM Lowering / Mechanics | Rationale |
| :--- | :--- | :--- | :--- |
| **Vector Types** (`f32x4`, `i32x4`, etc.) | **Language** | `<N x Ty>` register layout | Maps directly to hardware vector registers (NEON, SSE/AVX). |
| **Mask Types** (`mask32x4`, etc.) | **Language** | `<N x i1>` (or width-matched signed int) | Represents vector comparison results for lane blending. |
| **Arithmetic / Math Methods** (`add`, `mul`, `fma`, `sqrt`) | **Language** | `fadd`, `fmul`, `@llvm.fma.*`, `@llvm.sqrt.*` | Directly translates to hardware SIMD instructions. |
| **Permutations** (`swizzle`, `reverse`, `interleave`) | **Language** | `shufflevector` with constant masks | Requires constant-pool mask generation in the backend. |
| **Reductions** (`sum`, `any`, `min_across`) | **Language** | `@llvm.vector.reduce.*` | Compiles to horizontal vector reduction hardware instructions. |
| **Memory Operations** (`load`, `store`) | **Language** | Aligned/unaligned `load` and `store` | Direct pointer dereferencing and vectorized memory access. |
| **High-level Vectors** (`Vec3`, `Vec4`) | **Package** | Structure wrapping primitive SIMD type | User-space math abstraction with clear domain-specific names. |
| **Linear Algebra** (`Mat4x4`, `dot`, `cross`) | **Package** | Sequence of compiler-native SIMD methods | Domain-specific algorithms built on compiler primitives. |
| **Invariants Management** (e.g., lane-3-zero in `Vec3`) | **Package** | Zeroing parameter registers during build/ops | Pure software contract enforced by the library API. |

---

## 2. Current Implementation Status (v0.0.8)

The foundation of SIMD is fully implemented in the compiler and packaged in `vendor/simd`.

### Compiler Capabilities (`cpc`)
* **Types**: Supports 128-bit (`f32x4`, `f64x2`, `i32x4`, etc.) and 256-bit (`f32x8`, `i64x4`, etc.) primitive vector widths along with their matching mask types (e.g., `mask32x4`).
* **Sema Validation**:
  * Enforces that lane counts match between operating vectors.
  * Validates that indices passed to `.lane(i)`, `.with_lane(i, x)`, and `.swizzle([idx])` are **compile-time constants**.
  * Maps comparisons (e.g. `lt`, `ge`) to width-matched mask types.
* **Codegen**: Lowers all core methods to optimized LLVM IR instructions or LLVM target intrinsics.

### Package Capabilities (`vendor/simd`)
Located in [vendor/simd/src/](file:///Users/adel/Workspace/C+/vendor/simd/src/):
* **[vec3.cplus](file:///Users/adel/Workspace/C+/vendor/simd/src/vec3.cplus)**: 3D vector wrapping `f32x4` that enforces the **lane-3-zero invariant**. This invariant keeps dot-products and lane-wise min/max/clamp mathematically correct without masking overhead. It implements `cross`, `normalize`, `reflect`, `refract`, and `lerp` entirely in SIMD.
* **[vec4.cplus](file:///Users/adel/Workspace/C+/vendor/simd/src/vec4.cplus)**: 4D vector wrapping `f32x4` with no padding, ideal for homogeneous coordinates.
* **[mat4x4.cplus](file:///Users/adel/Workspace/C+/vendor/simd/src/mat4x4.cplus)**: Column-major 4x4 matrix backed by `[Vec4; 4]`. Uses chained FMA (Fused Multiply-Add) operations to achieve high-performance matrix-vector (`mul_vec`) and matrix-matrix (`mul`) multiplication.

---

## 3. Future Roadmap

### Phase A: SIMD Extensions (CPU)
1. **512-bit vector widths**: Add support for `f32x16`, `i64x8`, etc., once AVX-512 or ARM SVE/SVE2 features are standardized as tier-1 targets.
2. **Explicit Alignment Attributes**: Support `#[align(N)]` on struct fields containing SIMD types to prevent misalignment traps on architectures that require strict vector alignment.
3. **Advanced Intrinsics**: Add hardware-specific CPU features (e.g., cryptographic hash rounds, matrix multiplication acceleration) as they are requested by workloads.
