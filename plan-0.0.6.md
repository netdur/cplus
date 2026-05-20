# C+ — Plan

Version 0.0.5 shipped 2026-05-19. See [plan-0.0.5.md](plan-0.0.5.md) for the archived 0.0.5 roadmap and resolved log; [plan-0.0.4.md](plan-0.0.4.md) covers v0.0.4, [plan-0.0.3.md](plan-0.0.3.md) v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.6 — Enable SIMD + GPU via external packages (stdlib model)

**Strategy:** ship the *minimum* compiler surface that lets external packages do SIMD and GPU work, then stop. No SIMD library in cpc; no GPU backend; no kernel-launch syntax; no autograd. The bet is the same one v0.0.2 made for stdlib and v0.0.5 for iterators — the compiler ships primitives, the ecosystem ships libraries.

The audit phase (see [docs/design/v0.0.6-llvm-survey.md](docs/design/v0.0.6-llvm-survey.md) when written) confirmed two things: (a) LLVM already provides everything we'd want for SIMD as IR primitives (vector types, `llvm.fma`/`sqrt`/`sin`/`cos`/`reduce.*` on vectors, masked loads, shuffles), and (b) GPU work via host-side runtime wrapping (Metal / CUDA Driver / Vulkan) is pure FFI plus precompiled-shader embedding — the device backends in LLVM are not on our integration path. That cuts ~80% of what the research note "Bringing AI and Compute Workloads to C+" estimated.

Three slices, ordered cheapest-first. Total estimated effort: ~1.5 weeks aggregate. None of these change the language model.

Slice sizes use assistant-paced framing (S/M/L), not human-typing weeks.

---

### Phase 1 — Enable external packages · size S–M

The two compiler features without which SIMD and GPU external packages literally cannot be written. Everything else (ergonomic alignment attributes, address-space syntax, const generics) is intentionally cut — packages work around the absence by hand-packing structs and using runtime shape, same as every C codebase does today.

#### Slice 1A — `include_bytes!("path")` builtin · size S

**Goal:** compile-time embed of a file's raw bytes as a `*const [u8; N]` (or whatever shape resolves cleanly). Enables external packages to embed shader binaries (`.metallib`, `.cubin`, `.spv`), pretrained ML weights, test fixtures, and any other binary asset, with no runtime file-read in `main`.

**Why this matters for GPU:** every GPU-via-FFI workflow requires loading a precompiled shader. Without `include_bytes!`, every program ships a loose binary file alongside its executable and reads it at startup — workable but ugly. With it, the shader becomes a static const in the binary and the GPU package's API is `gpu::load_shader(SHADER_BYTES)` instead of `gpu::load_shader_from_file("./shaders/foo.metallib")`.

**Locked design decisions:**

1. **Syntax: `include_bytes!("relative/path")`**, mirroring Rust's well-known form. The `!` suffix marks it as a compiler builtin rather than a regular function call, so the parser routes it before sema's name resolution. No new keyword.

2. **Result type: `*const [u8; N]`** where `N` is the file's byte length, known at compile time. Returned as an address that the user can pass to FFI directly, or cast to a typed pointer for structured access. The `*const` (immutable raw pointer) matches the language's safety story — the bytes live in a `.rodata` section, writing through them is UB.

3. **Path resolution:** relative to the *source file containing the call*, not the project root. Matches `import` resolution. Cross-platform path separators normalize to `/`.

4. **File read happens once at compile time** by sema. The bytes are emitted as a single `@.bytes.N = private unnamed_addr constant [N x i8] c"\xx\xx..."` global; the call site returns the global's address. Repeated `include_bytes!` calls with the same path dedup via the existing string-literal table (extended with a separate bytes-literal table to avoid name collisions with `@.str.N`).

5. **Errors:**
   - **E0870** — `include_bytes!` path not found at compile time. Diagnostic carries the resolved absolute path + the calling source location.
   - **E0871** — `include_bytes!` called with a non-string-literal argument. The path must be a literal, not a variable, so the file read is a pure compile-time operation.
   - **E0872** — file exceeds a sanity-check size limit (default 64 MiB; configurable via `#[include_bytes_max(N)]` attribute on the call site or — defer — module-level). Prevents accidental gigabyte embeds from compiler-bombing CI.

6. **No `include_str!` companion in this slice.** A string-typed file embed is plausible but uses `Ty::Str` which has its own constructor story; not worth coupling. Add later if demand surfaces.

**Implementation surface:**

