# C+ — Plan

Version 0.0.3 shipped 2026-05-17. See [plan-0.0.3.md](plan-0.0.3.md) for the archived 0.0.3 roadmap and resolved log; [plan-0.0.2.md](plan-0.0.2.md) covers v0.0.2 and [plan-0.0.1.md](plan-0.0.1.md) covers v0.0.1.

---

## v0.0.4 — Close every gap

**Strategy: hard load-bearing work first, no deferrals.** Four phases, ordered by dependency. Phase 1 is compiler-internals — no shipping artifact, but everything else waits on it. Phase 2 ships shared ownership (`Send`/`Sync` + `Arc`/`Mutex`/`Channel`/etc.) because the async runtime wants `Arc` internally. Phase 3 ships the async runtime + stdlib polish in parallel — both are unlocked the same way. Phase 4 ships generators on top of the coroutine machinery.

**No deferrals policy:** every item in [plan-0.0.3.md](plan-0.0.3.md)'s v0.0.4 carryover catalog lands in this milestone. Nothing rolls forward to v0.0.5 unless a hard compiler limitation surfaces during implementation that can't be resolved without breaking-change-scoped redesign.

Slice sizes use assistant-paced framing (S/M/L), not human-typing weeks. A "session" means one focused implementation pass with verification; a phase is "ship the phase when its exit criteria are green," not "schedule N weeks."

---

### Phase 1 — Compiler unblockers · size L

Every Phase-2/3/4 slice is blocked on one or more of these. Land them first, accept no user-visible artifact until Phase 2.

#### Slice 1A — Cross-module generic-method instantiation · M

`impl Vec[T] { fn push(...) }` attaches to `Vec[u8]` regardless of which module instantiates it.

Today: works only inside the stdlib module that defines the impl block. Consumer modules importing stdlib and calling `vec.push(x)` on a stdlib-returned `Vec[u8]` fail at monomorphization — the consumer's resolver doesn't walk imported modules' impl blocks.

Fix: at link time, walk every imported module's impl blocks and re-run impl-attachment for the type-arg-pairs observed in the consuming module.

Tests: `cross_module_vec_push`, `cross_module_vec_iter_then_push`, `cross_module_generic_method_chain`. Regression suite is the stdlib bodies parked in v0.0.3 Phase 1.

#### Slice 1B — Generic-fn return-type T-substitution · S

`fn make_vec[T]() -> Vec[T] { ... }` substitutes T at the call site so `make_vec::[i32]()` returns `Vec[i32]`.

Fix: extend `subst_type_ast` in [cplus-core/src/monomorphize.rs](cplus-core/src/monomorphize.rs) to recurse through `TypeKind::Path { args, .. }`. Same shape as the v0.0.3 fix for non-Path Tys.

#### Slice 1C — `Type[args]::assoc_fn(...)` call shape · S

`Vec[i32]::with_capacity(16)` parses and resolves. Required for any constructor on a parameterized type.

Parser hand-off for `Ident '[' type_args ']' '::' Ident`. Sema: resolve to the impl block instantiated with `type_args`. Codegen: existing `Type::method` path post-monomorphization.

#### Slice 1D — E0900: borrow check across `await` · M

Sema enforces "borrows held across `await` must live in the coroutine frame, not the caller's stack." Hard precondition for Phase 3's reactor.

Body checker walks every `let` binding in scope and checks whether any binding's borrows (in C+'s sense — `mut self`/`self`-receiver result, slice-of-local, raw-pointer-into-local) cross an `await`. If so, the binding's owner must be a parameter or coroutine-local, not a caller-stack value.

Tests: positive (borrow-from-coroutine-local across await — accepts), negative (borrow-from-caller-stack across await — rejects with E0900), edge (await inside `if let` — borrow scope ends at branch).

#### Slice 1E — Non-Copy O in `thread::spawn` + `JoinHandle::join` + `async fn` return · M

