# v0.0.24 de-Rust — execution checklist

Branch: `work/v0.0.24-de-rust-vocab`. Full rationale lives in
`plans/plan.md` (authoritative) and `refac.txt` (archive). This file is the
ordered task list: sorted easiest → hardest, with prerequisites called out.
Land each task with the full suite green before starting the next.

**Dependencies at a glance:** `Decision A → 9`, `Decision B → 10`, `7 → 11`.
Tasks 1–6 are independent (any order among themselves).

## Decisions (zero-code blockers)

- [x] **Decision A** → KEEP `let` = immutable, `var` = mutable. `const`
  (module compile-time value) and `static` (mutable C global) already exist,
  so #9's binding work is just the local tiers (let/var) + retiring `mut` +
  the `ref`/`take` param model — NOT const/static. JS `let`=mutable residual
  accepted, not reopened.
- [x] **Decision B** → DROP `pub` entirely. Everything public by default; `_`
  prefix = private. ONE uniform rule across items (fn/struct/enum), fields,
  and methods. `export` stays the separate C-ABI/linker/header marker.

## Tasks (easiest → hardest)

- [x] **1. Dead-keyword deletes** — `trait`/`use`/`mod`/`try`/`union` now emit
  targeted "did you mean" diagnostics. NOTE: kept them RESERVED (not freed as
  identifiers) and routed to tailored errors — freeing `try` would let
  `try { ... }` mis-parse as a struct literal. parser.rs: 4 arms in
  `parse_item` (trait/use/mod/union), 1 in the expr-atom dispatch (try), 5
  `tok_name` entries, 5 unit tests. cplus-core 1529 / cpc e2e 623, both green.

- [x] **2. Forbid same-scope shadowing** — new **E0363** in sema: at the
  `StmtKind::Let` registration (sema.rs:5294), reject when the name is already
  in the *current* scope map; nested-block and parameter shadowing stay legal
  (they live in outer maps). errors.toml entry added. 4 tests (let+let,
  let+`let mut`, nested-allowed, param-shadow-allowed). cplus-core 1533 / cpc
  e2e 623, green. NOTE: `const`/`static` are module-only (no local `const`),
  so only `let`/`let mut` (future `var`) declarations hit this — one site.

- [x] **3. Three arithmetic UB fixes** — all in codegen: shift count masked to
  `& (bitwidth-1)` (was raw `shl`/`ashr`/`lshr`, UB on overshift); float→int via
  `llvm.fptosi.sat`/`fptoui.sat` + `.sat` decls (was raw `fptosi`/`fptoui`, UB on
  overflow/NaN); signed `INT_MIN ÷ −1` / `% −1` trapped in
  `divide_with_zero_check` (was UB in `sdiv`/`srem`). 3 new IR-assertion unit
  tests (matching the existing cast/shift/div test style) + updated the old
  `cast_float_to_int` test. cplus-core 1535 / cpc e2e 623, green.

