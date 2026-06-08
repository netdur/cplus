# C+ changelog

User-facing changes per release, newest first. The changelog starts at v0.0.14;
earlier history lives in each version's archived plan.

## v0.0.18 — 2026-06-08

The owned string is now `Text` — a single, fully-stdlib string type — and the
compiler-blessed `string` is gone. One owned-string concept, with most of its
API living in the standard library instead of the compiler.

### Language — `Text` replaces `string` (breaking)
- **`string` is removed.** Source-level `string` (and `string::new` /
  `string::with_capacity`) now error with E0303. The owned, growable string is
  `Text`, implemented entirely in `vendor/stdlib/src/text.cplus` and recognised
  by one compiler lang-item (`#[lang("string")]`). `str` (the borrowed view) is
  unchanged.
- **Import-required.** A file that names an owned string or uses interpolation
  must `import "stdlib/text"`. Single-file programs that only need views use
  `str`. (`.to_string()` / interpolation still work without the import via type
  inference, producing an un-nameable owned value; to *name* the type, import
  `Text`.)
- **`Text` API** — all in stdlib, extensible without touching the compiler:
  `new` / `with_capacity` / `from_str`, `push_str` / `clear` / `truncate` /
  `clone`, `len` / `capacity` / `is_empty`, `find` / `rfind` / `contains` /
  `starts_with` / `ends_with`, `slice` / `trim*` / `split -> Vec[Text]`, the
  `unsafe as_str` borrow escape hatch, and `c_str -> Option[CString]` for the C
  ABI. `Text` is `Send + Sync` (usable as a `thread::spawn` payload and in
  `Arc[Text]`).
- **Multi-line string literals** `"""..."""` — verbatim: no indentation
  stripping, no escape processing; the bytes between the quotes are the value.
- String interpolation and `.to_string()` now produce an owned `Text`.

### Language — `unsafe fn`
- Functions can be declared `unsafe fn`; calling one outside an `unsafe { }`
  block is rejected (E0801). The grep-able escape hatch for operations whose
  safety the compiler can't verify (e.g. `Text::as_str`, raw FFI returns).

### stdlib + vendor
- Migrated off `string` to `Text`: stdlib `cow` / `fs`; vendor `json`, `appkit`
  (the Objective-C string bridge), `uuid`, and `agent_core`. The owned `Text`
  made the JSON deep-clone paths safe (`Text::clone()` instead of an
  `as_str().to_string()` round-trip), removing `unsafe` from them.

## v0.0.17 — 2026-06-07

Foundations: an ownership-safe `Vec`, a compiler soundness fix behind it, the
framework-agnostic core of the agent surface, and a scoped-down package manager.

### Compiler
- **`string` value-param soundness fix:** a `string` (or other owning value)
  passed by value and then *stored or forwarded* (e.g. `self.v.push(s)`,
  `self.field = s`) instead of returned is no longer double-freed. `effective_move`
  now covers `Ty::String` alongside `Ty::Struct`/`Ty::Enum`. Repro in
  `bugs/string-param-store-double-free/`. (Requires a `cpc` reinstall from source.)

### stdlib — `Vec` rewrite (breaking)
- `Vec` is now ownership-safe: overflow-checked allocation sizing, null-checked
  malloc/realloc, and **no silent out-of-bounds reads**.
- API changes: `get` is a bounds-checked `vec::get::[T](v, i) -> Option[T]`
  (Copy elements); `at_copy(i) -> T` asserts in-bounds; `at(i) -> Option[*T]`
  reads a non-Copy element in place; `pop` returns `Option[T]`; added `set`,
  `swap_remove`, `truncate`, `shrink_to_fit`, `is_empty`. `iter` stays a gen
  method. All in-tree callers (json, clap, agent_core) migrated.

### Package manager (new: `cplus-pm`)
- A standalone tool to **manage packages in a project's `vendor/`**:
  `install` / `remove` / `update`, with git-tag versioning, `pubgrub`
  resolution, SHA-256 content addressing, a shared cache, and a lockfile. No
  dependency on the compiler.