`thread::spawn(|| string::from("hello"))` works. `async fn foo() -> string` works. `async fn foo() -> Vec[u8]` works.

sret-aware trampoline: worker writes O to the heap ctx via sret; join reads via memcpy into caller's sret slot. Mirrors v0.0.2 Slice 1P sret widening applied to the spawn/join return path *and* the coroutine return shape. The coroutine case writes to the caller-frame sret slot via the promise.

#### Slice 1F — Recursive type-name mangling for raw/fn-pointer O · S

`thread::spawn(|| ptr)` where `ptr: *u8` works. `async fn foo() -> fn(i32) -> i32` works.

Recursive `mangle_o_for_tramp` over `Ty`: `*u8` → `ptr_u8`, `fn(i32) -> i32` → `fnptr_i32_to_i32`, `Vec[i32]` → `Vec__i32` (existing monomorph mangler shape).

#### Slice 1G — Generic `async fn` e2e + `is_async` threading verification · S

`async fn id[T](x: T) -> T { return x; }` works for multiple instantiations.

Likely already-works after 1E; budgeted as a slice so it gets actually tested.

#### Phase 1 exit criteria

- [ ] `Vec[u8]::push` callable from any module
- [ ] `fn make_vec[T]() -> Vec[T]` returns the right Vec
- [ ] `Vec[i32]::with_capacity(16)` parses and resolves
- [ ] E0900 catches borrow-across-await
- [ ] `thread::spawn(|| "hello")` returns a `JoinHandle[string]`
- [ ] `async fn() -> Vec[u8]` works through `block_on`
- [ ] Generic async fn instantiates and runs

---

### Phase 2 — Shared ownership: `Send` / `Sync` + the type zoo · size L

Lifts v0.0.3's hard contract: shared-ownership types now exist, type system has marker traits to gate cross-thread safety. Phase 3's reactor builds on `Arc` from this phase rather than `unsafe *T` internals.

#### Slice 2A — `Send` / `Sync` marker traits · M

Not full Rust auto-traits — C+-flavored: every struct gets auto-`Send`/`Sync` unless it contains a `*T` field or a `!Send`/`!Sync` marker. Manual `unsafe impl Send for T` for edge cases.

The check is structural at type-definition time, not lifetime-typed. Cross-thread API surfaces (`thread::spawn`'s closure type, `Channel::send`) gate on `Send`. Cross-thread *sharing* (`Arc[T]`'s `T`) gates on `Sync`.

Tests: rejects `Arc[RefCell[T]]`-shape misuse with a precise diagnostic; accepts `Arc[Mutex[T]]`; rejects raw-pointer-containing structs from cross-thread move unless explicitly marked.

#### Slice 2B — `Box[T]` · S

The simplest owned-heap type. No refcount, no sharing. Baseline for the rest of the zoo. Drop calls `free` on the inner ptr after the inner T's Drop.

#### Slice 2C — `Arc[T]` · M

Refcounted shared ownership. Atomic refcount uses the v0.0.3 Phase 5A atomic primitives. `Arc[T]: Send + Sync` iff `T: Sync`.

#### Slice 2D — `Rc[T]` · S

Single-threaded sibling of `Arc`. Same shape, non-atomic refcount. `Rc[T]: !Send`.

#### Slice 2E — `Mutex[T]` · M

`Mutex[T]` wraps `T` + a pthread mutex. `lock(mut self) -> MutexGuard[T]`. `MutexGuard::drop` releases. Canonical shape: `Arc[Mutex[T]]`.

#### Slice 2F — `Channel[T]` · M

MPSC by default. Send half is `Sync` (cloneable across threads); receive half is `!Sync` (single consumer). Unbounded for v1 (bounded waits for a real use case). Lock-free or mutex-based — pick at implementation time based on what compiles cleanest.

#### Slice 2G — `Cow[T]` · S

