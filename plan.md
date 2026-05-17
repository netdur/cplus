# C+ — Plan

Version 0.0.3 shipped 2026-05-17. See [plan-0.0.3.md](plan-0.0.3.md) for the archived 0.0.3 roadmap and resolved log; [plan-0.0.2.md](plan-0.0.2.md) covers v0.0.2 and [plan-0.0.1.md](plan-0.0.1.md) covers v0.0.1.

---

## v0.0.4 — TBD

Roadmap not yet sequenced. The carryover catalog below consolidates every item that earlier milestones deliberately deferred. Items inside each section are roughly leverage-ordered — the top items are either compiler unblockers (everything else waits on them) or shippable wins with high ROI. Re-sequence into phases before opening implementation slices.

Authoritative source for rationale on each item: search the linked archive for the section title.

---

### Compiler unblockers (load-bearing for stdlib + concurrency)

These are the items that gate other work — finishing them widens what can ship in libraries.

- **Cross-module generic-method instantiation.** `impl Vec[T] { fn push(...) }` methods aren't attached to `Vec[u8]` when used from a module other than `stdlib/vec`. Blocks the `stdlib/fs`, `stdlib/net`, `stdlib/env` body work (Phase 1 slices 1B / 1C / 1E carried from v0.0.3). Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 1.
- **Generic-fn return types mentioning generic structs don't substitute T at the call site.** Blocks fully-generic `Vec[T, A: Allocator]` per the Phase-11 vec/allocator sample. Source: [plan-0.0.1.md](plan-0.0.1.md) §"Allocator + VecI32 reference library".
- **`Type[args]::assoc_fn(...)` not yet wired.** Same blocker as above. Source: [plan-0.0.1.md](plan-0.0.1.md) §Phase 11.
- **5E.4 — borrow check across `await` (E0900).** Hard precondition for the async reactor. Sema currently lets borrows live across `await` without enforcing "the borrow's owner must live in the coroutine frame, not the caller's stack". Latent today because the runtime never suspends; **becomes a live footgun the moment the reactor lands**. Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 Slice 5E.
- **`is_async` through `subst_type_ast`.** Generic `async fn foo[T](x: T) -> T` is mechanically reachable but unverified — no e2e exists. Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 Slice 5E.
- **Non-Copy `O` in thread spawn/join + async-fn returns.** Trampoline needs sret-aware return handling; join needs caller-sret memcpy. Same scope shape as v0.0.2 Slice 1P widening, applied to the spawn/join return path *and* the coroutine return shape. Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 Slice 5B / 5C / 5E.
- **Raw / fn-pointer O via recursive type-name mangling.** Codegen's `mangle_o_for_tramp` only handles scalar primitives; `Future__ptr_u8`-style names need a recursive builder. Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 Slice 5C.

### Async runtime (everything 5E.4 unlocks)

- **The async reactor: kqueue (macOS) / epoll (Linux).** Without it, no async I/O. Required for `TcpStream::read_async`, `File::read_async`, sleep/timer futures.
- **`executor::spawn_local` and `executor::yield_now`.** Plan called for both in 5E.5; only `block_on` shipped. `yield_now` is load-bearing for cooperative-multitasking patterns.
- **Async I/O wrappers** — every `*_async` variant on stdlib's sync types (`TcpStream`, `TcpListener`, `File`, eventually `Process`).
- **`async_fetch` recipe** — the plan's worked example, `fetch(host, port) -> Result[string, IoError]`. Blocked on reactor + async TcpStream + non-Copy T-as-return.
- **The "1000 concurrent async tasks" exit test** from the original Slice 5E plan.
- **Hand-rolled `Future` implementations.** Today `Future` is constructible only via `async fn`. Users can't `impl Future for MyType { fn poll(...) -> Poll[T] }`. The user-facing `Poll[T]` enum is in stdlib but unreachable.
- **Multi-threaded async executor.** Single-threaded `current_thread`-style only ships before this. v0.0.5+ territory.

Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 Slice 5E.

### Concurrency utilities (`Send` / `Sync` / shared ownership)

The hard contract from v0.0.3 was: no shared-ownership types until the marker-trait design lands. Lifting it unlocks the rest of this section together.

- **`Send` / `Sync` marker traits.** Design + threading through the type system.
- **`Arc[T]`.** Refcount the reactor needs for the coroutine-frame lifetime.
- **`Rc[T]`.** Single-threaded sibling. Cheaper to land alongside `Arc`.
- **`Mutex[T]`.** Required once multi-task workloads share state.
- **`Channel[T]`.** The canonical concurrency primitive that pairs with `Mutex`.
- **`Box[T]`.** Workhorse owned-heap type. Useful before any of `Arc`/`Rc`/`Mutex`.
- **`Cow[T]`.** Useful even before `Arc` — copy-on-write borrows lift a lot of allocation pressure.
- **True fire-and-forget thread detach** via refcounted `JoinHandle` ctx. Today `JoinHandle::drop` blocks on `pthread_join`. Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 Slice 5B.

Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 non-goals.

