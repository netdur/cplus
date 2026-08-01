# Issue 06 — Lang-item registry: identity by structure, not by name suffix

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
