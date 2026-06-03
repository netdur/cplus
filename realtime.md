# C+ Real-Time Roadmap

This plan makes C+ usable for real-time systems by turning its existing
systems-language strengths into enforceable contracts.

The near-term target is **soft real-time**: audio callbacks, game frame loops,
robotics control loops, market-data hot paths, and embedded-style firmware code
running on a normal OS. Hard real-time requires OS, scheduler, interrupt, and
hardware guarantees outside the language; C+ can support that later, but the
first milestone should be deterministic hot-path code on ordinary platforms.

## Status (v0.0.12, 2026-05-30)

| Step | State |
| :--- | :--- |
| `#[no_alloc]` (Phase 1, first pass) | ✅ shipped (v0.0.10) |
| `#[no_alloc]` string-interpolation detection (Phase 1 hardening) | ✅ shipped — E0901 on `"…${x}…"` in a marked fn |
| `#[no_alloc]`/`#[no_block]` method-dispatch enforcement (Phase 1 hole) | ✅ shipped (2026-05-30) — `recv.method()` inside a marked body must dispatch to a method carrying the same contract (else E0901/E0907); resolved precisely at typecheck time via a `(source-type, method)` contract map; `to_string()` rejected as an allocator. No false positives across vendor/rt + the demo + full suites; 4 e2e tests |
| Annotate atomic stdlib subset (Phase 2 slice) | ✅ `u64` atomics + `atomic_thread_fence` marked `#[no_alloc]`/`#[no_block]` |
| `#[no_block]` (Phase 3) | ✅ shipped — E0907 on blocking primitives (locks, waits, sleep, blocking I/O, sockets) |
| `#[realtime]` bundle (Phase 4) | ✅ shipped — `#[no_alloc]` + `#[no_block]` + `#[bounded_recursion]` |
| `vendor/rt` SPSC ring + fixed pool (Phase 5) | ✅ shipped — `SpscRingU64` (lock-free, 7 tests) + `FixedPoolU64` (free-list object pool, 6 tests); `push`/`pop`/`acquire`/`release` all `#[no_alloc]`/`#[no_block]` |
| `proves/realtime_audio` demo (First Demo) | ✅ shipped — `#[realtime]` audio callback + SPSC control channel; E0901/E0907 verified to fire in-context |
| Bounded stack / `#[max_stack(N)]` (Phase 4) | ✅ shipped — E0908 when the estimated frame (params + typed locals, all nested blocks) exceeds N; ABI-accurate sizes for primitives/pointers/arrays/structs/enums/SIMD |
| `[profile.realtime]` + `cpc check` tooling (Phase 8) | ✅ shipped — manifest `[profile.realtime]` (deny_alloc/deny_block/deny_unknown_extern/stack_limit) synthesizes contracts onto local functions (deps exempt); `cpc check` whole-project front-end gate (no codegen); JSON diagnostics via `--diagnostics=json` |
| Platform RT packages (Phase 7) | ✅ shipped — `vendor/rt_darwin`: `clock` (monotonic ns timestamps), `thread` (QoS priority → `Result`), `mem` (`mlock`/`munlock` → `Result`); 8 tests; demo configures audio QoS + records per-frame latency. `rt_linux`/`rt_posix` mirror it later. |
| Tighten `Send`/`Sync` (Phase 6) | 🟡 core shipped — `Rc[T]` is `!Send`+`!Sync`, `MutexGuard[T]` is `!Send` (E0502 at `Send`/`Sync`-bounded sites incl. `thread::spawn`); `Arc[T]` stays `Send`+`Sync`. Broad "raw-ptr structs `!Send`" rule deferred — needs an `unsafe impl Send` opt-in that doesn't exist yet |

**Phase 1 — method dispatch now enforced (2026-05-30).** `recv.method()` calls
inside a `#[no_alloc]`/`#[no_block]`/`#[realtime]` body are now checked: the
dispatched method must itself carry the same contract, else E0901/E0907.
Resolved precisely at typecheck time in `check_method_call` (the receiver type
is known there) against a `(source-type, method) → (no_alloc, no_block)` map
built from every impl's actual attributes — keyed on the source type via
`generic_origin` so an instantiation (`Vec[i32]`) maps to its template
(`Vec`), and a dependency method's verdict stays correct regardless of any
local `[profile.realtime]` injection. Blessed `to_string()` (which allocates)
is rejected at its own site. The common sources (libc allocators, string
interpolation, `Vec`/`String`/`Box` constructors as free/assoc fns) were
already caught by the post-pass; this closes the allocation-through-a-method
hole that was the one open *soundness* gap.

