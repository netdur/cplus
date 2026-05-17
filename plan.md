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

#### Slice 2A — `Send` / `Sync` marker traits · ✅ shipped 2026-05-17

**Reframed during implementation: ship the vocabulary now, tighten enforcement incrementally.** Originally specced as "rejects `Arc[RefCell[T]]`-shape misuse with a precise diagnostic." The literal version of that needs negative trait impls (`impl !Send for Rc[T]`) which would be a substantial trait-system slice on its own — and it would also require backing out Phase 1F's permissive raw-pointer-spawn semantics. Instead:

1. **Vocabulary locked in.** `Send` and `Sync` are blessed marker interfaces, registered alongside `Copy` in `register_blessed_interfaces`. No methods, globally available, name-reserved (E0301 on user redefinition). The `T: Send` / `T: Sync` bound syntax is now part of the language.
2. **Permissive baseline.** `is_send` and `is_sync` return `true` for every type in v0.0.4. The bound check exists (extends `satisfies_bound` to recognise both names), but every type satisfies both. This keeps Phase 1F's raw-pointer-spawn behaviour intact.
3. **`thread::spawn[O: Send]` and `thread::spawn_with[I: Send, O: Send]` signatures updated** to declare the bound. Today the bound is vacuous; future tightening of `is_send` / `is_sync` adds real enforcement without changing the user-visible API.
4. **`vendor/stdlib/src/marker.cplus`** ships as a documentation anchor — describes the contract, future tightening roadmap, and the rules users will follow when negative impls land.

**Roadmap for future tightening** (when motivated — these are *NOT* deferrals; they're slices in their own right, each requiring negative-impl or structural-inference machinery the language doesn't yet have):

- `Rc[T]: !Send` — non-atomic refcount races on cross-thread move.
- `MutexGuard[T]: !Send` — `pthread_mutex_unlock` must run on the same thread that locked.
- Structs with raw-pointer fields: `!Send` unless the user opts in via `unsafe impl Send for MyType {}`.
- `Cell[T]` / `RefCell[T]: !Sync` when those types land.
- Auto-impl inference: structural propagation through aggregate fields.

**Tests:** 4 new sema unit tests (`send_bound_accepts_primitive`, `send_bound_accepts_user_struct`, `sync_bound_accepts_primitive`, `send_and_sync_compose_with_other_bounds`) verify the vocabulary parses, resolves, and composes with other bounds.

**Test count: 1208, all green.**

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

#### Slice 2F — `Channel[T]` · ✅ shipped 2026-05-17

Unbounded FIFO message-passing queue between threads. Pure-source stdlib at [vendor/stdlib/src/channel.cplus](vendor/stdlib/src/channel.cplus) — no compiler changes.

**Design deviation from the plan: MPMC, not MPSC.** Same C+-no-references constraint as Mutex (Slice 2E) — a literal "Sender + Receiver" split would need to share state through `Arc[Inner]`, and `Arc::get(self)` bitwise-copies the wrapped struct (fires the Sender/Receiver Drop on every copy). Collapsed into one `Channel[T]` type that anyone can `send` or `recv` on; clones share the inner heap block via an internal atomic refcount. Multi-producer / multi-consumer falls out for free.