Copy-on-write borrow. Useful for stdlib hot paths (e.g., `str` operations that may or may not need to allocate).

#### Slice 2H — True fire-and-forget thread detach · S

`JoinHandle::drop` switches from blocking `pthread_join` to refcounted-ctx detach using `Arc[ThreadCtx]`. Closes the v0.0.3 carryover.

#### Phase 2 exit criteria

- [ ] `Arc[Mutex[Vec[i32]]]` shared across 4 threads, deterministic final state
- [ ] `Channel[i32]` with producer/consumer threads, no missed messages
- [ ] `Send`/`Sync` rejects misuse with precise diagnostic
- [ ] `JoinHandle::drop` no longer blocks

---

### Phase 3 — Async runtime + stdlib polish · size L

Two parallel tracks. Track A (async runtime) is the headline v0.0.4 win. Track B (stdlib polish) ships the measured wins from the v0.0.3 curl-lite audit. Both are unlocked by Phase 1, independent of each other.

#### Track A — Async runtime

##### Slice 3A.1 — The reactor: kqueue (macOS) / epoll (Linux) · L

`Reactor` struct in `stdlib/runtime` holds a kqueue/epoll fd + a map from `(fd, direction)` to coroutine handle backed by `Arc[Coroutine]`. `executor::block_on` initializes a per-thread reactor on first call. I/O wrappers post their fd + direction + coroutine handle when they hit EWOULDBLOCK, then suspend. The reactor's poll loop calls kevent/epoll_wait, walks ready events, resumes the registered coroutine.

##### Slice 3A.2 — `executor::spawn_local` + `executor::yield_now` · M

Task queue on top of the reactor. `spawn_local` enqueues; `yield_now` is the cooperative-multitasking primitive (load-bearing for cancellation-aware loops).

##### Slice 3A.3 — Async I/O wrappers · M

`TcpStream::read_async` / `write_async`, `TcpListener::accept_async`, `File::read_async`, `sleep`. Each: set fd nonblocking, attempt sync op, on EWOULDBLOCK register-with-reactor + suspend.

##### Slice 3A.4 — Hand-rolled `Future` implementations · S

Users `impl Future for MyType { fn poll(...) -> Poll[T] }`. Lift the "Future is compiler-known, constructed only via async fn" restriction in sema; the `Poll[T]` enum in stdlib becomes reachable.

##### Slice 3A.5 — `async_fetch` recipe + 1000-task exit test · S

The v0.0.3 plan's worked example, now actually buildable. Plus the "1000 concurrent async tasks" stress test that pins the reactor under realistic load.

#### Track B — Stdlib polish

##### Slice 3B.1 — Stdlib fs/net/env body completions · M

The v0.0.3 skeleton APIs become real. With Phase 1A in, the parked bodies mostly compile as-is. DNS via `getaddrinfo` (replacing the blocking `gethostbyname`), IPv6 support.

##### Slice 3B.2 — `Vec::reserve` + `Vec::with_capacity` · S

Single biggest stdlib win. Trivial once Phase 1C is in.

##### Slice 3B.3 — `Vec::extend_from_slice` + `Vec<u8>` element-type specialization · M

`Vec<u8>::extend_from_slice` lowers to a single `memcpy`; generic path stays as N pushes.

##### Slice 3B.4 — `Result::unwrap_unchecked` + match-inlining hints · S

##### Slice 3B.5 — Generic `HashMap[K, V]` + `Hash[K]` interface · M

Unblocks the `StrIntMap`-only API. Re-derive `StrIntMap` as a type alias.

##### Slice 3B.6 — CPU-bound benchmarks in `proves/` · S

`06-vec-sum-1m`, `07-csv-parse-10mb`, `08-hashmap-100k`. Without these we can't tell if stdlib is regressing.

#### Phase 3 exit criteria