- **[cplus-core/src/parser.rs](cplus-core/src/parser.rs):** recognize `Ident("include_bytes") + Bang + LParen + StringLit + RParen` as a single `ExprKind::IncludeBytes(path: String, span: ByteSpan)`. No method-style invocation; this is the only builtin macro form we accept.
- **[cplus-core/src/ast.rs](cplus-core/src/ast.rs):** add `ExprKind::IncludeBytes`.
- **[cplus-core/src/sema.rs](cplus-core/src/sema.rs):** during `check_expr`, resolve the path relative to the source file, read the bytes, emit E0870/E0871/E0872 as needed. Stash the bytes in a new `IncludeBytesTable` keyed by absolute path → (symbol, len). Return type `Ty::RawPtr(Box::new(Ty::Array(Box::new(Ty::U8), len as u64)))`.
- **[cplus-core/src/codegen.rs](cplus-core/src/codegen.rs):** lower to a private constant array global; the SSA value at the call site is the global's pointer (no load).

**Tests:**

- Unit: parser produces `IncludeBytes` node for `include_bytes!("foo.bin")`; rejects `include_bytes!(some_var)` at parse time.
- Unit (sema): missing file fires E0870 with the resolved absolute path in the message.
- Unit (sema): non-literal argument fires E0871.
- Unit (sema): oversize file fires E0872 (use a synthesized 65 MiB file in a temp dir).
- E2E: a small project with `assets/hello.bin` containing 6 bytes `"hello\n"`. Source does `let p = include_bytes!("../assets/hello.bin");` and pretty-prints via libc `write`. Verifies the bytes match and the program exits 0.
- E2E: two `include_bytes!` calls referring to the same absolute path produce the same SSA pointer (dedup verified via emit-ll comparison).

**Exit:** GPU recipe packages can embed shader binaries at compile time. Any external package needing baked-in binary data uses the same primitive.

---

#### Slice 1B — SIMD types + intrinsics + lane access · size M

**Goal:** expose LLVM's vector machinery in C+ source so external SIMD packages can be written. No SIMD package in this slice — only the compiler-side surface.

**Why this matters:** without `f32x4` etc. as primitive types, there is no way to construct a vector value in C+ source. Without intrinsic plumbing, there is no way to call `llvm.fma.<4 x float>` or similar. Without lane access, there is no way to read/write individual elements. All three are mandatory; everything else (shuffles, masked ops, reductions) can ship in a follow-on slice once the foundation is in place.

**Locked design decisions:**

1. **Fixed-width only.** Variable-length SIMD (SVE on AArch64, RVV on RISC-V) is deferred. The widths in this slice match what x86_64-SSE/AVX2 and AArch64-NEON natively support:
   - 128-bit: `f32x4`, `f64x2`, `i8x16`, `i16x8`, `i32x4`, `i64x2`, `u8x16`, `u16x8`, `u32x4`, `u64x2`, `mask8x16`, `mask16x8`, `mask32x4`, `mask64x2`
   - 256-bit (AVX2-only on x86): `f32x8`, `f64x4`, `i8x32`, `i16x16`, `i32x8`, `i64x4`, `u8x32`, `u16x16`, `u32x8`, `u64x4`, `mask8x32`, `mask16x16`, `mask32x8`, `mask64x4`

   On AArch64-only hosts the 256-bit widths still compile (LLVM splits them into two 128-bit ops); not as fast as native AVX2 but functionally correct.

   **Defer:** 512-bit (`f32x16`, `f64x8`, etc.) until AVX-512 / SVE2 becomes a tier-1 target. Adding them later is a pure extension — no breakage.

2. **Method-call ops, not operators.** Per [SKILL.md §2.6](SKILL.md), C+ has no operator overloading. SIMD arithmetic goes through methods:

   ```cplus
   let c: f32x4 = a.add(b);                  // a + b element-wise
   let r: f32x4 = a.mul(b).add(c);           // a*b + c (the LLVM fmuladd peephole composes)
   let n: f32x4 = a.sqrt();
   let m: f32x4 = a.min(b);
   let lane0: f32 = v.lane(0 as u32);
   let v2: f32x4 = v.with_lane(0 as u32, 5.0f32);
   ```

   No exception to §2.6 for SIMD. The audit-clarity argument from the research note applies directly: explicit method calls make allocations and ABI shapes visible.

