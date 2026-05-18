# C+ — Plan

Version 0.0.4 shipped 2026-05-18 (Phase 4 MVP). See [plan-0.0.4.md](plan-0.0.4.md) for the archived 0.0.4 roadmap and resolved log; [plan-0.0.3.md](plan-0.0.3.md) covers v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.5 — Correctness, then ergonomics

**Strategy: correctness gaps first, then the small compiler unblockers, then features built on top.** The two double-free Drop bugs and the missing inner-T Drop on container drop are real footguns with workarounds; they're not "wait for a workload" issues. After they land, two small compiler slices (generic-method-body propagation + gen-methods) unlock the iterator ecosystem and the async method APIs. Async polish and platform parity round out the release; perf + polish close it.

Slice sizes use assistant-paced framing (S/M/L), not human-typing weeks. A "session" means one focused implementation pass with verification; a phase is "ship the phase when its exit criteria are green," not "schedule N weeks."

---

### Phase 1 — Drop correctness · size M

The Drop-machinery gaps that survived v0.0.3 and v0.0.4. Each is a real correctness hole with a known workaround; the workarounds shouldn't have to exist.

#### Slice 1A — Auto-promote non-Copy value param to `move` (or reject without it) · S

**Bug:** `fn echo(x: string) -> string { return x; }` double-frees at runtime. The caller's `x` flows in as a value (bitwise copy); `return x` extends the lifetime to the result; both ends register Drop. Workaround today: write `fn echo(move x: string)` and the caller-side `mark_moved` flips the source's drop flag.

**Fix:** in sema's `check_function`, for every non-Copy value-passed param without `move`, either (a) emit a hard error pointing at the param with a "use `move`" hint, or (b) auto-promote — treat the param as if `move` was written. (a) is the conservative shape; (b) is the ergonomic one. Probably ship (b) since the alternative is asking every user to type `move` 80% of the time.

**Where it bites:** any helper that takes a `string` / `Vec[T]` / `string` field of a non-Copy struct without thinking carefully about ownership.

#### Slice 1B — Re-bind Drop tracking · S

**Bug:** `let a: string = "x".to_string(); let b: string = a;` registers Drop on both `a` and `b` at scope exit → double-free. Pre-existing in v0.0.1; survived because re-bind isn't idiomatic in stdlib code.

**Fix:** when the right-hand-side of a `let` is an `Ident(x)` referring to a binding of non-Copy type, flip x's drop flag (same `mark_moved` machinery used by `move`-marked args). Sema-level Place tracking already exists for binding moves through method-call args; this just extends the same logic to the let RHS.

#### Slice 1C — Inner-T Drop on container Drop · M

**Bug:** `Box[T]`, `Arc[T]`, `Rc[T]`, `Mutex[T]`, `Channel[T]`, `Vec[T]`, and `HashMap[K, V]` all free their heap storage on Drop but **don't call `T::drop()`** on the contained value(s). With `T = string` or `T = Vec[U]`, every container leaks per-instance.