- [ ] `TcpStream::read_async` reads without blocking the executor
- [ ] 1000 concurrent `async fetch_one(url)` tasks complete in ~1× wall-clock-of-slowest, not Σ
- [ ] `sleep(100.ms()).await` actually sleeps
- [ ] `impl Future for MyTimer { ... }` compiles and runs
- [ ] `Vec::with_capacity(n) + push × n` does 1 alloc, not log₂(n)
- [ ] `Vec<u8>::extend_from_slice` lowers to `memcpy`
- [ ] Generic `HashMap[str, i32]` works
- [ ] All v0.0.3 stdlib skeleton APIs are real
- [ ] 3 CPU-bound benchmarks added; cplus-stdlib delta watched

---

### Phase 4 — Generators (`gen fn` + `Iterator[T]` + `for-in`) · size M

Reuses Phase 1G's coroutine machinery — marginal work is parser/AST + the `Iterator[T]` compiler-known interface + `for-in` desugar + borrow-check-across-yield.

#### Slice 4A — `gen fn` + `yield` parser/AST · S
#### Slice 4B — `Iterator[T]` compiler-known interface · S
#### Slice 4C — `for-in` loop sugar (lowering) · S
#### Slice 4D — Borrow check across `yield` · M

Different rule from `await`: caller's stack frame *does* persist across `yield` because `next()` is a synchronous call from a live frame. Check allows caller-stack borrows that come from the immediate `next()` caller's frame; still rejects nested-coroutine misuse.

#### Slice 4E — Migrate stdlib hot paths to iterators · S

`Vec::iter`, `HashMap::iter`, `File::lines`, `range(0, 100)`. Each is a `gen fn`. `vec.iter().filter().map().collect()` starts being writable.

#### Phase 4 exit criteria

- [ ] `for x in count_up(10) { ... }` works
- [ ] `vec.iter().filter(...).map(...).collect()` works
- [ ] Borrow check across yield catches nested-coroutine misuse
- [ ] At least 3 stdlib types expose `iter()`

---

### Carryovers — also landing in v0.0.4

Per the no-deferrals policy, the remaining v0.0.3 carryovers land alongside the phases above as opportunistic slices:

- **Platform parity** — Linux/x86_64 ABI verification for stdlib; pthread `[link]` entry; aarch64-Linux smoke test; Windows-MSVC deferred (real `inalloca` work, not just polish — revisit only if a real consumer asks).
- **Language polish** — string Drop at scope exit; double-Drop on `let b = a` re-bind; format specifiers; per-instruction `!DILocation`; DILocalVariable; dsymutil integration; `cpc fmt` turbofish-pointer fix; `cpc doc` project mode + HTML; LSP cross-file code actions + pull diagnostics; ANSI-colored diagnostics; full `println` intrinsic removal (alongside `cpc init` tooling); slice indexing `s[i]` with bounds-check; array→slice coercion.
- **Tooling** — `cpc init` one-liner scaffolder; `cpc bindgen` out-of-scope items.

These are size-S each and don't gate any phase. Bundle them into phase-end "polish" sub-slices or ship between phases as warmup.

### Things explicitly NOT on this roadmap

Locked decisions; don't reopen without a clear motivating case:

- Effect tracking + built-in contracts (rejected 2026-05-14)
- Phase 9 / TS-flavored review (rejected 2026-05-13)
- Null in safe code (locked — FFI null is `0 as *T` in `unsafe`)
- `?*T` nullable pointers (killed 2026-05-14)
- Dynamic dispatch / `dyn Interface` (Phase 7 is monomorphization-only)
- Multi-package repos (subdirectory packages)
- Package-manager sandbox / capabilities
- Windows-MSVC `pub extern fn` (needs `inalloca`, rejected v0.0.2 Slice 1H Tier-3)
- Multi-threaded async executor (v0.0.5+ territory)
- SIMD primitives (waits for an intrinsic-plumbing slice)

---

## Resolved log

_(nothing yet for v0.0.4)_