**Phase 1 remaining gap (documented, deferred):** **compiler-inserted drop
glue** — a `Drop` destructor that itself allocates, run implicitly at scope
exit of a marked body. Narrow in practice (destructors free, they don't
allocate) and closing it cleanly needs ownership analysis to know which values
drop in the body. Not yet enforced.

**Note on inline assembly:** raw inline `#asm` (if added — see `plan.asm.md`)
must be treated as an unknown callee by these passes — rejected inside
`#[no_alloc]`/`#[no_block]` functions unless explicitly annotated safe, exactly
like an unknown extern. Inline asm does not otherwise advance this roadmap.

## Current Baseline

C+ already has several good real-time properties:

- No garbage collector.
- No exceptions or unwinding story.
- Explicit ownership, moves, drops, and borrow checking.
- Monomorphized generics and predictable codegen.
- C ABI, raw pointers, `unsafe`, and low-level FFI.
- Atomics, atomic fences, `#cpu_relax()`, OS threads, mutexes, channels, async,
  and reactor primitives in `vendor/stdlib`.
- `#[no_alloc]` exists as a sema-level allocation contract.
- `#[bounded_recursion]` exists as a sema-level recursion-cycle check.
- `vendor/static-arena` exists as a fixed-size no-heap arena pattern.

This means C+ is already capable of real-time-shaped code. The missing part is
making the contract strict enough that users can prove a hot path does not
allocate, block, recurse unexpectedly, or depend on unbounded runtime behavior.

## Definition

A C+ real-time function should be able to promise:

```cplus
#[realtime]
fn process_frame(input: f32x4[], output: f32x4[]) {
    // no heap allocation
    // no blocking syscall or lock wait
    // no unbounded recursion
    // no unbounded container growth
    // no unknown calls
}
```

`#[realtime]` should start as sugar for a bundle of smaller checks:

- `#[no_alloc]`
- `#[no_block]`
- `#[bounded_recursion]`
- bounded stack usage check
- conservative call-graph closure check

The smaller attributes should remain available independently because many
systems code paths need only one part of the contract.

## Phase 1: Harden `#[no_alloc]`

The current `#[no_alloc]` implementation is useful but too permissive for a
real contract. It walks direct AST call names and intentionally skips method
dispatch / unresolved calls. That is acceptable for a first pass, but not for
real-time claims.

Required work:

1. ✅ Build the `#[no_alloc]` check from sema-resolved call information, not only
   textual AST call names.
2. ✅ Resolve method calls (2026-05-30), associated calls, generic
   instantiations, and imported package functions. Method dispatch is checked
   at typecheck time against a `(source-type, method)` contract map.
3. ✅ Reject unknown callees inside `#[no_alloc]` functions. Unknown extern must
   mean "not proven", not "allowed".
4. ⬜ Include compiler-inserted work in the contract: string/interpolation
   helpers ✅ (interpolation rejected), **drop glue still deferred**.
5. ✅ Track `#[link_name]` externs by effective C symbol.
6. ✅ Keep the explicit extern escape hatch:

```cplus
#[no_alloc]
extern fn sinf(x: f32) -> f32;
```

Exit criteria:

- Direct `malloc`, `calloc`, `realloc`, `aligned_alloc`, and `free` calls reject.
- Calls through `#[link_name = "malloc"]` reject.
- User functions called from `#[no_alloc]` must also be `#[no_alloc]`.
- Method calls in `#[no_alloc]` bodies are checked.
- Generic functions are checked per concrete instantiation.
- Unknown externs reject unless explicitly marked `#[no_alloc]`.

## Phase 2: Annotate The Real-Time Stdlib Surface

`#[no_alloc]` becomes useful only when normal library helpers carry the marker.

Annotate proven no-allocation APIs:

- `stdlib/option`
- `stdlib/result`
- `stdlib/atomic`
- low-level SIMD helpers
- selected `stdlib/io` raw write helpers
- `vendor/static-arena`
- selected math / memory extern declarations

Do not mark:

- `Vec`, `HashMap`, `Box`, `Rc`, `Arc`, `string` constructors
- `Channel` send paths that can grow
- reactor initialization paths
- async helpers that allocate coroutine/reactor state
- thread spawn helpers that allocate worker context

Exit criteria:

- A realistic no-allocation DSP or control-loop example compiles with
  `#[no_alloc]`.
- The same example fails when it accidentally constructs a `Vec`, `string`,
  `Box`, `Arc`, or `HashMap`.