**Fix:** the container's `drop(mut self)` body needs to walk live entries and invoke `T::drop()` for each. Source-level workaround in v0.0.4: every container's `unwrap_*` consumer transfers ownership back to a let-binding, whose scope-exit Drop fires. That works for `Box::unwrap`-shape APIs but not for `HashMap::drop()` (you can't pull every entry out before dropping the map).

The principled fix needs Drop glue invocable from generic body context — sema's `check_function` resolves the concrete `T::drop` method only at monomorphization. Land via:
1. New compiler intrinsic `__cplus_drop_in_place::[T](p: *T)` — sema accepts the call shape; codegen lowers to `T::drop(*p)` for the monomorphized T (or to a no-op when T has no Drop).
2. Stdlib containers' `drop` methods walk their storage and call the intrinsic per element.

Affects every container; net stdlib change is ~6 small edits.

#### Slice 1D — ASan + async coroutine interaction · M

**Bug carried forward from Phase 1E:** scalar `i32` async fns under `--asan` return 0 instead of the expected value. Non-ASan path is correct. Probably ASan instrumentation of the alloca-promise + CoroSplit interaction.

**Fix:** investigation slice — repro under `lldb`, narrow to a minimal IR shape, check whether `coro.id`'s promise alloca needs an `noasan` attribute or whether CoroSplit needs different lifetime markers around the frame.

#### Phase 1 exit criteria

- [ ] `fn echo(x: string) -> string { return x; }` doesn't double-free
- [ ] `let b: string = a;` doesn't double-free at scope exit
- [ ] `HashMap[str, string]` doesn't leak entries on drop
- [ ] `async fn id(x: i32) -> i32 { return x; }` driven by `block_on` returns x under ASan

---

### Phase 2 — Compiler unblockers · size S

Three small slices that together unlock the v0.0.5 feature work. None ships a user-visible artifact directly; each closes a pre-existing limitation that was worked around in v0.0.4.

#### Slice 2A — Generic-method-body propagation · S

**Limitation:** Phase 1B's `propagate_fn_instantiations` walks generic-FREE-fn bodies but skips impl-method bodies. So `HashMap[K, V]::get` calling `result::io_err::[V](...)` never gets the propagated `(io_err, [i32])` entry → codegen panics looking up `io_err` (un-mangled). v0.0.4's HashMap worked around it by constructing `Result::Err(...)` directly.

**Fix:** extend the propagation loop to also walk every impl-method body for each `struct_instantiation`. For impl `Vec[T]`'s instantiation with `T = i32`, walk every method body, substitute `T → i32` through the recorded turbofish args, add the resolved `(callee, concrete_args)` to the worklist. ~30 lines in `monomorphize.rs`.

#### Slice 2B — Gen-methods · S

**Limitation:** v0.0.4 Slice 4A added `is_gen` to `Method` and the parser accepts `pub gen fn iter(self) -> T`, but sema's `check_method` doesn't thread `current_fn_is_gen` and codegen's `gen_method` doesn't dispatch to `gen_gen_function`. Writing a `gen` method silently fails at sema (E1001 fires on `yield`).

**Fix:** mirror `check_function`'s `is_gen` branch in `check_method` (set `current_fn_is_gen` + `current_gen_yield_ty` for the method's body). Mirror `gen_async_function`'s dispatch site in `gen_method` to route `is_gen` methods to a `gen_gen_method` (or reuse `gen_gen_function` with the receiver param threaded). ~50 lines across sema + codegen.

#### Slice 2C — `impl` on enum types · M

**Limitation:** v0.0.4 sema rejects `impl Foo` when `Foo` is an enum (E0325). Blocks `impl CowStr { fn as_str(self) -> str { ... } }`, `impl Option[T] { fn unwrap(move self) -> T { ... } }`, `impl Result[T, E] { fn map[U](self, f: fn(T) -> U) -> Result[U, E] }`. Every enum-shaped stdlib type ships free-fn API today.

**Fix:** lift the E0325 check to also accept `Ty::Enum`. The method-lookup table already keys on `Ty::Struct(id)` — generalize to `Ty::Struct | Ty::Enum`. Codegen's `gen_method_call` likewise. Pattern-matching on `self` inside an enum's impl methods works via the existing match shape.

#### Phase 2 exit criteria

- [ ] `HashMap[K, V]::get` can call `result::io_err::[V](...)` directly without inlining
- [ ] `pub gen fn iter(self) -> T` parses, sema-checks, and codegens correctly
- [ ] `impl Option[T] { pub fn unwrap(move self) -> T { ... } }` compiles and dispatches

---

### Phase 3 — Iterator ecosystem · size M

Builds on Phase 2 to close the "you can have a generator but you can't iterate over your stdlib types" gap. The headline deliverable is `vec.iter().filter(pred).map(f).collect_to_vec()` reads and runs.

#### Slice 3A — `Vec[T]::iter()` · S

```cplus
impl Vec[T] {
    pub gen fn iter(self) -> T {
        let mut i: usize = 0;
        while i < self.len {
            yield self.get(i);
            i = i +% (1 as usize);
        }
        return;
    }
}
```

Verifies that gen-methods (Phase 2B) work on a real type. `for x in v.iter() { ... }` and explicit `v.iter().next()` both work after this.

#### Slice 3B — Tuple types + `HashMap::iter()` · M

`HashMap[K, V]::iter()` needs to yield `(K, V)` pairs — which requires tuple types as values. v0.0.4 doesn't have `(a, b)` as a type. Land as a small slice:
1. Sema: `(T1, T2)` is a synthetic struct with fields `.0`, `.1`. Parse `(a, b)` as a tuple constructor; `.0`/`.1` access as field projection.
2. Codegen: lower as `{T1, T2}` LLVM struct.
3. `HashMap::iter()` yields `(K, V)`; `for (k, v) in m.iter() { ... }` works via destructuring.

