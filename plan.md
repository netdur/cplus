# v0.0.13 — open

Scope undecided. The candidate topics below are the real backlog drawn from
v0.0.12's deferred work, the design docs (`plan.own.md`, `plan.asm.md`,
`plan.jni.md`), and the llama.cplus port's open gaps. Pick a theme; not all of
this lands in one version.

> v0.0.12 (shipped) is archived in [plan-0.0.12.md](plan-0.0.12.md): the
> real-time contract system (8 phases), the llama-port gap round
> (G-034/043/044/045), native `f16`, and the `vendor/jni` adoption.

## Candidate topics

### A. Ownership & drop model completeness
The largest *designed-but-deferred* arc; `plan.own.md` already specs it.
- **Auto field-drop + the `own` marker** ([plan.own.md](plan.own.md)) — recurse
  drop into owning C+ fields (`string`/`Vec`/`Box`/Drop structs); `own ptr: *u8`
  declares a raw resource (→ **W0003** if no releasing `drop`); unmarked raw
  fields stay silent. Closes the silent-leak footgun. *Global drop-semantics
  change — land at a port-milestone boundary, gated by the E0509 migration audit
  in the doc.*
- **`unsafe impl Trait for T {}`** — the opt-out mechanism marker traits need.
  Unblocks the broad "raw-pointer structs are `!Send`" rule (the last open
  Send/Sync piece) without breaking ObjC/channel/mutex FFI.

### B. FFI & literal polish (small, high-leverage, low-risk)
- **`c"..."` C-string literals** ([plan.jni.md](plan.jni.md)) — null-terminated
  string literals so FFI stops needing the `"...\0"` workaround (JNI, Cocoa,
  libc). `str` is a fat pointer; `c"..."` would be a bare `*u8` to a
  NUL-terminated `.rodata` blob.
- **`f16` literal suffix** (`1.5f16`) — deferred polish from G-045; today needs
  `1.5 as f16`.
- **Struct-literal statics** (`static S: T = T { ... };`) — the remaining half of
  G-043 (array-literal statics shipped; struct/aggregate literals still rejected).
  The ggml `static const sphere_t scene[10] = {...}` pattern.
- **Const-eval for array lengths** — `[EXPR; N]` / `[T; N]` still need `N` a
  literal; a small const-evaluator would admit `[T; SOME_CONST]`.

### C. Real-time tail (additive; the roadmap's wrapped, these are the long tail)
- **`rt_linux` / `rt_posix`** siblings of `vendor/rt_darwin` (CLOCK_MONOTONIC=1,
  `sched_setaffinity`, `pthread_setschedparam`).
- **`--realtime-report`** — the machine-readable summary view deferred from
  Phase 8 (`cpc check` already gates; this aggregates violations).
- **`#[no_alloc]` drop-glue** — reject a `Drop` destructor that allocates, run
  implicitly at scope exit (needs ownership analysis; pairs with topic A).

### D. Performance
- **Cross-function inlining / `#[inline]`** (llama.cplus G-041) — cpc only
  auto-inlines trivial getters; a kernel built from `vendor/simd` Tier-2 calls
  keeps them as `bl`. Watch for the Q4_K CPU hot path; fix = run LLVM's inliner
  at `--release` or honor `#[inline]`.

### E. Dogfood — continue the llama.cplus port
The port is the engine that surfaced every gap this cycle. `f16` just unblocked
pure-C+ fp16↔fp32 (the "zero-`.c`" milestone); next is removing the remaining
`cplus-shim` bridges and widening CPU-kernel coverage. Let the port lead and
file gaps as it hits them, pulling ready items (A/B) as needed.

### F. Code knowledge graph (agent + LSP navigation)
Designed in [plan.graph.md](plan.graph.md). A compiler-backed, queryable index —
resolved `def` / `refs` / `callers` / `call-hierarchy` / `type-at` / `members` /
`context` — so an agent (and the LSP) navigates C+ by *symbol and type*, not by
`grep`. The thesis: `cpc` already computes resolved names, types, spans, and call
reachability on every build and throws them away; the graph is **retention +
edge-inversion**, not new analysis. Lands in a new `cplus-core/src/graph.rs`
(pure data over the resolved+typed program), with three consumers off one index:
`cpc query`/`cpc graph` (CLI/CI), a **resident** mode backing `cpc lsp` (folds
the LSP's coarse name-based goto-def onto the real index), and an **MCP adapter**
for direct agent use. Two non-obvious load-bearing points the doc stresses:
(1) **resident, not subprocess** — an on-demand re-parse per query is slower than
ripgrep, so warm-in-memory is what actually kills the grep loop; (2) **adoption
is a design concern** — tool names/descriptions and a `CLAUDE.md` nudge decide
whether the model reaches for the graph instead of its trained `grep` reflex.
The method-dispatch completeness for `callers`/`refs` is mechanical (C+ has no
dynamic dispatch — sema resolves every `recv.method()` to a concrete
`Type::method`), so the only irreducible gap is indirect fn-pointer calls.
Phased roadmap (index skeleton → def/symbols → call edges → reference edges →
type-at → bounds/imports → MCP) in the doc. **This is the strongest standalone
headline candidate** — orthogonal to A–E, high agent-loop leverage, and "cheap"
because the hard analyses already exist and are tested.

## Recommendation

Three coherent shapes:
- **"Code knowledge graph"** (F): the strongest standalone headline. Additive
  (no language-semantics change → low risk), self-contained, and high leverage
  on the agent loop that *builds* C+. Cheap because the analyses already exist;
  the real work is the data model, JSON surface, the resident mode, and the
  adoption design. Doesn't touch the language, so it can run in parallel with
  any of A/B/E.
- **"Finish the ownership model"** (A): highest *conceptual* payoff, but a
  global drop-semantics change — do it deliberately at a port-milestone
  boundary, gated by the E0509 audit.
- **"FFI polish + keep the port moving"** (B + E): `c"..."` and struct-literal
  statics are small, low-risk, and directly remove port friction; let the port
  drive the rest.

Suggested shape for v0.0.13: **F as the headline** (it improves how every future
version gets built), **B + E in parallel** (cheap, port-driven), and **A reserved**
for a clean milestone boundary. F and B/E don't contend — F is tooling, B/E is
language+port — so they can land together.
