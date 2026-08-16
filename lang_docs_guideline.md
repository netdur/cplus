# Language documentation guideline

## Why this exists

`vendor_docs_guideline.md` gives every vendor package three fixed
documentation roles, so a reader always knows which file to open. The language
itself has no such thing. Today `docs/` mixes four audiences in one flat
folder: the person writing C+ (SPEC, SKILL, MEMORY-MODEL, ERRORS), the person
reading the compiler (COMPILER.md, design/), a decision archive (GPU.md), and
sample code (examples/). Worse, the first audience — the only one that grows —
is missing the two documents every language needs most: a tutorial and a
guide. SKILL.md is a reference compressed for a context window; SPEC.md is
normative prose. Neither teaches.

This guideline fixes both problems: it names a folder for the language's own
documentation and assigns fixed roles inside it, scaled up from the vendor
trio.

## The folder: `docs/lang/`

The language's documentation lives in **`docs/lang/`**. One folder, one
audience: a person (or agent) writing C+. If a document's reader is instead
someone changing the compiler, it belongs in `docs/compiler/`; if it is a
runnable program, it belongs in `docs/examples/`. Nothing else sits at the
top of `docs/`.

Not a new top-level folder: the repo root already carries the compiler, the
ecosystem, and the apps; `docs/` remains the one place prose lives, and the
subfolder is what gives each audience its own door.

## Layout

```
docs/
  lang/                    # THE LANGUAGE — the only folder a C+ writer needs
    tour.md                # pick it up fast (the tutorial role)
    guide.md               # how to think in C+ (the judgment role); indexes the topic files
    ownership.md           # topic: take/ref/bare, views, Drop — the guide's largest chapter
    error-handling.md      # topic: Status / Option / Result, no-panic patterns
    packages.md            # topic: Cplus.toml, entries, platforms, [link], vendor/
    ffi.md                 # topic: extern, export extern, ObjC, bindgen
    ref.md                 # the manual (the reference role): every construct, lookup-shaped
    spec.md                # NORMATIVE: syntax and semantics          (was docs/SPEC.md)
    memory-model.md        # NORMATIVE: ownership and borrow rules    (was docs/MEMORY-MODEL.md)
    errors.md              # GENERATED from errors.toml               (was docs/ERRORS.md)
    errors.toml            # the registry — edit THIS, never errors.md
    gen_errors.py          # the generator (also publishes to cplus.dev)
    skill.md               # the agent projection                     (was docs/SKILL.md)
  compiler/                # the compiler — audience: someone reading cplus-core/cpc source
    internals.md           # pipeline, passes, seams                  (was docs/COMPILER.md)
    design/                # phase notes and decision records          (was docs/design/; GPU.md joins it)
  examples/                # runnable projects and recipes — code, not prose (unchanged)
```

The topic files listed are the starting set, not a cap: add one when a topic
outgrows its section in `guide.md` (`async.md`, `platforms.md`, `simd.md`).
Every topic file is linked from `guide.md` — the guide is the index, and a
topic file never becomes a second guide.

`plans/` is untracked scratch. Nothing that ships lives there; a plan that
turns real graduates into `docs/lang/` or `docs/compiler/design/` in the same
change that lands the work.

## The roles

| Need | File |
|---|---|
| Write a first program today | `tour.md` |
| Know which construct to reach for, and why | `guide.md` (+ topic files) |
| Look up one construct's exact shape | `ref.md` |
| Settle a dispute about what the language IS | `spec.md`, `memory-model.md` |
| Decode an error code | `errors.md` |
| Load the language into a context window | `skill.md` |

Do not mix the roles. The guide does not restate signatures; the reference
does not teach; the tour does not argue.

### Two tiers of truth

`spec.md` and `memory-model.md` are **normative**: when any other document
disagrees with them, the other document is wrong (or the spec is stale — fix
whichever is behind, in the same change). `tour.md`, `guide.md`, and `ref.md`
are **descriptive**: they must never contradict the normative tier, and they
cite it instead of restating its edge cases. Keep the version stamp at the
top of `spec.md` current — a spec that says 0.0.24 under a 0.0.28 compiler
reads as abandoned.

### `tour.md` — pick it up fast

**Job:** after one sitting, the reader has built and run a program: scaffold
(`cpc init` or the manifest snippet), core syntax, ownership in ten lines,
one enum + match, one error handled, `cpc build` and `cpc test`.

**Write:** copy-pasteable code for the happy path, in the order a first
session actually goes. The handful of day-one rules that stop a first
compile error (explicit `return`, `as` everywhere, `take` is reserved).

**Do not write:** rationale, decision trees, exhaustive forms of anything,
compiler architecture. One link each to `guide.md` and `ref.md` at the top.

**Length target:** readable in under an hour. Prefer code over prose.

### `guide.md` + topic files — how to think in C+

**Job:** teach judgment. When a value should be `take`n versus borrowed; how
to shape an API around `Status` / `Option` / `Result`; how a project grows
from one file to platform entries; what the C ABI boundary does and does not
promise; where the UI stack (`facet`) begins and the language ends.

**Write:** decision trees where real forks exist; comparison tables; explicit
**gotcha** subsections for the traps that cost hours (integer literals wrap
through `i32` before `as`; no array→slice coercion; view lifetimes; the
`_<platform>.cplus` override versus `#platform()`). Each topic file opens
with the decision its topic exists to settle.

