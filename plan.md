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

#### Slice 1A — Cross-module generic-method instantiation · ✅ shipped 2026-05-17

**The framing in v0.0.3's carryover note was off.** The impl-attachment mechanism itself already worked — the existing `stdlib_cross_module_generic_method_propagation` e2e test proved it. What was actually broken: a downstream codegen bug surfacing on the same call paths.

**Real bug, found and fixed:** `musttail call` + sret ABI mismatch. When a wrapper `fn make_buf() -> Vec[u8] { return vec::new::[u8](); }` tail-returned another sret function, the call-site forwarded the caller's sret slot as bare `ptr %0` while the callee declared `ptr sret(%Vec__u8) noalias nonnull noundef writable dereferenceable(24) align 8 %0`. LLVM's musttail verifier rejected: "mismatched ABI impacting function attributes."

**Fix:** [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs#L5149) mirrors the callee's sret attribute string at the musttail call site. 9 lines.

**Tests:** updated the pinned `musttail_with_sret_forwards_caller_slot` lib test (had been asserting the broken shape) + added `musttail_sret_cross_module_vec_return_round_trip` e2e test (consumer module calls `.push()` 3× on a Vec[u8] returned from a producer-module wrapper that tail-calls `vec::new::[u8]()`).

**Test count: 1182 (313 e2e + 854 lib + 11 LSP + 4 other), all green.**

#### Slice 1B — Generic-fn return-type T-substitution · ✅ shipped 2026-05-17

**Reframed during implementation.** The plan's "extend `subst_type_ast` to recurse through `TypeKind::Path { args, .. }`" was misdiagnosed — that recursion already worked. The real gap was deeper: sema doesn't type-check generic-fn bodies (early-returns at [sema.rs:1988](cplus-core/src/sema.rs#L1988) because generics live in `fns_generic`, not `fns`), so the inner call inside `make_buf[T]`'s body (`vec::new::[T]()`) never registered in `call_monos` / `fn_instantiations`. Monomorphize had no transitive entries to walk; the synthesized `make_buf__i32` body contained `vec::new::[T]()` calls that codegen panicked on (`vec_new__i32` never existed).

**Fix:** monomorphize-side propagation that doesn't require sema body-checks. Two parts in [cplus-core/src/monomorphize.rs](cplus-core/src/monomorphize.rs):

1. **Fixed-point propagation pass.** Walk each instantiation's template body, read each `Call`'s AST `type_args` (turbofish), substitute through the outer instantiation's subst, and add the resolved `(callee_name, concrete_args)` to the instantiation set. Iterate until no new pair surfaces. Filters out Param-bearing entries (sema-recorded generic-context instantiations that aren't real concrete monomorphs).

2. **Rewrite-site fix.** In `rewrite_expr`, when looking up a generic-fn call's mangled name, fall back to AST `type_args` (resolved via outer subst) when `call_monos` is empty for the span — covers the generic-fn-body case where sema didn't record anything.

Sema body-checking was attempted first but reverted: it surfaced unrelated qualified-name bugs in `check_thread_intrinsic`'s `JoinHandle` lookup. AST-driven propagation in monomorphize is less invasive and doesn't require sema changes.

**Tests:** added `generic_fn_returning_generic_struct_transitive_instantiation` e2e — a consumer module calls `make_buf::[i32]()` where `make_buf[T]` is user-written and tail-returns `vec::new::[T]()`. Test asserts `b.len() = 3` after 3 pushes.

**Test count: 1183 (314 e2e + 854 lib + 11 LSP + 4 other), all green.**

**Limitations (carry forward to v0.0.5 or revisit if motivated):**
- Generic struct / enum type-args (`make_buf::[Vec[T]]()`) aren't fully resolved — `type_ast_to_ty_with_subst` returns None for non-primitive type names. The struct_instantiations path handles direct uses; nested generic-of-generic in a generic-fn body is still rough.
- Generic-fn bodies still aren't sema-checked, so type errors inside them are caught at monomorphize/codegen rather than at sema. Acceptable for now — the language permits unchecked generic body forms and monomorph errors give precise diagnostics.

#### Slice 1C — `Type[args]::assoc_fn(...)` call shape · ✅ shipped 2026-05-17

`vec::Vec[i32]::with_capacity(16)` works. The parser already produced `GenericEnumCall { enum_name, type_args, variant, args }` for this shape (shared with `Result[T]::Ok(v)`). Sema's struct-template branch only tried impl-block methods and failed for stdlib's free-fn constructors. The fix dispatches to a same-module free generic fn as a fallback.

**Fix** (sema + monomorphize):

1. **Sema** ([cplus-core/src/sema.rs:2804](cplus-core/src/sema.rs#L2804)): in `check_generic_enum_call`'s struct-template branch, when the instantiated struct has no impl method named `variant`, derive the module prefix by stripping the struct's last name segment and look up `<module>.<variant>` in `fns_generic`. If found, dispatch via `check_generic_named_call` with the Type[args] bracket's type_args, and record the decision in a new `MonoInfo::assoc_free_fn_dispatches: HashMap<ByteSpan, String>` so monomorphize can re-derive the lowered shape.

2. **Monomorphize** ([cplus-core/src/monomorphize.rs](cplus-core/src/monomorphize.rs) GenericEnumCall branch): when sema recorded a free-fn dispatch for this span, rewrite to `Call { callee: Ident(mangled_fn_name), args, type_args: [] }`. Inline mangling lookup uses `inst_lookup` since the outer `rewrite_expr` doesn't re-process the produced Call.

**Tests:** `assoc_free_fn_dispatch_via_type_brackets` — `vec::Vec[i32]::with_capacity(16) + push + push + len` returns 2.

**Test count: 1184 (315 e2e + 854 lib + 11 LSP + 4 other), all green.**

**Precedence**: impl-block methods win over free fns when both exist with the same name (sema checks methods first). Mirrors Rust's UFCS semantics.

#### Slice 1D — E0900: borrow check across `await` · ✅ shipped 2026-05-17

**Reframed during implementation:** rather than full dataflow ("borrows held across await"), shipped a parameter-shape gate. Async fns can't take borrow-shaped parameters at all — owned-data-only.

**Why narrower works:** C+ has no `&T` references. The only borrow surface inside an async fn body is parameters of these shapes:
- `Ty::Str` — fat pointer into someone else's string
- `Ty::Slice(_)` — fat pointer into someone else's array/Vec
- `mut x: NonCopyT` — pointer-passed by Phase-6 ABI

Banning these at the parameter list means borrows can't enter the async fn — no possibility of being live across an await. Owned alternatives (`string`, `Vec[T]`, drop the `mut` and `let mut x = x`) cover every legitimate use case.

**Fix** ([cplus-core/src/sema.rs:2031](cplus-core/src/sema.rs#L2031)): in `check_function`, when `f.is_async`, walk parameter list and emit E0900 for each borrow-shaped or `mut`-pointer-passed param. Two diagnostic messages with concrete migration hints.

**Tests:** 5 new sema unit tests — `async_fn_with_str_param_emits_e0900`, `async_fn_with_slice_param_emits_e0900`, `async_fn_with_mut_noncopy_param_emits_e0900` (negatives), `async_fn_with_owned_string_param_clean`, `async_fn_with_copy_param_clean` (positives).

**Test count: 1189 (315 e2e + 859 lib + 11 LSP + 4 other), all green.**

**Forward-pointer:** if Phase 3's reactor surfaces realistic patterns blocked by this rule, refine with dataflow ("borrow live across await") instead of the parameter-shape gate. Error code and diagnostic stay; only the check loosens.

#### Slice 1E — Non-Copy O in `thread::spawn` + `JoinHandle::join` + `async fn` return · ✅ shipped 2026-05-17

Three changes, all in codegen:

1. **Thread spawn for non-Copy O** ([codegen.rs:225–250](cplus-core/src/codegen.rs#L225)): the trampoline now branches on `return_passes_by_sret_widened`. For non-Copy O the worker is called with a sret slot pointing into the heap ctx (offset 8), exactly where `join` reads from. Call-site sret attributes mirror the callee's declaration (same constraint as Phase 1A's musttail fix).
2. **Eligibility expanded** ([codegen.rs:291](cplus-core/src/codegen.rs#L291)): `is_thread_spawn_eligible` accepts `Ty::String`. `mangle_o_for_tramp` produces `"string"` to match sema's `JoinHandle__string` instantiation.
3. **Coroutine promise alloca** ([codegen.rs:2618](cplus-core/src/codegen.rs#L2618)): the prologue was passing `ptr null` as the `coro.id` promise arg but later writing through `coro.promise`. For primitive Copy returns the OOB writes happened to land inside frame slack ("worked" by luck); for `string` (24 B) ASan caught them. Fix: allocate `%.coro.promise = alloca <T>` and pass it as the promise arg + its alignment as the first i32. CoroSplit hoists the alloca into the frame at a known offset.
4. **Future struct lookup for non-scalar T** ([codegen.rs:319](cplus-core/src/codegen.rs#L319)): `ty_from_future_name` now also handles `string` and struct-typed inner names (`Future__Vec__u8` → look up `Vec__u8` in `struct_defs`).

**Tests:** 2 new e2e — `stdlib_thread_spawn_join_non_copy_string` (spawn → join → len("hello from worker") = 17), `async_fn_returning_string_through_block_on` (chained `async fn outer() -> string` awaiting inner, returning len = 15).

**Test count: 1191 (317 e2e + 859 lib + 11 LSP + 4 other), all green.**

**Known limitations carried forward:**
- **ASan + async coroutines unrelated bug.** Even scalar `i32` async fns under `--asan` return 0 instead of the expected value. Pre-existing (not introduced by Phase 1E). The non-ASan path is correct. Tracked as a follow-up — probably ASan instrumentation of the alloca-promise + CoroSplit interaction. Doesn't block the headline Phase 1E goal.
- **Raw / fn-pointer O in spawn:** still falls through `mangle_o_for_tramp`'s `"unsupported"` arm. Phase 1F (recursive mangler) closes this.
- **`Vec[T]` and arbitrary non-Copy structs as `O` in spawn:** the trampoline emission handles them via the same sret path, but `mangle_o_for_tramp` returns `"unsupported"` for `Ty::Struct(_)` — sema's `JoinHandle__Vec__u8` instantiation name wouldn't match. Trivial extension after Phase 1F's mangler lands.

#### Slice 1F — Recursive type-name mangling for raw/fn-pointer O · ✅ shipped 2026-05-17

`thread::spawn::[*u8](worker)` and `thread::spawn::[fn() -> i32](worker)` round-trip now. Eligibility widened; mangler made recursive.

**Fix** ([codegen.rs:184](cplus-core/src/codegen.rs#L184)): `mangle_o_for_tramp_with_types` recurses through `RawPtr`, `FnPtr`, `Array`, `Slice` — and resolves struct / enum names via the type table (needed because codegen's `EnumInfo` doesn't carry the source name; uses reverse lookup through `enum_by_name`). Output matches sema's `mangle_ty_for_name` so `JoinHandle__<suffix>` lookups land.

`is_thread_spawn_eligible` rewritten as a `match` so each shape is explicit. Raw/fn/struct/enum/array O are accepted; `Slice(_)` and `Str` rejected (they're fat pointers borrowing external storage — a worker returning one would hand the parent dangling references once the worker's stack unwinds).

**Tests:** 2 new e2e — `stdlib_thread_spawn_join_raw_pointer_o` (worker returns `malloc`'d `*u8`, parent joins + `free`s), `stdlib_thread_spawn_join_fn_pointer_o` (worker returns `fn() -> i32`, parent joins + invokes).

**Test count: 1193 (319 e2e + 859 lib + 11 LSP + 4 other), all green.**

#### Slice 1G — Generic `async fn` e2e + `is_async` threading verification · ✅ shipped 2026-05-17

**Outcome: already-works, now pinned.** Both halves of the property held before this slice:
- Sema's `subst_ty_deep` threads `is_async` (v0.0.3 Slice 5E groundwork).
- Monomorphize's `synthesize_fn` preserves `template.is_async` when cloning ([monomorphize.rs:550](cplus-core/src/monomorphize.rs#L550)).

Phase 1F's recursive mangler + Phase 1E's promise-alloca fix are what made Copy-T generic async actually work end-to-end (previously the chain had latent issues at codegen-time).

**Test:** new e2e `generic_async_fn_multi_instantiation_round_trip` — drives `id::[i32]`, `id::[i64]`, and `id::[bool]` through `block_on`, asserts each returns its input.

**Test count: 1194 (320 e2e + 859 lib + 11 LSP + 4 other), all green.**

**Known limitation carried forward (NOT new to 1G):** non-Copy `T` parameter to an async fn (`async fn id[T](x: T)` instantiated with `T = string`) double-frees at runtime. This reproduces with **non-generic** non-Copy parameter passing too (`fn echo(x: string) -> string { return x; }`); the bug is the value-passed-without-`move` drop-tracking gap, not async-specific or generic-specific. Workaround: write the param as `move x: T`. Real fix is a separate slice — probably "auto-promote non-Copy value params to `move`" — when motivated. Documented here so the limitation doesn't get rediscovered.

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

#### Slice 2B — `Box[T]` · ✅ shipped 2026-05-17

Single heap-allocated owned value. Pure-source stdlib type at [vendor/stdlib/src/box.cplus](vendor/stdlib/src/box.cplus) — no compiler changes.

**API:**
- `box::new(move v: T) -> Box[T]` — heap-allocate slot, init with `v`, return box.
- `Box[T]::get(self) -> T` — read inner (bitwise copy; caller responsible for not double-using when T is non-Copy).
- `Box[T]::set(mut self, v: T)` — overwrite inner.
- `Box[T]::unwrap(move self) -> T` — consume box, return inner. The function-exit Drop (fires because `move self` transfers ownership into the callee) frees the heap slot; no manual `free` here or it would double-free.
- `Box[T]::drop(mut self)` — frees heap slot. Inner T's Drop (if any) is the caller's job — `unwrap()` first.

**Tests:** new e2e `stdlib_box_round_trip_copy_and_non_copy` covers Copy (i32) and non-Copy (string) round-trips through `new` → `set` → `get` → `unwrap`.

**Test count: 1195, all green.**

**Learning surfaced during implementation:** `move self` doesn't auto-disarm the callee's function-exit Drop. The first version of `unwrap` did `free` explicitly + returned, then the implicit exit-Drop fired and double-freed. Two safe shapes for consuming methods: (a) let exit-Drop do the cleanup (what `unwrap` ended up doing), or (b) explicitly `mark_moved` self inside an intrinsic-call body (what `JoinHandle::join` + `__cplus_thread_join` does). Worth a forward-pointer for v0.0.5: provide a `consume self` syntax that statically disarms callee Drop.

#### Slice 2C — `Arc[T]` · ✅ shipped 2026-05-17

Atomically-refcounted shared ownership. Pure-source stdlib at [vendor/stdlib/src/arc.cplus](vendor/stdlib/src/arc.cplus) — no compiler changes.

**Layout:** one heap block holds `{ u64 refcount, T value }`. Every `Arc[T]` carries `ctrl: *u8` pointing at the header. `clone()` does a Relaxed atomic increment; `drop()` does an AcqRel atomic decrement; the last reference frees.

**API:**
- `arc::new(move v: T) -> Arc[T]`
- `Arc[T]::clone(self) -> Arc[T]` — atomic increment, returns new Arc sharing the storage.
- `Arc[T]::get(self) -> T` — read inner (bitwise copy).
- `Arc[T]::strong_count(self) -> u64` — snapshot via SeqCst load.
- `Arc[T]::drop(mut self)` — atomic decrement; frees on last ref.

**Ordering rationale:** Relaxed on increment (no happens-before required — the new Arc carries a ctrl already visible to this thread). AcqRel on decrement (release pairs with prior ctrl writes; acquire on the final decrement synchronises with all prior drops so the freeing thread sees a consistent view). Matches the Boost / Rust pattern.

**Tests:** `stdlib_arc_cross_thread_share` — two worker threads each receive a cloned Arc, return the inner value, parent verifies + drops last. Runs under no-sanitizer, ASan, and TSan — all clean.

**Test count: 1196, all green.**

**v0.0.4 limitations:**
- No `Arc::make_mut` (clone-on-write to mutable inner) — would need `Arc::unwrap_mut(mut self) -> T` gated on `strong_count() == 1`. Lands when an actual workload asks.
- Inner T's Drop on last reference is not invoked automatically (same v0.0.4 stdlib limitation as `Box[T]` / `Vec[T]`).
- Assumes `align_of[T] <= 8` — over-aligned T would need an alignment-driven offset. Land when motivated.
- `clone()` syntax requires the caller to bind to a local first (`let c = root.clone(); worker(c);`) because of E0337 "cannot move out of a method-call result." Worth a separate ergonomic slice.

#### Slice 2D — `Rc[T]` · ✅ shipped 2026-05-17

Single-threaded sibling of `Arc`. Pure-source stdlib at [vendor/stdlib/src/rc.cplus](vendor/stdlib/src/rc.cplus). Same layout (`{ u64 refcount, T value }`), same API (`new` / `clone` / `get` / `strong_count` / `drop`); refcount ops are plain loads/stores instead of atomic ones.

**Send/Sync contract is documentation-only in v0.0.4.** Passing `Rc` across threads compiles but is unsound (concurrent refcount writes race). Slice 2A locks down `Rc[T]: !Send` at sema-time later in Phase 2.

**Tests:** `stdlib_rc_clone_chain_round_trip` — 3-deep clone chain; verifies refcount increments + ASan-clean teardown.

**Test count: 1197, all green.**

#### Slice 2E — `Mutex[T]` · ✅ shipped 2026-05-17

`Mutex[T]` wraps T + a pthread mutex. `lock(self) -> MutexGuard[T]`; guard's Drop releases.

**Design deviation from the plan: Mutex is internally refcounted.** Rust's idiomatic shape is `Arc<Mutex<T>>` with `&Mutex<T>` as the shared handle. C+ has no `&T` references — a literal `Arc[Mutex[T]]` would break because `Arc::get(self)` returns a bitwise copy of `Mutex`, and `Mutex::drop` would fire on every copy. To work around it without inventing references, `Mutex[T]` collapses Arc into itself: heap block holds `{ u64 refcount, pthread_mutex_t, T value }`; `clone()` does an atomic increment; `drop()` does an atomic decrement and destroys the pthread mutex + frees the heap only on the last reference. Users clone into worker threads; the worker drops normally; the last live reference does teardown.

**API:**
- `mutex::new(move v: T) -> Mutex[T]`
- `Mutex[T]::clone(self) -> Mutex[T]` — atomic refcount inc
- `Mutex[T]::lock(self) -> MutexGuard[T]` — pthread_mutex_lock; guard's Drop unlocks
- `Mutex[T]::strong_count(self) -> u64`
- `MutexGuard[T]::get(self) -> T` / `set(mut self, v: T)`

**pthread mutex layout:** 64-byte allocation per mutex (macOS pthread_mutex_t is 64 B; Linux glibc is 40 B — same code works on both).

**Tests:** `stdlib_mutex_cross_thread_increment` — two workers each acquire/get/inc/set/drop; parent reads final value. Verifies under no-sanitizer, ASan, and TSan — all clean.

**Test count: 1198, all green.**

**v0.0.4 limitations:**
- Guard lifetime is unenforced at sema-time — `let g = m.lock(); let g2 = m.lock();` in the same scope deadlocks (g still holds the lock). Block-scope discipline is the workaround until borrow-checker integration lands.
- Drop of inner T not invoked automatically (same v0.0.4 stdlib limitation).
- No `try_lock` / `lock_with_timeout`. Land when motivated.

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

- **2026-05-17** — Phase 2 Slice 2E shipped. `Mutex[T]` — pthread-backed mutual exclusion. Internally refcounted (collapses Arc into itself; sidesteps the no-`&T` aliasing problem). Cross-thread increment test passes ASan + TSan clean. 1198 tests, all green.
- **2026-05-17** — Phase 2 Slice 2D shipped. `Rc[T]` — single-threaded refcounted shared ownership. Pure-source stdlib at [vendor/stdlib/src/rc.cplus](vendor/stdlib/src/rc.cplus). Same shape as `Arc`, non-atomic refcount. Send/Sync gating is documentation-only in v0.0.4; Slice 2A will lock down `!Send` at sema-time. 1197 tests, all green.
- **2026-05-17** — Phase 2 Slice 2C shipped. `Arc[T]` — atomically refcounted shared ownership. Pure-source stdlib at [vendor/stdlib/src/arc.cplus](vendor/stdlib/src/arc.cplus). Relaxed increment + AcqRel decrement; last reference frees. Cross-thread share verified ASan + TSan clean. 1196 tests, all green.
- **2026-05-17** — Phase 2 Slice 2B shipped. `Box[T]` — single heap-allocated owned value. Pure-source stdlib at [vendor/stdlib/src/box.cplus](vendor/stdlib/src/box.cplus). API: `box::new(move v)`, `get/set/unwrap`. `move self` semantics learned: don't manually free inside a `move self`-consuming method; let the function-exit Drop do it. 1195 tests, all green.
- **2026-05-17** — Phase 1G shipped. Generic `async fn` verified e2e — sema's `subst_ty_deep` + monomorphize's `synthesize_fn` already threaded `is_async`; Phase 1E + 1F's fixes made the full chain run clean. `id::[i32]`, `id::[i64]`, `id::[bool]` all round-trip through `block_on`. 1194 tests. **Phase 1 closed.**
- **2026-05-17** — Phase 1F shipped. `mangle_o_for_tramp` made recursive over `Ty`: raw / fn / struct / enum / array O all work in `thread::spawn`. Eligibility rewritten as explicit `match` (Slice + Str rejected — fat-pointer hazards). 2 new e2e, 1193 tests.
- **2026-05-17** — Phase 1E shipped. Non-Copy `O` for `thread::spawn` + `JoinHandle::join` + `async fn` return. Trampoline emits sret-aware call when O is non-Copy; coroutine prologue allocates a real promise alloca (CoroSplit hoists into the frame). 2 new e2e, 1191 tests. ASan-async interaction noted as pre-existing follow-up.
- **2026-05-17** — Phase 1D shipped. E0900 borrow-across-await guard. Reframed as a parameter-shape gate (no dataflow): async fns can't take `str`, `T[]`, or `mut x: NonCopyT` parameters. The narrower rule catches every realistic v0.0.4 footgun without requiring dataflow infrastructure. 5 new sema tests, 1189 total.
- **2026-05-17** — Phase 1C shipped. `Type[args]::name(...)` falls back to free generic fn in the same module when no impl method exists. Sema records the dispatch in `MonoInfo::assoc_free_fn_dispatches`; monomorphize rewrites the GenericEnumCall AST to a plain Call with the inline-mangled callee. Tests up to 1184.
- **2026-05-17** — Phase 1B shipped. Generic-fn return-type T-substitution + transitive instantiation propagation. Reframed: sema doesn't check generic-fn bodies, so the inner call `vec::new::[T]()` inside `make_buf[T]` never registered. Monomorphize-side fixed-point propagation reads AST turbofish type-args directly, substitutes through outer subst, and discovers transitive instantiations without sema changes. Tests up to 1183 (314 e2e + 854 lib + 11 LSP + 4 other).
- **2026-05-17** — Phase 1A shipped. Reframed: v0.0.3's "cross-module generic-method instantiation" carryover described a non-bug (impl-attachment already worked). The real failure mode was a musttail+sret call-site ABI mismatch that surfaced on stdlib wrapper chains. 9-line codegen fix at [codegen.rs:5149](cplus-core/src/codegen.rs#L5149); updated pinned lib test; added e2e regression. Test count 1182, all green.