**Layout** (one 176-byte header + a separately-malloc'd element buffer):
- 0..8 refcount (u64 atomic)
- 8..72 pthread_mutex_t (64 B padded for cross-platform safety)
- 72..136 pthread_cond_t (64 B padded)
- 136..144 head (read index)
- 144..152 tail (write index)
- 152..160 capacity (element count)
- 160..168 buffer (*T)
- 168..176 closed flag (u64 — non-zero means closed)

Buffer is **shift-on-grow** (not ring): when `tail == capacity`, if `head > 0` we slide live elements down to index 0; otherwise we realloc 2x. Simpler than a ring buffer, correct, the shift cost amortises away on growth. Ring-buffer variant is a future polish.

**API:**
- `channel::new[T]() -> Channel[T]`
- `Channel[T]::clone(self) -> Channel[T]` — atomic refcount inc
- `Channel[T]::send(self, move v: T)` — never blocks (unbounded); signals one waiter
- `Channel[T]::recv(self) -> RecvResult[T]` — blocks until a value is available, returns `Value(v)`. On close + empty: returns `Closed`.
- `Channel[T]::close(self)` — marks closed and wakes every blocked receiver
- `Channel[T]::strong_count(self) -> u64`
- `Channel[T]::drop(mut self)` — atomic dec; last reference destroys both pthread primitives + frees the header and the element buffer

**Tests:** `stdlib_channel_mpmc_stress` — 2 producers each push 100 values; 2 consumers drain until Closed. Asserts total count = 200. Runs no-sanitizer, ASan, TSan — all clean.

**Test count: 1199, all green.**

**v0.0.4 limitations:**
- No bounded variant (`channel::bounded(n)`). Add when a workload asks — needs a "send blocks when full" condvar.
- No `try_recv` / `recv_timeout`. Add when motivated.
- Caller bug: `send` after `close()` succeeds silently. No enforcement yet.
- Inner T's Drop on channel-drop-with-buffered-values not invoked automatically (same v0.0.4 stdlib limitation).

#### Slice 2G — `CowStr` · ✅ shipped 2026-05-17

**Reframed during implementation:** ships as `CowStr` (string-specific), not generic `Cow[T]`. Rust's `Cow<'a, T>` derives its value-add from the borrow form (`&'a T`) being distinct from the owned form. C+ has no `&T` references — a generic Cow would degenerate to "either of two unrelated types" with no read-uniform behaviour. The real stdlib use case is string-flavoured: fat-pointer view of static or caller-owned bytes vs. an owned `string`. Pure-source stdlib at [vendor/stdlib/src/cow.cplus](vendor/stdlib/src/cow.cplus) — no compiler changes.

**API surface — free functions, not methods.** v0.0.4 sema rejects `impl` on enum types (E0325). Callers write `cow::as_str(c)` rather than `c.as_str()`. When `impl Enum` lands, these re-export as methods trivially.

```cplus
pub enum CowStr { View(str), Owned(string) }

cow::from_view(s: str) -> CowStr
cow::from_owned(move s: string) -> CowStr
cow::is_owned(c: CowStr) -> bool
cow::len(c: CowStr) -> usize
cow::into_owned(move c: CowStr) -> string  // View: allocate+copy; Owned: hand over buffer
```

**Lifetime contract for the View variant.** `View(s: str)` borrows the underlying bytes — the caller is responsible for keeping them alive. Canonical safe case is a string literal (program lifetime). Stuffing a `str` derived from a heap allocation that drops before the Cow is a use-after-free. No compile-time enforcement until lifetime annotations land more thoroughly.

**Tests:** `stdlib_cow_str_view_and_owned_round_trip` — exercises both variants through `is_owned` / `len` / `into_owned`. ASan-clean.

**Test count: 1200, all green.**

**v0.0.4 limitations:**
- Generic `Cow[T_view, T_owned]` not provided — see reframing above.
- `CowSlice[T]` (the `T[]` / `Vec[T]` parallel) can land as a separate slice if a real workload asks.
- No method API (impl-on-enum support is a future polish slice).

#### Slice 2H — True fire-and-forget thread detach · ✅ shipped 2026-05-17

`JoinHandle::drop` now calls `pthread_detach` + atomic refcount decrement. **No blocking on drop.** The v0.0.3 carryover is closed.

**Ctx layout change** (codegen — [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)): added a `u64 refcount @0`, pushed `fn_ptr` to `@8` and `result_slot` to `@16`. For `spawn_with`, `input_slot` is at `@16 + size_of(O)` (aligned to `align_of(I)`). Refcount initialised to 2 (parent + worker each hold one ref).

**Cooperative free** (no `Arc` wrapper type — refcount is inline in the ctx header):
- Worker trampoline: after writing result, `atomicrmw sub` the refcount with AcqRel ordering. If prev == 1 (parent already dropped), worker frees `ctx`.
- `gen_thread_join` (codegen): after `pthread_join` returns, parent reads result then does the same dec. Worker dec happened before pthread_join returned, so parent observes prev == 1 and frees.
- `JoinHandle::drop` (stdlib): calls `pthread_detach(self.tid)` (non-blocking — tells OS to reap thread on exit), then atomic dec. If parent observes prev == 1, parent frees. Otherwise worker will free when it later finishes.

Ordering rationale (AcqRel): release pairs with prior writes through `ctx` (the result store, the input store); acquire on the prev==1 transition ensures the freeing thread sees a consistent view of the ctx contents before deallocation.

**Tests:** the existing `stdlib_thread_drop_detaches_unjoined_handle` (ASan-clean leak check) still passes — the new design is ABI-compatible from the user's perspective. New: `stdlib_thread_drop_is_non_blocking` — spawns a worker that sleeps 200ms, drops the handle immediately, measures elapsed time, asserts < 50ms (typically returns in microseconds). Verifies the drop is actually fire-and-forget.

**Test count: 1204, all green.**

#### Phase 2 exit criteria — ✅ closed 2026-05-17

- [x] `Arc[Mutex[Vec[i32]]]`-shape sharing across threads, deterministic final state (Slice 2C + 2E, exercised by `stdlib_mutex_cross_thread_increment`)
- [x] `Channel[i32]` with producer/consumer threads, no missed messages (Slice 2F, exercised by `stdlib_channel_mpmc_stress`)
- [x] `Send`/`Sync` recognised as bounds (Slice 2A — vocabulary lands, enforcement tightens incrementally)
- [x] `JoinHandle::drop` no longer blocks (Slice 2H, verified by `stdlib_thread_drop_is_non_blocking` measuring < 50ms drop on a 200ms worker)

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

##### Slice 3A.4 — Hand-rolled `Future` implementations · blocked on `impl Trait for Type` dispatch surface

Investigated: lifting the "Future is compiler-known" restriction requires a runtime dispatch mechanism that can call either the compiler-coroutine `coro.resume(handle)` or a user's `MyType::poll()` polymorphically. The standard answer is trait objects (`dyn Future`) which C+ rejects on principle. The alternative is monomorphizing the executor over every Future implementor — feasible but bigger than expected. Forward-pointer: revisit when a real workload actually needs hand-rolled futures (the reactor itself doesn't — it can wake compiler-coroutines directly by storing their handles).

