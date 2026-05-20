# C+ — Plan

Version 0.0.6 shipped 2026-05-20. See [plan-0.0.6.md](plan-0.0.6.md) for the archived 0.0.6 roadmap and resolved log; [plan-0.0.5.md](plan-0.0.5.md) covers v0.0.5, [plan-0.0.4.md](plan-0.0.4.md) v0.0.4, [plan-0.0.3.md](plan-0.0.3.md) v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.7 — Carry-over polish: LLVM perf wins + SIMD expansion + macro-shaped builtins

**Strategy:** finish the v0.0.6 items that were deferred for principled reasons (each one a session-shaped refactor rather than a fast annotation), expand SIMD now that a real binding-package precedent exists (`vendor/appkit`'s shape vindicates the stdlib-model bet), and bless one or two more compile-time builtins where the metal_compute recipe surfaced a real need.

No new language features. No new principles. Everything here either finishes a previously-locked Phase 1 lane or extends the Phase 1B / Phase 2 surface along axes the v0.0.6 work explicitly anticipated.

Slice sizes use the same S/M/L assistant-paced framing as v0.0.6.

---

### Phase 1 — Carry-over from v0.0.6 Slice 1C · size M aggregate

Three sub-slices that v0.0.6 split out of the original 1C bundle when the work was actually attempted. Each needs a focused refactor pass, *not* an annotation. The v0.0.6 1C status table records the rationale for each deferral; this is the place where they ship.

#### Slice 1.1 — `llvm.lifetime.start` / `llvm.lifetime.end` on locals · size M

**Goal:** bracket every `alloca`'s live range with lifetime intrinsics so LLVM's SROA can reuse stack slots across non-overlapping scopes and shrink frame sizes.

**Why it was deferred from v0.0.6:** today every alloca is batched at the top of the function entry block. A function-wide bracket (start at fn entry, end at every `ret`) is mechanical but useless — LLVM already knows that lifetime. The per-scope implementation requires:

1. Each `alloca_*` helper records the *current scope frame* it was registered in (FnState already tracks frames for Drop / defer hooks).
2. At alloca emission, the lifetime.start intrinsic emits inline at the binding's `let` source position rather than batched at fn entry.
3. At scope pop, the existing `pop_scope` walks the per-frame alloca list in reverse and emits one `llvm.lifetime.end.p0(i64 size, ptr slot)` per binding.

Disabled at `-O0` (lifetime intrinsics confuse the debugger). Enabled at `-O2`/`-O3`.

**Expected payoff:** smaller stack frames, occasionally measurable speedups via better cache behavior on recursion-heavy code (raytracer's `ray_color`). Not dramatic, but free once shipped.

**Tests:** unit — every alloca in a non-debug build is followed by `llvm.lifetime.start` immediately, and `llvm.lifetime.end` appears before each scope exit. E2E — raytracer benchmark stays within ±1% (regression guard).

#### Slice 1.2 — TBAA (Type-Based Alias Analysis) metadata · size M

**Goal:** emit the standard TBAA tree at module init and tag every load/store with the right type leaf so LLVM's alias analysis can prove `*i32` and `*f64` don't alias.

**Why it was deferred from v0.0.6:** ~280 load/store emission sites in codegen, none centralized. Doing TBAA correctly requires migrating all sites to a `gen_load(ty, ptr) -> String` / `gen_store(ty, ptr, val)` helper that emits `, !tbaa !N` once at the helper. The migration is the cost; the TBAA emission itself is one line per helper.

**Plan:**

1. Introduce `gen_load` / `gen_store` helpers on `FnState`. Take the `Ty` so the helper picks the right TBAA tag.
2. Build the standard TBAA tree at module init in codegen, stored on `ModuleMetadata`:
   - `!0 = !{!"C+ TBAA Root"}`
   - `!1 = !{!"i8", !0, i64 0}` ... `!N = !{!"f64", !0, i64 0}` for each primitive
   - Struct + enum nodes inherit from `!0` for now; a per-field nested tree lands in v0.0.8 when we know it actually changes raytracer perf measurably.
3. Migrate the 280 sites in batches by file. Each batch: pick one source file, swap every direct `load`/`store` for `gen_load`/`gen_store`, run the full test suite, commit. ~10 commits, no behavioral diffs in between.

**Expected payoff:** highest-impact of the three carry-overs. Tight loops with mixed-type buffer access (Vec3 + Sphere field loads in the raytracer pattern) get measurable speedups because LLVM can hoist past disjoint-type accesses.

**Tests:** unit — TBAA tree appears in module preamble; a load of a primitive int carries `!tbaa !N` referencing the correct type node. E2E — raytracer benchmark stays within ±1% (regression guard); the goal is not to ship a measured win in this slice, just to not break.

#### Slice 1.3 — Statement-level attributes + `#[unroll(N)]` / `#[vectorize_width(N)]` · size M

**Goal:** ship `#[unroll(N)]` and `#[vectorize_width(N)]` on `while` / `loop` / `for` statements so SIMD package authors and hot-loop tuners can pass hints through to LLVM's loop optimizer.

**Why it was deferred from v0.0.6:** attribute parsing today is item-level only (functions, structs, enums, impl methods, fields). Statement-level attributes require parser + AST + attrs.rs + sema surgery before codegen can consume them.

**Plan:**

1. **Parser**: accept `#[name(args)] STMT` for `while` / `loop` / `for` statements. Reuse the existing `parse_attributes` helper; thread the result into a new `attributes: Vec<Attribute>` field on `StmtKind::While` / `StmtKind::Loop` / `StmtKind::For`.
2. **AST**: extend the relevant `StmtKind` variants with the new field. Walker stubs in borrowck / lower / monomorphize / resolver.
3. **attrs.rs**: add `unroll` and `vectorize_width` to `KNOWN_ATTRS` with `ArgsSpec::ExactlyOneInt` (new variant) and `targets: TARGET_LOOP_STMT` (new mask bit).
4. **Sema**: validate N is a literal in `[1, 256]`. Emit `E0510` (new code) for out-of-range.
5. **Codegen**: when a loop carries `#[unroll(N)]`, attach `!llvm.loop !M` metadata to the loop's back-edge branch where `!M = !{!M, !"llvm.loop.unroll.count", i32 N}`. Same shape for `vectorize_width`.

**Expected payoff:** explicit knob for hot inner loops. Marginal for general code; load-bearing for SIMD-package authors who know the right unroll factor.

**Tests:** unit — `#[unroll(4)] while ... { }` parses + sema-accepts; out-of-range N fires E0510; codegen attaches the metadata. E2E — a hand-tuned vector dot product reaches the expected single-call LLVM lowering.

---

### Phase 2 — SIMD expansion · size M aggregate

The v0.0.6 SIMD foundation is real: nine widths, the full method matrix for each, NEON codegen confirmed on AArch64-darwin, the `simd_dot` recipe ships. Two natural follow-on slices land next.

#### Slice 2.1 — Shuffles + reductions + masked ops · size M

**Goal:** add the remaining LLVM vector primitives that the v0.0.6 1B "first cut" intentionally deferred: per-lane permutations, horizontal reductions, and compare-and-blend.

**Method matrix to add** (per existing SIMD width):

| Method | LLVM lowering | Applies to |
|---|---|---|
| `swizzle(lanes: [u32; N])` | `shufflevector` with constant mask | all widths; lane count must match |
| `reverse()` | `shufflevector` with reverse mask | all widths |
| `interleave_lo(b)` / `interleave_hi(b)` | even/odd `shufflevector` masks | all widths |
| `sum()` | `llvm.vector.reduce.fadd.<vN>` / `add.<vN>` | all numeric widths |
| `product()` | `llvm.vector.reduce.fmul.<vN>` / `mul.<vN>` | all numeric widths |
| `min_across()` / `max_across()` | `llvm.vector.reduce.{fmin,fmax,smin,smax,umin,umax}.<vN>` | all numeric widths |
| `lt(b)` / `le(b)` / `gt(b)` / `ge(b)` / `eq(b)` / `ne(b)` | `fcmp`/`icmp` returning `<N x i1>` then sext to a mask vector | all numeric widths |
| `select(true_v, false_v)` | `select <N x i1>` (mask receiver) | all mask widths |
| `any()` / `all()` | `llvm.vector.reduce.or.<vN>` / `and.<vN>` on i1 vector then i1→bool | all mask widths |

**Mask types**: `mask8x16`, `mask16x8`, `mask32x4`, `mask64x2` ship at the same time — they are the comparison-result types and the `select` receiver. Lower to `<N x i1>` for the conceptual shape; codegen stores them as `<N x iN>` where `iN` is the width-matched signed int (NEON / SSE both prefer that for `vcmp` results).

#### Slice 2.2 — 256-bit widths + remaining 128-bit widths · size S

**Goal:** complete the width matrix listed in v0.0.6 plan-0.0.6.md but deferred in 1B's first cut.

**Widths to add**:

- 128-bit: `f64x2` ✓ (shipped), plus any width the 1B audit missed (review at slice start).
- 256-bit: `f32x8`, `f64x4`, `i8x32`, `i16x16`, `i32x8`, `i64x4`, `u8x32`, `u16x16`, `u32x8`, `u64x4`, mask variants.

**On AArch64-only hosts** the 256-bit widths still compile (LLVM splits into two 128-bit ops). Not as fast as native AVX2 but functionally correct — same behavior the v0.0.6 1B design doc anticipated.

**Defer:** 512-bit (`f32x16` etc.) until AVX-512 / SVE2 becomes tier-1.

---

### Phase 3 — Compile-time builtins beyond `include_bytes!` · size S

The v0.0.6 metal_compute recipe surfaced one concrete pain point — having to thread the embedded `.metallib`'s byte length through a build.sh sed-substitution dance. This phase ships the small fix.

#### Slice 3.1 — `include_str!("path")` · size S

**Goal:** companion to `include_bytes!` that returns a `str` (fat pointer `{ptr, len}`) instead of `*[u8; N]`. The user-visible difference: the length is part of the type, accessible via `str_len(s)`, so no out-of-band size threading is needed.

**Locked design decisions:**

1. **Syntax**: `include_str!("relative/path")` — same shape as `include_bytes!`, same `!` macro marker.
2. **Return type**: `str` (the existing fat-pointer view type). Read-only — the bytes live in a `.rodata` section; backed by an `[N x i8]` global; the `str` aggregate's `len` is the file's UTF-8 byte length.
3. **UTF-8 validation at compile time**: sema verifies the file's bytes are valid UTF-8. **E0875** fires on invalid byte sequences with the byte offset of the first bad byte. (`include_bytes!` doesn't validate — it returns raw bytes.)
4. **Same path resolution + same dedup** as `include_bytes!`. Same `MonoInfo::include_bytes` table (renamed to `compile_time_blobs`) covers both; the variant tag picks the return type.

**Where this lights up:**

- The `metal_compute` recipe's build.sh sed-substitution can go away. Replace the placeholder dance with a sibling `include_str!("../shaders/double_metallib.size")` whose contents is just the byte count produced by the metal compile step. Cleaner.
- General config-file embedding (test fixtures, lookup tables in JSON, etc.).

**Tests:** unit — parser + sema + codegen mirror of the existing `include_bytes!` tests; new test for UTF-8 validation rejecting a random-bytes file. E2E — round-trip embed of a known UTF-8 file, compare against `read_to_string` on the same file.

#### Slice 3.2 — `env!("NAME")` · size S (defer if no demand surfaces)

**Goal:** compile-time read of an environment variable into a `str`. Pure quality-of-life; not load-bearing for any in-tree recipe. Ship only if a real consumer asks; otherwise it slips to v0.0.8.

**E0876**: env var not set at compile time.

---

### Phase 4 — Recipe polish · size S aggregate

The two v0.0.6 GPU/SIMD recipes ship working today. Phase 4 fixes the rough edges the integration revealed.

#### Slice 4.1 — `metal_compute` simplification · size S

**Why:** the v0.0.6 recipe still uses `cpc --emit-ll | clang` because I incorrectly assumed `cpc build` didn't know about `-framework`. The follow-on `appkit_hello` recipe proved `cpc build` *does* honor `[link]` from dep manifests via `collect_dep_link_args`.

**Plan:**

1. Drop `build.sh`'s `--emit-ll | clang` two-step. Use plain `cpc build` after the `xcrun metal` step.
2. With Slice 3.1's `include_str!` shipped, drop the sed-substitution dance entirely — read the metallib size directly from a sibling file.
3. Update the README to match.

**Net result:** the recipe shrinks by ~50 lines, looks like the other recipes, and validates the same GPU pipeline.

#### Slice 4.2 — One more bindings package to validate the model · size S

**Open question:** which one? Candidates:

- `vendor/coreaudio` — small enough to bind in a session, validates "audio output" as a category beyond GUI.
- `vendor/sqlite` — purely C ABI (no ObjC), tests the bridge model against a non-Apple library.
- `vendor/metal` — natural follow-on to `metal_compute` + `appkit`. Heavy ObjC; would exercise the same surface as `vendor/appkit` against a different framework.

Pick one based on a workload that surfaces during v0.0.7. The point isn't completeness — it's having a second independent bindings-package data point to confirm the v0.0.6 stdlib-model bet generalizes.

---

### Phase 5 — `vendor/appkit` ergonomic polish · size S aggregate

The v0.0.6 Phase 2 work shipped a real consumer of cpc's ObjC interop. The recipe build hit two small papercuts that future appkit users will also hit.

#### Slice 5.1 — `NSString`-typed setter variants on the binding API · size S

**The papercut**: `Window::set_title(*u8)` and `TextField::set_string_value(*u8)` take a NUL-terminated C string and wrap via `rt::ns_string()` internally. That's fine for literals (`"Hi\0"`). It's *wrong* for dynamic strings — the user has a `string`, has to drop into raw `msg_void_id(label.obj, sel(...), bridge::cplus_string_to_nsstring(s))`, which defeats the typed-binding goal.

**Plan:** add `_ns` suffix variants on every widget setter that accepts a string-shaped arg. They take `*u8` already-NSString and skip the `ns_string` wrap.

```cplus
impl Window {
    pub fn set_title(self, title: *u8) { ... }            // existing; wraps via ns_string
    pub fn set_title_ns(self, title_ns: *u8) { ... }      // new; pass NSString through
}
```

Recipes that build dynamic content (via `bridge::cplus_string_to_nsstring`) call the `_ns` variant. Recipes with literals call the existing variant. Both paths are typed.

**Mechanical extent**: ~30 setter methods across `controls.cplus` / `text.cplus` / `window.cplus` / `dialogs.cplus`. ~1 session.

#### Slice 5.2 — `pub fn` re-exports through `appkit/appkit` facade · size S (TENTATIVE)

**Open question** — the v0.0.6 Phase 2 decision was deliberately *not* to re-export functions through the facade. The argument: consumer imports stay explicit, grep stays reliable, one canonical path per call.

**Counter-argument** that's emerged from writing the `appkit_hello` recipe: the consumer ends up importing 6 sub-modules. `import "appkit/runtime" as rt; import "appkit/application" as application; import "appkit/window" as window; import "appkit/view" as view; import "appkit/controls" as controls; import "appkit/convert" as bridge;`. That's signal at the cost of repetitive boilerplate.

**Decision rubric for v0.0.7:** if writing two more recipes (Phase 4.2 + something else) keeps hitting the same import pile, ship the re-export and document the rationale change. If two recipes naturally split into 2-3 sub-modules each (because they touch different concern areas), the explicit-imports model holds and the facade re-export stays cut.

**No code lands until the rubric decision in mid-cycle.**

---

## Phase ordering rationale

Loose dependency order:

- **Phase 1** (1.1 / 1.2 / 1.3) are independent codegen refactors. Any order works. TBAA (1.2) is highest-impact; do that first if perf is the v0.0.7 prize.
- **Phase 2** (SIMD expansion) is independent of Phase 1, but a TBAA-tagged vector load is a cleaner shape than an untagged one. Ship Phase 1.2 first if both are in scope.
- **Phase 3.1** (`include_str!`) unblocks Phase 4.1's cleaner `metal_compute`. Ship 3.1 before 4.1.
- **Phase 4** (recipe polish) and **Phase 5** (appkit ergonomics) are independent of everything else; pick them up when a relevant consumer surfaces.

The estimated effort across all phases is ~6 sessions aggregate. Realistic v0.0.7 ship target: 3–4 of those sessions, with the rest sliding to v0.0.8.

---

## Open questions (do not block phase work)

- **Submodule re-export through the `appkit/appkit` facade for functions** — the Phase 2 v0.0.6 cut. Re-litigated in Phase 5.2's rubric.
- **Blessed `vendor/simd` package** — v0.0.2 stdlib precedent suggests one. Still open: ship as a blessed external (like stdlib) or leave purely external. Phase 4.2's bindings-package choice can be `vendor/simd` to settle it.
- **Other macro-shaped builtins beyond `include_bytes!` / `include_str!`** — `env!`, `concat!`, `cfg!(target_os = "macos")`. Phase 3.2 is one; the others wait for a concrete use case to drive design.
- **Generic `nsarray_to_vec[T]` with element-type bound** — the v0.0.6 Phase 2B `convert.cplus` ships monomorphic helpers (`_i32 / _i64 / _f32 / _f64`) deliberately. Needs a `ToCocoa`/`FromCocoa` interface design before the generic path is right.
