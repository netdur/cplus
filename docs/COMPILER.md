# C+ Compiler Internals

A compiler-internals reference for [`cplus-core`](../cplus-core/src/) (the library), [`cpc`](../cpc/src/) (the driver binary), and [`cpc-lsp`](../cpc-lsp/src/) (the language server). Audience: someone reading the source.

If you want to use the language, read [tutorial.md](../tutorial.md). If you want to extend it, read this file then `sema.rs` and `codegen.rs`.

---

## 1. Pipeline

```
source (.cplus)
   │
   ▼
┌─────────┐    Vec<Token>
│ lexer   │  ────────────────► tokens with span
└─────────┘
   │
   ▼
┌─────────┐    Program (AST)
│ parser  │  ────────────────► items, imports, types, exprs
└─────────┘
   │
   ▼
┌─────────┐    Vec<Diagnostic>
│ attrs   │  ────────────────► #[test] / #[repr(C)] / #[link_name] validation
└─────────┘
   │
   ▼
┌─────────┐    mutated Program
│ lower   │  ────────────────► `if let` / `guard let` → match;  const substitution
└─────────┘
   │
   ▼ (multi-file path only, via resolver)
┌──────────┐    LoadedProject
│ resolver │  ───────────────► entry + every imported file, merged into one Program
└──────────┘   with file_id-qualified item names
   │
   ▼
┌─────────┐    (Vec<Diagnostic>, MonoInfo)
│ sema    │  ────────────────► type-checks; records every generic instantiation;
└─────────┘                    resolves compile-time constants
   │
   ▼
┌──────────┐   Vec<Diagnostic>
│ borrowck │  ───────────────► flow-sensitive per-fn move/borrow analysis
└──────────┘
   │
   ▼
┌───────────────┐   Program (no generic templates)
│ monomorphize  │ ────────────► generic templates expanded; call sites
└───────────────┘                rewritten to mangled symbols
   │
   ▼
┌─────────┐    String of LLVM IR text
│ codegen │  ────────────────► emits .ll, including DWARF + TBAA + sanitizer hooks
└─────────┘
   │
   ▼ (cpc invokes clang)
LLVM .ll  ──►  clang  ──►  native binary
```

Each phase is total — it produces a value or a list of `Diagnostic`s. There is no in-band error propagation between phases; the driver decides whether to stop based on diagnostic severity.

### File map

| File | LOC | Phase |
|---|---|---|
| [`lexer.rs`](../cplus-core/src/lexer.rs) | 1.3k | Lex |
| [`parser.rs`](../cplus-core/src/parser.rs) | 4.7k | Parse |
| [`ast.rs`](../cplus-core/src/ast.rs) | 0.9k | AST types |
| [`attrs.rs`](../cplus-core/src/attrs.rs) | 0.8k | Attribute validation + test discovery |
| [`lower.rs`](../cplus-core/src/lower.rs) | 1.2k | AST → AST desugaring |
| [`resolver.rs`](../cplus-core/src/resolver.rs) | 2.7k | Multi-file project loading |
| [`sema.rs`](../cplus-core/src/sema.rs) | 14.4k | Name resolution + type checking + generic-instantiation collection |
| [`borrowck.rs`](../cplus-core/src/borrowck.rs) | 5.4k | Move/borrow analysis |
| [`monomorphize.rs`](../cplus-core/src/monomorphize.rs) | 3.0k | Generic-template expansion |
| [`codegen.rs`](../cplus-core/src/codegen.rs) | 16.6k | LLVM IR emission |
| [`manifest.rs`](../cplus-core/src/manifest.rs) | 1.0k | `Cplus.toml` parser |
| [`diagnostics.rs`](../cplus-core/src/diagnostics.rs) | 0.4k | Diagnostic shape + rendering helpers |
| [`fmt.rs`](../cplus-core/src/fmt.rs) | 0.6k | `cpc fmt` |
| [`docgen.rs`](../cplus-core/src/docgen.rs) | 0.5k | `cpc doc` |
| [`doctest.rs`](../cplus-core/src/doctest.rs) | 0.4k | Doctest extraction |
| [`atomic.rs`](../cplus-core/src/atomic.rs) | 0.2k | `__cplus_atomic_*` intrinsic name → LLVM atomic mapping |
| [`cpc/src/main.rs`](../cpc/src/main.rs) | 2.7k | CLI driver |
| [`cpc-lsp/src/main.rs`](../cpc-lsp/src/main.rs) | — | Language server (LSP over stdio) |

Total: ~54k LOC of Rust. About 30k is the sema + codegen pair; the rest is plumbing.

---

## 2. Driver flow

`cpc <subcommand> [args]` dispatches in [`cpc/src/main.rs`](../cpc/src/main.rs):