**Do not write:** a second copy of any signature (link `ref.md`); a second
walkthrough (link `tour.md`); compiler internals (link `docs/compiler/`);
history or roadmap.

### `ref.md` — the manual

**Job:** look up one construct — a keyword, a type, a statement form, an
attribute, an intrinsic, a manifest key — and know its exact shape and
behavior in one screen. This is the human-paced counterpart of `skill.md`,
organized for lookup rather than for loading whole.

**Write:** one section per construct, each with the grammar or signature in
a `cplus` fence and one behavioral paragraph: defaults, failure modes, the
error codes it raises. Tables where they compress (operators, attributes,
intrinsics, manifest keys). Cross-cutting contracts stated once.

**Do not write:** tutorials, recipes, rationale, internals. No section may
require reading a previous section.

### `skill.md` — the agent projection

SKILL.md already has a clear job and keeps it: the whole language compressed
for a context window, read top to bottom by an agent before writing C+. It is
a **projection** of tour + guide + ref, not a fourth source of truth — when a
rule lands in the language, it lands in the normative tier and the reference
first, and `skill.md` is updated in the same change. Everything CLAUDE.md and
the tooling say about reading it before writing `.cplus` continues to hold;
the pointer just moves with the file.

### `errors.md` — generated, never edited

The registry is `errors.toml`; the generator is `gen_errors.py`; `errors.md`
is output. A fix written into the output survives exactly until the next
regeneration — the same rule as every generated file in this repo. Note the
generator also publishes into the `cplus.dev` checkout; say so in the commit
when it fires.

## Migration map

| Today | Becomes | Watch out for |
|---|---|---|
| `docs/SKILL.md` | `docs/lang/skill.md` | update the CLAUDE.md pointer and every cross-link (SPEC, COMPILER, READMEs) |
| `docs/SPEC.md` | `docs/lang/spec.md` | bump the stale version stamp while moving |
| `docs/MEMORY-MODEL.md` | `docs/lang/memory-model.md` | keep its "normative" status line |
| `docs/ERRORS.md` | `docs/lang/errors.md` | update the output path in `gen_errors.py` |
| `docs/errors.toml`, `docs/gen_errors.py` | `docs/lang/` | registry lives next to its output |
| `docs/COMPILER.md` | `docs/compiler/internals.md` | audience line already says "someone reading the source" |
| `docs/design/` | `docs/compiler/design/` | history; nothing in it is user-facing |
| `docs/GPU.md` | `docs/compiler/design/gpu-position.md` | it is a decision record, not a user doc |
| `docs/examples/` | unchanged | code, not prose |
| — (missing) | `docs/lang/tour.md` | write new — the biggest gap |
| — (missing) | `docs/lang/guide.md` + topic files | write new; `MEMORY-MODEL.md` seeds `ownership.md`'s citations, not its text |
| — (missing) | `docs/lang/ref.md` | write new; SPEC's tables and SKILL's cheat sheet are quarries, not sources to copy verbatim |

Do the moves and the pointer updates in one change; write the three missing
documents in follow-ups (tour first — it is the gap a newcomer hits on day
one).

## Hard separation rules

1. **One audience per folder.** A document whose reader changes halfway
   through is two documents.
2. **No signatures in the tour, no teaching in the reference, no internals in
   the guide.** Same wall as the vendor trio.
3. **The normative tier wins.** Descriptive docs cite it; nothing contradicts
   it silently.
4. **Source is truth, spec is arbiter.** If compiler behavior and `spec.md`
   disagree, one of them is a bug — file it; do not paper over it in a
   descriptive doc.
5. **Generated output is never edited.** `errors.md` changes only via
   `errors.toml` + `gen_errors.py`.
6. **Examples must compile.** Snippets use real import paths and current
   defaults; prefer snippets aligned with tests. `docs/examples/` projects
   are outside test coverage — build them after interface changes.
7. **`skill.md` is a projection.** It updates in the same change as the rule
   it compresses, never instead of it.

## Prose and code style

- The house register: complete sentences, plain language, no filler, no
  asides, no overclaiming. State what is true today; mark what is planned as
  planned.
- No positioning against other languages. Factual mentions are fine
  ("`take` consumes; parameters otherwise borrow"); superiority claims are
  not.
- `cplus` fenced blocks for code; tables for catalogs and comparisons;
  bullets for rules.
- Cross-link the sibling roles from the top of each document — every entry
  point one click from any other, same as the vendor trio.

## Checklist before merging language docs

- [ ] The document sits in the folder its audience owns.
- [ ] `tour.md` still reads in one sitting; nothing in it needs `guide.md`
      to make sense.
- [ ] `guide.md` indexes every topic file; no topic file has become a second
      guide.
- [ ] `ref.md` entries are self-contained and lookup-shaped.
- [ ] Nothing contradicts `spec.md` / `memory-model.md`; the spec's version
      stamp matches the compiler.
- [ ] `errors.md` untouched by hand; `errors.toml` edited instead.
- [ ] `skill.md` updated in the same change as any rule it compresses.
- [ ] Snippets build against the current compiler (`target/release/cpc`).
- [ ] Cross-links resolve after any file move (CLAUDE.md, READMEs, SPEC ↔
      SKILL ↔ COMPILER references).

## Naming this guideline

This file is `lang_docs_guideline.md` at the repo root, next to
`vendor_docs_guideline.md` and `naming_guideline.md`. The language's own
documentation stays under `docs/lang/`; the compiler's under
`docs/compiler/`.
