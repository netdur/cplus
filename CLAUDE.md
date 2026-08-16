# Working in this repo

C+ is a systems language: LLVM backend, manual memory, ownership with a borrow
checker, one-way C ABI. `cplus-core/` is the compiler, `vendor/` is the package
ecosystem (stdlib, facet UI, AppKit backend), `docs/lang/skill.md` is the language
reference — read it before writing any `.cplus`.

This file is the things that cost hours when you don't know them.

## Use the local compiler

    cargo build --release          # then ALWAYS ./target/release/cpc
    cd vendor/<pkg> && ../../target/release/cpc test

A Homebrew `cpc` on PATH is stale and will not have your changes. Package
suites are run from the package directory; `cpc test` with no file reads
`Cplus.toml`.

## Symbol lookup: the graph, not grep

    cpc mcp        # query / refs / callers, resolved and typed
    cpc query ...

Use it for "who calls X", "where is X defined", "what is the type here". Grep
misses generated code and gives false positives across ~450k lines of vendor.
Answers come back `file:line` and are jumpable; grep is the fallback, not the
default.

## Some source is GENERATED — edit the generator

| Generated | Generator |
|---|---|
| `vendor/facet/src/*.cplus` (control modules, `props`, `elements`, `vocabulary`) | `tools/gen_contract.py` |
| `vendor/appkit/src/appkit.cplus` | `cpc-bindgen` (hand additions go in `appkit_ext.cplus`) |
| `docs/lang/errors.md` | `docs/lang/gen_errors.py` from `docs/lang/errors.toml` |

A fix written into a generated file survives exactly until the next regen. This
has already cost this project real behaviour — see below.

## Before rebuilding any AppKit behaviour, check whether it already exists

`vendor/facet.old/` and `vendor/facet_appkit.old/` are the previous backend
generation **and they worked**. `vendor/appkit/src/appkit_ext.cplus` is
hand-written and survives regeneration.

Two bugs in one session (2026-08-08) were behaviour that still existed in
`appkit_ext` while the regenerated `facet_appkit` had stopped calling it —
`FacetScrollView` (nested-scroll axis forwarding) and the drag-session
press/threshold sequence. Both left comments naming classes the package no
longer built. Grep the old tree and `appkit_ext` first; you may be
reimplementing something whose traps are already documented in its comments.

## Conventions that bite

- **`bugs/`**: top level = open, `bugs/closed/` = resolved. Move a report in
  the same change as its fix. The directory is gitignored, so name the report
  in the commit message — that is the only durable record.
- **`iris/components.txt`**: never cite entry numbers from source. Resolved
  entries are deleted, which renumbers everything below; name the missing
  behaviour in words instead.
- **`playground/`** is gitignored. Local probe apps live there and are not committed.
- **`examples/`** is tracked. Commit-worthy sample programs go there.
- **Don't commit unless asked.** Flip task-tracker checkboxes in the same turn
  as the work.

## Testing

Full coverage per module — unit, e2e, and negative. Don't propose a lighter
bar. UI *feel* (drag, wheel, momentum) cannot be asserted: an agent has no
hands. Pin the state machine in a test, then build a probe app under
`playground/` and ask the user to try it.

Suites, all expected green:

    cargo test --release -p cplus-core      # compiler
    vendor/{stdlib,facet,facet_appkit,terminal,events}   # cpc test each

## Language gotchas that look like compiler bugs

- C+ wears Rust vocabulary with C semantics — verify against the compiler
  rather than pattern-matching from Rust.
- Integer literals wrap through `i32` before `as`; build big masks
  arithmetically (const expressions fold now).
- `take` is a reserved keyword, including as a local name.
- No array→slice coercion; go through `Vec::as_slice`.
- Explicit `return` everywhere except a unit fn ending in `if`/`match`/block.