### Generators (`gen fn` + `Iterator[T]` + `for-in`)

Bundled slice. Shares ~80% of v0.0.3's coroutine codegen (5E.3). All three pieces ship together — each is useless alone. Estimated ~1 week if the v0.0.3 generator-ready lowering note was honored.

```cplus
gen fn count_up(n: i32) -> i32 {
    let mut i: i32 = 0;
    loop {
        if i >= n { return; }
        yield i;
        i = i + 1;
    }
}

pub interface Iterator[T] {
    fn next(mut self) -> Option[T];
}

for x in count_up(10) { io::println("${x}"); }
```

Borrow-check delta: caller's stack frame *does* persist across `yield` (because `next()` is a synchronous call from a live frame), unlike across `await`. So E0900's analysis splits: "no caller-stack borrows across `await`" + "caller-stack borrows OK across `yield` from the immediate `next()` caller's frame." Different check; both needed.

Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 5 forward-pointers.

### Stdlib polish — measured wins after v0.0.3 curl-lite audit

`cplus-stdlib` measures +14.7% instructions over libc-only on 04-curl-lite. ~3–5 points are recoverable codegen; ~10% is honest abstraction cost (bounds checks + tagged-union dispatch). Priorities ordered by leverage:

1. **`Vec::reserve` + `Vec::with_capacity`** — single biggest win for any non-trivial Vec workload. Trivial to ship.
2. **`Vec::extend_from_slice`** — collapses N pushes into a memcpy. Cascading win wherever stdlib builds buffers (request build, response read, fizzbuzz output).
3. **`Vec<u8>` element-type specialization** — `extend_from_slice` should lower to a single `memcpy`. Today no path to that.
4. **Iterators via `gen fn`** — see Generators slice above. Unlocks compose-without-allocate.
5. **CPU-bound benchmarks in `proves/`.** 04-curl-lite is syscall-dominated, doesn't exercise stdlib hot paths. Add: "sum 1M i32s in a Vec", "parse 10MB CSV", "hashmap 100k entries". Without these we can't tell if stdlib is regressing.
6. **`Result::unwrap_unchecked` + match-inlining hints.** Small wins, easy to ship.
7. **Generic `HashMap[K, V]` + `Hash[K]` interface.** Unblocks the `StrIntMap`-only API. Blocked on cross-module generic-method work (top of this doc).
8. **SIMD primitives.** No `memchr`-equivalent, no SIMD byte-compare, no bulk-zero. Rust's std uses these in `Read::read_to_end`, `str::find`, `slice::contains`. Lower priority — needs intrinsic plumbing first.

Source: [plan-0.0.3.md](plan-0.0.3.md) §Stdlib optimization gap.

Watch the cplus → cplus-stdlib delta in [proves/stats.md](proves/stats.md)'s 04-curl-lite section as the self-benchmark: drift up past 17% = stdlib slowed down; drop below 12% = codegen improvements landed.

### Stdlib body completions deferred from v0.0.3 Phase 1

These are skeleton APIs whose bodies were blocked on the compiler unblockers above. They become near-trivial once cross-module generic-method instantiation works.

- **`stdlib/fs` bodies.** Skeleton in place; bodies blocked on `Vec[u8]::push` cross-module.
- **`stdlib/net` non-trivial parts** — DNS via `getaddrinfo` (currently `gethostbyname`-only, blocking + not thread-safe). IPv6.
- **`stdlib/env` bodies.**
- **Generic `HashMap[K, V]`** — see stdlib polish above.

Source: [plan-0.0.3.md](plan-0.0.3.md) §Phase 1.

### Platform parity

- **Linux/x86_64 for stdlib.** macOS-only today. Needs `[link] libs = ["pthread"]` in stdlib's `Cplus.toml` + ABI verification for the `[2 x i64]` aggregate-coercion path. Carried since v0.0.2 Phase 3C.
- **Linux parity for `pthread_create` / `pthread_join`.** Same manifest entry above unblocks threading on Linux.
- **Cross-platform C-ABI verification.** Today verified on x86_64 macOS only. x86_64 Linux, aarch64 macOS, and Windows-MSVC haven't been smoke-tested for ABI edge cases (struct-passing rules, varargs register conventions, `byval` / `sret` parameter attributes). Trust LLVM's per-target lowering; verify per platform when a real consumer asks.
- **HFA optimization on aarch64.** Still deferred per v0.0.2 decision — correct but suboptimal for SIMD float aggregates.
- **Windows-MSVC for `pub extern fn`.** Windows-x86 ABI needs `inalloca` which Slice 1H Tier-3 already rejected for v0.0.2. Revisit if a real consumer asks.

