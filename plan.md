# C+ — Plan

A systems language in the same domain as C — close to the metal, no runtime — with the well-known C footguns removed and Rust-level memory safety. LLVM backend.

C+ is **AI-native**: tooling and structured information are first-class from the early phases, not a Phase-9 afterthought. Compiler-as-library, structured diagnostics, deterministic builds, a single canonical formatter, a built-in test runner, and an LSP all land alongside the language itself — so AI agents (and humans using AI tools) get maximum signal from day one. See §5.

---

## 1. Foundational decisions (settled)

### 1.1 C compatibility

**ABI-compatible only.** C+ emits standard object files; the system linker stitches them together with C-compiled objects. C source is never compiled by `cpc`.

The only language-level interop primitive is `extern fn`:

```cp
extern fn printf(fmt: *u8, ...) -> i32;
extern fn malloc(size: usize) -> *u8;
```

Header consumption (libclang-based bindgen) is a **separate future tool** (`cpc-bindgen`), not a language feature. It emits `.cp` files containing `extern fn` declarations that get committed to the consumer's repo. Same shape as Dart `ffigen`, Rust `bindgen`, Swift's clang importer-as-tool. Not on any phase plan; built when hand-writing bindings becomes painful, possibly never.

**Not** source-compatible. Existing `.c` files do not compile.

### 1.2 Other foundations

- **Compiler implementation language: Rust.** Good LLVM crates, modern tooling, already installed.
- **Comptime: out.** No comptime evaluator. `const fn`-style bounded compile-time evaluation, generics via type params + interfaces (monomorphization), and `print` as a compiler intrinsic. Same shape as Rust minus macros. No comptime expansion of language semantics; spec stays small.
- **Ownership tracking: full borrow checker, phased in.** Move semantics in Phase 3, immutable borrows + lifetime inference in Phase 5, mutable borrows + aliasing-XOR-mutability rule + explicit lifetime annotations in Phase 6. Delivers Rust-level memory safety: use-after-free, double-free, data races, iterator invalidation all caught at compile time. Borrow syntax (`&T`, `&mut T`) is reserved in the grammar from Phase 1 even where the checker isn't running yet, to avoid migration pain later.
- **Release-mode overflow: wrap.** Debug traps, release wraps (Rust/Go-style). Plain `+ - *` are checked in debug, modular in release. `+% -% *%` operators wrap regardless of build mode (documents intent). Division by zero traps in both modes (always).
- **Memory model: C11 + borrow checker.** Atomics are library types `Atomic[T]` with explicit memory-ordering parameters (`Relaxed`, `Acquire`, `Release`, `AcqRel`, `SeqCst`), backed by compiler intrinsics. Data races on non-atomic memory are compile errors via the aliasing-XOR-mutability rule; the C11 "data race is UB" clause is structurally unreachable in safe C+. `unsafe` blocks can construct races, in which case C11 UB applies. Volatile access via `read_volatile` / `write_volatile` functions, not a type qualifier.

---

## 2. Settled design

### 2.1 Types

- Fixed-width integers only: `i8 i16 i32 i64 u8 u16 u32 u64 isize usize`. No `int`, no `long`.
- Floats: `f32`, `f64`.
- Real `bool`. No implicit int conversion.
- Real `enum`. No implicit int conversion.
- Tagged unions / sum types as a distinct construct, with pattern matching and exhaustiveness checking.
- Slices `T[]` are fat pointers (`ptr + len`). Indexing is bounds-checked; raw pointer escape hatch available.
- Strings: length-prefixed UTF-8 `string` type. Separate `cstring` for C interop.
- Pointers non-null by default. Nullable opt-in: `?*T`. Forced narrowing at use site.
- Explicit casts only: `as` or `@cast`. Implicit narrowing forbidden. Mixed signed/unsigned comparison forbidden or warned.

### 2.2 Memory and resources

- Manual memory management. No GC.
- Allocator passed as a parameter wherever allocation happens (Zig pattern).
- `defer` for scope-bound cleanup. Runs on scope exit including early returns.
- Definite assignment required — reading an uninitialized variable is a compile error.

### 2.3 Arithmetic

- `+ - * /` trap on overflow in debug; wrap (modular) in release.
- `+% -% *%` wrap regardless of build mode. Use to document intent and to get predictable wrapping in debug too.
- Division by zero traps in all build modes.

### 2.4 Errors

- Error unions: `!T` or equivalent.
- `try` operator for propagation.
- No exceptions.
- Errors are values, not control flow.

### 2.5 Modules

- Real module system. No preprocessor.
- One file = one module (working assumption).
- Explicit imports. No transitive imports leaking.
- Declarations and definitions live together. No headers.

### 2.6 Standard library

**Out of scope for this plan.** A stdlib is necessary for real programs but is a separate follow-on project, written *in* C+ once the language compiles. This plan covers only the language and its compiler.

The one exception is `println(n: i32)` in Phase 1 — that's a *compiler intrinsic* (the codegen emits a direct `printf` call), not a stdlib function. The eventual proper formatted-print facility will replace it.

### 2.7 Removed from C

- Preprocessor (no `#include`, `#define`, `#ifdef`)
- `gets`, `strcpy`, `sprintf`, other unbounded variants
- Implicit `int`
- K&R syntax
- Trigraphs and digraphs
- Comma operator (the expression-level form `a, b`). The `,` separator in argument lists, struct literals, and `for` headers stays.
- Variable-length arrays (probably)
- Implicit array-to-pointer decay

### 2.8a Style rules (locked in)

These are syntactic preferences settled after Phase 2. They apply to all C+ code in samples, tests, and future features.

- **Explicit `return` at function-body level.** No implicit tail-expression return — `fn f() -> i32 { 0 }` is rejected (E0333). `fn f() -> i32 { return 0; }` is correct. Block expressions still work in `let` initializers, assignments, and return-expression positions: `let x: i32 = if c { 1 } else { 2 };` is fine; `return if c { 1 } else { 2 };` is fine. Only the function-body-tail is forbidden.
- **Precise function names.** No "abbreviated semantics" where a name lies. `magnitude` means `sqrt(x²+y²)`; the squared form is `magnitude_squared`. Optimize for AI/LLM readability: the name tells the truth.
- **No `&` syntax. Values + `mut` markers.** Method receivers are `self` (read-only) or `mut self` (mutable). Both lower to a `ptr` parameter at the LLVM level; user never writes `&`. The borrow checker (Phase 5/6) will enforce aliasing-XOR-mutability around the `mut` keyword, not lifetime-typed references. Non-receiver parameter mutability (`mut x: T`) is deferred to Phase 3+.
- **`::` for type/namespace, `.` for instance.** Strict separation. `Color::Red`, `Point::new(3, 4)` use `::`. Field access `p.x` and method calls `p.method()` use `.`. Never mix.