3. **Constructors:**
   - `f32x4::splat(s: f32) -> f32x4` — broadcast a scalar to every lane.
   - `f32x4::new(a: f32, b: f32, c: f32, d: f32) -> f32x4` — per-lane initializer; one constructor per width × type combination. Generated by sema, not user-defined.
   - `f32x4::load(p: *f32) -> f32x4` — unsafe; user owns alignment.
   - `v.store(p: *mut f32)` — unsafe; user owns alignment.

4. **Lane access via blessed methods.** `v.lane(i)` and `v.with_lane(i, x)`. The index argument must be a literal `u32` (sema rejects non-literal lane indices with E0873 to prevent runtime bounds blowups) — this matches how `extractelement` / `insertelement` consume constants in well-formed IR. The literal must be in range `0..N` or E0874 fires.

5. **Method tables stored on a new `SimdTypeDef` per type** in sema's tables, distinct from `StructDef`. Codegen consults the table to map `a.add(b)` → `fadd <4 x float> %a, %b`, `a.fma(b, c)` → `call <4 x float> @llvm.fma.v4f32(...)`, etc. The Vec3 raytracer pattern (struct of 3 f32s, methods named `.add`/`.mul`/`.dot`) is unchanged; SIMD types are a separate axis.

6. **No shuffles, no reductions, no masked ops in this slice.** Those land in a v0.0.7 follow-on once a real external SIMD package is in tree and we know what shapes it actually wants. Shipping the foundation first matches the v0.0.2 stdlib model — bootstrap, then iterate.

7. **`#[repr(C)]` interop:** SIMD types are NOT `#[repr(C)]`-compatible by default. Passing `f32x4` across an `extern fn` boundary fires E0410 (the C-ABI-export check already in place). The user can hand-bitcast `f32x4 → [f32; 4]` via `to_array` / `from_array` blessed methods for FFI; that's the escape hatch.

**Method matrix** (per slice 1B; expand in follow-ons):

| Method | LLVM lowering | Applies to |
|---|---|---|
| `splat(s)` | `insertelement` + `shufflevector` broadcast | all widths |
| `new(a, b, ...)` | sequential `insertelement` | all widths |
| `load(p)` | `load <N x T>, ptr p, align 4` | all widths |
| `store(p)` | `store <N x T>, ptr p, align 4` | all widths |
| `lane(i)` | `extractelement` | all widths |
| `with_lane(i, x)` | `insertelement` | all widths |
| `add`/`sub`/`mul` | `fadd`/`fsub`/`fmul` (floats), `add`/`sub`/`mul` (ints) | all numeric widths |
| `div` | `fdiv` (floats), `sdiv`/`udiv` (ints) | all numeric widths |
| `fma(b, c)` | `llvm.fma.<vN>` | float widths |
| `min(b)` / `max(b)` | `llvm.minnum.<vN>` / `maxnum.<vN>` | float widths |
| `min(b)` / `max(b)` | `llvm.smin.<vN>` / `umin.<vN>` etc. | integer widths |
| `sqrt()` | `llvm.sqrt.<vN>` | float widths |
| `abs()` | `llvm.fabs.<vN>` (float), `llvm.abs.<vN>` (int) | float + signed-int widths |
| `to_array()` | `bitcast <N x T> to [N x T]` | all widths (FFI escape) |
| `from_array(a)` | `bitcast [N x T] to <N x T>` | all widths (FFI escape) |

**Implementation surface:**

- **[cplus-core/src/sema.rs](cplus-core/src/sema.rs):** new `SimdTypeDef { width: u32, elem_ty: Ty, lanes: u32 }` registry; populate at sema start (analogous to `register_builtins` / `register_blessed_interfaces`). Each type's `methods` field holds the lowering recipe — a new `SimdMethodKind` enum drives codegen.
- **`resolve_type`:** recognize `f32x4`, etc. as named types backed by `Ty::Simd(SimdTypeId)`.
- **`check_method_call`:** when receiver is `Ty::Simd(_)`, dispatch via the SIMD method table before the struct/enum paths.
- **[cplus-core/src/codegen.rs](cplus-core/src/codegen.rs):** `lty` for `Ty::Simd` returns `<N x T>` string. New `gen_simd_method_call` emits the right LLVM op based on `SimdMethodKind`. Existing arithmetic emission can reuse `fadd contract` / `fmul contract` / etc. — the `contract` fast-math flag we shipped in v0.0.5 carries over to vector ops correctly.

**Tests:**

