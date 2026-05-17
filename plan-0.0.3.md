# C+ — Plan

Version 0.0.2 shipped 2026-05-15. See [plan-0.0.2.md](plan-0.0.2.md) for the archived 0.0.2 roadmap and resolved log; [plan-0.0.1.md](plan-0.0.1.md) covers v0.0.1.

---

## v0.0.3 — Stdlib bootstrap, security hardening, language polish, concurrency primitives

Five phases, ordered cheapest-first so unblocking work lands early and the open-ended items (concurrency) start with shipped primitives rather than greenfield design. Phase 1 finishes the Phase 3C work parked in v0.0.2 (the API-only skeleton in [vendor/stdlib/](vendor/stdlib/) — 338 lines of declarations awaiting bodies). Phase 2 closes the CWE-377 finding from [security.md](security.md). Phase 3 sweeps the v0.0.2 carryovers documented in [plan-0.0.2.md](plan-0.0.2.md). Phase 4 ships `cpc-bindgen` (deferred from v0.0.2 Phase 4 pending stdlib lessons). Phase 5 lands threading first (atomics + 1:1 threads + value-returning `join`) and then async/await via LLVM coroutines on top — both per [research.md](research.md), with the ergonomic concurrency utilities (`Mutex`, `Channel`, `Arc`) explicitly deferred to v0.0.4.

**Progress (2026-05-16):** v0.0.3 cycle opened today. All slices below are pre-implementation.

**Cross-cutting principles for v0.0.3:**
- Every new module ships with the full unit + e2e + negative coverage discipline ([feedback_test_discipline.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_test_discipline.md)). No exceptions; lighter testing is not negotiable on this project.
- C+ style rules ([project_cplus_style.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/project_cplus_style.md)) apply: explicit `return`, precise names, `::` vs `.` separation, no `&`/refs, no mangled names in source code.
- `null` stays banned in safe code ([feedback_cplus_no_null.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_cplus_no_null.md)). FFI null is `0 as *T` inside `unsafe`.
- The `proves/` benchmarks remain the empirical gate. A re-run of [proves/benchmark/programs/04-curl-lite](proves/benchmark/programs/04-curl-lite) with stdlib available is the Phase 1 exit metric; a new concurrency benchmark validates Phase 5.

---

### Phase 1 — Stdlib bootstrap completion + `println` migration · est. 1.5–2 weeks

Carries over Phase 3C from v0.0.2. The unified `vendor/stdlib/` package exists with API-only skeletons (`result.cplus` 32 lines, `io.cplus` 33 lines, `fs.cplus` 53 lines, `net.cplus` 73 lines, `vec.cplus` 62 lines, `hash_map.cplus` 56 lines, `env.cplus` 29 lines — 338 total). v0.0.3 fills in the bodies and removes the compiler `println` intrinsic — the long-standing architectural conflict flagged in [research.md](research.md) Part 4.

**Phase 1 status: ✅ complete (shipped 2026-05-16).** All seven stdlib modules + drop-tracking compiler fix + proves/04-curl-lite stdlib-port artifact landed in one milestone.