### 2.8 Won't add (deliberate non-features)

These are popular in modern languages but are out of scope for C+. AI-friendliness, locality of reasoning, and small spec are the rationale.

- **No function overloading.** A function with a given name has exactly one signature. AI sees `frobnicate(x, y)` and the function is the function — no overload resolution.
- **No operator overloading.** `+ - * /` work only on built-in numeric types. User types use named methods (`v.add(w)`). Smallest language; no surprise dispatch.
- **No `any`-type or dynamic-typing escape hatch.** The type system is load-bearing. AI-written TypeScript is plagued by stale `any`/`as`; C+ refuses to admit the failure mode.
- **No implicit type conversions.** Already covered in §2.1 — explicit `as` casts only.
- **No macros, no decorators, no compile-time AST transformation.** Already covered by no-preprocessor + no-comptime.
- **No glob imports as a default.** Each `use` names what it brings in; project linting can ban glob imports entirely. (Detail in Phase 4 module design.)
- **No inheritance.** Composition + interfaces only. (Detail in Phase 7.)

---

## 3. Phased implementation

Each phase ends with a working compiler that handles strictly more programs than the previous one. Do not polish earlier phases at the expense of moving forward.

### Phase 0 — Repo and toolchain · 1–2 weeks · ✅ done

- Compiler implementation language: Rust.
- Cargo workspace; later split into `cplus-core` library + `cpc` binary (per §5.1).
- Hand-written LLVM IR ([cpc/src/hello.ll](cpc/src/hello.ll)) that prints `hello, world` via `printf`. Driver writes IR to a temp file and invokes `clang` for assemble+link.
- E2E test harness in place ([cpc/tests/e2e.rs](cpc/tests/e2e.rs)): 4 tests, all passing.

**Exit met:** `cargo test` passes; hand-written IR produces a runnable binary.

### Phase 1 — Tracer bullet · 3–6 weeks · ✅ done

Minimum viable end-to-end pipeline. Grammar fully specified in [docs/design/phase1-grammar.md](docs/design/phase1-grammar.md).

Language subset:
- Function definitions
- `i32` only (plus `bool` for conditions)
- Locals with type inference (`let` / `let mut`)
- Arithmetic on `i32` with overflow trapping in debug, wrap in release; division-by-zero traps in both modes
- `if`/`else` (statement and expression), `while`
- C-style and range-based `for`
- `return`, function calls, short-circuiting `&&` / `||`
- Compiler intrinsic `println(n: i32)` lowered to `printf("%d\n", n)`

Compiler components — all green:
- [x] **Library/binary split** ([cplus-core/](cplus-core/), [cpc/](cpc/)) per §5.1
- [x] Lexer + spans + sample programs ([cplus-core/src/lexer.rs](cplus-core/src/lexer.rs), [docs/examples/](docs/examples/)) — 14 unit tests
- [x] Recursive-descent parser → AST ([cplus-core/src/parser.rs](cplus-core/src/parser.rs), [cplus-core/src/ast.rs](cplus-core/src/ast.rs)) — 13 unit tests
- [x] **Structured diagnostics infrastructure** ([cplus-core/src/diagnostics.rs](cplus-core/src/diagnostics.rs), [docs/design/diagnostics.md](docs/design/diagnostics.md)) — 8 unit tests; `cpc --diagnostics=json|short|human` wired through CLI
- [x] **Name resolution + type checker** ([cplus-core/src/sema.rs](cplus-core/src/sema.rs)) — 14 error codes E0300–E0313, 29 unit tests covering every design-note §7.2 rejection plus happy-path positives
- [x] **AST → LLVM IR codegen** ([cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)) — `alloca`+`mem2reg` strategy; debug-mode overflow trapping via `llvm.{sadd,ssub,smul}.with.overflow.i32` + `llvm.trap`; always-on division-by-zero check; 19 unit tests
- [x] **Driver: `cpc FILE [-o OUT]`** ([cpc/src/main.rs](cpc/src/main.rs)) — full pipeline lex → parse → sema → codegen → temp `.ll` → invoke clang → binary. `cpc --emit-ll FILE` for IR inspection. `cpc --release` / `--debug` (default) selects build mode. 13 e2e tests including all four sample programs running end-to-end, runtime overflow trap verification, runtime div-zero trap verification, and release-mode wrap behavior.

Test count: **102 tests passing** (89 library + 13 e2e), zero warnings.

**Sample programs all run with correct output:**

| Program | Output |
|---|---|
| [factorial.cplus](docs/examples/factorial.cplus) | `3628800` |
| [fibonacci.cplus](docs/examples/fibonacci.cplus) | `6765` |
| [sum_range.cplus](docs/examples/sum_range.cplus) | `5050` |
| [c_for.cplus](docs/examples/c_for.cplus) | `45` |

**Grammar reservations** (no semantics yet, but the syntax must parse, per the lexer): `&T`, `&mut T`, `mut`, `unsafe`, plus future-keywords (`struct`, `enum`, `trait`, `impl`, `match`, `defer`, etc.). Reserving now avoids painful migrations later.

**LLVM features used in this phase:** `alloca` for every local + `mem2reg` pass to promote to SSA (avoids hand-rolled SSA construction); `add`/`sub`/`mul`/`sdiv`/`srem`/`icmp`/`br`/`ret`/`call`/`xor`/`load`/`store`/`extractvalue`/`unreachable`; `llvm.sadd.with.overflow.i32` / `llvm.ssub.with.overflow.i32` / `llvm.smul.with.overflow.i32` for debug-mode overflow detection; `llvm.trap` for both overflow and divide-by-zero traps; `declare i32 @printf(ptr, ...)` for `println`.

**Deferred (not blocking Phase 2):**
- [ ] **AST/IR JSON dumps**: `cpc parse --json`, `cpc check --json`. AST nodes need `serde::Serialize` derive. No consumer needs them yet; pick up when a tool (LSP, formatter, external analyzer) requires it.

**Exit met:** all four sample programs compile and run end-to-end via `cpc FILE -o OUT`; sema rejects every design-note §7.2 case with a structured diagnostic; runtime overflow traps in debug, wraps in release; runtime division-by-zero traps in both modes; CI green at 102 tests.

### Phase 2 — Aggregates and full primitive types · 4–8 weeks · ✅ done (slice 2E deferred)

Structured as three slices:

#### Slice 1 — Full primitive types + explicit casts · ✅ done

Design note: [docs/design/phase2-types.md](docs/design/phase2-types.md).

- All integer types (`i8 i16 i32 i64 u8 u16 u32 u64 isize usize`) and floats (`f32 f64`)
- Per-family operator semantics: signed-int debug-overflow trap; unsigned wrap; float IEEE 754
- Per-type comparison predicates: `slt/sle` signed, `ult/ule` unsigned, `olt/ole` float
- Float `%` rejected (E0316). Float `/` doesn't trap on zero (IEEE inf/nan).
- Negate (`-`) rejected on unsigned types
- Explicit `as` casts: numeric ↔ numeric, `bool → integer` allowed; `* → bool` rejected (E0315)
- Literal type inference from declared type (`let x: u64 = 42` works); strict no-implicit-conversion otherwise
- 3 new sample programs: [mixed_ints.cplus](docs/examples/mixed_ints.cplus) (i64 + casts), [float_arith.cplus](docs/examples/float_arith.cplus) (f64), [unsigned.cplus](docs/examples/unsigned.cplus) (u64 in a `for` loop)
- Test count: **143 total** (127 library + 16 e2e), zero warnings

**LLVM features used:** `add/sub/mul/sdiv/udiv/srem/urem`; `fadd/fsub/fmul/fdiv/fneg`; `icmp slt/sle/sgt/sge/ult/ule/ugt/uge/eq/ne`; `fcmp olt/ole/ogt/oge/oeq/one`; `llvm.{sadd,ssub,smul}.with.overflow.{i8,i16,i32,i64}` for signed checked arithmetic; cast instructions `trunc`/`sext`/`zext`/`fptosi`/`fptoui`/`sitofp`/`uitofp`/`fpext`/`fptrunc`/`bitcast`.

#### Slice 2A — Plain enums + path expressions · ✅ done

Design note: [docs/design/phase2-enums.md](docs/design/phase2-enums.md).

- `enum Name { V1, V2, ... }` declarations (no payloads, no discriminators)
- Two-segment path expressions `Name::Variant` (foundation for Phase 4 modules and Phase 3 tagged unions)
- Sema: `Ty::Enum(EnumId)`; declaration-order indexing; cross-enum types are distinct (E0302)
- Equality on enums works; ordering rejected (use `as i32` if you want it)
- Cast `EnumValue as i32` allowed (yields variant index); `int as Enum` rejected (E0315 — needs runtime range check)
- 1 new sample program: [direction.cplus](docs/examples/direction.cplus)
- New error codes: E0317 (unknown variant), E0318 (duplicate variant)
- Test count: **162 total** (145 library + 17 e2e), zero warnings

**LLVM features used:** Each enum lowers to `i32`. `Color::Red` is the constant `i32 0`. Equality is `icmp eq i32`. `as i32` cast is a no-op (already i32).

#### Slice 2B — Structs (no methods) · ✅ done

Design note: [docs/design/phase2-structs.md](docs/design/phase2-structs.md).

- `struct Name { f: T, ... }` declarations (including empty `Empty {}`); fields in declaration order
- Struct literal: `Point { x: 1, y: 2 }` — must specify all fields, no extras, no duplicates
- Field read: `p.x` (postfix `.`)
- Field assignment: `p.x = 5` — sema walks the Field chain to find the root mutable Ident; nested writes (`l.to.x = 5`) work
- Struct-literal-vs-block disambiguation: in head of `if`/`while`/`for-in <iter>`, an `Ident` followed by `{` is parsed as the cond/iter and the body block; force literal with parens
- Pass-by-value: structs as fn params and return types (`fn distance(a: Point, b: Point) -> i32`)
- Forward references: a struct field can name a type declared later in the file (two-pass collection)
- 3 new sample programs: [point.cplus](docs/examples/point.cplus), [mutable_struct.cplus](docs/examples/mutable_struct.cplus), [nested.cplus](docs/examples/nested.cplus)
- New error codes: E0319 (duplicate field), E0320 (unknown field on struct), E0321 (missing field in literal), E0322 (extra field in literal), E0323 (field access on non-struct). E0301 covers cross-type-namespace name collisions.
- Test count: **193 total** (173 library + 20 e2e), zero warnings

**LLVM features used:** `%Name = type { ... }` named-struct declarations in the preamble; `getelementptr` for field addressing; struct types as `define`-level params and return types (LLVM handles platform ABI lowering); aggregate `load`/`store` for whole-struct assignment.

#### Slice 2C — Methods on structs (`impl` blocks) · ✅ done

Design note: [docs/design/phase2-methods.md](docs/design/phase2-methods.md).

Resolved §11 open question on method syntax in favor of **Rust-style `impl` blocks** (over UFCS / no-methods): the borrow checker (Phase 5/6) needs `&self` / `&mut self` to express ownership, Phase 7 traits use the same `impl` syntax, and UFCS conflicts with the §2.8 no-overloading rule.

