# C+ — Plan

Version 0.0.7 shipped 2026-05-20. See [plan-0.0.7.md](plan-0.0.7.md) for the archived 0.0.7 roadmap and resolved log; [plan-0.0.6.md](plan-0.0.6.md) covers v0.0.6, [plan-0.0.5.md](plan-0.0.5.md) v0.0.5, [plan-0.0.4.md](plan-0.0.4.md) v0.0.4, [plan-0.0.3.md](plan-0.0.3.md) v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.8 — Validate the v0.0.7 surface with real workloads

**Strategy:** v0.0.7 shipped a lot of compiler work — TBAA, lifetime intrinsics, loop attributes, the full SIMD method matrix across nineteen widths + eight mask types, ergonomic appkit polish, `include_str!`. None of it has been pressure-tested by a real workload. v0.0.8 is the validation cycle: write a raytracer that exercises the perf-critical path, bless a `vendor/simd` library that shows what packaging the new SIMD surface looks like, add one more bindings package off the Apple stack to confirm the model generalizes, and close the small-but-deferred items.

No new language features. No new principles. Three packaging-shaped slices and one micro-feature.

Slice sizes use the same S/M/L assistant-paced framing as v0.0.6 / v0.0.7.

---

### Phase 1 — Raytracer benchmark · size L

**Goal:** port a recognizable ray-sphere intersection workload to cpc and run it head-to-head against the same algorithm in C, Rust, and Swift under `proves/`'s C-stdlib-only fair-mode harness. The output is a single PNG + a wall-clock + a per-second-ray throughput number for each language.

**Why this matters:** the v0.0.7 perf work (TBAA on every primitive load/store; lifetime intrinsics; loop attributes) was infrastructure with no measured perf delta. The plan repeatedly cites "raytracer" as the regression target. Until a raytracer actually runs, the perf claim is hypothesis. With one, TBAA-vs-no-TBAA and lifetime-vs-no-lifetime are A/B switches we can measure, and any v0.0.8+ codegen change has a concrete regression guard.

#### Slice 1A — Reference port: scalar raytracer · size M

**Source workload:** Peter Shirley's "Ray Tracing in One Weekend" — chapters 1-9 (Vec3 ops, sphere intersection, recursive bounce, Lambert + metal + dielectric materials, defocus blur). Single-threaded, single-channel float math.

**Why this workload:** it's the standard portable raytracer benchmark. Every comparison language already has a known-good port. The hot path is Vec3 dot product + sphere-quadratic root + recursive `ray_color` — three primitive-typed structs in tight loops, exactly the shape TBAA was designed for.

**Implementation surface:**

