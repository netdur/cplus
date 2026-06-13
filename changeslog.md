# C+ changelog

User-facing changes per release, newest first. The changelog starts at v0.0.14;
earlier history lives in each version's archived plan.

## v0.0.22 — unreleased

### Contextual builder blocks (DSL.1–4: parser, lowering, lookup, containers + flow control)
- New expression syntax `@ctx { ... }`: the contextual builder block.
  `ctx` is any module path (`@view`, `@ui::view`); the body holds item
  expressions, leading-dot modifier lines that apply to the item above
  them (`.font = bigger`, `.on_click(f)`), `let` setup bindings, and
  nested `@` blocks. `@` was previously an invalid character; no
  existing source changes meaning.
- Modifier lines are line-oriented: a `.name` that starts a line attaches
  to the current item, while a same-line `.name` stays ordinary postfix
  access on the item expression. Inside call arguments, indexing,
  grouping parentheses, and nested blocks the rule is off, so wrapped
  subexpressions are unaffected.
- Parse-time rejections with builder-specific messages: a modifier with
  no current item (including after an interposed `let`), and `return` /
  `break` / `continue` / `yield` / `await` / loops / `defer` / `guard`
  inside a block.
- Blocks lower to the fixed builder protocol — ordinary package code,
  no macros: `ctx::Builder::new()`, one temporary per item with its
  modifiers applied (`__i.font = v`, `__i.method(args)`), `add(item)`
  per item, and `finish()` as the block's value. `let` entries splice
  through with ordinary block scoping; nested `@` blocks compose when
  `finish` returns the item type; the empty block is `new` + `finish`.
  Any package that ships `Builder` (`new`/`add`/`finish`), an item
  type, and constructor functions becomes a construction DSL.
- Synthesized nodes reuse the user's spans, so sema's ordinary
  diagnostics land on the DSL lines: a wrong item type reports at the
  item line, an unknown modifier field at the modifier line, a context
  module without `Builder` at the `@ctx` line.
- Contextual name lookup: inside `@view { ... }` a bare item name
  (`text(...)`) and a bare context member used as a modifier value
  resolve through the context as `view::text` without qualification.
  Precedence is locals → same-file top-level → contextual, so a `let`
  binding or a same-file function of the same name shadows the package
  member; a bare name that is no member at all falls through to the
  ordinary located "undefined" error. Item field/method names in
  modifiers (`.font`, `.boost(...)`) are never contextual. Because the
  rewrite produces real `view::text` references before the graph
  builds, code-graph/LSP navigation resolves them to the package
  symbols automatically.
- Container elements and item-control (DSL.4): a bare `name { ... }`
  inside a builder block is a *container element of the same context*
  (`vstack { ... }` builds `view::vstack`, its children resolve in
  `view`) — not a nested DSL. Containers take a filled `Builder`
  (`fn vstack(b: Builder) -> Item`), so the whole feature lowers to
  `Builder::new`/`add` plus a finisher (the root calls `.finish()`, a
  container calls `ctx::name(builder)`) — the compiler's output never
  names a collection type, so DSL packages work even on targets where
  `Vec` is gated. `if`/`else` and `for` are Flutter-style collection
  control flow: their items add into the same builder
  (`if logged_in { logout_button() }`, `for row in rows { item(row) }`),
  `if` needs no `else`. A nested *different* `@`-DSL block is rejected
  (write a same-context container without `@`); revisit if a real
  cross-DSL nesting use case appears.
- `cpc fmt` keeps `@ctx` glued and round-trips builder blocks —
  containers and `if`/`for` included — unchanged.

### Multi-backend consolidation
- New `--min-os VERSION` flag (after `--target`): overrides the OS floor
  in versioned target triples — 13.0 default for the iOS targets, API 24
  for android-arm64. Unversioned targets reject it.