- Unit (sema): each width's `splat`, `new`, `load`, `store`, `lane`, `with_lane`, arithmetic methods type-check.
- Unit (sema): `v.lane(idx_var)` rejected with E0873; `v.lane(99 as u32)` rejected with E0874 when N=4.
- Unit (sema): `pub extern fn f(v: f32x4) -> f32x4` rejected with E0410.
- Codegen snapshot: `f32x4::new(1, 2, 3, 4).add(f32x4::splat(0.5)).sqrt()` produces clean `<4 x float>` IR with `fadd contract` + `llvm.sqrt.v4f32`.
- E2E: dot product via SIMD. Construct two `f32x4`, multiply, sum lanes by hand (`v.lane(0) + v.lane(1) + v.lane(2) + v.lane(3)`), verify against scalar.
- E2E: AArch64-darwin emits expected NEON instructions (`fmul.4s`, `fadd.4s`) for the dot product. x86_64-linux emits SSE/AVX equivalents — verify via `--emit-asm` smoke test.
- E2E: `f32x4` round-trips through `to_array` / `from_array` for FFI.

**Exit:** an external `vendor/simd` package — even just an empty skeleton with documentation — is buildable today. Anyone writing a SIMD-using package has the full LLVM vector machinery accessible from C+ source.

---

#### Slice 1C — Free LLVM perf wins · size S–M (split, was S)

**Status (2026-05-20):** Decomposed into four sub-slices once the work was actually attempted. The original "1 session for all four" estimate underestimated each item's threading cost. Only 1C.1 ships in this slice; 1C.2–1C.4 carry forward.

| Sub-slice | Status | Why split |
|---|---|---|
| **1C.1 — `noundef` widening** | **Shipped** | One-line audit; widened to include `Ty::Simd` (new in 1B) alongside the existing primitive + plain-enum + raw-ptr + fn-ptr cases. |
| **1C.2 — `llvm.lifetime.start/end`** | **Deferred to a future v0.0.6 slice** | Needs per-scope alloca tracking + interleaving lifetime intrinsics with body emission. Current codegen batches allocas at function entry. A function-wide bracket (start at fn entry, end at every `ret`) is mechanical but useless — LLVM already knows that lifetime. A per-scope implementation requires refactoring the alloca-emission pipeline. |
| **1C.3 — TBAA metadata** | **Deferred to a future v0.0.6 slice** | The metadata tree + per-type-leaf design is straightforward; the threading is not. There are ~280 load/store emission sites in codegen, none of them centralized through a helper. Doing TBAA correctly requires either (a) migrating all sites to a `gen_load(ty, ptr)` / `gen_store(ty, ptr, val)` helper, then adding `!tbaa` once at that helper, or (b) sprinkling `!tbaa !N` suffixes at every emission site. (a) is the right answer but a session-shaped refactor; (b) is brittle. Defer until time is allocated for (a). |
| **1C.4 — `#[unroll(N)]` / `#[vectorize_width(N)]`** | **Deferred to a future v0.0.6 slice** | Attribute parsing today is item-level only (functions / structs / enums). Statement-level attributes (which a loop attribute needs) require parser + AST + attrs.rs surgery before codegen can consume them. Out of scope for an "emit one more metadata node" slice. |

**Original goal (retained as reference):** four small "emit one more attribute" changes that cost LLVM nothing extra at compile time but unlock more aggressive optimization downstream. None of these change source-level surface or what can be expressed; they speed up the resulting binaries.

**Bundled here because each is half-a-session of pure codegen work and they all share the "just emit more LLVM metadata" pattern.** — In practice each is closer to a full session once the surrounding infrastructure work is included.

**1. TBAA (Type-Based Alias Analysis) metadata.** Every C/C++/Rust/Swift compiler emits `!tbaa` on loads and stores so LLVM's alias analysis can prove that, e.g., a `*i32` and a `*f64` never alias. cpc currently emits no TBAA, which forces LLVM into the worst-case "every pointer may alias every other" assumption. Adds the standard TBAA tree at module level (`Root → C+ types → i8/i16/i32/.../ptr/struct types`) and tags loads/stores via the existing alias-scope metadata pipeline.

**Expected payoff:** measurable speedups on tight loops with mixed-type buffer access. Raytracer-shaped code in particular benefits because Vec3 field loads currently can't be hoisted past nearby Sphere loads.

**2. `llvm.lifetime.start` / `llvm.lifetime.end` on locals.** Bracket every `alloca`'s live range with lifetime intrinsics. Lets LLVM's SROA reuse stack slots aggressively and shrink frame sizes — common case is a function with multiple short-lived non-Copy structs that today each get their own alloca slot for the full body, vs. sharing one slot under lifetime brackets.