If tuple types feel too big for one slice, ship `HashMap::keys()` + `HashMap::values()` first (each yields a single primitive), then revisit `iter()` for v0.0.6.

#### Slice 3C — Iterator adapters · M

`Iterator[T]::filter(self, pred: fn(T) -> bool) -> Iterator[T]`, `::map[U](self, f: fn(T) -> U) -> Iterator[U]`, `::take(self, n: usize) -> Iterator[T]`, `::collect_to_vec(move self) -> Vec[T]`. Each is a gen-method on `Iterator[T]` that pulls from `self.next()`.

**Compiler need:** `fn` pointer arguments to a gen-method work today (Phase 11 fn-pointer slice), but the call site `vec.iter().filter(is_even)` requires the method receiver `self: Iterator[T]` plus an `fn(T) -> bool` param to monomorphize cleanly. Verify nothing else surfaces.

#### Slice 3D — `File::lines()` · S

```cplus
impl File {
    pub gen fn lines(self) -> string {
        // read self.fd buffered; yield each newline-terminated chunk.
    }
}
```

Common-enough use case (`for line in f.lines() { ... }`) that it's worth shipping alongside the adapters.

#### Slice 3E — Borrow check across yield (dataflow tightening) · M

v0.0.4 ships permissive — gen fns can have `str` / `T[]` params. That's safe for the typical case (next() caller's frame outlives iteration) but unsafe for nested generators where one gen fn's yielded value borrows into another's frame. Tighten with a dataflow rule: parameters with borrow-shaped types may not be live across a yield-into-nested-gen-fn boundary.

**Forward-pointable** if no real workload surfaces the gap.

#### Phase 3 exit criteria

- [ ] `for x in v.iter() { ... }` works on `Vec[i32]`
- [ ] `v.iter().filter(is_pos).map(double).collect_to_vec()` works
- [ ] `for (k, v) in m.iter() { ... }` works (or `for k in m.keys()` as the smaller cut)
- [ ] `for line in f.lines() { ... }` works
- [ ] At least 3 stdlib types expose iterator-style API

---

### Phase 4 — Async polish · size M

Closes the v0.0.4 Track A wrappers and unblocks a real-world async demo.

#### Slice 4A — `sleep(ms)` via `EVFILT_TIMER` · S

```cplus
pub async fn sleep(ms: u64) {
    unsafe { __cplus_reactor_wait_timer(ms); }
    return;
}
```

New compiler intrinsic mirrors `__cplus_reactor_wait_read`: registers a timer with kqueue (EVFILT_TIMER), suspends, reactor wakes when timer fires. epoll port adds `timerfd_create`.

#### Slice 4B — `TcpStream::read_async` / `write_async` / `accept_async` method form · S

Phase 1A's `move`-on-value-pass fix lets `self.read_async(buf, n)` work without consuming the stream forever. Method wrappers shed the free-fn-on-`fd` shape that v0.0.4 ships.

#### Slice 4C — `File::read_async` · S

Same shape as `read_fd_async` — set O_NONBLOCK on the open fd, loop try-read / EAGAIN / `wait_read`. Mechanical.

#### Slice 4D — Hand-rolled `Future` implementations · M

v0.0.4 forward-pointed this on "needs dyn-dispatch design." The pragmatic answer: **monomorphize**. `block_on::[F: Future]` synthesizes one drive loop per concrete F. No `dyn Future`, no trait objects — same model the rest of the language uses.

Land `interface Future[T] { fn poll(mut self) -> Poll[T]; }`, accept user `impl Future for MyTimer`, generate `block_on::[MyTimer]` monomorph on demand. Compiler-coroutine futures (from `async fn`) get a synthetic `impl Future for Future__T` so the same path drives both.

#### Slice 4E — `async_fetch` real-TCP recipe + 1000-task stress · S

Build on the wrappers (Phase 3 of v0.0.4 substrate + Phase 4A-C above). `docs/examples/recipes/async_fetch/` opens a TCP connection, sends an HTTP GET, reads the response with `read_async`. The stress variant spawns 1000 such tasks via `spawn_local` and asserts the wall time is ~1× the slowest task, not Σ.

#### Phase 4 exit criteria