- `cpc FILE.cplus -o OUT` — single-file build. Reads source → `build_ir` (lex/parse/attrs/lower/sema/borrowck/mono/codegen) → writes `.ll` to a temp file → invokes `clang` to assemble + link.
- `cpc build` — project build. `manifest::load("Cplus.toml")` → `resolver::load_project_full` → same downstream phases → `clang` with the manifest's `frameworks` / `libs` / `[link].extra_objects` / dep `[link]` contributions.
- `cpc test [FILE]` — like build but uses `codegen::generate_test_binary` (synthesizes a `main` that walks every `#[test]` fn, tracks pass/fail counts, prints results). The driver runs the produced binary directly.
- `cpc check FILE` — runs up to borrowck inclusive, then stops. No codegen, no clang.
- `cpc fmt FILE` — `cplus_core::fmt::format_source(src)` then writes back (or with `--check`, diffs stderr and exits non-zero on drift).
- `cpc doc FILE` — extract `///`-doc'd `pub` items and render markdown to `target/doc/<name>.md`.
- `cpc lsp` — exec's the `cpc-lsp` binary (looked up next to `cpc` first, then on `PATH`).
- `cpc --emit-ll FILE` / `--emit-ll-opt` / `--emit-asm` — same pipeline but stops after IR (with optional `opt` pass) or assembly.

The driver never embeds LLVM as a library — it shells out to `clang` for assembly and linking. The IR-text-only contract means the build can be reproduced by hand: `cpc --emit-ll x.cplus > x.ll; clang x.ll -o x`.

---

## 3. Lexer

`fn tokenize(src: &str) -> Result<Vec<Token>, LexError>` ([`lexer.rs`](../cplus-core/src/lexer.rs)).

Hand-written lexer. Returns a `Vec<Token>` where each `Token` carries a `TokenKind` and a `Span { byte_start, byte_end }`. Comments and whitespace are dropped by default; `tokenize_with_trivia` keeps them (used by `fmt` and `docgen`).

Notable shapes:

- **Numeric literals** keep their suffix (`42i64`, `3.14f32`, `1.0e9`, `0xFFu8`, `0b1010`). Sema does the suffix → `Ty` mapping.
- **String literals** carry the decoded payload (escapes already processed: `\n`, `\t`, `\xHH` ASCII-only, `\u{...}` rejected to keep the payload UTF-8). The lexer reports `LexError` on malformed escapes.
- **Char literals** (`'a'`, `'\n'`, `'\x1b'`) lex to a numeric token tagged as a char so sema can require an integer type.
- **Keywords** are matched exhaustively against an enum. `borrow`, `move`, `mut`, `restrict`, `unsafe`, `defer`, `gen`, `async`, `await` are all reserved tokens — the parser decides where each is legal.

Spans are byte offsets into the original source. The `LineMap` helper in `diagnostics.rs` maps a byte offset to (line, column) for error rendering.

---

## 4. Parser

`fn parse(tokens: Vec<Token>) -> Result<Program, ParseError>` ([`parser.rs`](../cplus-core/src/parser.rs)).

Recursive descent. One-token lookahead with occasional two-token disambiguation (e.g. `Type::method` vs `mod::name`). On the first error the parser returns — no recovery, no partial parse. (Acceptable because incremental editor support runs through the LSP, which calls `parse` repeatedly on snapshots.)

### AST shape

The AST lives in [`ast.rs`](../cplus-core/src/ast.rs). The two top-level types:

```rust
pub struct Program {
    pub items: Vec<Item>,
    pub imports: Vec<Import>,
}

pub enum ItemKind {
    Function(Function),
    Struct(StructDef),
    Enum(EnumDef),
    Impl(ImplBlock),
    Interface(InterfaceDef),
    TypeAlias(TypeAliasDef),
    Const(ConstDef),
    Static(StaticDef),
    Use(UseDecl),
}
```

`Function`, `StructDef`, etc. each carry a `generic_params: Vec<GenericParam>` (with optional bounds), a body, and an `origin_file: FileId` (set by the resolver in multi-file mode, blank in single-file).

`ExprKind` and `StmtKind` are the recursive payloads. Notable variants:
- `ExprKind::IfLet { pattern, scrutinee, then, else_ }` and `ExprKind::GuardLet { ... }` — desugared by `lower.rs` to `Match` before sema sees them.
- `ExprKind::StructLit { name, generic_args, fields }` and `GenericStructLit` — the latter is the form with an explicit `Name[T]::{...}` turbofish.
- `Unsafe(Block)` — wraps everything that touches raw pointers or extern fns.
- `Async(Block)` / `Await(Box<Expr>)` — explicit-state-machine async (see §13).

### Disambiguation tricks

- **`name<...>`** could be a comparison or a turbofish. The parser tries turbofish first; if the closing `>` is followed by `(` or `::`, it commits. Otherwise it backtracks.
- **`a as b`** is a cast. `as` is a keyword; right-hand side must be a type. The parser doesn't allow nested casts without parens: `(a as i32) as i64`.
- **`{` after a condition** — `if x { ... }` is a block, not a struct literal. Struct literals after expressions in condition position need parens: `if (Point { x: 0, y: 0 }) == p`.

---

## 5. Attribute validation + test discovery

[`attrs.rs`](../cplus-core/src/attrs.rs).

Two entry points:

- `fn check(prog: &Program, file: PathBuf, src: &str) -> Vec<Diagnostic>` — validates every attribute against a fixed table (kind, target kind, accepted args). Errors:
  - E0354 unknown attribute
  - E0355 duplicate
  - E0356 wrong target (e.g. `#[test]` on a struct)
  - E0357 missing required arg
  - E0358 wrong arg shape
  - E0359 extra args
  - E0360 conflicting attributes
- `fn discover_tests(prog: &Program) -> Vec<TestFn>` — walks every `#[test]` fn and returns `TestFn { qualified_name, display_name, origin_file, returns_i32, span }`. The test driver uses this to synthesize `main`.

The attribute table is the single source of truth. Adding a new attribute means:
1. Add to the table in `attrs.rs`.
2. If the attribute affects codegen (like `#[repr(C)]`, `#[link_name]`, `#[unroll(N)]`), add a reader in `codegen.rs`.
3. Add positive + negative tests in `cpc/tests/e2e.rs`.

---

## 6. Lowering

`fn lower(prog: &mut Program, file: &PathBuf, src: &str) -> Vec<Diagnostic>` ([`lower.rs`](../cplus-core/src/lower.rs)).

In-place AST rewrites that happen before sema so the rest of the compiler only deals with the canonical forms:

- **`if let PAT = EXPR { THEN } else { ELSE }`** → `match EXPR { PAT => THEN, _ => ELSE }`.
- **`guard let PAT = EXPR else { ELSE }; REST_OF_BLOCK`** → `match EXPR { PAT => { REST_OF_BLOCK }, _ => ELSE }`.
- **`const FOO: T = EXPR;`** at module scope is substituted at every use site (no runtime indirection). Mutable statics (`static mut FOO: T = EXPR;`) and immutable statics (`static FOO: T = EXPR;`) stay as items — codegen emits one LLVM global per static.
- **Implicit return checking** (E0333) — every non-`void` function must end with an explicit `return`. Lower validates this.

Lower emits diagnostics in the E0347–E0352 range for malformed `if let` / `guard let` patterns, plus E0X30 for const substitution rules.

---

## 7. Multi-file resolver

`fn load_project_full(entry: &Path, manifest_root: &Path, is_lib: bool, deps: Option<&[String]>) -> Result<LoadedProject, LoadFailure>` ([`resolver.rs`](../cplus-core/src/resolver.rs)).

Starts at the entry file, follows every `import "PATH" as ALIAS;` recursively. Three import forms:

- **`./foo`** or **`../foo`** — file-relative. Resolves against the importer's parent directory.
- **`<pkg>/<sub>`** where `<pkg>` is in `deps` — vendor import. Primary lookup: `<manifest_root>/vendor/<pkg>/src/<sub>.cplus`. **Fallback**: when running from inside a vendor package, sibling deps live at `<manifest_root>/../<pkg>/src/<sub>.cplus` — the resolver tries that path if the primary doesn't exist. This is what makes `cd vendor/uuid && cpc test` work without per-package symlinks (G-030, 2026-05-24).
- **`stdlib/<sub>`** — always treated as a vendor import; the driver adds `stdlib` to `deps` automatically for single-file builds.

Each loaded file gets a `FileId` (a short string derived from its path relative to `manifest_root`, with `/` → `.` and `.cplus` stripped). All items get rewritten with `origin_file = file_id` so sema can produce cross-file diagnostics with the right source map.

Returns a `LoadedProject { program, files, entry_file_id }` where `program.items` is the union across all files (with name collisions resolved by `file_id.item_name` mangling for non-`pub` items).

Security: vendor imports with `..` segments fire E0859; vendor imports of undeclared packages fire E0852.

---

## 8. Semantic analysis

`fn check_multi_with_mono(program: &Program, entry_file: PathBuf, entry_src: &str, files: BTreeMap<String, (PathBuf, String)>) -> (Vec<Diagnostic>, MonoInfo)` ([`sema.rs`](../cplus-core/src/sema.rs)).

This is the largest file in the compiler (~14k LOC). It does name resolution, type checking, generic instantiation collection, attribute-driven layout decisions, and a dozen other passes wrapped together.

### Type representation

```rust
pub enum Ty {
    I8 | I16 | I32 | I64 | I128 | ISize,
    U8 | U16 | U32 | U64 | U128 | USize,
    F32 | F64 | Bool,
    Str,
    Char,
    RawPtr(Box<Ty>),
    Slice(Box<Ty>),
    Array(Box<Ty>, usize),
    Struct(StructId),
    Enum(EnumId),
    Function(FnSig),
    Simd { elem: Box<Ty>, lanes: u32 },
    Mask { elem: Box<Ty>, lanes: u32 },   // distinct from Simd (shipped 2026-05-23)
    Param(String),                          // generic type parameter
    Future(Box<Ty>),
    Iterator(Box<Ty>),
    Error,                                  // sentinel for failed type inference
}
```

`StructId` and `EnumId` are indices into `TypeTables { struct_defs, enum_defs }`. Each `StructDef` records its fields, methods, generic params, and a `generic_origin: Option<(String, Vec<Ty>)>` pointing back to its template if it's a monomorphized instantiation.

