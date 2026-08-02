# Issue 06 — Lang-item registry: identity by structure, not by name suffix

- Status: DONE 2026-08-02 — steps 1, 2, 3, 5 in `3ec30cf`; step 4 in
  `1e33595`; step 6 in `1c7cce4`.
- Type: structural consolidation
- Area: `cplus-core/src/sema.rs`, `codegen.rs`, `attrs.rs`; `vendor/stdlib/src`
- Effort: M
- Retires / prevents: bug-08's class (substring hijack); per-process nondeterminism when
  a user defines a generic `Option`/`Iterator`/`Future`; leaf-name marker collisions;
  enables clean fixes for bug-20/bug-21/bug-24
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #6, §4)

## Problem

Well-known stdlib types are located by suffix-matching HashMap keys, and Send/Sync
markers plus the no_alloc-drop blessing key on bare LEAF names. Both mechanisms answer
"which type is THE Option" by string shape, which is shadowable by any user type and, on
a two-key match, nondeterministic per process (std HashMap iteration order). The
structured mechanism already exists and works: `#[lang("string")]` →
`designated_string_struct` (sema.rs:2032-2047).

## Current state

Suffix-matching locator sites (all `.keys().find(|k| k == "X" || k.ends_with(".X"))`
over `std::HashMap` declared at sema.rs:953/958):