### Agent surface core (new: `vendor/agent_core`, groundwork)
- The framework-agnostic core for agent-controllable apps: the build-time-stable
  agent-id tree, curated `describe`, the all-or-none auth gate + exposure +
  affordance ceiling, bubbling events with `{node,verb,role}` subscriptions, and
  action/text-op authorization with optimistic-concurrency versioning. Headless
  and fully tested; the AppKit backend (GUI wiring) and MCP bridge are next.

## v0.0.16 — 2026-06-07

The AppKit surface: full binding coverage, a leak-free ownership model, and
event-driven drag-and-drop — plus a P0 calling-convention fix behind all macOS
geometry, and a loop Drop/move soundness fix.

### Language
- **`#` sigil for compiler builtins:** the FFI/raw and byte-swap builtins
  (`str_ptr`, `slice_ptr`, `slice_len`, `str_from_raw_parts`, `bswap32`,
  `htons`, …) and `println` now require the `#name(...)` form, like the existing
  `#size_of`/`#addr_of`. A bare call is a fix-it error. This makes a
  compiler-known builtin self-evident at the call site (the library `io::println`
  is unchanged).
- **Infinite `loop` diverges:** a function whose body ends in an infinite `loop`
  (no `break` can exit it) no longer needs a dead trailing `return`.
- **`let _ = expr;`** is now a discard binding (evaluates and drops the value).

### AppKit (vendor/appkit)
- **Event-driven drag-and-drop:** a drag *source* can now start a drag from a
  `mouseDragged:` gesture (`create_drag_source_view` + `begin_string_drag` /
  `DraggingItem` / `begin_dragging_session`), alongside the existing drop
  destination. See the `appkit_drag_drop` recipe.
- **Leak-free ownership:** every `alloc/init` widget wrapper now follows the
  "+1 normal form" (owns its object, releases once in `drop`) — controls, text,
  containers, toolbar items, panels, controllers, data views, and the base
  views. Factory/shared/top-level objects (windows, the app, status bar,
  shared panels, colors/fonts) correctly stay non-owned.
- **Full module coverage:** every vendor/appkit module now has tests.
- **`TextField::new_label`** is a real static label (non-editable, non-bezeled);
  it no longer behaves like an input field or accepts dropped text.

### Fixes
- **Struct-by-value ABI (P0):** `NSPoint`/`NSSize`/`NSRect` and other
  homogeneous float aggregates passed by value to `objc_msgSend` now go in FP
  registers per AAPCS64. Previously they were integer-coerced / passed
  indirectly, so every geometry argument (`setFrame:`, `initWithContentRect:`,
  `moveToPoint:`, …) silently received garbage coordinates on Apple Silicon.
- **Loop-body Drop:** an owned value created inside a `while`/`for`/`loop` body
  is now dropped at the end of each iteration (and on `break`/`continue`).
  Previously it leaked every iteration.
- **Move across loop iterations:** `let y = x;` on a non-Copy value now moves
  the source, and re-moving a binding declared outside a loop on each iteration
  is rejected (E0335) — previously an un-tracked move that, with the loop-Drop
  fix, would double-free. Re-initializing the binding before the move stays
  valid.
- **Negative float literals** no longer emit invalid IR (`double -5`).
- **`Slider`** value get/set used the wrong (float vs double) ABI; fixed to
  `doubleValue`/`setDoubleValue:`.

### Infra
- **macOS CI:** a `cargo test --workspace` job now runs on Apple Silicon
  (push-to-main + PRs), alongside the tag-triggered Linux and Windows CI.

## v0.0.15 — 2026-06-05

Language hardening, a P0 ownership fix, the first Linux and Windows ports, and
GPU/CPU BLAS bindings.

### Language
- **Module-level global asm:** `#asm("...")` at item scope lowers to LLVM
  `module asm`, for raw module-level symbols or directives. The function-body
  `#asm(...)` inline-asm form is unchanged.
