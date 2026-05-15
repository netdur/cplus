# C+ — Plan

Version 0.0.1 shipped 2026-05-14. See [plan-0.0.1.md](plan-0.0.1.md) for the archived 0.0.1 roadmap and resolved-log.

---

## v0.0.2 — Performance, packages, language completeness

Four phases. Phase 1 closes the LLVM information gap (runtime perf) — **shipped 2026-05-14**. Phase 2 ships a minimum-viable package system so a stdlib can live outside the language. Phase 3 closes the agent-ergonomics gap surfaced by `proves/` (the catastrophic 04-curl-lite outlier: 39 turns / $1.74 for C+ vs 9–12 turns / $0.27–0.29 for Swift/Rust). Phase 4 (`cpc-bindgen`) is TBD pending Phase 3 lessons.

**Progress:**
- Phase 1 — ✅ done (7 slices + AppKit-via-`Cplus.toml` bonus; carryovers documented in the Phase 1 block below)
- Phase 2 — ✅ shipped 2026-05-15: 2A (manifest schema + [dependencies] + [link].bundled/triples), 2B (vendor resolver + strict path shape + migration), 2C (build-driver dep walk + manifest-is-truth verification with E0854/E0855/E0860/E0861/E0862/E0863), 2D (`tiny_source` / `tiny_artifact` smoke tests under `docs/examples/projects/`, design note [docs/design/phase2-packages-mvp.md](docs/design/phase2-packages-mvp.md), SKILL.md §1 updated). **Workspace: 811 lib + 11 e2e + 275 cpc tests green; both smoke tests build + run to exit 42.**
- Phase 3A — ✅ shipped (bitshifts + byte-swap intrinsics); 3B ✅ shipped 2026-05-15 (10 recipe programs + smoke tests); 3C pending
- Phase 4 — TBD (cpc-bindgen, depends on Phase 3 lessons)
- Phase 5 — ✅ shipped (C ABI export: `[lib]` manifest, aggregate ABI coercion, header generation, reference example). **798 lib + 11 e2e + 261 cpc tests green.**