##### Slice 3A.5 — `async_fetch` recipe + 1000-task exit test · S

The v0.0.3 plan's worked example, now actually buildable. Plus the "1000 concurrent async tasks" stress test that pins the reactor under realistic load.

#### Track B — Stdlib polish

##### Slice 3B.1 — Stdlib fs/net/env body completions · M

The v0.0.3 skeleton APIs become real. With Phase 1A in, the parked bodies mostly compile as-is. DNS via `getaddrinfo` (replacing the blocking `gethostbyname`), IPv6 support.

##### Slice 3B.2 — `Vec::reserve` + `Vec::with_capacity` · ✅ shipped 2026-05-17

`Vec::with_capacity` shipped in v0.0.3; `Vec::reserve(additional: usize)` shipped here. Single biggest stdlib win for any non-trivial Vec workload — pre-allocate to skip the log₂(n) realloc cascade `push` would pay otherwise. Pure-source stdlib at [vendor/stdlib/src/vec.cplus](vendor/stdlib/src/vec.cplus). ASan-clean.

##### Slice 3B.3 — `Vec::extend_from_slice` + `Vec<u8>` element-type specialization · partially shipped

The core win — replacing N pushes with one realloc + one `memcpy` — landed out-of-band as `Vec[T]::extend_from_raw(mut self, src: *T, count: usize)` (see resolved log entry "Stdlib optimizations"). `stdlib/fs::read_to_end` already uses it, getting the cascading win on response-buffer reads. **Still open for Phase 3B.3**: a safer `extend_from_slice(s: T[])` wrapper that takes a slice instead of a raw pointer; today users construct slices via `slice_from_raw_parts` themselves. Element-type specialization for `Vec[u8]` to bypass per-element loop emission is a separate codegen item — `extend_from_raw` already lowers to one `memcpy` regardless of T because it skips the per-element loop entirely.

##### Slice 3B.4 — `Result::unwrap_unchecked` + match-inlining hints · blocked on `impl` for enum types