Methods are stored per-target in `methods_per_struct` / `methods_per_enum`. For generic types, there's a parallel `generic_impl_methods` keyed by struct name — instantiation reads from this and re-substitutes the type parameters to build the concrete methods table.

### Pass structure

`check_multi_with_mono` orchestrates roughly 15 passes:

1. **Collect type declarations** — `StructDef` / `EnumDef` / `TypeAlias` registered without bodies.
2. **Resolve type aliases** transitively. Cycle detection here.
3. **Collect struct fields** with full type resolution. This is where field types may trigger generic-struct instantiation (the source of G-022 — see §15).
4. **Collect methods** — both inherent (`impl T { ... }`) and interface (`impl Interface for T { ... }`). Two-phase: register all generic-impl-method templates first, then resolve their signatures.
5. **Backfill generic struct methods** (G-022 fix) — re-runs the impl-template substitution for any struct whose `methods` table was populated empty by a field-driven instantiation that ran before `collect_methods`.
6. **Resolve function signatures** — param types, return type, generic bounds.
7. **Check function bodies** — the main type-checking pass. Each fn gets its own `FunctionChecker` with a stack of local scopes.
8. **Collect generic instantiations** as a side effect of body checking. Every `f::[T](...)` call site, every `Struct[T] { ... }` literal, every method call on a generic receiver records into `MonoInfo`.
9. **Compile-time blob resolution** — `include_bytes!("path")`, `include_str!("path")`, `env!("VAR")` all resolved at sema time. Paths checked, values read, errors → E0810/E0876.
10. **Static collection** — every `static FOO: T = EXPR;` gets a `StaticInfo` entry. Codegen reads from `MonoInfo.statics`.
11. **Attribute-driven follow-ups** — `#[repr(C)]` recorded on the struct; `#[link_name = "..."]` recorded on the extern fn; `#[align(N)]` (when present) recorded for codegen.

### MonoInfo

The handoff to monomorphize + codegen:

```rust
pub struct MonoInfo {
    pub instantiations: BTreeSet<(String, Vec<Ty>)>,
    pub call_monos: HashMap<ByteSpan, Vec<Ty>>,
    pub struct_instantiations: BTreeSet<(String, Vec<Ty>)>,
    pub enum_instantiations: BTreeSet<(String, Vec<Ty>)>,
    pub method_instantiations: BTreeSet<(String, String, Vec<Ty>)>,
    pub type_aliases: BTreeMap<String, Type>,
    pub compile_time_blobs: HashMap<Span, CompileTimeBlobEntry>,
    pub env_vars: HashMap<Span, EnvVarEntry>,
    pub statics: BTreeMap<String, StaticInfo>,
}
```

The driver passes this to `codegen::generate_with_mono` and `monomorphize::monomorphize`.

### Error codes

Sema emits ~77 unique `Exxxx` codes. The main ranges:

| Range | Theme |
|---|---|
| E0300–E0346 | Core name / type / call / pattern errors |
| E0347–E0352 | `if let` / `guard let` shape errors |
| E0354–E0360 | Attribute validation |
| E0370–E0399 | Borrow checker (emitted by `borrowck.rs`) |
| E0407–E0411 | Project / library targets, `restrict` |
| E0500–E0509 | Generics + interfaces |
| E0801–E0810 | Unsafe / compile-time intrinsics |
| E0821–E0876 | Function pointers, statics, env vars |
| E0852–E0864 | Vendor packages + `[link]` validation |
| E0900 | Borrow-shaped params in async fns |

