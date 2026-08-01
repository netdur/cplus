# Issue 11 — Dead code sweep (verified-vestigial paths, one checklist per item)

- Type: cleanup
- Area: parser, ast, sema, lower, borrowck, monomorphize, codegen, resolver, graph
- Effort: S per item; M total
- Retires / prevents: ghost features carried through 7 files; masked pass-ordering
  regressions; reader confusion feeding future drift
- Master report: `core-drift-audit-2026-08-01.md` (§5)

Each item is independently landable. For each: verify the "dead" claim exactly as stated
BEFORE deleting (the audit verified them on 2026-08-01; re-confirm on the current tree),
delete, run `cargo test -p cplus-core && cargo test -p cpc --test e2e`.

## Item 1 — `TypeKind::Borrowed` + `Param.borrow_` + region plumbing (~25 sites, 7 files)

Verify: grep parser.rs for `TypeKind::Borrowed` construction — none (the parser rejects
the retired `borrow` spelling at 1743-1751, 1689-1698); `Param.borrow_` sole producer is
`let borrow_ = false;` parser.rs:1631. Sema's own comment (15095-15097) says the checked
syntax was retired in v0.0.24 #9.
Delete: the `TypeKind::Borrowed` variant and every match arm on it — sema (3780, 4082,
4216, 4405, 4583, 5391, 16580-16585 "kept defensively"), lower (1321), borrowck (537,
576, 1708, 1721), codegen (4063), resolver (2505), monomorphize (820, 1824, 3373, 3641),
graph; unreachable E0511 (sema 15076) and E0512 (15140); region plumbing
`current_fn_param_regions`/`current_fn_return_region` (15034-15087); `Param.borrow_` and
`ParamSig.borrow_` with every `!expected.borrow_` guard (14687, 14712), sig-match 20214,
stale doc 374-382.
Note: coordinates with bug-01/issue-05 (their guards mention `borrow_`) — land either
order, just rebase.

## Item 2 — `&x` / `&mut x` unary parse → targeted retired-syntax hint

Verify: `parse_unary_inner` parser.rs:3010-3023 still parses `UnaryOp::Ref` (using the
retired `mut` keyword); sema unconditionally rejects with E0312 "references are not yet
supported (Phase 5/6)" (sema.rs:16196-16204) — the "yet" is false; v0.0.24 removed
references by design.
Change: reject `&`-prefix in the parser with a targeted hint like the other retired
spellings (policy documented at parser.rs:422-427); delete `UnaryOp::Ref` and its sema
arm; remove `Amp` from the expression FIRST set (parser.rs:4299 / its successor after
bug-15).

## Item 3 — while-let scaffold

Verify: parser.rs:2021-2047 parses the full while-let statement then unconditionally
errors "(while-let not supported in v0.0.7)" with `let _ = &attrs; ... unreachable!()`.
Change (recommend the cheap option): reject at statement start with a clean "while-let is
not supported" before parsing. Alternative recorded for the owner: the comment claims
attrs would lower to the synthesized loop anyway, so support may be near-free — separate
decision, not this sweep.

## Item 4 — Mono/lower dead pattern-let arms (~110 lines) → `unreachable!`

Verify: lower desugars IfLet/GuardLet/WhileLet BEFORE sema (lower.rs:19-23); mono can
never see them. The comment at monomorphize.rs:2222-2224 claiming otherwise is wrong.
Change: monomorphize.rs:2225-2332, visit_ident_calls_in_block 1379-1400, and lower.rs
subst_stmt 1718-1747 defensive twins become
`unreachable!("lower desugars pattern-lets before mono")` — reconstructing the nodes
would MASK a pass-ordering regression.

## Item 5 — Legacy `Ty::String` dual path in codegen (DECISION + delete)

State: 51 sites; every string feature exists twice (legacy `{ptr,i64,i64}` aggregate vs
`#[lang("string")]` Text): `lang_string_or_string` 16875, interp fallback 16303-16313,
`clone_string_aggregate`, `DropKind::String`; sema's unreachable interp arm 12665. Live
only for programs using string features WITHOUT importing stdlib/text.
Recommendation: require the import (str methods already gate this way — the E0613-family
precedent) and delete the legacy arm. This is a language decision for the owner; write
the diagnostic ("string interpolation requires stdlib/text") before deleting.
Interaction: land bug-20 (ToText signature) first — it removes the last blessed-surface
`Ty::String` consumer.

## Item 6 — `satisfies_bound` dead enum arm + E0325 swapped args

Verify: no (iface, enum) pair can register — `validate_interface_impls` (sema.rs:4734)
resolves targets via `struct_by_name`, so enum targets die with E0325 before the sole
insert at 4768; the arm at 3459-3463 is unreachable.
Fix also: E0325's message interpolation is inverted (4737-4739) — prints
"impl Sized2: E" for source `impl E: Sized2`; the language is type-first.

## Item 7 — StructDef field `is_pub` dead slot

Verify: populated (sema.rs:2141, 17023), every read discards it (`_is_pub` at 7176,
10245, 10327) — v0.0.24 moved to name-based privacy. Delete the tuple slot everywhere,
including mono instantiation plumbing (17023).

## Item 8 — `return_passes_by_sret` narrow variant

Verify: codegen.rs:3369 (Ty::String-only) beside `_widened`; its only non-subsumed use
at 6279 is redundant (a C-export Text return classifies Indirect anyway; sema bars Drop
at the C boundary). Delete; callers use `_widened`. (Absorbed anyway if issue-03 lands.)

## Item 9 — Parity fossil + dead param in mono

monomorphize.rs:682-694 (use the value already in hand; delete the `let _ =` theater)
and the `_call_monos` param at 1415. (Also listed in issue-10's plan — dedupe when
landing.)

## Item 10 — `borrowed_params` auto-clone net in codegen

Verify FIRST on current tree: borrowck rejects `return x;` AND `return { x };` for a
borrowed Text param (E0337) — compile both probes with target/release/cpc. If confirmed:
convert the arm at codegen.rs:10093-10101 to `unreachable!` for one release, then delete
it and the 6 feeder inserts (6584, 6604, 8236, 8249, 8491, 8503).

## Minor (bundle freely)

- `edit_distance` duplicated verbatim: attrs.rs:870 and resolver.rs:910 — share one.
- resolver `is_builtin`'s `"println"` entry shadowing user fns — owned by issue-09; skip
  here if that lands.

## Verification (whole sweep)

Full suites after every item; after items 1-2, grep the whole crate for
`Borrowed|borrow_|UnaryOp::Ref` — zero hits outside comments/tests that pin the new
hints. Build one facet example and the vendor suites at the end.