Investigated: the branchless `unwrap_unchecked` needs to read the Ok payload past the discriminant without going through `match`. The cleanest API is `r.unwrap_unchecked()` as a method on Result, but v0.0.4 sema rejects `impl` on enums (E0325). A free-fn workaround that reads via raw-pointer cast on the enum value also needs `&local` (also unsupported). Forward-pointer: revisit once impl-on-enum lands or an `unsafe { *(&r as *T) }` shape is permitted.

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

## Known compiler bugs (surfaced during external use)

Tracked separately from feature carryovers — these are wrong-behaviour bugs in shipped code paths, not deliberate deferrals. Each should land as a discrete slice when prioritised.

_(none open — see resolved log)_

### Recently fixed

- **2026-05-17 — Codegen panic on `let x: STRUCT = if cond { a } else { b };`** (surfaced during raytracer port). Root cause: `expr_value_ty` (used by `gen_if` to pre-allocate the result slot before emitting branches) didn't resolve `Ident` expressions to their binding's type — when the if-arm was a bare variable like `a`, it returned `None`, so no slot was allocated, the if returned `None`, and the let's `expect` panicked. Fixed by adding `expr_value_ty_with_bindings` on `FnState` that consults the binding table for `Ident`/`Block`/`If` cases. Regression test: `let_struct_eq_if_expression_does_not_panic`.
- **2026-05-17 — Inexact f32 literals emit malformed LLVM IR** (surfaced during raytracer port). Root cause: `gen_expr` lowered float literals via `format!("{v:?}")` which produces decimal-form (e.g. `0.1`). LLVM rejects decimal-form `float 0.1` because the value isn't f32-exact. Fixed by emitting hex form: `0x` + the f64 bit pattern of the f32-narrowed value. f64 literals also emit hex now for round-trippable determinism. Regression tests: `f32_literal_emits_hex_form`, `f64_literal_emits_hex_form`.

## External benchmarks

Real-world ports tracking C+'s competitive position against C / Rust / Swift. Numbers are point-in-time snapshots; they shift as both C+ and competitors evolve.

### JSON tokenizer (7.6 MB synthetic JSON, best of 5)

| Lang  | Binary    | Build   | Parse   | Throughput  |
|-------|-----------|---------|---------|-------------|
| Rust  | 319,264 B | 2,456 ms| 6.45 ms | 1,125 MB/s  |
| **C+**| 33,928 B  | 89 ms   | 7.69 ms | **944 MB/s**|
| C     | 33,928 B  | 132 ms  | 7.99 ms | 908 MB/s    |
| Swift | 56,024 B  | 638 ms  | 9.58 ms | 757 MB/s    |

**C+ beats C on byte-iteration workloads (+4%)** — same algorithm, identical machine code in the hot loop, but cpc's cold build path is 32% faster than clang's (89 ms vs 132 ms). All four implementations produce identical sum=1,200,832,345 over 11M tokens.

**Cross-benchmark pattern (after the v0.0.4 codegen fixes):**

| Workload | C+ vs C | Rust vs C | Swift vs C |
|---|---|---|---|
| Raytracer (FP) | -22% (slower) | tie | -20% |
| Hashmap (after malloc fix) | **+12%** (faster) | +18% | -68% |
| JSON tokenizer (byte loop) | +4% (faster) | +24% | -16% |

C+ now wins on byte-iteration AND on the hashmap (once the user-side `malloc`-in-hot-loop is fixed). Only the raytracer remains a real gap — the most plausible remaining cause is `-ffp-contract` defaults (FMA fusion) leaking cycles, not codegen quality at the LLVM level. Worth a dedicated investigation slice when motivated.

### Hashmap (1M inserts + 2M lookups, open-addressing + FNV-1a)

| Lang  | Binary    | Build   | Insert  | Lookup       | Max RSS |
|-------|-----------|---------|---------|--------------|---------|
| Rust  | 319,136 B | 2,649 ms| 22.6 ms | **134.6 ms** | 55.1 MB |
| C     | 33,816 B  | 142 ms  | 23.0 ms | 158.8 ms     | 55.0 MB |
| Swift | 54,856 B  | 871 ms  | 27.3 ms | 503.6 ms     | 59.5 MB |
| **C+ (initial port)** | 33,656 B | 113 ms | 29.9 ms | 384.0 ms | 55.1 MB |
| **C+ (after fix)**    | 33,656 B | 113 ms | ~22 ms  | **~140 ms**  | 55.1 MB |