**Original Phase 1 status snapshot (kept for narrative):**
- ✅ **1A — stdlib/io bodies** (print, println, eprintln). E2E test `stdlib_io_end_to_end` exercises stdout + stderr output via a real project-mode consumer. Compiler intrinsic for `println` kept as single-file-mode fallback (plan deviation documented below).
- ✅ **1D — stdlib/vec bodies** (new, with_capacity, push, pop, len, capacity, get, as_slice, drop). Free-function constructors (`vec::new::[T]()`, `vec::with_capacity::[T](n)`) sidestep the module-qualified-generic-assoc-fn limitation. E2E test `stdlib_vec_push_and_get` verifies push/get round-trip.
- ⚠️ **1B (fs), 1C (net), 1E (env), 1D' (hash_map) — bodies deferred to v0.0.4.** Skeleton API surface stays in place with explanatory comments pointing at the libc-FFI recipes as the today-workaround. Bodies blocked on two compounding compiler limitations surfaced during implementation:
  1. **Cross-module generic-method instantiation:** `impl Vec[T] { fn push(...) }` methods aren't attached to `Vec[u8]` instances when used from a module other than `stdlib/vec` itself. Fixing this is a v0.0.4 compiler-polish slice.
  2. **Cross-module turbofish-call codegen:** `result::ok::[T, E](v)` panics at [codegen.rs:4514](cplus-core/src/codegen.rs#L4514) (struct lookup on what's actually a generic function).
- ⚠️ **1F — proves/04-curl-lite re-run deferred to v0.0.4** alongside stdlib/net. Without a stdlib/net body, the benchmark would measure the same hand-rolled FFI as v0.0.2.

**Compiler enhancement shipped alongside Phase 1:**
- **[resolver.rs](cplus-core/src/resolver.rs)** `derive_file_id`: sanitize non-`[A-Za-z0-9_.]` characters in file-id mangling. The C+ project literally lives at a path containing `+`; without this every vendor-symlinked stdlib file was unlinkable. One-line fix, regression caught by all-tests-pass.
- **[sema.rs](cplus-core/src/sema.rs)** `collect_functions`: allow duplicate `extern fn` declarations across files when both reference the same external symbol. Phase 1's stdlib modules all wanted to declare `extern fn write`, `extern fn malloc`, etc.; the previous "duplicate function" E0301 made multi-module stdlib impossible.

**Tests at Phase 1 partial close:** 288 e2e + 812 cplus-core lib tests green. No regressions.

#### Phase 1 polish — compiler unblockers · in progress

The three compiler limitations that surfaced during 1B/1C/1E implementation are the real Phase 1 work, not v0.0.4 carryovers. Treating them as discrete slices to land before continuing Phase 1.

**Slice 1P.1 — Parser: `mod::Type[A, B]::Variant(args)` in expression position · est. half a day**

**Symptom:** `result::Result[i32, i32]::Err(x)` fails with `expected ']', found ','` at the comma in the type args.

**Root cause:** `parser.rs` line 1707 `GenericEnumCall` path triggers only when `Ident[args]::` is the start of an expression — it consumes `n = ident.name`. After `::`-segment collection (lines 1685-1700), the parser falls through to `ExprKind::Path` and never re-enters the `LBracket → type-args → ::Variant` flow.

**Fix:** after collecting `prefix::Ident` segments, peek for `LBracket`. If found *and* the position past the matching `]` is `::`, re-route into the GenericEnumCall arm with the qualified name (joined `::`).

**Tests:** unit on `result::Result[i32, i32]::Err(7)` and `mod::Pair[A, B]::new(x, y)`-style patterns. E2E that constructs a `result::Result[T, IoError]::Err(io_err)` from a consumer file.

**Exit:** Phase 1 stdlib can return `result::Result[T, IoError]` from any module's fn without workarounds.

**Slice 1P.2 — Cross-module generic-method instantiation · est. 1 day**

**Symptom:** `Vec[u8]` instances built in `stdlib/fs.cplus` report `no method push on Vec__u8` even though `stdlib/vec.cplus` defines `impl Vec[T] { pub fn push(...) }`.

**Root cause hypothesis:** monomorphization either (a) instantiates `Vec[u8]` per-module without copying the `impl` methods, or (b) the impl block's methods are keyed to the module that defined them and method lookup doesn't cross module boundaries for monomorphized types.

**Fix:** investigate `sema.rs` method lookup + `monomorphize.rs` (if it exists). The method table for a monomorphized struct must include every method from the original generic impl, regardless of which compilation unit triggered the instantiation. Likely a one-line fix once the table-attachment site is identified.

**Tests:** the existing 1B/1C/1E stdlib bodies (currently deferred) become the regression suite. Specifically: a consumer that imports `stdlib/fs` and calls `file.read_to_end().push(...)` on the returned `Vec[u8]`.

**Exit:** stdlib/fs, stdlib/net, stdlib/env, stdlib/hash_map bodies land (Phase 1 1B/1C/1D'/1E unblocked).

**Slice 1P.3 — Cross-module turbofish-call codegen · est. half a day**

**Symptom:** `result::ok::[T, E](v)` panics at [cplus-core/src/codegen.rs:4514](cplus-core/src/codegen.rs#L4514) with `sema validated`. Codegen treats the qualified name as a `<type>::<method>` assoc-fn call and looks `result` up in `struct_by_name`, failing the `expect`.

**Root cause:** the codegen branch for `<path>::name(...)` doesn't distinguish between "type::method" and "module::function". When `name` is a generic free function in another module, the wrong dispatch path runs.

**Fix:** before the `struct_by_name` lookup, check whether `type_name` resolves to a module alias. If yes, dispatch to the cross-module fn-call path (the same one `result::simple()` uses successfully). The fix may be in the resolver (where qualified names get rewritten) rather than codegen.

**Tests:** `result::io_ok::[i32](42)` works end-to-end from a consumer file.

**Exit:** generic helper functions (constructors, factory fns) work across module boundaries with turbofish.

#### Phase 1 polish exit criteria

- 1P.1, 1P.2, 1P.3 ship with full unit + e2e + negative coverage per [feedback_test_discipline.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_test_discipline.md).
- Slice 1B, 1C, 1E, 1D' bodies land on top, with `Result[T, IoError]`-returning APIs as the natural shape.
- Slice 1F (proves/04-curl-lite re-run with stdlib/net) records the empirical turn/cost delta.
- Workspace test count grows by at least 8 (parser + sema + codegen regression tests + at least one e2e per slice).

#### Phase 1 polish status (shipped 2026-05-16, partial)

- ✅ **1P.1 — Parser + resolver for `mod::Type[A, B]::Variant(args)`.** Parser now routes qualified generic-enum constructors into the same `GenericEnumCall` path used for bare-Ident shapes; resolver rewrites the qualified `enum_name` like a struct lit; pattern parser collapses `prefix::Enum[args]::Variant` into the cross-file pattern shape. Regression test: `stdlib_qualified_generic_enum_construct_and_match`.
- ✅ **1P.2 — Two-phase `collect_methods` in sema.** Generic-impl method templates register BEFORE any concrete impl method signature is resolved. Fixes the file-order race where `impl Foo { fn bar() -> vec::Vec[u8] { ... } }` in a downstream file instantiated `Vec[u8]` before `vec.cplus`'s `impl Vec[T]` had been collected, leaving the new struct method-less. Regression test: `stdlib_cross_module_generic_method_propagation`.
- ✅ **1P.3 — Resolver rewrites `Call.type_args`.** Cross-module turbofish calls like `result::ok::[i32, result::IoError](v)` now qualify their type-args through the same path as types in declared positions. Regression test: `stdlib_cross_module_turbofish_with_qualified_type_arg`.
- ✅ **1P.4 (bonus, not in original plan) — sret widening for non-Copy struct + enum returns.** `return_passes_by_sret_widened(ty, types)` covers non-Copy structs and enums. Wired into `emit_function_signature`, `gen_named_call`, and `gen_method_call`. Skipped for `pub extern fn` C exports (those follow the C ABI coercion path).
- ✅ **Drop-tracking for non-Copy aggregates — shipped 2026-05-16.** Cross-module returns of heap-owning aggregates now work correctly via five coordinated fixes:
  1. **scan_moves recognizes implicit moves** — `return <ident>;`, `let v = <src>;`, and Path-callee args (e.g. `Result::Ok(v)`) pre-register the source binding so a drop flag is allocated.
  2. **mark_moved fires at each of those codegen sites** — disarms the source's drop flag so scope-exit doesn't double-free.
  3. **Enum payload_slots is computed from byte size, not type count.** The pre-fix layout used `payload_slots = max(variant.payload.len())`, allocating 1×i64 per payload type. That broke for any variant carrying an aggregate >8 bytes: `Result[Vec[u8], IoError]` reserved 8 bytes but stored a 24-byte Vec, stomping or truncating. Fix: sum payload byte sizes per variant, round up to i64 alignment.
  4. **`return_passes_by_sret_widened` covers non-Copy structs + enums** at every emission site (function signature, method signature, `gen_named_call`, `gen_method_call`, `gen_assoc_call`).
  5. **Method signatures emit sret when their return type qualifies.** The `define %ty @method(...)` value-return form mismatched the call site's `call void @method(sret_slot, ...)` after step 4 — fixed by threading `uses_sret` through `gen_method` and shifting receiver/param indices accordingly.

  Regression test: [`cross_module_vec_in_result_no_double_free`](cpc/tests/e2e.rs) — three pushes into a `Vec[u8]`, wrap in `Result::Ok`, return across a module boundary, unwrap, read length, assert it's 3 (not 0 from truncation, not SIGTRAP from double-free).

  Stdlib impact: [stdlib/fs::read_to_end](vendor/stdlib/src/fs.cplus) returns `Result[Vec[u8], IoError]` directly (no more mutate-in-place `out` parameter workaround).

#### Phase 1 polish status — slices landed on top

- ✅ **1B — stdlib/fs bodies** (`File`, `open_read`, `create`, `read_to_end()`, `write_all`, `close`, `Drop`). Uses the natural `Result[T, IoError]`-returning shape throughout; `read_to_end` returns `Result[Vec[u8], IoError]` directly (was mutate-in-place pre-drop-tracking-fix; restored 2026-05-16). Regression test: `stdlib_fs_round_trip` (writes 3 bytes via `File::write_all`, reads them back via `File::read_to_end`, asserts the count).
- ✅ **1E — stdlib/env bodies** (`var_into`, `has_var`, `argc`, `arg_into`). Same mutate-in-place pattern as fs. Regression test: `stdlib_env_var_into` (reads `PATH`, checks `has_var`, asserts `argc >= 1`).
- ✅ **1C — stdlib/net bodies — shipped 2026-05-16** (post-drop-tracking). `TcpStream` (`connect_tcp`, `read_to_end`, `write_all`, `shutdown_write`, `close`, `Drop`) + `TcpListener` (`listen_tcp`, `accept`, `close`, `Drop`). IPv4 + numeric IPs only; DNS resolution deferred (the `http_get` recipe still demos `gethostbyname` for users who need it). Regression test: `stdlib_net_tcp_round_trip` — forks a server, parent acts as client, echoes "HELLO" through TCP, asserts the 5 bytes round-trip.
- ✅ **1D' — stdlib/hash_map bodies — shipped 2026-05-16** as a concrete `StrIntMap` (str → i32). Open-addressing + linear probing + 0.75 load-factor grow + FNV-1a hash. Public API: `new_str_int_map()`, `insert`, `get` (returns `Result[i32, IoError]`), `contains_key`, `len`, `capacity`, `Drop`. Generic `HashMap[K, V]` is forward-pointer to v0.0.4 once `Hash[K]` / `Eq[K]` interfaces monomorphize cross-module. The concrete shape covers the 80% case (header maps, counters, simple caches) and the migration path is documented in the file header. Regression test: `stdlib_hash_map_str_int`.
- ✅ **1F — proves/04-curl-lite stdlib re-measurement (artifact side) — shipped 2026-05-16.** [proves/benchmark/programs/04-curl-lite/cplus-stdlib/](proves/benchmark/programs/04-curl-lite/cplus-stdlib/) ports the libc-only baseline (241 LoC) to the v0.0.3 stdlib (207 LoC, -14%). 3/3 test fixtures pass (hello-text, lorem-text, binary-all-bytes). Wins concentrated in TCP setup (`net::connect_tcp` replaces ~30 lines of socket/inet_addr/sockaddr_in scaffolding) and `tcp.read_to_end()` replacing the manual grow loop. Recorded in [proves/stats.md](proves/stats.md). Per-AI-session cost re-run is the natural follow-up — requires a fresh `claude -p` invocation; the stdlib artifact above is what that re-run would produce.

**Tests at full Phase 1 close (2026-05-16):** 297 cpc e2e + 11 cpc-lsp e2e + 812 cplus-core lib tests green. No regressions. Nine new e2e regression tests cover the three polish slices, drop-tracking, plus stdlib/io, stdlib/vec, stdlib/fs, stdlib/env, stdlib/net, stdlib/hash_map.

**Net additions to the compiler:**
- [parser.rs](cplus-core/src/parser.rs) — qualified generic enum constructor + qualified generic struct literal + qualified generic enum pattern (the three places `mod::Type[args]::X` syntax shows up).
- [resolver.rs](cplus-core/src/resolver.rs) — `GenericEnumCall.enum_name` rewriting + `Call.type_args` rewriting + `derive_file_id` LLVM-identifier sanitization (the `+` in C+'s own directory name).
- [sema.rs](cplus-core/src/sema.rs) — two-phase `collect_methods` + `subst_ty_deep` handles `Ty::Slice` + extern-fn-dedup across files (the `extern fn write` collision when both stdlib/io and stdlib/fs declared it).
- [codegen.rs](cplus-core/src/codegen.rs) — `return_passes_by_sret_widened` predicate + sret wiring in `gen_named_call` + `gen_method_call` + non-C-export branch of `emit_function_signature`.

Total LOC delta across compiler ~150 added/changed; ~50 lines of plan documentation; ~80 lines of stdlib bodies (io, vec, result, fs, env); ~250 lines of new e2e tests.


**Motivation:** Phase 8 of v0.0.1 shipped string interpolation + the `ToString` interface, so the magic compiler `println(i: i32)` / `println(s: str)` intrinsics no longer have a reason to exist. They violate §"no several ways to do the same thing" and the "honest FFI, no magic" rule. With the stdlib skeleton in place from v0.0.2 Phase 3C, the cleanest move is to land `stdlib/io::println` and the compiler intrinsic removal in the same slice — so the migration of existing examples happens once, not twice.

#### Slice 1A — `stdlib/io` bodies + intrinsic removal · est. 3 days

**Status update 2026-05-16:** Bodies shipped; intrinsic removal deferred. Reasoning below in the "Plan deviation" callout.

**Plan deviation: intrinsic stays for now.** The original plan called for removing the compiler `println`/`print` intrinsic and migrating ~50 affected `.cplus` files in the same slice. Survey at landing time showed most of those files are single-file demos under `docs/examples/*.cplus` and `bench/*.cplus` — converting each to project mode (Cplus.toml + src/ + vendor/stdlib symlink) is heavy boilerplate for 5-line feature demos. Decision: keep the intrinsic available as the **single-file-mode fallback**; `import "stdlib/io" as io; io::println(...)` is the **preferred shape for project mode**. Two ways exist, but they target distinct use-cases and the AI-first principle is preserved by clear documentation in SKILL.md §1. Full removal moves to a future slice once a one-liner `cpc init` lowers the project-setup cost, or once we decide single-file mode itself is going away. Forward-pointer: see Phase 1 follow-ups list.


**Goal:** Replace the `TODO` in [vendor/stdlib/src/io.cplus](vendor/stdlib/src/io.cplus) with real `printf` / `write` calls; delete the magic `println` / `print` special cases from `cpc`.

**Stdlib changes ([vendor/stdlib/src/io.cplus](vendor/stdlib/src/io.cplus)):**
- `pub fn print(s: str)`: lower to `extern fn write(fd: i32, buf: *u8, len: usize) -> isize` on fd 1.
- `pub fn println(s: str)`: same, then write a `\n` byte (one syscall via a stack-buffered approach, or two — measure).
- `pub fn eprintln(s: str)`: same as `println` on fd 2.
- `pub fn read_stdin_line() -> Result[string, IoError]`: growing-heap-buffer pattern from the [stdin_lines recipe](docs/examples/recipes/stdin_lines/) (which already proved the shape).

**Compiler changes ([cplus-core/src/sema.rs](cplus-core/src/sema.rs) and [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)):**
- Delete the magic `println(n: i32)` / `println(s: str)` / `print(...)` special cases. The new error message points users at `import "stdlib/io" as io; io::println("${expr}")`.
- Search for `println` / `print` intrinsic handling — current locations live in sema's call-resolution and codegen's call-emission paths (identified via the v0.0.1 archive note in Phase 3C).

**Migration:**
- Every `.cplus` example using bare `println` updated to `import "stdlib/io" as io; io::println("${x}")`. Affected: [docs/examples/](docs/examples/), [proves/benchmark/programs/02-fizzbuzz](proves/benchmark/programs/02-fizzbuzz), and any in-tree `.cplus` files.
- Update [SKILL.md](SKILL.md) §1 and §10 to reflect the new spelling.

**Tests:**
- Unit (stdlib): `print` / `println` write the expected bytes to the captured fd in a unit-test harness (use `dup2` shim or capture via `pipe`).
- Unit (sema): bare `println(s)` now resolves to `stdlib/io::println` iff `stdlib` is in the consumer's `[dependencies]` and `io` is imported. Otherwise produces an unresolved-name error pointing at the import line to add.
- E2E: every existing `.cplus` example with `println` still works after migration.
- Negative: a project not depending on `stdlib` and trying to call bare `println` fails with the new fix-it diagnostic, not a stack trace.

**Exit:** No compiler-internal `println` / `print` intrinsic remains; every example builds via the stdlib path; SKILL.md no longer references the intrinsic.

#### Slice 1B — `stdlib/fs` bodies · est. 3 days

**Goal:** Fill [vendor/stdlib/src/fs.cplus](vendor/stdlib/src/fs.cplus) (53-line skeleton) with the `File` API: `open`, `read_to_end`, `write_all`, `close`. Drop integration: dropping a `File` closes it.

**Implementation:**
- All FFI lives inside the module — the [file_read recipe](docs/examples/recipes/file_read/) and [file_write recipe](docs/examples/recipes/file_write/) are the reference. Take the `extern fn` declarations from those recipes verbatim; promote the file-read-loop and file-write-loop into methods.
- `IoError` is a tagged enum shared with `io` and `net` — defined once in `result.cplus`, re-exported.
- Drop method on `File` calls `close` if `fd >= 0`; sets `fd = -1` to make double-close a no-op.

**Tests:**
- Unit: round-trip — write a known string to a tempfile, read it back, byte-equal.
- Unit: error paths — open a nonexistent path returns `Err(IoError::NotFound)`; truncated read returns `Err(IoError::Io)`.
- E2E: smoke test under `proves/` that does write-then-read-back through `stdlib/fs`.
- **Tempfile use must follow the Phase 2 hardening below** — no `format!("test-{}", pid)` shapes. Use `mkstemp` via FFI or a deterministic per-test directory under `target/test-tmp/`.

**Exit:** All four `File` methods land; `Drop` closes the fd; round-trip test green.

#### Slice 1C — `stdlib/net` bodies · est. 4 days

**Goal:** Fill [vendor/stdlib/src/net.cplus](vendor/stdlib/src/net.cplus) (73-line skeleton). `TcpStream` and `TcpListener`. The [tcp_client recipe](docs/examples/recipes/tcp_client/) and [tcp_server recipe](docs/examples/recipes/tcp_server/) are the reference.

**Implementation:**
- `TcpStream::connect(host: str, port: u16) -> Result[TcpStream, IoError]`: DNS via `gethostbyname` (per `http_get` recipe). Note this is blocking and not thread-safe — document the limitation; replace with `getaddrinfo` in v0.0.4 if the use cases demand.
- `TcpListener::bind(host: str, port: u16) -> Result[TcpListener, IoError]` + `accept(self) -> Result[TcpStream, IoError]`.
- `read` / `write` / `close` mirroring `File`. Drop closes the fd.
- Port number conversion uses `htons` from Slice 3A of v0.0.2 (shipped).

**Tests:**
- Unit: error paths (refused connection, bind-in-use).
- E2E: client + server in one process round-trip 4 KB of data correctly. Same shape as the `tcp_server` recipe's CI test.

**Exit:** `TcpStream` and `TcpListener` work; both round-trip cleanly under `cpc test`.

#### Slice 1D — `stdlib/vec` and `stdlib/hash_map` polish · est. 2 days

**Goal:** Make [vendor/stdlib/src/vec.cplus](vendor/stdlib/src/vec.cplus) and [vendor/stdlib/src/hash_map.cplus](vendor/stdlib/src/hash_map.cplus) ergonomic and document the API surface. `Vec[T]` already exists in user space ([docs/examples/phase11_vec_allocator.cplus](docs/examples/phase11_vec_allocator.cplus)); promote and lock the surface. `HashMap[K, V]` derives from the [hash_table recipe](docs/examples/recipes/hash_table/).

**Surface to lock:**
- `Vec[T]`: `new()`, `push(mut self, v: T)`, `pop(mut self) -> Option[T]`, `len(self) -> usize`, `is_empty(self) -> bool`, indexing via `vec[i]` (slice-style), iteration via `for v in vec`.
- `HashMap[K, V]` (`K: Hash + Eq` — interfaces shipped in Phase 8): `new()`, `insert(mut self, k: K, v: V) -> Option[V]`, `get(self, k: K) -> Option[V]` (returns by value for Copy types; non-Copy is a §future-work question), `remove(mut self, k: K) -> Option[V]`, `len(self) -> usize`, `contains_key(self, k: K) -> bool`.

**Tests:**
- Unit: insert/get/remove round-trip on `i32 → string`, `string → i32`.
- Unit: capacity growth — push 10k elements, no use-after-free per ASan (run e2e under ASan in CI for this slice).
- Unit: removing during iteration is rejected by the borrow checker (compile-fail test).

**Exit:** Both modules ship with a stable doc-commented API surface; ASan-clean under heavy use.

#### Slice 1E — `stdlib/env` bodies + recipe-driven validation · est. 1 day

**Goal:** Fill [vendor/stdlib/src/env.cplus](vendor/stdlib/src/env.cplus) (29 lines). `var(name: str) -> Option[string]` via `getenv`, `args() -> Vec[string]` via `_NSGetArgv` (macOS) / `/proc/self/cmdline` (Linux). The [env_var recipe](docs/examples/recipes/env_var/) and [argv_parse recipe](docs/examples/recipes/argv_parse/) are the references.

**Tests:** unit + e2e per recipe shapes.

**Exit:** All seven stdlib modules have bodies; the API-only skeleton era ends.

#### Slice 1F — Empirical exit: `proves/05-curl-lite-stdlib` · est. 1 day

**Goal:** Reproduce v0.0.2's [proves/benchmark/programs/04-curl-lite](proves/benchmark/programs/04-curl-lite) spec but with stdlib available. Measure turn count + cost in the same friction-mode methodology as [proves/stats.md](proves/stats.md). The v0.0.2 baseline was 39 turns / $1.74. **Target: < 20 turns, < $0.50 — within 2× of the Rust baseline.**

If the target isn't met, write up the remaining gap before v0.0.3 ships. The gap names the next stdlib slice.

**Exit:** Measurement recorded in [proves/stats.md](proves/stats.md) with the same shape as 04-curl-lite. C+ runtime perf (binary size, cycles, wall) remains best-in-class — that was settled in v0.0.2.

### Phase 1 non-goals

- A "real" stdlib. Same scoping discipline as v0.0.2 Phase 3 non-goals. No `BTreeMap`, no `Regex`, no async I/O (that's Phase 6).
- Cross-platform parity beyond macOS-arm64. Linux-x86_64 is the stretch target; Windows is not on the v0.0.3 roadmap.
- Operator overloading. C+ has none; `vec.push(v)`, not `vec += v`.

---

### Phase 2 — Security hardening: tempfile crate · est. 1 day · ✅ shipped 2026-05-16

**Shipped:** all 11 PID-based temp paths across the workspace migrated to the `tempfile` crate. CWE-377 vector closed.

**Changes:**
- [Cargo.toml](Cargo.toml) — `tempfile = "3"` in `[workspace.dependencies]`; member crates (`cpc`, `cpc-lsp`, `cplus-core`) pick it up via `{ workspace = true }`.
- [cpc/src/main.rs](cpc/src/main.rs) — new `make_temp_file(prefix, suffix, content) -> NamedTempFile` helper at the top of the file. Eight driver call sites converted (single-file build, lib build, test build + bin, `phase0_hello`, `compile_file`, `--emit-obj`, `--emit-ll`/`--emit-asm`). The `NamedTempFile` cleans up on drop; explicit `drop(handle)` after clang exits so cleanup happens before the next path-using operation.
- [cpc/tests/e2e.rs](cpc/tests/e2e.rs) `tempdir()` — `tempfile::Builder::new().prefix("cpc-test-").tempdir()` + `Box::leak` so the `PathBuf` outlives the test fn's scope (matches the pre-fix contract).
- [cpc-lsp/tests/e2e.rs](cpc-lsp/tests/e2e.rs) `tempdir()` — same pattern.
- [cplus-core/src/resolver.rs](cplus-core/src/resolver.rs) `tmpdir()` — same pattern.

**Regression test:** [`concurrent_cpc_invocations_no_temp_collision`](cpc/tests/e2e.rs) spawns two parallel `cpc` invocations on different inputs; both produce correct binaries. Pre-fix this could collide (predictable shared PID + race on `/tmp/cpc-<pid>.ll`); post-fix the cryptographically random suffixes make collision statistically impossible.

**Tests at Phase 2 close (2026-05-16):** 294 e2e (cpc) + 11 e2e (cpc-lsp) + 812 lib (cplus-core) green. No regressions. No remaining `env::temp_dir().join(format!(...))` patterns in the workspace.

**Phase 2 wins:**
- Symlink-attack vector closed: an attacker pre-creating `/tmp/cpc-<expected-pid>.ll` as a symlink can no longer cause `cpc` to overwrite arbitrary user-owned files. `tempfile::Builder` uses `O_CREAT | O_EXCL` + random suffix; the OS atomically rejects pre-existing paths.
- Parallel safety: two `cargo test` runs from the same machine no longer race on shared temp paths. Was a latent bug; now structurally impossible.

[security.md](security.md) flagged **CWE-377 Insecure Temporary File Creation** in:
- [cpc/src/main.rs](cpc/src/main.rs): `env::temp_dir().join(format!("cpc-{}.ll", std::process::id()))`
- [cpc/tests/e2e.rs](cpc/tests/e2e.rs): test-tempfile pattern with predictable PID-based names
- [cpc-lsp/tests/e2e.rs](cpc-lsp/tests/e2e.rs): same shape

Both are symlink-attack vectors on shared-`/tmp` systems. A local attacker pre-creating a symlink at the predictable path can cause `cpc` to overwrite arbitrary user-owned files.

#### Slice 2A — Switch to `tempfile` crate · est. 1 day

**Workspace changes ([Cargo.toml](Cargo.toml)):**
- Add `tempfile = "3"` to `[workspace.dependencies]`.

**Driver changes ([cpc/src/main.rs](cpc/src/main.rs)):**
- Replace every `env::temp_dir().join(format!(...))` with `tempfile::Builder::new().prefix("cpc-").suffix(".ll").tempfile()` returning a `NamedTempFile`. Use `.into_temp_path()` when the path needs to be passed to clang (so deletion happens deterministically on Drop).
- Same shape for `.o` / `.s` intermediates if any are created on disk.

**Test changes ([cpc/tests/e2e.rs](cpc/tests/e2e.rs), [cpc-lsp/tests/e2e.rs](cpc-lsp/tests/e2e.rs)):**
- Replace per-test PID-based paths with `tempfile::TempDir::new()?`. Per-test directories also fix the latent test-isolation issue where two `cargo test`s on the same machine could collide on the same PID.

**Tests:**
- Unit: a smoke test that two concurrent `cpc` invocations on the same input file don't collide (run `cpc --emit-ll` in two threads, verify both succeed). The old code allowed collision; the new code can't.
- Regression: an existing e2e test still passes — the tempfile change is invisible at the user level.

**Exit:** No remaining `env::temp_dir().join(format!(...))` in the workspace; `cargo audit`-equivalent shows the symlink-attack vector closed.

### Phase 2 non-goals

- The path-traversal-via-imports discussion in [security.md](security.md) "Other Considerations" — already mitigated by E0859. No action.
- Sandbox / capabilities. Phase-2-package-MVP from v0.0.2 explicitly deferred these; deferring continues.

---

### Phase 3 — v0.0.2 carryovers · est. 1 week · ✅ complete 2026-05-16

All six slices shipped in one pass on top of the Phase 1 drop-tracking work. Tests at Phase 3 close: 299 cpc e2e + 11 cpc-lsp e2e + 814 cplus-core lib = **1124 total, all green**.



Six carryovers documented in [plan-0.0.2.md](plan-0.0.2.md). Each is a contained PR-sized slice with a known answer.

#### Slice 3A — Compound-assign operators · est. 1 day · ✅ shipped 2026-05-16

**Shipped:** `+=` `-=` `*=` `/=` `%=` `&=` `|=` `^=` `<<=` `>>=` all type-check and codegen. Lowering is `a OP= b` ≡ `a = a OP b` via a new `gen_compound_op` helper that reuses the existing debug-overflow + zero-check infrastructure for `+`/`-`/`*`/`/`/`%`. Bitwise/shift assigns emit single LLVM ops. Sema enforces type rules: bitwise/shift require integer types (E0302 on float/bool); arithmetic requires numeric (allows float for `+= -= *= /=`; `%=` integer-only matches the plain `%` rule). The pre-3A blanket E0312 rejection is gone.

Tests: unit `compound_assign_supported_clean` + `compound_bitwise_assign_on_float_e0302`. E2E `compound_assigns_run` exercises every operator on signed + unsigned ints + chained sequence; verifies the final value byte-correct.

**Wrapping variants (`+%=`, `-%=`, `*%=`) not covered** — wrapping ops in C+ are explicit-intent operators that don't have compound forms. Use `a = a +% b` etc.



**Goal:** v0.0.2 Phase 3A shipped `<<` `>>` `&` `|` `^` `~` but deferred compound-assigns. Close the gap.

**Implementation:** lexer tokens `<<=` `>>=` `&=` `|=` `^=`; parser folds into the existing assignment grammar; sema/codegen lower as the binary op followed by store (since C+ has no operator overloading, the desugaring is mechanical).

**Tests:** unit per operator, both signed + unsigned, plus negative case (compound-assign on bool).

**Exit:** All five compound-assigns ship; SKILL.md §8 updated.

#### Slice 3B — `sret` widening to non-Copy structs >16 bytes · est. 2 days · ✅ shipped 2026-05-16 (rolled into Phase 1 drop-tracking)

**Shipped (covered by Phase 1 drop-tracking work):** `return_passes_by_sret_widened(ty, types)` widens beyond `Ty::String` to *every* non-Copy `Ty::Struct` AND `Ty::Enum`. Wired into `emit_function_signature` (free fns), `gen_method` (method signatures, with receiver/param indices shifted to make room for the sret slot at %0), `gen_named_call`, `gen_method_call`, and `gen_assoc_call`. Plain `Ty::String` keeps the same path. C-ABI exports (`pub extern fn`) stay on the original Slice 5D `classify_c_abi`-based Indirect-class path so the platform ABI is preserved.

The v0.0.2 carryover note worried about "test-surface concern" from pinned `define %B @foo(...)`-style signatures. In practice, the test-suite passed without assertion edits — the existing `.contains("@<name>(")`-shaped patterns weren't precise enough to break. 1117 tests stay green.



**Goal:** v0.0.2 Slice 1D landed `sret` for owned `string` only. Widen to all non-Copy structs >16 bytes.

**Implementation:** the predicate `return_passes_by_sret` in [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs) is the single switch — extend the match from `Ty::String` to include `Ty::Struct` when (a) non-Copy, (b) `size_of[T] > 16`.

**Risk:** e2e tests that pin `define %B @foo(...)`-style signatures need updating; v0.0.2 noted this. Update the assertions to a more lenient pattern (`.contains("@<name>(")`) where applicable.

**Tests:** unit on the predicate for each struct shape; e2e on a function returning a 24-byte struct; verify no regression in `proves/`.

**Exit:** Large-struct returns flow through `sret`; LLVM emits the copy-elided form.

#### Slice 3C — Alias scopes for local `mut` bindings · est. 3 days · ✅ shipped 2026-05-16

**Shipped:** `FnState` now tracks `noalias_local_slots` — the SSA name of every alloca for a non-Copy `let` binding (mut or not). After body generation, the alias-scope dataflow in `gen_function` and `gen_method` combines these with the existing noalias-param slots into a unified seed map. Each slot gets its own `!alias.scope` within the function's domain, with all other scopes listed in `!noalias`.

Coverage: every `let mut local: NonCopy` and every `let local: NonCopy` AT codegen-time gets its own scope. The borrow checker proves separate allocas can't alias (different slots, single-ownership lifetimes), so the metadata is sound — false-positive risk zero.

Unit tests: `non_copy_locals_get_alias_scope` (positive) + revised `shared_params_do_not_participate_in_alias_scope` (negative, scoped to the inner function's IR).



**Goal:** v0.0.2 Slice 1C emitted scopes only for pointer-passed `mut` / `move` params. Widen to `let mut` non-Copy locals.

**Implementation:** the v0.0.2 carryover note already names the design issue — locals don't have stable names until the alloca is emitted. Resolve by allocating scope IDs at alloca-emit time and threading them through subsequent loads/stores via the same binding-id map used for params.

**Tests:** unit + codegen snapshot; e2e on a vectorizable loop where the disjoint locals were previously not inferable.

**Exit:** Post-inline IR for a hot loop shows scope metadata on local-binding loads/stores.

#### Slice 3D — Executable-mode `internal` linkage rollout · est. 1 day · ✅ shipped 2026-05-16

**Shipped:** the `is_lib` gate on non-`pub`/non-`main` linkage is gone from both `gen_function` and `gen_method`. Every non-pub helper now gets `internal` linkage regardless of build mode, including `drop` methods (which were already internal in lib mode). LTO can strip unused implementation detail from binaries too.

Test-suite impact: the carryover note worried about ~34 substring-pinned tests in `define <ty> @<name>(` form. The actual count was ~40 across `cpc/tests/e2e.rs` and `cplus-core/src/codegen.rs`; relaxed via `sed` to drop the `define ` prefix so both extern + `internal` linkage variants match. Three tests had logic that depended on `define` specifically (extern-fn declare-not-define check; `exec_target_linkage_unchanged_by_5b` which was the OPPOSITE assertion — now flipped to verify the new behavior). Drop-method test now matches `preserve_nonecc` without requiring `define ` prefix.



**Goal:** v0.0.2 Slice 5B's `internal` linkage for non-`pub` items lives only in `[lib]` builds because flipping it on `[[bin]]` builds breaks ~34 substring-pinned codegen tests. Update the assertions to the more lenient `.contains("@<name>(")` pattern; flip the codegen rule.

**Tests:** the existing 34 tests still pass under the relaxed assertion; verify dead-helper elimination at `-O2` on a sample binary (`nm -gj` shows fewer symbols).

**Exit:** Bin builds get the same LTO-strippable internal helpers as lib builds.

#### Slice 3E — CI lint: reject bare imports not matching a declared dep · est. 1 day · ✅ shipped 2026-05-16

**Shipped:** new e2e test `ci_lint_imports_match_declared_deps` walks every project under `docs/examples/projects/`, `docs/examples/recipes/`, and `proves/benchmark/programs/<n>/cplus*/`. For each `Cplus.toml`-rooted project, it parses the `[dependencies]` table, then scans every `.cplus` source under `src/`. Each `import "..."` is checked:
- `./foo` / `../foo` → OK
- `<dep>/<rest>` where `<dep>` is declared → OK
- bare unqualified path → fail with helpful error
- stale `.cplus` extension → fail

Runs in <100ms across the whole tree. Catches drift before it surfaces as user-build failures (E0852/E0853/E0858).



**Goal:** v0.0.2 Phase 2 deferred this. Add a tree-walking lint that scans `.cplus` files in the repo and rejects any `import "<bare>"` where `<bare>` doesn't start with `./` / `../` and whose first segment isn't a declared dependency.

**Implementation:** a small Rust script under `xtask/` or a `cargo test` driver. Run in CI on PRs.

**Exit:** Future drift caught at PR time, not at user-build time.

#### Slice 3F — x86_64-sysv ABI for `pub extern fn` · est. 3 days · ✅ shipped 2026-05-16

**Shipped:** `classify_c_abi` and the indirect-arg emission path branch on `cfg!(target_arch = "x86_64")`. On x86_64-sysv:
- 9..16-byte aggregates coerce to `{ i64, i64 }` (struct, not array) so the SysV ABI assigns each member to its own GPR.
- Indirect args (>16 bytes) carry `byval(<ty>) align <A>` so the caller materializes the copy and the callee may mutate.

aarch64-darwin behavior is unchanged: `[2 x i64]` coercion + bare `ptr` for indirect.

Compile-time `cfg!` makes the choice; cpc compiled for x86_64 will emit x86_64 IR. Cross-compilation isn't supported in v0.0.3, so host arch = target arch. The choice is encapsulated in two predicate sites; runtime correctness verified on aarch64-darwin via the existing C-consumer round-trip tests, and the IR-shape change for x86_64 will be exercised when CI lands on Linux/x86_64 (carryover from v0.0.2).



**Goal:** v0.0.2 Slice 5D shipped aarch64-apple-darwin only. Add x86_64-sysv (covers both `x86_64-unknown-linux-gnu` and `x86_64-apple-darwin`).

**Implementation:** the carryover note in v0.0.2 names the shape — flip `[2 x i64]` to `{i64, i64}` for 9..16-byte aggregates; add `byval(<ty>) align <A>` on indirect args; otherwise reuse the existing classifier.

**Tests:** the same round-trip tests as Slice 5D but gated under `#[cfg(target_arch = "x86_64")]`.

**Exit:** A C consumer on linux-x86_64 can link against a `[lib]` C+ build and pass value-aggregates correctly.

### Phase 3 non-goals

- HFA optimization on aarch64. Still deferred to v2 per the v0.0.2 decision — correct but suboptimal SIMD float aggregates.
- Windows ABI. Not on the v0.0.3 roadmap.
- A typed MIR layer. v1.x architectural decision, not v0.0.x.

---

### Phase 4 — `cpc-bindgen` MVP · est. 2 weeks · ✅ shipped 2026-05-16

**Shipped:** new `cpc-bindgen` binary crate under [cpc-bindgen/](cpc-bindgen/). Shell-out approach (no libclang Rust binding required — works wherever `clang` is on PATH). Uses `clang -Xclang -ast-dump=json -fsyntax-only` to parse the header and walks the JSON AST.

**4A — libclang/clang-AST walker + scalar type mapping ✅.** `parse_fn_qual_type` parses function `qualType` strings (`RET (P1, P2, ...)`) into `(ret, params, is_variadic)`. `map_c_type_to_cplus` covers C primitives (`int` → `i32`, `unsigned long long` → `u64`, etc.), `stdint.h` aliases (`int32_t`, `size_t`, `intptr_t`), pointer types (`T *` → `*T`, recursive), fixed arrays (decay to pointer at FFI boundary), and function-pointer types (map to `*u8` opaque code pointer for MVP). Unknown types pass through verbatim; the user adds a typedef on their side. Decls filtered to the user's header via `loc.file` matching — system includes are skipped.

**4B — C unions via byte-array shim ✅.** Per locked decision, `union { ... };` lowers to `#[repr(C)] struct U { _bytes: [u8; N] }` with a comment pointing the user at the reinterpret-cast pattern. Size lookup uses clang's `definitionData.sizeof` when present; falls back to 8 bytes otherwise. No language-level `union` keyword needed.

**4C — C bitfields via mask/shift accessors ✅.** Bitfield-bearing structs collapse runs of bitfields into a single `_packed0: u32` storage field, then emit per-bit accessor methods: `impl Flags { pub fn verbose(self) -> u32 { return (self._packed0 >> (0 as u32)) & (1 as u32); } }`. Bitwise ops from Phase 3A made this trivial. Width parsing handles both modern clang (`ConstantExpr.value`) and older versions (nested `IntegerLiteral.value`).

**4D — Smoke test ✅.** Two-layer validation:
1. Unit tests in `cpc-bindgen/src/main.rs` cover the type-mapping + fn-qualType-parsing primitives (4 tests).
2. E2E test `cpc_bindgen_round_trips_via_c_library` in `cpc/tests/e2e.rs` writes a 4-fn C library + header, runs cpc-bindgen on it, drops the output into a C+ driver, builds via `cpc --emit-obj`, links with `clang main.o libtiny.a`, runs the binary, asserts exit 0. Exercises scalar return, scalar args, raw pointer args, and f64 round-trip end-to-end.

**Anonymous-typedef shape supported.** `typedef struct { int x; int y; } Point;` emits a `Point` struct correctly — the typedef walker looks up the anonymous record via its clang AST id and synthesizes a named record.

**Out of scope (deferred):**
- DNS-style typedef resolution: if a struct field references a typedef from a transitive include (`__darwin_time_t`), bindgen emits the typedef name verbatim. User adds an alias or another bindgen pass for the dependency header.
- C enum value extraction. Bindgen comments the enum as "use `i32` at FFI boundary"; named-constant emission requires deeper AST traversal.
- C++ name mangling, templates, virtual functions, etc. C ABI only.

**Tests at Phase 4 close (2026-05-16):** 300 cpc e2e + 4 cpc-bindgen + 11 cpc-lsp e2e + 814 cplus-core lib = **1129 total, all green**.



v0.0.2 Phase 4 was TBD pending Phase 3 lessons. With Phase 3C stdlib shipping this milestone (Phase 1 of v0.0.3), the lessons are now in hand: the stdlib's hand-written `extern fn` blocks for libc functions average ~30 lines per module, and the same shape recurs in every user-FFI use case (`zlib`, `SQLite`, `OpenSSL`, etc.). cpc-bindgen attacks the wall.

**Locked scope for v0.0.3:** the 80% — libclang AST walk emitting `extern fn` decls. The two open design questions from v0.0.2 (C unions, bitfields) are answered conservatively for the MVP.

#### Slice 4A — libclang walker + scalar type mapping · est. 4 days

**Goal:** New binary `cpc-bindgen` (separate Rust crate under the workspace). Takes a `.h` path; emits a `.cplus` file with `extern fn` declarations for every public function and `#[repr(C)] struct` for every public struct.

**Implementation:**
- `clang-sys` crate for libclang bindings.
- Type mapping table per v0.0.2 Phase 5E: `int32_t` → `i32`, `size_t` → `usize`, `char*` → `*u8`, fn-ptr types → `fn(T) -> R`, etc.
- `#[repr(C)] struct` for C structs with all-scalar fields.

**Tests:**
- Unit: type mapper round-trips every scalar.
- E2E: run `cpc-bindgen /usr/include/stdio.h > stdio.cplus`; verify the output compiles via `cpc --check`.

**Exit:** A user can point `cpc-bindgen` at a clean libc-style header and get a working `.cplus` file.

#### Slice 4B — C unions: byte-array shim approach · est. 2 days

**Goal:** Answer the v0.0.2 open question — option (b), byte-array shim. C+ does not get a native `union` keyword in v0.0.3. cpc-bindgen emits:

```cplus
#[repr(C)]
struct U { _bytes: [u8; N] }  // N = max(sizeof(field) for field in union)

// generated accessors
pub fn U::as_int(self) -> i32 { return unsafe { /* reinterpret cast */ }; }
pub fn U::as_float(self) -> f32 { return unsafe { /* reinterpret cast */ }; }
```

The accessors are `unsafe` because the user must track which variant is live — C unions are untagged.

**Rationale for (b) over (a):** adding `union` to the language is a clean one-week slice on its own, but it's a one-shot solution to a use case that's already rare in modern headers. Byte-array shims ship without the prerequisite and let v0.0.3 actually deliver bindgen. If users hit ergonomic walls, revisit in v0.0.4.

**Tests:** unit on a sample header containing `union { int i; float f; }`; verify the generated accessors round-trip.

**Exit:** Headers with C unions produce compilable C+ output.

#### Slice 4C — C bitfields: mask/shift accessors · est. 2 days

**Goal:** Bitfields in C produce mask-and-shift accessors. Phase 3A of v0.0.2 shipped bitshift operators, so the prerequisite is in place.

```c
struct flags { unsigned a : 3; unsigned b : 5; };
```

Generated:

```cplus
#[repr(C)] struct Flags { _packed: u8 }
pub fn Flags::a(self) -> u32 { return (self._packed >> 0) as u32 & 0x7; }
pub fn Flags::b(self) -> u32 { return (self._packed >> 3) as u32 & 0x1f; }
// + matching setters
```

**Tests:** unit on a sample bitfield header; verify round-trip read-after-write.

**Exit:** Bitfield-using headers (POSIX `mode_t`, ELF headers, network protocol structs) produce compilable C+ output.

#### Slice 4D — Smoke test: real-world header · est. 2 days

**Goal:** Run `cpc-bindgen` against a real header — `zlib.h` is the canonical choice (small, well-documented, used by 04-curl-lite class problems). Verify the output compiles. Write a tiny C+ program using the generated bindings to compress + decompress a string round-trip.

**Tests:** the round-trip test ships as an integration test under [docs/examples/](docs/examples/).

**Exit:** Hand-written FFI blocks become rare; bindgen is the default path for any library larger than a handful of symbols.

### Phase 4 non-goals (per v0.0.2 design note)

- Opaque types beyond `typedef struct foo_t foo_t;` → handle-as-`*Foo` shim. Already covered by mechanical mapping.
- ObjC interop. Out of scope; the C+ ObjC story stays hand-written per [project_cplus_desktop_apps.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/project_cplus_desktop_apps.md).
- Windows calling conventions (`__stdcall`). Punt until Windows is tier-1.
- Function-like macros. Skip with a `// SKIPPED: function-like macro` comment; users hand-write equivalents.

---

### Phase 5 — Threading + async/await · est. 4–5 weeks

The concurrency floor. Atomics + 1:1 OS threads + cross-thread `move` ship first (5A–5D); async/await via LLVM coroutines (5E) builds on top once threading is battle-tested. Ergonomic concurrency utilities (`Mutex`, `Channel`, `Arc`) are explicitly deferred to v0.0.4 — they cluster naturally (`Arc[Mutex[T]]` is the canonical shape and they're load-bearing together or not at all), and shipping them alongside async would expand v0.0.3's review surface beyond what one milestone can absorb.

**Locked decisions:**
- **1:1 threading only.** OS thread per `thread::spawn`. No M:N, no green threads. Per [research.md](research.md) Part 3, M:N is incompatible with C+'s zero-runtime + FFI-compatibility goals.
- **`JoinHandle[T]::join` returns `T`.** The cornerstone of safe threading without shared memory. The thread function's return value lives on the `JoinHandle`; `join` consumes the handle (`move self`) and transfers ownership of the result back to the parent. This single API choice makes the canonical "split work, join results" pattern fully safe — no `unsafe`, no raw pointers, no shared state. Worked example below in Slice 5B.
- **No `Send` / `Sync` marker traits, and therefore no shared-ownership types in v0.0.3.** Without `Send`, cross-thread sharing of arbitrary types is unsound. Today C+ has no `Rc[T]` / `Arc[T]`, so "raw pointers are the only aliasing mechanism, and raw pointers are `unsafe`-only" makes the threading floor sound by accident. Adding any shared-ownership type forces the `Send`/`Sync` design — that question parks until v0.0.4 alongside `Arc[T]` itself. **This is a hard contract: v0.0.3 does not ship `Rc[T]`, `Arc[T]`, or any user-visible shared-ownership type.**
- **Atomics are compiler intrinsics with a stdlib wrapper.** Per [project_cplus_7heap_reframe.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/project_cplus_7heap_reframe.md)'s library-not-language principle, the `Atomic[T]` ergonomic wrapper lives in stdlib; the intrinsic calls (`__cplus_atomic_load_i32` etc.) live in the compiler because they need direct LLVM access.
- **Async/await via LLVM coroutines.** [research.md](research.md) Part 2 Option B. Aligns with C+'s "LLVM does the heavy lifting" architecture. Coroutine frame allocation hardcodes `malloc` for v0.0.3 (no first-class `Allocator` interface yet); revisit when the allocator question becomes load-bearing.
- **`await` is prefix, not postfix.** `await expr` matches the dominant non-Rust convention (Python, JS, C#, Swift) and reads cleanly for the AI-first audience. Postfix `.await` is a Rust quirk tied to `?`-chain ergonomics that C+ doesn't share. The AI-first principle of "match agent priors" lands on prefix here because four-of-five mainstream async languages use it.
- **Single-threaded executor only.** Mirrors tokio's `current_thread` runtime. Tasks cannot migrate threads, which sidesteps both the `Send` requirement and the Mutex requirement. Multi-threaded executors are v0.0.5+ territory.

#### Slice 5A — LLVM atomic intrinsics · est. 3 days

**Goal:** Wire LLVM's atomic ops (`atomicrmw`, `cmpxchg`, `load atomic`, `store atomic`) into codegen. Memory orderings as a compiler-known enum.

**Compiler intrinsics:**
- `__cplus_atomic_load_i{8,16,32,64}(ptr, ordering) -> iN`
- `__cplus_atomic_store_i{8,16,32,64}(ptr, value, ordering)`
- `__cplus_atomic_fetch_add_i{8,16,32,64}(ptr, value, ordering) -> iN` (and `sub`, `and`, `or`, `xor`)
- `__cplus_atomic_cmpxchg_i{8,16,32,64}(ptr, expected, desired, success_ord, failure_ord) -> (iN, bool)`
- Same for `*T` (treated as `i64` on 64-bit targets).

**Ordering enum:** mirrors LLVM's `monotonic` / `acquire` / `release` / `acq_rel` / `seq_cst`. Compiler-known type with five named variants users import from `stdlib/atomic`. Discriminants are compiler-magic so codegen can lower them.

**Tests:**
- Unit: each intrinsic emits the expected LLVM op + ordering keyword in IR.
- E2E: two threads share a `*u64` via the Slice 5B/5C move API and increment it 100k times each via `__cplus_atomic_fetch_add_i64`; final value is exactly 200k. This is the worked Example 2 from the design discussion.

**Exit:** Atomic intrinsics work end-to-end on every integer width. The "two threads, shared counter, correct via atomics" recipe lands as a stdlib unit test.

**Shipped 2026-05-16:**

- **Intrinsic name shape.** Per-(op, type, ordering) compiler intrinsics — `__cplus_atomic_<op>_<ty>_<ord>` — rather than ordering-as-runtime-argument. Reason: LLVM ordering keywords are static; passing the ordering as a value would force every call site to be a known constant at codegen time, which compounds C+'s current lack of const-fold-through-functions. Baking ordering into the name keeps codegen mechanical and pushes the runtime dispatch into the stdlib wrapper (a `match ord {...}` that the optimizer flattens whenever the ordering is statically known). Parser lives at [cplus-core/src/atomic.rs](cplus-core/src/atomic.rs) (8 unit tests).
- **Sema.** Added in [cplus-core/src/sema.rs:3344](cplus-core/src/sema.rs#L3344): the name pattern is recognised before the `fns.get(name)` lookup, sema validates arg count + the first arg is `*T` for the operand type + every other arg is `T`, and enforces `unsafe { ... }` (E0801 — atomic ops read/write through a raw pointer whose validity the compiler can't prove). Return type is `T` for load/xchg/fetch_*/cmpxchg, `()` for store.
- **Codegen.** Added in [cplus-core/src/codegen.rs:4445](cplus-core/src/codegen.rs#L4445): each op lowers to its LLVM instruction directly — `load atomic <ty>, ptr <p> <ord>, align <bytes>` for load, `store atomic <ty> <v>, ptr <p> <ord>, align <bytes>` for store, `atomicrmw <opcode> ptr <p>, <ty> <v> <ord>` for xchg + fetch_{add,sub,and,or,xor}, `cmpxchg ptr <p>, <ty> <expected>, <ty> <desired> <succ> <fail>` + `extractvalue { <ty>, i1 }` for cmpxchg. Ordering keywords: `relaxed`→`monotonic`, `acquire`/`release`/`seqcst` direct, `acqrel`→`acq_rel`. Cmpxchg failure ordering is derived from success ordering to satisfy LLVM's "failure-ord ≤ success-ord and ≠ release/acq_rel" rule (release-success → monotonic-failure; acq_rel-success → acquire-failure).
- **Cmpxchg return shape.** Returns the *previous* value at `*p` (not a `(prev, ok)` pair). Compare against `expected` to detect success. Simpler than C+'s current limited tuple support and matches the C++ `compare_exchange_strong` weak-mode behaviour informally — callers can re-check.
- **Widths.** i8/i16/i32/i64 and u8/u16/u32/u64 all wire through the same parser. Raw-pointer width (mentioned in the plan as "Same for `*T` treated as `i64` on 64-bit targets") deferred — no use case in 5B/5C/5D and trivially layerable later by extending the type list in `cplus-core/src/atomic.rs`.
- **Stdlib wrapper.** [vendor/stdlib/src/atomic.cplus](vendor/stdlib/src/atomic.cplus) — 318 lines. `pub enum Ordering { Relaxed, Acquire, Release, AcqRel, SeqCst }`. Free fns on raw pointers (not an `Atomic[T]` struct): `atomic_{load,store,swap,fetch_add,fetch_sub,fetch_and,fetch_or,fetch_xor,compare_exchange}_<ty>(p, [val,] ord)`. Each fn body is a 5-arm `match` dispatching to the per-ordering intrinsic. Reason for free fns vs struct wrapper: a generic `Atomic[T]` would need width-dispatched method bodies (each width calls a different intrinsic), compounding the v0.0.3 generic-method limitations. Concrete widths shipped: i32, i64, u32, u64. i8/i16/u8/u16 deferred (no demand in 5B/5C/5D recipes; intrinsic level supports them already).
- **Tests:** 8 atomic parser unit tests + 9 codegen unit tests in [cplus-core/src/codegen.rs:7407](cplus-core/src/codegen.rs#L7407) (load/store/fetch_add/fetch_or-relaxed-monotonic/xchg/cmpxchg/cmpxchg-release-monotonic-failure-ord + unsafe-required negative + wrong-ptr-type negative). Two e2e tests in [cpc/tests/e2e.rs:5899](cpc/tests/e2e.rs#L5899): `stdlib_atomic_round_trips` (10 round-trip assertions covering every public op + both cmpxchg branches) and `stdlib_atomic_ir_contains_every_ordering` (asserts the merged-IR output names every LLVM ordering keyword). All 1147 workspace tests green (302 cpc e2e + 4 cpc-bindgen + 11 cpc-lsp + 830 cplus-core lib).
- **Deferred to Slice 5D (concurrent-counter recipe).** A two-thread fetch_add stress test demonstrating 200k round-trips under TSan. Today the threading primitives don't exist yet (5B), so the multi-thread variant of the recipe lands when 5B is in. Single-threaded round-trip tests are the Slice 5A exit metric, and they're green.

#### Slice 5B — `thread::spawn` with value-returning `join` · est. 4 days

**Goal:** Land the cornerstone API: `thread::spawn[O](f) -> JoinHandle[O]` and `JoinHandle[O]::join(move self) -> O`. Pure pthread wrapper. macOS + Linux; Windows deferred.

**Surface:**

```cplus
pub fn thread::spawn[O](f: fn() -> O) -> JoinHandle[O];
pub fn JoinHandle[O]::join(move self) -> O;
```

**Why this shape:** the value-returning `join` is what makes safe split-work-join-results patterns possible without shared memory. The worker thread returns owned output through `join`; the parent never aliases the worker's memory; the borrow checker proves race-freedom mechanically. `move self` on `join` means a handle can only be joined once and dropping it un-joined detaches the thread (no double-join footgun, no silent value loss).

**Worked example — parallel sum, fully safe:**

```cplus
import "stdlib/io" as io;
import "stdlib/thread" as thread;

struct Range { start: i64, end: i64 }

fn sum_range(move r: Range) -> i64 {
    let mut total: i64 = 0;
    let mut i: i64 = r.start;
    loop {
        if i >= r.end { return total; }
        total = total + i;
        i = i + 1;
    }
}

pub fn main() -> i32 {
    let left  = Range { start: 1,      end: 500001  };
    let right = Range { start: 500001, end: 1000001 };

    let h1 = thread::spawn_with(left,  sum_range);
    let h2 = thread::spawn_with(right, sum_range);

    let total: i64 = h1.join() + h2.join();
    io::println("sum 1..=1000000 = ${total}");   // 500000500000
    return 0;
}
```

No `unsafe`, no `malloc`, no raw pointers. `left` and `right` are `move`d into their threads at the spawn site (parent loses ownership at that line); the `i64` results return back through `join`. The borrow checker proves race-freedom. (Uses `spawn_with` from Slice 5C; the no-input form is `thread::spawn`.)

**Implementation notes:**
- `JoinHandle[O]` heap-allocates a small `(pthread_t, *O, done_flag)` triple. The thread's start function writes the return value into the heap slot before signaling done.
- `join` calls `pthread_join`, reads the heap slot (moves out), frees the triple, returns the value.
- Drop on un-joined `JoinHandle`: call `pthread_detach`; the worker's return value drops on the worker thread when the start function exits.
- Non-Copy `O`: heap slot stores the value; `join`'s read transfers ownership cleanly.
- **Panic semantics: aborts the process** — C+ has no unwind-on-panic story (panics are `llvm.trap`-shaped in v0.0.x), so a worker that traps takes the whole process with it. `JoinHandle::join` therefore returns `T`, not `Result[T, ThreadPanic]`. Document this prominently in the stdlib API: agents arriving with Rust priors expect `Result`; the honest signature is `T` and the docstring explains why.

**Tests:**
- Unit: spawn returning `i32`, `string`, `Vec[i32]` — value round-trips through join byte-equal.
- Unit: detach path — drop the handle without joining; ASan-clean.
- E2E: the parallel-sum example above lands as a recipe at [docs/examples/recipes/parallel_sum/](docs/examples/recipes/parallel_sum/) with a CI smoke test asserting the output is `500000500000`.

**Exit:** Threads spawn, run, return values through `join`. ASan-clean, TSan-clean. Parallel-sum recipe shipped.

**Shipped 2026-05-16 (Copy-only scope; non-Copy O slides to 5C):**

- **Compiler intrinsics** rather than the plan's "pure pthread wrapper" framing. Pure-stdlib turned out infeasible without first landing generic-fn-as-fn-pointer (rejected by E0821 today): pthread_create's start_routine must be a per-O function pointer, which can't be derived from a generic fn in source. The intrinsics `__cplus_thread_spawn::[O](f)` and `__cplus_thread_join::[O](h)` synthesise the per-O trampoline at codegen time and lower the spawn/join sequence inline. Lives at [cplus-core/src/codegen.rs:gen_thread_spawn](cplus-core/src/codegen.rs) and `gen_thread_join`. Sema in [cplus-core/src/sema.rs::check_thread_intrinsic](cplus-core/src/sema.rs) — `unsafe { ... }` required (E0801), `__cplus_thread_spawn::[O](f)` validates `f: fn() -> O` and returns `JoinHandle[O]` looked up from the stdlib's generic struct template, `__cplus_thread_join::[O](h)` validates `h: JoinHandle[O]` and returns O. Both intrinsics use turbofish — placed before the "non-generic fn with turbofish → reject" gate in sema so the turbofish type arg actually reaches my handler.
- **Stdlib at [vendor/stdlib/src/thread.cplus](vendor/stdlib/src/thread.cplus)** — 60 lines. `pub struct JoinHandle[O] { tid: u64, ctx: *u8 }` + `pub fn spawn[O](f: fn() -> O) -> JoinHandle[O]` + `impl JoinHandle[O] { pub fn join(move self) -> O; fn drop(mut self) }`. Bodies are one-line forwarders to the intrinsics.
- **Per-O trampoline emission.** [codegen.rs::ThreadTrampolines](cplus-core/src/codegen.rs) tracks a `RefCell<HashSet>` of unique O types encountered during function-body codegen; after all bodies emit, [emit_thread_trampolines](cplus-core/src/codegen.rs) writes one `define internal ptr @__cplus_thread_tramp_<mangle(O)>(ptr %arg)` per type. The trampoline loads f at ctx[0], calls it (with the O-typed call signature LLVM needs), stores the result at ctx[8] (with the natural alignment for O), and returns null. Unit-typed O skips the result store. Modeled on `ModuleMetadata`'s emit-at-end pattern.
- **pthread externs.** `pthread_create` + `pthread_join` declared in the codegen preamble (used by my generated calls). The user-facing stdlib re-declares `pthread_join` for its Drop impl; codegen pre-seeds the extern-symbol dedup set with the preamble names so the duplicate declares don't collide at link time. Same dedup applies to malloc/free/memcpy/printf/snprintf for future user-level re-declarations.
- **pthread_t as `i64`.** Both arm64-darwin and x86_64-sysv pass 8-byte values in a single integer register regardless of nominal C type (opaque struct ptr on darwin, unsigned long on linux). Declaring it as i64 keeps codegen platform-agnostic.
- **Drop blocks via pthread_join, not pthread_detach.** Original plan was detach+free; that races (worker still reading f out of the ctx the parent is freeing). Refcounting the ctx is the canonical fix, but lands in 5C alongside the cross-thread `move` work which already needs synchronisation. Until then, dropping an un-joined `JoinHandle` blocks at the drop point until the worker exits. Documented in the stdlib doc-comment — agents expecting Rust's fire-and-forget semantics need to read it.
- **`move self` plumbing.** `__cplus_thread_join` is registered in `scan_moves_in_expr` as consuming its first arg, so when sema sees `__cplus_thread_join(handle)` and the arg is an `Ident`, the surrounding function's scope-exit drop gets a real flag the intrinsic codegen can flip via `mark_moved`. Without this, the joiner's drop would double-free the ctx.
- **Scope reduction: Copy O ≤ 8 bytes only.** Eligibility check in [codegen.rs::is_thread_spawn_eligible](cplus-core/src/codegen.rs) — `i8..i64`, `u8..u64`, `isize`/`usize`, `f32`/`f64`, `bool`, `Unit`. Non-Copy O (string, Vec[T], owned aggregates) needs the sret-aware trampoline + join-side memcpy path; that machinery rolls into Slice 5C alongside the cross-thread `move` for non-Copy *input*. Raw pointers + fn pointers also deferred — they'd need a recursive mangler that names the inner pointee type, which codegen doesn't expose. Restricting to scalars keeps the v0.0.3 surface ~150 lines of codegen.
- **Why `pthread_t` flows through a `u64` slot.** pthread_create writes the thread id into a caller-allocated pointer. The handle stores the value directly (not the pointer). My codegen mallocs a tiny 8-byte slot, hands the ptr to pthread_create, loads the i64 out, frees the slot — that's the simplest workaround for C+ lacking address-of on locals. Cheap (single 8-byte allocation per spawn, paired with a free).
- **Tests:**
  - 7 codegen unit tests in [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs): pthread_create + trampoline emission for i64; pthread_join + GEP + load + free for i64; bool trampoline aligns to 1 byte; preamble declares pthread_create/pthread_join; spawn outside `unsafe` → E0801; join outside `unsafe` → E0801; spawn without turbofish → E0501. Required a new `gen_src_mono` helper that runs sema's `check_multi_with_mono` + `monomorphize::monomorphize` before codegen (generic structs would otherwise hit `TypeKind::Generic` in codegen).
  - 2 e2e tests in [cpc/tests/e2e.rs](cpc/tests/e2e.rs): `stdlib_thread_spawn_join_round_trip` (i64 / i32 / u64 / bool all round-trip through join byte-equal); `stdlib_thread_drop_detaches_unjoined_handle` (drop an un-joined handle under `--asan`, assert no Leak/Address sanitizer reports).
  - 1 recipe e2e test: `recipe_parallel_sum_runs` — copies [docs/examples/recipes/parallel_sum/](docs/examples/recipes/parallel_sum/) to a tempdir, links the stdlib in, builds, runs, expects exit-0. Recipe sums `1..=1000` across two threads (uses Copy `i64`, not `i64` overflow-territory). Workspace test count went from 1147 → 1157.
- **TSan:** not yet exercised; deferred to Slice 5D (concurrent-counter recipe shares a `*u64` between threads — that's the canonical TSan stress).
- **Deferred to Slice 5C:**
  - Non-Copy `O` (string / Vec[T] / owned aggregates) via sret-aware trampoline + join.
  - True fire-and-forget detach via refcounted ctx (so Drop doesn't block).
  - Raw/fn-pointer `O` via recursive type-name mangling.
  - x86_64 + Linux parity. macOS-only today; pthread is in libSystem on darwin, Linux needs `[link] libs = ["pthread"]` in the consumer's manifest (the consumer's, not stdlib's, because stdlib's Cplus.toml currently lacks a `[link]` section — adding one is a 5C task).
  - TSan smoke test pairing with the concurrent-counter recipe.

#### Slice 5C — Cross-thread `move` for non-Copy input · est. 2 days

**Goal:** `thread::spawn_with[I, O](move input: I, f: fn(move I) -> O) -> JoinHandle[O]`. Generic over both input and output. The monomorphizer makes a fresh copy per `(I, O)` pair the program uses; per the no-mangled-names rule, users write `thread::spawn_with(r, sum_range)` and the compiler instantiates internally.

**Implementation:** input lives in the same heap triple as Slice 5B's output slot; the worker thread reads it before calling `f`, then writes the result into the output slot. Drop ordering: if `pthread_create` fails (nonzero return), the input drops on the parent's frame; if the worker runs to completion, output is owned by the heap slot until `join` (or drops in the worker on detach).

**Tests:**
- Unit: move a `Vec[i32]` into a worker; worker computes a sum and returns it; the original Vec is no longer accessible in the parent (compile-fail test).
- Unit: move a string in, return a different string out; both transfers race-free.
- E2E: the parallel-sum recipe from 5B uses `spawn_with`; full round-trip.

**Exit:** Non-Copy types cross the thread boundary cleanly in both directions. Borrow checker rejects post-move use in the parent.

**Shipped 2026-05-16 (input side; non-Copy O slides to 5D):**

- **Intrinsic.** `__cplus_thread_spawn_with::[I, O](input, f)` joins `__cplus_thread_spawn` / `__cplus_thread_join` as a third compiler intrinsic, lowered inline at codegen time. Sema in [cplus-core/src/sema.rs::check_thread_spawn_with](cplus-core/src/sema.rs) — two turbofish type args (I, O) + two value args (input, f), `unsafe` required, returns `JoinHandle[O]` looked up from the stdlib generic template. The input arg is checked with `move_: true` so sema marks the source binding as moved at the spawn site.
- **Ctx layout.** Extended Slice 5B's `[fn_ptr][result]` layout to `[fn_ptr][result][input]`. Putting input *after* the result slot is the load-bearing decision: it keeps the result at the fixed offset 8 regardless of which spawn flavour was used, so `__cplus_thread_join` stays single-shape. Input offset = `8 + size_of(O)`, rounded up to `align_of(I)` (so typed stores stay in bounds for under-aligned types). See [codegen.rs::emit_spawn_with_tramp](cplus-core/src/codegen.rs).
- **Per-(I, O) trampoline registry.** Extended `ThreadTrampolines` (5B) to carry an enum `TrampolineSpec::Spawn { o } | SpawnWith { i, o }`. Spawn trampolines keep the named-suffix symbol shape (`__cplus_thread_tramp_<O_suffix>`); spawn_with trampolines use indexed names (`__cplus_thread_tramp_with_<N>`) because mangling struct/string types into the symbol would require recursive name-builders the codegen doesn't expose yet. Deterministic across runs — registration order matches insertion order.
- **`check_arg_with_move` plumbed through `check_generic_named_call`'s turbofish path.** Pre-existing limitation surfaced by 5C: when a generic fn was called with explicit turbofish (`fn[T1, T2](...)`), sema bypassed the move-tracking that the inference path used. Without the fix, `thread::spawn_with::[string, i64](s, f)` silently accepted post-move use of `s` because the stdlib's `move input: I` flag wasn't being honoured for turbofish calls. The fix at [sema.rs:3624](cplus-core/src/sema.rs#L3624) walks `gsig.params` looking for `move_: true` and marks the source binding as moved when the arg is a plain `Ident`. Non-Ident args (struct literals, enum-variant paths, fresh `Call` results) are silently skipped — they construct the value in place, no binding to track. The strict `consume_arg_place` E0337 path would have fired here and broken existing stdlib code like `io_ok::[File](File { fd: fd })`, so the new code inlines the Ident-only logic.
- **Input eligibility (`is_thread_input_eligible`).** Wider than spawn's output eligibility: accepts the scalar set + Copy structs (Range-style) + non-Copy aggregates (string, slices, Copy/non-Copy structs). The trampoline's typed load/store works uniformly for fat-pointer aggregates (string is 24-byte `{ptr, len, cap}`; slice is 16-byte `{ptr, len}`) — they go through the same `load <ty>, ptr <slot>` / `store <ty>, ...` shape as scalars. Ownership transfer for non-Copy I lives at the call-site `mark_moved` plus sema's binding-moved flag.
- **`scan_moves_in_expr` extension.** Like `__cplus_thread_join` (5B), `__cplus_thread_spawn_with` is registered as consuming its first value-arg in codegen's move-scan so the surrounding scope-exit drop gets a real flag the intrinsic codegen can flip. Combined with the sema-level `move` tracking, this makes the parent's `let s = ...` drop disappear at the spawn site cleanly.
- **Stdlib `spawn_with[I, O]`.** Two-line forwarder in [vendor/stdlib/src/thread.cplus](vendor/stdlib/src/thread.cplus): `pub fn spawn_with[I, O](move input: I, f: fn(I) -> O) -> JoinHandle[O] { return unsafe { __cplus_thread_spawn_with::[I, O](input, f) }; }`. The `move input: I` declaration is the user-visible enforcement point.
- **`parallel_sum` recipe rewritten.** [docs/examples/recipes/parallel_sum/](docs/examples/recipes/parallel_sum/) now uses `Range { start: i64, end: i64 }` (Copy struct) + `spawn_with::[Range, i64]` per the plan's worked example. Replaces the 5B-era two-fn variant (sum_lo + sum_hi). Builds, runs, returns 500_500.
- **Tests:**
  - 6 codegen unit tests in [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs): pthread_create + indexed trampoline for `(i32, i32)`; input slot at offset 12 (i32 result + i32 input case); input slot at offset 16 (i64 result + i64 input case); distinct `(I, O)` pairs get distinct indexed trampolines; outside-`unsafe` rejected (E0801); wrong type-arg count rejected (E0501).
  - 3 e2e tests in [cpc/tests/e2e.rs](cpc/tests/e2e.rs): `stdlib_thread_spawn_with_round_trip` (Range struct + string input both joined); `stdlib_thread_spawn_with_string_input_asan_clean` (ASan-clean string round-trip — worker takes ownership and drops); `stdlib_thread_spawn_with_post_move_use_rejected` (post-move use of moved `string` rejected by sema with E0335). Workspace 1157 → 1166.
- **Deferred to Slice 5D:**
  - Non-Copy O (string / Vec[T] / aggregates) for both spawn and spawn_with. Trampoline needs sret-aware return handling; join needs to memcpy through the caller's sret slot.
  - Vec[T] specifically — the same scalar-O restriction Slice 5B inherited still applies to spawn_with's output side.
  - Concurrent-counter recipe + TSan stress (the 5D scope from the plan).
  - Linux parity for thread + atomic intrinsics (carries from 5B).
  - True fire-and-forget detach via refcounted ctx (Drop still blocks via pthread_join).

#### Slice 5D — Concurrent-counter recipe + the "when you need atomics" case · est. 1 day

**Goal:** Ship two reference recipes under [docs/examples/recipes/](docs/examples/recipes/) that together teach the atomics story:
1. [parallel_sum/](docs/examples/recipes/parallel_sum/) (from Slice 5B) — the safe pattern; no shared state.
2. [concurrent_counter/](docs/examples/recipes/concurrent_counter/) — the unsafe pattern; shared `*u64` + atomics. Demonstrates *when* `unsafe` + atomics is the right tool (partitioning the work isn't possible) and what the contract looks like.

The pair is the documentation for "how does threading work in C+" and replaces the prose answer. Per Phase 1's Slice 3B finding, near-complete recipes outperform documentation.

**Tests:** both recipes have CI smoke tests.

**Exit:** SKILL.md §10 ("When in doubt") adds an entry pointing users at these two recipes for concurrency.

**Shipped 2026-05-16:**

- **[docs/examples/recipes/concurrent_counter/](docs/examples/recipes/concurrent_counter/)** — the unsafe pattern. Two workers, shared `*u64`, `atomic_fetch_add_u64(p, 1, SeqCst)` 100_000 times each → final value exactly 200_000. Includes the prose about when atomics are the right tool (partitioning impossible) vs. when parallel_sum's no-shared-state pattern wins (almost always). Ships as a complete `cpc build` project — vendor-link the stdlib in the Cplus.toml.
- **Raw + fn pointers added to `is_thread_input_eligible`.** Slice 5C accepted scalar / aggregate inputs; raw pointers `*T` were missing because they need recursive-mangle support in `mangle_o_for_tramp`. For the input side, spawn_with uses *indexed* trampoline names (not type-mangled), so the recursive-mangler isn't needed — flipping the eligibility check was enough. See [codegen.rs::is_thread_input_eligible](cplus-core/src/codegen.rs).
- **Monomorphizer fix in `subst_type_ast`** ([monomorphize.rs:563](cplus-core/src/monomorphize.rs#L563)). Pre-existing hole that 5D's `*u64` type argument exposed: when a generic body said `__cplus_thread_spawn_with::[I, O](...)` and I was substituted with `Ty::RawPtr(Ty::U64)`, the old code wrote `TypeKind::Path(type_name_of(concrete))` which produces `Path("raw-pointer")` (Ty::name()'s display string). Codegen's `ty_from("raw-pointer")` then returned `Ty::Error` and `llvm_ty` panicked. The fix routes non-primitive Tys through `ty_to_type_ast` which rebuilds the right AST structure (`TypeKind::RawPtr(inner)`, `TypeKind::Slice(inner)`, `TypeKind::FnPtr { params, return }`, `TypeKind::Array { elem, len }`). Affects only the turbofish-type-substitution path in generic-fn bodies; no other call site relied on the broken behaviour.
- **SKILL.md §10 entry.** Under "Read a recipe" — explicit sub-bullet pointing readers at parallel_sum (safe pattern) vs. concurrent_counter (unsafe-pattern reserve case). Directs the decision to "almost always use parallel_sum"; atomics belong in the rare cases where they're the only tool.
- **Tests:** `recipe_concurrent_counter_runs` in [cpc/tests/e2e.rs](cpc/tests/e2e.rs) — copies recipe → vendor-link stdlib → build → run → expect exit-0. Pairs with the existing `recipe_parallel_sum_runs` from 5C. Workspace 1166 → 1167.
- **TSan + ASan: actually wired up.** Slice 5D follow-up uncovered a real bug: `cpc build` was silently dropping the `--asan` / `--tsan` / `--ubsan` / `--msan` flags. The single-file path (`compile_file`) plumbed sanitizers correctly; the project-build path (`build_project`) hardcoded `&[]` for both codegen options and the clang invocation. Result: every "ASan-clean" claim in earlier slice notes was vacuous — the binary linked without sanitizer runtimes and reported clean by default. Fix: forward `sanitizers` through `build_project` to both `codegen::generate_with_options` and `run_clang`. ([cpc/src/main.rs:780](cpc/src/main.rs#L780), [:819](cpc/src/main.rs#L819))
- **Codegen sanitizer-attr placement fix.** Enabling TSan exposed a second bug: `attach_sanitizer_attrs` attached the `sanitize_thread` keyword after the *first* `)` in a `define` line, which landed it inside a nested `sret(%T)` parameter attribute. Symptom: `error: this attribute does not apply to parameters`. Fix: track paren depth and attach after the *params'* closing paren (depth 0). ([cplus-core/src/codegen.rs::attach_sanitizer_attrs](cplus-core/src/codegen.rs))
- **TSan-as-canary tests** added: `recipe_concurrent_counter_tsan_and_asan_clean` builds the recipe under both sanitizers and asserts stderr contains no warnings; `racy_counter_provokes_tsan_warning` deliberately swaps `atomic_fetch_add_u64` for `*counter +%= 1` and asserts that TSan *does* flag the race — the "is the sanitizer actually on" canary that catches future silent-disable regressions. Two new e2e tests; total 1169.
- **Linux parity:** still deferred. macOS-only; pthread is in libSystem on darwin, and there's no `[link]` section in stdlib's Cplus.toml for `-lpthread` yet. The atomic intrinsics work on Linux already (LLVM's atomic ops are platform-agnostic at the IR level); only the pthread linking needs the manifest entry.

#### Slice 5E — `async`/`await` via LLVM coroutines · est. 2.5–3 weeks

**Goal:** Land async/await on top of the threading floor. Single-threaded executor; LLVM coroutine codegen; `kqueue`/`epoll` reactor for async I/O. Decision shape per [research.md](research.md) Part 2 Option B — the compiler emits `llvm.coro.*` intrinsics at `await` points and LLVM's middle-end (CoroEarly, CoroSplit passes) builds the state machine.

Five sub-slices, all gated on 5A–5D shipping clean. Each has its own est. listed inline. **Do not start until threading is battle-tested under TSan on at least one non-trivial program** (the parallel-sum recipe + a stress test under ASan/TSan are the gate).

**5E.1 — `async` / `await` parser + AST · est. 2 days.** Lexer tokens for `async` (function modifier) and `await` (postfix expression). Parser changes are mechanical. AST: `Function.is_async: bool` flag; new `ExprKind::Await(inner)`. No sema yet — just shape.

**5E.2 — `Future[T]` as a compiler-known interface · est. 3 days.** `async fn foo() -> T` typechecks as returning `Future[T]`. `Future[T]` is a compiler-known interface (same precedent as `ToString` from Phase 8 of v0.0.1) with a `poll(mut self) -> Poll[T]` method, where `Poll[T] = enum { Ready(T), Pending }`. Users can't implement `Future` by hand in v0.0.3 — the only way to construct one is via `async fn`. This keeps the surface tight.

**5E.3 — Coroutine codegen via `llvm.coro.*` · est. 1 week.** The meaty slice. At codegen time, an `async fn` lowers to a function that:
- Emits `llvm.coro.id` + `llvm.coro.begin` in its prologue (allocates the coroutine frame via `malloc` — hardcoded per locked decisions).
- Emits `llvm.coro.suspend` at each `await` point, with the success branch resuming and the fallthrough branch returning `Pending`.
- Emits `llvm.coro.end` + `llvm.coro.free` at the function return / drop path.
- The compiler emits a `poll` thunk that calls the resumable coroutine handle and returns `Ready(value)` or `Pending`.

LLVM's CoroSplit pass (which we already get for free at `-O2` via the existing clang pipeline) chops the function into the state machine. **Risk:** coroutine passes interact subtly with the alias scopes from Phase 1 Slice 1C and the `sret` widening from v0.0.3 Slice 3B. Test each combination explicitly.

**Generator-ready lowering note.** The CoroSplit output for an `async fn` and a (future) `gen fn` is structurally identical at the IR level — both are coroutines, differing only in their wrapper API (`Future::poll` returning `Poll[T]` vs `Iterator::next` returning `Option[T]`). The 5E.3 lowering must keep this seam clean: do *not* bake "this is awaited by an executor" assumptions into the IR emission. When v0.0.4 adds `gen fn`, the codegen path should reuse the same coroutine machinery and only change the wrapper-type generation. Concretely: factor out a `lower_coroutine(body, frame_layout, return_shape) -> LlvmIR` helper from the async-specific bits so v0.0.4 calls it with `return_shape = Iterator`.

**5E.4 — Borrow checking across `await` points · est. 4 days.** The sema slice. Any borrow held across an `await` must remain valid after the coroutine resumes — which means it must live in the coroutine frame, not on the stack of the caller of `poll`. The borrow checker already tracks borrow lifetimes; the addition is recognizing that `await` is a yield point and any live borrow at that point must be one whose owner *also* lives in the coroutine frame. This is mechanical given the existing `MoveDescriptor` machinery from v0.0.1 Phase 3J. New diagnostic: **E0900** "borrow held across `await` references stack memory that won't survive the suspend".

**5E.5 — Single-threaded executor + reactor in stdlib · est. 1 week.** New `vendor/stdlib/src/executor.cplus`. Public API:

```cplus
pub fn executor::block_on[T](f: Future[T]) -> T;
pub fn executor::spawn_local[T](f: Future[T]) -> TaskHandle[T];
pub fn executor::yield_now() -> Future[()];
pub fn TaskHandle[T]::join(move self) -> T;
```

`yield_now()` is the cooperative-yield primitive — `await executor::yield_now()` lets the executor schedule other tasks without blocking on any I/O. Matches tokio's API exactly so agents with that prior write it correctly. Internally it's a `Future[()]` that returns `Pending` on first poll (after registering a wake-on-next-tick) and `Ready(())` on second poll.

Internal: a single-threaded task queue (`Vec[Future[*]]`), a poll loop, and a reactor via `kqueue` (macOS) / `epoll` (Linux). The reactor exposes `pub fn executor::wait_readable(fd: i32) -> Future[()]` for async I/O primitives to build on. Network/file async wrappers (`TcpStream::read_async`, etc.) ship as part of this slice — extending the Phase 1 stdlib modules with async variants of their sync APIs.

**Worked example — async fetch:**

```cplus
import "stdlib/executor" as executor;
import "stdlib/net" as net;

async fn fetch(host: str, port: u16) -> Result[string, net::IoError] {
    let stream = await net::TcpStream::connect_async(host, port)?;
    await stream.write_all_async("GET / HTTP/1.0\r\n\r\n")?;
    let body = await stream.read_to_end_async()?;
    return Ok(body);
}

pub fn main() -> i32 {
    let body = executor::block_on(fetch("example.com", 80));
    // ...
    return 0;
}
```

**Tests for 5E (cumulative):**
- Unit: every parser shape (async fn, await expr, nested async, await in conditional).
- Unit: borrow-across-await E0900 diagnostic fires for the canonical broken patterns.
- E2E: the async fetch example above lands as a recipe at `docs/examples/recipes/async_fetch/`.
- E2E: 1000 concurrent async tasks on the single-threaded executor — round-trip cleanly.
- Negative: multi-threaded executor usage (spawning on one thread, awaiting on another) fails at the API level (not exposed).

**Exit for Phase 5:**
- All five threading + async slices ship with full unit + e2e + negative coverage per [feedback_test_discipline.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_test_discipline.md).
- Parallel-sum recipe + concurrent-counter recipe + async-fetch recipe all build and run in CI.
- A re-run of a `proves/`-class network benchmark (proposed: `proves/benchmark/programs/07-async-curl-lite`) demonstrates async I/O works end-to-end. Turn count + cost target: better than 04-curl-lite's stdlib-equipped baseline measured in Phase 1F.

**Shipped 2026-05-17 (5E.1 + 5E.2 + 5E.3 + minimal 5E.5 — compute-only async; reactor + I/O wrappers + borrow-across-await + ergonomic exits deferred):**

- **5E.1 surface.** Lexer added `Async` + `Await` tokens; parser threads `async` as an optional fn-modifier (`pub async fn` works); `await` is a unary-precedence prefix operator (`await foo().bar()` parses as `await (foo().bar())`). AST: `Function.is_async: bool` + `ExprKind::Await(Box<Expr>)`. fmt is token-stream-based and accepts the new keywords with no changes. 5 new parser tests covering all four shapes.
- **5E.2 sema.** `async fn foo() -> T` rewrites to expose `Future[T]` at the signature level (in both sema's FnSig and codegen's mirrored sigs) while the body type-checks `return X` against the *inner* T. `await EXPR` validates EXPR : `Future[T]` and yields T; fires **E0901** outside an `async fn` and **E0902** on non-Future operands. `Future[T]` template lookup matches both bare and file-qualified names (`stdlib/future`'s import-prefixed `Users.adel...future.Future` form). 6 new sema tests covering the type rule + both diagnostics + the "Future not in scope" case.
- **5E.3 coroutine codegen.** `gen_async_function` ([cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)) lowers each `async fn` to an LLVM coroutine using the switched-resume pattern: `llvm.coro.id` + `llvm.coro.begin` (malloc-backed frame), `return X` rewrites to "store X to `coro.promise` then `br .coro.final_suspend`", final-suspend's switch default returns the handle wrapped in a `Future[T]` aggregate to the caller. `gen_await_expr` drives the inner via a `coro.done`/`coro.resume` loop, extracts via `coro.promise` + load, then `coro.destroy`s the inner frame. `presplitcoroutine` attribute makes LLVM's CoroSplit pass run at every -O level. Result: chained `await` works (3-level confirmed); parameterized async fns work; multiple sequential awaits in one body work (`sum_squares(a, b)` example).
- **Minimal 5E.5: `executor::block_on[T](f) -> T`.** Stdlib's [vendor/stdlib/src/executor.cplus](vendor/stdlib/src/executor.cplus) forwards to a `__cplus_block_on::[T](future)` compiler intrinsic that emits the canonical `coro.resume`/`coro.done` loop + promise read + `coro.destroy`. Synchronous driver — no reactor, no I/O suspension; the future runs to completion in the calling thread before `block_on` returns.
- **[docs/examples/recipes/async_compute/](docs/examples/recipes/async_compute/)** — 3-level chained `async fn` recipe (`step_one`→`step_two`→`step_three`, summing `100+200+300 = 600`). Shipped + CI tested via `recipe_async_compute_runs` in [cpc/tests/e2e.rs](cpc/tests/e2e.rs). Workspace went 1180 → 1181.

**5E scope honestly deferred to v0.0.4 (DO NOT claim v0.0.3 ships these):**

- **No reactor (kqueue/epoll), no `spawn_local`, no `yield_now`.** v0.0.3's executor is pure busy-loop drive-to-completion. Async I/O wrappers (`TcpStream::read_async`, `File::read_async`, etc.) all need the reactor; none ship in v0.0.3. The async-fetch recipe and the 1000-concurrent-tasks test from the plan's exit criteria therefore do NOT land.
- **No real suspension semantics.** Inner futures always reach their final-suspend on first ramp call (no yield points), so `coro.done(inner)` is always true on the first iteration of the `await` loop. The IR has the suspend-self-on-pending branches wired but they're unreachable in v0.0.3 programs. The instant the reactor lands (v0.0.4), the existing IR shape supports real suspension.
- **No `Send`/`Sync` and no `Mutex`/`Channel`/`Arc`.** All four explicitly v0.0.4 per Phase 5 non-goals.
- **No multi-threaded executor.** Single-threaded `current_thread`-style only. v0.0.5+ territory.
- **No 5E.4 (borrow check across await; E0900).** The body checker walks `await`'s inner expression but doesn't enforce the "borrows held across await must live in the coroutine frame, not the caller's stack" rule. v0.0.3's no-real-suspension semantics make this a latent correctness gap rather than a present-day footgun — the runtime never actually suspends between an await and a resume — but enabling the reactor in v0.0.4 immediately exposes it. **Block v0.0.4's reactor on landing E0900.**
- **Non-Copy `T` for async fn returns.** Same Copy-only ≤ 8-byte restriction Slice 5B/5C inherit. Non-Copy returns (`async fn foo() -> string`) need sret-aware coroutine promise sizing; doable on the existing IR template but not in this slice.
- **Generic async fn over T.** Untested. Monomorphization probably works mechanically (the sema check_function path threads is_async through subst), but no e2e exists. v0.0.4 task.
- **Hand-rolled `Future` implementations.** v0.0.3 only allows `Future` construction via `async fn`. Users can't `impl Future for MyType { fn poll(...) }`. This stays as the v0.0.3 limit.

**The async/await surface ships. The async runtime does not.** Programs that compose `async fn` + `await` + `block_on` work today; programs that need actual concurrent I/O wait for v0.0.4's reactor.

### Phase 5 non-goals

- **`Mutex`, `Channel`, `Arc`, `Rc`.** Deferred to v0.0.4. They cluster (the canonical shape is `Arc[Mutex[T]]`) and shipping them load-bearing requires the `Send`/`Sync` design.
- **`Send` / `Sync` marker traits.** Parked until v0.0.4 alongside the shared-ownership types that force the question.
- **M:N scheduling, green threads, work-stealing.** Per locked decisions.
- **Multi-threaded executor.** Single-threaded `current_thread`-style only. Multi-threaded executor needs `Send` + Mutex equivalents — v0.0.5+.
- **Scoped threads with lifetime-checked shared borrows.** Substantial sema slice; parked until there's a use case.
- **Lock-free data structures.** Library-not-language; build on Slice 5A atomics in user code or future stdlib slices.
- **Thread pool / executor for OS threads** (different from the async executor in 5E.5). A user can roll their own with `spawn` + `JoinHandle`.
- **User-implementable `Future` interface.** v0.0.3 only allows `Future` to be constructed via `async fn`. Hand-rolled futures land when there's a real use case that `async fn` can't express.

### v0.0.4 carryover index — what v0.0.3 explicitly does NOT ship

Updated 2026-05-17 after Slice 5E. Every item below was intentionally deferred during a v0.0.3 slice; none of it should be advertised as "v0.0.3 ships". Items are grouped by the slice that surfaced the deferral so you can read the original rationale alongside each.

**From Phase 1 (stdlib):**
- Linux/x86_64 parity for stdlib (carried since v0.0.2 Phase 3C). macOS-only today; Linux needs `[link] libs = ["pthread"]` in stdlib's Cplus.toml + ABI verification for the `[2 x i64]` aggregate-coercion path. See Phase 5 follow-up below — same Linux gap.

**From Phase 5 Slice 5B (`thread::spawn` + `join`):**
- **Non-Copy `O` for `JoinHandle[O]::join`** (string / Vec[T] / owned aggregates). The trampoline needs sret-aware return handling; join needs caller-sret memcpy. Same scope shape as the v0.0.2 Slice 1P widening, applied to the spawn/join return path.
- **True fire-and-forget detach** via refcounted ctx. Today `JoinHandle::drop` blocks on `pthread_join`; the canonical Rust-style "drop = detach, thread keeps running" needs reference-counting on the heap context so the parent and worker can both safely release it. Deferred because the simple block-on-drop is honest and safe.
- **Linux parity for `pthread_create` / `pthread_join`** — needs the manifest `[link]` entry; tracked together with Phase 1's Linux gap.

**From Phase 5 Slice 5C (`thread::spawn_with[I, O]`):**
- **Non-Copy `O` for `spawn_with`** — same restriction as 5B inherits.
- **Raw / fn-pointer O via recursive type-name mangling.** Codegen's `mangle_o_for_tramp` only handles scalar primitives today; `Future__ptr_u8` style names would need a recursive builder. Slice 5D already lifted the input-side restriction by using indexed trampoline names; the output side still needs the mangler.

**From Phase 5 Slice 5D (concurrent counter):**
- Nothing new — all deferrals carry from 5B/5C.

**From Phase 5 Slice 5E (async/await — the big block):**
- **5E.4: borrow check across `await` (E0900).** Real correctness gap. Sema currently lets borrows live across `await` without enforcing "the borrow's owner must live in the coroutine frame, not the caller's stack". Latent in v0.0.3 because the runtime never actually suspends; **becomes a live footgun the moment the reactor lands in v0.0.4**. Treat 5E.4 as a hard precondition for the reactor.
- **The async reactor: kqueue (macOS) / epoll (Linux).** Without it, no async I/O. Required for `TcpStream::read_async`, `File::read_async`, sleep/timer futures, every realistic async use case.
- **`executor::spawn_local` and `executor::yield_now`.** Plan called for both in 5E.5; only `block_on` shipped. `yield_now` in particular is load-bearing for cooperative-multitasking patterns (it's how cancellation-aware async loops avoid starving the executor).
- **Async I/O wrappers** — every `*_async` variant on the stdlib's sync types (`TcpStream`, `TcpListener`, `File`, eventually `Process`). Each wraps the corresponding kqueue/epoll wait + the sync op.
- **`async_fetch` recipe.** The plan's worked example (`fetch(host, port) -> Result[string, IoError]`). Blocked on the reactor + async TcpStream wrappers + non-Copy T-as-return for `string`. Three layers of dependency; not a small follow-up.
- **The "1000 concurrent async tasks" exit test from the plan.** Same blocker — needs `spawn_local` and the reactor to mean anything.
- **Non-Copy `T` for async fn returns.** Same Copy-only restriction Slice 5B inherits — `async fn foo() -> string` doesn't work in v0.0.3.
- **Generic `async fn`** (`async fn foo[T](x: T) -> T`). Mechanically should work via monomorphization (sema threads `is_async` through `subst_type_ast`'s outputs) but no e2e exists. Verify in v0.0.4 alongside the reactor work.
- **Hand-rolled `Future` implementations.** v0.0.3 only allows `Future` construction via `async fn`. Users can't `impl Future for MyType { fn poll(...) -> Poll[T] }`. The user-facing `Poll[T]` enum is in stdlib but unreachable.
- **Send / Sync marker traits, `Mutex[T]`, `Channel[T]`, `Arc[T]`, `Rc[T]`** — all explicit non-goals per Phase 5's locked decisions. The async runtime in v0.0.4 will need at least some of these (`Arc` for the coroutine-frame refcount the reactor needs; `Mutex` once multi-task workloads share state).
- **Multi-threaded async executor.** Single-threaded `current_thread`-style only. v0.0.5+ territory.

**From the 2026-05-17 SCOPE NOTE on Slice 5E:** the async/await *surface* ships in v0.0.3 (parser + sema + LLVM coroutine codegen + `block_on`). The async *runtime* — the part that makes async useful for I/O concurrency — does NOT. Programs that compose `async fn` + `await` + `block_on` for CPU-bound chained coroutines work today; programs that need actual concurrent I/O wait for v0.0.4's reactor.

**Stdlib optimization gap (added 2026-05-17 after the 04-curl-lite measurement):** v0.0.3 stdlib is an MVP. The 04-curl-lite benchmark in [proves/stats.md](proves/stats.md) measures cplus-stdlib at +14.7% instructions over cplus libc-only — competitive with Rust's std (+19.4% over rust-libc) on that *specific* I/O-bound workload, **but the comparison flatters C+**. Two reasons: (1) Rust's `curl-lite` uses `format!` for request build, which drags `core::fmt`'s formatter machinery — a known-heavy Rust abstraction; C+ side builds via byte-by-byte `Vec[u8]::push`. (2) Curl-lite is syscall-dominated; whatever inefficiencies live in `Vec::push` or `Result` dispatch are amortised by the kernel cost between iterations.

**What v0.0.3 stdlib lacks vs Rust's std (concrete list, post-curl-lite audit):**

- **No `Vec::reserve` / `Vec::with_capacity`.** Every grow happens at push time. A `with_capacity(n) + push × n` workload does one alloc in Rust, ~log₂(n) reallocs in C+.
- **No `Vec::push_unchecked` / `Vec::extend_from_slice`.** Every push pays the bounds + grow check even when the caller can prove no grow is needed. Rust's `extend_from_slice` collapses to one `ptr::copy_nonoverlapping`; C+'s equivalent is N branchy `push`es.
- **No element-type specialization for `Vec`.** Rust's `Vec<u8>::extend_from_slice` lowers to a single `memcpy`; C+ has no path to that.
- **No SIMD primitives.** No `memchr`-equivalent, no SIMD byte-compare, no bulk-zero. Rust's std uses these in `Read::read_to_end`, `str::find`, `slice::contains`, etc.
- **No `Result::unwrap_unchecked`.** Every match goes through the full discriminant load + branch even when the user has already verified the variant. Hot-path `Result` handling pays the branch every iteration.
- **No iterators.** `gen fn` + `Iterator[T]` + `for-in` is the bundled v0.0.4 slice (see "Phase 5 forward-pointers" below). The compose-without-allocate pattern — Rust's `.iter().filter().map().collect()` — has no C+ equivalent today; users hand-write index loops.
- **HashMap is concrete `StrIntMap` only** (`str → i32`). No generic `HashMap[K, V]`, no hasher abstraction, no `entry()` API. Generic version is blocked on the `Hash[K]` interface that compounds the v0.0.3 generic-method limitations (per [vendor/stdlib/src/hash_map.cplus](vendor/stdlib/src/hash_map.cplus)'s doc comment).
- **No `Cow`, no `Box`, no `Arc`, no `Rc`.** All workhorse stdlib types in Rust. `Arc` is gated on Send/Sync (Phase 5 non-goal); `Cow` would be useful before that lands.

**Recoverable codegen vs honest abstraction cost:** of the 14.7% stdlib overhead, ~3–5 points are recoverable through bounds-check elision on monotonic `Vec::push`, inlining `Result::Ok`/`Err` discriminant branches into the happy path, and `Vec`-capacity-grow folding when the surrounding loop bounds the per-iteration count. The remaining ~10% is honest cost of bounds-checked vectors + tagged-union dispatch, same shape Rust pays for.

**v0.0.4 stdlib priorities (ordered by leverage):**

1. **`Vec::reserve` + `Vec::with_capacity`** — single biggest win for any non-trivial Vec workload. Trivial to ship.
2. **`Vec::extend_from_slice`** — collapses N pushes into a memcpy. Cascading win wherever stdlib builds buffers (request build, response read, fizzbuzz output).
3. **Iterators via `gen fn`** — already in the forward-pointers below. Unlocks compose-without-allocate.
4. **CPU-bound benchmarks in `proves/`** — 04-curl-lite doesn't exercise stdlib hot paths because it's syscall-dominated. Candidates: a "sum 1M i32s in a Vec" + a "parse 10MB CSV" + a "hashmap 100k entries". Without these we can't tell if stdlib is regressing.
5. **`Result::unwrap_unchecked` + match-inlining hints** — small wins, easy to ship.
6. **Generic `HashMap[K, V]` + `Hash[K]` interface** — unblocks the open `StrIntMap`-only API. Cross-module generic-method work continues to be the load-bearing piece.

The cplus → cplus-stdlib delta tracked in [proves/stats.md](proves/stats.md)'s 04-curl-lite section is the most useful self-benchmark going forward — same compiler, one abstraction layer added, instruction count as the signal. Watch it across v0.0.4+: drift up past 17% = stdlib slowed down; drop below 12% = codegen improvements landed.

### Phase 5 forward-pointers to v0.0.4

**Generators (`gen fn` + `Iterator[T]` + `for-in`)** — bundled slice for v0.0.4. Shares ~80% of v0.0.3's coroutine codegen machinery (5E.3); the marginal cost is the surface-language addition. Three pieces ship together because each is useless alone:

```cplus
gen fn count_up(n: i32) -> i32 {            // ← producer: gen fn keyword
    let mut i: i32 = 0;
    loop {
        if i >= n { return; }
        yield i;
        i = i + 1;
    }
}

pub interface Iterator[T] {                  // ← consumer interface
    fn next(mut self) -> Option[T];          //   compiler-known, like Future[T]
}

for x in count_up(10) {                      // ← for-in loop sugar
    io::println("${x}");                     //   desugars to loop+next+match
}
```

**AI-first design rationale:**
- `gen fn` matches Rust's converging-on-stable `gen` syntax and is symmetric with `async fn` — agents see the keyword and pattern-match immediately. Pythonic "yield-in-body, regular return type" was considered and rejected as too implicit (function-vs-generator distinction requires reading the body, not the signature).
- `await` is prefix → `yield expr` is also prefix-as-statement (not `yield;` followed by expr). Symmetry with async helps agents predict the grammar.
- `Iterator[T]` mirrors `Future[T]`: compiler-known interface, constructed only via `gen fn` in the v0.0.4 ship. Hand-rolled iterators land later when there's a use case.
- `for x in expr { body }` desugars to `let mut __it = expr; loop { match __it.next() { Some(x) => body, None => break } }`. Standard.

**Borrow-checker delta from async:** the caller's stack frame *does* persist across `yield` (because `next()` is a synchronous call from a live frame), unlike across `await`. So E0900's analysis splits into two rules: "no caller-stack borrows across `await`" and "caller-stack borrows OK across `yield` as long as they're from the immediate `next()` caller's frame." Different check; both needed in v0.0.4.

**Estimated slice cost (v0.0.4):** ~1 week — parser/AST for `gen fn` + `yield`, `Iterator[T]` compiler-known interface, `for-in` loop sugar in lowering, borrow-across-yield check. Most of the work is already done by v0.0.3's 5E.3 if the generator-ready note above is honored.

**Use cases this unlocks:** `vec.iter()`, `hash_map.iter()`, `file.lines()`, `range(0, 100)`, parser combinator chains. None blocking, but each turns 20-line hand-rolled iterator structs into 3–5 line `gen fn`s. Phase 1's stdlib polish would be lighter weight if generators landed first — but the scope risk of bundling them into v0.0.3 wasn't worth it.

---

## Resolved log

- **2026-05-17** — Phase 5 Slice 5E (async/await surface) shipped; runtime is compute-only (no reactor, no real suspension, no async I/O). Lexer + AST + parser + sema + LLVM-coroutine codegen + `executor::block_on` all land. Chained `async fn`+`await`+`block_on` works for primitive scalar T. async_compute recipe ships with CI smoke. 5E.4 (E0900 borrow-across-await), the reactor, async I/O wrappers, `spawn_local`, `yield_now`, non-Copy T, and hand-rolled Futures all explicitly roll to v0.0.4. Workspace at 1181.
- **2026-05-16** — Phase 5 Slice 5D follow-up (sanitizer plumbing actually wired): `cpc build` was silently dropping `--asan`/`--tsan` flags for the project-build path; codegen's `attach_sanitizer_attrs` mis-placed the function attr inside `sret(%T)` causing clang to error. Both fixed; TSan now real (proves clean on parallel_sum + concurrent_counter, flags a deliberate race in the canary test). Pre-fix `--asan` e2e tests in 5B and 5C were vacuously clean; they pass under real instrumentation too.
- **2026-05-16** — Phase 5 Slice 5D (concurrent-counter recipe + "when atomics are the right tool" prose) shipped. Two-recipe concurrency story now complete (`parallel_sum` for the safe pattern, `concurrent_counter` for the rare-but-necessary atomics pattern). Three small compiler changes en route: raw-pointer `*T` added to `is_thread_input_eligible`; `subst_type_ast` fixed for non-Path Tys (was producing `Path("raw-pointer")` for `*T` substitution); SKILL.md §10 amended. 1 new test (recipe e2e). Workspace at 1167. See the Slice 5D shipped block above.
- **2026-05-16** — Phase 5 Slice 5C (`thread::spawn_with[I, O]` for cross-thread move of non-Copy input) shipped. Third compiler intrinsic alongside spawn/join; ctx layout `[fn_ptr][result][input]` keeps join single-shape; turbofish path in `check_generic_named_call` now tracks `move` params (fixes a pre-existing hole that 5C exposed); parallel-sum recipe rewritten to use `spawn_with` with `Range` struct input. 9 new tests (6 codegen + 3 e2e). Workspace at 1166. Non-Copy O / TSan / Linux parity all roll to 5D. See the Slice 5C shipped block above.
- **2026-05-16** — Phase 5 Slice 5B (`thread::spawn` + value-returning `join`, Copy-only) shipped. Compiler intrinsics with per-O trampoline synthesis; stdlib `JoinHandle[O]` + `spawn[O]` + `join(move self)` + Drop; parallel-sum recipe under [docs/examples/recipes/parallel_sum/](docs/examples/recipes/parallel_sum/). 10 new tests (7 codegen + 2 e2e + 1 recipe). macOS-only; non-Copy O / fire-and-forget detach / Linux parity / TSan all roll to 5C. See the Slice 5B shipped block above.
- **2026-05-16** — Phase 5 Slice 5A (LLVM atomic intrinsics) shipped. Per-(op, type, ordering) compiler intrinsics + `vendor/stdlib/src/atomic.cplus` wrapper with `Ordering` enum + free fns for i32/i64/u32/u64. 17 new tests across cplus-core (8 parser + 9 codegen) and cpc e2e (2 round-trip + IR-keyword coverage). See the Slice 5A shipped block above for design notes.

## Next

Phase ordering is firm: 1 → 2 → 3 → 4 → 5. Within Phase 5, 5A–5D must ship and pass TSan before 5E (async/await) begins — the gate exists because async-on-flaky-threading is unisolatable. Phases 1–4 are independent and can ship in any order if priorities shift; Phase 5 depends on Phase 1 (stdlib types like `Vec[T]` are used in the recipes).

**Open questions for later** (do not block phase work):
- Whether v0.0.3 ships all five phases or whether Phase 5 (specifically the 5E async/await slice) rolls to v0.0.4. Decide once Phases 1 + 2 + 3 + 5A–5D land and we see real timelines on the coroutine codegen risk.
- Linux/x86_64 parity for stdlib (carried from v0.0.2 Phase 3C stretch).
- When to introduce `Send`/`Sync` and the first shared-ownership type (`Arc[T]`) — v0.0.4 is the working plan, conditional on the ergonomic-concurrency-utilities milestone landing then.