Each code lives in the file that emits it. There is no central error registry — the codes are documented in [tutorial.md §29](../tutorial.md#29-common-error-codes) and the error message itself.

---

## 9. Borrow checker

`fn check(prog: &Program, file: &PathBuf, src: &str) -> Vec<Diagnostic>` ([`borrowck.rs`](../cplus-core/src/borrowck.rs)).

**Flow-sensitive, per-function, lexical-scope based.** No region inference, no constraint solver, no lifetime variables. The model is closer to a forward dataflow analysis over the AST than to Rust's NLL.

### Place tracking

Every storage location the analyzer tracks is a `Place` — a path like `x.field.subfield[i]`. State is one of:

```rust
enum PlaceState {
    Owned,
    Moved,
    BorrowedShared(usize),    // refcount of outstanding shared borrows
    BorrowedExclusive,
    MaybePartial,             // joined from divergent control flow
    Uninit,                   // pre-assignment
}
```

`MaybePartial` exists for branch joins: after `if cond { move x } else { /* keep */ }`, `x` is partially-moved. Reading `x` is rejected; assigning to `x` is allowed.

### Per-function analysis

For each function body:

1. Initialize state: every param is `Owned` (or `Moved` if the caller marked it for ownership transfer — but that's call-site, not analyzed here).
2. Walk statements in order, updating per-`Place` state.
3. At `if` / `match`: snapshot state, analyze each arm, then **merge by intersection** — a place is `Owned` post-join only if every arm left it `Owned`; otherwise it becomes `MaybePartial` or `Moved`.
4. At `while` / `loop`: analyze body once; on the back-edge, intersect with the entry state. If they differ, re-run (one fixpoint iteration is usually enough since the state lattice is shallow).
5. At function exit: check that no `BorrowedExclusive` / `BorrowedShared(>0)` escapes. (Borrows in C+ today are restricted to function arguments, so this rarely fires.)

### What it catches

- E0370 use-after-move (`let y = move x; let z = x;`)
- E0371 double-move (`let y = move x; let z = move x;`)
- E0372 move from borrowed (`borrow x; let y = move x;`)
- E0373 mutate during borrow
- E0374 conflicting borrows
- E0334 mutually-exclusive markers (`mut` + `move` on same param)

### What it deliberately doesn't do

- No region inference — every borrow's scope is determined lexically. The borrow ends at the end of the enclosing block.
- No two-phase borrows. `f(&mut x, x.field)` either rejects or accepts based on argument order — no clever reordering.
- No reborrows. A `*p` access in a function that took `borrow p: T` is a read of `p`, not a borrow.

This keeps the implementation manageable (5.4k LOC vs Rust's MIR borrowck) at the cost of some valid programs being rejected. Most rejected programs have a straightforward rewrite using `move` semantics.

---

## 10. Monomorphization

`fn monomorphize(program: Program, mono: &MonoInfo, type_name_of: &dyn Fn(&Ty) -> String) -> Program` ([`monomorphize.rs`](../cplus-core/src/monomorphize.rs)).

Input: the type-checked `Program` (still containing generic templates) + the `MonoInfo` collected by sema. Output: a `Program` where every generic template has been replaced with concrete per-instantiation functions / structs / enums, and every call site rewritten to the mangled symbol.

### Name mangling

Generic instantiations get mangled names so LLVM sees them as distinct symbols:

```
fn  identity[T](x: T) -> T          →    identity__i32   (instantiation [i32])
struct Box[T]                        →    Box__vec__Vec__i32 (instantiation [Vec[i32]])
enum  Option[T]                      →    Option__i32
impl  Vec[T] { fn push(...) }        →    Vec__i32.push
```

Mangling is recursive: `Vec[Vec[i32]]` mangles to `Vec__Vec__i32`. The `type_name_of` closure handles the per-call mapping from `Ty` to its mangled string — this is built up by the driver because sema doesn't track string-form names for all instantiations directly.

### Walk + rewrite

The pass is a two-step rewrite:

1. **Synthesize concrete items.** For each `(template_name, type_args)` in `MonoInfo.instantiations` / `struct_instantiations` / `enum_instantiations`, clone the template, substitute every `Ty::Param(name)` with the corresponding concrete type, and append to `program.items` under the mangled name.
2. **Rewrite call sites.** Walk every `ExprKind::Call`, `ExprKind::MethodCall`, `ExprKind::StructLit`, etc. and replace generic invocations with the mangled name. This is where G-026 lived: `ItemKind::Enum` variant payloads also need walking to rewrite generic types in `Array(Vec[Value])`-shaped variants.

After monomorphize, the program has no `Ty::Param` anywhere. Codegen can assume every type is concrete.

### Why monomorphize and not boxing?

C+'s tag is "Rust without exceptions and closures." Monomorphization gives the predictable performance + zero-cost abstractions story. The cost is bigger binaries when a generic is instantiated with many types — acceptable for a systems language.

---

## 11. Codegen

`fn generate(program: &Program, mode: BuildMode) -> String` plus variants ([`codegen.rs`](../cplus-core/src/codegen.rs)).

The largest file in the compiler at 16.6k LOC. Emits **text LLVM IR** directly — no `inkwell` or LLVM C++ bindings. The output is a `String` that can be written to a `.ll` file and passed to `clang -x ir`.

### Why text IR?

- One less build dependency (no LLVM library linkage).
- Trivially reproducible: `cpc --emit-ll x.cplus > x.ll && diff` produces a stable text artifact.
- Debuggable: paste IR into Godbolt, run `opt -S` by hand, etc.
- Forces a clean separation: codegen doesn't reach into LLVM internals.

The cost is some redundant string formatting and a less helpful error story when malformed IR slips through. We rely on `cargo test` running `clang` over every fixture to catch invalid IR.

### Variants

```rust
pub fn generate(program, mode) -> String;
pub fn generate_with_mono(program, mode, debug_source, sanitizers, is_lib, mono) -> String;
pub fn generate_lib(program, mode) -> String;
pub fn generate_test_binary(program, mode, tests, json, mono) -> String;
pub fn generate_with_debug(program, mode, source_file) -> String;
pub fn generate_with_options(program, mode, source_file, sanitizers) -> String;
```

All route through a single private `generate_inner` taking the union of options. The variants are convenience entry points for each driver subcommand.

### Memory model

Local variables use `alloca` + `load` / `store`. Every binding gets a stack slot, and reads/writes go through `load i32, ptr %slot` etc. LLVM's `mem2reg` pass promotes these to SSA registers in `-O2`; the `-O0` path keeps them as stack slots, which gives lldb predictable variable inspection.

### Calling conventions

- **`main`** keeps C calling convention so the OS runtime can invoke it.
- **`extern fn`** declarations use C cc by default. `#[link_name = "X"]` aliases a C+ name to a foreign symbol.
- **User-defined non-`pub` non-`extern` functions whose address is not taken** get `fastcc`. This is a v0.0.8 optimization: about 30% of functions in typical projects qualify, and `fastcc` lets LLVM use a more efficient register convention internally. Drop methods stay on `preserve_nonecc` because `fastcc` can't compose with it.
- **Return values larger than 2× pointer-size** use `sret` (struct-return) — the caller allocates the destination slot, passes its address as the first arg, and the callee writes through that pointer. Avoids a redundant memcpy.
- **`musttail`** is emitted for direct tail calls that match the caller's calling convention exactly. v0.0.8 fixed a bug where the matching check missed some valid sites.

### Drop tracking

Every binding of a non-Copy type gets a **drop flag** — an `i1` alloca initialized to `true`. At each scope exit point, the codegen emits:

```llvm
%flag = load i1, ptr %x.drop_flag
br i1 %flag, label %do_drop, label %skip
do_drop:
  call void @Type.drop(ptr %x.slot)
  br label %skip
skip:
```

A `move x` transfers ownership: codegen flips `%x.drop_flag` to `false` so the scope-exit check skips. Sites that need to do this:

1. `move`-flagged argument at a call site.
2. `return EXPR` where `EXPR` is a bare Ident.
3. `let y = x` (Copy types: bitwise copy; non-Copy: ownership transfer).
4. `struct_field: bare_ident` in a struct literal — **G-023, 2026-05-23**: this was missing for non-Copy field inits.
5. `*p = bare_ident` raw-pointer store after a `move` param — also G-023.
6. Method-call args at positions the callee marked `move` — **G-027, 2026-05-23**: the scanner that pre-registers move sources was walking free-fn args but not method-call args.

The `mark_moved` helper is a no-op when the source binding has no drop flag (Copy types, non-move sources), so it's safe to call unconditionally on bare Idents.

### TBAA + alias metadata

Codegen emits `!tbaa` metadata on every typed load/store and `!alias.scope` / `!noalias` for `restrict` parameters. This lets LLVM's alias analysis hoist loop-invariant loads through pointer stores it would otherwise treat as possible aliases.

The TBAA tree today is coarse — one root, one per scalar type. Per-field TBAA (where `struct.a` and `struct.b` get distinct nodes) is a documented-but-deferred optimization; the win didn't show up on the raytracer benchmark that motivated v0.0.8.

### Compile-time intrinsics

- `addr_of(x)` — returns `*T` pointing at `x`'s alloca slot. Zero-instruction codegen: the alloca pointer is the address.
- `size_of::[T]()` / `align_of::[T]()` — folded to integer literals at codegen time.
- `include_bytes!("path")` / `include_str!("path")` — sema reads the file and stashes the bytes in `MonoInfo.compile_time_blobs`; codegen emits one `@.bytes.N = private constant [N x i8]` global per unique path and resolves the call site to that global's address.
- `env!("VAR")` — same shape as `include_str!`, with the env var read at sema time.

### Static items

`emit_statics` runs near the top of codegen output. For `static FOO: T = EXPR;` it emits `@FOO = constant <ty> <lit>` (immutable, lives in `.rodata`); for `static mut`, `@FOO = global <ty> <lit>` (in `.data`). The `Ty::Str` case emits a paired `@FOO.data` byte array + a `@FOO` `{ptr, i64}` fat-pointer global pointing at it.

Reads of static names route through `gen_place`'s `md.statics` lookup. G-028 (fixed 2026-05-24) was that `generate_test_binary` was constructing the codegen with `&Default::default()` for the statics map, so vendor-package tests using `static` lookups would crash.

### Test driver synthesis

`generate_test_binary` emits a synthetic `main` that:

1. Zeros a shared `@cpc_test_failed` flag.
2. For each `#[test]` fn, prints `test NAME ... ` then calls it.
3. If the test returned `i32`, OR the result into the failure flag; if it had no return, just check the flag.
4. Print `ok` or `FAILED` based on the flag, increment pass/fail counters.
5. After all tests, print `test result: <ok|FAILED>. N passed; M failed`.
6. Return the failure count (clamped to `[0, 255]` so the OS exit code distinguishes "all passed" from "some failed").

Each `assert` statement throughout the project lowers to a write of `@cpc_test_failed` instead of `llvm.trap`, so a failed assertion sets the flag, the test function falls through, and the driver reads the flag after the call.

---

## 12. Async / iterators / SIMD

### Async

C+'s async is **explicit state machines**, not green threads or stackful coroutines. An `async fn f() -> T` is sugar for a function returning `Future[T]`. The compiler lowers the body into a state machine struct with one variant per `await` point, plus a `poll` method.

The `executor` (in stdlib) is the user's responsibility — they pick a runtime: single-thread reactor, thread pool, etc. The compiler doesn't bundle one.

### Iterators

`gen fn` is sugar for a function returning `Iterator[T]`. Lowering is similar to async: state machine struct + a `next` method. `for x in iter { ... }` calls `iter.next()` in a loop and pattern-matches on the result.

The recent G-026 codegen fix was specifically about iterator-of-recursive-type mangling: `Iterator[Value]` where `Value` is a generic enum needed careful name handling in the `unwrap_iterator_ty` helper.

### SIMD

`Ty::Simd { elem, lanes }` lowers to LLVM's `<N x T>` vector types. `Ty::Mask` (shipped 2026-05-23) is a distinct variant that uses the same `<N x iN>` representation for ABI compatibility but enforces type-level distinction: arithmetic on masks is rejected (E0324), comparison ops produce masks not values, `mask.to_bits() / simd.to_mask()` are no-op conversions.

The motivating principle: a SIMD comparison should produce a value that can only be passed to `select` / `any` / `all`, not added to another SIMD vector. Without `Ty::Mask`, you couldn't enforce this at the type level — Rust handles it through wrapper types, but C+ chose the per-Ty approach because we already have a discriminated `Ty` enum.

---

## 13. Multi-file builds: manifest

[`manifest.rs`](../cplus-core/src/manifest.rs).

```rust
pub struct Manifest {
    pub package: Package,
    pub bins: Vec<BinTarget>,
    pub lib: Option<LibTarget>,
    pub link: Option<LinkSpec>,
    pub dependencies: Vec<Dependency>,
    pub root: PathBuf,
}
```

`fn load(manifest_path: &Path) -> Result<Manifest, ManifestError>` parses `Cplus.toml` (using `toml-rs`) and applies defaults:

- If no `[[bin]]` and no `[lib]` declared → auto-inject a phantom `[[bin]]` with `name = package.name, path = "src/main.cplus"`.
- `[lib]` and `[[bin]]` are mutually exclusive (E0408).
- `[link].bundled` requires `[link].triples`; both validated against the filesystem (`src/lib/<triple>/<basename>`).

The `cpc test` driver has a fallback for the phantom-bin case: when `src/main.cplus` doesn't exist, try `src/<package-name>.cplus` as the entry. This is what makes vendor packages (which declare no target) self-testable.

### Cross-package validation

`collect_dep_link_args` in `cpc/src/main.rs` walks every dependency, loads its manifest, and validates the manifest-is-truth contract:

- E0854 — `[dependencies] x = "*"` declared but `vendor/x/Cplus.toml` missing.
- E0855 — package name mismatch (`vendor/x/`'s manifest declares `name = "y"`).
- E0860 — `[link].bundled` declared but file missing.
- E0861 — orphan binary file in `src/lib/<triple>/` not declared in `[link].bundled`.
- E0862 — host triple not supported by package's `[link].triples`.
- E0863 — `[link].bundled` declared without `[link].triples`.
- E0864 — `[link].extra-objects` entry doesn't exist on disk.

Vendor packages can declare their `[link]` frameworks/libs at top level; consumers don't need to re-state them. The `cpc test` driver picks these up too — G-029 fix.

---

## 14. Diagnostics + LineMap

[`diagnostics.rs`](../cplus-core/src/diagnostics.rs).

```rust
pub struct Diagnostic {
    pub severity: Severity,        // Error | Warning | Note
    pub code: DiagCode,             // e.g. DiagCode("E0335")
    pub message: String,
    pub primary: SourceSpan,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub suggestions: Vec<Suggestion>,
}
```

`LineMap::new(src)` indexes the source's newline positions so any byte offset can be mapped to (line, column). Diagnostic rendering happens in the driver, not in `cplus-core` — there are three modes:

- `--diagnostics=human` (default) — colored, multi-line with carets and labels.
- `--diagnostics=short` — one line per diagnostic, no labels. Used by editors that have their own UI.
- `--diagnostics=json` — structured JSON, one diagnostic per line. Used by `cpc-lsp` and external tools.

The driver routes every diagnostic through `emit_diag(diag, mode, src)` which picks the renderer based on `mode`.

---

## 15. Worked example: the G-026 fix (recursive enum payload)

This is a good case study because it touched three phases. The bug: `Value::Array(vec::Vec[Value])` in the typed json refactor fired `E0303: unknown type <concrete>`.

**Sema layer (`sema.rs`)** — `substitute_param_in_type_ast` was emitting `<concrete>` as a placeholder string when substituting a generic-param type into an AST `Type` node. The placeholder was meant to be replaced later but the replacement site wasn't aware of struct/enum names from the type tables. **Fix**: added `substitute_param_in_type_ast_with_tables` that takes a closure mapping `Ty → name` and uses the real name everywhere a placeholder would have gone.

**Monomorphize layer (`monomorphize.rs`)** — `rewrite_item_calls` walks `ItemKind::Function` and `ItemKind::Impl` bodies, rewriting generic call sites to mangled names. But it didn't have an arm for `ItemKind::Enum`, so a variant payload like `Array(Vec[Value])` kept the generic `Vec[Value]` shape after mono. **Fix**: added `ItemKind::Enum` arm that walks every variant's payload `TypeKind` and rewrites `TypeKind::Generic` to mangled `TypeKind::Path`.

**Codegen layer (`codegen.rs`)** — `unwrap_iterator_ty` was using `rsplit_once('.')` to find the `Iterator__T` substring in a mangled name. But for `Iterator__src.main.Value` (where the inner T's mangled name contains dots from file-qualification), the split would land in the wrong place. **Fix**: use `rfind("Iterator__")` to find the marker, then take everything after the prefix.

After all three: the typed json refactor (994 LOC) shipped with 23 in-package `#[test]` fns, ASan-clean. The principle: a "single bug" that crosses phase boundaries needs a fix in every phase. Look for the symptom at each phase before declaring it solved.

---

## 16. Where to extend

| Task | Files to touch | Notes |
|---|---|---|
| Add a new attribute | `attrs.rs`, possibly `codegen.rs`, `cpc/tests/e2e.rs` | Attribute table is the single source. Read §5. |
| Add a new error code | The file that emits it. Tutorial table (E0xxx). | No central registry. Make the message specific. |
| Add a new builtin type | `ast.rs` (`Ty` variant), `sema.rs` (resolution + ops), `codegen.rs` (lowering), `monomorphize.rs` (mangling). | Mask was the most recent — search for `Ty::Mask` to see every site touched. |
| Add a new compile-time intrinsic | `sema.rs` (`check_named_call`), `codegen.rs` (lowering), `MonoInfo` field if it carries data. | `addr_of` is the smallest example. |
| Add a new generic stdlib type | `vendor/stdlib/src/<name>.cplus`, plus a smoke test under `docs/examples/projects/`. | No compiler changes if the type uses existing primitives. |
| Add a new vendor package | `vendor/<pkg>/Cplus.toml` + `src/<pkg>.cplus`. Add `#[test]` fns; run `cd vendor/<pkg> && cpc test`. | Sibling deps resolve via the parent-fallback (§7). |
| Track down a codegen bug | Reproduce with `cpc --emit-ll FILE.cplus`. Paste IR into a `clang -x ir -` invocation. Use `opt -S -passes='...'` to bisect optimization passes. | The IR-text contract is the debugging surface. |

---

## 17. Testing strategy

Three layers:

- **Unit tests** colocated in each `cplus-core/src/*.rs` file (`#[cfg(test)] mod tests { ... }`). 1035 of these. Cover sema rules, parser edge cases, codegen helpers.
- **End-to-end tests** in [`cpc/tests/e2e.rs`](../cpc/tests/e2e.rs). 399 of these. Each invokes the `cpc` binary on a fixture, asserts on diagnostics or runs the produced binary and checks output.
- **In-package vendor tests** — 66 `#[test]` fns across `vendor/{arena, clap, json, log, metal, uuid}`. Run via `cd vendor/<pkg> && cpc test`.

Every new feature ships with at least:
1. Positive — program compiles and runs.
2. Negative-with-code — program rejects with a specific Exxxx.
3. End-to-end — `cpc build` exercised start to finish.

When a compiler bug is fixed, add an `e2e.rs` regression test. Examples: `g023_struct_literal_field_init_does_not_double_drop`, `g023_raw_pointer_store_does_not_double_drop`, `addr_of_round_trips_via_time`, `mask_to_bits_no_op`.

The `fmt_check_all_samples_clean` test walks `docs/examples/` with `cpc fmt --check`. Any file that drifts from canonical formatting fails it. Symlinks that create directory cycles (like the failed-attempt vendor symlinks from this session) also break this test, which is how G-030 was caught.

---

## 18. The LSP

[`cpc-lsp/src/main.rs`](../cpc-lsp/src/main.rs).

Speaks LSP over stdio. The implementation reuses `cplus-core` directly: every `textDocument/didChange` re-runs the pipeline through borrowck (no codegen) and reports diagnostics. Hover / goto-definition / find-references use sema's symbol tables.

There's no incremental sema yet — every change triggers a full re-check of the file. Files are kept small enough in practice that this is fine; if it becomes a problem, sema would need a query-system pass.

---

## 19. Design principles

These are the rules the language commits to. Implementation decisions defer to these when they conflict.

1. **No null in safe code.** Raw pointer null (`0 as *T`) exists only in `unsafe`. Safe code uses `Option[T]`.
2. **No closures.** Function pointers + explicit captures via struct fields. The runtime stays simple; codegen has no closure-conversion pass.
3. **No exceptions.** Errors are `Result[T, E]`. The codegen has no landingpads, no unwind tables, no try/catch.
4. **No macros.** Compile-time intrinsics (`include_bytes!`, `env!`, `addr_of`) are recognized by name in sema; users can't define their own. Keeps the surface tractable.
5. **Function over syntax.** When there are two reasonable ways to write something, the language picks one. (`Phase 9` proposal — a TS-flavored syntax sugar pass — was rejected 2026-05-13 on this principle.)
6. **Manifest is the single source of truth.** The build driver refuses to link anything the manifest doesn't declare. E0860–E0864 enforce this.
7. **Text IR + clang.** Codegen produces a `.ll` file; clang assembles and links. No LLVM library dependency.

When extending the compiler, check the proposed feature against these. A feature that requires a closure conversion pass, an exception runtime, or a hidden allocator is wrong for C+ regardless of how convenient it is at the source level.