**What `proves/` taught us** (full data in [proves/stats.md](proves/stats.md); friction-mode analysis in conversation history):
- C+ runtime perf is already great: smallest binary (33 KB), fewest cycles, lowest wall on fizzbuzz. The cost is *writing* C+, not *running* it.
- The 6× cost gap on 04-curl-lite is mostly **language-completeness gaps and missing recipes**, not pure stdlib absence. Hard numbers: 9 of the 39 turns (23%) were spent spelunking the compiler source to discover that **bitshift operators don't exist** (the agent needed `port >> 8` for network byte order). Another ~10 turns (26%) were SKILL.md/example-file paging — the agent has no internalized model and must re-derive idioms every session. 4 more turns repaired a `+%`-vs-`+` confusion on pointer arithmetic (pure SKILL.md gap).
- 03-hello-appkit was competitive (15 turns vs Swift's 27) because a near-complete 245-line reference existed at [objc-c-interop/cocoa-min/hello_appkit.cplus](objc-c-interop/cocoa-min/hello_appkit.cplus) and the agent did a near-verbatim adaptation. **Reference programs are a higher-leverage move than documentation.**

### Phase 1 motivation

The 0.0.1 codegen path is correct and idiomatic but conservative: it emits clean IR and hands it to clang at `-O2`. The borrow checker, the type system, and monomorphization all compute safety/range/uniqueness facts that get **dropped on the floor** before the IR reaches LLVM. Phase 1 publishes those facts as LLVM metadata and attribute annotations so the existing `-O2` pipeline can act on them.

Background analysis: [llvm.md](llvm.md). Survey of current codegen: AST lowers directly to LLVM IR text in [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs) (no MIR layer); the only aliasing attrs emitted today are `noalias` / `readonly` on by-ptr non-Copy struct params via `param_attr_prefix` at [codegen.rs:726](cplus-core/src/codegen.rs#L726). Everything else listed in §4.3 of the archived plan ("Performance unlocks the borrow checker hands us") is unshipped despite the §4.7 table claiming Phase 2/3 status — that table was aspirational. Phase 1 of v0.0.2 closes the gap.

**Guiding principle.** Every attribute or metadata added must correspond to a fact the frontend has already proved. No speculative annotations. If sema/borrowck can't justify it, it doesn't get emitted. `noundef`, `nonnull`, `noalias`, `dereferenceable` are all unsound if the proof is wrong — LLVM treats violations as UB.

### Phase 1 — LLVM information dividend · ✅ shipped 2026-05-14

Three tiers, sliced by effort and ROI. Tier 1 is pure metadata/attribute emission with no semantics changes. Tier 2 reshapes a small number of calling-convention decisions. Tier 3 is structural and deferred — listed for completeness with revisit criteria.

**Original estimate:** 3–5 weeks (human-paced). Actual: one assistant session. The estimate was right for the *amount of work* and wrong for the *coding speed* — code generation in a well-mapped codebase compresses what would have been days of typing into minutes, but the design decisions, risk calls, and verification reads stay roughly constant. Future C+ phase estimates should split "design + verification time" from "raw typing time" — only the latter is meaningfully shorter when the assistant does the keystrokes.

**Status:** All 7 slices + AppKit-via-`Cplus.toml` bonus shipped in one pass. Workspace tests: 745 lib + 11 e2e + 229 cpc = green. All 4 `proves/` benchmarks build via `cpc build --release`. Post-Phase-1 measurement of 02-fizzbuzz recorded in [proves/stats.md](proves/stats.md) — deltas inside the ±5% noise floor, which is the expected reading because fizzbuzz exercises none of the Phase 1 features (no enums, no slice indexing, no mut-disjoint pairs, no tail recursion, no large struct returns).

**What landed (slice → key emitted artifacts):**
- 1A — full param attr set: `noalias`/`readonly`, `nonnull`, `noundef`, `dereferenceable(N)`, `align A` on pointer-passed params + receivers; `noundef` on scalar value-passed params. Static layout calculator at [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs) (`static_layout`).
- 1B — `!range !{i32 0, i32 N}` on enum tag loads; `llvm.assume(idx ult N)` after bounds checks; `llvm.assume(sge len, 0)` after `slice_len`. Module-level metadata table starting at id `!100000` to avoid colliding with DWARF.
- 1C — per-function `!alias.scope` domain + per-mut-param scope; post-pass dataflow propagates scope through GEP→load/store chains; gated on ≥2 noalias-shaped params (no payoff with 1).
- 1D — `sret({ ptr, i64, i64 })` for owned `string` returns (narrow, deliberately conservative — see carryover).
- 1E — `musttail call` for direct tail-position calls to known-signature non-variadic non-method targets with no pending drops/defers.
- 1F — `preserve_nonecc` + `cold` on synthesized drop-glue method definitions and matching call sites.
- 1G — `cpc --emit-ll-opt FILE` / `cpc --emit-asm FILE`, routing IR through clang at the build mode's optimization level.
- Bonus — `[[bin]] frameworks = [...]` / `libs = [...]` in `Cplus.toml`, expanded to `-framework <name>` / `-l<name>` at link time. Unblocks `cpc build` on 03-hello-appkit.

**Carryover** (deferred, not blocker):
- Slice 1D is narrow: `sret` fires only for owned `string` returns. The predicate `return_passes_by_sret` at [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs) is the single switch — widening to non-Copy structs >16 bytes is a contained follow-up PR. Risks listed below in the original Slice 1D block remain valid; the main test-surface concern is e2e tests that pin `define %B @foo(...)`-style signatures.
- Slice 1C is also narrow: scopes are emitted only for pointer-passed `mut`/`move` params, not for `let mut` non-Copy locals. Local-binding scopes would compound the win after inlining but require pre-allocating scopes during codegen (the current implementation pre-collects param SSA names; locals don't have stable names until the alloca is emitted). Contained follow-up.
- No `proves/` benchmark exists yet that hits Phase 1 features. Three candidates suggested in [proves/stats.md](proves/stats.md): slice-indexed loop, deep tagged-union walk, big-struct `swap`. Building one of these would close the "we have nothing to measure against" gap that surfaced at the end of Phase 1.

#### Slice 1A — `nonnull` + `dereferenceable` + `align` + `noundef` on borrow params · Tier 1 · ✅ done

**Goal:** Promote the existing `noalias`/`readonly` annotation site at [codegen.rs:726](cplus-core/src/codegen.rs#L726) to emit the full attribute set the borrow checker actually justifies.

**Facts already proven:**
- `&T`/`&mut T`-shaped params (in C+ surface form: `x: T` / `mut x: T` for non-Copy `T`) cannot be null — C+ has no null in safe code (§2.9; cross-ref [feedback_cplus_no_null.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_cplus_no_null.md)).
- Definite-assignment (slice 3J) means every value at a parameter slot is initialized — `noundef` applies.
- Type layout is fully known via the slice 11.LAYOUT `size_of`/`align_of` infrastructure — `dereferenceable(N)` and `align N` are exact, not lower-bounds.

**Codegen changes:**
- Extend `param_attr_prefix` (currently returns `"noalias "` / `"readonly "` / `""`) to a `param_attrs` builder that composes:
  - `noalias` (mut, existing) or `readonly` (shared, existing)
  - `nonnull` (always, when pointer-passed)
  - `noundef` (always, when pointer-passed — definite assignment + non-null)
  - `dereferenceable(N)` where `N = size_of(T)` from the TypeTable
  - `align A` where `A = align_of(T)`
- Apply the same set to `self` / `mut self` receivers in [codegen.rs](cplus-core/src/codegen.rs) method lowering.
- For non-Copy struct returns via `sret` (Slice 1D below), apply `noalias`, `noundef`, `nonnull`, `writable`, `dereferenceable(N)`, `align A` to the sret pointer.

**Sema:** No changes. All proofs exist.

**IR shape (before → after):**
```
; before
define void @Point__translate(ptr noalias %p, i32 %dx, i32 %dy)

; after
define void @Point__translate(ptr noalias nonnull noundef align 4 dereferenceable(8) %p, i32 noundef %dx, i32 noundef %dy)
```

**Tests:**
- Unit: `param_attrs` returns expected attr strings for each combination of (move/shared/mut, Copy/non-Copy, struct/primitive).
- Codegen snapshot: emit IR for a function with one of each param shape; pin the attribute set.
- E2E: end-to-end build of all `proves/` benchmarks still passes; no regression in runtime correctness.
- Negative: pass `unsafe` reinterpret of zero to a non-null param at runtime is a borrow-checker bug, not in scope here — but add a unit test that the integer-to-pointer cast (slice 11.INTPTR) **does not** propagate `nonnull` from a typed primitive.

**Exit:** `cpc --emit-ll` on a representative sample (one Copy primitive arg, one non-Copy `mut`, one non-Copy shared, one slice) produces IR containing all attrs above; `-O2` output measurably elides at least one null check (verify via `cpc --emit-asm` diff).

#### Slice 1B — `!range` metadata on enum tag, slice length, bounds-checked indices · Tier 1 · ✅ done

**Goal:** Publish the integer ranges sema already knows, so `-O2`'s `ConstraintElimination` and `InstCombine` can remove redundant checks downstream.

**Facts to publish:**
- **Enum discriminants.** An `enum` with N variants lowers to `iN'` (`i32` per slice 2A). The runtime tag is always in `0..=N-1`. Emit `!range !{i32 0, i32 N}` on every load of the tag (match dispatch in [codegen.rs](cplus-core/src/codegen.rs) match-lowering) and on enum-typed function arguments.
- **Slice length.** `slice_len` extracts the `i64` length from a `{ ptr, i64 }` fat pointer. Length is always `>= 0`; for a slice constructed from a known array of size `K`, length is exactly `K`. Emit `!range !{i64 0, i64 9223372036854775807}` (non-negative) on every `extractvalue` of the len field; emit a tighter range when the slice came from a literal or fixed-size array.
- **Bounds-checked indices.** After the bounds check (`if i < len { ... }`), `i` is `0..len` on the success branch. Insert `llvm.assume(icmp ult i, len)` immediately after the bounds-check branch so a subsequent index in the same block can be elided.
- **`size_of[T]()` / `align_of[T]()` results.** Both return `usize` and are always `>= 1` (or `>= 0` for ZSTs — verify ZST policy). Mark the intrinsic call sites with `!range`.

**Codegen changes:**
- Add a `!range` helper that allocates a metadata node with the next available `!N` id and emits the literal at the top of the module (similar to how DWARF metadata is laid out today).
- Add `range_for_arg(ty)` returning `Option<(min, max)>` keyed off `Ty::Enum(id)`, `Ty::Bool`, and (later) tagged-union discriminant slots.
- Wire `!range` onto: function-arg loads (for `noundef`-eligible primitive args with bounded type), tag loads in match lowering, `slice_len` extractvalue results.
- Emit `llvm.assume` after each bounds-check branch in slice-indexing lowering.

**Sema:** No changes for enums/slices/intrinsics. For the bounds-check `assume`, no sema change — the assume is implied by the branch.

**IR shape:**
```
; enum tag load in match
%tag = load i32, ptr %opt, !range !42
; ...
!42 = !{i32 0, i32 2}   ; for Option (Some/None)

; slice length
%len = extractvalue { ptr, i64 } %s, 1, !range !43
!43 = !{i64 0, i64 9223372036854775807}

; bounds-checked index
%ok = icmp ult i64 %i, %len
br i1 %ok, label %ok_blk, label %trap
ok_blk:
  call void @llvm.assume(i1 %ok)
  ; subsequent uses of %i can assume %i < %len
```

**Tests:**
- Unit: `range_for_arg` returns `Some((0, N))` for an `N`-variant enum, `Some((0, 1))` for `bool`, `None` for unbounded `i32`.
- Codegen snapshot: match on a 3-variant enum produces a tag load with `!range !{i32 0, i32 3}`.
- E2E perf check: write a tight loop summing `s[i]` for `i in 0..s.len()`; `-O2` should remove the per-iteration bounds check (compare emit-asm before/after).

**Exit:** A loop indexing a slice without explicit `unsafe` produces the same hot-loop assembly as a hand-written raw-pointer loop at `-O2`. Spot-check one `proves/` benchmark improves on this metric.

#### Slice 1C — Scoped `!alias.scope` / `!noalias` for local bindings · Tier 1 · ✅ done (param-only; local-binding scopes deferred — see Carryover)

**Goal:** The existing `noalias`/`readonly` param attrs only survive the *uninlined* function. After inlining (which `-O2` does aggressively), the attrs degrade. Scoped alias metadata survives inlining and feeds the loop vectorizer.

**Facts to publish:**
- The borrow checker (Phase 6) proves that for every `mut x: T` non-Copy binding, no other live pointer in the same function reaches the same memory. That's exactly the disjointness needed for `!alias.scope`.

**Codegen changes:**
- Per function: emit one `!noalias.domain` node (one domain per function = "this call frame").
- Per `let mut` binding *and* per pointer-passed `mut` parameter: emit a unique `!alias.scope` within the domain.
- Tag every `load`/`store` through that binding with `!alias.scope !{scope_of_binding}` and `!noalias !{all_other_scopes_in_function}`.
- Implement as a small map `binding_id -> scope_md_id` carried through codegen alongside the existing alloca tracking.

**Sema/borrowck:** No changes — borrow checker already enforces the disjointness, we just publish it.

**IR shape:**
```
define void @swap(ptr noalias %a, ptr noalias %b) {
  %va = load i32, ptr %a, !alias.scope !10, !noalias !11
  %vb = load i32, ptr %b, !alias.scope !11, !noalias !10
  store i32 %vb, ptr %a, !alias.scope !10, !noalias !11
  store i32 %va, ptr %b, !alias.scope !11, !noalias !10
  ret void
}
!9  = !{!9}              ; domain for @swap
!10 = !{!10, !9}         ; scope for %a
!11 = !{!11, !9}         ; scope for %b
```

After inlining into a caller, the domain stays attached and the optimizer can still prove the loads/stores don't alias even though the `noalias` param attr is gone.

**Tests:**
- Unit: scope-id allocator is per-function (resets between functions).
- Codegen snapshot: a function with two `mut` params emits two scopes in one domain; the loads/stores reference both correctly.
- E2E: take a function that gets inlined at `-O2` and verify (via `cpc --emit-ll-opt` — see Slice 1G below) that post-inline IR preserves the scope metadata.

**Exit:** A small benchmark exercising vectorizable `mut`-disjoint slice writes (e.g., `for i in 0..n { dst[i] = src[i] + 1 }` where `dst` and `src` are both `mut`-passed) shows vectorized output without runtime aliasing checks.

#### Slice 1D — `sret` for large non-Copy value returns · Tier 2 · ✅ done (narrow: owned `string` only — see Carryover)

**Goal:** Today, all non-Copy returns "move" — the survey shows no `sret` attribute is used. For aggregates larger than 2 registers (slices, strings, multi-field structs), this means LLVM lowers via stack-passed memory anyway but without the `sret`/`noalias`/`writable` annotations that enable copy elision.

**Codegen changes:**
- Add `return_passes_by_sret(ty)`: true when `ty` is a non-Copy struct, slice, owned string, or any aggregate exceeding a target-specific size threshold (start with `> 16 bytes`).
- For sret returns: rewrite the IR function signature from `define { ptr, i64 } @f(...)` to `define void @f(ptr sret({ ptr, i64 }) noalias nonnull noundef writable dereferenceable(16) align 8 %ret, ...)`.
- At call sites: allocate the result in the caller's frame, pass the alloca as the first arg, treat the call as `void`-returning.
- Composes with Slice 3F (Drop) — the sret pointer is the place where the returned value lands, so Drop registration happens against that place.

**Sema/borrowck:** No changes. `sret` is a pure ABI lowering decision.

**Risk:** This is a non-trivial calling-convention change. Must verify (a) all `proves/` benchmarks still link and run; (b) extern-fn boundary unaffected (extern fns keep C ABI); (c) recursive functions returning aggregates don't blow the stack via extra alloca.

**Tests:**
- Unit: `return_passes_by_sret` decisions for each aggregate kind.
- Codegen snapshot: pin the rewritten signature for a function returning a slice.
- E2E: every existing test in the suite passes unmodified (this is a regression-prevention exit).

**Exit:** All existing tests pass; `cpc --emit-asm` on a function like `fn make_slice() -> i32[]` shows the return value being constructed in-place at the caller's alloca rather than via intermediate copies.

#### Slice 1E — `musttail` for tail-position `return foo(args)` · Tier 2 · ✅ done

**Goal:** Today no `tail` / `musttail` markers are emitted. For recursive functions (factorial, fibonacci-style, tagged-union walks), a tail-position call followed by `return` could be a guaranteed tail call, eliminating the stack-frame cost.

**Codegen changes:**
- Add a `is_tail_return(expr)` predicate over the lowered statement: `return foo(args);` where `foo`'s return type matches the enclosing function's return type and the callee uses the same calling convention.
- Emit `musttail call` for matching sites. Fall back to plain `call` if any criterion fails (ABI mismatch, sret-vs-value mismatch, generic parameter shape difference).

**Sema/borrowck:** No changes. The borrow checker already enforces that all locals are dropped before `return`, so the tail-call destruction order is preserved.

**Risk:** `musttail` is strict — LLVM rejects the IR if the call doesn't truly qualify. Need a clean fallback path. Also: defer/drop emit code *after* the call expression in the lowering, which would prevent `musttail`; the predicate must check that no defer/Drop runs between the call and the return.

**Tests:**
- Unit: tail-position predicate for a hand-rolled set of patterns (matching ret types, mismatched, with intermediate let, with defer in scope).
- Codegen snapshot: a `match`-arm-only recursive sum function emits `musttail`.
- E2E: a deeply recursive tagged-union walk (10k depth) terminates without stack overflow in `--release`.

**Exit:** Deep tagged-union recursion runs in O(1) stack at `-O2`. (Note: at `-O0` `musttail` is still honored by LLVM, so this should hold even in debug builds.)

#### Slice 1F — `preserve_none` calling convention for cold helpers · Tier 2 · ✅ done

**Goal:** Today all functions use the default C calling convention. Hot internal callers pay the full callee-save register cost for tiny helper calls. The `preserve_none` CC has no callee-save registers — ideal for: drop-glue thunks, the overflow-trap helper, the bounds-check trap helper, and any internal `#[cold]` path.

**Codegen changes:**
- Add a function attribute `cc preserve_nonecc` on emitted `define`s for compiler-synthesized helpers:
  - Drop-glue per-type drop functions (slice 3F infrastructure)
  - Trap dispatch helpers if/when factored out (currently inline `llvm.trap` — may not need extraction)
- Apply `cold` attribute to the same set.
- Verify the platform target supports `preserve_nonecc` (LLVM 17+; cpc currently uses whatever clang ships).

**Sema/borrowck:** No changes.

**Risk:** `preserve_none` interacts with `musttail` (Slice 1E) — both functions must agree on CC. Low risk because drop-glue is rarely tail-called.

**Tests:**
- Codegen snapshot: a drop-glue function emits with `preserve_nonecc cold` attributes.
- E2E: all existing tests pass; benchmark a tight `Drop`-heavy loop (e.g., allocating and dropping a string in a hot loop) and verify no regression at `-O2`.

**Exit:** Drop-glue functions emit with `preserve_nonecc` and `cold`; tight Drop loops show at least no regression and ideally a small improvement at `-O2`.

#### Slice 1G — Tooling: `cpc --emit-ll-opt` for post-pass IR inspection · supporting infrastructure · ✅ done

**Goal:** Slices 1A–1F can't be validated without seeing the *post-optimization* IR. Today `cpc --emit-ll` shows pre-LLVM IR only.

**Driver changes ([cpc/src/main.rs](cpc/src/main.rs)):**
- Add `--emit-ll-opt` flag that pipes the IR through `clang -O2 -S -emit-llvm` (or directly to `opt -O2 -S` if `opt` is available) and prints the result.
- Add `--emit-asm` flag that pipes through `clang -O2 -S` for assembly inspection.
- Both flags compose with `--release` (default `-O2`) and `--debug` (`-O0`).

**Tests:**
- E2E: `--emit-ll-opt` on a known-vectorizable function produces IR containing `<4 x i32>` (or similar vector type) on platforms with SIMD.

**Exit:** All slice-1B/1C exit criteria are testable via `--emit-ll-opt` output.

#### Slice 1H — Tier 3: deferred items with revisit criteria

The following are documented here so future contributors don't re-derive the analysis. Not on the v0.0.2 milestone.

- **Byte type (`b8`) for memcpy provenance.** Defer until LLVM stabilizes the byte type lowering across all backends C+ targets (x86_64, arm64). Revisit when LLVM 20+ ships with `b8` documented as stable. Expected payoff: 0.2–0.8% on memcpy-heavy workloads. Adoption is cheap because codegen emits IR as text — flip the type letter in one place.
- **Custom address-space pointer compression.** Defer until C+ has a heap-allocator story (currently stack + `Box` only; no GC, no large heap). Revisit if/when `Vec[T]` + `Allocator` interface (parked from Phase 7HEAP reframe — see [project_cplus_7heap_reframe.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/project_cplus_7heap_reframe.md)) creates a use case where 32-bit heap offsets pay off. Estimated 20–50% peak heap reduction in pointer-heavy data structures *if* a use case exists.
- **MLIR / multi-level IR.** Defer indefinitely. Today's pipeline (AST → Lower → Sema → Borrowck → Monomorphize → Codegen) has no MIR. The architecturally honest next step is a typed MIR layer between monomorphize and codegen — which unlocks iterator/closure fusion and trait-method devirt before LLVM sees them. MLIR itself is a v1.x architectural decision, not a v0.0.x decision. Revisit when:
  - Closures land (not on roadmap), or
  - Trait-object devirtualization becomes a measured bottleneck, or
  - A SIMD/GPU offload story is wanted.
- **`inalloca` for argument passing.** Rejected. `inalloca` is primarily a Windows-x86 ABI feature; C+ already passes non-Copy structs by-ptr with `noalias`/`readonly` (covering most of the win) and Slice 1D adds `sret` for returns (covering the rest). No revisit unless Windows-x86 specifically becomes a tier-1 target with measurable regression.

### Phase 1 exit criteria

- Slices 1A through 1F shipped, each with full unit + e2e coverage per project test discipline ([feedback_test_discipline.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_test_discipline.md)).
- `--emit-ll-opt` ships (Slice 1G) and is documented in `cpc --help`.
- Every `proves/` benchmark either improves or stays within noise at `-O2`; no regression at `-O0`.
- At least one benchmark in `proves/` demonstrates a measurable win attributable to the new metadata (vector loop, deep recursion, or eliminated redundant check).
- Documentation: update §4.3 of the archived plan's LLVM-strategy section *or* mirror the updated table into this milestone's plan section before v0.0.2 ships, so the "performance unlocks the borrow checker hands us" claim matches reality.

### Phase 1 non-goals

- Custom LLVM passes. The whole point is to feed the *existing* `-O2` pipeline better signal.
- A pass manager in-process. Continue invoking clang for optimization and linking — same architecture as v0.0.1.
- Surface-language changes. Phase 1 is invisible at the source level.
- Performance regressions to compile time. The added metadata is bounded per-binding and per-function; expected compile-time delta is well under 5%.

### Phase 2 — Package system MVP · est. 1–2 weeks

The full package-manager design lives in [pm.md](pm.md). v0.0.2 ships **the smallest slice that runs end-to-end**: a per-project `vendor/` directory that users populate manually (`git clone`, submodule, `curl | tar`), a unified `Cplus.toml` schema that works in both consumer projects and vendor packages, and a build driver that auto-collects each package's linker hookups. No fetch tool, no lockfile, no resolver, no sandbox, no capabilities. Those are forward-compatible additions in later milestones once the resolution shape is locked.

**Why MVP, not full pm.md:** the proves/04-curl-lite cost data (39 turns, $1.74 for C+ vs 9–12 turns at $0.27–0.29 for Swift/Rust) says the agent-ergonomics gap is structural — no stdlib, libc by hand. Closing that gap needs a stdlib *package*, not a finished package manager. MVP lets the stdlib ship and unblock real programs; the rest of pm.md can land incrementally as actual pain shows up.

**Locked design decisions** (settled in v0.0.2 design discussion, 2026-05-14):

1. **`cpc` is a C+ compiler + linker driver. It is never a C compiler.** A package never contains `.c`/`.cpp`/`.m` source files. If a package needs native code, it ships prebuilt artifacts (`.a`) and/or declares system libs/frameworks for the linker. Compiling those native artifacts happens *outside* the C+ ecosystem (upstream project's own build, vendor SDK, etc.). This rules out build scripts and multi-language manifests in one stroke.

2. **String stays in the language.** Owned `string` + view `str` are primitive types (slice 8.STR.* shipped 2026-05-13). Not a package.

3. **Vendor directory: `vendor/`.** Name chosen over `cpc_modules/` for forward-compatibility with the eventual "resolved deps land here" role. Matches Go's existing convention.

4. **Unified manifest format.** A vendor package's `Cplus.toml` is the *same file format* as a consumer's `Cplus.toml`. Different fields are populated. Package mode is implicit: dir lives under `vendor/`, has `[package]`, may have `[link]`, doesn't need `[[bin]]`. Consumer mode: lives at project root, has `[package]` + `[[bin]]` + `[dependencies]`.

5. **Import surface: strict, not fallback. No `.cplus` extension in import paths.**
   - `./foo` / `../foo` → file-relative; resolves to `<dir>/foo.cplus` (or `<dir>/../foo.cplus`).
   - bare `stdlib/env` → vendor; resolves to `<consumer>/vendor/stdlib/src/env.cplus`. First segment must match a declared dep.
   - No fallback chain. Aligned with §"no several ways to do the same thing". The `.cplus` extension is conventional and known to the resolver — users never write it.

6. **No `.cplus` files at the package root.** A package directory contains only `Cplus.toml` + `README.md` + `src/`. All importable C+ code lives under `src/`. Visibility is purely `pub`-keyword-based — there is no file-level "public dir" vs "private dir" split. A file is reachable cross-package iff its containing package is in `[dependencies]` and its items are `pub`.

7. **Future `cpc fetch` default: local.** When fetch lands, no flag → populate `./vendor/` (common case); `-g` flag → global cache. Same shape as `npm install` / `npm install -g`. MVP's vendor path is exactly where eventual fetch lands; no migration cost.

**Package layout** (the four kinds, all using the same `Cplus.toml` schema):

```
vendor/<name>/
├── Cplus.toml                ← [package] required; [link] optional
├── README.md                 ← optional; package metadata for humans
└── src/                      ← all importable C+ code lives here
    ├── foo.cplus             ← importable from a consumer as "<name>/foo"
    ├── bar.cplus             ← importable as "<name>/bar"
    ├── internal_helpers.cplus  ← also importable as "<name>/internal_helpers";
    │                           items without `pub` are unreachable cross-file
    ├── sub/                  ← sub-dirs work: "<name>/sub/baz"
    │   └── baz.cplus
    └── lib/                  ← bundled prebuilt artifacts (only if declared in [link].bundled)
        ├── aarch64-apple-darwin/
        │   └── foo.a
        └── x86_64-unknown-linux-gnu/
            └── foo.a
```

| Mode | `[link]` fields | `src/lib/` | Example |
|---|---|---|---|
| **Pure C+ source** | absent | empty | `stdlib`, with `src/result.cplus` containing `pub enum Result[T, E]` |
| **System-libs only** | `libs` and/or `frameworks` | empty | `appkit`, with `src/appkit.cplus` declaring `extern fn` into Cocoa + `[link] frameworks=["Cocoa"]` |
| **Bundled artifact + transitive system libs** | `bundled` + `triples` (+ `libs` for transitive system deps) | matching files per triple | `curl_bindings`, with `src/curl.cplus` + `src/lib/<triple>/curl.a` + `[link] bundled=["curl.a"] triples=["aarch64-apple-darwin"] libs=["z"]` |
| **Mixed source + artifact (per-module)** | `bundled` for the modules backed by binaries | only the declared `.a` files | Source for portable code, artifact for hot loops |

**Manifest = single source of truth.** The build driver does NOT scan
`src/lib/<triple>/` to discover linkable artifacts. Every prebuilt `.a`
that should be linked into a consumer must be named in `[link].bundled`,
and every triple the package supports must be named in `[link].triples`.
The driver verifies the filesystem matches what the manifest declares,
and fails both ways:

- **E0860 — declared but absent:** `[link].bundled = ["curl.a"]` but
  `src/lib/<host-triple>/curl.a` doesn't exist. Either ship the binary
  or remove the declaration.
- **E0861 — present but undeclared:** a `.a` file exists under
  `src/lib/<triple>/` that isn't in `[link].bundled`. Either declare it
  or delete the file. (Catches the "developer shipped curl.a but forgot
  to tell the manifest" case the user surfaced 2026-05-15.)
- **E0862 — host triple unsupported:** the consumer's host triple isn't
  in this package's `[link].triples` list. The author hasn't built for
  this platform; the build fails loudly, not silently with garbage symbols.

This is intentionally strict: no implicit discovery, no "the build figured
it out from what's on disk". If the manifest doesn't say it, the linker
doesn't get it; if the filesystem disagrees, the build refuses.

**Sample vendor-package manifest** (the `appkit` case — pure system-libs, no shipped binaries):
```toml
[package]
name    = "appkit"        # must match the parent dir name
version = "0.1.0"
edition = "2026"

[link]
frameworks = ["Cocoa"]
libs       = ["objc"]
# `bundled` absent → no binaries shipped → no `triples` required.
```

**Sample vendor-package manifest** (the `curl_bindings` case — ships a prebuilt `.a`):
```toml
[package]
name    = "curl_bindings"
version = "0.3.0"
edition = "2026"

[link]
# This package's own shipped artifacts. Each entry is a basename;
# the file must exist at src/lib/<host-triple>/<basename> for every
# triple in the `triples` list. Missing file → E0860; orphan file → E0861.
bundled    = ["curl.a"]
triples    = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"]
# Transitive system libs the bundled `.a` itself requires (curl needs z).
libs       = ["z"]
```

**Sample consumer manifest** (importing the above):
```toml
[package]
name    = "my_app"
version = "0.1.0"
edition = "2026"

[[bin]]
name = "my_app"
path = "src/main.cplus"

[dependencies]
appkit = "*"              # version string parsed but unused today
```

`cpc build` walks the consumer's `[dependencies]`, opens each
`vendor/<name>/Cplus.toml`, and constructs the link line entirely from
declared fields: `[link].frameworks` → `-framework <name>`, `[link].libs`
→ `-l<name>`, `[link].bundled` → `<pkg>/src/lib/<host-triple>/<file>`
(full path; not `-l`). Before splicing, the driver verifies every
declared file is present (E0860), every present file under
`src/lib/<triple>/` is declared (E0861), and the consumer's host triple
appears in `[link].triples` whenever `[link].bundled` is non-empty
(E0862). The consumer's source uses `import "<name>/<module>"` with no
extension; the resolver appends `.cplus` and prepends `src/` to locate
the file inside the vendor package.

#### Slice 2A — Unified `Cplus.toml` schema · `[dependencies]` and `[link]` · ✅ done

**Shipped:** `[dependencies]` parsing (name = "version-string" pairs; version string accepted but unused at resolution time, MVP); name validation (E0857 — `[a-z][a-z0-9_]*` only, no dots/slashes/uppercase). Top-level `[link]` table with `frameworks`, `libs`, `bundled`, `triples` fields. Bundled-requires-triples enforcement (E0863) — declaring `bundled` without `triples` fails at parse time. 13 manifest unit tests cover the positive cases and every diagnostic. Surface: `[package]`, `[[bin]]`, `[lib]`, `[link]`, `[dependencies]` all in one file; consumer vs vendor manifest is implicit by which sections are populated.


**Goal:** Extend the existing `Cplus.toml` parser at [cplus-core/src/manifest.rs](cplus-core/src/manifest.rs) (currently ~340 lines after Phase 1 added `frameworks`/`libs` to `[[bin]]`) to:
1. Parse a `[dependencies]` table on consumer manifests.
2. Parse a `[link]` table on vendor-package manifests.

**Manifest changes:**
- Add `pub dependencies: Vec<Dependency>` to `Manifest`, where `Dependency { name: String, version: String, declared_span: Span }`. Names must match `[a-z][a-z0-9_]*` (lowercase identifier shape, no dots, no slashes).
- Add `pub link: LinkSection` (frameworks + libs) to `Manifest`. Both fields default `[]`. Mirrors the per-`[[bin]]` `frameworks`/`libs` shipped in Phase 1's AppKit-via-`Cplus.toml` work.
- Both new tables are optional. Existing manifests parse unchanged.

**Build-driver changes ([cpc/src/main.rs](cpc/src/main.rs)):**
- Pre-resolution sanity pass over `manifest.dependencies`:
  - **E0850** "package `stdlib` declared in `[dependencies]` but not found in `vendor/stdlib/`" — suggestion: "drop the package into `vendor/stdlib/` (e.g. `git clone <url> vendor/stdlib`)".
  - **E0851** "vendor directory `vendor/stdlib/` has no public `.cplus` files at its root" — package is structurally broken.

**Sema/codegen:** No changes.

**Tests:**
- Unit: parse a manifest with `[dependencies]` containing one, many, and zero entries; reject invalid name shapes.
- Unit: parse a vendor manifest with `[link].frameworks` / `[link].libs`; defaults to empty when absent.
- E2E positive: project with `[dependencies] stdlib = "*"` and a populated `vendor/stdlib/` builds.
- E2E negative: same project without `vendor/stdlib/` fails with E0850; with empty `vendor/stdlib/` fails with E0851.

**Exit:** `cpc build` on a manifest with declared dependencies enforces presence + structural integrity; diagnostics carry actionable suggestions.

#### Slice 2B — Import resolver: vendor lookup + strict path-shape rule + no extension · ✅ done

**Shipped:** `classify_import_path` in [cplus-core/src/resolver.rs](cplus-core/src/resolver.rs) routes every import string by shape:
- `./foo` / `../foo` → file-relative under the importing file's dir
- `<dep>/<rest>` where `<dep>` ∈ manifest's `[dependencies]` → `vendor/<dep>/src/<rest>.cplus`
- bare path with declared first segment but trailing `..` → E0859 (vendor escape)
- bare path whose first segment isn't a declared dep → E0853 (single-segment) or E0852 (multi-segment, "package not declared")
- any path with a `.cplus` extension → E0858 (Phase 2 imports are extension-less)

Strict mode is gated on whether a manifest is present: `cpc build` (project mode) enforces all the rules; `cpc FILE.cplus -o BIN` (single-file mode) falls through to pre-2B file-relative behavior for backward compat. The resolver consumes `Option<&[String]>`: `None` = single-file, `Some(...)` = project (strict).

Migration: pre-Slice-2B in-tree imports (`import "math.cplus"`, `import "foo.cplus"`) rewritten to `./math`, `./foo` style across [docs/examples/projects/](docs/examples/projects/) and the test fixtures in [cpc/tests/e2e.rs](cpc/tests/e2e.rs). The did-you-mean machinery now compares filename *stems* (extension-less) so suggesting `math` for a typo of `maths` works under the new spelling.

End-to-end verified: a project with `[dependencies] utils = "*"` and `vendor/utils/src/math.cplus` exposing `pub fn add` can be imported as `import "utils/math" as math;` and called from the consumer's main. 6 new e2e tests cover the positive case, every error code (E0852/E0853/E0858/E0859), and the local-relative-still-works regression guard.


**Goal:** Teach [cplus-core/src/resolver.rs](cplus-core/src/resolver.rs) the new import grammar:
- `./<path>` / `../<path>` → file-relative; resolver appends `.cplus`.
- `<dep>/<path>` → vendor; resolver locates `<manifest-root>/vendor/<dep>/src/<path>.cplus`. First segment must match a declared dependency.

Import paths in user source code never include the `.cplus` extension.

**Resolver changes ([resolver.rs:547-555](cplus-core/src/resolver.rs#L547-L555)):**
- Reject any `imp.path` that ends in `.cplus` — fire **E0853** with a fix suggestion stripping the extension. This catches stale habits during the migration.
- Reject any `imp.path` starting with `/` (absolute) — fire E0853.
- Classify each `imp.path`:
  - Starts with `./` or `../` → **local mode**: resolve via `import_dir.join(&imp.path).with_extension("cplus")` (current behavior, with the new auto-extension).
  - Otherwise → **vendor mode**: resolve via `manifest_root.join("vendor").join(dep).join("src").join(rest).with_extension("cplus")`. First path segment must match a declared dependency in the *consumer's* manifest; reject otherwise.
- Pass `manifest_root: &Path` and `&[Dependency]` into the resolver constructor.
- Reject any `..` segment that escapes `vendor/<name>/` at compile time (security: a package can't reach files outside its own dir via static imports).
- Existing recursion / cycle-detection / diagnostic-attachment logic stays unchanged.

**Diagnostics:**
- **E0852** "import `stdlib/vec`: first segment `stdlib` is not a declared dependency in `Cplus.toml`".
- **E0853** covers three near-cases with one code: (a) bare path not matching any declared dep ("did you mean `./foo`?"); (b) leftover `.cplus` extension in the import string ("drop the `.cplus`"); (c) absolute path ("import paths must be relative or vendor-bare").

**Sema/codegen:** No downstream changes — once the path resolves, the rest of the pipeline doesn't care where it came from.

**Tests:**
- Unit: path classifier returns `Local` for `./foo`, `../foo`, `./sub/foo`; returns `Vendor` for `stdlib/vec`; rejects `foo` (bare, not declared), `/foo`, and `./foo.cplus` (stale extension).
- Unit: vendor path resolution produces `<manifest_root>/vendor/stdlib/src/vec.cplus` for `import "stdlib/vec"`.
- Unit: nested vendor path produces `<manifest_root>/vendor/stdlib/src/collections/vec.cplus` for `import "stdlib/collections/vec"`.
- E2E positive: project with `vendor/stdlib/src/vec.cplus` containing `pub fn make() -> i32 { return 0; }` builds; the calling program reads `vec::make()` correctly.
- E2E negative: undeclared package, bare-path-without-`./`, `vendor/`-escaping (`stdlib/../../etc/passwd`), and stale `.cplus` extension all fail with the right error codes.

**Exit:** All path shapes work end-to-end: local-explicit (`./foo`), local-parent (`../foo`), vendor-direct (`stdlib/vec`), vendor-nested (`stdlib/collections/vec`). Every misuse produces a structured diagnostic with a fix suggestion.

#### Slice 2C — Build driver: package-aware linking · host triple + artifacts + `[link]` propagation · ✅ done

**Shipped 2026-05-15.** `cpc build` (and `--emit-ll-project`, `cpc test`, the `[lib]` cdylib path) walk the consumer's `[dependencies]`, load each `vendor/<name>/Cplus.toml`, validate the manifest-is-truth contract, and splice each dep's `[link]` contributions onto the clang command line in declaration order. Host triple is detected once per build via `clang -print-target-triple` (in `cpc/src/main.rs` `detect_host_triple`); the dep walker (`collect_dep_link_args`) emits E0854 / E0855 / E0860 / E0861 / E0862 as structured diagnostics. Pure-source packages (no `[link]` table at all) still flow through the orphan-binary check so stray `.a`/`.dylib`/`.so` files under `vendor/<name>/src/lib/<triple>/` always raise E0861. 8 new e2e tests pin the surface (workspace: 811 lib + 11 e2e + 275 cpc tests green).


**Goal:** Teach `build_project` in [cpc/src/main.rs](cpc/src/main.rs) to walk the consumer's `[dependencies]`, open each `vendor/<name>/Cplus.toml`, validate it, and contribute the package's transitive linker requirements to the consumer's final link line.

**Build-driver changes:**
- For each declared dep:
  - Load `vendor/<name>/Cplus.toml`. Fail with **E0854** "vendor package `<name>` is missing `Cplus.toml`" if absent; fail with the existing E0406 family if malformed.
  - Validate `[package].name == <name>` — package-internal name must match its dir name. Fail with **E0855** "package `Cplus.toml` declares name `foo` but lives in `vendor/bar/`".
  - Collect `[link].frameworks` and `[link].libs` into the consumer's accumulated link args.
  - Detect host triple once at build start via `clang -print-target-triple`. Cache for the rest of the build.
  - **Bundled-binary verification (the manifest-is-truth path):**
    - If `[link].bundled` is non-empty:
      - Verify `[link].triples` is non-empty. Empty `triples` with non-empty `bundled` is **E0863** "package `<name>` declares bundled binaries but no `triples`".
      - Verify the host triple appears in `[link].triples`. If not, **E0862** "package `<name>` does not ship a build for host triple `<host>` (supports: {list})".
      - For each name in `[link].bundled`, verify `vendor/<name>/src/lib/<host-triple>/<basename>` exists. If absent, **E0860** "package `<name>` declares bundled `<basename>` but `src/lib/<host-triple>/<basename>` is not present (the package manifest says you ship it for this triple, but the file is missing)".
      - For each `.a` file present under `src/lib/<host-triple>/` that is NOT named in `[link].bundled`, **E0861** "package `<name>` ships `src/lib/<host-triple>/<basename>` but the manifest doesn't declare it; the manifest is the single source of truth (either add `<basename>` to `[link].bundled` or delete the file)".
      - Each verified bundled `.a` is added to the link line as its full path (NOT `-l<name>` — bundled artifacts aren't on the linker's search path).
    - If `[link].bundled` is absent or empty:
      - Verify NO `.a` files exist under `vendor/<name>/src/lib/<triple>/` for any triple. Orphan files there → **E0861** as above. (A source-only package with stray artifacts is a manifest bug, not a graceful-degradation case.)
- Order: consumer's own `[[bin]] frameworks`/`libs` first, then each dep's `[link]` and bundled artifacts (in `[dependencies]` declaration order).
- `cpc test` and `cpc build` use the same link path. `--emit-ll-project` already exists; it doesn't need link args (no link step) but should still surface the dep walk so E0850/E0854/E0855/E0860/E0861/E0862/E0863 fire before codegen.

**Sema/codegen:** No changes.

**Risk:** Cross-platform path handling — Windows uses `\` in `src/lib/x86_64-pc-windows-msvc/foo.lib` but the rest of the toolchain is POSIX-shaped. Phase 2 target is darwin-arm64 first, linux-x86_64 stretch; Windows lands when a Windows port is on the roadmap.

**Tests:**
- Unit: `[link]` parsing produces the right `Vec<String>` for `frameworks`, `libs`, `bundled`, and `triples`.
- Unit: `bundled` non-empty + `triples` absent → E0863.
- Unit: host-triple detector returns a non-empty string under normal conditions (mock the clang call if needed; the format is `<arch>-<vendor>-<sys>-<env>`).
- E2E (darwin-arm64): consumer with `[dependencies] appkit = "*"` and `vendor/appkit/` declaring `[link] frameworks=["Cocoa"]` builds a binary that links Cocoa — reuses the proven Phase 1 AppKit-via-`Cplus.toml` smoke test.
- E2E: a package declaring `[link] bundled=["tiny.a"] triples=["<host>"]` with the matching `src/lib/<host>/tiny.a` (generated by shelling to clang on a tiny C source file in the test harness — *not* via cpc) and one `extern fn` in `tiny_artifact.cplus` links and runs.
- E2E negative — declared but missing: `bundled=["tiny.a"]` with `triples=["<host>"]` but NO `src/lib/<host>/tiny.a` → **E0860**.
- E2E negative — present but undeclared: `vendor/foo/src/lib/<host>/orphan.a` exists, manifest's `bundled` doesn't include `orphan.a` → **E0861**.
- E2E negative — host unsupported: package declares `triples=["x86_64-unknown-linux-gnu"]` only, host is darwin-arm64 → **E0862**.
- E2E negative — bundled without triples: `bundled=["x.a"]` with no `triples` field → **E0863**.
- E2E negative — package `Cplus.toml` name/dir mismatch → **E0855**.

**Exit:** A consumer that depends on a pure-system-libs package (appkit) and a bundled-artifact package (tiny_artifact) builds, links, and runs end-to-end on the host triple. Every misuse of the bundled-binary surface (missing file, orphan file, unsupported host, missing triples list) fires a distinct E08xx with a fix suggestion that names the manifest field to edit. No filesystem auto-discovery anywhere in the path.

#### Slice 2D — Migrate existing imports + ship two smoke-test packages · ✅ done

**Shipped 2026-05-15.** Migration was completed in Slice 2B (no remaining `import "foo.cplus"` patterns in the tree). Slice 2D landed:

- [docs/examples/projects/tiny_source/](docs/examples/projects/tiny_source/) — pure-C+ vendor package. Consumer declares `[dependencies] tiny = "*"`, imports `tiny/lib`, runs `tiny::echo(42)`. Exit 42.
- [docs/examples/projects/tiny_artifact/](docs/examples/projects/tiny_artifact/) — bundled-artifact vendor package. Vendor's `[link] bundled = ["libtiny_artifact.a"] triples = [...]`. `upstream/build.sh` produces the `.a` for the host; `vendor/tiny_artifact/src/lib/` is gitignored so binaries don't drift. Consumer calls `ta::double(21)` through the FFI wrapper, exit 42.
- [docs/design/phase2-packages-mvp.md](docs/design/phase2-packages-mvp.md) — design note covering the three package modes, the import-shape rules, the manifest-is-truth contract (E0852–E0863), the resolution flow, and the forward path.
- [SKILL.md](SKILL.md) §1 — updated to describe local `./` imports vs vendor `depname/...` imports and the no-extension rule.

The location deviates from the plan's original `proves/benchmark/programs/` placement: `docs/examples/projects/` better fits canonical-reference shape next to `calc` and `hello_mods`, and avoids polluting the benchmark tree (which carries multi-language scaffolds and `tests/run.sh`). The deferred items — CI regression scan rejecting bare imports that don't match a declared dep — are not blockers and land in a Phase-2 polish PR if needed.


**Goal:** v0.0.1's small set of cross-file imports (`docs/examples/projects/*`, `proves/benchmark/programs/*`) currently use bare `foo.cplus` paths that mean file-relative — and carry the now-illegal `.cplus` extension. Under the new rules (Slice 2B), every existing import needs both `./` and the extension stripped. This slice handles the migration and lands the canonical "how do packages work" smoke tests.

**Migration:**
- Scan: `grep -rn 'import "' docs/examples/ proves/benchmark/` and any other in-tree `.cplus` files.
  - `import "foo.cplus"` → `import "./foo"` (add `./`, drop `.cplus`).
  - `import "./foo.cplus"` → `import "./foo"` (drop `.cplus`).
- Mechanical but committed by hand for review.
- Update [SKILL.md](proves/SKILL.md) §1 and any cross-references in `docs/design/` notes to show the new spelling.

**Two smoke-test vendor packages** under `proves/benchmark/programs/`:

1. **`tiny-source`** (pure C+ source mode):
   - `vendor/tiny/Cplus.toml` declaring `[package] name = "tiny"`.
   - `vendor/tiny/src/lib.cplus` containing one `pub fn echo(n: i32) -> i32 { return n; }`.
   - Consumer's manifest declares `[dependencies] tiny = "*"`.
   - Consumer's `src/main.cplus` does `import "tiny/lib" as tiny; ... tiny::echo(42)`.
   - Test harness verifies the binary prints `42`.

2. **`tiny-artifact`** (bundled-artifact mode):
   - `vendor/tiny_artifact/Cplus.toml` declaring `[package] name = "tiny_artifact"` (no `[link]` needed — the `.a` self-contains).
   - `vendor/tiny_artifact/src/api.cplus` containing `extern fn ta_double(n: i32) -> i32;` + `pub fn double(n: i32) -> i32 { return unsafe { ta_double(n) }; }`.
   - `vendor/tiny_artifact/upstream/tiny_artifact.c` — the C source the package *author* used to produce the artifact. Not built by cpc; checked into the smoke test for transparency (lives outside `src/` to make absolutely clear it isn't part of the C+ source the consumer compiles).
   - `vendor/tiny_artifact/src/lib/<host-triple>/tiny_artifact.a` — the prebuilt artifact. The smoke test's CI hook regenerates this via `clang` on the host the first time the test runs, then caches it.
   - Consumer's `src/main.cplus` does `import "tiny_artifact/api" as ta; ... ta::double(21)` → 42.
   - Test harness verifies the binary prints `42`.

These become the canonical references for future package work — pure-source for stdlib (Phase 3), bundled-artifact for bindgen output (Phase 4).

**Documentation:**
- Land `docs/design/phase2-packages-mvp.md` capturing: why MVP, locked decisions, package modes table, schema, the path forward to `cpc fetch`.

**Tests:**
- E2E: all existing programs in `docs/examples/projects/*` and `proves/benchmark/programs/*` still build after migration.
- E2E: both smoke-test packages build and run.
- CI: regression test scans in-tree `.cplus` files and rejects any bare import not matching a declared dependency. Catches future drift.

**Exit:** Zero broken imports in the tree; both smoke-test programs run green; design note exists.

### Phase 2 exit criteria

- All four slices shipped with full test coverage per [feedback_test_discipline.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_test_discipline.md).
- A user can: declare `[dependencies] stdlib = "*"` in their manifest, drop a stdlib repo into `vendor/stdlib/`, write `import "stdlib/vec" as vec;` (no `.cplus` extension), and build successfully.
- A user can: depend on a package that ships a prebuilt `.a` under `src/lib/<host-triple>/`; the binary links and runs.
- A user can: depend on a package that declares `[link].frameworks=["Cocoa"]`; the `-framework Cocoa` flag flows through without per-`[[bin]]` duplication.
- The compiler refuses every documented misuse with a structured diagnostic plus actionable suggestion (E0850–E0856 at minimum).
- Both smoke-test packages in `proves/` prove the workflow end-to-end.
- Design note `docs/design/phase2-packages-mvp.md` exists.

### Phase 2 non-goals

- `cpc fetch`. Forward-compatible but out of scope. Users populate `vendor/` themselves.
- Lockfile. The manifest plus the contents of `vendor/` together *are* the lockfile; integrity is whatever git gives you.
- **Transitive C+ dependencies.** A vendor package's own `[dependencies]` is ignored by the compiler. (`[link]` is separate — it propagates system linker hookups, not C+ source deps.)
  - *AI-First Design Note:* This is an intentional architectural choice. Instead of building complex SAT solving and dependency resolution into `cpc` (like `cargo` or `npm`), the compiler stays fast, simple, and deterministic. We offload the dependency resolution and flattening to the AI agent. When adding a package, the AI reads its `Cplus.toml`, recursively fetches its dependencies, and flattens them into the consumer's top-level `Cplus.toml` and `vendor/` directory. The compiler expects a flat manifest; flattening is the AI's job.
- Version resolution / SemVer. The version string is parsed and discarded.
- Dynamic-loaded artifacts (`.so` / `.dylib` / `.dll`). Phase 2 ships static (`.a`) only — link-time, no runtime loader dance. Dynamic lands when a real use case shows up.
- `cpc` invoking a C / C++ / ObjC compiler. **Never.** Packages ship prebuilt artifacts or system-lib declarations; C source compilation is upstream of cpc by design.
- Cross-compilation. Slice 2C detects the *host* triple. Cross-compile is a separate milestone.
- Sandbox, capabilities, API-surface enforcement, signing — all the pm.md goodies. Each lands when actual demand for it shows up.
- Multi-package repos (pm.md §9). Not on the MVP path; subdirectory packages can be re-derived later if needed.

### Phase 2 estimated effort

**2–3 weeks.** Bigger than the original 1–2 because of Slice 2C's link-driver work (host-triple detection, per-package manifest loading, artifact path collection). The expanded scope is the difference between "a stdlib can technically live here" and "a real-world FFI package — including artifact-backed ones like a hypothetical `curl_bindings` — works end-to-end."

### Phase 3 — Language completeness + reference library + stdlib bootstrap · est. 3–4 weeks

Three slices ordered cheapest-first. The `proves/` analysis shows the agent-cost gap is **three separable problems**: missing language features the compiler intentionally rejects (cheap), missing recipes (medium), missing stdlib (open-ended). Tackle in that order so the cheapest fixes ship first and de-risk the bigger ones.

#### Slice 3A — Language completeness gaps · est. 3–5 days · ✅ done (compound-assigns deferred)

**Goal:** Close the specific gaps surfaced by 04-curl-lite where the compiler already knows the feature is missing.

**Bitwise and shift operators on integer types.** Today the compiler emits `E0312: bitwise and shift operators are not yet supported` for `<<`, `>>`, `&`, `|`, `^`, `~`. Implement them:
- Lexer: tokens `<<`, `>>`, `&`, `|`, `^`, `~`, plus compound-assign `<<=` `>>=` `&=` `|=` `^=`.
- Parser: precedence matches C (shift > comparison; bitand > bitxor > bitor).
- Sema: defined on every integer type; reject on `bool`, floats, pointers (E0302). Shift count is `u32` or coerced from an integer literal; runtime check in debug for `shift >= bitwidth` (trap), wrap in release (matches §2.3 arithmetic semantics).
- Codegen: `shl` / `lshr` / `ashr` / `and` / `or` / `xor` / `xor -1` LLVM ops. Trivial.

**Byte-swap intrinsics.** Add compiler intrinsics `bswap16(x: u16) -> u16`, `bswap32(x: u32) -> u32`, `bswap64(x: u64) -> u64`, plus aliases `htons` / `htonl` / `ntohs` / `ntohl` that lower to `bswap` on little-endian targets and to identity on big-endian. Codegen via `llvm.bswap.i{16,32,64}`. The 04-curl-lite agent needed exactly this — building a 16-bit port in network order is the canonical use case.

**SKILL.md doc gaps from the proves analysis:**
- Document that raw-pointer arithmetic uses `+` (plain), **not** `+%` (wrapping). Add a §6.2 callout. The error code is structured (E0302) but the agent shouldn't have to learn this by failing.
- Add a "common errors" callout in §8 for the now-implemented bitshifts and the byte-swap intrinsics.
- Document the `--version` / `-V` flag in the CLI section (shipped 2026-05-14 but undocumented in SKILL.md).

**Tests:**
- Unit: every new operator with positive cases on `i8`/`i16`/`i32`/`i64`/`u8`/`u16`/`u32`/`u64`; negative cases on `bool`, `f32`, `f64`, `*T`.
- Unit: shift-count overflow traps in debug, wraps in release.
- Unit: byte-swap intrinsics on every supported width, plus htons round-trip identity.
- E2E: a `.cplus` program building a network-order port via `(port >> 8) as u8` + `(port & 0xff) as u8` compiles and runs.
- E2E: a program using `htons(8080u16)` produces the byte-swapped value at runtime.

**Exit:** Bitshifts work end-to-end; byte-swap intrinsics work; SKILL.md no longer has the `+`/`+%` pointer-arithmetic gap. A re-run of 04-curl-lite should not hit any E0312 / E0302 errors.

#### Slice 3B — Reference program library · est. 1 week · ✅ shipped 2026-05-15

**Shipped:** ten task-oriented recipes under [docs/examples/recipes/](docs/examples/recipes/), each its own `cpc build` project. Lengths fall in the 30–230 line band the AppKit reference established as the right scale. Each ships with a one-line `// purpose / libc symbols` header.

| Recipe | Demonstrates |
|--------|--------------|
| [argv_parse](docs/examples/recipes/argv_parse/) | `_NSGetArgc` / `_NSGetArgv` macOS path; iterating `**u8` |
| [env_var](docs/examples/recipes/env_var/) | `getenv` + null-pointer check (`p == (0 as *u8)`) |
| [stdin_lines](docs/examples/recipes/stdin_lines/) | growing heap buffer + scan-and-emit line splitter |
| [file_read](docs/examples/recipes/file_read/) | variadic `extern fn open(...)`, `read` → growing buffer |
| [file_write](docs/examples/recipes/file_write/) | `open` with `O_CREAT \| O_WRONLY \| O_TRUNC`, mode 0o644 |
| [hash_table](docs/examples/recipes/hash_table/) | reinterpret cast `*u8 → **u8` / `*usize` for typed pointer arrays; FNV-1a + linear probing |
| [tcp_client](docs/examples/recipes/tcp_client/) | `socket` / `connect` / `shutdown(SHUT_WR)`, hand-built `sockaddr_in` |
| [tcp_server](docs/examples/recipes/tcp_server/) | `bind` / `listen` / `accept` + echo loop, `SO_REUSEADDR` |
| [json_parse](docs/examples/recipes/json_parse/) | recursive-descent parser, SAX-style output, `Parser` struct with err flag |
| [http_get](docs/examples/recipes/http_get/) | DNS via `gethostbyname` (`hostent` struct walk), URL parsing, HTTP/1.0 |

**CI smoke tests:** 11 new e2e tests at [cpc/tests/e2e.rs](cpc/tests/e2e.rs) (`recipe_*`) exercise each recipe end-to-end — argv passing, stdin piping, file round-trip, TCP round-trip with both client + server in the same harness, JSON success + malformed-input rejection. tcp_server round-trip completes in ~1s because `tcp_client` half-closes (`shutdown(SHUT_WR)`) instead of relying on process-exit teardown.

**One compiler bug fixed in flight:** `StmtKind::Loop` was missed by `collect_and_emit_str_lits` in [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs), so any `str` literal inside a `loop { ... }` body tripped a codegen `expect`. Walks the body now; regression test `str_literal_inside_loop_block_collected`.

**SKILL.md:** §10 ("When in doubt") now leads with "read a recipe" — bumping the existing `docs/examples/` pointer to second-priority. The recipes generalize the AppKit data point: a near-complete reference is more useful than prose.


**Goal:** Ship canonical `.cplus` reference programs for the next anticipated benchmarks. The hello-appkit data point (15 turns, competitive with Swift/Rust) proves that **a near-complete reference is worth more than a paragraph of prose**.

**Target programs** (one `.cplus` file each, working end-to-end, under `docs/examples/recipes/`):
- `tcp_client.cplus` — connects to `host:port`, sends bytes, reads response, prints. Covers the 04-curl-lite recipe gap directly.
- `tcp_server.cplus` — bind, listen, accept-loop, echo. Pair with `tcp_client.cplus`.
- `file_read.cplus` — open, read-to-end, print bytes.
- `file_write.cplus` — open(create), write bytes, close.
- `stdin_lines.cplus` — read stdin line-by-line until EOF, echo to stdout.
- `argv_parse.cplus` — read `argc`/`argv`, print each arg. (Resolves the recurring "how do I get command-line args" question.)
- `env_var.cplus` — read an env variable via `getenv`, print or fall back.
- `hash_table.cplus` — minimal `HashMap[K, V]` with insert/lookup/delete. Reference for stdlib Slice 3C.
- `json_parse.cplus` — minimal JSON parser (object/array/string/number/bool/null). Common-enough recipe.
- `http_get.cplus` — full HTTP GET request + response parsing. Builds on `tcp_client.cplus` + `json_parse.cplus`.

Each program:
- Compiles clean with `cpc build` from a tiny `Cplus.toml` co-located in its directory.
- Has a one-line comment at the top stating what it does and which libc symbols it uses.
- Is *short* — the appkit reference was 245 lines and worked. Aim for that scale, not "production-quality library".
- Lives under `docs/examples/recipes/` (new directory). The existing `docs/examples/` stays as the language-feature showcase; `recipes/` is task-oriented.

**Tests:** Each program runs in CI with a shell-level smoke test (start, exit cleanly, expected stdout). Failures block the slice — references must work or they harm rather than help.

**Exit:** Ten reference programs ship; CI smoke-tests all of them; SKILL.md §10 ("When in doubt") gets a new top entry pointing to `docs/examples/recipes/`.

#### Slice 3C — Stdlib bootstrap (vendor package) · est. 1.5–2 weeks

**Goal:** Ship `vendor/stdlib/` as a real C+ package consumable via Phase 2's resolution. **MVP, not "real stdlib"** — the discipline question is *how thin* (see Phase 3 non-goals).

**Package consolidation (2026-05-15):** the earlier `vendor/stdlib/` (source-only skeleton) and `vendor/stdlib_bin/` (binary-distribution skeleton) have been merged into one unified `vendor/stdlib/`. The unified package supports three distribution modes — source-only (today), binary-only (future), and mixed — selectable by what the author commits, not by manifest schema. Per-arch binary slots live at `src/lib/<host-triple>/` (currently empty placeholders for `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`). The merge keeps the bootstrap path simple: Phase 3C fills in source bodies; binary releases come later by populating `src/lib/<triple>/` with prebuilt `.a` files. See [vendor/stdlib/README.md](vendor/stdlib/README.md) for the distribution-modes table and symbol-naming convention.

**Module list** (each is one `.cplus` file under the package's `src/`). The API-only skeleton is already in tree at [vendor/stdlib/](vendor/stdlib/) — Phase 3C fills in the bodies.

| Module file | Import path | API surface |
|---|---|---|
| `src/result.cplus` | `stdlib/result` | `enum Result[T, E] { Ok(T), Err(E) }`, `enum IoError { ... }` |
| `src/io.cplus` | `stdlib/io` | `print(s: str)`, `println(s: str)`, `read_stdin_line() -> Result[string, IoError]`, `eprintln(s: str)` |
| `src/fs.cplus` | `stdlib/fs` | `struct File`, `File::open(path: str) -> Result[File, IoError]`, `File::read_to_end(mut self) -> Result[Vec[u8], IoError]`, `File::write_all(mut self, data: Vec[u8]) -> Result[(), IoError]`, `File::close(move self)` (Drop also closes) |
| `src/net.cplus` | `stdlib/net` | `struct TcpStream`, `TcpStream::connect(host: str, port: u16) -> Result[TcpStream, IoError]`, `read`/`write`/`close` mirroring `File`, plus `TcpListener` for servers |
| `src/vec.cplus` | `stdlib/vec` | `Vec[T]` polish — exists already at user level ([docs/examples/phase11_vec_allocator.cplus](docs/examples/phase11_vec_allocator.cplus)). Promote to a stable surface; document the API. |
| `src/hash_map.cplus` | `stdlib/hash_map` | `HashMap[K, V]` with `insert` / `get` / `remove` / `len`. Open addressing, linear probing. Derived from the Slice 3B `hash_table.cplus` reference. |
| `src/env.cplus` | `stdlib/env` | `var(name: str) -> Option[string]`, `args() -> Vec[string]` |

**Implementation notes:**
- All FFI happens *inside* stdlib — user code never sees `extern fn socket(...)`.
- Each module is independently importable: `import "stdlib/net" as net;` pulls in `net` only, not all of stdlib. Dead-code elim at `-O2` strips unused functions per Phase 2's "only needed functions" property.
- `IoError` is one tagged-union shared across `io`/`fs`/`net` so `Result` chains compose.
- Drop integration: `File`, `TcpStream`, `Vec[T]`, `HashMap[K, V]` all close/free on scope exit via the existing Drop infrastructure (Slice 3F from the archived plan).
- The stdlib repo lives at `github.com/<owner>/cplus-stdlib` (or chosen URL). For development, [vendor/stdlib/](vendor/stdlib/) at the cpc repo root holds the in-tree skeleton; consumer projects symlink or submodule the standalone repo into their own `vendor/stdlib/`.

**Tests:**
- Each module ships unit tests via `cpc test` against a small in-tree consumer project.
- A new `proves/benchmark/programs/05-curl-lite-stdlib/` re-runs 04-curl-lite's spec but with the stdlib available — measure the turn/cost delta to validate that closing the gap actually works. **This is the empirical exit criterion.**
- Cross-platform: macOS/arm64 is the primary target (matches `proves/stats.md` methodology); Linux/x86_64 is a stretch goal. Document any gaps.

**Exit:**
- Slices 3A and 3B already shipped.
- `vendor/stdlib/` package installs and builds via Phase 2 resolution.
- A re-run of 04-curl-lite with stdlib available drops cost to within 2× of the Rust baseline (loose target: < 20 turns, < $0.50). If it doesn't, we learned something — write up the gap before moving on.

### Phase 3 non-goals

- **A "real" stdlib.** This is MVP, attacking measured gaps. No `Future` / async, no `BTreeMap`, no `Regex`, no `serde`-equivalent, no thread API, no atomic-rich concurrency primitives. Each of those is its own future package.
- **Cross-platform parity beyond macOS/arm64.** Stretch goal, not exit criterion.
- **A package registry, package discovery, or curated package list.** Phase 2 is intentionally registryless. Discoverability is someone else's problem until it isn't.
- **Operator overloading for stdlib types.** Per §2.6, C+ has no operator overloading. `Vec[T]::push(self, ...)` not `vec += ...`.

### Phase 4 — `cpc-bindgen` · TBD pending Phase 3 lessons

Libclang-based header-consumption tool that emits `.cplus` files containing `extern fn` declarations (committed to the consumer's repo). Same shape as Dart `ffigen`, Rust `bindgen`, Swift's clang importer-as-tool. Originally noted as "stretch / built when hand-writing bindings becomes painful" in archived Phase 10.

**Concrete motivation from `proves/`:** 04-curl-lite's C+ implementation contains a ~200-line block of hand-written `extern fn` declarations for `socket`, `connect`, `read`, `write`, `close`, `inet_addr`, `htons`, `malloc`, `realloc`, `free`, `memcpy`, `memset`, and friends. Rust used one line: `use std::net::TcpStream;`. Phase 3C closes this for stdlib-shaped functionality, but every *user* who wants to consume a C library (zlib, SQLite, OpenSSL, ImageMagick, …) hits the same wall. cpc-bindgen attacks the wall directly.

**Deferred to Phase 4 design time:** scope and slice breakdown. Wait until Phase 3C is shipped because writing the stdlib by hand will teach us what bindgen actually needs to handle (struct ABI corner cases, function-pointer-typed fields, varargs, enum-of-int mappings, opaque pointer types, attribute-conditional declarations on different platforms). Design note drafts after Phase 3C lands.

**Technical risk: low.** rust-bindgen is the well-trodden playbook. The 80% that's purely porting: libclang AST walk → emit `extern fn` declarations, C scalars → C+ scalars (`int32_t` → `i32`, `size_t` → `usize`, `char*` → `*u8`), C structs → `#[repr(C)] struct`, function-pointer types → `fn(T) -> R` (slice 11.FN_PTR), symbol aliasing → `#[link_name]` (slice 11.LINKNAME), varargs already supported on `extern fn`, simple-constant `#define` → emitted constants, function-like macros skipped with a warning. Mechanical.

**Two open questions to answer when we get there** (not blockers, just real design calls):

1. **C unions.** C+ has tagged unions (`enum` sum types) but no untagged `union` like C/Rust. Two viable answers when bindgen hits a header containing `union { int i; float f; }`:
   - **(a) Add `union` to the language.** Small aggregate feature — overlapping field offsets, `size_of` = max-of-fields, all field reads `unsafe` (no live-variant tracking). Roughly a one-week language slice on its own.
   - **(b) Generate byte-array shims** — `struct U { _bytes: [u8; N] }` plus reinterpret-cast accessors. Zero language change, uglier output, same semantics.
   - Decide at Phase 4 design time. (a) is cleaner long-term; (b) lets bindgen ship without a prerequisite.

2. **C bitfields.** `struct flags { unsigned a : 3; unsigned b : 5; }` requires generated mask-and-shift accessors. **Phase 3A's bitshift operators are a hard prerequisite** — bindgen cannot emit working bitfield accessors without `<<` / `>>` / `&` / `|`. This is part of why the plan ordering is 3 → 4.

**Other items to revisit at design time, all with known idioms:** opaque types (`typedef struct foo_t foo_t;` with no body) — generate `struct Foo {}` + handles as `*Foo`; null pointers in C signatures — raw `*T` is already nullable, users write `unsafe { 0 as *T }`; ObjC interop — out of scope, C headers only; Windows calling conventions (`__stdcall` etc.) — punt until Windows is a tier-1 target.

This section is a snapshot, not a spec — plan.md is evolving as questions get answered. Slice these out when Phase 4 actually starts.

### Phase 5 — C ABI export: build a library C can link against · 6 slices · ✅ shipped 2026-05-15

**Motivation:** Phase 4 (`cpc-bindgen`) covers the C-→-C+ direction (consume C headers). Phase 5 covers C+→-C: emit `.a` / `.dylib` / `.so` artifacts that a C, C++, Swift, Python-cffi, Lua-FFI, Java-JNA, or Ruby-FFI consumer can link against. Today C+ produces only executables — a hand-test from the recent Phase 1 closeout (`cpc --emit-ll | clang -c | ld`) showed scalar fns work but value-passed aggregates corrupt across the boundary because cpc emits LLVM "first-class aggregate" parameters, not the platform C ABI's register-coerced equivalents.

**Locked decisions:**
- **Library target in manifest.** New `[lib]` section in `Cplus.toml` with `name`, `crate-type` ∈ `{ "staticlib", "cdylib", "both" }`. Only one library per crate (matches Cargo). Bin and lib are mutually exclusive in v0.0.2 (`cpc build` errors if both are declared). The plain `cpc build` route handles linking — users don't have to call `ar` / `ld` by hand for the common case.
- **Surface syntax for "this function uses the C ABI":** `pub extern fn NAME(...) -> T { body }`. The existing parser rejects `extern fn` with a body (`extern_fn_with_body_rejected` test). Lifting that restriction *only* when `pub` precedes `extern` gives a clean syntactic split: `extern fn name(...);` is import (current), `pub extern fn name(...) { ... }` is export. The `extern` keyword already means "C ABI" in C+ — no `extern "C"` string needed, since C is the only ABI.
- **Non-exportable types at the boundary.** `string`, `str`, slice (`T[]`), tagged enums, and any non-`#[repr(C)]` struct are rejected by sema in `pub extern fn` signatures. Drop types are rejected too (no destructor runs in C, so cross-boundary ownership is undefined). The user writes `(*u8, usize)` instead of `str` and converts at the boundary manually. Same conservatism as Rust's `extern "C"` since C+'s no-null principle means there's no ambiguity left to inherit from C.
- **Targets in v1.** macOS arm64 + Linux x86-64. Windows / aarch64-Linux deferred (windows-x86 ABI needs `inalloca` which Slice 1H Tier-3 already rejected; aarch64-Linux differs from aarch64-darwin in HFA / vararg edge cases).
- **`pub` matters at codegen.** Non-`pub` items get `internal` linkage in IR so `-O2`'s LTO + `-fvisibility=hidden` can strip unused implementation details from the shipped `.dylib`. Today every `define` is external — fine for executables, leaky for libraries.

**Slice 5.A — Library target + object-file emission · MVP unblocker · ✅ done**

**Shipped:** `[lib]` section in `Cplus.toml` (`name`, `path`, `crate-type` ∈ `{staticlib, cdylib, both}`, `frameworks`, `libs`). `cpc --emit-obj FILE -o OUT.o` produces a relocatable object. `cpc build` on a `[lib]` manifest emits `target/<mode>/lib<name>.a` and/or `lib<name>.{dylib,so}`. Sema-level gates: E0408 (bin+lib together), E0409 (fn main in lib), E0412 (unknown crate-type). Resolver: top-level items in a lib's entry file skip path-mangling so `pub fn add` exports as bare `_add` for C consumers. A full round-trip test (build C+ lib → link from C → run) passes both staticlib and dylib paths on macos-arm64.

The smallest delta that lets a user produce a `.a` / `.dylib` from a C+ source tree, before any ABI coercion lands. Most users will write `pub extern fn` signatures that already work (scalars + pointer-passed structs) and avoid the broken value-passed-aggregate case.

**Manifest changes ([cplus-core/src/manifest.rs](cplus-core/src/manifest.rs)):**
- Add `RawLib { name: Option<String>, crate_type: Option<String> }` plus `Manifest::lib: Option<LibTarget>` where `LibTarget { name, crate_type, path }`. `path` defaults to `src/lib.cplus`, mirroring `bin.path = src/main.cplus`.
- Validate `crate_type` ∈ {"staticlib", "cdylib", "both"}; default to "staticlib".
- Reject mutual presence of `[[bin]]` and `[lib]` in one manifest with E0408 ("a manifest declares either a binary or a library, not both").

**Driver changes ([cpc/src/main.rs](cpc/src/main.rs)):**
- Add `cpc --emit-obj FILE -o out.o`. Pipes IR through `clang -c <opt> input.ll -o out.o`. Reuses `run_clang_to_stdout`'s plumbing.
- `cpc build` consults `manifest.lib`. If present:
  - Skip the test-driver `@main` injection that's normally in `generate_inner`. (Actually: today, `generate` always emits the user's `fn main`; for libs we need the user to *not declare* `fn main` and codegen to not inject one. Add `BuildMode::Library` enum variant *or* a separate `generate_lib` path.)
  - Emit IR → `.o` → archive into `lib<name>.a` via `ar rcs target/<mode>/lib<name>.a out.o` for staticlib.
  - Or invoke `clang -shared out.o -o target/<mode>/lib<name>.dylib` (`.so` on Linux) for cdylib.
  - For `crate_type = "both"`, emit both.
- Sema gate: a `[lib]` manifest with `fn main` defined emits E0409 ("library targets must not define `fn main`"). Conversely a `[[bin]]` with `pub extern fn` exports is allowed but the exports won't be linker-visible (executables don't expose symbols).

**Sema/codegen changes:**
- Add a build-mode bit threading "this is a library" through `generate_inner` so the `@main` injection skips.
- Otherwise unchanged for this slice — value-passed aggregate bugs remain user-visible, fixed in 5.D.

**Tests:**
- Unit (manifest): parse a `[lib]` section; reject `[[bin]]` + `[lib]` together; default `crate_type` to staticlib.
- E2E: build a tiny C+ library exposing `pub fn add(a: i32, b: i32) -> i32` (no `extern` yet — scalar fn, already C-callable as side effect). Produce `lib<name>.a` and link a C consumer. Verify the runtime answer.
- E2E (negative): a `[lib]` manifest declaring `fn main { return 0; }` emits E0409.
- Cross-platform: gate the `.dylib` test under `#[cfg(target_os = "macos")]`, `.so` test under `#[cfg(target_os = "linux")]`.

**Exit:** `cpc build` produces `target/<mode>/lib<name>.{a,dylib,so}` per `crate_type`. A C consumer can `#include` a hand-written header, link against the artifact, and call any scalar or pointer-passed-struct function correctly. Value-passed aggregates remain broken (fixed in 5.D).

**Slice 5.B — `pub` → external linkage; non-`pub` → internal linkage · ✅ done (lib-mode-only)**

**Shipped:** in `[lib]` builds, non-`pub` fns / methods emit with `internal` linkage; `pub` items and `main` keep external. `drop` methods are always internal (compiler-synthesized, never part of the public C-ABI surface). Executable builds keep external linkage on every item to avoid breaking the substring-pinned test suite. Verified end-to-end: `nm -g liblinkage.a` exposes only `pub_api`, never the private `helper` — and `-O2 --release` lets LTO fold `helper` away entirely. Tests: `lib_target_non_pub_fns_get_internal_linkage`, `lib_target_non_pub_methods_get_internal_linkage`, `exec_target_linkage_unchanged_by_5b`.

**Carryover:** rolling the same rule into executable builds would let LTO strip dead helpers there too, but flips ~34 substring-pinned tests that pin `define <ty> @<name>(` patterns. Cheap to do as a separate slice — update the assertions to a more lenient pattern (e.g. `.contains("@<name>(")`).

Pure codegen change. Today every `define` is external-linkage; LTO can't strip helpers. After: only `pub` items expose external symbols.

**Codegen changes ([cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)):**
- In `gen_function` and `gen_method`, prepend `internal ` to the `define` line iff `!is_pub`.
- Caveat: a non-`pub` fn called from another file in the same project must stay externally linkable across compilation units. Today cpc builds the whole project in one IR module (single-`.ll`-file build), so this is fine. Once incremental builds land (post-v0.0.2), revisit — internal linkage is module-local, and Rust handles this via the `priv` / `pub(crate)` distinction.
- Methods: `Type.method` is non-pub if the impl block / type are non-pub. Apply the same rule.

**Tests:**
- Codegen snapshot: a non-pub fn emits `define internal ...`.
- E2E: at `-O2`, an unused non-pub fn is stripped from the final `.dylib`. Verify via `nm -gj lib<name>.dylib | grep <fnname>` returning empty.

**Exit:** `nm -gj` on a built `.dylib` shows only `pub` symbols. Size shrinks on a sample lib.

**Slice 5.C — `pub extern fn` with body = C-ABI export · ✅ done**

**Shipped:** parser now accepts `pub extern fn NAME(...) -> T { body }` as a C-callable export (definition with C ABI). The plain `extern fn name(...);` decl-form keeps its current shape and rejects `pub` (likely-forgotten-body case). Variadic + body is rejected (C+ has no `va_list` API; varargs is import-only). Sema runs a C-exportable predicate over every signature type and emits **E0410** for non-C types: `string`, `str`, slice `T[]`, tagged enum, non-`#[repr(C)]` struct, struct with `Drop`, generic `Ty::Param`, fn-ptr containing any of those, struct field containing any of those. Each diagnostic includes the conventional workaround in the message (e.g. "pass `*u8` + `usize` length instead"). Codegen routes `pub extern fn` definitions to the normal `define` path (not `declare`). 17 sema unit tests + 2 e2e (round-trip through C, str rejection).

**Carryover for 5.D:** value-passed aggregate C ABI coercion. Today `pub extern fn square(p: Point) -> i32` where `Point` is a 2×i32 `#[repr(C)]` struct emits `define i32 @square(%Point %0)` — LLVM "first-class aggregate" passing, which does NOT match the platform C ABI on aarch64 (where 8-byte structs go in a single GPR). 5.D fixes that.

Add the surface syntax and reject non-C-exportable signatures at sema time. Codegen still uses today's LLVM ABI for the body — Slice 5.D fixes the aggregate-coercion issue.

**Parser changes ([cplus-core/src/parser.rs](cplus-core/src/parser.rs)):**
- When `pub extern fn` is seen, parse a body block instead of demanding `;`. Drop the current "extern fns have no body" rejection for the `pub extern` case.
- AST: `Function` already has `is_extern` and `is_pub` flags — no struct change needed. Codegen routes off both flags.

**Sema changes ([cplus-core/src/sema.rs](cplus-core/src/sema.rs)):**
- A function with `is_pub && is_extern && !body.is_empty()` is an "export". For each param + return type, run a "C-exportable" predicate:
  - Allowed: every primitive, raw `*T`, fn-ptr (with C-exportable params and return), `#[repr(C)]` non-Drop struct of allowed-field types, `Ty::Unit` (return only), plain (untagged) enum (lowers to `i32`).
  - Rejected: `string`, `str`, `Slice(_)`, tagged enum, any struct that's non-`#[repr(C)]` or has Drop, anything with `Ty::Param` (generic exports rejected for v1).
- New errors: E0410 "type `<T>` is not C-ABI compatible; cannot appear in a `pub extern fn` signature" with a suggestion (`use *u8 and a paired length` for `str`; `#[repr(C)]` for naked structs; etc.).

**Codegen changes:** none for 5.C — value-passed aggregates still use the LLVM IR-level "first-class aggregate" ABI. Documented limitation in 5.C's exit notes, fixed in 5.D.

**Tests:**
- Unit (sema): each rejected type produces E0410 in a `pub extern fn` signature.
- Unit: scalar / `*T` / fn-ptr / `#[repr(C)]` struct compile clean.
- E2E: a `pub extern fn add(a: i32, b: i32) -> i32 { return a + b; }` in a library, called from C, returns the right answer.

**Exit:** Library authors can declare exports with `pub extern fn`. Scalar / pointer-arg exports work end-to-end. The non-C-exportable types are caught at sema with actionable diagnostics.

**Slice 5.D — Target C ABI coercion for value-passed aggregates · the technically meaty slice · ✅ done (aarch64-darwin)**

**Shipped on aarch64-apple-darwin** (the primary target):
- `classify_c_abi(ty, types) -> CAbiClass` predicate: scalars → `Direct`; aggregate ≤8 bytes → `Coerce("i64", 8, 8)`; 9..=16 bytes → `Coerce("[2 x i64]", 16, 8)`; >16 bytes → `Indirect`.
- Param side: at `pub extern fn` definitions, `Coerce` rewrites the LLVM param type and allocates a slot sized for the coerced type so the wide store doesn't overflow. `Indirect` binds directly to the SSA pointer the C caller provided (no `byval` on aarch64-darwin).
- Return side: `Coerce` stages the value through an alloca + reloads as the coerced LLVM type before `ret`. `Indirect` reuses Slice 1D's `sret` path (generalized from `Ty::String` only to any indirect-class return).
- Body unchanged: gen_field GEPs use the original struct type for offsets; opaque pointers make the alloca's wider footprint invisible to subsequent loads/stores.

**Verified end-to-end:** the canonical `square(Point{3,4})` case that returned garbage in the Phase-1-closeout hand-test now returns 25 from a C consumer. 5 round-trip e2e tests cover param/return for 8/16/24-byte structs.

**Carryover:**
- x86_64-sysv: same shape (≤8 → i64, 9..16 → {i64, i64}, >16 → byval). Need to flip the `[2 x i64]` to `{i64, i64}` and add `byval(<ty>) align <A>` on indirect args. ~1-day slice.
- HFA (homogeneous float aggregates): `struct { float x, y; }` should pass in 2 FP registers per aarch64 PCS but currently coerces to `i64` (integer class). Correct but suboptimal for SIMD-heavy code. Defer to v2.
- C+ calls to its own `pub extern fn`: the call-site doesn't coerce (only the define-site does), so an internal call to a `pub extern fn` taking an aggregate would mismatch. Library authors should call a private helper internally and let the `pub extern fn` be a thin C-ABI wrapper. Documented limitation.

This is the slice that closes the actual ABI gap. Implements a minimal subset of the platform C ABI for the cases users will hit.

**Scope, by platform:**
- **aarch64-apple-darwin (primary).** AArch64 Procedure Call Standard. For a struct of size `S`:
  - `S ≤ 16`: passed in up to two general-purpose registers, coerced to `[2 x i64]` or `i64` in IR. Returned the same way (no sret).
  - `S > 16`: passed by hidden pointer (caller allocates, callee reads); returned via sret (caller-provided pointer).
  - HFA (homogeneous floating aggregate, all f32 or all f64): use up to 4 floating registers — defer to v2 (skip in v1; treat as integer-class coercion, slightly slower but ABI-correct).
- **x86_64-unknown-linux-gnu / x86_64-apple-darwin.** SysV AMD64 ABI:
  - `S ≤ 8` and all fields fit in INTEGER class: coerce to `i64`.
  - `8 < S ≤ 16` and all fields integer-class: coerce to `{i64, i64}`.
  - `S > 16`: pass by hidden pointer; return via sret.
  - Floats-in-classification (SSE class): defer to v2.
- **Other targets:** error at codegen ("target X not supported for C ABI export; consider building on macos-arm64 or linux-x86_64"). Add new error E0411.

**Codegen changes:**
- Add `target_c_abi.rs` (new module) with `fn classify_arg(ty: &Ty, types: &TypeTable) -> AbiClass` and matching `classify_return`. Returns one of:
  - `Direct` (pass as-is, current behavior — covers scalars).
  - `Coerce(LlvmType)` (pass as the given LLVM type, then bitcast inside the callee).
  - `Indirect` (pass by hidden pointer with `byval` attr; return via `sret`).
- For each `pub extern fn` definition: rewrite the function signature using the classification + emit prologue stubs that bitcast the coerced LLVM-level params back to the C+ struct type.
- For each call site that's CALLING a `pub extern fn`: would also need coercion. But since `pub extern fn` is for exports, internal call sites use the regular C+ ABI. **Decision: internal callers of a `pub extern fn` use the rewritten C ABI signature, not the C+ one.** Cleanest, single ABI per function.
- Detect target triple via `LLVM_DEFAULT_TRIPLE` env or codegen call signatures matching `arm64-apple-*` / `x86_64-*-linux*` / `x86_64-apple-*`. Cache the choice on the `BuildMode` extension.

**Tests:**
- Unit: classification for every shape (1×i8, 1×i32, 1×i64, 2×i32, 2×i32+1×i8 (padded), {i64,i64}, big aggregate). Pin LLVM type strings as goldens.
- E2E round-trip: C++ caller (or C with `_Alignas` ergonomics) passes a struct by value, C+ callee receives it, returns it modified. Verify byte-equality of the round-trip for each shape.
- Negative: a non-supported target triple emits E0411 with the upgrade suggestion.

**Exit:** Every `#[repr(C)]` struct round-trips correctly through `pub extern fn` calls from C on the two supported targets. The `square(Point)` test from the Phase 1 closeout (where Point is `#[repr(C)] {i32, i32}`) returns `25` instead of `-1454817015`.

**Slice 5.E — Header generation: `cpc --emit-header` · ✅ done**

**Shipped:** `cpc --emit-header FILE.cplus` prints a self-contained C header with `#pragma once`, `<stdbool.h>`/`<stddef.h>`/`<stdint.h>` includes, an `extern "C"` C++ guard, and a prototype/definition for every C-ABI-representable `pub` item. Type mapping covers all primitives (`i32` → `int32_t`, `usize` → `size_t`, etc.), raw pointers (`*T` → `T *`), function pointers, fixed arrays, `#[repr(C)]` structs (as `typedef struct {...} Name;`), and plain enums (as `typedef enum {...} Name;`). Non-C-representable items (`str`, `string`, slice, tagged enum, generic, borrow) are silently skipped — sema's 5.C predicate already rejects them in `pub extern fn` signatures. `cpc build` on a `[lib]` manifest also emits `target/<mode>/<libname>.h` alongside the `.a` / `.dylib`. The generated header passes `clang -fsyntax-only -Wall -Wextra -Werror` (regression-tested) for primitive, struct, enum, and fn-pointer mixes. 8 e2e tests cover stdout output, the `[lib]`-build hookup, non-pub-skipping, and the clang round-trip.

Generate a `.h` so consumers don't hand-write declarations.

**Driver changes:**
- Add `cpc --emit-header FILE.cplus > out.h`.
- For `cpc build` on a `[lib]` target: also emit `target/<mode>/<libname>.h` alongside `.a` / `.dylib`.
- Header content:
  - `#pragma once`
  - `#include <stdint.h>` + `#include <stddef.h>`
  - For each `pub` non-extern fn that's C-exportable: emit a C prototype using the type mapping table.
  - For each `pub extern fn` body (an export): same.
  - For each `pub #[repr(C)] struct`: emit a `struct` definition.
  - For each `pub` plain enum: emit a C `enum`.
  - Items rejected by 5.C's predicate (Drop types, non-`#[repr(C)]` structs, generics): skip with a `// SKIP: <reason>` comment.

**Type mapping table:**
| C+ type | C type |
|---|---|
| `i8`/`u8`/`i16`/`u16`/`i32`/`u32`/`i64`/`u64` | `int8_t`/`uint8_t`/... |
| `isize`/`usize` | `intptr_t`/`size_t` |
| `bool` | `bool` (requires `<stdbool.h>`) |
| `f32`/`f64` | `float`/`double` |
| `*T`/`*mut T` (no C-side const) | `T*` |
| `fn(T1, T2) -> R` | `R (*)(T1, T2)` |
| `#[repr(C)] struct S` | `struct S` |
| plain enum E | `enum E` (or `int` with named constants) |

**Tests:**
- Unit: for a sample C+ source, the generated header round-trips through `clang -c -xc - < out.h` (i.e., it's valid C).
- E2E: build a tiny library, generate the header, write a C caller that `#include`s the header, link against the lib, run.

**Exit:** Library authors get a working `.h` for free. A consumer build flow is `cpc build` → `clang my_app.c -L./target/release -l<name> -I./target/release -o my_app`.

**Slice 5.F — Reference example + design note · ✅ done**

**Shipped:** [docs/examples/c_consumer/](docs/examples/c_consumer/) — the canonical Phase 5 reference. Two crates: `mathlib/` exports one `pub extern fn` per ABI class (scalar, ≤8B aggregate, 16B aggregate, >16B aggregate, plain enum, raw pointer, fn pointer, internal-helper delegation); `c_user/c_user.c` calls every export and asserts the runtime answer. `c_user/Makefile` drives the whole `cpc build` → `clang` → `./c_user` pipeline; both static and dynamic linking covered. Design note: [docs/design/phase5-c-abi-export.md](docs/design/phase5-c-abi-export.md) walks through locked decisions, the ABI classification rule, two worked examples (`square(Point) -> i32` and `make_triple() -> Triple`), and the deferred non-goals. CI smoke test `c_consumer_reference_example_runs_clean` drives the full workflow on every test run (macOS-gated; Linux gets the same shape once x86_64-sysv lands).

Same shape as Slice 3B reference programs — a near-complete example documents the workflow better than prose.

- New tree under `docs/examples/c_consumer/`:
  - `mathlib/Cplus.toml` declaring `[lib] crate-type = "both"`.
  - `mathlib/src/lib.cplus` exposing ~10 lines of `pub extern fn` covering each ABI class.
  - `c_user/Makefile` running `cpc build`, then `clang c_user.c -lmathlib -o c_user`.
  - A shell-level smoke test in CI that runs the whole pipeline and checks stdout.
- Design note: `docs/design/phase5-c-abi-export.md` per project's per-feature workflow. Covers: motivation, locked decisions, ABI classification rules with worked examples, what's rejected and why, what's deferred (Windows ABI, HFA, generics).

**Exit:** The reference example builds + runs in CI. The design note documents the surface and the rationale for the conservatism (Drop rejection, fat-pointer rejection, `#[repr(C)]` requirement).

**Phase 5 exit criteria:**
- All six slices shipped with full test coverage per [feedback_test_discipline.md](/Users/adel/.claude/projects/-Users-adel-Workspace-C-/memory/feedback_test_discipline.md).
- A C consumer can: `cpc build` a C+ library, `#include` the generated header, link against `lib<name>.a` or `lib<name>.dylib`, and call any function whose signature uses only C-ABI-compatible types — including value-passed `#[repr(C)]` structs — with byte-correct results on macos-arm64 and linux-x86_64.
- Reference example in `docs/examples/c_consumer/` builds + runs in CI on both targets.

**Phase 5 non-goals:**
- Windows ABI (x86 nor x86_64). Defer until Windows is a tier-1 target.
- HFA optimization on aarch64. Aggregates of floats go through integer-class coercion in v1 — correct but suboptimal for SIMD-heavy code.
- C++ name mangling. C++ consumers must `extern "C"` the headers themselves (standard practice).
- Generic exports. `pub extern fn foo[T](...)` is rejected by sema; users monomorphize manually.
- Cross-language `Drop`. Types with destructors cannot cross the boundary by value. Workaround: opaque-pointer pattern with a paired `*_free(*T)` export.
- C++ inheritance / virtual / templates. Out of scope; this is a C ABI, not C++.

## Next

Phase ordering is locked: 1 → 2 → 3 → 4. Phase 1 and 2 can technically proceed in parallel (no shared files between LLVM metadata work and manifest/resolver work) but doing them sequentially keeps review surface small.

**Open questions for later** (do not block phase work):
- Per-instruction `!DILocation` for debug info (Phase 1 follow-up).
- Linux/x86_64 parity for stdlib (Phase 3C stretch).
- Whether v0.0.2 ships all four phases or whether Phases 3–4 roll to v0.0.3. Decide once Phase 1 + 2 land and we see real timelines.