## Phase 3: Add `#[no_block]`

Allocation is only one real-time hazard. Blocking is just as important.

Add:

```cplus
#[no_block]
fn process_audio(...) { ... }
```

Rejected operations:

- `pthread_mutex_lock`
- `pthread_cond_wait`
- `pthread_join`
- blocking file I/O
- blocking socket I/O
- sleep / timer wait APIs
- reactor `poll_one_event` / blocking waits
- unbounded channel `recv`
- unknown extern calls unless marked `#[no_block]`

Allowed operations:

- plain arithmetic
- stack memory
- atomics
- `atomic_thread_fence`
- `#cpu_relax`
- nonblocking try-style APIs
- known nonblocking math / memory functions

Exit criteria:

- A `#[no_block]` function cannot call mutex lock, condvar wait, join, sleep, or
  blocking I/O.
- Extern functions are rejected unless known or explicitly marked.
- `#[no_block]` composes transitively like `#[no_alloc]`.

## Phase 4: Add Bounded Stack Checks

`#[bounded_recursion]` prevents recursive cycles, but real-time code also needs
bounded stack usage.

Add either:

```cplus
#[max_stack(4096)]
fn callback(...) { ... }
```

or a reporting command:

```sh
cpc check --stack-report src/main.cplus
```

The checker should estimate:

- local variable storage
- fixed-size arrays
- by-value aggregate temporaries
- call-chain worst case
- coroutine frames if async is allowed in a checked function

Exit criteria:

- Large local arrays and large by-value temporaries are visible in diagnostics.
- Recursive functions under a bounded-stack contract reject unless explicitly
  bounded in a future design.
- `vendor/static-arena` documents safe stack sizes using actual compiler output,
  not comments alone.

## Phase 5: Add Bounded Real-Time Data Structures

Current `Channel[T]` is unbounded and can grow with `realloc`, so it is not
real-time-safe.

Add package-level data structures:

- `vendor/rt/src/spsc_ring.cplus`
- `vendor/rt/src/mpmc_ring.cplus` later, only if needed
- fixed-capacity queue
- fixed-capacity pool
- fixed-capacity slab
- static arena wrappers

Start with SPSC because it is simple, useful, and maps directly to atomics:

```cplus
pub struct SpscRing[T] {
    buf: *T,
    cap: usize,
    head: u64,
    tail: u64,
}
```

The first implementation can use fixed concrete sizes if const generics are not
ready.

Exit criteria:

- `push` and `pop` are `#[no_alloc]` and `#[no_block]`.
- Full and empty are represented as `Option` / enum results, not blocking.
- Tests cover wraparound, full queue, empty queue, and producer/consumer
  ordering.

## Phase 6: Tighten `Send` And `Sync`

`Send` and `Sync` exist today as marker vocabulary, but the baseline is
permissive. Real concurrent systems need stronger checks.

**Status: core shipped (v0.0.12).** `is_send`/`is_sync` now recognize the
stdlib marker types structurally (by the instantiated struct's generic-template
name): `Rc[T]` is `!Send` and `!Sync`, `MutexGuard[T]` is `!Send`. A `Send`- or
`Sync`-bounded generic site — most importantly `thread::spawn` /
`thread::spawn_with` — rejects them with E0502; `Arc[T]` remains `Send`+`Sync`.
Verified against the real stdlib types (spawn-with-`Arc` compiles, spawn-with-`Rc`
errors). Implemented + remaining:

- `Rc[T]: !Send`  ✅
- `MutexGuard[T]: !Send`  ✅
- `Arc[T]: Send + Sync`  ✅ (stays the threadsafe sibling)
- raw-pointer-containing structs default to `!Send`  ⬜ **deferred** — needs an
  `unsafe impl Send for T {}` opt-in (not yet a language feature); enabling it
  without an escape hatch would reject most FFI code (ObjC bindings, channel,
  mutex) with no recourse.
- structural propagation (a struct *holding* an `Rc` is `!Send`)  ⬜ follow-up.
- future `Cell[T]` / `RefCell[T]` as `!Sync`  ⬜ when those land.

Exit criteria:

- `thread::spawn` rejects non-`Send` inputs and outputs.
- moving `Rc[T]` across threads rejects.
- moving `MutexGuard[T]` across threads rejects.
- structs with raw pointer fields reject unless explicitly opted in.

## Phase 7: Platform Real-Time Packages

