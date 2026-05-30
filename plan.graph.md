# C+ Code Knowledge Graph Plan

This document records an architecture pass over the `cpc` front-end (lexer → parser → AST → resolver → sema) and scopes what it would take to expose a queryable **code knowledge graph**: a compiler-backed index an agent can ask "where is this function defined", "who calls it", "what's its call hierarchy", "what type is this", instead of falling back to `grep`.

The bet is the same one the GPU and SIMD plans make: `cpc` already computes the hard part (resolved names, types, spans, call sites) on every build. Today that information is thrown away after diagnostics. A graph is just that information, kept and made addressable.

---

## 1. Problem statement

An agent (Claude Code, or any LLM coding loop) navigates C+ source the way a human without an IDE does: `grep` for a name, read the hit, `grep` again for callers, guess at which `Foo` a `Foo::new` refers to. That is lossy and imprecise:

- **`grep` matches text, not symbols.** It can't tell the `Point` struct from a local named `point`, or `Vec::push` on `vec::Vec` from an unrelated `push`.
- **No resolution.** `grep` can't follow `prefix::Item` to the module that defines `Item`, or a method call to the `impl` block it dispatches to.
- **No types.** "What does `make_buf()` return" needs reading the signature; "what is `v` here" needs reading back to the `let`.
- **No reverse edges.** "Who calls this" and "what implements this bound" are whole-tree scans every time.

`cpc` has none of those gaps internally. The resolver knows which file each name comes from; sema knows every type; the borrow checker and the realtime contracts already build call-graph reachability. The graph turns that internal knowledge into a stable, queryable surface.

---

## 2. What `cpc` already computes (and discards)

The architecture pass found the inputs already exist:

- **AST** ([cplus-core/src/ast.rs](cplus-core/src/ast.rs)): every item, statement, and expression carries a `Span`. `ItemKind` covers `Function`, `Impl`, `Struct`, `Enum`, `Const`, `Static`, type alias, `extern`. `ExprKind` distinguishes `Ident`, `Field`, `Index`, `Call`, etc.
- **Resolved project** (`resolver::load_project` → `LoadedProject { program, files }`): the multi-file program after import resolution, plus the source text and `LineMap` per file id (e.g. `src.math`). This is what turns a `Span` into a real `file:line:col`.
- **Typed sema tables** ([cplus-core/src/sema.rs](cplus-core/src/sema.rs)): `structs: Vec<StructDef>` with `fields: Vec<(name, Ty, is_pub)>`, a `methods` map, `is_copy`/`is_drop`, and `origin_file`; a function signature table (`ParamSig` per param with `move_`/`mutable`/`borrow_`); enum defs; resolved `Ty` for expressions during checking.
- **Call-graph machinery** (sema): `build_no_alloc_fn_table` and the `#[bounded_recursion]` reachability walk already enumerate "function X calls callees {…}" across the program. The graph generalizes this from a contract-check helper into a first-class edge set.
- **A name-based goto-definition** ([cpc-lsp/src/main.rs](cpc-lsp/src/main.rs) `find_decls_in_project`): the LSP already answers "definition of `ident`", but by scanning the AST for a matching name. It is coarse on purpose (the design note records that clicking `prefix` of `prefix::Item` won't jump). The graph is the resolved replacement for that scan.

Nothing here needs a new analysis. The work is to retain the resolution/type results keyed by node, and add the reverse edges.

---

## 3. Architectural division: index vs. query surface

Mirroring the language-vs-package split in the GPU/SIMD plans, the graph splits into a **core index** and a **query surface**.

**Core index (in `cplus-core`, a new `graph` module).** Builds a `CodeGraph` from a `LoadedProject` after sema. It owns the node/edge model (§4), stable symbol IDs, and the span→location mapping. It depends only on the existing front-end; it never touches codegen. It is pure data: build once, query many.

**Execution mode is what makes or breaks the loop.** The index runs in one of two modes, and the choice decides whether querying is actually faster than `grep`:

- **On-demand** (`cpc query <kind> …`, `cpc graph`): a subprocess builds the index, answers, and exits. Stateless and simple, ideal for CI and one-shot use. But it re-lexes, parses, resolves, and type-checks the whole project on every call. On a large project that trades the grep loop for a re-parse loop, and ripgrep is milliseconds. Fine for occasional queries; too slow for a tight `refs`/`type-at` cadence.
- **Resident**: the index stays warm in a process and answers from memory. This is the load-bearing mode for the agent loop, and it half-exists already: `cpc lsp` is a resident process that loads the same `LoadedProject`. Treating resident as the default (and on-demand as the CI/bootstrap fallback) is the difference between "kills the loop" and "a different loop".

**Query surface (consumers), in priority order:**

1. **`cpc lsp`**: the resident server reuses the index to gain `references`, `documentSymbol`, `callHierarchy`, and `hover`, and to make `definition` resolved rather than name-based. Editors get this for free, and the index is built once per project, not per request.
2. **MCP server**: a thin adapter that holds the index resident and exposes each query as an MCP tool, so an agent calls them directly with no per-query process spawn. This is the agent-facing path that keeps the loop short. It is *not* "optional, later"; given the thesis is "kill the loop", the resident agent surface is closer to the point than the primitive set is. (See §7.)
3. **`cpc query` / `cpc graph` CLI**: the on-demand path above, for CI, scripting, and bootstrapping the resident modes.

Keeping the index in core (not in the LSP crate) is the load-bearing decision: it means the CLI, the LSP, the MCP adapter, and any future tool all return identical results, and the index is testable without an editor.

---

## 4. The graph model

**Nodes** are program entities with stable identity:

| Node kind | From |
|---|---|
| Module / file | resolver file ids (`src.math`) |
| Function (free) | `ItemKind::Function` |
| Method | `impl` block methods (carries its receiver kind) |
| Struct, Enum, type alias | `ItemKind::Struct` / `Enum` / alias |
| Enum variant | enum payload arms |
| Field | `StructDef.fields` |
| Const, Static | `ItemKind::Const` / `Static` |
| Extern fn | `extern` declarations |
| Bound | `Ord` / `Eq` / `Hash` / `Send` / `Sync` |

Each node carries a **stable symbol ID** (a qualified path, e.g. `src.math::Point::translate`; IDs use the source name, never a mangled `Point__i32`, consistent with [[feedback_cplus_no_mangling]]), a definition `Span` resolved to `file:line:col`, visibility (`pub`?), and the relevant typed facts (signature, `is_copy`/`is_drop`, field types).

**Edges** are directed and typed:

| Edge | Meaning | Source |
|---|---|---|
| `defines` | module → item | resolver |
| `has_method` / `has_field` | type → member | sema tables |
| `calls` | fn → fn/method | call-graph walk |
| `references` | any → symbol use site | AST `Ident`/`Field`/`Call` resolution |
| `returns` / `param_of` | fn → type | signatures |
| `field_of_type` | field → type | `StructDef.fields` |
| `variant_of` | variant → enum | enum defs |
| `imports` | module → module | resolver import edges |
| `has_bound` | generic param → bound | generic params |
| `drops` | Drop type → fields it frees | `is_drop` + drop-body walk (best-effort) |

Reverse edges (`called_by`, `referenced_by`, `implemented_by`) are derived by inverting the forward set at build time, since "who calls / who references" is the most common agent query and a forward-only graph would re-scan for it.

---

## 5. Query surface (what an agent actually asks)

The query set is driven by the operations an agent performs while editing, not by graph-theory completeness:

- `def <symbol>`: definition site(s) of a name. Resolved, so `math::area` and a local `area` are distinguished. Replaces the first `grep`.
- `refs <symbol>`: every use site (`referenced_by`). Replaces the "grep for callers/users" loop.
- `callers <fn>` / `callees <fn>`: one hop on the call edge.
- `call-hierarchy <fn>`: transitive `callers`/`callees` to depth N (reuses the realtime reachability walk).
- `type-at <file:line:col>`: the `Ty` of the expression under a cursor. "What is `v` here."
- `members <type>`: fields + methods of a struct/enum.
- `impls <bound>`: types that satisfy a bound (`implemented_by`).
- `symbols [<file>]`: outline of a file or the whole project (the `documentSymbol` shape).
- `module-deps [<module>]`: import graph, for "what does this file pull in" and cycle detection.

Every query returns JSON: a list of `{ symbol_id, kind, location: {file, line, col}, signature?, ... }`. Locations are clickable `file:line:col`, the same format diagnostics already emit, so an agent can act on them without parsing prose.

### Composite queries (design around the edit, not the graph)

The primitives above are each one hop, which means an agent reconstructing the context of a function it is about to change pays a round-trip per hop, and round-trips are the thing this is supposed to kill. A query designed around the *task* returns the whole neighborhood in one shot. These compose the primitives; they are not new analysis:

- `context <fn>`: the edit-context pack for one function: its signature and source span, its `callers` and `callees`, the types of the symbols it references, the types of the locals it touches, and the diagnostics currently on it. One call gives an agent everything it needs to change `fn` safely, instead of five.
- `neighborhood <type>`: a type's `members`, the functions that take or return it, and its `impls`/bounds. The "I'm about to change this struct" pack.

Composite queries cut the loop further than more primitives would, because the cost being optimized is round-trips, not coverage. The primitive set stays small; the composites are where the ergonomics live.

---

## 6. Interfaces

```bash
# Whole-project graph as JSON (nodes + edges), for indexing or one-shot load.
cpc graph                         # reads Cplus.toml; --format=json (default)

# Targeted queries (agent-facing, fast subprocess calls).
cpc query def    math::area
cpc query refs   Point::translate
cpc query callers sum_range
cpc query call-hierarchy process_frame --depth 3
cpc query type-at src/main.cplus:42:10
cpc query members Vec
cpc query symbols src/main.cplus
```

Output is JSON on stdout; exit code signals found/not-found. The same handlers back the LSP requests (`references`, `documentSymbol`, `callHierarchy`, resolved `definition`, `hover`). The MCP adapter holds the index resident and registers one tool per query kind, so an agent calls `cpc_query_refs(symbol)` with no per-query process spawn (§3).

This keeps the agent loop short: instead of "grep, read, grep, guess", it is "`cpc query context X` → signature, callers, callees, and types in one shot".

---

## 7. Adoption: making it the obvious first reach

A precise index that the agent never reaches for is worthless, and availability does not create adoption. A model whose training biases it toward "grep first" will grep first, even with a better tool one call away. Adoption is a harness and tool-surface problem as much as an indexing one, and it has to be designed, not assumed:

- **Tool names and descriptions are the interface to the model.** The MCP tools have to read as the obvious first reach. Names like `find_definition` / `find_references` / `code_context`, with descriptions that say plainly "use this instead of `grep` to locate or trace a C+ symbol; it is resolved and typed, `grep` is neither." A vague name or a hedged description loses to a trained `grep` reflex.
- **The harness should nudge.** A line in the project's `CLAUDE.md` / skill ("for C+ navigation, query the code graph before grepping; it resolves names `grep` can't") moves the default. This is cheap and it is where adoption actually comes from.
- **This is the same discipline as the language.** C+ is designed so the reader (human or model) can decide locally without hidden global knowledge; the index's tool surface has to be written *for the model* with the same care, so the right move is the legible one. Writing the descriptions is a write-for-the-auditor exercise, not boilerplate.

The cost of getting this wrong is quiet: the index works, the tests pass, and the agent greps anyway. So it is a first-class design concern, not a packaging afterthought.

---

## 8. Implementation roadmap

Phased so each step ships a usable slice and reuses existing front-end output.

- **Phase 1, index skeleton.** New `cplus-core/src/graph.rs`: build `CodeGraph` nodes from the resolved+typed program (functions, methods, types, fields, consts, statics, modules) with stable IDs and resolved spans. `cpc graph` emits nodes as JSON. No edges yet beyond `defines`/`has_method`/`has_field`, which come straight from sema tables.
- **Phase 2, definitions and symbols.** `cpc query def` and `cpc query symbols`, resolved (not name-based). Retire the LSP's `find_decls_in_project` scan in favor of the index; `definition` becomes precise. Full positive + negative + e2e tests per [[feedback_test_discipline]].
- **Phase 3, call edges.** Generalize `build_no_alloc_fn_table` into a reusable `calls` edge set; add `callers`/`callees`/`call-hierarchy`. Method-dispatch (`recv.method()`) is where `callers`/`refs` completeness is won or lost, and the realtime checker skips it today, but it is *mechanical, not theoretical*: C+ has no dynamic dispatch (no trait objects, no vtables), so sema already resolves every method call to a concrete `Type::method` from the receiver's known type. The fix is to consult the resolved target sema already computes, not to over-approximate virtual targets; the realtime checker skips it only because its effects-walker matches callees by name (see [[project_realtime_v0012]]). The one genuinely unresolvable case is indirect calls through fn-pointers, which §10 already scopes out as "indirect call through this fn-pointer type". So the bounded, irreducible gap is small and named; everything else resolves.
- **Phase 4, reference edges.** Walk every `Ident`/`Field`/`Call` and bind it to its resolved symbol, producing `references` / `referenced_by`. This is the highest-value query (`refs`) and the most work, since it needs the same name resolution sema runs, retained per-node.
- **Phase 5, types at positions.** `type-at <file:line:col>` by retaining sema's per-expression `Ty` keyed by span. Powers LSP `hover` too.
- **Phase 6, bounds, imports, drop edges.** `impls`, `module-deps`, and the best-effort `drops` edge. Round out the LSP (`documentSymbol`, `callHierarchy`).
- **Phase 7, MCP adapter (optional).** Expose the query kinds as MCP tools for direct agent use.

---

## 9. Relationship to the LSP

The LSP is a consumer, not a competitor. Today it reimplements a coarse subset (name-based goto-definition) because no shared index exists. Once the index lands, the LSP's `definition`, `references`, `documentSymbol`, `callHierarchy`, and `hover` are thin translations from `CodeGraph` queries to LSP response types. The CLI and the LSP then answer identically by construction, and the index is unit-testable without an editor in the loop.

---

## 10. Non-goals

- **No on-disk database or incremental rebuild.** The index lives in memory; there is no persisted store and no incremental "rebuild only the changed file" yet. A resident *in-memory* index (warm process, rebuilt on change) is in scope and load-bearing (§3); incremental rebuild is a later optimization on top of it.
- **Not runtime/dynamic analysis.** Edges come from the static AST + sema. Dynamic dispatch through fn-pointers is recorded as "indirect call through this fn-pointer type", not resolved to targets.
- **Not a new IR.** This is an *index over* the existing AST/sema, not a replacement IR between AST and codegen. Codegen is untouched.
- **Not monomorphization-aware.** Nodes use source-level symbol IDs; the graph describes the program as written, not its monomorphized instantiations.

---

## 11. Why this is cheap

Every input already exists and is recomputed on every `cpc check`. The graph is retention plus inversion: keep the resolution/type results that sema computes and currently discards, key them by a stable symbol ID, and invert the forward edges once. The expensive analyses (name resolution, type checking, call reachability) are already written and tested. The new surface area is a data model, a JSON serializer, and a handful of query functions, with the LSP folding onto the same index as a bonus.
