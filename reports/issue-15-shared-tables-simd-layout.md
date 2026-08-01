# Issue 15 — Shared tables: SIMD triplicate, layout duo, scalar-list gap

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