**Status: started (v0.0.12) — `vendor/rt_darwin` shipped.** Darwin subset:
`clock` (monotonic high-resolution `now_monotonic_ns`/`elapsed_ns` via
`clock_gettime(CLOCK_MONOTONIC=6)`), `thread` (`set_current_priority` over the
Darwin QoS API, returning `Result`), and `mem` (`lock_pages`/`unlock_pages` via
`mlock`/`munlock`, returning `Result`). 8 unit tests; `proves/realtime_audio`
raises its thread to audio QoS before the hot loop and records per-frame latency
with the monotonic clock. Package name is `rt_darwin` (not `rt-darwin` — dep
names must match `[a-z][a-z0-9_]*`). `rt_linux`/`rt_posix` mirror this surface
with their own constants/syscalls when needed.

Keep OS-specific real-time controls in packages, not core syntax.

Add packages such as:

- `vendor/rt-posix`
- `vendor/rt-darwin`
- `vendor/rt-linux`

APIs to expose:

- thread priority
- CPU affinity
- scheduler policy
- page locking / prefaulting
- monotonic clock
- high-resolution timestamps
- nonblocking file/socket flags
- deadline/timer helpers

Example shape:

```cplus
import "rt-posix/thread" as rt_thread;

fn main() -> i32 {
    rt_thread::set_current_priority(rt_thread::Priority::RealtimeAudio);
    return 0;
}
```

Exit criteria:

- A real-time demo can configure the host thread before entering the hot path.
- Platform-specific failures return explicit `Result`, never exceptions.
- The hot-path callback remains package-independent and checked by attributes.

## Phase 8: Real-Time Profiles And Tooling

**Status: shipped (v0.0.12).** A project opts in globally via its `Cplus.toml`:

```toml
[profile.realtime]
deny_alloc = true
deny_block = true
deny_unknown_extern = true
stack_limit = 4096
```

When present, the build/check driver synthesizes the matching contract
attributes (`#[no_alloc]`, `#[no_block]`, `#[max_stack(stack_limit)]`) onto every
function defined in *this* package — dependencies are exempt (a file is "local"
iff its canonical path is under the project root but not under `root/vendor`).
The existing sema passes then do the enforcement, so the profile reuses the same
E0901 / E0907 / E0908 diagnostics with no special-casing. `deny_unknown_extern`
is subsumed by `deny_alloc`/`deny_block` (both already reject unknown externs).

`cpc check` (no FILE) runs the whole-project front-end (lex → parse → sema →
borrowck, including the profile gate) and stops before codegen — the fast CI
gate. `--diagnostics=json` emits one machine-readable diagnostic object per line
for editor/CI tooling. `cpc --realtime-report[=json]` (v0.0.13) runs the same
analysis but prints a **digest** — the active profile, functions-under-contract
count, and every E0901/E0906/E0907/E0908 violation grouped by contract — exiting
non-zero on any violation (CI gate + artifact). Remaining for a later pass:
`Send`/`Sync` (Phase 6) violation kinds in the report.

Compiler commands:

```sh
cpc check --profile realtime src/main.cplus
cpc check --realtime-report src/main.cplus
```

Report:

- allocation violations
- blocking violations
- unknown calls
- stack estimate
- non-`Send` cross-thread transfers
- unbounded data structure use

Exit criteria:

- CI can enforce a real-time profile.
- Diagnostics identify the exact call chain that violates the profile.
- JSON diagnostics expose machine-readable violation kinds for editor tooling.

## First Demo

The first proof should be an audio-style callback, because it is familiar and
strict enough to expose real problems.

Project:

```text
proves/realtime_audio/
```

Shape:

- fixed input/output buffers
- no heap allocation in callback
- no blocking in callback
- static arena or stack scratch space only
- atomics for parameter updates
- SPSC queue for UI/control messages
- benchmark that runs the callback in a tight loop and records max latency

Success condition:

- `process_frame` compiles with `#[realtime]`.
- Adding a `Vec::new`, string interpolation, mutex lock, channel receive, sleep,
  or unknown extern call causes a compile error.

## Recommended Order

1. Harden `#[no_alloc]`.
2. Annotate the no-allocation stdlib subset.
3. Add `#[no_block]`.
4. Add `#[realtime]` as a bundle.
5. Build `vendor/rt` with SPSC ring and fixed pools.
6. Add stack reports / `#[max_stack]`.
7. Tighten `Send` / `Sync`.
8. Add platform real-time packages.
9. Ship `proves/realtime_audio`.

This order turns C+ from "real-time capable by style" into "real-time checked by
the compiler" without bloating the core language.
