# Guide

How the package is layered on language SIMD types, and which module to
import. Fast start: [tutorial.md](tutorial.md). Catalog: [ref.md](ref.md).

## Two audiences

| Need | Modules |
|---|---|
| Games / graphics / 3D math | `vec3`, `vec4`, `mat4x4` |
| Integer SIMD / FFI / quant | `lanes`, `integer` |

All of it is **library-only** on top of compiler builtins (`f32x4`,
`i32x4`, `i8x16`, …). No extra native deps.

## Module map

| Path | Contents |
|---|---|
| `simd/vec3` | `Vec3` — geometry vector |
| `simd/vec4` | `Vec4` — homogeneous / 4-lane float |
| `simd/mat4x4` | `Mat4x4` — 4 columns of `Vec4` |
| `simd/lanes` | `Int32x4`, `UInt32x4`, `Int16x8`, … thin newtypes |
| `simd/integer` | free fns: `mull_*`, `mlal_*`, `paddl_i8`, `dot_i32` |
| `simd/simd` | test umbrella only |

## Vec3 lane-3 zero invariant

`Vec3` stores an `f32x4` with **w = 0** always:

- `dot` = full 4-lane mul-sum without masking (`w*w = 0`).
- `cross` stays correct for padded vectors.
- `min`/`max` on the padding lane stay well-behaved with zeros.

Construct with `new` / `splat` / `zero` / arithmetic / `cross`.  
`from_raw(v)` **trusts** you: if lane 3 is nonzero, dots and friends are wrong.

## Vec4

All four lanes live. Use for positions with `w`, colors, Mat4 columns.
No cross/reflect/refract (those are 3D ops on `Vec3`).

## Mat4x4 layout

**Column-major** (OpenGL / Metal style):

```
M * v = col0*v.x + col1*v.y + col2*v.z + col3*v.w
```

Each term is a scale of a `Vec4` (SIMD-friendly). No separate Mat3x3 —
use the upper 3×3 of a Mat4 when you need linear 3D.

`col(at:)` — documented columns; out-of-range behavior is implementation-
defined for bad indices (use 0..4).

## Option vs sentinel

| Op | Failure |
|---|---|
| `normalize` | `None` if length is 0 |
| `refract` | `None` on total internal reflection |
| lane `lane(at:)` | `None` if index OOB |

Prefer matching `None` over checking for a magic zero vector.

## Integer path

### `lanes`

Newtypes over builtins for safer FFI:

```cplus
let v: lanes::Int32x4 = lanes::Int32x4::from_raw(some_i32x4);
let r: i32x4 = v.add(other).raw();
```

`lane(at:)` is bounds-checked via `Option`. Arithmetic: `add` / `sub` /
`mul` / `min` / `max` (and `sum` where present).

### `integer`

Widening helpers so **i8 mul does not wrap in i8**:

- `mull_i8` / `mull_lo_i8` / `mull_hi_i8` → i16 products  
- `mlal_i8` — multiply-accumulate at i16  
- `paddl_i8` — pairwise widen-add  
- `dot_i32(a, b)` — 16-lane signed byte dot into **i32** (quant / NEON-style)

Use `dot_i32` instead of `i8x16.mul().sum()` when products can exceed i8.

## Gotchas

### Not operator overloads

Write `a.add(b)`, not `a + b` (language rule for this API surface).

### FMA / sqrt

Hot paths lower to LLVM vector ops (`fma`, `sqrt`) where the compiler can;
you still call package methods, not intrinsics by hand.

### Import the right module

`import "simd/vec3"` gives `Vec3`. There is no `import "simd"` for apps.

### Mixing Vec3 and raw f32x4

Prefer staying in `Vec3` until an FFI boundary, then `raw()` / `from_raw`
once.

## Typical patterns

**Direction and reflection**

```cplus
guard let option::Option[vec3::Vec3]::Some(n) = normal.normalize() else {
    return;
};
let r: vec3::Vec3 = incident.reflect(across: n);
```

**Transform a point**

```cplus
let p4: vec4::Vec4 = vec4::Vec4::new(p.x(), p.y(), p.z(), 1.0f32);
let world: vec4::Vec4 = model.mul_vec(p4);
```
