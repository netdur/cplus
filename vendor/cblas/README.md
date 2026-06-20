# vendor/cblas

Reference CBLAS bindings for C+ — the cross-platform **CPU** numerics
fallback from [docs/GPU.md](../../docs/GPU.md): the "no GPU available" path,
or when a problem is small enough that host↔device transfer would dominate.
Plain `extern fn` bindings to the standard CBLAS C interface; OpenBLAS (or
any CBLAS provider) does the math.

On Apple Silicon prefer [`vendor/accelerate`](../accelerate/) (Apple
hand-tunes it). Everywhere else, use this. The surface is identical — the
CBLAS C ABI is the same whoever ships it.

## Requirements

A CBLAS provider. `Cplus.toml` links `openblas` by default:

- **Debian/Ubuntu:** `apt install libopenblas-dev` (ships the CBLAS
  interface and registers as the system BLAS). It's on the default linker
  path, so no `search-paths` needed.
- **Other providers:** edit `[link] libs` — `["cblas", "blas"]` for Netlib
  reference BLAS, or your MKL config. If the library lives outside the
  default path, add `search-paths = ["/path/to/lib"]`.

## Surface

Naming matches `cblas.h` verbatim. `Order` (`RowMajor`/`ColMajor`) and
`Transpose` (`NoTrans`/`Trans`/`ConjTrans`) are typed enum wrappers over the
CBLAS `int` constants.

| Level | Functions (f32 + f64) |
|---|---|
| 1 | `sdot`/`ddot`, `saxpy`/`daxpy`, `sscal`/`dscal`, `snrm2`/`dnrm2`, `sasum`/`dasum` |
| 2 | `sgemv`/`dgemv` |
| 3 | `sgemm`/`dgemm` |

Pointer args are `*f32` / `*f64`; produce them from arrays via
`#addr_of(arr) as *f32`. Unlike raw cuBLAS, CBLAS takes an
`Order`, so you can work **row-major** directly.

## Example — `y = A·x`

```cplus
import "cblas/cblas" as blas;

// A = [[1,2],[3,4]] (row-major), x = [5,6]  ->  y = [17, 39]
let a: [f32; 4] = [1.0f32, 2.0f32, 3.0f32, 4.0f32];
let x: [f32; 2] = [5.0f32, 6.0f32];
var y: [f32; 2] = [0.0f32, 0.0f32];
blas::sgemv(
    blas::Order::RowMajor, blas::Transpose::NoTrans,
    2 as i32, 2 as i32,
    1.0f32, #addr_of(a) as *f32, 2 as i32,
    #addr_of(x) as *f32, 1 as i32,
    0.0f32, #addr_of(y) as *f32, 1 as i32);
// y == [17, 39]
```

## Tests

The bindings ship a `#[test]` suite that runs real BLAS calls on the CPU
(no special hardware). From this directory:

```sh
cpc test     # 9 passed (sdot/saxpy/sscal/snrm2/sasum/sgemv/sgemm)
```