**Expected payoff:** smaller stack frames, occasionally measurable speedups via better cache behavior on recursion-heavy code (raytracer's `ray_color` is the obvious candidate).

**3. `#[unroll(N)]` and `#[vectorize_width(N)]` loop attributes.** Pass-through to LLVM's loop metadata (`!llvm.loop !{!"llvm.loop.unroll.count", i32 N}` and the vectorizer counterpart). Source: `#[unroll(4)] while i < n { ... }` on a `while` or `loop`. Sema validates N is a literal in `[1, 256]`; codegen attaches the metadata to the loop header's branch.

**Expected payoff:** explicit knob for SIMD package authors who know the right unroll factor for their hot loops. Marginal for general code; load-bearing for tight inner loops.

**4. `noundef` on more parameters.** v0.0.2 Slice 1A added `noundef` for pointer-passed non-Copy params. Audit whether scalar Copy params (every primitive integer, float, bool) get it too — they should, because definite-assignment guarantees the bits are fully defined at the call site. If not, this is a one-line widening.

**Expected payoff:** lets `-O3`'s freeze/select folding fire more often. Small but cheap to enable.

**Locked design decisions:**

- All four are pure codegen changes — no syntax additions beyond `#[unroll(N)]` / `#[vectorize_width(N)]` attributes.
- TBAA tree is built once at module init in codegen, stored on `ModuleMetadata`, and loads/stores look up the appropriate node lazily.
- Lifetime intrinsics are skipped at `-O0` (debug builds) since they confuse the debugger; enabled at `-O2`/`-O3`.

**Tests:**

- Unit: TBAA tree is emitted in module preamble; a load of a primitive int carries `!tbaa !N` referencing the correct type node.
- Unit: every `alloca` in a non-debug build is followed by `llvm.lifetime.start` and preceded by `llvm.lifetime.end` at scope exit.
- Unit: `#[unroll(4)] while ... { }` parses + sema-accepts; codegen attaches the metadata.
- E2E: raytracer benchmark stays within ±1% (no regression). Ideally improves; the goal is not to ship a perf gain, just to not break.
- E2E: existing tests stay green. 869 lib + 353 e2e baseline.

**Exit (revised for 1C.1):** `Ty::Simd` carries `noundef` on value-passed params (sound because vectors are register-class scalars, no aggregate poison padding). 1C.2/1C.3/1C.4 remain on the v0.0.6 roadmap as separate slices.

---

### Phase 1 exit criteria

- [x] `include_bytes!` ships with E0870 (file not found) / E0871 (non-literal arg) / E0872 (>64 MiB sanity limit) + dedup + e2e asset-embed test. Strict parser form rejects non-literal args before sema.
- [x] SIMD types — `f32x4`, `f64x2`, `i32x4`, `i64x2`, `u32x4`, `i8x16`, `i16x8`, `u8x16`, `u16x8` — and the method matrix (constructors, arithmetic, lane access, FMA + sqrt for floats, bitwise + shifts for ints, min/max/abs, load/store, to_array/from_array) all compile, run, and produce native NEON `fmla.4s` etc. on AArch64-darwin (verified via `--emit-asm`).
- [ ] TBAA + lifetime intrinsics + loop attributes ship without regressions — **1C.1 (`noundef` widening) shipped; 1C.2 / 1C.3 / 1C.4 split out as dedicated slices, see Slice 1C status table above for the per-item deferral rationale.**
- [x] Documentation: [`docs/design/v0.0.6-external-package-enable.md`](docs/design/v0.0.6-external-package-enable.md) — full LLVM-gives-us-X / cpc-plumbs-Y / packages-handle-Z breakdown, status tables, locked non-goals, open questions.
- [x] One small external recipe demonstrating the SIMD path: [`docs/examples/recipes/simd_dot/`](docs/examples/recipes/simd_dot/) — computes a dot product via `f32x4::fma` + lane reduction, validates against a scalar reference, lowers to NEON `fmla.4s` on AArch64-darwin (verified via `--emit-asm`).
- [x] One small external recipe demonstrating the GPU enable path: [`docs/examples/recipes/metal_compute/`](docs/examples/recipes/metal_compute/) — embeds a precompiled `.metallib` via `include_bytes!`, dispatches `double_each` on the GPU through ObjC interop into Metal. macOS-gated; prereq is one-time `xcodebuild -downloadComponent MetalToolchain`. C+ source compiles cleanly (verified with a placeholder `.metallib` of the right shape); full GPU run requires the Metal toolchain locally. The recipe also exposed one v0.0.7 design opportunity: an `include_str!` (or matching length-query) companion to `include_bytes!` would eliminate the build.sh sed-substitution dance — currently filed as an open question, not a blocker.

### Phase 1 non-goals

These were considered and explicitly cut. Each entry exists so a future contributor can see why the gap exists rather than re-derive the analysis.

- **`#[align(N)]` attribute.** External package authors pad upload structs manually with explicit `_pad: u8` fields. Workable; not enabling. Revisit if SIMD package authors hit a real ergonomic wall.
- **`addrspace(N)` source syntax.** Only relevant if cpc emits device-side code. The stdlib model has cpc emit host-side code only; GPU memory is opaque `*u8` from C+'s perspective.
- **Const generics for tensor shape.** External tensor packages use runtime shape via `Vec[usize]` strides, same as NumPy/PyTorch. AI-generated code composes runtime shapes fine.
- **Inline asm.** Real but huge. Defer until a concrete workload demonstrates the need.
- **GPU backend emission from cpc.** PTX, SPIR-V, AMDGPU — LLVM has the backends, but the surrounding pipeline (extract `#[kernel]` fns, compile separately, embed) is a v0.1.x architectural shift. Out of scope for v0.0.x.
- **JIT compilation.** cpc is AOT-only. ML frameworks that JIT (XLA, PyTorch 2, JAX, Mojo) live in a different design space.
- **Autograd.** No source-to-source transformation in cpc (§2.7 forbids it). Future autograd story would be library-only, walking explicit `Function`-objects — but that's a stdlib question, not a compiler question.
- **Operator overloading exception for SIMD or tensor types.** Same as everywhere else: §2.6 holds. Method calls, no exceptions.
- **`include_str!` companion to `include_bytes!`.** Coupled to `Ty::Str`'s constructor story; ship after `include_bytes!` if real demand surfaces.
- **`f32x16`, `f64x8`, AVX-512 / SVE2 widths.** Add when AVX-512 or SVE2 becomes a tier-1 target. Pure extension; doesn't break Phase 1.
- **SIMD shuffles, reductions, masked ops.** Foundation first; expansion in v0.0.7 once a real external SIMD package exists in tree to drive what's needed.

### Phase 1 estimated effort

**1.5 weeks aggregate**, assistant-paced. Breakdown:
- Slice 1A (`include_bytes!`): ~0.5 day
- Slice 1B (SIMD foundation): ~2 sessions
- Slice 1C (LLVM perf wins bundle): ~1 session
- Recipes + design doc: ~0.5 session

---

### Phase 2 — `vendor/appkit` bindings + C+/ObjC data bridge · size M

**Status (2026-05-20):** Phase 2A landed organically alongside Phase 1; remaining slices (2B/2C) carry into v0.0.6's remainder. The work validates that the v0.0.5/v0.0.6 ObjC-interop surface (`extern fn`, `#[link_name]`, `#[repr(C)]`, raw-pointer FFI) is sufficient to bind a real frameworks-heavy library — and what *additional* user-space utilities the binding-author ergonomically needs.

#### Slice 2A — Vendor package skeleton · size S (**shipped**)

[`vendor/appkit/`](vendor/appkit/) — 15 modules, ~2400 lines, covers the common AppKit surface:

- `runtime` — geometry types (`Point`/`Size`/`Rect`/`EdgeInsets`), the typed `objc_msgSend` family (one `#[link_name = "objc_msgSend"]` `extern fn` per call signature), `ns_string` / `alloc_init` / `new_object` / `attach_callback` helpers, the dynamic `CActionTarget` class for binding `target/action` callbacks back to C+ `fn(*u8)` callbacks via `objc_setAssociatedObject`.
- `application`, `window`, `view`, `controls`, `text`, `containers`, `data`, `graphics`, `menu`, `dialogs`, `panels`, `toolbar`, `controllers` — per-concern typed wrappers around the runtime layer.
- `appkit` — facade re-exporting the common path.

**Exit:** every Cocoa class a typical app reaches for has a typed C+ struct in the package; `import "appkit/appkit" as appkit;` is the canonical entry point.

#### Slice 2B — C+/ObjC data bridge module · size S

**Problem:** C+'s richer types (`string`, `str`, `Vec[T]`, tagged enums) have no direct ObjC counterpart. The current `runtime.cplus` only handles primitives, raw pointers, and `#[repr(C)]` value structs — everything else expects the user to know how to bridge by hand. Without a bridge module, every binding consumer reinvents the same NSString / NSArray / NSData conversions, badly.

**Goal:** ship `vendor/appkit/src/convert.cplus` as a small (~200 LOC) bridge with the conversions a real app actually hits.

**Locked design decisions:**

1. **The bridge lives in the binding package, not in cpc.** Same stdlib-model bet as everywhere else in v0.0.6 — the compiler ships primitives, the ecosystem ships libraries. Adding NSString / NSArray awareness to cpc would couple it to Cocoa.

2. **Strings — both directions.**
   - `cplus_str_to_nsstring(s: str) -> *u8` — copies via a NUL-terminated stack scratch buffer + `NSString.stringWithUTF8String:`. Returns an autoreleased `NSString*`.
   - `cplus_string_to_nsstring(s: string) -> *u8` — same shape; takes the owned variant.
   - `nsstring_to_cplus_string(ns: *u8) -> string` — `[ns UTF8String]` then `string` copy (owned). Allocates a heap copy because `UTF8String`'s lifetime is the autorelease pool's, not the caller's.
   - `nsstring_to_cplus_str_unsafe(ns: *u8) -> str` (unsafe) — borrow-shaped, points into the `[ns UTF8String]` buffer; callers asserting they outlive only the pool.

3. **NSArray helpers — primitive flavors first.** `nsarray_count(arr: *u8) -> usize`, `nsarray_obj_at(arr: *u8, idx: usize) -> *u8`. For numeric round-trips, ship `nsarray_to_vec_i32 / _i64 / _f32 / _f64` (each unwraps via `NSNumber`). Generic `Vec[T]` round-tripping is deferred — needs an interface bound declaring the element knows how to bridge, which is Phase 3 territory.

4. **NSData ↔ Vec[u8].** `nsdata_to_vec_u8(d: *u8) -> Vec[u8]` (copy), `vec_u8_to_nsdata(v: Vec[u8]) -> *u8` (no-copy view via `dataWithBytesNoCopy:length:freeWhenDone:NO`). The no-copy case is the load-bearing one for `include_bytes!`-embedded blobs (e.g. the metal_compute recipe's `.metallib`).

5. **No NSDictionary / NSSet / NSDate / NSURL bridges in 2B.** Each can live in a follow-on slice once a real consumer hits the need.

6. **All bridge fns are `pub fn` over already-`unsafe` ObjC calls.** The bridge's `pub fn` doesn't need to be `unsafe`-tagged itself — the inner `objc_msgSend` calls already are. Callers can use it from safe code.

**Implementation surface:**

- **New file**: [`vendor/appkit/src/convert.cplus`](vendor/appkit/src/convert.cplus).
- **New extern fn**: `strlen(*u8) -> usize` (for `UTF8String` round-trip).
- **Re-exports**: extend `appkit/appkit.cplus` facade to `import "./convert"` so `appkit::cplus_str_to_nsstring(...)` works.

**Tests:**

- Unit (cpc test inside the package): every bridge fn round-trips. `assert cplus_str_to_nsstring("hello") -> NSString -> nsstring_to_cplus_string()` produces a `string` equal to "hello".
- E2E: extend an existing AppKit recipe to use bridges instead of hand-coded NSString conversions; verify identical visible behavior.
- Negative: `nsstring_to_cplus_str_unsafe` is `unsafe`-gated.

**Exit:** binding consumers no longer need to write `NSString.stringWithUTF8String:` boilerplate manually; the bridge handles every common shape.

#### Slice 2C — AppKit integration recipe · size S

**Goal:** ship one runnable recipe at `docs/examples/recipes/appkit_hello/` that uses the `vendor/appkit` package — a window with a label and a button, demonstrating both the binding-typed API and the new conversion utilities from 2B. Validates the full chain: vendor package → bridge → real GUI app.

Mirrors `proves/03-hello-appkit` but rewritten to use `import "appkit/..." as appkit;` instead of inline `extern fn` declarations. The diff in source length is the recipe's main selling point.

**Tests:** smoke test along the lines of the existing `appkit_vendor_package_smoke` shape but ordered correctly (each `TabViewItem` belongs to exactly one tab view — the failing smoke test in working state today adds the same item to two parents, which Cocoa raises on; fix while we're here).

#### Phase 2 exit criteria

- [x] `vendor/appkit` ships with the full module split + README + `[link] frameworks = ["Cocoa"]`.
- [x] [`vendor/appkit/src/convert.cplus`](vendor/appkit/src/convert.cplus) ships the full primitive bridge surface — string × {`str`, `string`} → `NSString` and `NSString` → {`string` owned, `str` unsafe-view}; `NSArray` × {`i32`, `i64`, `f32`, `f64`} primitive readout; `NSData` ↔ `Vec[u8]` copy + zero-copy variants. Re-exported through the `appkit/appkit` facade. E2E verified via `appkit_bridge_round_trip` (Foundation-only, no main-thread requirement).
- [x] [`docs/examples/recipes/appkit_hello/`](docs/examples/recipes/appkit_hello/) builds cleanly via `cpc build`, shows a labeled window with a working "Quit" button via `Button::set_on_click`. Demonstrates dynamic-string label writing through the Phase 2B bridge (`cplus_string_to_nsstring`) and the read-back round-trip (`nsstring_to_cplus_string`) inside `main` as a sanity check.
- [x] `appkit_vendor_package_smoke` e2e green. Root cause was two-fold: the same `NSTabViewItem` was being added to both an `NSTabView` and an `NSTabViewController`, AND `NSTabViewController.addTabViewItem:` insists every item have a non-nil `viewController`. Fixed by (a) using a fresh `TabViewItem` for the controller branch, (b) extending `vendor/appkit/src/containers.cplus::TabViewItem` with `set_view_controller`, (c) updating the test's success-exit assertion to match the sentinel `return 42` at the end of the smoke source (event loop is intentionally not run).

#### Phase 2 non-goals (cut, with rationale)

- **Auto-bridging at the `extern fn` boundary.** Sema could in principle recognize `extern fn foo(s: string)` and emit a string-to-NSString stub. Rejected — it hides allocation, ties cpc to Cocoa, and conflicts with the stdlib-model bet. Bridges stay explicit.
- **Submodule re-export through the `appkit/appkit` facade for functions.** Considered (`pub fn cplus_str_to_nsstring = convert::cplus_str_to_nsstring`-style aliasing). Rejected — `pub type X = ...` already covers the load-bearing case (types need to flow through the facade so user-side annotations work; functions don't). Re-exporting functions would split consumers between two equally-valid call paths (`appkit::cplus_str_to_nsstring` vs `bridge::cplus_str_to_nsstring`), which makes grep less reliable and conflicts with the "one way to do one thing" spirit of §2.9. The README's module table is the discoverability path; sub-module import (`import "appkit/convert" as bridge;`) is the canonical call shape.
- **`Drop` on Cocoa wrapper types.** The vendor structs (`Window`, `Button`, etc.) wrap a `*u8` ObjC handle, which the autorelease pool owns. Adding a `drop` method that calls `[obj release]` would conflict with ARC-shaped lifetimes that the app's pool already manages. Leave the wrappers Copy-ish and let the pool's `drain` do the work — matches the proves/03 reference C code.
- **Generic Vec[T] bridges with element-type bounds.** Needs a `ToCocoa`/`FromCocoa` interface declaration + impls per primitive, which is a Phase 3 design exercise. Ship the four monomorphic helpers (`_i32 _i64 _f32 _f64`) and revisit once the demand is concrete.

#### Phase 2 estimated effort

- Slice 2A: shipped (~organic during v0.0.5/v0.0.6 cycle).
- Slice 2B: ~0.5 session.
- Slice 2C: ~1 session.

Total remaining: ~1.5 sessions.

---

## Next

After v0.0.6 ships, the natural follow-on questions:

- Does a real `vendor/simd` external package get written and used? If yes, that drives v0.0.7's SIMD expansion (shuffles, reductions, masked ops). If no, the foundation sits and we look elsewhere.
- Does a real `vendor/metal` external package emerge? If yes, the recipes pile up and we may want `#[align(N)]` after all. If no, GPU stays an experiment.
- Are there workloads asking for `f32x16` / AVX-512? If yes, the width-expansion slice is trivial.

**Open questions for later** (do not block phase work):

- Whether to ship a single blessed `vendor/simd` as part of cpc itself (like stdlib) or leave it purely external. The v0.0.2 stdlib precedent suggests blessing one is reasonable.
- Whether the `include_bytes!` syntax with `!` is the only macro-shaped builtin we'll ever bless, or whether others (`include_str!`, `env!`, `concat!`) will follow. The §2.7 no-macros rule prohibits user-defined macros but doesn't forbid compiler-blessed ones; the question is where the line is.