- [ ] `sleep(100).await` actually sleeps ~100ms in `block_on`
- [ ] `stream.read_async(buf, n).await` works (method form)
- [ ] `impl Future for MyTimer { ... }` compiles and runs through `block_on`
- [ ] 1000 concurrent `async_fetch(url)` tasks complete in ~1× wall-clock-of-slowest

---

### Phase 5 — Platform parity · size L

The big "Linux works" lift. macOS arm64 is fully validated; Linux x86_64 has stdlib + reactor + threading code that's never been verified there.

#### Slice 5A — Linux x86_64 stdlib ABI verification · M

- Run the full test suite under Linux x86_64. Expected breakage: variadic ABI quirks (`fcntl` was already fixed for AArch64), pthread struct layout (already padded for portability in Mutex/Channel), errno location (`__errno_location()` vs `__error()`).
- Fix per-platform constants in `net::eagain_*` and add the missing extern decls.
- Verify thread spawn/join, atomics, async runtime all green.

#### Slice 5B — pthread `[link]` entry · S

Stdlib's `Cplus.toml` needs `[link] libs = ["pthread"]` for Linux. macOS gets it free via libSystem. Add the manifest field; codegen passes `-lpthread` to clang on Linux targets.

#### Slice 5C — aarch64-Linux smoke test · S

HFA optimization differs from darwin-aarch64. Run a stdlib smoke test on aarch64-Linux to catch any places where Phase 1F's recursive mangler or the thread trampoline got the wrong calling convention.

#### Slice 5D — epoll variant of reactor · M

Mirror `vendor/stdlib/src/reactor.cplus`'s kqueue implementation for Linux:
- `epoll_create1`, `epoll_ctl(EPOLL_CTL_ADD, EPOLLIN | EPOLLONESHOT)`, `epoll_wait`.
- Same `(fd, filter, hdl)` waiter table shape; same `poll_one_event` semantics.
- Compiler intrinsics (`__cplus_reactor_wait_read` etc.) stay platform-agnostic; the stdlib internals branch on `cfg(target_os = "linux")`.

#### Phase 5 exit criteria

- [ ] Full test suite passes on Linux x86_64
- [ ] Full test suite passes on Linux aarch64 (smoke at minimum)
- [ ] `async_fetch` recipe works under epoll on Linux

---

### Phase 6 — Perf + polish · size M

The leftover bucket — closing the language-polish carryovers and the raytracer codegen gap.

#### Slice 6A — Raytracer perf investigation · M

C+'s raytracer is ~30% slower than C / Rust on the same algorithm with the same RNG seed. Same machine code in the hot loop per `--emit-asm`; the gap is elsewhere. Plausible items:
- `-ffp-contract=on` (FMA fusion) defaults — C+ emits without FMA hints
- Missing `noundef` / `noalias` / `nofree` attributes on hot-path params
- Autovec heuristics — LLVM's loop vectorizer may skip without the right metadata
- Profile-guided inlining hints

Audit slice. Each plausible item gets benchmarked in isolation.

#### Slice 6B — Slice indexing `s[i]` with bounds-check · S

`let b: u8 = s[i];` parses today as a function call; rewrite as a real indexing operator on slice types. Bounds-check via `llvm.assume` so `-O2` elides the check after a prior bound proves it.

#### Slice 6C — Array → slice coercion (`arr as T[]`) · S

`let a: [u8; 16] = ...; let s: u8[] = a as u8[];` — converts a stack array to a slice (fat pointer with len from the array type). Today users construct slices via `slice_from_raw_parts` manually.

#### Slice 6D — String interpolation polish · S

Format specifiers: `"{x:5}"` / `"{x:.2}"` / `"{x:08x}"`. v0.0.4 ships bare interpolation; format specifiers are deferred.

#### Slice 6E — `while let` statement + multi-binding `guard let` patterns · S

Both inherited from v0.0.1 carry-forward. `while let Some(x) = it.next() { ... }` desugars to the existing `for x in it` shape; multi-binding `guard let Pattern1 | Pattern2 = expr` matches either alternative.

#### Slice 6F — Negative trait impls (`Rc[T]: !Send`) · M

Phase 2A in v0.0.4 shipped permissive Send/Sync. Negative impls let stdlib mark `Rc[T]: !Send`, `MutexGuard[T]: !Send`, and raw-pointer-bearing structs `!Send` unless the user opts in with `unsafe impl Send for MyType {}`. Real enforcement.

#### Slice 6G — Auto-derive attributes · M