- **`#[no_alloc]` drop glue:** the check now also rejects owned drop-carrying
  parameters (`move x`, a move-by-default non-Copy struct, `move self`) and
  discarded drop-carrying temporaries, not just `let` locals.
- **`if`/`else` value typing:** an if-expression sizes its result from the type
  codegen actually produces, so any value-producing arm shape (including a
  method call) is accepted; the previous hand-kept type predictor is removed.

### Graph / LSP
- **value-refs precise scoping:** uses resolve to the innermost in-scope
  definition (shadowing is handled correctly); `match`-arm payload bindings and
  `for` loop variables are first-class definitions; and a binding returned from
  a function records the caller-side bindings its value flows into.

### Fixes
- **Ownership (P0):** a heap-owning enum passed by value as a call or method
  argument (e.g. `vec.push(v)` where `v` owns a nested `Vec`) is now moved
  rather than borrow-copied. Previously the caller's scope-exit drop could free
  memory the callee had already stored, a use-after-free (surfaced by a
  `vendor/json` parse + stringify round-trip).
- **Borrow checker:** a bare non-Copy concrete struct/enum argument used while
  its place is borrowed now reports E0372 (move while borrowed) instead of
  E0383 (read while borrowed), matching the move semantics.
- **Codegen:** string interpolation frees its per-segment conversion buffers
  (previously leaked).
- **Use-after-move on generic-payload types:** an enum or struct whose
  Copy-ness depends on a generic payload/field (e.g. `enum W { A(Vec[i32]) }`,
  a recursive `Node { Branch(Vec[Node]) }`, the `vendor/json`
  `Value::Array(Vec[Value])` shape) is now correctly treated as non-Copy, so a
  use-after-move on it is reported (E0335). The move check is also now
  flow-sensitive: a move that happens only on a branch that `return`s/`break`s/
  `continue`s no longer falsely poisons the value on the path where that branch
  is not taken.

### Numerics / GPU
- **`vendor/cuda`:** CUDA Runtime + cuBLAS bindings (NVIDIA GPU) — device
  management, `DeviceBuffer` (Drop = `cudaFree`), a cuBLAS `Handle`
  (Drop = `cublasDestroy`) with `sgemm`/`sgemv` (column-major). Plain C FFI, no
  kernel language; C+ stays a consumer of GPU SDKs.
- **`vendor/cblas`:** reference CBLAS bindings (OpenBLAS / Netlib / MKL) — the
  cross-platform CPU path. Level 1/2/3 (`sdot`/`saxpy`/`sscal`/`snrm2`/`sasum`,
  `sgemv`, `sgemm`, plus d-variants).