- `wrap_in_iterator` sema.rs:5524-5546
- `instantiate_option` 5552-5575
- `unwrap_iterator` 5578-5594
- `wrap_in_future` 5596-5623
- `unwrap_future` 5629-5645 (a user's `Future[T]` is await-able through this)
- `resolve_join_handle_ty` 11255-11262

Leaf-name keying:

- marker registration `(marker, name_leaf(target))` sema.rs:4699-4705, lookup 3078-3113 —
  an `impl Handle: Send {}` in package A unblocks every type named `Handle` anywhere;
- builtin !Send list sema.rs:3141-3147 (`"Rc" | "MutexGuard"` by leaf — any struct named
  `Rc` in any package is !Send);
- `collect_no_alloc_drop_types` 3585-3601 + lookup 3614 — `#[no_alloc] fn drop` on
  `a.Foo` blesses `b.Foo`'s allocating drop.

Codegen side (bug-08): `rfind("Iterator__")` / `Future__` classification
(codegen.rs:16351, `ty_from_future_name`), blessed `next()` arm firing before user
dispatch (14922-14926), and the silent `StructId(0)` fallbacks in
`lookup_future_ty`/`lookup_iterator_ty` (893-916) vs the hardened `lookup_option_ty`
panic (16532-16536).

Runtime-ABI prefix (same disease, small): `__cplus_` built by format string at
sema.rs:8278/10903 and sniffed at resolver.rs:1809 — no shared constant, and user code
can declare `extern fn __cplus_x` to squat the namespace.

## Target design

1. Extend `#[lang(...)]` to `option`, `iterator`, `future`, `join_handle`: attribute on
   the stdlib declarations (find them under `vendor/stdlib/src` — option/future/etc.
   modules), validated in attrs.rs (one target: the declaring package), resolved ONCE in
   sema into dedicated fields exactly like `designated_string_struct`
   (`designated_option_enum`, `designated_iterator_struct`, …). All six locator sites
   read the fields. A duplicate `#[lang(x)]` or a missing one (when the feature is used)
   is a clean diagnostic.
2. Key markers, the builtin !Send list, and no_alloc-drop by RESOLVED identity: resolve
   the `impl T: Send {}` target through the same struct/template tables at registration
   time; store type ids or fully-qualified template names, never leafs.
3. Codegen consumes an origin flag: synthesized Iterator/Future instantiations get a
   `StructInfo` flag at collect time (the `is_lang_string` pattern, 16857); the blessed
   `next()`/await lowering dispatches on it (this is bug-08's fix — coordinate, don't
   duplicate).
4. `RUNTIME_ABI_PREFIX` shared constant for `__cplus_`; reject user-declared `__cplus_*`
   externs outside the stdlib package (new E-code or reuse the reserved-name family near
   E0917).

## Migration plan

1. attrs.rs: accept the four new lang names (+ tests).
2. stdlib: annotate the declarations; `cd vendor/stdlib && cpc test`.
3. sema: add the designated fields + resolution; flip the six locator sites; delete the
   suffix matchers. Negative test: a user-defined generic `Option` no longer shadows
   (and gets a clean "conflicts with lang item? no — simply coexists" behavior: user
   type resolves nominally, stdlib features keep using the designated one).
4. Markers/no_alloc re-keying (+ tests: two same-leaf types in two packages behave
   independently).
5. Codegen origin flag (bug-08's report owns the detail).
6. The `__cplus_` constant + squatting rejection.

## Verification

- Full suites + vendor stdlib suite after steps 2, 3, 4.
- Determinism probe: build a program defining its own generic `Option` twice; identical
  behavior across runs (was nondeterministic before).
- bug-08's repro as e2e; existing gen/async/await tests as the coroutine guard.

## Risks and constraints

- The stdlib annotation is a coordinated two-repo-area change (compiler + vendor/stdlib);
  land compiler-side acceptance BEFORE the stdlib annotations so intermediate states
  build.
- `import "x" as _;` discard-alias imports and the transitive-gate rules must keep
  working for stdlib modules that now carry lang attrs (no import-surface change
  expected; test one).

## Outcome

`#[lang("...")]` now designates five things, and the attribute is the identity:
`string` (already), plus `iterator`, `future`, `option` and `join_handle`. The
marker reaches enums as well as structs (`Option` is an enum), and a name
outside the known set is **E0390** rather than a designation of nothing.

- `attrs.rs`: `LANG_ITEMS` is the list; the spec targets structs and enums;
  `emit_unknown_lang_item` rejects a typo. Two unit tests.
- `sema.rs`: `LangItems` holds the four generic templates by the name their
  template table is keyed under, recorded at template-registration time by
  `record_lang_item` (a second claim on the same item is E0301, the same answer
  duplicate `#[lang("string")]` already got). `lang_template` is the ONE place
  that produces "your build has no `X`" for a feature that needs one.
- All six locator sites read the registry: `wrap_in_iterator`,
  `instantiate_option`, `unwrap_iterator`, `wrap_in_future`, `unwrap_future`,
  `resolve_join_handle_ty`. `unwrap_lang_struct` is the shared unwrapper. No
  `ends_with(".Iterator")` remains.
- `vendor/stdlib`: `#[lang("option")]` on `Option[T]`, `#[lang("join_handle")]`
  on `JoinHandle[O]` (`iterator`/`future` were annotated with bug-08).
- **codegen (step 5)**: `lookup_iterator_ty` / `lookup_future_ty` filter
  candidates by `StructInfo::is_lang_iterator` / `is_lang_future`, and
  `EnumInfo` gained `is_lang_option` so `lookup_option_ty` does the same. This
  turned out to be load-bearing, not cleanup — see below.

### The user-shadowing repro, and what it took

The report asked for a negative test: a user type named like a lang item must
not shadow it. Writing it found that the sema fix alone is not enough.

On the pre-change binary, a program declaring its own `Iterator[T]` alongside
`stdlib/iterator` fails to compile — with an E0302 inside stdlib's OWN
`iterator.cplus`, because the user's template won the suffix match and stdlib's
adapters were then type-checked against it. With the sema registry in place the
program got further and died in codegen instead: `lookup_iterator_ty` and
`lookup_option_ty` matched candidates by mangled NAME, so the user's
`Iterator__i32` / `Option__i32` competed for the coroutine protocol's lookup.
Filtering those three lookups by the lang flag is what makes the program build
and run. e2e `a_user_type_named_like_a_lang_item_does_not_shadow_it` builds and
runs it four times (the failure it replaces was per-process random).

## Step 4 — markers and the no-alloc blessing, re-keyed (`1e33595`)

Three mechanisms keyed on the LEAF name, so two packages that each declared a
`Handle` shared one answer:

- `impl Handle: Send {}` in one module vouched for every type called `Handle`
  in the build — including one wrapping a raw pointer, which is the type the
  structural rule exists to stop. The report asked for a two-package test and
  it is `a_send_marker_in_one_module_does_not_vouch_for_a_same_named_type_in_another`
  (e2e, three files): on the pre-change binary the program compiles clean.
- the builtin `!Send` list matched `"Rc" | "MutexGuard"` by leaf.
- `#[no_alloc] fn drop` on one package's `Foo` blessed another package's
  allocating `Foo`.

All three now key on `nominal_identity`: a type's generic template's declared
name, or its own. Both are qualified by the resolver, and both are what the
template tables are keyed under — which is why a registration site holding an
`impl` target name and a use site holding a `Ty` agree without either guessing.

`Rc` and `MutexGuard` became designated lang items, which is what the registry
from steps 1-3 was for: `#[lang("rc")]` and `#[lang("mutex_guard")]` on the
stdlib declarations, and a build without them blocks nothing. Three sema tests
that relied on leaf matching now carry the marker — they were claiming to BE
those types — and `a_struct_named_rc_without_the_marker_is_send` pins the
other direction.

## Step 6 — the prefix constant and the claim marker (`1c7cce4`)

`__cplus_` was built by format string in sema and codegen and sniffed in the
resolver. It is now `mangling::RUNTIME_ABI_PREFIX`, read by every producer and
consumer.

The squatting half took a decision the report left open: the stdlib
legitimately declares twelve `extern fn __cplus_*` bindings, so a blanket
rejection was not available and "is this the stdlib package?" is not a
question sema can ask. The answer is the same one this issue used for lang
items — write the claim down. `#[runtime_abi]` marks a declaration that names
a compiler-generated symbol; an unmarked one is **E0919**. The stdlib's twelve
now say so.

## Verification (as run)

- `cargo test -p cplus-core` 1845 + 8 (including 2 new attrs tests),
  `cargo test -p cpc` 607 + 16 + 5 + 6; `cpc test` in `vendor/stdlib` 290 green
  in debug and `--release`.
- Test fixtures that stand in for the stdlib declarations (inline `JoinHandle`,
  `Future`, `Iterator`, `Option` in codegen/sema/mono unit tests and three e2e
  programs) now carry the marker — 20 sites, updated deliberately: they are
  claiming to BE the lang item, and that claim is now written down.
- Vendor-wide `cpc check`: the new binary reproduces the same per-package error
  counts as the pre-change baseline (22 for the gtk family, 12 static-arena, 2
  terminal, 0 elsewhere). The A/B script's "before" column is meaningless for
  this change — the old binary rejects the annotated stdlib outright (E0356,
  `#[lang]` on an enum), which is exactly the ordering constraint the report
  called out.
