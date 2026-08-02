# Issue 15 — Shared tables: SIMD triplicate, layout duo, scalar-list gap

- Status: PARTIAL 2026-08-02, commit <pending> — (a) and (c) done; (b), the
  layout duo, not done, see below
- Type: consolidation (small, mechanical)
- Area: `cplus-core/src/sema.rs`, `codegen.rs`
- Effort: S
- Retires / prevents: three ~150-line tables synced by comment; two layout engines with
  no cross-check; the f16/mask noundef gap
- Master report: `core-drift-audit-2026-08-01.md` (§2 SIMD and layout rows; ABI audit F17)

## (a) SIMD name→type table, three copies

Sites (in sync today at 34 entries each — keep it that way by deleting two):

- sema.rs:16657-16814 (`resolve_type` match; the comment at 16657-16659 mandates the
  mirror: "Each entry here must also appear in `simd_ty_from_name` … and in codegen's
  mirror")
- sema.rs:19528-19677 (`simd_ty_from_name`, free fn)
- codegen.rs:17669+ (`codegen_simd_ty_from_name`)

Change: one shared const table — `pub const SIMD_TYPES: &[(&str, ElemKind, u8 /*lanes*/,
bool /*is_mask*/)]` — in a shared location (lib-level module or ast.rs); all three
consumers derive their answers from it. This is name→shape DATA, so sharing it does not
cross the deliberate sema/codegen id-universe separation.
Verification: a unit test asserting the three consumer paths agree for every entry
(trivial once they read one table); full suites.

## (b) Layout computed twice with no cross-check

- sema.rs:6744-6807 — `layout_of`: struct offset/padding walk; enums `8 + slots*8` with
  `(sz+7)&!7` per payload. Feeds `#[max_stack]` budgets and layout intrinsics
  (`#size_of`/`#align_of`).
- codegen.rs:568 (`align_of_ty`) + 2531-2568 (payload slot computation, same
  `div_ceil(8)` shape).

No shared function; a codegen packing change silently invalidates sema's numbers. Both
sides hold `Ty` plus their own struct/enum tables, so a shared free function
`layout_of(ty, &impl LayoutTables) -> Layout { size, align, offsets }` crosses no id
universes (each side implements the small table trait).
Change: extract sema's walk into the shared fn; codegen's alignment/slot sites call it.
Verification: a differential test — for every struct/enum in a representative corpus,
assert sema's size/align/offsets equal what codegen derives (write it BEFORE unifying to
catch any existing divergence; the audit found none live, but the test is the point).

## (c) `is_scalar_ty` omits `Ty::F16` and `Ty::Mask`

codegen.rs:3625-3649 — while every sibling type list includes F16 (`is_copy_ty` 2789,
tbaa table 266, `static_layout` 3431). Consequence: value-passed f16/mask params
silently lose `noundef`. Cosmetic impact today, but it is the type-list-sync disease in
miniature — fix the two arms and add a comment pointing new scalar types at the sibling
lists (or better: derive the scalar predicate from one list the others use).

## Verification (all)

Full suites; codegen IR-text tests asserting `noundef` on an f16 param after (c).

## Outcome

**(a) The SIMD table, once.** `sema::SIMD_TYPES` is `&[(&str, Ty, u8, bool)]` —
name, lane element, lane count, is-mask — and all three consumers read it:
sema's `resolve_type` arm, `simd_ty_from_name` (path dispatch), and codegen's
`codegen_simd_ty_from_name`, which is now a one-line delegation. Three ~150-line
matches, kept in step by a comment on the first one, are 35 data rows.

Sharing this crosses none of the sema/codegen id-universe separation: it is
name→shape data with no id in it.

The new test `every_simd_name_resolves_the_same_way_everywhere` walks the table
and asserts, for each entry, that the name lookup and the TYPE-POSITION path
(`fn probe(v: f32x4)`, which goes through `resolve_type`) agree — the check the
comment used to ask a reader to perform by hand.

**(c) `is_scalar_ty` gained `Ty::F16` and `Ty::Mask`.** Every sibling list
(`is_copy_ty`, the TBAA leaf table, `static_layout`) already had f16, so a
value-passed `f16` parameter was the only scalar losing its `noundef`. Pinned by
`f16_and_mask_params_carry_noundef`. The arm now carries a note naming the four
lists a new scalar type belongs in — they answer different questions, which is
why they are separate, and nothing but that note says so.

## What is still open

**(b) The layout duo.** Sema's `layout_of` and codegen's `align_of_ty` plus the
payload-slot computation still derive sizes independently, with no cross-check.
The report's own recommendation is to write the differential test FIRST — for a
corpus of structs and enums, assert sema's size/align/offsets equal what codegen
derives — and only then unify behind a shared function with a small table trait.
That test is the deliverable, and it is a bigger piece of work than (a) and (c)
together; the audit found no live divergence, so nothing is broken today.

## Verification (as run)

- `cargo test -p cplus-core` 1850 + 8 (two new tests), `cargo test -p cpc` 608 +
  16 + 5 + 6, `cpc test` in `vendor/stdlib` 290.
- `--emit-ll` over 40 `docs/examples` plus the ABI probes: byte-identical.