- New `esp32c3-riscv32` target (RV32IMC, ilp32, ESP-IDF): 32-bit IR and
  RISC-V ELF objects through esp-clang, ABI pinned against an ilp32 probe
  (8-byte direct window, bare-pointer indirect, no byval). The object
  links into an ESP-IDF esp32c3 firmware. `TargetSpec` gains
  `extra_clang_args` (-march/-mabi for the C3).
- Windows/Linux CI now also run on main pushes, not only release tags.
- New recipe `docs/examples/recipes/android_hello`: C+ source to a signed
  APK (NDK link + script-assembled package), emulator-validated, with
  Gradle integration notes.

### Android UI (vendor/android_view + vendor/jni)
- `vendor/android_view` adopted and validated on the emulator: a C+-built
  View tree renders (nativeCreateView host contract), and Button taps
  reach C+ two ways — a host-shipped adapter class, or the self-contained
  `android_view/listener`, whose adapter ships in-package as a 976-byte
  pre-compiled DEX (`#include_bytes`), loaded with InMemoryDexClassLoader
  (API 26+) and bound via RegisterNatives; apps export one token-routed
  `cplus_on_click` hook.
- `vendor/jni` covers the full 233-slot JNI 1.6 function table (verified
  against the NDK's jni.h; object arrays, RegisterNatives, ExceptionCheck,
  NewDirectByteBuffer bound) and models `JNIEnv *` as the double pointer
  JNI requires (the bare table pointer trips an ART abort).

### Compiler
- File-aware spans: every span carries its source file (stamped at lex
  time), so cross-file diagnostics route themselves, monomorphization's
  call-site records cannot collide across files by construction (the
  v0.0.20 `(origin_file, span)` compound key is gone), and
  `#include_bytes`-style relative paths resolve against the call's own
  file. Internal-only; no language-visible change.
- String literals accept a bare `$` (previously an error; `$$` and
  `${...}` interpolation unchanged) — JNI descriptors for nested Java
  classes (`android/view/View$OnClickListener`) need it.
- Fixed an LLVM-redefinition error when a program both defines a C-ABI
  symbol (`pub extern fn`) and declares it as an extern import elsewhere
  (the app-provided-hook pattern): the import declare is now skipped for
  program-defined symbols.

## v0.0.21 — 2026-06-11

### esp32: heap types + the espidf package
- Embedded package profile: a target can exclude stdlib modules whose
  mechanism it lacks. On `esp32-xtensa`, importing the POSIX half of
  stdlib (`thread`, `mutex`, `channel`, `env`, `net`, `netsys`,
  `reactor`, `executor`, `time`, `fs`) fails at resolve time with E0866
  naming the target and pointing at `vendor/espidf` — instead of an IR
  verifier error after codegen. `async fn` on 32-bit targets is rejected
  at check time with E0867 (the coroutine runtime is 64-bit only). Heap
  modules (`vec`, `text`, `box`, ...) stay available; the host profile is
  unchanged.
- The 32-bit heap runtime: fat pointers (`{ ptr, usize }`), string/Text/Vec
  lengths, pointer-arithmetic GEP indices, and the libc size_t surface
  (`malloc` / `memcpy` / `memcmp` / `snprintf`) now follow the target's
  pointer width, lifting the heap-type restriction on `esp32-xtensa`.
  Verified by esp-clang's IR verifier (kept as a regression gate in e2e)
  and on hardware: a Text and a Vec[i32] built on the ESP32's newlib heap
  print correctly from the device. 64-bit targets are byte-identical.
- New `vendor/espidf` package: GPIO, esp_timer (`now_us`), task sleep
  (`delay_ms` via newlib `usleep`, tick-rate independent), and UART
  console printing. The gpio/timer externs are `#[no_alloc]`+`#[no_block]`
  leaves, so `#[realtime]` control loops can drive pins and read the
  clock under the contract. Entry convention: the app exports
  `cplus_app_main`; ESP-IDF's main component keeps a two-line
  `app_main` C shim. Validated on hardware with an all-C+ firmware
  (GPIO blink + `#[realtime]` PID + telemetry — no C beyond the shim).

