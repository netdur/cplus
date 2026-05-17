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

- **2026-05-17** — Phase 1D shipped. E0900 borrow-across-await guard. Reframed as a parameter-shape gate (no dataflow): async fns can't take `str`, `T[]`, or `mut x: NonCopyT` parameters. The narrower rule catches every realistic v0.0.4 footgun without requiring dataflow infrastructure. 5 new sema tests, 1189 total.
- **2026-05-17** — Phase 1C shipped. `Type[args]::name(...)` falls back to free generic fn in the same module when no impl method exists. Sema records the dispatch in `MonoInfo::assoc_free_fn_dispatches`; monomorphize rewrites the GenericEnumCall AST to a plain Call with the inline-mangled callee. Tests up to 1184.
- **2026-05-17** — Phase 1B shipped. Generic-fn return-type T-substitution + transitive instantiation propagation. Reframed: sema doesn't check generic-fn bodies, so the inner call `vec::new::[T]()` inside `make_buf[T]` never registered. Monomorphize-side fixed-point propagation reads AST turbofish type-args directly, substitutes through outer subst, and discovers transitive instantiations without sema changes. Tests up to 1183 (314 e2e + 854 lib + 11 LSP + 4 other).
- **2026-05-17** — Phase 1A shipped. Reframed: v0.0.3's "cross-module generic-method instantiation" carryover described a non-bug (impl-attachment already worked). The real failure mode was a musttail+sret call-site ABI mismatch that surfaced on stdlib wrapper chains. 9-line codegen fix at [codegen.rs:5149](cplus-core/src/codegen.rs#L5149); updated pinned lib test; added e2e regression. Test count 1182, all green.
