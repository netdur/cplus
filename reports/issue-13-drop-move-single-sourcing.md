# Issue 13 — Drop/move machinery: fail loudly, classify once

- Type: structural consolidation + targeted fixes
- Area: `cplus-core/src/codegen.rs`
- Effort: M
- Retires / prevents: the scanner-miss double-free class (≥4 patch comments in the
  scanner's own history); the needs_drop mirror desync; assorted verified leaks/holes
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 2; emission audit F6-F10, F13)

Independent sub-items; land separately. Line numbers from 2026-08-01.

## (a) Disposition-aware `find_drop_flag` — make scanner/emitter drift fail loudly

The invariant: the syntactic pre-pass `scan_moves_in_*` (codegen.rs:3658-4020) must
over-approximate every one of ~35 emission-time `mark_moved` sites. Today a miss means
`find_drop_flag` (9257) returns the `%x.drop_flag.unused` sentinel fabricated for
`Always`-disposition entries (9228-9230) — a store to an undeclared SSA name, or a
double-free. The scanner's comments record at least four historical bugs of this shape
(3776-3787 fn-ptr args, 3832-3833, 3855, 3908, 3972).
Change: `find_drop_flag` returns None for `Always` entries; add a debug_assert that
`mark_moved` is never called on an `Always` binding. Drift then fails at compile time of
the test suite instead of at the user's runtime.
Long-term direction (record, do not do here): sema/borrowck already compute
flow-sensitive moves — export a span-keyed move-site set and delete the syntactic
scanner.

## (b) `carries_drop` decided once

"Does this type need drop" is computed in sema (`ty_carries_drop`, sema.rs:3553) and
mirrored in codegen (`needs_drop`, codegen.rs:9317-9335, "Mirrors sema's
ty_carries_drop") plus restated inside `register_value_drop` (9178-9192). Widening drop
semantics in sema silently desyncs codegen.
Change: precompute a `carries_drop` bit on StructInfo/EnumInfo at `collect_types`
(where `is_drop`/`is_tagged` already live); `needs_drop` and `register_value_drop` read
the bit. (Sema keeps its own copy — different id universe — but each side is then
internally single-sourced; a differential test compiles a corpus and compares both
answers via a debug dump if cheap to add.)

## (c) Delete the duplicate place predicate

`method_receiver_is_place` (codegen.rs:14827-14841) is a character-identical copy of
`is_place_expr` (9091-9109). Verify identical; delete one, call the other. The v0.0.26
temp-drop semantics depend on them agreeing.

## (d) Field-load cache invalidation — one helper

The cache is invalidated on named and method calls but NOT indirect or assoc calls:
invalidation sites 8890 (open_block), 9752 (stmt), 13636 (gen_named_call), 14865
(gen_method_call), 15799 (assign), 15947 (watch); missing at `gen_indirect_call` (13356)
and `gen_assoc_call` (15470) — a cached `s.f` reused after a fn-pointer call that
mutates `s.f` through a `ref` param reads stale within one statement.
Change: one `emitted_call_invalidate()` helper called wherever a `call` is emitted (or
fold into issue-08's emit_call funnel if that lands first). Add a repro test: struct
field mutated through a fn-pointer `ref` call, read in the same statement.

## (e) Option variant tags by name, not source order

Coroutine lowering hardcodes Some=0/None=1 "by declaration order in stdlib/option.cplus"
(codegen.rs:16399-16400, 16434-16439, 16464). Reordering the stdlib enum silently flips
tags in coroutine lowering ONLY (normal construction uses the enum's variant table).
Change: resolve the tag by variant NAME from `EnumInfo.variants` at the three sites.

## (f) Interp-string rvalue Text temporaries leak

`gen_interp_str` rolls a private `temps_to_free` and exempts Text parts as "owned by its
binding" (codegen.rs:16211-16216) — but an rvalue part (`"${make_text()}"`) has no
binding, and no `register_temp` site covers interp (sites: 10221, 13681, 15084, 15227,
15287, 15406, 15568). The buffer leaks.
Change: register rvalue Text parts as statement temps (`is_place_expr` distinguishes
place vs rvalue). Leak-check with the `leaks` tool pattern the project already uses for
appkit, or a MADE==DROPS counter test.

## Verification

Full suites after each sub-item; (a) needs a debug-assert build run of the whole e2e
suite; (e) add a unit test pinning tag-by-name; (f) a drop-counter e2e.

## Risks and constraints

- (a) may surface latent scanner misses immediately (that is the point) — fix those as
  their own bugs, do not widen the sentinel back.
- Do not touch the deliberate name-rederivation seam; (b)'s bit lives on codegen's own
  collected infos.
