# C+ vs C — Sanity Benchmark

Goal: confirm that `cpc --release` produces machine code at parity with `clang -O2` on representative non-aliasing workloads, and that the codegen pipeline isn't doing anything stupid.

This is **not** the headline "C+ beats C" comparison. That story rests on the `noalias` parameter attribute the borrow checker hands us — but `noalias` lands in Phase 6 (along with `&T`/`&mut T` references, which let us write the pointer-aliasing workloads where the win would actually show). For now we're just checking that we haven't pessimized anything.

## What the sanity check found

The first run came in **213× slower** than clang on the arithmetic loop. Cause: `cpc --release` invoked clang without an `-O` flag, so clang defaulted to `-O0` and the LLVM optimizer pipeline never ran on our IR. Fix: pass `-O2` to clang in release mode, `-O0` in debug. After the fix, all three benchmarks are at parity.

## Results

Best of 5 runs, wall-clock seconds, Apple Silicon (Darwin 25.4).

| benchmark | `cpc --release` | `clang -O2` | ratio |
|---|---|---|---|
| sum — 1B-iter integer accumulation | 0.003 s | 0.003 s | **1.00×** |
| fib(40) — recursive Fibonacci, ~204M calls | 0.310 s | 0.312 s | **0.99×** |
| arr — 100M bounds-checked array reads | 0.052 s | 0.052 s | **1.00×** |

Compiler invocations:
- C+: `cpc --release file.cplus -o file` → internally `clang -O2 ...`
- C: `clang -O2 file.c -o file`

## How to reproduce

```sh
cargo build --release --bin cpc
bash bench/run.sh
```

[bench/run.sh](bench/run.sh) builds both binaries, sanity-checks that their outputs match, then takes the best of 5 wall-clock runs for each.

---

## Benchmark 1 — `sum`: tight integer loop

Sums `1..=1_000_000_000` with wrap arithmetic. No memory access beyond the accumulator. Tests whether `mem2reg` + loop reduction collapse our alloca/load/store pattern to a closed form the way they do for C.

**C+** — [bench/sum.cplus](bench/sum.cplus):

```cp
fn main() -> i32 {
    let mut sum: i64 = 0i64;
    let n: i64 = 1000000000i64;
    let mut i: i64 = 1i64;
    while i <= n {
        sum = sum +% i;
        i = i +% 1i64;
    }
    println(sum as i32);
    return 0;
}
```

**C** — [bench/sum.c](bench/sum.c):

```c
#include <stdio.h>
#include <stdint.h>

int main(void) {
    int64_t sum = 0;
    int64_t n = 1000000000;
    for (int64_t i = 1; i <= n; i++) {
        sum += i;
    }
    printf("%d\n", (int)sum);
    return 0;
}
```

Both compile to the closed form `n*(n+1)/2`, truncated to i32 → `-243309312`. The 0.003 s figure is essentially measurement noise.

---

## Benchmark 2 — `fib`: recursive Fibonacci

`fib(40)` makes ~204M function calls. Tests the calling convention and call-overhead path. LLVM cannot reduce this to a closed form (each call recurses on two new values), so we're actually doing the work.

**C+** — [bench/fib.cplus](bench/fib.cplus):

```cp
fn fib(n: i32) -> i32 {
    if n < 2 {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

fn main() -> i32 {
    let r: i32 = fib(40);
    println(r);
    return 0;
}
```

**C** — [bench/fib.c](bench/fib.c):

```c
#include <stdio.h>

int fib(int n) {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}

int main(void) {
    int r = fib(40);
    printf("%d\n", r);
    return 0;
}
```

Both produce `102334155`. Ratio: 0.99× — within noise. Confirms no per-call pessimization.

---

## Benchmark 3 — `arr`: bounds-checked array access

Reads from an 8-element array 100M times with a runtime-computed index. C+ inserts a runtime bounds check (`icmp uge` + branch + `llvm.trap`) on every indexing expression; C does not. LLVM should be able to prove `idx < 8` from `idx = x % 8` and elide the check — this benchmark sees whether it actually does.

**C+** — [bench/arr.cplus](bench/arr.cplus):

```cp
fn main() -> i32 {
    let a: [i32; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    let mut sum: i64 = 0i64;
    let mut iter: i32 = 0;
    while iter < 100000000 {
        let idx: usize = ((iter as i64) % 8i64) as usize;
        sum = sum +% (a[idx] as i64);
        iter = iter +% 1;
    }
    println(sum as i32);
    return 0;
}
```

**C** — [bench/arr.c](bench/arr.c):

```c
#include <stdio.h>
#include <stdint.h>

int main(void) {
    int32_t a[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    int64_t sum = 0;
    for (int32_t iter = 0; iter < 100000000; iter++) {
        size_t idx = (size_t)((int64_t)iter % 8);
        sum += (int64_t)a[idx];
    }
    printf("%d\n", (int)sum);
    return 0;
}
```

Both produce `450000000`. Ratio: 1.00× — LLVM elides the bounds check, as hoped.

---

## What this sanity check does *not* prove

- **Aliasing-heavy code.** The current benchmarks don't pass two writable pointers to the same function. The whole reason C+ might beat C is `noalias` on `&mut T` parameters; that needs Phase-6 work plus references to even express. The "C+ beats C on memcpy-style loops" benchmark comes when references land.
- **Large programs.** All three benchmarks fit on screen. Whole-program optimization, link-time effects, code-size impact — none of those are measured here.
- **Cold-start / I/O / allocation.** No malloc, no syscalls, no startup cost. The numbers are pure CPU loop performance.

## Pre-existing limitation surfaced

The `arr` benchmark uses an 8-element array because C+ does not yet have a `[expr; N]` fill-syntax for array literals. For a 1024-element array we'd have to write all 1024 elements by hand. Tracked separately; doesn't affect the benchmark's conclusion.

## Findings summary

- `cpc --release` now passes `-O2` to clang. Without that flag we were 100×+ slower than necessary; with it, parity.
- IR shape is fine — no codegen pattern is blocking LLVM's standard passes.
- Bounds checks don't hurt when LLVM can prove the index is in range, which it does for the common loop-over-fixed-array case.
- Calling-convention overhead is at parity; no per-call pessimization.

Re-run this benchmark after Phase 6 lands `noalias`. Add a memcpy-style two-pointer workload at that point — that's where the borrow checker is supposed to win.
