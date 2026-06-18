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

- [→] **9. Binding model + params** — `const`/`static`/`let`/`var`, retire
  `mut` entirely; `move`→`take`; bare `x: T` = read-only borrow; `ref x:`/`ref
  this` with the `ref`-requires-a-`var`-place `is_var` check. *Hardest core; the
  headline.* STAGED (in progress).
  **Recon findings (2026-06-18):** (a) today's `let` already freezes field
  writes (`let x: A; x.b = 3` → E0305) and `let mut` allows them — so the
  binding model is a clean RENAME (`let mut`→`var`), NOT a semantic tightening
  of `let`. (b) Migration surface in `.cplus`: ~457 `let mut`, ~26 `static mut`,
  ~267 other `mut` (params/receivers), ~44 `move`, ~60 `borrow`. (c) region-
  lifetime `borrow A T` (`TypeKind::Borrowed`) is effectively unused (only a
  bug-repro comment) but threads 9 compiler files.
  **DECIDED with user:** `take`/`ref`/`var` are reserved as binding/param
  NAMES (rejected with a diagnostic) but stay legal as MEMBER names (after
  `.`/`::`/`fn`) — so `Iterator::take`, `iter.take(n)`, the `fn take`
  definition all survive. Implemented contextually (the words stay lexer
  identifiers; recognized as modifiers only in leading positions; member
  positions need no change). Param-named-`take` vs take-modifier disambiguated
  by 1-token lookahead (`take :` ⇒ name; `take <ident>` ⇒ modifier). 0 params
  named take/ref/var exist; only 2 `let var` locals (uuid, async_fetch) need
  renaming (Stage 2/3).
  - [x] **Stage 1 — additive recognition (dual-spelling).** `var NAME ...` ≡
    `let mut` (StmtKind::Let mutable:true); `ref x:`/`take x:` ≡ `mut`/`move`
    params; `ref this`/`take this` ≡ `mut this`/`move this` receivers. All
    contextual (only where they LEAD a binding/param/receiver, with lookahead),
    so `let var`, `fn take`, `iter.take()`, value-position `var` are untouched.
    Old `let mut`/`mut`/`move`/`borrow` all still work. NOTHING migrated or
    rejected yet. parser.rs only (try_parse_receiver, parse_param modifier loop,
    parse_var_stmt + at_var_binding, block-body + builder-entries dispatch).
    7 parser tests + 1 e2e. cplus-core 1557 / cpc e2e 628, green.
  - [x] **Stage 2 — migrate the `.cplus` corpus.** `let mut`→`var`,
    `mut x:`→`ref x:`, `mut this`→`ref this`, `move`→`take` across 120 files
    (739 line-swaps), via a comment/string-aware migrator (only CODE regions
    touched — `mut`/`move` are keywords in `.cplus`, so every code hit is the
    keyword and safe; comments/strings/`mut_field` left alone). DEFERRED to
    Stage 3 (NOT done here): (a) `static mut`→`static` — needs the SEMANTIC
    change first (bare `static` is still immutable today, so migrating now
    silently breaks every written-to static; verified by the 13 e2e failures on
    the first attempt; the migrator now explicitly PROTECTS `static mut`).
    (b) the `.rs` test-string migration — doing it compile-error-guided in
    Stage 3's hard-switch is safer than a blind string rewrite that could
    corrupt Rust `mut`/`&mut self` or diagnostic-message text. (c) the 2
    `let var` locals + `drop_move`'s `fn take` are fine as-is (var/take not
    reserved as names yet). Surfaced + fixed a Stage-1 GAP the migration
    exposed: `var` wasn't recognized in the C-style for-init (`for (var i …)`)
    — wired into `parse_let_no_semi` + the C-for dispatch (+1 parser test).
    cplus-core 1558 / cpc e2e 628, green.
  - [x] **Stage 3a — migrate `.rs` test-string C+ source** to `var`/`ref`/`take`
    (457 strings, 6 files). A hardened Rust-lexing migrator rewrites ONLY
    string-literal contents — Rust code (`let mut parser`, `&mut self`,
    `fn f(mut x:)`), comments, char/byte literals (`b'"'`, `'\u{..}'`), and
    lifetimes (`'a`) untouched (`cargo build` passing proves no Rust corruption).
    `static mut` protected (escape-adjacency bug found+fixed: a `\n` before
    `static` defeated the `\b`). lexer.rs excluded (its `keywords_and_idents`
    legitimately tokenizes `let mut`; `mut`/`move` stay reserved tokens). E0305
    diagnostic updated to suggest `var`. cplus-core 1558 / cpc e2e 628.
  - [x] **Stage 3b — keyword hard-switch.** Parser rejects `mut`/`move`/`let mut`
    with targeted hints (kept as reserved tokens, #1-style). Reserve
    `var`/`ref`/`take` as binding/param NAMES via `reject_reserved_binding_name`
    (still legal as member names — `fn take`, `iter.take`, `Iterator::take` all
    work; verified). Renamed the 2 `let var` locals (uuid→`variant`,
    async_fetch→`env_p`). Repurposed the obsolete combo tests to `ref take`/
    `take ref` (still parse-permitted, still E0334 in sema) and flipped the
    `let var` test to assert rejection; added 5 parser rejection tests + 1 e2e.
    `#asm` operand message updated `mut`→`var`. cplus-core 1562 / cpc e2e 629.
  - [x] **Stage 3c — `ref`-requires-a-`var`-place `is_var` check.** Wired the
    pre-existing `is_writable_place_quiet` into `check_arg_with_move` (the
    single arg/param-modifier site, covers direct + fn-ptr + generic calls):
    a `ref` argument must be a `var` place, reusing E0328 (the receiver
    analogue `ref this` was already E0328-checked). No callee-body inspection.
    Closes the live hole (immutable `let` mutated across the param boundary).
    SCOPED to non-Copy `ref` for now — those are lowered by-pointer today, so
    the write-back is real; a Copy `ref` is still passed by value, so demanding
    `var` there would be a false promise (see 3c-copy). Excludes `borrow_`
    (read-only) and `take` (consume). Fixed the call-site fallout: 6 e2e
    borrow-exclusivity/partial-place/env tests passed `let` to `ref` params →
    `var` (incl. `stdlib_env_var_into`, a real latent bug — `var_into` fills its
    `out`). +3 sema tests, +1 runtime e2e (write-back reaches the caller). The
    DECISION that unblocked this: per mem.md, `ref` ALWAYS requires `var` (single
    is_var check, all types); there is NO read-only `ref` — read-only-by-ref is
    bare. So the 3c/3e "intertwining" I worried about was a misread; the noalias
    tests that used read-only `ref` just need `var` callers (interim; they move
    to bare at 3e). cplus-core 1565 / cpc e2e 630.
  - [ ] **Stage 3c-copy — Copy `ref` → `T*` write-back.** mem.md: `ref x: T`
    "lowers to a C out-parameter (`T*`)" for ANY type, incl. Copy (`bump(ref x:
    i32)` writes back). Today Copy `ref` is by-value (the C-ABI coerced form;
    pinned by `mut_param_copy_struct_passed_by_value_c_abi`). Unifying requires
    dropping the `!is_copy` guard in `is_mut_pointer_passed` (sema) + the codegen
    counterpart, extending the non-Copy by-pointer path to Copy, updating that
    ABI test, AND verifying the new `T*` lowering against clang (strict-C-ABI
    rule). Then extend the 3c is_var check to Copy `ref` too.
  - [x] **Stage 3d — `static` is the mutable, addressable global; `static mut`
    gone.** Per mem.md: every `static` is mutable, access is BARE, and an
    immutable addressable global is "a `static` you never write" → DECISION:
    leftover immutable `static` STAYS `static` (not converted to `const`; `const`
    is the inlined, non-addressable immutable value). Parser rejects `static mut`
    with a hint (is_mut always true). Sema dropped all three static gates:
    E0305 (immutable-static write), E0X33 (static read needs `unsafe`), E0X34
    (static write needs `unsafe`) — the `static` declaration is itself the
    marker; cross-`static` data races are the developer's responsibility. (This
    is the static-side of `unsafe`, which #7 retires wholesale; the deref/cast
    `unsafe` surface is untouched.) Codegen: all statics emit as `global`
    (.data) — the `constant`/.rodata path was the old immutable static. Migrated
    `static mut`→`static` across .cplus (9 files) + .rs test strings; repurposed
    the 3 gate sema tests to positive bare-access tests, updated 4 codegen
    `constant`→`global` assertions + the parser `is_mut` assertion, deleted the
    obsolete E0X34 e2e test, fixed the const-static-globals e2e + a lower test.
    cplus-core 1563 / cpc e2e 629, green.
  - [→] **Stage 3e — bare `x: T` move→borrow flip** (DEFERRED — verified core
    recipe ready; the migration is a dedicated stdlib-wide pass). **VERIFIED
    core change (tried + reverted to keep the tree green, like #7):** the flip
    is 3 small edits — (1) codegen `effective_move` → `p.move_ && matches!(ty,
    Struct|Enum|String) && !is_copy_ty` (bare no longer moves; only `take`
    does); (2) sema `consume_value_arg` → only `move_` consumes (bare doesn't
    mark_moved); (3) the 3 sema `owns_value:` param sites → `param.move_ ||
    self.is_copy(&psig.ty)` (bare non-Copy is a non-owning borrow — must hoist
    the `is_copy` call before the `&mut self` scope insert; the 3 sites sit right
    after the `restrict`/E0411 block). Bare non-Copy ALREADY passes by-pointer
    (`param_passes_by_ptr` unchanged), so this only flips ownership/drop, reusing
    the sound `borrow` path; escapes are caught by the existing E0337 the
    `owns_value=false` flag triggers. NO `param_passes_by_ptr` change needed.
    **MIGRATION SCALE (why deferred):** the flip is GLOBAL and ALL-OR-NOTHING —
    a partial migration leaves vendor uncompilable.
    **MEASURED SCOPE (two grind attempts, both reverted to keep green):** ~57
    sites total — **33 cplus-core lib + 32 cpc e2e failures** — NOT the 200+
    first feared. Crucial finding: container stores via a raw-pointer write
    (`Vec::push`'s `*slot = value`, etc.) do NOT trip the tracked-move check, so
    VENDOR PRODUCTION CODE is largely unaffected; the surface is almost entirely
    TEST SNIPPETS + a few real passthroughs. Two kinds: (a) generic/concrete
    passthroughs `fn f[T](x: T) -> T { return x; }` (return/forward a bare param)
    → `take`; (b) consume / E0335 / double-free / E0502-not-Send tests whose
    expectations shift under the flip. **CARE REQUIRED — blanket regex
    BACKFIRES:** a global "identity → `take`" sed bumped failures 25→29 by adding
    `take` to helpers in NEGATIVE tests (E0502/consume), where the now-consumed
    arg introduces spurious E0335 that breaks `assert_only_code`. So this is a
    careful PER-TEST pass, not a blanket one. VERIFIED core flip recipe (above)
    is correct and re-applies cleanly. AUDIT during it: adversarially confirm
    E0337 catches EVERY escape (return / field-store / global-store /
    re-pass-to-`take`) — a miss is a silent double-free (v0.0.14 json /
    v0.0.17 string class). e2e borrow-region tests (`borrow A T`) also fail —
    likely Stage-4 collateral, confirm during the pass. cplus-core 1563 / cpc
    e2e 629 at 3d.
  - [ ] **Stage 4 — `borrow` removal.** Param prefix `borrow x:` folds into the
    bare default. Region-lifetime `borrow A T` / `TypeKind::Borrowed` (9-file
    thread, E0511/E0512, ~19 e2e region tests) — SCOPE TBD with user (in #9 vs a
    separate follow-up).

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
