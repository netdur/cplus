# ray_tracer

One program, three languages. A Shirley "Ray Tracing in One Weekend" tracer
written in C+, C and Rust, transliterated line for line, so the only thing
the numbers compare is the language and its toolchain.

| | |
|---|---|
| [`cplus/`](cplus/src/main.cplus) | C+ — the reference the other two follow |
| [`c/`](c/src/main.c) | C11, libc + libm |
| [`rust/`](rust/src/main.rs) | Rust 1.93, std only, no crates |

## The workload

Hardcoded in all three; there are no flags to get wrong:

```
800x450, 32 samples/pixel, depth 15, 10 spheres, single-threaded
xorshift32 seeded 0x12345678, consumed in one sequential stream
```

Ten spheres tested linearly, with no acceleration structure and no threads —
deliberately. A BVH would make the tracer faster and the *comparison* worse,
because most of the runtime would move into one hand-tuned data structure
instead of into the language's ordinary code. One shared RNG in a fixed order
is what makes a byte-identical image possible at all, and also why this
cannot be threaded without changing the output.

## Results

Apple M1 Max, macOS 26. Build from clean, best of 3. Render is best of seven
wall-clock runs, interleaved so thermal drift hits every row equally. Peak
RSS via `/usr/bin/time -l`.

**Each port at its best setting:**

| port | render | build | binary | peak RSS |
|---|---:|---:|---:|---:|
| **C+** (cpc 0.0.27, `--release`) | **0.950 s** | 159 ms | 32.9 KB | 2.34 MB |
| **C** (Apple clang 21, `-O3`) | 0.990 s | 126 ms | 32.9 KB | 2.33 MB |
| **Rust** (rustc 1.93, `opt-level=3` + LTO) | 1.150 s | 2323 ms | 345.2 KB | 2.47 MB |

C+ is fastest, by 4% over C. Rust cannot reach that row at all, for a reason
that is about reproducibility rather than speed — see below.

**The catch: those three rows are not the same image.**

Fusing `a*b+c` into a single-rounding FMA is a codegen decision, and every
compiler makes it differently. Turn it off and all three agree byte for byte;
leave it on and each produces its own image:

| config | render | FMA instrs | image md5 |
|---|---:|---:|---|
| C+ `--fp-contract=on` (default) | **0.950 s** | 159 | `f642a7fa…` |
| C `-ffp-contract=on` (default) | 0.990 s | 30 | `8c515a11…` |
| C+ `--fp-contract=off` | 1.130 s | 0 | `7730fff3…` |
| Rust (no knob — never contracts) | 1.150 s | 0 | `7730fff3…` |
| C `-ffp-contract=off` | 1.170 s | 0 | `7730fff3…` |

So the honest like-for-like comparison is the bottom three, where the output
is identical: **C+ 1.130 s, Rust 1.150 s, C 1.170 s** — a 3% spread, which is
inside the run-to-run noise. On this program the three languages are the same
speed.

Two things that table settles:

- **Contraction costs about 19%**, in both C (1.18x) and C+ (1.19x). That is
  the price of a reproducible image, not a property of either language.
- **cpc contracts far more aggressively than clang** — 159 fused instructions
  against 30 — which is where its 4% lead at the default setting comes from.
  Rust has no way to ask for it, so it has one row instead of two.

Where the languages genuinely separate is not the render loop: **Rust's build
is 15x slower and its binary 10x larger** for a program with no dependencies.

## Building and running

Each port writes `out.ppm` in its own directory.

```sh
cd cplus && cpc build --release && ./target/release/cplus
cd c     && make                && ./build/rt
cd rust  && cargo build --release && ./target/release/rt
```

To reproduce the identical-image row, disable contraction — Rust needs
nothing:

```sh
cd cplus && cpc build --release --fp-contract=off
cd c     && make FPC=off
```

Then `md5 -q out.ppm` in each should give `7730fff3105ebbe75e7d00d1099aef85`.

## Notes for anyone porting a fourth language

- **Match the RNG exactly.** xorshift32, `x ^= x<<13; x ^= x>>17; x ^= x<<5`,
  and `randf` is `(next() >> 8) as f32 * (1.0/16777216.0)`. Draw order is
  load-bearing: `rand_in_unit_sphere` takes three draws per rejection attempt,
  x then y then z.
- **The ray direction is not normalized** in `sphere_hit`, so the quadratic's
  `a` term is carried through rather than assumed to be 1.
- **`Hit` and `Scattered` are returned by value** with a `valid` flag rather
  than through an out-pointer. SROA flattens them into registers; the shape is
  what keeps the three ports structurally comparable.
- **`ray_color` is recursive**, not an iterative bounce loop.
- The image is written bottom-up: row `j` lands at `(height-1-j)`.