- [x] **4. `#addr(p)` intrinsic** — ptr→`usize`, the loud `#name()` spelling of
  `p as usize` (inverse of `#addr_of`). Parser already builds the `#name` node;
  added a sema arm + `check_intrinsic_addr` (types `*T -> usize`, gated on
  `unsafe` like `p as usize` — un-gates with the rest in #7) and a codegen arm +
  `gen_intrinsic_addr` (`ptrtoint`). 3 sema tests + 1 codegen + 1 e2e runtime.
  cplus-core 1539 / cpc e2e 624, green.

- [x] **5. `impl` connector `for`→`:`** — parser hard-switches to `impl T: I`
  (inherent `impl T {}` unchanged); a Rust-habit `for` errors with a
  `:`-pointing diagnostic — which also drove completeness (every unmigrated C+
  site failed loudly). Migrated: 11 .cplus sites, all .rs C+ test strings
  (parser/sema + e2e.rs), errors.toml example code, docgen `update_impl_tracker`
  (the scanner already handled `:` via its take_while — comment+test updated),
  stdlib doc-comments. Real Rust `impl X for Y` (5 `fmt::Display`) preserved.
  +2 parser tests (`:` parses, `for` rejected). LESSON: the blanket sed
  clobbered English "for" in 6 freshly-written prose lines (parser
  comment/diagnostic + 2 sema doc-comments) — caught via a git-diff scan and
  fixed; run prose-touching seds BEFORE adding new prose, or scope to code.
  cplus-core 1540 / cpc e2e 624, green. DEFERRED to docs pass (non-compiled
  prose): errors.toml cause/fix text (7), SPEC/SKILL/design-.md narrative,
  ERRORS.md regen.

- [x] **6. `self`/`Self`→`this`/`This`** — FULLY DONE (stage B + stage A).
  **Stage B (dual-spelling):** lexer accepted both; migrated the user-facing
  surface — all vendor/*.cplus + docs/examples (2567 `self`→`this`), protecting
  25 ObjC `self, _cmd` comments; fixed 3 English/path false-positives and 1
  `this`-as-param collision in the uikit demo (`this`→`recv`). **Stage A
  (hard switch):** lexer now rejects `self`/`Self` (only `this`/`This`); a
  bare `self` receiver gets a `this` hint (+ test). Migrated the .rs test
  corpus with a purpose-built string-aware migrator (only rewrites C+-source
  string contents, never Rust code — 417 strings across 9 files) + 1 missed
  bare-`"self"` fragment + bugs/.cplus. Gotchas handled: migrator v1 mis-lexed
  Rust char/byte literals (`b'"'`) → corrupted code (reverted, hardened with
  char/byte/raw/lifetime handling); over-matched a production receiver-display
  string (`graph.rs` bare-arm fixed for consistency). cplus-core 1541 / cpc
  e2e 624, green. Optional later polish: sema `self.x`/`Self`-type hints (~10
  E0300/E0303 sites); docs prose.

- [→] **7. Drop `unsafe`** (DEFERRED — verified recipe ready, do as a focused
  fresh pass; #8 done first) — remove `unsafe {}`/`fn`/`impl` + the E0801 gate
  (lexer/parser/sema); migrate stdlib (unsafe blocks/fns; `unsafe impl
  Send/Sync` → bare marker impl); un-gate `#addr_of`; migrate `p as usize` →
  `#addr(p)`. *Moderate; FFI-heavy stdlib churn. Do before #11 (today's
  `as_str` is an `unsafe fn`).* SEQUENCE (green per step): (1) un-gate raw ops
  (remove the ~32 E0801 gates — 22 standalone `if unsafe_depth==0`, ~10
  compound — + `unsafe_depth`); update ~25 E0801 tests. (2) migrate `.cplus` +
  `.rs` source to drop `unsafe` (string-aware migrator for .rs). (3) make
  `unsafe` a hard error. (4) cleanup `ExprKind::Unsafe`. **Drop-accounting
  (folds in here):** the E0510 free-audit special-cases `ExprKind::Unsafe`
  (sema.rs ~4470) — remove those branches; the E0510 message suggests
  `unsafe { free(..) }` → `free(..)`; and **extend `is_null_guard` (~4452) to
  accept `this.ptr != 0 as *T`** (idiomatic null-guard once `unsafe` is gone —
  today it only matches `is_not_null()`/`!is_null()`, so the `!=` form falsely
  warns W0002). Sound (null owns nothing; structural, not a runtime-condition
  proof); add a test (comparison-form guard → clean).
  **VERIFIED un-gate recipe (tried + reverted to keep the milestone clean):**
  `sed 's/self\.unsafe_depth == 0/false/g'` in sema.rs un-gates all 32 gates
  cleanly (compiles; gates become dead `if … && false`). It breaks exactly 25
  tests, all `*_outside_unsafe_e0801` / `*_is_rejected` (they test the removed
  gate) → DELETE them, don't sloppily flip (their names encode the old
  behavior): sema.rs ~16865/16951/16962/16992/17034/17078/17100/19004/19812/
  19841/20896/21220/21314/22826/22971/24117/24152/24275 + codegen.rs ~19463/
  19577/19596/19734. KEEP the one legit negative test (sema 18992, "inside
  unsafe no E0801"). Then drop the `unsafe_depth` field/inc-dec + make
  `ExprKind::Unsafe` transparent. Reverted because finishing #7 well is a
  focused multi-hour soundness pass, not a tail-of-session rush.

- [x] **8. Type-inferred struct literals** — `let a: A = { … }` /
  `return { … }` (+ argument / nested-field / receiver positions). DONE.
  Added `ExprKind::InferredStructLit { fields }`; parser recognizes `{` +
  `Ident` + `:` in expr position (gated on `!no_struct_lit`, so an
  if/while/for/match body `{` still parses as a block). Sema
  `check_inferred_struct_lit` resolves the struct from `expected`, delegates
  field-checking to `check_struct_lit` (so E0319/E0321/E0322/E0403 + move
  tracking are byte-identical to the named form), and records the resolved
  concrete struct NAME in a span-keyed table on `MonoInfo`. New **E0364** when
  the type can't be inferred (no annotation / non-struct expected) — errors.toml
  entry added.
  **DEVIATION from plan ("codegen reads that table"):** instead, monomorphize
  rewrites `InferredStructLit → StructLit` via the table in `rewrite_expr`, and
  codegen gets a panic-arm — mirroring the EXISTING `GenericStructLit`
  discipline (convert-in-mono / panic-in-codegen). This is strictly more robust
  (both code-producing paths funnel through `rewrite_expr` — confirmed:
  non-generic via `rewrite_item_calls`, synthesized-impl via
  `rewrite_block_with_self`→`rewrite_block`) and avoids threading a 6th map
  through ~10 codegen entry points. The plan author didn't know
  `GenericStructLit` already established this idiom. Generic mangling alignment
  (sema's name == monomorphize's) verified live: `let b: Box[i32] = { val: 42 }`
  runs (exit 42).
  Exhaustive-match arms added/joined across resolver/lower/sema/borrowck/graph/
  monomorphize (all pre-mono passes treat it identically to `StructLit` —
  field-walking, move/borrow/call/alias tracking, E0513 view-escape parity);
  codegen's combined arms joined (dead post-mono but kept for exhaustiveness);
  `attrs.rs` + `fmt.rs` + `rewrite_expr_self` need nothing (catch-all correct /
  token-based / converted-before-the-pass-runs). LIMITATION (by design, safe):
  an inferred literal in a `static`/`const` initializer hits **E0X30** (not
  mono-walked → would reach codegen unconverted, so `is_static_initializer`
  rejects it) — use the named form there. Tests: 3 parser + 6 sema + 3 e2e
  (basic/nested/arg/return, generic, move-into-field no-double-free).
  cplus-core 1550 / cpc e2e 627, both green.

- [ ] **9. Binding model + params** — `const`/`static`/`let`/`var`, retire
  `mut` entirely (no `static mut`/`let mut`/`mut` param); `move`→`take` (owned
  = mutable); bare `x: T` = read-only borrow for EVERY type (verify E0337
  escape coverage); `ref x: T` / `ref this` with the **`ref`-requires-a-`var`-
  place** `is_var` check (closes the immutable-through-by-ref hole, unifies
  E0328). *Hardest core; the headline.* Decision A resolved → scope = let/var
  + retire `mut` + `ref`/`take`; const/static already done (confirm in-task).

- [ ] **10. Visibility** — `pub`→`_` privacy (fields/methods; ~266 field
  `pub`s removed); `export` keyword for the C-ABI/linker/header surface;
  header-gen walks `export` items; **error-level** privacy for raw-ptr /
  custom-`drop` types. Decision B resolved → drop `pub`, uniform `_`-private
  including top-level items.

- [ ] **11. `Text`→`str` coercion** — FIRST re-base the E0513 view-escape
  check off the `as_str` NAME onto coercion sites; THEN add the borrowed-
  `Text`→`str` coercion at arg/binding/return/receiver and drop the `as_str`
  method. *Hardest + highest UB risk.* **Blocked by #7; E0513 re-base must
  precede the coercion.**

## After the renames

- [ ] **Docs / SPEC / SKILL / tutorial pass** — much rewritten by the vocab
  work anyway; coordinate.
- [ ] **Resume bug-hunt** on the stable post-rename base (struct/enum dispatch
  divergences, `.expect("sema validated")` shape-assumptions, block-tail
  expected-type propagation, `subst_ty_plain` gap — see plan.md).
