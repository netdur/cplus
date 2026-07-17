# Vendor package documentation guideline

## Why this exists

Every `vendor/*` package should be readable without reading its source. Docs
split into fixed roles so a reader always knows which file to open:

| Need | File |
|---|---|
| Use it in minutes | `docs/tutorial.md` |
| How / why / gotchas | `docs/guide.md` |
| Exact signatures | `docs/ref.md` |

Do not mix those jobs. A signature dump does not belong in the tutorial; a
walkthrough does not belong in the reference.

Reference implementation of this split: `vendor/events/docs/`.

## Layout

```
vendor/<package>/
  README.md              # one-screen surface: what it is, depend, 1–2 snippets
  Cplus.toml
  src/                   # library (+ unit #[test]s, or a dedicated test root)
  docs/
    tutorial.md          # required for packages with a public API worth learning
    guide.md             # required once there are non-obvious choices or lifetimes
    ref.md               # required once there is more than a handful of entry points
  tests/                 # optional: integration / e2e fixtures (flat — no subdirs required)
```

Optional extras under `docs/` are fine when a topic is large enough to stand
alone (`grid.md`, `lifecycle.md`, `backends.md`). They must still **not**
replace the three roles — link them from guide/tutorial, do not re-home the
API manual into a topical essay.

Small packages may ship only `README.md` until the API grows. When you add
`docs/`, add the three files (or an honest subset — see [Minimum set](#minimum-set)).

### README.md

The front door. Keep it short:

1. One-line purpose.
2. `Cplus.toml` dependency + import line.
3. The common-case snippet (one path, not every option).
4. Links into `docs/` for the rest.
5. How to run tests (`cpc test`) — unit location is enough; no separate
   `tests/README.md`.

The README is **not** a fourth tutorial. If a section is growing past a
screen, move it into `docs/` and leave a link.

## Tests

Docs explain the API; **tests prove it**. Keep test material out of `docs/`
(no harnesses under `doc/` or `docs/`).

### Two layers

| Layer | Where | What | How it runs |
|---|---|---|---|
| **Unit** | `src/` (`#[test]` in modules, or a dedicated root such as `src/test_main.cplus` / `src/<pkg>.cplus`) | Fast checks of one package’s types and functions | `cd vendor/<pkg> && cpc test` |
| **Integration / e2e** | `tests/` (flat files — no `e2e/` subfolder, no README required) | Multi-step programs, subprocess builds, archived harnesses | Document in the package **root** `README.md` when present |

### `tests/` (optional)

```
tests/
  lang_e2e.rs            # example: integration harness
  …                      # other fixtures — live directly here
```

- Add `tests/` only when you have files that belong there. Do **not** create
  an empty tree, `tests/e2e/`, or `tests/README.md`.
- Do **not** put harnesses under `docs/` or a singular `doc/` folder.
- The package **root README** states where unit tests live and how to run
  them; mention `tests/` only if it has contents.

### Unit test placement in `src/`

| Pattern | Use when |
|---|---|
| `#[test]` next to the impl in `src/<module>.cplus` | Small / single-module packages (`uuid`, `log`, `json`) |
| Dedicated test root (`src/test_main.cplus`, or `src/<package>.cplus` umbrella) | Multi-module packages or when discovery needs a named entry (`events`, `stdlib`) |

Prefer unit tests **inside the package** so `cpc test` from the package
root is enough.

### What not to do

- Do not treat `docs/` as a test dump.
- Do not invent empty `tests/` scaffolding.
- Prefer growing `#[test]`s with the API; document only behavior tests cover
  when practical.

## The three files

### `tutorial.md` — pick it up fast

**Job:** after a short read, the reader can depend, import, call the main
entry points, and clean up.

**Write:**

- Setup (toml + import).
- Short, copy-pasteable snippets for the happy path only.
- One bound-method / integration line if that is the normal call shape.
- A handful of day-one rules (borrowed payload, token `0`, single-thread, …)
  — only what stops a first crash.

**Do not write:**

- Decision trees, design rationale, “why two layers”.
- Full method lists or signature tables.
- Exhaustive edge cases (point at the guide).

**Length target:** scannable in a few minutes. Prefer code over prose.

Opening pattern:

```markdown
# Tutorial

Quick path: …. Deeper rationale and gotchas live in
[guide.md](guide.md); signatures in [ref.md](ref.md).
```

### `guide.md` — how, why, gotchas

**Job:** teach judgment. When to pick A vs B, ownership and lifetimes,
delivery/mutation semantics, threading, naming conventions, teardown,
integration with other packages (facet, channels, …).

**Write:**

- Comparison tables (`Signal` vs `Bus`, shared vs private, …).
- Explicit **gotcha** subsections for footguns (stale tokens, `off_ctx(0)`,
  use-after-free of receivers, …).
- Contract-level delivery rules when mid-dispatch mutation matters.
- Unsubscribe / teardown strategies.
- Decision trees when there are real forks.

**Do not write:**

- A second copy of every signature (link to `ref.md`).
- A second full beginner walkthrough (link to `tutorial.md`).
- Unrelated package history or roadmap.

Opening pattern:

```markdown
# Guide

How the package is meant to be used, why the pieces exist, and the
gotchas that bite. For a fast start see [tutorial.md](tutorial.md); for
signatures see [ref.md](ref.md).
```

### `ref.md` — the manual

**Job:** look up a type or function and know its signature and behavior.
Nothing else.

**Write:**

- One section per public type / module surface.
- Each method or free function as:

  ```markdown
  ### `name`

  ```cplus
  fn name(...) -> ...
  ```

  One short paragraph: what it does, return meaning, defaults, failure modes.
  ```

- Tables only when they compress enums, fields, or verb lists.
- Cross-cutting contracts once (listener shape, token rules, delivery
  bullets) if every entry would otherwise repeat them.
- Package metadata at the end (import path, deps, how to run tests).

**Do not write:**

- Tutorials, recipes, or “prefer X when…”.
- Long narrative examples (one-liners inside a bullet are fine).
- Design essays.

Opening pattern:

```markdown
# Reference

Manual for the `packagename` package. Signatures and behavior only.
Import:

```cplus
import "packagename/..." as packagename;
```
```

## Minimum set

| Package size | Ship |
|---|---|
| Tiny (few free functions, obvious) | `README.md` only |
| Small public surface | `README.md` + `docs/ref.md` |
| Learnable API with real choices | all three under `docs/` + thin `README.md` |
| Large multi-topic (facet, flex_layout) | three roles **plus** topical files; README indexes them |

If you only write one of the three, write **`ref.md`** — source-shaped truth
ages better than a half tutorial. Add `tutorial.md` as soon as a newcomer
would otherwise have to invent the happy path. Add `guide.md` as soon as
someone has been bitten by a lifetime, ordering, or layering rule.

## Hard separation rules

1. **No signature tables in the tutorial.** One illustrative call is enough;
   the full list is `ref.md`.
2. **No “how do I…” recipes in the reference.** That is guide or tutorial.
3. **No gotcha essays in the tutorial.** One-line day-one rules only; expand
   in the guide.
4. **Source is truth.** Docs describe what is in `src/` today. If the code
   and the doc disagree, fix the doc (or the code) in the same change.
5. **Do not document private / internal helpers** in `ref.md` unless an
   external package must name them. Mark internal types clearly if listed
   for completeness.
6. **Examples must compile in spirit** with current APIs (names, defaults,
   import paths). Prefer snippets taken from or aligned with package tests.

## Prose and code style

- Match the house tone of `naming_guideline.md` and existing vendor docs:
  complete sentences, plain language, no filler.
- Prefer `cplus` fenced blocks for code.
- Use tables for comparisons and API catalogs; bullets for rules.
- Cross-link the other two files from the top of each doc — every entry
  point should be one click from any other.
- Event / API names in docs should match real identifiers (`file:open` only
  if that is the documented convention for that package).

## Checklist before merging docs

- [ ] `README.md` states purpose, depend, import, and links into `docs/`.
- [ ] `tutorial.md` is short; a reader can paste the main path without
      scrolling through rationale.
- [ ] `guide.md` covers choices, lifetimes, and footguns; no full API dump.
- [ ] `ref.md` lists every public constructor/method/verb with signature and
      one behavioral paragraph.
- [ ] No contradictory claims across the three files (tokens, defaults,
      threading, ownership).
- [ ] Snippets use the real import path (`"events/events"`, etc.).
- [ ] Tests or examples still match the documented behavior.
- [ ] Root README says how to run unit tests (`cpc test` + where they live).
- [ ] No test harnesses under `docs/` or `doc/`; integration files sit flat
      under `tests/` if any.

## Naming this guideline

This file is `vendor_docs_guideline.md` at the repo root, next to
`naming_guideline.md`. Package-local docs stay under `vendor/<pkg>/docs/`.
