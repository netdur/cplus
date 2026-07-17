# simd

3D float math and integer lane helpers on top of the language SIMD builtins
(`f32x4`, `i8x16`, …).

```toml
[dependencies]
simd = "*"
```

```cplus
import "simd/vec3" as vec3;
import "simd/mat4x4" as mat4x4;

let a: vec3::Vec3 = vec3::Vec3::new(1.0f32, 0.0f32, 0.0f32);
let b: vec3::Vec3 = vec3::Vec3::new(0.0f32, 1.0f32, 0.0f32);
let c: vec3::Vec3 = a.cross(b);
```

| Module | Role |
|---|---|
| `simd/vec3` | `Vec3` — f32×3 in `f32x4` (lane 3 = 0) |
| `simd/vec4` | `Vec4` — full `f32x4` |
| `simd/mat4x4` | `Mat4x4` — column-major, 4×`Vec4` |
| `simd/lanes` | integer/unsigned lane newtypes (FFI-friendly) |
| `simd/integer` | widening i8 ops / `dot_i32` (quant kernels) |

## Docs

- [docs/tutorial.md](docs/tutorial.md) — fast path
- [docs/guide.md](docs/guide.md) — layouts, Vec3 invariant, modules
- [docs/ref.md](docs/ref.md) — API catalog

## Tests

Unit tests in the modules above; umbrella `src/simd.cplus` for discovery.

```
cd vendor/simd && cpc test
```