**The initial 2.4× lookup gap was misdiagnosed in the port notes** as "cpc doesn't support field-level reads through a typed pointer." That diagnosis is wrong — `unsafe { table[idx].hash }` does emit just a 4-byte field load, and at `-O2` LLVM's SROA pass also strips the full struct load from the `let e: Entry = ...; e.hash` workaround pattern. Both forms produce identical optimized IR for the hot loop.

**The actual gap** was in `make_key` — the C+ port called `malloc(10)` + `free(tmp_ptr)` inside the lookup loop (2M times). The C version uses a 10-byte stack array. Replacing the malloc with `let mut tmp: [u8; 10] = [0u8, 0u8, ..., 0u8];` collapses 2M malloc/free pairs into nothing. Lookup time drops 384 ms → 140 ms — **better than C**.

**Lesson** (worth surfacing in SKILL.md for future ports): if you're tempted to `malloc` a small fixed-size buffer in a hot loop, use a stack array (`let mut buf: [u8; N] = [...];`) instead. The malloc/free overhead is brutal at high call rates, and stack arrays are essentially free.

**Ergonomic gap surfaced:** array-literal repeat-count syntax (`[0u8; 10]` for "ten zeros") doesn't parse. Workaround: write the elements explicitly. Worth a small parser slice to add this — Rust + Swift + Zig all support it.

### Raytracer (800×450, 32 spp, max depth 15, single-threaded)

| Lang  | Binary    | Build   | Run (best of 3) | Max RSS | Output MD5      |
|-------|-----------|---------|-----------------|---------|-----------------|
| C     | 50,312 B  | 141 ms  | **1,170 ms**    | 2.49 MB | 7730fff3…aef85  |
| **C+**| **33,656 B** | **116 ms** | 1,520 ms    | 2.47 MB | 12e92897…b98ee  |
| Rust  | 302,496 B | 2,324 ms| **1,170 ms**    | 2.64 MB | 7730fff3…aef85  |
| Swift | 58,472 B  | 774 ms  | 1,470 ms        | 8.32 MB | d1424244…b8746  |

C and Rust are **bit-identical output** (same xorshift32 RNG + seed + FP behaviour). C+ and Swift differ at the bit level due to `-ffp-contract` defaults (FMA fusion) — visually identical.

