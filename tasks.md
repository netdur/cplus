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

- [x] **7. Drop `unsafe`** (DONE 2026-06-19 — verified green: lib 1506 / e2e
  611.) E0801 gate + `unsafe_depth` removed; corpus migrated (2363→0 `unsafe` in
  code; only comments remain); `unsafe impl`→bare `impl` (Send/Sync marker
  registers without the keyword; E0861 re-keyed off marker-ness — an empty impl
  of a method interface still errors); parser hard-rejects `unsafe`
  (item/method/field/block); `ExprKind::Unsafe` + `is_unsafe` removed; lexer
  keeps the reserved-rejected token; `is_null_guard` extended to `!= 0 as *T`.
  Behavioral: raw `*p` compiles with no `unsafe`; `unsafe {}` rejected. Spec
  used: `plans/task7-drop-unsafe.md`
  (refreshed 2026-06-19: 2363 `unsafe`/.cplus, ~30 E0801 gates, 9 `unsafe impl`,
  ~30 gate-tests to delete by name, the sed un-gate, the Send/Sync marker-impl
  change, E0510/null-guard fold-ins, `.rs` migrator risk). — remove
  `unsafe {}`/`fn`/`impl` + the E0801 gate
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

- [x] **9. Binding model + params** — `const`/`static`/`let`/`var`, retire
  `mut` entirely; `move`→`take`; bare `x: T` = read-only borrow; `ref x:`/`ref
  this` with the `ref`-requires-a-`var`-place `is_var` check. *Hardest core; the
  headline.* DONE 2026-06-19 — all stages 1–4 landed; commits e2458d4 (3e),
  392a1f4 (`fn(take R)` + flip completion), 92aeed6 (`borrow` retired).
  cplus-core 1540 / cpc e2e 618, green. Two things beyond the original scope:
  (1) the flip surfaced a fn-pointer ownership-convention decision — shipped
  `fn(take R)` (consumes) vs `fn(R)` (borrows); see the 392a1f4 commit (and
  memory `project_cplus_fnptr_ownership`). (2) the region-lifetime internal machinery is now
  unreachable DEAD code (parser rejects `borrow`); its removal is DEFERRED (pure
  cleanup, zero user-facing change) — full scope/risk/ordering in
  plans/plan.md "Deferred follow-up — region-lifetime dead-code removal".
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
  - [x] **Stage 3c-copy — Copy `ref` → `T*` write-back.** DONE 2026-06-20.
    `ref x: T` now lowers to a `T*` out-parameter for ANY type, incl. Copy
    (`bump(ref n: i32)` writes back; verified native + through clang for an
    `export fn`). Codegen: `param_passes_by_ptr` returns true for every `mutable`
    (`ref`) param; the export-sig path emits `ptr` even for Coerce/Indirect Copy
    structs. Sema: dropped the `!is_copy` guard so the `var`-place rule (E0328)
    has teeth for Copy `ref`. Tests: re-pointed the old by-value pins
    (`ref_param_copy_struct_passed_by_pointer`, the `ref_*_propagates`/
    `ref_borrows_of_distinct_copy_places` e2e tests), added native + C-link
    write-back tests. **Corpus migration:** json.cplus used Copy `ref`
    (`Parser`/`Buf`) as the old "mutable local copy" idiom (threading state via
    the return value) — migrated 12 fns to bare param + `var` shadow
    (behavior-preserving). No other corpus affected (the rest are non-Copy refs).
    lib 1511 / e2e green.
  - [x] **export C-ABI signature check now covers plain `export fn`** (found
    while reviewing 3c-copy, 2026-06-20). The E0410 check was gated on
    `is_extern && is_pub`, so a plain `export fn` (which still gets a bare C
    symbol) was never validated — `export fn sink(take b: Buf)` on a Drop type
    silently compiled to an unsound by-value signature. Now every `export`
    (`is_pub`) fn is checked; non-C-representable types (Drop/owning, `str`/slice
    fat pointers, tagged enums, non-`#[repr(C)]` structs) are E0410 in any mode
    (`take`/`ref`/bare). `take`/`ref` of a Copy/repr-C type stays fine.
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
  - [x] **Stage 3e — bare `x: T` move→borrow flip** (DONE 2026-06-19; committed
    with one documented Stage-4 casualty). **cplus-core lib 1563/1563 GREEN;
    cpc e2e 628/629** — the single red, `shared_region_borrow_return_drops_once`,
    is a `borrow A B` REGION-LIFETIME test that Stage 4 deletes with the feature
    (left red on purpose, per user choice "commit 3e first, then remove").
    **How the remaining failures actually resolved:**
    (1) **fn-ptr was NOT a feature gap.** The feared `fn(take R)` type is
    unnecessary: since bare = borrow, a `fn(R)` pointer is correctly a *borrow*
    pointer — the CALLER keeps ownership and drops. Fix was one codegen edit:
    `gen_indirect_call` no longer `mark_moved`s the args (it was disarming the
    caller's drop while the callee — now a borrow — also doesn't drop → leak).
    `fn_pointer_…` (ident + field forms) now exit 8; the fn-ptr-type parser
    only accepts a bare type per param, confirming `fn(R)` can only mean borrow.
    (2) **The real production work: vendor `take` migration.** Container methods
    that STORE a bare non-Copy param into owned memory silently double-freed at
    runtime under the flip (caller drops + buffer drops) — invisible to sema
    because the store is behind `unsafe` raw-pointer writes. Fixed by adding
    `take`: `vec::push`, `vec::set`, `box::set`, `mutex::set`, `hash_map::insert`
    (the constructors `rc/box/mutex::new`, `channel::send`, `ok/err/io_ok/some`,
    `collect` were already `take`). This is what `enum_move_into_method_arg…`
    exercised (now exits 16, no double-free).
    (3) **Ownership-chain threading (the E0337 escape AUDIT in action).** Once
    the containers consume, callers that forward a borrowed value into them are
    correctly caught by E0337 — fixed `identity::push_node` (`take id`),
    `agent_mcp::member` / `ok_response` (`take` the owned json::Value). The
    appkit + mcp `theme_b` tests went green. **AUDIT RESULT: E0337 demonstrably
    fires on every escape shape — return, field-store-in-constructor, and
    re-pass-to-`take` — proven by these three being caught; no silent
    double-free found in the green surface.**
    (4) region collateral: `e0374`/`longest` pass via code-accepts (E0328/E0337
    now precede the region check); `e0384` rewritten to assert the new-correct
    E0337 (the `borrow REGION T` suggestion is obsolete under bare=borrow);
    `shared_region` is the lone Stage-4 casualty (see above).
    [verified recipe below still applies] **VERIFIED
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
  - [x] **Stage 4 — `borrow` removal.** (DONE 2026-06-19.) The `borrow` keyword
    is retired from the language: the parser rejects both the param prefix
    `borrow x:` and the region type `borrow REGION T` with a hint (kept a
    reserved token, #1-style, like `mut`/`move`). The `.cplus` corpus (45 sites)
    and `.rs` test strings were migrated off `borrow x:` to bare (semantically
    identical); all ~34 region-lifetime / E0384-diagnostic tests deleted.
    **DECIDED with user — the fn-pointer convention (a `borrow`-flip
    consequence, see the `feat(fn-ptr)` commit):** `fn(R)` borrows its arg,
    `fn(take R)` consumes it; the marker is part of the fn-pointer type and must
    match the pointed-to function (E0312). This is what made removing `borrow`
    possible — read-only callbacks (auth policies) borrow, thread workers
    consume.
    **DEFERRED (follow-up, not blocking — the language surface is `borrow`-free):**
    the internal region-lifetime machinery is now unreachable DEAD code, not yet
    deleted — `TypeKind::Borrowed` + its ~18 transparent match arms across 9
    files, sema `current_fn_param_regions`/`current_fn_return_region` +
    E0511/E0512 + the `param_owns_value` Borrowed branches, and the borrowck
    return-borrow/region/E0384 subsystem (`collect_e0384_diagnostics`,
    `detect_fn_explicit_regions`). E0384 is preempted by sema's E0337 in the full
    pipeline, so its now-stale `borrow REGION T` suggestion never surfaces. KEEP
    E0513 (str/slice dangling-view escape — not region-specific).
    cplus-core 1540 / cpc e2e 618, both green.

- [x] **10. Visibility** — `pub`→`_` privacy (fields/methods; ~266 field
  `pub`s removed); `export` keyword for the C-ABI/linker/header surface;
  header-gen walks `export` items; **error-level** privacy for raw-ptr /
  custom-`drop` types. Decision B resolved → drop `pub`, uniform `_`-private
  including top-level items. DONE 2026-06-19 (stages 1–4); the visibility model
  is live, `pub` retired. STAGE 4 (Option A, auto-rule): a non-export/non-repr-C
  struct with a raw-ptr field or custom `drop` has its fields module-private
  regardless of name (`struct_fields_are_invariant_private`, sema.rs:401; gated
  at the read + construct E0403 sites); +4 tests (cross-file read/construct
  E0403, same-file clean, export/repr-C exempt); verified green. Chose the
  auto-rule over a 235-struct `_`-rename migration (same safety, no churn,
  can't-forget). Spec: `plans/stage4-visibility-hardening.md`.
  --- (historical: stages 1–3 detail below) ---
  CORE DONE 2026-06-19 (stages 1–3); the visibility
  model is live and `pub` retired. REMAINING (Stage 4, hardening): the
  `_`-rename of invariant-protecting members + the **error-level privacy for
  raw-ptr / custom-`drop` field types** sub-requirement (a struct that hides a
  raw pointer or has a custom `drop` should not expose those fields). The
  pub-drop made all formerly-private members public-by-name; this hardening
  pass `_`-marks the ones that protect invariants. Distinct from the model flip;
  not yet done. **Scope: 235 structs / 112 files qualify (raw-ptr or custom-drop,
  non-export/non-repr-C, with public fields). Full implementation-ready spec
  (two options — auto-rule vs `_`-rename migration — with exact file:line hooks)
  in `plans/stage4-visibility-hardening.md`. User is doing this themselves.**
  **Recon (2026-06-19):** `is_pub` flag on 9 AST nodes, set from
  `eat(TokenKind::Pub)`; today it does DOUBLE duty — (a) privacy: only FIELDS
  enforce it (E0403 cross-file read/construct via `field_with_pub`); item/method
  privacy is NOT enforced today; (b) linkage + header-gen (`is_pub` → external
  linkage + `--emit-header` walks `is_pub` struct/enum/extern-fn). Corpus: 2632
  `pub` (≈2008 items + 273 fields + methods) / 139 files; 46 `pub extern fn`.
  Surface that flips to PUBLIC under the new default: ~1248 non-pub items + ~617
  non-pub fields — per plan, MOST just become public; only invariant-protectors
  get `_` (judgment, small set).
  - [x] **Stage 1 — visibility semantics.** (all sub-stages 1a–done; #10 shipped.) Privacy becomes NAME-based: an
    item/field/method is private iff its name starts with `_`; public otherwise
    (Dart model, public-by-default). Enforce cross-module/file access at error
    level for fields AND items AND methods (extend E0403 beyond fields). Split
    privacy OFF `is_pub` onto the name rule. `pub` still parses (redundant
    no-op) during transition. Update privacy-assertion tests (no-pub fields that
    were private now need `_`).
    - [x] **1a — field privacy name-based.** Added `is_private_name(name)`
      (`_`-prefix); the two field E0403 sites (cross-file read + struct-literal
      construct) now gate on the name, not `is_pub`. Updated the 2 cross-file
      E0403 tests to use a `_y` private field. `is_pub` still drives
      linkage/header (→ Stage 2 export).
    - [x] **1b — item + method privacy name-based.** CORRECTION to recon: item
      privacy was ALREADY enforced (resolver's public-surface map +
      PrivateAccess/E0403), gated on `is_pub`; only the *mechanism* changed.
      Flipped the resolver public-surface builder (8 sites: fn/enum/struct/
      method/interface/alias/const/static) to `exported_name(name)` =
      `!starts_with('_')`; updated E0403 messages (4) + 6 privacy tests
      (resolver ×4, e2e ×2) to `_`-prefixed private items. Verified end-to-end:
      `m::visible()` works cross-file, `m::_helper()` → E0403.
    Stage 1 DONE — uniform name-based privacy (fields + items + methods),
    public-by-default, cross-file E0403. lib 1540 / e2e 618 green. UNCOMMITTED.
  - [x] **Stage 2 — `export` keyword.** (DONE 2026-06-19.) Added `Export` lexer
    token + parser recognition at item + method level. SIMPLIFICATION vs the
    recon plan: no NEW `is_export` AST field — since Stage 1 made privacy
    name-based, the existing `is_pub` flag NO LONGER means privacy; it now means
    "exported" (drives external linkage + fastcc-exclusion + C-ABI check +
    header-gen, exactly as before). `export` sets it; `pub` still sets it
    transitionally (retired in Stage 3, after which `export` is its sole
    setter → `is_pub` becomes `is_export` in all but name). Migrated the 46
    `pub extern fn` → `export extern fn` across 7 .cplus files; updated the
    extern-decl-rejection parser messages (`pub`→`export`); +1 parser test
    (`export_keyword_marks_the_c_abi_surface`). The c_consumer example exercises
    `export extern fn` end-to-end (header + link + run). lib 1541 / e2e 618
    green. UNCOMMITTED.
  - [x] **Stage 3 — drop `pub`.** (DONE 2026-06-19.) Parser rejects `pub` on
    items / methods / fields with a hint (kept a reserved token, #1-style).
    Migrated the whole corpus: dropped ~2560 leading `pub ` + 12 attribute-then-
    `pub` from .cplus, and ~182 `pub` occurrences from .rs test-string C+ source
    (a Rust-lexer-aware string-contents-only migrator — Rust `pub` untouched;
    hit the same `\n`-adjacency `\b` gap as the borrow migration, fixed the few
    residues + `pub opaque`/`pub async`/`pub gen`). Re-`export`ed the C-header /
    linker surface that the pub-drop would have de-exported (mathlib structs +
    enum, the emit_header + lib_target + c_consumer tests). Flipped docgen +
    the code-graph node visibility to name-based (`_`-prefix), and updated
    cpc-bindgen to emit the new syntax (no `pub`; `_`-private opaque/packed
    fields). Repurposed the pub-specific tests: deleted 13 pub-parsing parser
    tests (replaced by one `export`-combos test + a `pub`-rejection test),
    repurposed the E0359 test-fn rule to `export`, fixed the field/graph/docgen
    visibility tests to `_`-private members. 146 files, ~2.9k/3.0k lines.
    cplus-core 1529 / cpc e2e 618, green.

- [x] **11. `Text`→`str` coercion** — DONE 2026-06-19. Borrowed `Text` coerces
  to `str` (its `{ptr,len}` prefix) at arg / binding / return positions; `as_str`
  method dropped; E0513 view-escape re-based onto coercion sites. lib 1506 / e2e
  615 (+4 new coercion tests), all green; ASan-clean.
  **How it landed (deviations from the spec noted):**
  - Coercion hook = the CENTRAL `check_expr` exit (sema.rs ~5762, after
    `check_expr_kind`, before the E0302 mismatch): `expected==Str` &&
    `actual==Ty::Struct(designated_string_struct)` → record `e.span` in
    `text_to_str_coercion_table`, return `Ty::Str`. One hook covers arg/binding/
    return automatically (all route through `check_expr` with an expected type).
  - **E0513 re-base was SMALLER than feared:** the bare-`return t` case needs NO
    re-base — once the coercion makes `t` `str`-typed, `check_returned_borrow` #3
    (str-shaped return + local non-Copy root) already fires. Only the AGGREGATE
    leaf (`return Holder { v: t }`) needed it: `flag_view_leaves` now consults the
    coercion table (`returned_borrow_root` for the root) since there's no `as_str`
    name to key on. `as_str`/`as_slice` name path KEPT for user structs (Buf).
  - **ADDED comparison coercion** (`Text == str` / `str == Text`, BinOp::Eq|Ne) —
    NOT in the spec's 4 positions, but the corpus needs it (matching a built Text
    against a literal is the common op) and it's ergonomic. `str == Text` already
    worked via the existing `check_expr(rhs, Some(lt))`; added the `Text == str`
    (lhs) direction. RECEIVER coercion NOT implemented — `str` has no
    user-callable methods, so it's vacuous; SKILL.md over-claim trimmed.
  - Codegen: `gen_text_to_str` (codegen.rs) extracts `{ptr,len}` via `gen_place`
    (Text is laid out {ptr,len,cap}); a one-span suppression guard
    (`text_coercion_suppress`) breaks the `gen_place`-fallback→`gen_expr`
    recursion on compound coerced exprs (`{ t }` blocks, call results). Span set
    threaded through `generate_inner` (6 callers) onto `ModuleMetadata`.
  - Corpus: dropped `Text::as_str` (text.cplus); migrated 36 Text `.as_str()`
    call sites across 10 .cplus files. `docs/examples/owned_string.cplus` LEFT
    AS-IS (its `OwnedString` is a user struct with its own `as_str`, not the
    lang-string — coercion doesn't apply; removing it would break the example).
    e2e.rs: bulk `.as_str() }`→` }` (21 Text sites), 2 Buf aggregate sites
    RESTORED (Buf is a user struct, no coercion), 2 `.to_text()` round-trips
    rewritten to `Text::contains`, 4 new tests added (E0513 local + aggregate +
    param-guard via a minimal `#[lang("string")]` struct, plus an end-to-end run).
  - Docs: E0513 entry in `errors.toml` updated to v0.0.24 syntax (`ref this`,
    no `unsafe`) + coercion mention; `ERRORS.md` regenerated (synced pre-existing
    drift: E0363/E0364, `impl P for`→`impl P:`). gen_errors.py also rewrote the
    website md (its documented dual output) — local-only, outside this repo.

## After the renames

- [x] **Docs / SPEC / SKILL / tutorial pass** — DONE 2026-06-20 (commits
  12fa4fd, ecfbb05, 67ed3f6, 212159d). SKILL: stripped the C+ history + Rust
  positioning (kept factual "no `&mut T`/`.unwrap()`→E0xxx" guardrails). SPEC +
  MEMORY-MODEL: full v0.0.24 rewrite (MEMORY-MODEL needed real surgery — bare
  `x: T` flipped move→borrow, `take`=consume, region `borrow A T` gone). errors:
  vocab pass + removed 7 unreachable codes (E0801/E0X33/E0X34/E0511/E0512/E0383/
  E0384); wrote a validator that recompiles every repro example (109/109 fire
  their code). Fixed the stale E0359 message (`pub`→`export`). COMPILER/GPU +
  vendor READMEs swept. LEFT as historical: docs/design/*.md, changeslog.md,
  objc-c-interop/notes.md. lib 1506 / e2e 615.
- [ ] **Resume bug-hunt** on the stable post-rename base (struct/enum dispatch
  divergences, `.expect("sema validated")` shape-assumptions, block-tail
  expected-type propagation, `subst_ty_plain` gap — see plan.md).
