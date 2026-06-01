# v0.0.13 — open

Scope undecided. The candidate topics below are the real backlog drawn from
v0.0.12's deferred work, the design docs (`plan.own.md`, `plan.asm.md`,
`plan.jni.md`), and the llama.cplus port's open gaps. Pick a theme; not all of
this lands in one version.

> v0.0.12 (shipped) is archived in [plan-0.0.12.md](plan-0.0.12.md): the
> real-time contract system (8 phases), the llama-port gap round
> (G-034/043/044/045), native `f16`, and the `vendor/jni` adoption.

## Candidate topics

### A. Ownership & drop model completeness
The largest *designed-but-deferred* arc; `plan.own.md` already specs it.
- **Auto field-drop + the `own` marker** ([plan.own.md](plan.own.md)) — recurse
  drop into owning C+ fields (`string`/`Vec`/`Box`/Drop structs); `own ptr: *u8`
  declares a raw resource (→ **W0003** if no releasing `drop`); unmarked raw
  fields stay silent. Closes the silent-leak footgun. *Global drop-semantics
  change — land at a port-milestone boundary, gated by the E0509 migration audit
  in the doc.*
- **`unsafe impl Trait for T {}`** — the opt-out mechanism marker traits need.
  Unblocks the broad "raw-pointer structs are `!Send`" rule (the last open
  Send/Sync piece) without breaking ObjC/channel/mutex FFI.

### B. FFI & literal polish (small, high-leverage, low-risk)
- **`c"..."` C-string literals** ([plan.jni.md](plan.jni.md)) — null-terminated
  string literals so FFI stops needing the `"...\0"` workaround (JNI, Cocoa,
  libc). `str` is a fat pointer; `c"..."` would be a bare `*u8` to a
  NUL-terminated `.rodata` blob.
- **`f16` literal suffix** (`1.5f16`) — deferred polish from G-045; today needs
  `1.5 as f16`.
- **Struct-literal statics** (`static S: T = T { ... };`) — the remaining half of
  G-043 (array-literal statics shipped; struct/aggregate literals still rejected).
  The ggml `static const sphere_t scene[10] = {...}` pattern.
- **Const-eval for array lengths** — `[EXPR; N]` / `[T; N]` still need `N` a
  literal; a small const-evaluator would admit `[T; SOME_CONST]`.

### C. Real-time tail (additive; the roadmap's wrapped, these are the long tail)
- **`rt_linux` / `rt_posix`** siblings of `vendor/rt_darwin` (CLOCK_MONOTONIC=1,
  `sched_setaffinity`, `pthread_setschedparam`).
- **`--realtime-report`** — the machine-readable summary view deferred from
  Phase 8 (`cpc check` already gates; this aggregates violations).
- **`#[no_alloc]` drop-glue** — reject a `Drop` destructor that allocates, run
  implicitly at scope exit (needs ownership analysis; pairs with topic A).

### D. Performance
- **Cross-function inlining / `#[inline]`** (llama.cplus G-041) — cpc only
  auto-inlines trivial getters; a kernel built from `vendor/simd` Tier-2 calls
  keeps them as `bl`. Watch for the Q4_K CPU hot path; fix = run LLVM's inliner
  at `--release` or honor `#[inline]`.

### E. Dogfood — continue the llama.cplus port
The port is the engine that surfaced every gap this cycle. `f16` just unblocked
pure-C+ fp16↔fp32 (the "zero-`.c`" milestone); next is removing the remaining
`cplus-shim` bridges and widening CPU-kernel coverage. Let the port lead and
file gaps as it hits them, pulling ready items (A/B) as needed.

## Recommendation

Two coherent shapes:
- **"Finish the ownership model"** (A): land auto-field-drop + `own` +
  `unsafe impl Send`. Highest conceptual payoff, but a global semantics change —
  do it deliberately at a port-milestone boundary.
- **"FFI polish + keep the port moving"** (B + E): `c"..."` and struct-literal
  statics are small, low-risk, and directly remove port friction; let the port
  drive the rest. Lower risk, faster feedback.

Lead with B+E unless a milestone boundary opens the window for A.