### Multi-backend: esp32-xtensa (first 32-bit target) + #[realtime] on-device
- New `esp32-xtensa` target (rungs 3-4 collapsed: the local WROOM-32D spike
  proved esp-clang accepts cpc's IR, so 32-bit support and the Xtensa rung
  shipped together). `usize`/`isize`/pointers are 4 bytes: `llvm_ty`,
  `ty_bit_width`, `static_layout`, and `#size_of`/`#align_of` consult the
  target's pointer width (64-bit targets byte-identical). The Xtensa C ABI
  is pinned against an empirical esp-clang 20.1.1 probe: aggregate args
  ≤ 24 bytes coerce to arrays of align-sized chunks (`[3 x i32]`,
  `[2 x i64]`), larger pass indirect `byval`; returns > 16 bytes use sret
  (argument and return classification now split); no FP-register HFAs.
  Heap/fat-pointer types (Text, Vec, str) are not yet supported on 32-bit
  targets and fail loudly at IR verification rather than miscompile.
- esp-clang resolution: `$CPC_ESP_CLANG` > `$IDF_TOOLS_PATH` (set-but-wrong
  errors) > `~/.espressif`, newest `tools/esp-clang/` version, LLVM 19+
  enforced; missing installs get the `idf_tools.py install esp-clang` hint.
- Verified on hardware: a `#[realtime]` fixed-point PID (compile-time
  no-alloc / no-block / bounded-recursion contract) built as an
  `esp32-xtensa` staticlib, linked into an ESP-IDF firmware, runs closed
  loop on an ESP32-D0WDQ6 at ~1.84 µs (442 cycles) per step; the same
  contract rejects an allocating variant with E0901 at `cpc check`.
- Fixed a `musttail` miscompile-rejection (host-affecting): a
  `pub extern fn` wrapper tail-calling an internal fn returning the same
  ≤16-byte aggregate emitted `musttail` across mismatched IR return types
  (the export's return is ABI-coerced, the callee's is the bare struct);
  clang rejected the module. musttail is now skipped when either side's
  return is coerced.

### Multi-backend: target model + iOS + Android
- New `--target NAME` on `build` / `check` / `--emit-ll` / `--emit-ll-opt` /
  `--emit-asm` / `--emit-obj`. Named targets: `host` (the default),
  `ios-arm64`, `ios-arm64-simulator`, `android-arm64`. An unknown name fails
  with the supported list. Omitting `--target` reproduces the previous host
  behavior byte for byte.
- `android-arm64` (rung 2: the first non-host external toolchain): emits
  `aarch64-linux-android24` ELF objects and staticlibs through the Android
  NDK's clang, resolved from `$CPC_NDK_CLANG`, `$ANDROID_NDK_HOME` /
  `$ANDROID_NDK_ROOT` / `$ANDROID_NDK_LATEST_HOME`, or the SDK's default
  `ndk/` directory (newest version). The resolved clang must report LLVM 19+
  (NDK r28.2+); older NDKs and misconfigured variables fail with the setup
  hint. Staticlibs are archived with the NDK's `llvm-ar` (the host BSD `ar`
  cannot index ELF members). Verified end to end: a C+ staticlib linked by
  NDK clang ran on a Pixel 9 Pro XL emulator.
- A `TargetSpec` (triple, pointer width, endianness, object format, ABI and
  intrinsic selectors, handoff mode) now drives codegen's per-target decisions.
  The former compile-time `cfg!` gates (HFA classification, Microsoft x64 size
  buckets, SysV register pairs, `byval`, spin-loop hints, NEON `tbl1`, the
  Windows binary-mode ctor) resolve against the selected target, so a cross
  build emits the target's ABI and intrinsics, not the host's.
- External-builder handoff: the iOS targets stop at object emission — cpc
  never runs their final link (Xcode owns it). `cpc build` of a `[lib]`
  staticlib emits the object, archive, and C header into
  `target/<target-name>/<mode>/`; clang gets `-target <triple>` plus
  `-isysroot` from `xcrun` when available. `[[bin]]` builds, `cdylib`
  crate-types, `cpc test`, and single-file binaries are rejected for these
  targets with the supported flow named in the message.