Sources: [plan-0.0.2.md](plan-0.0.2.md) §Phase 5, [plan-0.0.1.md](plan-0.0.1.md) §Phase 10.

### Language polish carry-forwards

- **`string` Drop at scope exit.** v1 leaks the buffer of every owned `string` value. Existing Drop machinery is keyed by `StructId`; integrating `Ty::String` needs either a parallel `string_locals` tracker or a synthesized struct entry. Source: [plan-0.0.1.md](plan-0.0.1.md) §Phase 8.STR.3.
- **`let b = a` non-explicit moves don't flip source's drop flag.** Pre-existing soundness gap in struct Drop machinery — re-binding a Drop struct registers Drop on BOTH bindings, double-free at scope exit. Programs that produce values and let them drop at end of scope work; the bug surfaces with explicit re-binds. Cross-cuts the entire Drop machinery. Source: [plan-0.0.1.md](plan-0.0.1.md) §Phase 11 slice-types ship.
- **String format specifiers.** `${n:>5}`, `${pi:.2}` — none in v1.
- **Per-instruction `!DILocation`** for debug info. Today function-level only. Source: [plan-0.0.1.md](plan-0.0.1.md) §Phase 11 DWARF.
- **DILocalVariable** for debug info. Today no variable-level metadata.
- **`dsymutil` integration on macOS.** cpc deletes its temp `.o` after linking; for DWARF resolution users must currently retain it manually. Either preserve the `.o` or run `dsymutil` ourselves. Source: [plan-0.0.1.md](plan-0.0.1.md) §Phase 11 DWARF.
- **Project mode for `cpc doc`** — today single-file mode only. Read `Cplus.toml` + walk imports.
- **HTML rendering for `cpc doc`** — today Markdown only.
- **AST-driven signature rendering in `cpc doc`** — today the signature line is raw source up to `{`/`;`.
- **`cpc fmt` `*`-after-`[` (turbofish open-bracket) type-position anchor.** Today `size_of::[*u8]()` reformats to `size_of::[* u8]()`. ~5 line fix.
- **Reformatting heuristics** (collapse incidental wraps, force multi-line on overflow). Slice 4D.2 of Phase 4 fmt.
- **`textDocument/diagnostic` pull diagnostics** in LSP. Today push-only. Defer until a real editor user asks.
- **Cross-file LSP code-action quick-fixes.** Today fixes are emitted only when the suggestion target file matches the asked URI. Cross-file fixes (e.g., E0403 with a suggestion at the declaration site) need a multi-file `WorkspaceEdit`.
- **Slice indexing `s[i]` with bounds-check.** Today users go via `slice_ptr` + raw-pointer arithmetic inside `unsafe`. Plus: array→slice coercion `arr as T[]`, slice mutation.
- **Slice 1C scoped `!alias.scope` for local `let mut` bindings.** v0.0.2 shipped param-only. Local-binding scopes compound the win after inlining but need pre-allocating scopes during codegen.
- **ANSI-colored diagnostic output.** Source: [plan-0.0.1.md](plan-0.0.1.md) §Phase 11 misc.
- **Full removal of compiler `println`/`print` intrinsic.** Today coexists with stdlib's `io::println`. Removal blocked on a one-liner `cpc init` lowering the project-setup cost for the ~50 demo files that use single-file mode. Source: [plan-0.0.3.md](plan-0.0.3.md) §Slice 1A.

### Tooling

- **`cpc init`** — one-liner project scaffolder. Mentioned as a prerequisite for removing the `println` intrinsic.
- **`cpc bindgen` deferred items** — Phase 4 v0.0.3 shipped MVP. Out-of-scope items from the design note remain TBD.

### Things explicitly NOT on this roadmap

Recorded for posterity — these decisions are locked and should not be reopened without a clear motivating case.

- **Effect tracking + built-in contracts.** Rejected 2026-05-14. Error codes E0900–E0920 reserved. No design will be written, no implementation planned.
- **Phase 9 (TS-flavored review).** Rejected 2026-05-13. Principles "function over syntax" and "no several ways to do the same thing" locked in.
- **Null in safe code.** Locked. FFI null is `0 as *T` inside `unsafe`, never a keyword.
- **`?*T` nullable pointers.** Killed by §2.1 / locked null-handling principle (2026-05-14).
- **Dynamic dispatch (`dyn Interface`).** Phase 7 is monomorphization-only. Separate later design decision if ever.
- **Multi-package repos** (pm.md §9). Subdirectory packages can be re-derived later if needed.
- **Sandbox / capabilities** for the package system. Deferred indefinitely.

### Resolved

_(nothing yet for v0.0.4)_
