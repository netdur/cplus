# Reference

Manual for the `simd` package. Public signatures and behavior.

```cplus
import "simd/vec3" as vec3;
import "simd/vec4" as vec4;
import "simd/mat4x4" as mat4x4;
import "simd/lanes" as lanes;
import "simd/integer" as integer;
```

---

## `vec3` — `Vec3`

```cplus
struct Vec3 { _v: f32x4 }   // lane 3 always 0
```

### Construction / raw

```cplus
fn new(x: f32, y: f32, z: f32) -> Vec3
fn splat(s: f32) -> Vec3          // (s,s,s,0)
fn zero() -> Vec3
fn raw(this) -> f32x4
fn from_raw(v: f32x4) -> Vec3     // caller preserves w=0
```

### Lanes

```cplus
fn x(this) -> f32
fn y(this) -> f32
fn z(this) -> f32
```

### Arithmetic

```cplus
fn add(this, o: Vec3) -> Vec3
fn sub(this, o: Vec3) -> Vec3
fn mul(this, o: Vec3) -> Vec3     // per-lane
fn scale(this, s: f32) -> Vec3
fn neg(this) -> Vec3
```

### Geometry

```cplus
fn dot(this, o: Vec3) -> f32
fn cross(this, o: Vec3) -> Vec3
fn len2(this) -> f32
fn length(this) -> f32
fn normalize(this) -> option::Option[Vec3]   // None if length 0
fn reflect(this, across: Vec3) -> Vec3
fn refract(this, across: Vec3, eta: f32) -> option::Option[Vec3]
```

### Compare / blend

```cplus
fn min(this, o: Vec3) -> Vec3
fn max(this, o: Vec3) -> Vec3
fn clamp(this, min: Vec3, max: Vec3) -> Vec3
fn lerp(this, o: Vec3, at: f32) -> Vec3
```

---

## `vec4` — `Vec4`

```cplus
struct Vec4 { _v: f32x4 }
```

```cplus
fn new(x: f32, y: f32, z: f32, w: f32) -> Vec4
fn splat(s: f32) -> Vec4
fn zero() -> Vec4
fn raw(this) -> f32x4
fn from_raw(v: f32x4) -> Vec4
fn x/y/z/w(this) -> f32
fn add/sub/mul/scale/neg(...)
fn dot(this, o: Vec4) -> f32
fn len2(this) -> f32
fn length(this) -> f32
fn normalize(this) -> option::Option[Vec4]
fn min/max/clamp/lerp(...)
```

No `cross` / `reflect` / `refract`.

---

## `mat4x4` — `Mat4x4`

```cplus
struct Mat4x4 { _cols: [vec4::Vec4; 4] }   // column-major
```

```cplus
fn new(c0: Vec4, c1: Vec4, c2: Vec4, c3: Vec4) -> Mat4x4
fn zero() -> Mat4x4
fn identity() -> Mat4x4
fn col(this, at: u32) -> Vec4
fn mul_vec(this, v: Vec4) -> Vec4      // M * v
fn mul(this, o: Mat4x4) -> Mat4x4      // M * N
fn add(this, o: Mat4x4) -> Mat4x4
fn scale(this, s: f32) -> Mat4x4
```

---

## `lanes` — integer/unsigned newtypes

Each type: `splat` / `from_raw` / `raw` / `count` / `to_array` / `lane(at:) -> Option` /
`add` / `sub` / `mul` / `min` / `max` (plus `sum` on 32-bit types; `new` on 4-lane types).

| Type | Builtin | Lanes |
|---|---|---|
| `Int32x4` | `i32x4` | 4 |
| `UInt32x4` | `u32x4` | 4 |
| `Int16x8` | `i16x8` | 8 |
| `UInt16x8` | `u16x8` | 8 |
| `Int8x16` | `i8x16` | 16 |
| `UInt8x16` | `u8x16` | 16 |

`lane(at:)` returns `None` when `at >= count`.

---

## `integer` — free functions

```cplus
fn mull_i8(a: i8x8, b: i8x8) -> i16x8
fn mull_lo_i8(a: i8x16, b: i8x16) -> i16x8
fn mull_hi_i8(a: i8x16, b: i8x16) -> i16x8
fn mlal_i8(acc: i16x8, a: i8x8, b: i8x8) -> i16x8
fn paddl_i8(v: i8x16) -> i16x8
fn dot_i32(a: i8x16, b: i8x16) -> i32
```

Widening multiply / accumulate / pairwise add / full 16-lane signed-byte
dot into `i32` (no silent i8 product wrap).

---

## Package

| | |
|---|---|
| Package name | `simd` |
| Dependencies | `stdlib` (`option`) |
| Tests | `cpc test` (`src/simd.cplus` umbrella) |
| Builtins used | `f32x4`, `i8x8`/`i8x16`, `i16x8`, `i32x4`, … |