- An explicit target pins `target triple = "<triple>"` in the emitted IR, so
  handed-off `.ll` artifacts carry their target. Host IR is unchanged.
- Bundled vendor artifacts resolve by the selected target's stable artifact
  triple (`vendor/<dep>/src/lib/arm64-apple-ios/...`); only the host target
  still consults `clang -print-target-triple`. E0862 now words the mismatch
  as a host or target triple accordingly.

### Bindings
- New `vendor/uikit` package: UIKit bindings mirroring `vendor/appkit`
  (ObjC-runtime FFI; `Window`, `ViewController`, `View`, `Label`, `Color`,
  `Screen`, app-delegate synthesis). Includes the `cplus_app_main` entry
  convention: a two-line C `main` shim in the Xcode target calls into the C+
  staticlib, which registers the delegate and enters `UIApplicationMain`.
  Verified on the iOS simulator: a C+-driven screen (white window, centered
  label) renders on an iPhone 16 Pro simulator.
- `vendor/uikit` expanded to the full binding surface (18 modules):
  controls (Button, Slider, Switch, SegmentedControl, ProgressView,
  ActivityIndicator, PageControl, DatePicker), text (TextField,
  SecureTextField, TextView, SearchBar), containers, data (TableView,
  CollectionView, PickerView), graphics (ImageView, Image, Font,
  BezierPath), dialogs (AlertController), toolbar/navigation/tab bars,
  pasteboard, Auto Layout anchors, events, notifications, navigation /
  tab / split / page controllers, custom-view synthesis (`drawRect:`),
  and ownership rules (owned wrappers release in `drop`). The umbrella
  module re-exports the set; the whole surface sema-checks for the iOS
  targets and links against the simulator SDK in e2e.

## v0.0.20 — 2026-06-11

### Agent surface (Theme B)
- New `agent_consent` recipe: a reference consent middleware over `agent_core`'s
  `AuthGate`. `decide(rules_dir, mode, agent_id, prompt)` resolves an agent in
  three steps — a remembered per-agent rule (persisted to disk), a standing Mode
  (allow-all / deny-all), else prompt the user and remember the answer — then
  maps the result onto a real `AuthGate`. Closes the "ask-user + persisted
  per-agent rules" residual; the gate itself stays a pure predicate.
- `agent_appkit` actions (click / set_text / scroll_to) now marshal to the main
  thread when called off it, so an MCP bridge driven on a background connection
  can't message AppKit off-main. Closure-free (`performSelectorOnMainThread:` +
  an `[NSThread isMainThread]` fast path; scroll_to's NSRect rides a once-
  registered `cplusScrollSelfVisible:` NSView method). `on_main_thread()` is
  public.
- New `Surface::layout_diagnostics`: per-node Auto Layout health
  (`uses_autolayout`, `has_ambiguous_layout` via `-[NSView hasAmbiguousLayout]`),
  so an agent can check a generated UI's layout without a screenshot. The tree
  walk guards the NSView-only selectors so the NSWindow root node is safe.

### Compiler
- Fixed a `musttail` miscompile on arm64: a tail call returning a by-value
  aggregate wider than 16 bytes (returned indirectly by AAPCS64) was marked
  `musttail`, which LLVM's arm64 backend rejects ("failed to perform tail call
  elimination on a call site marked musttail"). The >16-byte eligibility guard
  was x86-64-only; it now applies on all targets. Surfaced building the
  llama.cpp bindings (the 72-byte `llama_model_params` FFI return).
- Closed the inferred-call half of the v0.0.19 monomorphization fix: an
  inferred (no-turbofish) generic call resolved its concrete type-args through
  `call_monos`, keyed by a file-less span, so two such calls at the same byte
  offset in different files could select the wrong instantiation. `call_monos`
  is now keyed by `(origin_file, span)`. (Turbofish calls were already
  collision-free.)