`#[derive(Eq)]`, `#[derive(Hash)]`, `#[derive(Clone)]` on user structs. Generates the obvious field-walking impl. Lands after Phase 2C (`impl` on enums) since deriving on enums needs that machinery.

#### Slice 6H — Array repeat-count literal `[0u8; 10]` · S

Doesn't parse today. Workaround is listing elements. ~30-line parser slice.

#### Phase 6 exit criteria

- [ ] Raytracer perf within 10% of C / Rust on the same workload
- [ ] `s[i]` works with bounds-check
- [ ] `arr as T[]` works
- [ ] `"{x:5.2}"` formats correctly
- [ ] `Rc[T]: !Send` rejects cross-thread move

---

### Phase 7 — Tooling · size S

Small bucket of `cpc` UX wins. Each is independent; ship as warmup or between phases.

- **`cpc init <name>`** — scaffolder for a fresh project (Cplus.toml + src/main.cplus + .gitignore).
- **`cpc doc` project mode + HTML** — today `cpc doc` runs per-file; project mode walks Cplus.toml and emits an HTML site.
- **LSP cross-file code actions + pull diagnostics** — multi-file `WorkspaceEdit` support, on-save pull-mode diagnostics (the editor asks, not just push-when-changed).
- **ANSI-colored diagnostics** — `--color=auto/always/never` flag.
- **dsymutil integration + per-instruction `!DILocation` + `DILocalVariable`** — full debug info pipeline so `lldb` breakpoints + variable inspection work without `--release` tricks.
- **`cpc fmt` turbofish-pointer fix** — `size_of::[*u8]()` reformats to `size_of::[* u8]()` today. ~5-line fix.
- **Full `println` intrinsic removal** — v0.0.3 moved `println` to stdlib; the compiler still has a fallback intrinsic path. Remove it; stdlib's `io::println` is the only entry.

---

### Carryovers — also landing in v0.0.5

The "land when motivated" items from v0.0.4 that don't gate a phase but shouldn't be forgotten:

- **`Arc::make_mut`** (clone-on-write to mutable inner)
- **`Mutex::try_lock`** / `lock_with_timeout`
- **`channel::bounded(n)`** / `try_recv` / `recv_timeout`
- **Generic `Cow[T_view, T_owned]`** (only `CowStr` ships today)
- **`CowSlice[T]`** (the `T[]` parallel)
- **Method-level generic bounds inside generic-typed impls** (v0.0.1 carry-forward)
- **Generic struct / enum type-args in generic-fn bodies** (Phase 1B limitation — nested generic-of-generic still rough)
- **Editions support** (v0.0.1 design note)
- **HFA optimization on aarch64** (correct but suboptimal SIMD float aggregates)
- **String `s.scalars()` / `s.graphemes()` iteration**
- **Unicode `\u{HHHH}` escapes in string literals**
- **`impl Ord for i32`** (bounds on built-in primitives — newtype workaround today)
- **`cpc-bindgen`** (libclang-based — separate tool, not language; build when hand-writing bindings becomes painful)

These are S-each and ship between phases or as bundled polish PRs.

### Things explicitly NOT on this roadmap

Locked decisions from v0.0.2 / v0.0.3 / v0.0.4; don't reopen without a clear motivating case:

- Effect tracking + built-in contracts (rejected 2026-05-14)
- Phase 9 / TS-flavored review (rejected 2026-05-13)
- Null in safe code (locked — FFI null is `0 as *T` in `unsafe`)
- `?*T` nullable pointers (killed 2026-05-14)
- Dynamic dispatch / `dyn Interface` (Phase 7 is monomorphization-only)
- Multi-package repos (subdirectory packages)
- Package-manager sandbox / capabilities
- Windows-MSVC `pub extern fn` (needs `inalloca`, rejected v0.0.2 Slice 1H Tier-3)
- Multi-threaded async executor (v0.0.6+ territory)
- SIMD primitives (waits for an intrinsic-plumbing slice)

---

## Known compiler bugs (surfaced during use)

Tracked separately from feature carryovers — these are wrong-behaviour bugs in shipped code paths, not deliberate deferrals.

_(none open at v0.0.5 start — see plan-0.0.4.md's resolved log for v0.0.4's fixes; Phase 1 slices above absorb the carried-forward Drop bugs)_

---

## Resolved log

_(empty at v0.0.5 start)_