- New `proves/benchmark/programs/05-raytracer/` directory mirroring the existing benchmark layout (`cplus/`, `rust/`, `swift/`, `c/`, `tests/`, `_scaffolds/`).
- C+ port lives in `proves/benchmark/programs/05-raytracer/cplus/src/main.cplus`. PPM output to stdout (so the harness's stdout-capture works); 320×180 image, 32 samples/pixel, max bounce depth 8 (small enough that a CI run finishes in <30s on Release).
- Reference C port (Shirley's own one-file code, lightly de-C++'d to plain C for the fair-mode harness's libc-only contract).
- Rust + Swift ports for cross-language baselines.
- `tests/` driver: SHA256 of the PNG output (the PPM is deterministic — fixed RNG seed). The harness reports wall-clock + cycles + max-RSS for each.

**Locked design decisions:**

1. **Scalar first.** This slice ships a scalar raytracer in every language. SIMD-accelerated variants are a separate slice (1B) — keeping the comparison clean (cpc scalar vs C scalar vs Rust scalar vs Swift scalar) avoids confounders.

2. **Deterministic RNG seed.** A single `u64` LCG (the trivial `state = state.wrapping_mul(6364136223846793005) +% 1442695040888963407` step) shared across all ports. Same seed → same image bytes → harness can SHA-256 verify. No platform `rand()`, no `<random>`.

3. **No I/O beyond stdout PPM write.** Matches the fair-mode harness contract.

4. **Build via `cpc build`.** No `--emit-ll` shortcut. The recipe ships a `Cplus.toml` with `[package]` only — no link table — and uses the libc-only contract for `printf` / `fprintf` / `clock` from `extern fn` declarations.

**Tests:**
- Unit: per-module checks on Vec3 + Ray + Sphere via the existing `#[test]` runner.
- E2E: the harness round-trips a known image hash for each language; a 1% throughput delta between cpc and Rust is the regression guard.

**Expected payoff:** the raytracer becomes the standing perf target. Any future codegen change ships with a Δ-throughput number against it.

#### Slice 1B — SIMD raytracer variant · size M (gated on 1A)

**Goal:** rewrite the hot path (Vec3 add/sub/mul/dot, ray-sphere intersection) to use `f32x4` for component vectors. Compare scalar-vs-SIMD throughput.

**Why this slice exists:** v0.0.7's nineteen SIMD widths and reductions/shuffles/select have no real consumer. A SIMD raytracer is the smallest workload that exercises:
- `f32x4::new(x, y, z, 0)` for Vec3-as-vec4 packing
- `a.mul(b).sum()` for dot product (the v0.0.7 horizontal-reduce path)
- `a.add(b)` / `a.sub(b)` per ray bounce
- `a.lt(b).select(t1, t2)` for branchless front-face / inside-sphere selection

**Locked design decisions:**

1. **Same image hash as 1A.** The SIMD variant produces the same bytes — float rounding is identical because we use `fadd contract` semantics for both scalar and vector ops. Verifies the SIMD codegen produces the same arithmetic.

2. **No autovec disable.** Both scalar and SIMD variants build at `--release`. LLVM may autovectorize the scalar version; that's fine — the explicit-SIMD variant should still win because hand-packing into f32x4 saves the autovec analysis cost and the dot-product reduction is one `llvm.vector.reduce.fadd` call instead of three sequential floats.

3. **Cross-language SIMD ports.** Rust uses `std::simd` (nightly) or `wide`; Swift uses `simd_float4`; C uses `__attribute__((vector_size))`. The harness reports per-language SIMD-vs-scalar deltas.

**Tests:** same image hash as 1A. New regression guard: SIMD throughput stays within ±5% across cpc / Rust / C; if cpc drifts, the gap is the perf bug.

**Expected payoff:** the v0.0.7 SIMD method matrix gets its first real consumer. If a Vec3-shaped dot product doesn't beat scalar, that's a codegen bug we need to fix in v0.0.8; if it does, the SIMD foundation is vindicated.

---

### Phase 2 — Blessed `vendor/simd` package · size M

**Goal:** ship a stdlib-shaped library that wraps the v0.0.7 SIMD method matrix into a clean API. Same precedent as `vendor/stdlib` — one blessed external package per cycle, lives in-tree at `vendor/simd/`, every cpc consumer imports it the same way.

**Scope:**

- `Vec3 / Vec4 / Mat4x4` — minimal 3D math built on `f32x4` (so `Vec3` is `f32x4` with the 4th lane zeroed, `Mat4x4` is `[f32x4; 4]`).
- `simd::dot(a, b) / cross(a, b) / length(v) / normalize(v) / reflect(v, n) / refract(v, n, ratio)`.
- `simd::lerp(a, b, t)` with vector + scalar `t`.
- `simd::min(a, b) / max(a, b) / clamp(v, lo, hi)` — thin re-exports of the lane-wise primitives so consumers don't have to chain `f32x4::splat`.

**Why this exists:** the v0.0.7 method matrix is correct but raw. `f32x4::new(x, y, z, 0.0)` + `.mul(b).sum()` is wordy. A `Vec3 { x, y, z }` newtype wrapping the SIMD value, with `dot / cross / etc.` as inherent methods, is what consumer code actually wants. This is the same packaging step that stdlib's `Vec[T]` did for `malloc` + `realloc` raw allocation.

**Locked design decisions:**

1. **Library, not language.** No new compiler features. The package is pure C+ source code in `vendor/simd/src/*.cplus`. Validates that the v0.0.7 surface is sufficient for a real consumer.

2. **`f32x4` lane 4 is always zero for Vec3.** Documented invariant; `Vec3::new(x, y, z)` constructs `f32x4::new(x, y, z, 0.0)`. Dot product is correct because `a.lane[3] * b.lane[3] = 0`. Cross product uses `swizzle`.

3. **No Mat3x3.** 3D math libraries that use 3×3 matrices have to either pad to 3×4 (wastes the 4th lane) or hand-unroll (defeats the point). Mat4x4 is the universal shape; 3D linear ops live in the upper-left 3×3 submatrix.

4. **Consumed by Phase 1B raytracer.** The SIMD variant of the raytracer imports `vendor/simd` instead of using `f32x4::...` directly. This validates the package's API ergonomics.

**Tests:** unit (per-op) + e2e via Phase 1B's raytracer.

**Expected payoff:** the second blessed package after `stdlib`. Same role: prove the language surface is enough for users to write libraries, then ship one.

---

### Phase 3 — Second bindings package: `vendor/sqlite` · size M

**Goal:** typed C+ bindings to `libsqlite3`. Same shape as `vendor/appkit` but against a pure C ABI library (no ObjC, no Apple frameworks), to validate that the bindings model generalizes.

**Why this slice:** v0.0.7's open question was "pick a bindings package based on a workload that surfaces." SQLite is the obvious choice: ubiquitous, pure C ABI, one external dependency (`-lsqlite3` ships with macOS), no transitive headers. If `vendor/appkit`'s pattern works for ObjC-heavy AppKit, the same pattern against SQLite confirms the binding model isn't Apple-specific.

**Scope:**

- `vendor/sqlite/Cplus.toml` — `[link] libs = ["sqlite3"]`.
- `vendor/sqlite/src/lib.cplus` (or split per-concern: `open.cplus`, `prepare.cplus`, `bind.cplus`, `step.cplus`).
- API surface: `Database::open(path) / close / exec(sql) / prepare(sql) -> Statement`; `Statement::bind_i64(n, v) / bind_str(n, s) / step() -> StepResult / column_i64(n) / column_str(n) / finalize`.
- A `proves/benchmark/programs/06-sqlite-roundtrip/` recipe that uses the package to insert 10k rows, read them back, and verify (in Rust + Swift + C alongside, as is the harness convention).

**Locked design decisions:**

1. **No Drop on Database / Statement.** Same call shape as `vendor/appkit`: explicit `close()` / `finalize()`. Drop is a v0.1+ design question.

2. **Strings cross the FFI boundary as `*u8` C-strings.** No automatic bridging. Caller passes `"SELECT * FROM users\0".as_ptr()` (or `bridge::cplus_string_to_cstr`-style helper).

3. **Errors as `Result[T, i32]` where the i32 is the SQLite error code.** Sema's Phase 4 result types work today; this exercises them in a real consumer.

**Tests:** unit + the 06-sqlite-roundtrip e2e recipe.

**Expected payoff:** the bindings model is validated against a second, non-Apple library. After this, "could we bind X to C+?" is answered yes by precedent rather than by argument.

---

### Phase 4 — `env!("NAME")` builtin · size S

**Goal:** compile-time read of an environment variable into a `str`. Companion to the v0.0.7 `include_bytes!` / `include_str!` family.

**Why now:** v0.0.7's plan tagged this as "defer if no demand surfaces." The Phase 1A raytracer build surfaces the demand: a `RAYTRACE_SAMPLES_PER_PIXEL` env var is more convenient than recompiling for every tweak. Same shape as `include_str!`: macro-form `env!("NAME")`, sema-time read, returns `str`, error E0876 if unset.

**Locked design decisions:**

1. **Syntax:** `env!("NAME")` — same shape as `include_bytes!` / `include_str!`.
2. **Return type:** `str` (fat pointer to a `.rodata` global containing the var's value).
3. **Error E0876:** environment variable not set at compile time.
4. **No `option_env!`.** A nullable variant complicates the type signature; the strict form covers the common case (build-time config).

**Tests:** unit (parser + sema) + e2e (round-trip).

**Expected payoff:** completes the macro-shaped-builtins trilogy. The pattern is settled: any future "compile-time read X into Y" feature follows this template.

---

## Phase ordering rationale

- **Phase 1A first.** The raytracer is the prize; everything else can validate against it. Ship 1A standalone — even without the SIMD variant — and immediately measure the v0.0.7 TBAA + lifetime perf claims.
- **Phase 1B and Phase 2 are coupled.** The SIMD raytracer is the first consumer of `vendor/simd`. Land Phase 2 first (so the package exists), then Phase 1B (which imports it).
- **Phase 3 is independent.** Can land any time; pick when a real SQLite consumer surfaces or as filler.
- **Phase 4 lands alongside Phase 1A.** The env var read is a build-script convenience for the raytracer's tunables.

Estimated effort across all phases: ~5-6 sessions aggregate. Realistic v0.0.8 ship target: 3-4 of those sessions; if Phase 1B + Phase 2 take longer than expected, Phase 3 slips to v0.0.9.

---

## Open questions (do not block phase work)

- **Per-field TBAA tree** — v0.0.7 Slice 1.2 punted this with the rationale "ship when raytracer perf measures the win." Phase 1A + 1B make this measurable; if the raytracer's hot loops show a meaningful aliasing gap, the per-field tree lands as a follow-on slice in v0.0.8 or v0.0.9.
- **Mask types as a distinct `Ty` variant** — v0.0.7 Slice 2.1 aliased `mask32x4` to `i32x4` for simplicity. If the SIMD raytracer (Phase 1B) hits a real bug from the aliased shape (e.g. accidentally passing a non-mask `i32x4` to `select`), the cleanup lands; otherwise it sits.
- **Submodule re-export through `appkit/appkit` facade for functions** — re-litigated in v0.0.7 Slice 5.2 (still tentative). Phase 3's `vendor/sqlite` is the second binding package, which is the trigger for the rubric decision the v0.0.7 plan locked.
- **`#[align(N)]` for struct fields** — cut from v0.0.6 with the rationale "no concrete consumer yet." The SIMD raytracer's Vec3-as-f32x4 packing may surface alignment needs; if Phase 1B hits a misalignment trap or measurable perf loss, the attribute lands.
- **Threading the raytracer** — v0.0.5 shipped `thread::spawn` / `JoinHandle::join`. A trivial parallel-tiles raytracer (each thread renders an image row) is a one-session add-on once Phase 1A scalar ships, and would exercise the v0.0.5 thread surface against a real workload.