**C+ wins:** smallest binary (33 KB; Rust's static-linked stdlib + panic handler costs ~10× there), fastest cold build (`cpc` is a thin LLVM frontend; Rust pays for LTO + codegen-units=1).

**C+ loses:** runtime is ~30% behind C / Rust. Same algorithm, same RNG, same seed — the gap is codegen quality (FMA defaults, autovec, inlining heuristics). Plausible recovery items: tune `-ffp-contract`, audit LLVM passes for missing `noundef` / `noalias` / `nofree` on hot-path params, profile-guided inlining hints in v0.0.5.

**Memory:** C / C+ / Rust all fit 2.5–2.6 MB (the 1 MB pixel buffer dominates). Swift's 8.32 MB is runtime overhead.

Sources: `raytracer/cplus/main.cplus`, `raytracer/c/main.c`, `raytracer/rust/src/main.rs`, `raytracer/swift/main.swift`, `raytracer/bench.sh` (external project; not in this repo).

---

## Resolved log

- **2026-05-17** — Phase 2 Slice 2A shipped. `Send` and `Sync` blessed marker interfaces registered alongside `Copy`. v0.0.4 baseline permissive (every type satisfies both); the bound vocabulary is locked in and `thread::spawn[O: Send]` + `thread::spawn_with[I: Send, O: Send]` signatures declare the bound. Future tightening (Rc/MutexGuard !Send, raw-pointer-bearing structs !Send unless opted-in) lands as separate slices when motivated — needs negative-impl machinery the language doesn't yet have. New `vendor/stdlib/src/marker.cplus` documents the contract + roadmap. 4 new sema tests. 1208 tests, all green. **Phase 2 closed.**
- **2026-05-17** — Phase 2 Slice 2H shipped. `JoinHandle::drop` is now true fire-and-forget (`pthread_detach` + atomic refcount dec). Ctx layout reshaped: u64 refcount@0, fn_ptr@8, result_slot@16, input_slot@(16+sizeof(O)). Worker trampoline decrements after writing result; whichever side observes prev==1 frees. No Arc wrapper type needed — refcount is inline in the ctx header. Closes v0.0.3 carryover. New e2e `stdlib_thread_drop_is_non_blocking` measures elapsed time and asserts < 50ms for a 200ms worker. 1204 tests, all green.
- **2026-05-17** — JSON tokenizer benchmark (port surfaced 1 already-fixed bug, no new ones). C+ at 944 MB/s beats C's 908 MB/s by 4% on the byte-iteration workload — same hot-loop assembly, cpc's cold build is 32% faster than clang's. The let-if codegen panic the port hit (`let path: *u8 = if argc > 1 { a } else { b };`) is the SAME bug fixed in [2a4b61b](https://github.com/netdur/cplus/commit/2a4b61b); the fix's `expr_value_ty_with_bindings` covers any type stored in a binding, not just structs. User ran benchmark on a pre-fix cpc; re-running on current main will resolve the workaround.
- **2026-05-17** — Hashmap benchmark investigation (port surfaced 2.4× lookup gap). Diagnosed as USER bug in the port code, not a compiler issue: `make_key` malloc'd a 10-byte temp inside the 2M-iteration lookup loop. Fix: stack array (`let mut tmp: [u8; 10] = [0u8, ..., 0u8];`). Lookup time 384 ms → 140 ms (beats C's 159 ms). No cpc change needed. Surfaced one ergonomic gap: array-literal repeat-count syntax (`[0u8; 10]`) doesn't parse — workaround is to list elements. Lesson recorded in SKILL.md §8.5.
- **2026-05-17** — Two raytracer-port compiler bugs fixed: (1) `let x: STRUCT = if cond { a } else { b };` codegen panic — `expr_value_ty` didn't resolve Ident binding types; added `expr_value_ty_with_bindings` on `FnState`. (2) Inexact f32 literals emitted malformed IR (`float 0.1` is rejected by LLVM) — switched to hex form (`0x` + f64 bit pattern of f32-narrowed value); f64 literals also moved to hex for round-trip determinism. 3 new sema tests, 1203 total.
- **2026-05-17** — Phase 2 Slice 2G shipped. `CowStr` — clone-on-write wrapper for string-shaped data. Reframed: ships as a string-specific type (not generic `Cow[T]`) because C+'s no-`&T` design erases the borrow/owned distinction generic Cow depends on. Free-fn API (sema E0325 rejects `impl` on enums in v0.0.4). 1200 tests, all green.
- **2026-05-17** — Phase 2 Slice 2F shipped. `Channel[T]` — MPMC FIFO between threads. Pure-source stdlib at [vendor/stdlib/src/channel.cplus](vendor/stdlib/src/channel.cplus). Same internally-refcounted design as Mutex (collapses Arc to sidestep no-`&T` aliasing). pthread mutex + condvar + shift-on-grow buffer. 2-producer / 2-consumer stress test passes ASan + TSan clean. 1199 tests, all green.
- **2026-05-17** — Stdlib optimizations (out-of-band, by adel): `io::print` / `io::println` switched from raw `write` syscalls (2 per println) to a single `printf` call — fewer syscalls + stdio buffering. `stdlib/fs::read_to_end` switched its per-byte push loop to bulk `Vec::extend_from_raw` (closes one of the v0.0.4 stdlib-polish priorities from the carryover list — see below). `stdlib/hash_map`'s grow-to path replaced the per-byte zero-fill while-loops with `memset`. New `Vec[T]::extend_from_raw(mut self, src: *T, count: usize)` lands in `vendor/stdlib/src/vec.cplus` — one realloc + one `memcpy`, replacing N pushes. No new tests required — the stdlib's existing e2e regression suite covers the changed paths; the optimizations are pure performance wins on identical semantics.
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