- **`[link] search-paths`:** a manifest `[link]` table may now list library
  search directories; each becomes both `-L<dir>` (link time) and
  `-Wl,-rpath,<dir>` (run time), so a library outside the default path
  (e.g. CUDA's `lib64`) resolves without `LD_LIBRARY_PATH`. Relative entries
  resolve against the manifest directory.

### Platform
- **Linux/x86-64:** first Linux bring-up of the toolchain (requires
  clang/LLVM 19+). `cpc` discovers a clang ≥ 19 on its own, links via GNU ld
  with `-lm`, selects `*_linux.cplus` stdlib overrides (epoll reactor), and
  ships a `.deb`. All changes are platform-conditional; macOS output is
  unchanged.
- **Windows/x86-64 (MSVC):** the toolchain builds, tests, and runs on
  `x86_64-pc-windows-msvc`. `cpc` selects `llvm-ar`, links math from the UCRT
  (no `m.lib`), pulls f16 helpers from `compiler-rt`, applies the Microsoft x64
  struct ABI (indirect for non-1/2/4/8 aggregates), sets stdout/stderr to
  binary mode so `\n` stays a single LF (not `\r\n`), and provides a Win32
  `reactor_windows` async backend (timers + cooperative scheduling; socket/file
  IOCP is a follow-up). All changes are platform-conditional.
- **Coroutine codegen portability:** `llvm.coro.end` is emitted in the
  return-type form the target clang expects (`i1` on older LLVM / Apple
  clang 21, `void` on LLVM 22+), probed at build time. Previously a fixed form
  failed to verify on the other toolchain.

### Tooling
- Linux and Windows CI run `cargo test --workspace` on release tags and attach
  the prebuilt binaries (`.deb`; Windows `.zip`) to the GitHub Release,
  alongside the macOS tarball from the release workflow.
- CI actions bumped to `actions/checkout@v5` and `upload-artifact@v5`.

## v0.0.14 — 2026-06-05

Language track. The headline themes are the completed ownership/Drop model, inline
assembly, and code-knowledge-graph value depth.

### Ownership & Drop
- **`unsafe impl Send for T {}` / `unsafe impl Sync for T {}`** — a manual marker
  override. A nominal type that transitively hides a raw pointer is now `!Send`
  and `!Sync` by default (moving or sharing it across a `Send`/`Sync` bound is
  rejected, E0502); a bare `*T` used directly stays Send. The override re-enables
  a type you vouch for. `Send`/`Sync` impls must carry `unsafe` (E0860); `unsafe`
  applies only to those markers (E0861). Conditional generic form carries the
  condition as bounds: `unsafe impl Send for Arc[T: Send + Sync] {}`. `Arc`,
  `Mutex`, and `Channel` carry the right conditional impls.
- **`#[no_alloc]` drop-glue** — a `#[no_alloc]` function now also rejects implicit
  destructors run at scope exit that would allocate/free (a `string`/`Vec`/`Box`
  local, or a type whose `drop` is not itself `#[no_alloc]`), reaching through
  fields, enum payloads, and array elements (E0901).
- **Container element drop** — dropping a `Vec[T]` (and Box/Arc/Rc/HashMap) runs
  each element's `drop` exactly once before freeing the buffer.
- **Consumed-enum payload** — matching an owned enum and binding a payload that is
  not moved out now drops it at arm exit (no leak), while every move-out shape
  still disarms the drop (no double-free).

### Inline assembly
- **Tier 2 — operands + clobbers.** Rust-style named operands:
  `#asm("add {s}, {a}, {b}", s = out(reg) sum, a = in(reg) a, b = in(reg) b,
  clobber("cc"))`. `in`/`out`/`inout` set direction; `reg` lets the compiler pick
  a register (then `{name}` must appear in the template) or `"x0"` pins one.
  `out`/`inout` targets must be `mut` variables; operands are register-sized
  scalars.
- **Tier 3 — `#[naked]` functions.** No prologue/epilogue; the body is inline asm
  that handles the ABI and returns itself (E0909 if the body is not asm-only).
  For trampolines, entry stubs, custom-ABI shims.

### Code knowledge graph
- **`type-at` on inferred expressions.** `cpc query type-at FILE:LINE:COL` (and
  LSP hover) now answer call results, field/index reads, arithmetic, and
  `match`/`if` values, not just annotated positions, rendered with concrete names
  (`Result[Value, ParseError]`, `Vec[i32]`).
- **`value-refs`.** `cpc query value-refs FILE:LINE:COL` returns a binding's
  value-flow: its definition plus every use classified as read / call / construct
  (re-wrap) / return / match / assign.
- **LSP dirty-buffer overlay.** Hover, type-at, value-refs, goto-definition,
  references, and document-symbols reflect unsaved editor edits before save.

### Fixes
- **Codegen:** a `match` arm (or other value position) whose body is an `if`
  building a payload-carrying enum constructor no longer discards the value
  (previously a silent miscompile; surfaced by the json package migration).

### Other
- `vendor/json` parser migrated to a match-consumable result enum with recursive
  auto-Drop; accessors borrow and deep-clone.

Deferred to v0.0.15 (additive): module-level global asm, the if-result predictor
refactor, value-refs precise scoping (shadowing), and the package side
(AppKit → agent).