- `impl Type { fn method(...) -> T { ... } }` blocks for inherent methods on structs
- Three receiver forms: `self` (value, by-aggregate-value), `&self` (ptr, immutable), `&mut self` (ptr, mutable)
- Associated functions (no receiver): called as `Type::method(args)`
- Instance methods: called as `value.method(args)`
- `&mut self` requires a writable place at the call site (sema enforces); `&self` and `self` accept any expression
- `self` inside method bodies is a special local: receiver kind determines mutability (`&mut self` → mutable, others → immutable)
- LLVM name mangling: `Type::method` → `@Type.method` (using `.` separator — valid in LLVM, can't appear in C+ identifiers)
- New sample program: [methods.cplus](docs/examples/methods.cplus)
- New error codes: E0324 (no method on type), E0325 (impl on unknown/non-struct), E0326 (duplicate method), E0327 (wrong call form: instance-via-type or assoc-via-instance), E0328 (`&mut self` on non-mut place)
- Test count: **217 total** (196 library + 21 e2e), zero warnings

**LLVM features used:** function-name dot-mangling; `ptr` parameter type for `&self`/`&mut self` receivers (no extra alloca for the receiver param — bound directly to the SSA parameter, so `mem2reg` sees a clean pointer); struct-typed parameter for value receivers (LLVM handles aggregate ABI lowering).

#### Slice 2D — Fixed-size arrays · ✅ done

Design note: [docs/design/phase2-arrays.md](docs/design/phase2-arrays.md).

Scope decision: this slice handles **fixed-size arrays only**. Raw pointers `*T` and slices `T[]` interact directly with the Phase-5/6 borrow checker (`&T`, `&mut T`, fat pointers) and are deferred to Phase 3 or 5 where the same machinery is being built anyway.

- `[T; N]` fixed-size array type
- Array literal `[e1, e2, ...]` — element type uniform, length inferred from element count
- Indexing `a[i]` — runtime bounds check via `icmp uge` + `llvm.trap`; `i` must be `usize` (explicit `as usize` cast for `i32` loop counters)
- Indexed assignment `a[i] = v` — extends place-walk to Index chains; root mutability still required
- Pass-by-value as fn params and return types; arrays as struct fields
- **`Ty` refactor**: `Copy` → `Clone` to support `Box<Ty>` in `Ty::Array`. ~50 small `.clone()` insertions across sema and codegen. The right long-term model for generics and slices later.
- 2 new sample programs: [array_sum.cplus](docs/examples/array_sum.cplus), [array_struct.cplus](docs/examples/array_struct.cplus)
- New error codes: E0329 (mixed element types), E0330 (length mismatch), E0331 (indexing non-array), E0332 (empty array literal not supported)
- Test count: **236 total** (212 library + 24 e2e), zero warnings

**LLVM features used:** `[N x T]` array type; `getelementptr` for indexing (two-step: base GEP then element GEP); `icmp uge` + `br` + `call void @llvm.trap()` + `unreachable` for runtime bounds-check; array-as-aggregate parameter and return types.

#### Slice 2E — Slices + raw pointers · deferred to Phase 3 / 5

The original Phase-2 plan bundled raw pointers `*T` and slices `T[]` with arrays. After implementing arrays we realized: both depend on the reference-and-borrow machinery the borrow checker brings in Phase 5/6. Doing them in Phase 2 means designing the pointer story twice. Deferred to land naturally alongside `&T` / `&mut T`.

**Phase 2 exit:** ✅ met — sample programs walk arrays of structs ([array_struct.cplus](docs/examples/array_struct.cplus)). Linked lists via raw pointers deferred to Phase 3/5 with the borrow-checker rollout.

### Phase 3 — Core safety + move semantics · 4–8 weeks

- Definite assignment analysis
- Overflow trapping in debug + `+% -% *%` wrapping operators
- Non-null pointers, `?*T`, narrowing on `if (x != null)`
- Tagged unions + pattern matching with exhaustiveness
- Error unions `!T` and `try`
- `defer`
- **Move semantics:** every value has one owner; passing or assigning a non-`Copy` value consumes it; reading a moved-from variable is a compile error. `Copy` marker for primitives and small POD types. No borrows yet.

**Ownership surface syntax** — design note: [docs/design/phase3-ownership.md](docs/design/phase3-ownership.md). Three parameter forms map to the three semantic kinds: `x: T` shared borrow, `mut x: T` exclusive borrow, `move x: T` ownership transfer. Same forms on receivers: `self` / `mut self` / `move self`. Returns are always moves (no marker). `Copy` types collapse borrow forms to by-value copies. Call sites carry no markers — signature tells the story. New error codes: E0334 (move+mut both), E0335 (use of moved value), E0337 (move from non-owned place).

**LLVM features used:** all overflow-with-intrinsic forms for the full integer type set; `llvm.trap` for division-by-zero and overflow traps; `switch` instruction for tagged-union `match`; `noundef` parameter attribute (definite assignment lets us promise this everywhere).

**Exit:** Phase 2 samples rewritten using error unions, `defer`, and move semantics; double-free and use-after-move caught at compile time.

### Phase 4 — Modules + tooling foundations · 6–10 weeks

Modules and the first wave of project-level tooling. The pairing is intentional: a manifest, a formatter, and an LSP only make sense once the language has multi-file projects.

**Modules:**
- Module discovery and loading
- Explicit imports (no globs as default — see §2.8)
- Exports and visibility
- Multi-file compilation
- Mutual references between modules
- Basic incremental rebuilds (module-granular)

**Tooling:**
- **`Cplus.toml` project manifest** — name, version, edition (deferred — but field reserved), dependencies, targets, build flags. Cargo.toml as the model.
- **`cpc fmt`** — single canonical formatter, shipped in the same binary. No style options. Settled defaults; no `.editorconfig` debates.
- **LSP foundations** — `cpc-lsp` binary. Diagnostics, go-to-definition, hover for `fn` signatures. Built on the same `cplus-core` library as the CLI compiler.
- **Determinism guarantees enforced** — `BTreeMap` instead of `HashMap` in any code path that affects output, sorted iteration in codegen, no timestamps in artifacts.

**LLVM features used:** ThinLTO/FullLTO across modules.

**Exit:** a project split across 5+ files with a `Cplus.toml` manifest builds; `cpc fmt` round-trips all sample programs; `cpc-lsp` connects to VS Code and serves diagnostics + go-to-definition.

### Phase 5 — Immutable borrows + tooling polish · 3–4 months

**Borrow checking:**
- `&T` references — many readers, no writer
- Lifetime inference (no explicit annotations yet); follow Rust's elision rules wherever they cover the case
- Borrow conflict detection for shared references vs. moves
- Borrow-checker diagnostics framework (the long pole of error-message quality work)

**Tooling:**
- **`cpc test`** — built-in test runner. `#[test]` attribute (or `test fn` keyword — TBD design note). Auto-discovered, parallel by default, JSON output via `--json` (per-test pass/fail/duration/captured output). AI agents iterate via this loop.
- **Doctests** — `assert` lines inside `///` doc comments are extracted, compiled, and run by `cpc test`. Forces docs to stay correct.
- **LSP completions and find-references** — sema is now rich enough to drive these.

**Exit:** can write `fn longest(xs: &string, ys: &string) -> &string` without annotations; using a moved value while a `&T` borrow is alive is a compile error; `cpc test --json` runs the test suite for any in-tree program.

### Phase 6 — Mutable borrows + full aliasing rule · 3–4 months

- `&mut T` references — exclusive
- Aliasing-XOR-mutability: at any program point either many `&T` or one `&mut T`, never both
- Explicit lifetime annotation syntax `<'a>` for cases inference can't solve
- Drop analysis with conditional moves (different branches consuming different values)
- Iterator-invalidation, data-race, dangling-pointer cases all rejected at compile time

**LLVM features used:** **`noalias` parameter attribute on every `&mut T`** — the borrow checker proves uniqueness, so we can tag the LLVM parameter and unlock aggressive load/store reordering (this is one of the main reasons borrow-checked code can outperform C); atomic instructions (`load atomic`, `store atomic`, `cmpxchg`, `atomicrmw`, `fence`) with C11 ordering specifiers (`monotonic`/`acquire`/`release`/`acq_rel`/`seq_cst`) for `Atomic[T]` lowering.

**Exit:** A small in-tree test program implementing a `Vec[T]`-style growable array compiles cleanly and rejects iterator invalidation (`for x in vec { vec.push(...) }` errors at compile time).

### Phase 7 — Generics + interfaces · 2–4 months

- Parametric functions and types: `fn max[T: Ord](a: T, b: T) -> T`
- Monomorphization
- Interface mechanism (constraint trait-style: declare what operations a type must support)

**Exit:** A generic `Pair[A, B]` and a generic `Vec[T]`-style array can be defined and used in test programs; the type checker correctly monomorphizes per-instantiation.

### Phase 8 — C interop hardening · 1–3 months

- `extern fn` declarations: the foundation lands earlier in Phase 1; this phase is calling-convention hardening and edge cases
- ABI compliance verified on x86_64 Linux first, then macOS/ARM, then Windows
- Struct layout compatibility (`#[repr(C)]`-equivalent)
- Varargs in `extern fn`
- `cstring` ↔ `string` conversions
- `cpc-bindgen` (separate tool, not language feature) as a stretch goal

**LLVM features used:** `ccc` calling convention for `extern fn`; ABI parameter attributes (`byval`, `sret`, `inreg`, `nest`) to match the platform C ABI on struct-passing edge cases; varargs handled via LLVM's per-target rules; ThinLTO/FullLTO link-time optimization across modules.

**Exit:** a program calling libc to open and read a file works end-to-end across all supported platforms.

### Phase 9 — Polish · indefinite

(LSP, formatter, test runner, structured diagnostics all landed earlier.)

- Better error messages (continuous; borrow-checker diagnostics are the long pole)
- Debugger support (DWARF — largely free from LLVM via `!DIFile` / `!DISubprogram` / `!DILocation` metadata; ideally wired up earlier so source positions don't have to be retrofitted)
- Sanitizer flags (`cpc --asan` / `--ubsan` / `--tsan` / `--msan`) — instrumented user binaries via LLVM's existing pass infrastructure
- Package manager (dependency resolution, registry — extends Phase 4 manifest)
- Documentation generator (extends Phase 5 doctests)
- Effect tracking design exploration (deferred speculative feature; see §11)
- Built-in contracts design exploration (deferred speculative feature; see §11)
- CLI niceties

---

## 4. LLVM strategy

LLVM is doing most of the heavy lifting. This section catalogs what we use it for, organized by category. Per-phase usage is annotated inline in §3.

### 4.1 Codegen freebies

Things LLVM does so we don't have to.

- **`alloca` + `load`/`store` + `mem2reg` pass.** Every local gets an `alloca`; reads and writes use `load`/`store`; the `mem2reg` pass promotes well-behaved locals to SSA registers and inserts PHI nodes. This is the single biggest gift — we don't write SSA construction. Same approach as clang and rustc.
- **`getelementptr` (GEP).** All pointer arithmetic, struct field access, and array indexing. Typed and target-aware. Notoriously confusing at first; spend an afternoon on the GEP FAQ before Phase 2.
- **Optimization passes.** The `-O0`/`-O1`/`-O2`/`-O3` pipeline is free. Inlining, dead-code elim, GVN, LICM, instcombine, loop vectorization, sccp. We emit straightforward IR; LLVM does the smart work. Within ~5% of clang's output without ever writing an optimization ourselves.
- **DWARF debug info.** `!DIFile` / `!DISubprogram` / `!DILocation` metadata gives gdb/lldb support, source-level breakpoints, and stack traces. Wire it up early — retrofitting source positions through every instruction is painful.

### 4.2 Intrinsics that match C+ semantics directly

- **Checked arithmetic.** `llvm.sadd/ssub/smul.with.overflow.iN` (signed) and `llvm.uadd/usub/umul.with.overflow.iN` (unsigned). Returns `{result, overflow_bit}`. Branch on the bit to call `llvm.trap`. Direct match for our debug-mode trap-on-overflow rule. Release mode uses plain `add`/`sub`/`mul`.
- **`llvm.trap` and `unreachable`.** `llvm.trap` emits the architecture's trap (UD2 / BRK). What runtime panics call. `unreachable` is a control-flow hint for places we've proven can't be reached (post-`return`, exhaustive match defaults, after `try` branches that always diverge).
- **Atomic instructions.** `load atomic` / `store atomic` / `cmpxchg` / `atomicrmw` / `fence` with ordering specifiers `monotonic` / `acquire` / `release` / `acq_rel` / `seq_cst`. One-to-one with our C11 memory model. `Atomic[T]` (compiler-blessed library types) lower verbatim.

### 4.3 Performance unlocks the borrow checker hands us

This is where C+ can outperform C in some workloads. The borrow checker proves things the C compiler isn't allowed to assume.

- **`noalias` parameter attribute.** Every `&mut T` is provably non-aliasing. Tag the LLVM parameter with `noalias` and the optimizer reorders loads/stores aggressively. Rust does this; we do the same. Phase 6.
- **`nonnull` and `dereferenceable(N)`.** Non-null pointers are the default in C+; `*T` and `&T` are guaranteed non-null. Tagging with `nonnull` elides null checks; `dereferenceable(N)` says "≥ N bytes here are valid." Phase 2/3.
- **`align` attribute / metadata.** Pointer alignment guarantees enable better load/store codegen on strict-alignment targets.
- **`noundef` parameter attribute.** Definite assignment + non-null + initialized values let us promise this widely; LLVM uses it to eliminate UB-related conservatism in optimization.

### 4.4 ABI / interop machinery

- **Calling conventions.** `ccc` (C-compatible, default) for `extern fn`; `fastcc` for non-`extern` C+ internals (better register usage on some targets). Phase 8.
- **Parameter ABI attributes.** `byval`, `sret`, `inreg`, `nest` — for matching the platform C ABI on struct-passing edge cases. The most error-prone part of C interop; LLVM handles the per-target rules.
- **Varargs.** `declare i32 @printf(ptr, ...)` — LLVM handles platform-specific calling conventions for varargs (which differ wildly between x86_64 SysV and ARM AAPCS).

### 4.5 Tooling that's just there

- **Sanitizers.** `cpc --asan` / `--ubsan` / `--tsan` / `--msan` — instrumented user binaries. Also useful for self-testing the compiler during development. Phase 9.
- **ThinLTO / FullLTO.** Cross-module link-time optimization. Just a flag on the linker invocation. Phase 4 onward.
- **PGO / AutoFDO hooks.** Available if and when profile-guided optimization is wanted. Not on the roadmap but free if needed.

### 4.6 Out of scope (don't spend time on)

- Exception handling (`landingpad` / `invoke` / `resume`) — C+ has no exceptions.
- Garbage collection statepoints — N/A.
- JIT (MCJIT / ORC) — we're AOT.
- Coroutines — no async/await in C+.
- Polly (polyhedral loop optimizer) — beyond scope.

### 4.7 Phase-by-phase summary

| Phase | LLVM features |
|-------|---------------|
| 1 | `alloca`/`load`/`store` + `mem2reg`; basic instructions; `llvm.sadd.with.overflow.i32` + `llvm.trap`; `printf` extern |
| 2 | `getelementptr`; aggregate types; `extractvalue`/`insertvalue`; `nonnull`/`dereferenceable` |
| 3 | All overflow intrinsics; `switch` for `match`; `noundef` |
| 4 | ThinLTO/FullLTO across modules |
| 5 | (no new LLVM features — borrow analysis is on our IR, not LLVM's) |
| 6 | `noalias` on `&mut`; atomic instructions + orderings |
| 8 | `ccc` calling convention; ABI parameter attributes; varargs |
| 9 | DWARF metadata; sanitizer passes |

---

## 5. Tooling architecture (AI-native)

C+ treats tooling as Phase 1–4 plumbing, not Phase 9 polish. The unifying principle: **the language gives tools as much information as possible, and the tools are present and excellent from early phases.** Pre-AI languages got away with bolt-on tooling. C+ doesn't.

### 5.1 Compiler-as-library

The compiler is split into:

- **`cplus-core`** — library crate. Lexer, parser, AST, sema, codegen, diagnostics. Stable Rust API consumed by every tool.
- **`cpc`** — binary crate, ~200 lines. Argument parsing and dispatch into `cplus-core`. Exposes `cpc build`, `cpc check`, `cpc parse`, `cpc fmt`, `cpc test`, `cpc lsp`.
- **`cpc-lsp`** — separate binary (Phase 4) that links the same `cplus-core`.

This is a Phase-1 architectural decision. Any tool ever written for C+ uses the same library that the compiler does — no separate reimplementation, no tool drift.

### 5.2 Structured diagnostics

Every error and warning is structured, not a printf string. JSON shape (per-diagnostic):

```json
{
  "file": "foo.cplus",
  "span": { "start_line": 12, "start_col": 5, "end_line": 12, "end_col": 8 },
  "severity": "error",
  "code": "E0042",
  "message": "expected `;` after expression",
  "suggestions": [
    { "description": "insert `;`", "span": {...}, "replacement": ";" }
  ]
}
```

`cpc --diagnostics=json` emits this; `cpc --diagnostics=human` (default) renders the human-readable form on top of the same data. Suggestions are machine-applicable: an AI agent can apply a fix without round-tripping through an LLM.

This format is part of the language's stable interface from Phase 1. Every error site in the compiler produces a structured diagnostic; the rendering layer is downstream.

### 5.3 Determinism

Same inputs → byte-identical outputs. Required so AI agents can hash-compare to verify changes.

- `BTreeMap` not `HashMap` in any code path that affects output (codegen, diagnostics ordering, AST emit).
- Sorted iteration over collections.
- No timestamps, build paths, or other environmental data baked into output.
- Deterministic codegen — no nondeterministic optimizer choices.

### 5.4 Built-in subcommands

A single binary, multiple modes. All of them link `cplus-core`.

- `cpc build foo.cplus -o foo` — full pipeline; produces a binary.
- `cpc check FILE` — lex + parse + sema; no codegen. The fast feedback loop.
- `cpc parse FILE [--json]` — AST dump. JSON mode is for tools.
- `cpc check FILE --json` — type-resolved IR dump.
- `cpc fmt FILE` — canonical formatter, in place.
- `cpc test [PATH]` — discover + run `#[test]` functions and doctests.
- `cpc lsp` — start the LSP server (delegates to `cpc-lsp` binary).
- `cpc --tokens FILE` — debug helper (already exists).
- `cpc --ast FILE` — debug helper (already exists).
- `cpc --emit-ir` — debug helper (Phase 0 frozen IR).

### 5.5 AST/IR as serialized data

Every AST node and IR node derives `serde::Serialize` from the start. Tools we don't anticipate yet will rely on this. Cost is near zero if the AST avoids `Rc<RefCell<…>>` cycles; cost is enormous to retrofit.

### 5.6 Doctests

`///` doc comments may contain `assert` expressions:

```cp
/// Returns the larger of two i32s.
///
/// Examples:
///   assert max(1, 2) == 2;
///   assert max(-5, -10) == -5;
fn max(a: i32, b: i32) -> i32 { ... }
```

`cpc test` extracts these, compiles each as a test function, runs them. Documentation that doesn't compile is a test failure. AI is excellent at writing doc comments with examples; this gives those examples teeth.

Design note required before Phase 5 implementation: extraction syntax, scope/imports inside doctests, error attribution.

### 5.7 Editions (deferred)

Not implemented yet, but the design intent is recorded so we don't paint ourselves into a corner. Eventually each `.cplus` file or project will declare its edition; the compiler handles each edition's syntax and semantics. Currently all code is treated as edition `2026` implicitly. We add the system when we actually need to break something.

### 5.8 Locality of reasoning

A single function should be readable using only the function and the signatures of what it calls. C+ already excludes the worst offenders (macros, decorators, inheritance, comptime). Future additions are evaluated against this constraint.

### 5.9 AI recovery

C+ assumes AI-generated code will often be wrong. The compiler and tools are designed to minimize the cost of finding and fixing those errors — **AI recovery** is the loss function that the principles in §5.1–§5.8 are optimizing. This section names what that means concretely and how we'll eventually measure it.

**Qualitative goals.** Most are promised elsewhere; bringing them together to articulate the recovery property:

- **Diagnostics identify the smallest useful span.** A missing semicolon highlights the gap, not the surrounding statement. A type mismatch highlights the mismatched value, not the whole expression. Cross-checked at every error site we add. *(New constraint, applies everywhere.)*
- **Generated fixes do not require parsing human prose.** Every suggestion is `(span, replacement)` per §5.2. AI agents apply fixes by string substitution against structured fields, not by NLP on `message`. *(Structural property of the diagnostic format.)*
- **Stable diagnostic JSON** — §5.2 commits the format. Tools and agents rely on the shape across versions.
- **Structured test output** — §5.4 (`cpc test --json`). Iteration loops read pass/fail/duration/output without parsing human-readable test output.
- **Canonical formatter output** — §5.4 (`cpc fmt`). Diffs between AI revisions stay small because formatting doesn't drift.
- **Explicit imports** — §2.8 forbids glob imports as default; missing-import errors are local, repairable without project-wide search.
- **Sound types, no `any`** — §2.8. The type system always rejects what's actually broken; never silently passes broken code through to runtime.
- **Locality of reasoning** — §5.8. A function plus the signatures it calls suffices to understand it; AI generations don't need to globally reason to be correct.

**Quantitative metrics (future).** Once a corpus of broken AI-generated C+ programs exists, we evaluate the compiler against it:

- % diagnosed with a precise error code (vs. a generic "expected token" fallback)
- % where the diagnostic carries a `MachineApplicable` or `MaybeIncorrect` suggestion
- % repaired in one agent pass (apply suggestions → recompile → pass)
- % repaired without changing the program's intended behavior (requires per-program intent labels — hardest to measure; needs differential testing or human review)

**The corpus does not exist yet.** Building it is research work: collect real broken programs from AI tools in the wild, label them with intent, freeze as a regression suite. Tracked in §11. Until that lands, the bullets above are aspirational, not SLOs.

---

## 6. Per-feature design-note workflow

Before implementing any non-trivial feature, write a short doc in `docs/design/` (1–2 pages):

1. **Problem.** What does this solve?
2. **Syntax.** With 3–5 examples.
3. **Semantics.** Including edge cases.
4. **Interactions.** With every already-implemented feature.
5. **Open questions.**

Keep these short. The cost is an hour and they prevent multi-day rewrites. Throw them away when implementation reveals the design was wrong — that's the point.

---

## 7. Project layout

```
.
├── Cargo.toml                    workspace
├── plan.md                       this file
├── docs/
│   ├── design/                   per-feature design notes, before implementing
│   ├── spec/                     reference manual, written late
│   └── examples/                 sample programs that must compile
├── cplus-core/                   library crate — all language logic
│   └── src/
│       ├── lib.rs                re-exports the public API
│       ├── lexer.rs
│       ├── parser.rs
│       ├── ast.rs
│       ├── sema.rs               name resolution, type checking
│       ├── codegen.rs            LLVM IR generation
│       └── diagnostics.rs        structured-error infrastructure (§5.2)
├── cpc/                          binary crate — thin CLI wrapper
│   ├── src/
│   │   ├── main.rs               argument parsing + dispatch into cplus-core
│   │   └── hello.ll              Phase-0 frozen IR (vestigial)
│   └── tests/
│       └── e2e.rs                program → compile → run → assert output
└── cpc-lsp/                      binary crate (Phase 4) — also links cplus-core
    └── src/
        └── main.rs
```

The cplus-core/cpc/cpc-lsp split is the §5.1 compiler-as-library architecture. Every tool ever written for C+ uses the same `cplus-core` library, no separate reimplementations.

---

## 8. Testing strategy

- **Unit tests** for lexer, parser, sema components.
- **Snapshot tests** for IR output of canonical programs. Catches regressions in codegen.
- **End-to-end tests**: program → compile → run → assert stdout/exit code. The most important kind. Each phase adds programs.
- **Negative tests**: programs that must fail to compile, asserting the specific error.
- **Differential tests** (Phase 8+): if a program is valid both as our language and as C, output should match clang.

A new feature is not done until it has unit + e2e + at least one negative test.

---

## 9. How to use this plan with Claude Code

For any implementation task:

1. Identify the phase and feature in §3.
2. If the feature has no design note yet, write one in `docs/design/` first (§6). Get review.
3. Implement in this order: lexer → parser → AST → sema → IR → tests.
4. Each change adds at least one e2e test program in [cpc/tests/](cpc/tests/).
5. If a phase's scope shifts, update this plan in the same change.

Do not skip step 2.

---

## 10. References

- LLVM Kaleidoscope tutorial — covers ~70% of IR generation patterns needed for Phase 1–2.
- Zig language reference — closest existing point in design space, especially for comptime, error unions, allocator-passing, slices.
- Rust reference — for module system, generics, and ownership ideas.
- Crafting Interpreters (Nystrom) — frontend techniques, even though it targets a tree-walking interpreter.
- Engineering a Compiler (Cooper & Torczon) — semantic analysis and IR construction.
- LLVM Language Reference — authoritative on IR semantics; consult before assuming what an instruction does.

---

## 11. Open questions log

Track here as they come up. Resolve before they block work.

Deferred (not blocking Phase 2):
- [ ] `serde::Serialize` derive on AST nodes; `cpc parse --json` / `cpc check --json` subcommands. Pick up when a tool (LSP, formatter, external analyzer) actually needs it.
- [ ] **AI recovery corpus** (per §5.9): collect broken AI-generated C+ programs with intent labels; freeze as a regression suite. Enables the quantitative metrics in §5.9 to become real measurements. Research work — depends on having enough AI tools producing C+ to scrape. Plausibly Phase 4+ once tools exist.

Design notes needed before their phase (per §6):
- [x] Phase 3: ownership surface syntax (`x: T` / `mut x: T` / `move x: T`; receivers symmetric; returns always moves) — see [docs/design/phase3-ownership.md](docs/design/phase3-ownership.md)
- [ ] Phase 3: `Copy` derivation rules — auto-derive for struct-of-Copy / array-of-Copy or require explicit marker? (note §8 open question)
- [ ] Phase 3: `Drop`/destructors — declaration syntax, drop ordering at scope exit, interaction with `defer`
- [ ] Phase 4: `Cplus.toml` manifest schema; module discovery rules; glob-import policy
- [ ] Phase 4: `cpc fmt` style decisions (indent, line length, alignment, comment handling)
- [ ] Phase 4: LSP transport, capability set, what diagnostics to surface live
- [ ] Phase 5: lifetime elision rules; do we copy Rust's wholesale or simplify
- [ ] Phase 5: `#[test]` attribute syntax; doctest extraction rules and scoping
- [ ] Phase 6: explicit lifetime annotation syntax (`<'a>` is taken; consider alternatives)
- [ ] Phase 7+ (speculative): effect tracking syntax (`fn pure`, `fn allocates`, etc.) — Koka/Roc/OCaml 5 references
- [ ] Phase 7+ (speculative): contracts syntax (`requires`, `ensures`) — Eiffel/Dafny references

Resolved (kept for history):
- §1.1 ABI-only via `extern fn`; bindgen as separate future tool
- §1.2 Rust as the compiler implementation language
- §1.2 No comptime
- §1.2 Full borrow checker, phased across Phases 3/5/6
- §1.2 Wrap on release overflow, trap on debug
- §1.2 C11 memory model + borrow checker prevents data races; volatile via functions
- §2.7 Comma operator removed (operator only); separator stays in arg lists, struct literals, and `for` headers
- Language name: **C+**
- Source file extension: `.cplus`
- `for` loop: both C-style `for (init; cond; update)` (parens required) and range/iterator `for x in 0..n` / `for x in coll`
- Statement terminator: semicolons
- Function keyword: `fn`
- Variable declaration: `let` (immutable by default), `let mut` for mutable
- Range syntax: `..` exclusive, `..=` inclusive
- Block expressions: blocks evaluate to their last expression (Rust-style)
- Phase 1 grammar drafted: see [docs/design/phase1-grammar.md](docs/design/phase1-grammar.md)
- §2.8 No function overloading
- §2.8 No operator overloading (skipped entirely; named methods for arithmetic-like types)
- §2.8 No `any`-type / dynamic escape hatch
- §2.8 No glob imports as default
- §2.8 No inheritance
- §5.1 Compiler-as-library: cplus-core (library) + cpc (binary) + cpc-lsp (binary), all sharing the same library
- §5.2 Structured diagnostics from Phase 1 (JSON output, machine-applicable suggestions)
- §5.3 Determinism guaranteed (BTreeMap, sorted iteration, no timestamps in artifacts)
- §5.4 Built-in subcommands: `build`, `check`, `parse`, `fmt`, `test`, `lsp`
- §5.6 Doctests in scope (Phase 5; design note required)
- §5.7 Editions: deferred until we need a second one; intent recorded
- Library/binary split landed: cplus-core + cpc separate crates
- Phase 1 lexer landed: 14 unit tests, all green
- Phase 1 parser landed: 13 unit tests, all green
- Phase 1 diagnostics infrastructure landed: see [docs/design/diagnostics.md](docs/design/diagnostics.md), 8 unit tests + structured `--diagnostics=json|short|human` flag
- Phase 1 sema landed: 14 error codes E0300–E0313, 29 unit tests
- Phase 1 codegen landed: 19 unit tests; alloca+mem2reg strategy; all four samples emit IR; debug-mode overflow trapping (`llvm.{sadd,ssub,smul}.with.overflow.i32` + `llvm.trap`); always-on divide-by-zero trap; `BuildMode::Debug` (default) vs `BuildMode::Release`
- Phase 1 driver landed: `cpc FILE [-o OUT]` runs full pipeline; `cpc --release` / `--debug` mode flags; 13 e2e tests including 4 sample programs, runtime overflow trap verification, runtime div-zero trap verification, and release-mode wrap behavior
- **Phase 1 complete: 102 tests, 0 warnings, all 4 sample programs compile + run; overflow traps in debug, wraps in release; div-zero always traps.**
- Phase 2 slice 1 (types + casts) landed: full primitive type set; 17 new sema tests; 17 new codegen tests; 3 new sample programs (mixed_ints, float_arith, unsigned); E0315 invalid cast, E0316 float modulo. Test total: **143** (127 library + 16 e2e), 0 warnings.
- Phase 2 slice 2A (plain enums + paths) landed: `enum Name { V1, V2, ... }`; two-segment path expressions `Name::Variant`; `Ty::Enum(EnumId)`; cross-enum types distinct; equality but no ordering; `enum as i32` cast; design note [docs/design/phase2-enums.md](docs/design/phase2-enums.md); 13 new sema tests; 4 new codegen tests; 1 new sample program (direction); E0317 unknown variant, E0318 duplicate variant. Test total: **162** (145 library + 17 e2e), 0 warnings.
- Phase 2 slice 2B (structs, no methods) landed: `struct Name { f: T, ... }`; struct literals `Name { f: e }`; field read `p.x`; field assignment with mutable-root walk (`l.to.x = 5` works when `l` is `let mut`); struct-literal-vs-block disambiguation (no struct literals in `if`/`while`/`for-in` heads without parens); pass-by-value as fn params and returns; forward references between structs; design note [docs/design/phase2-structs.md](docs/design/phase2-structs.md); 19 new sema tests; 7 new codegen tests; 3 new sample programs (point, mutable_struct, nested); E0319 duplicate field, E0320 unknown field, E0321 missing field, E0322 extra field, E0323 non-struct field access. Test total: **193** (173 library + 20 e2e), 0 warnings.
- Phase 2 slice 2C (methods + `impl`) landed: Rust-style `impl Type { fn method(...) }`; three receiver forms (`self`, `&self`, `&mut self`); name mangling `Type::method` → `@Type.method`; `value.method()` and `Type::method()` call dispatch in both sema and codegen; resolved §11 open question on method syntax. Design note [docs/design/phase2-methods.md](docs/design/phase2-methods.md); 15 new sema tests; 7 new codegen tests; 1 new sample program (methods); E0324 no method, E0325 impl on non-struct, E0326 duplicate method, E0327 wrong call form, E0328 `&mut self` on non-mut place. Test total: **217** (196 library + 21 e2e), 0 warnings.
- Phase 2 slice 2D (fixed-size arrays) landed: `[T; N]` types; `[e1, e2, ...]` literals; bounds-checked `a[i]` indexing via `icmp uge` + `llvm.trap`; indexed assignment via place-walk extension; arrays as fn params, returns, struct fields; `Ty: Copy → Clone` refactor (~50 small clones); 2 new sample programs (array_sum, array_struct); E0329–E0332 error codes; deferred slices+raw pointers to Phase 3/5. Test total: **236** (212 library + 24 e2e), 0 warnings.
- **Phase 2 ✅ done.** All four slices landed; slice 2E (slices + raw pointers) deferred to Phase 3/5 where the borrow-checker brings in references.
- **Style migration (post-Phase 2)** landed: §2.8a style rules now enforced. Function bodies use explicit `return` (E0333 rejects implicit tail). Method receivers use `self`/`mut self` syntax — `&` removed from the language. All 14 sample programs rewritten. `Point::magnitude` renamed to `Point::magnitude_squared` (precise naming rule). Receiver enum collapsed: `Receiver::Value/Ref/RefMut` → `Receiver::Read/Mut`; codegen now always pointer-passes for receivers. Test count after migration: **234** (210 library + 24 e2e), 0 warnings.
- AI-native: tooling moved out of Phase 9 into Phases 1, 4, 5
- Speculative kept on roadmap: (17) effect tracking, (18) contracts (Phase 7+ design exploration)
- Speculative dropped: (19) capability-based imports
