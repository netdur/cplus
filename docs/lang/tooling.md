# Tooling — the compiler and everything around it

The decision this page settles: **which command answers the question you
have.** Every one of them is offline and version-matched to the compiler in
front of you; none of them needs a network.

The one-line commands live in [ref.md](ref.md#cli). This page is the model:
what each tool reads, what it produces, and when it is the wrong tool.

## 1. The build commands

```bash
cpc build                     # multi-file: reads ./Cplus.toml, walks imports
cpc FILE.cplus -o BIN         # single file, no imports, no manifest
cpc check                     # whole project, front-end only, no codegen
cpc check FILE                # one file, front end + codegen (discarded)
cpc test                      # #[test] discovery and run
```

The distinction that costs people the most time: **`cpc check FILE` does
not read the manifest.** A file with any `import` fails there with E0852.
Single-file mode is for import-free snippets; anything real goes through
`cpc build` or project-mode `cpc check`.

The other asymmetry is deliberate: project-mode `check` stops after
borrowck, so it is the fast feedback loop; file-mode `check` also runs
codegen and throws the IR away, so a codegen-stage *fault* — a panic in the
emitter — is caught too.

**What `check` cannot catch: invalid IR.** It never invokes clang, so IR that
cpc emits but LLVM rejects passes `check` and fails only in a real build. That
gap is not hypothetical — `==` on an array type-checked, emitted
`icmp eq [2 x i32]`, and died in clang with no error code and no span in the
user's file. When the question is "does this actually compile", build it;
`check` answers "is the front end happy".

Build flags, all of which apply to `cpc FILE`, `cpc build`, and `cpc test`:

| Flag | Effect |
|---|---|
| `--release` | `-O3`, no overflow checks on `+ - *` |
| `--debug` | `-O0` with overflow traps — the default |
| `-g` / `--debug-info` | DWARF metadata, and `-g` to clang |
| `--asan` / `--ubsan` / `--tsan` / `--msan` | LLVM sanitizers; asan/tsan/msan are mutually exclusive, ubsan composes |
| `--target NAME` | cross-compile ([platforms.md](platforms.md)) |
| `--min-os VERSION` | override a versioned triple's OS floor; place *after* `--target` |
| `--fp-contract=off\|on\|fast` | float contraction; `off` keeps `a*b+c` as fmul+fadd for bit-identical-to-C output |
| `--warn-deps` | show dependency warnings too |
| `--timings` | build cost to stderr, per phase and per package |
| `--diagnostics=human\|short\|json` | diagnostic rendering |

**`--warn-deps` is worth knowing about.** By default warnings are shown for
your own `src/` only, which keeps a cold build of a large app readable —
but it also means a vendored package's warnings are invisible until you ask.
Errors are never suppressed.

**`--timings`** answers "why is this slow" with data instead of a guess. It
prints the phase table (resolve+sema+borrowck / codegen / prune /
clang+link) and then a per-package roll-up. On this project the answer is
usually clang.

## 2. The code graph — use it instead of grep

C+ ships a resolved, typed code-knowledge graph. For *any* "where is X",
"who calls X", "what is the type here" question, query the graph. It
returns the answer already resolved, which removes both the grep pass and
the reasoning you would spend disambiguating names and stitching call sites
together. Across a large vendor tree, grep also misses generated code and
produces false positives that read as real.

Three front ends over one index:

| Front end | Shape | Use when |
|---|---|---|
| `cpc graph` | whole graph as JSON on stdout | you want to process it yourself |
| `cpc query <kind> …` | one answer as JSON, exit code signals found | a one-off from a shell |
| `cpc mcp` | resident MCP server on stdio | an agent or editor asking repeatedly |

`cpc query` kinds:

```
def SYMBOL              members TYPE          symbols [FILE]
refs SYMBOL             callers FN            callees FN
call-hierarchy FN [--depth N]                 context FN
type-at FILE:LINE:COL   value-refs FILE:LINE:COL   scope-at FILE:LINE:COL
complete FILE:LINE:COL
```

`complete` is the composed one. `scope-at`, `type-at` and `members` are the
three primitives a caret question decomposes into, and deciding *which* of the
three a caret is asking — after a `.`, after a `::`, or neither — is C+'s own
rules, not the caller's policy. So that decision lives in the compiler and one
verb answers it:

```
after a `.`    the receiver's fields and methods (variants are `::`, not `.`)
after a `::`   the module an alias binds, or a type's methods and variants
otherwise      everything in scope
```

The answer names its own `context` (`member` / `path` / `scope`), the `prefix`
it filtered on, and the ranked `items`. `receiver_type` absent on a member
answer means the receiver's type is not locally known — an empty list, never a
guess.

Every `cpc query` invocation pays the whole-project graph build (~2s). An
editor asking on a keystroke wants `cpc mcp`, which builds once and answers
from memory.

`cpc mcp` is the one to reach for from an agent. Beyond the read tools
(`find_definition`, `find_references`, `find_callers`, `find_callees`,
`call_hierarchy`, `find_members`, `file_symbols`, `code_context`,
`type_at`, `scope_at`, `complete_at`) it is **live**:

- `did_change` hands over an unsaved buffer; every later answer is about
  that text. The caret is always in a buffer that differs from disk, so for
  anything at the caret this is not a refinement, it is the whole question.
- Rebuilds run on a worker. Reads never block — they answer from the newest
  finished graph. Pass `wait: true` on the call right before a question
  whose answer must reflect this exact text.
- **A buffer that does not parse is normal**, not an error state: the last
  good graph keeps answering and `graph_status` carries the parse error.
- `reload` rebuilds from disk after a branch switch, a generator run, or a
  dependency update — changes the server cannot see.
- `graph_status` observes without rebuilding. Ask it before concluding an
  answer is wrong.

`cpc lsp` starts the language server on stdin/stdout over the same index
(it delegates to the `cpc-lsp` binary on PATH or next to `cpc`). It is
resident on the same terms as `cpc mcp` — one graph per project root, built on
the first graph-backed request and kept warm, with open buffers overlaid onto
it and rebuilds on a worker — and it serves `textDocument/completion` from the
same `complete` composition, so an editor and an agent get the same answer at
the same caret. Trigger characters are `.` and `:`.

`code_context` deserves its own note: it is the one-shot edit pack for a
function — signature, callers, callees, and the types it touches. Prefer it
over three separate lookups when you are about to change something.

**`#[test]` functions and completion.** `scope-at` and `complete` answer "what
can I type here", and a test function is never the answer — it takes no arguments, only
the harness calls it, and in a suite-carrying module the tests outnumber the
API. So both omit them. They stay everywhere else: `def`, `refs`,
`callers`, and `callees` all find them, because a test calling a helper is a
real call edge and hiding it would make `callers` lie. In `symbols` — the
file outline, which should list tests — each one carries `is_test: true`, so
a consumer using that list for completion can filter on the same rule.

## 3. Diagnostics

```bash
cpc explain E0502            # cause, fix, worked example — offline
cpc explain --list           # every code
```

The diagnostics are the designed teaching surface of this language. When an
error and your intuition disagree, `cpc explain` before you edit: it
carries the *redesign* the message alone cannot.

`--diagnostics=json` emits NDJSON, one object per diagnostic — the shape a
CI annotator or an editor wants. `short` is one line per diagnostic.

Warnings worth reading rather than silencing:

| Code | Says |
|---|---|
| `W0002` | a `drop` frees a raw-pointer field only conditionally |
| `W0005` | a `.cplus` file under `src/` is unreachable from the entry — it never compiles, so nothing it says is checked |
| `W0006` | a `#[deprecated]` item is used here |
| `W0824` / `W0825` | a callback parameter is declared without its `_ctx` slot, so callers can never pass a bound method |

W0005 matters more than a warning usually does: unreachable code is false
evidence. A reader — human or agent — takes it for the live API.

## 4. Source and docs

```bash
cpc fmt FILE|DIR …           # rewrite in place
cpc fmt --check DIR          # no write; exit non-zero on any diff
cpc fmt --emit FILE          # print to stdout, leave the file alone
cpc fmt --stdin              # read stdin, write stdout — the editor hook
cpc doc FILE                 # public items + `///` docs -> target/doc/<name>.md
cpc headers                  # lib/include/ for the package in this directory
```

`cpc fmt` settles layout arguments. If a file does not round-trip through
the formatter, the file is wrong; there is nothing to discuss.

`cpc headers` turns `src/` into C+ *declaration* files under `lib/include/`:
a concrete module becomes signatures (`fn f(...) -> T;`), and a module that
declares generics is copied verbatim, because a generic has no object code
until a consumer instantiates it. This is what lets a prebuilt package be
consumed without its source.

`cpc --emit-header FILE` is a different thing: a **C** header for every
C-ABI-representable `export` item, for a C consumer.

## 5. Reading the language itself

```bash
cpc skill                    # the whole language reference for an agent
cpc skill --lang-only        # …without dependencies' own SKILL.md files
```

`cpc skill` prints [skill.md](skill.md) embedded in the binary —
version-matched, no network. Inside a project it also prints the SKILL.md of
every dependency that ships one, which is usually what you want: the
language plus the packages actually in this build.

## 6. Packages

```bash
cpc init [--kind cli|gui] [--platform P]... [NAME]
cpc pm install [DIR]                  # resolve deps into the store
cpc pm update  [DIR]                  # re-resolve and refresh
cpc pm add DIR NAME [SPEC]            # add a package and its declared closure
cpc pm remove DIR NAME                # delete DIR/vendor/NAME
cpc pm manifest [DIR]                 # normalized JSON of a manifest
```

`cpc init` with no `--platform` scaffolds the zero-config host app. With
`--platform` (repeatable) it scaffolds a deliberately **scoped** app: one
`[<platform>] entry` per named platform, and building for one you did not
name is E0413 rather than a guess. `--kind gui` scaffolds a facet app with
the backend's full dependency closure already in the manifest; without
`--kind`, the platform decides — `--platform ios` is `gui` and cannot be
anything else, because iOS has no console for a printing entry to print to.

`cpc pm add` is the one to prefer over hand-editing `[dependencies]`: it
writes the package **and its declared closure**, mapping platform sections
onto your project's target platforms. Dependency resolution is flat — the
resolver validates every import against your one manifest and never reads a
dependency's own — so a missing transitive line is an error in *your*
manifest (E0852 / E0866), which is exactly the class of error `add` exists
to prevent.

Store flags: `--local` installs into `DIR/vendor/` instead of the per-user
store; `--store DIR` overrides the store root (default `$CPLUS_HOME`, else
`~/.cplus`); `--repo-url URL` pointed at a local path is the offline mode.

## 7. Introspection

```bash
cpc --tokens FILE            # the token stream
cpc --ast FILE               # the AST
cpc --emit-ll FILE           # cpc's own IR
cpc --emit-ll-opt FILE       # post-optimization IR (through clang)
cpc --emit-asm FILE          # native assembly
cpc --emit-obj FILE -o OUT.o # relocatable object
cpc --emit-ll-project        # merged IR for the whole project
cpc build --print-link-args  # what the dependencies add to the link line
cpc --realtime-report[=json] # whole-project real-time contract digest
```

Place `--target` and `--fp-contract` **before** an inline emit flag and its
file.

When chasing a suspected miscompile, the useful ladder is `--emit-ll` (is
cpc's IR right?) → `--emit-ll-opt` (did an optimizer pass change the
meaning?) → `--emit-asm`. Comparing `--release` against `--debug` at each
rung localizes an optimization-level bug quickly.

## 8. Gotchas

- **`cpc` on PATH may not be the one you are building.** In this repo,
  always run `./target/release/cpc` after `cargo build --release`; a
  package-manager-installed `cpc` will not have your changes and will fail
  in ways that look like language bugs.
- **`cpc test` with no file reads `Cplus.toml`.** Package suites are run
  from the package directory (`cd vendor/<pkg> && cpc test`), not from the
  repo root.
- **Warnings from dependencies are hidden by default** — `--warn-deps`
  when you are auditing rather than building.
- **`cpc query` rebuilds the graph every time.** Repeated questions want
  `cpc mcp`.
- **`cpc headers` reads `src/` only** and never `target/`, so a module that
  exists solely as a generated file in `target/` is not in the published
  surface.