### Bindings
- `llama_cpp` verified end to end: the `llama_cpp_smoke` recipe links against a
  current llama.cpp via `${LLAMA_CPP_LIB}` and runs real text generation on the
  Metal GPU (gemma-4-E2B). Closes the loop with the env-var portability change
  and the arm64 `musttail` fix above.

### Build / manifest
- New W0003 warning: a `[[bin]]` package's own `[link] libs`/`frameworks` are
  ignored when building the binary (those are read only when the package is a
  *dependency*). The warning points to `[[bin]] libs`/`frameworks`, where a
  binary's own libraries belong. The build still succeeds.
- `[link].search-paths` and `[link].extra-objects` now expand `${VAR}` and
  `${VAR:-default}` against the environment, so a binding can point at an
  external SDK without baking an absolute path into the manifest. An unset
  `${VAR}` with no fallback fails at parse time with E0865 naming the variable,
  rather than an opaque linker error. `vendor/llama_cpp` reads `${LLAMA_CPP_LIB}`;
  `vendor/cuda` reads `${CUDA_LIB:-/usr/local/cuda/lib64}`.

## v0.0.19 — 2026-06-09

The agent surface reaches the GUI: a macOS app can expose itself to an external
agent — described, driven, and observed — over a consent-gated JSON-RPC bridge.
Also the breaking intrinsic and string-method renames, a monomorphization fix,
and bindings for llama.cpp.

### Language / compiler (breaking)
- Intrinsics use the `#name(...)` sigil; the legacy `__cplus_*()` call spelling
  is removed.
- `.to_string()` / `ToString` are now `.to_text()` / `ToText`.
- Naming an owned string via `.to_text()` or interpolation requires
  `import "stdlib/text"` (E0613); borrowed views (`str`) need no import.

### Compiler
- Fixed a monomorphization miscompile: a turbofish generic call now mangles its
  callee from its own type-args instead of the file-keyed `call_monos`, so two
  same-offset turbofish calls in different files no longer resolve to the same
  wrong instantiation.
- Multi-file diagnostics render against the right file (GAP 3); static-init
  narrowing casts; clearer E0303 (suggests `Text`) and E0502 (names the real
  type) messages.

### Agent surface — GUI side (Theme B)
- `vendor/agent_appkit`: `open(window)` walks the live NSView tree into a
  `Surface`. describe_ui snapshot (`Vec[UiNode]`); authorized `click` /
  `set_text` / `scroll_to` through the agent_core authorization brain (exposure
  via `set_agent_id`, optimistic-concurrency text edits); notification-to-verb
  event translation.
- `vendor/agent_mcp`: the MCP bridge. JSON-RPC 2.0 (describe_ui / actions /
  events) over Unix-domain sockets (`serve_uds` / `serve_fd`), every request
  gated by an agent_core consent `AuthGate`.
- New `appkit_agent` recipe showing the whole flow.

### vendor/appkit
- Ownership `into_raw` / `from_raw` for parented view wrappers (GAP 2); SF
  Symbols, a layer-backed `RoundedView`, toolbar and text coverage (GAP 4/5);
  the correct NSImage symbol-configuration selector (GAP 6).

### vendor/llama_cpp (new)
- C+ bindings for llama.cpp's C API: raw FFI generated from the upstream headers
  with cpc-bindgen (`build.sh`), plus a hand-written safe facade (`Session`:
  load / generate / tokenize / decode / sample). Links `libllama` / `libmtmd`;
  the `[link]` search-path points at a local llama.cpp build. A `llama_cpp_smoke`
  recipe shows greedy generation.

### vendor/coreai (new)
- Swift bridge for Apple's CoreAI, adapted to the real API (Xcode 27 / macOS 27).

### Tooling
- cpc-bindgen emits safe `pub fn` wrappers over `#[link_name]` externs, `pub`
  records/fields, and `pub type` typedef aliases (the bindings llama_cpp needs).

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
