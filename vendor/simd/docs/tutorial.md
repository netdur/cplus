# Tutorial

Quick path: Vec3, Mat4x4, integer lanes. Gotchas in [guide.md](guide.md);
signatures in [ref.md](ref.md).

## Setup

```toml
[dependencies]
simd = "*"
```

```cplus
import "simd/vec3" as vec3;
import "simd/vec4" as vec4;
import "simd/mat4x4" as mat4x4;
import "stdlib/option" as option;
```

## Vectors

```cplus
let a: vec3::Vec3 = vec3::Vec3::new(1.0f32, 2.0f32, 3.0f32);
let b: vec3::Vec3 = vec3::Vec3::splat(0.5f32);

let s: vec3::Vec3 = a.add(b).scale(2.0f32);
let d: f32 = a.dot(b);
let n: vec3::Vec3 = a.cross(b);

match a.normalize() {
    option::Option[vec3::Vec3]::Some(u) => { /* unit */ }
    option::Option[vec3::Vec3]::None => { /* zero length */ }
}

let mid: vec3::Vec3 = a.lerp(b, at: 0.5f32);
```

`Vec4` is the same idea with `w` and no padding invariant:

```cplus
let v: vec4::Vec4 = vec4::Vec4::new(1.0f32, 2.0f32, 3.0f32, 1.0f32);
```

## Matrices

Column-major; `mul_vec` is M·v:

```cplus
let m: mat4x4::Mat4x4 = mat4x4::Mat4x4::identity();
let p: vec4::Vec4 = m.mul_vec(v);
let m2: mat4x4::Mat4x4 = m.mul(m);
```

## Day-one rules

- Import **modules** (`simd/vec3`), not a single barrel (except tests).
- `Vec3` keeps **lane 3 = 0** — use `Vec3::new` / ops; be careful with
  `from_raw` if you invent an `f32x4` yourself.
- `normalize` / `refract` return **`None`** on undefined cases (zero length,
  TIR), not a zero vector.
- Graphics convention: **column-major** `Mat4x4`.
