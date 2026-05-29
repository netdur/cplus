# C+ Code Inspection Report

Date: 2026-05-25. Repo root: `/Users/adel/Workspace/C+`. Compiler used: `/Users/adel/Workspace/C+/target/debug/cpc`.

Scratch probes live under `/tmp/cpx/cases/*.cplus`; they are cited like source files because they are the exact programs compiled.

## T1 - Repo Map & Build Economics

Depth-2 tree:

```text
.
.claude
.git
bench
cpc
cpc/src
cpc/tests
cpc-bindgen
cpc-bindgen/src
cpc-lsp
cpc-lsp/src
cpc-lsp/tests
cplus-core
cplus-core/src
docs
docs/design
docs/examples
editors
editors/vscode
objc-c-interop
objc-c-interop/cocoa-min
proves
proves/benchmark
proves/runs
proves/target
proves/vendor
target
target/debug
target/flycheck0
target/release
target/tmp
vendor
vendor/accelerate ... vendor/uuid
```

Component map and LOC:

| Component | Path | Evidence | LOC |
|---|---|---:|---:|
| Lexer | `cplus-core/src/lexer.rs` | token model and `tokenize` entry at `cplus-core/src/lexer.rs:40-57`, `cplus-core/src/lexer.rs:156-170` | part of core |
| Parser | `cplus-core/src/parser.rs` | hand-written `Parser` and `parse_program` at `cplus-core/src/parser.rs:42-57`, `cplus-core/src/parser.rs:179-193` | part of core |
| Sema/type-check | `cplus-core/src/sema.rs` | diagnostics and type rules throughout; string method checker at `cplus-core/src/sema.rs:8323-8371` | part of core |
| Borrow checker | `cplus-core/src/borrowck.rs` | module header and error families at `cplus-core/src/borrowck.rs:1-59` | part of core |
| Codegen/LLVM | `cplus-core/src/codegen.rs` | build modes and trap emission at `cplus-core/src/codegen.rs:822-826`, `cplus-core/src/codegen.rs:3189-3190` | part of core |
| Compiler CLI | `cpc/src/main.rs` | CLI help text emitted by `target/debug/cpc --help`; source warnings refer to `cpc/src/main.rs:1263` | 2,730 |
| LSP | `cpc-lsp/src/main.rs` | LSP crate path present; e2e tests in `cpc-lsp/tests/e2e.rs:66-76` | 826 |
| Stdlib | `vendor/stdlib/src` | SKILL lists stdlib import model at `SKILL.md:397-419`; files present under `vendor/stdlib/src` | 3,209 |
| Vendor packages | `vendor/*` | SKILL package list at `SKILL.md:422-437` | 10,552 |
| Compiler tests | `cpc/tests/e2e.rs` | canonical negative patterns at `cpc/tests/e2e.rs:843-883` | 15,814 |

LOC command:

```bash
for d in cplus-core/src cpc/src cpc-lsp/src cpc-bindgen/src vendor/stdlib/src vendor docs/examples cpc/tests cpc-lsp/tests; do ... wc -l; done
```

Trimmed output:

```text
cplus-core/src    56626 total
cpc/src     2730 cpc/src/main.rs
cpc-lsp/src      826 cpc-lsp/src/main.rs
cpc-bindgen/src      567 cpc-bindgen/src/main.rs
vendor/stdlib/src     3209 total
vendor    10552 total
docs/examples     3893 total
cpc/tests    15814 cpc/tests/e2e.rs
cpc-lsp/tests      575 cpc-lsp/tests/e2e.rs
```

Build timing:

| Subject | Command | Result |
|---|---|---|
| Rust clean | `cargo clean` then `/usr/bin/time -p cargo build` | `real 11.61`, `user 36.61`, `sys 4.27`; warnings only |
| Rust incremental after `touch cplus-core/src/lexer.rs` | `/usr/bin/time -p cargo build` | `real 0.97`, `user 0.96`, `sys 1.12`; warnings only |
| Scratch C+ project clean | `cd /tmp/cpx/project && rm -rf target && /usr/bin/time -p cpc build` | `real 0.06`, `user 0.06`, `sys 0.03` |
| Scratch C+ project incremental after `touch src/main.cplus` | `cd /tmp/cpx/project && /usr/bin/time -p cpc build` | `real 0.06`, `user 0.06`, `sys 0.03` |
| Scratch single-file check | `/usr/bin/time -p cpc check /tmp/cpx/cases/small.cplus` | `real 0.00`, `user 0.00`, `sys 0.00` |

Path drift: SKILL paths at `SKILL.md:22-30` are accurate after rebuild: `target/debug/cpc`, `target/debug/cpc-lsp`, `vendor/stdlib`, `vendor/{appkit,accelerate,metal,simd,arena,clap,json,log,uuid,static-arena}`, and `docs/examples/` all exist.

## T2 - Accepted Surface vs SKILL §3

Grammar location: no `.lalrpop`, `.pest`, `.y`, or `.peg` files were found. The parser is hand-written Rust: `parse(tokens)` creates `Parser::new(tokens)` and calls `parse_program()` at `cplus-core/src/parser.rs:42-45`; parser item dispatch is explicit at `cplus-core/src/parser.rs:213-258`.

Construct probe table:

| Construct | Compiles? | Evidence |
|---|---:|---|
| literals: ints, suffixes, float, bool, `str`, `_`, `0x`, `0b`, byte `'a'` | yes | `/tmp/cpx/cases/t2_literals.cplus:1-10`; `cpc check .../t2_literals.cplus` returned `status=0` |
| `if` expression, `while`, range `for`, C-style `for`, `loop` | yes | `/tmp/cpx/cases/t2_control.cplus:1-6`, `/tmp/cpx/cases/t2_control.cplus:9-10`; failure below is array-for only |
| `for v in arr` | no | `/tmp/cpx/cases/t2_control.cplus:7-8`; output: `error[E0312]: for ... in requires either a closed range (0..n) or an Iterator[T], got [i32; 3]` |
| `while let` | yes | `/tmp/cpx/cases/t2_while_let.cplus:1-7`; `cpc check .../t2_while_let.cplus` returned `status=0` |
| struct + `self`, `mut self`, `move self` | yes | `/tmp/cpx/cases/t2_struct_receivers3.cplus:1-8`; `status=0` |
| `borrow self` receiver | no | `/tmp/cpx/cases/t2_struct_receivers.cplus:4-7`; output: `error[E0100]: expected identifier, found token` at `borrow self` |
| plain/tagged/generic enums, exhaustive `match`, generics `[T]`, turbofish `::[T]` | yes | `/tmp/cpx/cases/t2_enums_match.cplus:1-9`; `status=0` |
| array fill literal `[0u8; N]` | yes | `/tmp/cpx/cases/t2_array_fill.cplus:1`; `status=0`; AST describes `ArrayFill` at `cplus-core/src/ast.rs:729-740` |
| string interpolation as SKILL `\{x}` | no | `/tmp/cpx/cases/t2_interp.cplus:1`; output: `error[E0001]: unexpected character {` |
| string interpolation as compiler `${x}` | yes | `/tmp/cpx/cases/t2_interp_dollar2.cplus:1`; `status=0`; lexer documents `${name}` at `cplus-core/src/lexer.rs:49-56` |
| format specifier `\{x:04d}` | no | `/tmp/cpx/cases/t2_format_spec.cplus:1`; same lexer error as above |

DRIFT from SKILL: array iteration is claimed at `SKILL.md:140-142` but rejected by compiler; `borrow self` is claimed at `SKILL.md:246` but parser rejects it; struct shorthand `Point { x, y }` is shown at `SKILL.md:151` but the parser required `Point { x: x, y: y }`; interpolation is claimed as `\{x}` at `SKILL.md:224-230` but actual accepted syntax is `${x}`.

Parser accepts forms SKILL does not foreground in §3: type aliases (`type Foo = Bar;`) at `cplus-core/src/parser.rs:720-735`, tuple types and tuple literals at `cplus-core/src/parser.rs:1250-1284` and `cplus-core/src/parser.rs:2749-2770`, and `#name(...)` intrinsics at `cplus-core/src/ast.rs:794-809`.

## T3 - Enforcement of the 13 Locked Principles

| Principle | Enforcement | Error | Evidence |
|---|---|---|---|
| no `null` | compiler | E0300 | `/tmp/cpx/cases/t3_null.cplus:1`; `undefined name null` |
| no closures/lambdas | parser/compiler | E0100 | `/tmp/cpx/cases/t3_closure.cplus:1`; `expected expression` |
| no `&T` / `&mut T` types | parser/compiler | E0100 | `/tmp/cpx/cases/t3_ref_type.cplus:1`; `expected type` |
| no `try` / `?` | lexer/compiler | E0001 | `/tmp/cpx/cases/t3_try_q.cplus:1-2`; `unexpected character ?` |
| no implicit width conversion | compiler | E0302 | `/tmp/cpx/cases/t3_implicit_width.cplus:1`; `expected i64, found i32` |
| no overloads | compiler | E0301 | `/tmp/cpx/cases/t3_overload.cplus:1-2`; duplicate `f` |
| no macros/decorators | compiler-known attributes only | E0354 | `/tmp/cpx/cases/t3_macro_attr.cplus:1-2`; unknown `#[derive]`; known attributes are metadata per `SKILL.md:468-478` |
| no `class` / `var` | parser/compiler | E0100 | `/tmp/cpx/cases/t3_class_var.cplus:1-2`; `class` parsed as identifier where item expected |
| no mutable by default | compiler | not separately probed here | `let mut` surface is in SKILL at `SKILL.md:117-126`; assignment to immutable is covered by sema, but not re-run in this matrix |
| generics use `[T]`, not `<T>` | parser/compiler | E0100 | `/tmp/cpx/cases/t3_angle_generics.cplus:1-2`; expected `(` |
| explicit function return | compiler | E0333 | `/tmp/cpx/cases/t3_tail_return.cplus:1`; `use return ...;` |
| `::` vs `.` separation | compiler | E0303/E0327 family | `/tmp/cpx/cases/t3_dot_colon.cplus:1-3`; `p::val()` is treated as unknown type `p` |
| module-private by default | resolver/compiler | E0403 | `/tmp/cpx/privateproj/src/main.cplus:1-2`; output: `function hidden is private (mark it pub...)`; resolver defines E0403 at `cplus-core/src/resolver.rs:65-66` and `cplus-core/src/resolver.rs:567-570` |

Convention-only: none found among the probed items, except "no mutable by default" was not independently reprobed in this run.

## T4 - Error-Handling Reality

Actual `Result[T,E]` API: enum variants `Ok(T)` and `Err(E)` plus constructors `ok`, `err`, `io_ok`, `io_err`; `IoError` has fixed variants. Evidence: `vendor/stdlib/src/result.cplus:3-23`, `vendor/stdlib/src/result.cplus:25-43`.

Actual `Option[T]` API: enum variants `Some(T)` and `None` plus constructor `some`. Evidence: `vendor/stdlib/src/option.cplus:12-21`.

Requested helpers inventory:

| Helper | Result | Option |
|---|---:|---:|
| `map`, `and_then`, `map_err` | NOT FOUND | NOT FOUND |
| `unwrap`, `expect` | NOT FOUND | NOT FOUND |
| `unwrap_or`, `unwrap_or_default` | NOT FOUND | NOT FOUND |
| `ok_or` | n/a | NOT FOUND |
| `is_ok`, `is_some` | NOT FOUND | NOT FOUND |
| pattern destructure helpers | enum patterns only | enum patterns only |

Canonical propagation idiom is `guard let`, not `?`. Representative recipe:

```cplus
guard let result::Result[net::TcpStream, result::IoError]::Ok(s) = net::connect_tcp("127.0.0.1", port)
    else { return 0 -% 1 as i32; };
```

Evidence: `docs/examples/recipes/async_fetch/src/main.cplus:52-57`; broader search found the same shape in `docs/examples/projects/stdlib_smoke/src/main.cplus:15-22` and `vendor/json/src/json.cplus:731-743`.

Context/wrapping support: NOT FOUND for source chaining, message attachment, `anyhow`-style uniform error, or boxed error. Search hits are plain enum errors such as `IoError` and domain enums; package docs say errors are tagged-union values at `SKILL.md:298-327`.

Res-xor-err analysis: fallible stdlib APIs return one tagged union value, e.g. `HashMap::get -> Result[V, IoError]` at `vendor/stdlib/src/hash_map.cplus:122-145`, `fs::open_read -> Result[File, IoError]` at `vendor/stdlib/src/fs.cplus:32-45`. There is no separate `(res, err)` pair in the probed APIs, so the Go-style "both set/neither set" ambiguity is not the stdlib channel.

## T5 - Panic / Abort / Trap Escape Hatches

| Mechanism | Safe reachability | Token cost | Runtime behavior/evidence |
|---|---:|---:|---|
| `assert EXPR;` | yes | `assert false;` | parser has `Assert` token at `cplus-core/src/lexer.rs:75`; parser statement at `cplus-core/src/parser.rs:1633-1639`; codegen traps at `cplus-core/src/codegen.rs:7500-7525` |
| arithmetic overflow in debug | yes | `x + 1` | build/run `/tmp/cpx/cases/t5_overflow.cplus:1`; runtime `Trace/BPT trap: 5`, exit `133` |
| divide by zero | yes | `1 / 0` | build/run `/tmp/cpx/cases/t5_divzero.cplus:1`; runtime `Trace/BPT trap: 5`, exit `133` |
| array OOB | yes | `a[2]` | build/run `/tmp/cpx/cases/t5_oob.cplus:1`; runtime `Trace/BPT trap: 5`, exit `133`; bounds-check trap lowering at `cplus-core/src/codegen.rs:8280-8289` |
| direct `panic`/`abort`/`unreachable` function | NOT FOUND as C+ API | n/a | tree search found Rust internals/docs, not a public C+ panic API |
| `Box::unwrap` | safe method | `.unwrap()` | exists only for `Box[T]`, not `Result`/`Option`; `vendor/stdlib/src/box.cplus:55-70` |

Trap command:

```bash
cpc /tmp/cpx/cases/t5_overflow.cplus -o /tmp/cpx/t5_overflow && /tmp/cpx/t5_overflow
```

Trimmed output for overflow/div-zero/OOB/assert was the same shape:

```text
Trace/BPT trap: 5
run_status=133
```

Assessment: bailing out is very cheap for invariants (`assert false;`) and implicit runtime checks, but there is no named `panic()` convenience API for fallible flow.

## T6 - Ownership & Borrow Checker in Practice

Borrow checker module: `cplus-core/src/borrowck.rs`, with `CopyOracle` and error-family docs at `cplus-core/src/borrowck.rs:258-289`, `cplus-core/src/borrowck.rs:3571-3590`.

Confirmation table:

| Claim | Result | Evidence |
|---|---:|---|
| non-Copy move-by-default | confirmed | `/tmp/cpx/cases/t6_move_default.cplus:1-2`; output `E0335 use of moved value s` |
| structural `Copy` | confirmed | `/tmp/cpx/cases/t6_copy_struct.cplus:1-3`; `status=0`; `CopyOracle` marks types structural at `cplus-core/src/borrowck.rs:331-360` |
| `fn drop(mut self)` forces non-Copy | confirmed | `/tmp/cpx/cases/t6_drop_noncopy.cplus:1-4`; output `E0335`; code path at `cplus-core/src/borrowck.rs:317-324` |
| `borrow` param can reborrow across function call | confirmed | `/tmp/cpx/cases/t6_reborrow.cplus:1-5`; `status=0` |
| `borrow self` receiver | DRIFT | parser rejects it; see T2 |

Edge probes:

| Edge | Result | Evidence |
|---|---|---|
| Return owner and view into it from same fn | accepted | `/tmp/cpx/cases/t6_return_owner_view.cplus:1-4`; `cpc check` returned `status=0` |
| Reborrow across function-call boundary | accepted | `/tmp/cpx/cases/t6_reborrow.cplus:1-5`; `status=0` |
| E0370-family overlapping borrow | visible compiler result is E0335, not E0370 | `/tmp/cpx/cases/t6_e0370_candidate.cplus:1-5`; output `error[E0335]: use of moved value y` |

The codebase still contains borrowck unit tests expecting E0370 internally (`cplus-core/src/borrowck.rs:3573-3588`), but e2e tests explicitly accept E0335 or E0370 because default-move changed the surface (`cpc/tests/e2e.rs:843-883`). Therefore the "fix is always a `{}` scope" claim in `SKILL.md:281` is too strong: some surfaced issues are ownership restructuring, not just scope shortening.

## T7 - Stringly-Typed Fallback Availability

String API ground truth:

| Operation | Exists? | Evidence |
|---|---:|---|
| `str.to_string()` | yes | blessed for `Ty::Str` at `cplus-core/src/sema.rs:7025-7041`, `cplus-core/src/sema.rs:7510-7527` |
| primitive `.to_string()` | yes | same blessed receiver list at `cplus-core/src/sema.rs:7510-7527` |
| `string::new`, `string::with_capacity` | yes | `cplus-core/src/sema.rs:8374-8423` |
| `string.len`, `is_empty`, `as_str`, `clone` | yes | `cplus-core/src/sema.rs:8323-8371` |
| concat | NOT FOUND | no string `+`; design explicitly says string concat was not added at `docs/design/phase8-string-interpolation.md:161` |
| split/search/slice/parse | NOT FOUND as stdlib string methods | recipes do manual pointer/length parsing, e.g. HTTP split loop at `docs/examples/recipes/http_get/src/main.cplus:196-227` |
| interpolation | yes, but `${x}` | `/tmp/cpx/cases/t2_interp_dollar2.cplus:1`; lexer docs at `cplus-core/src/lexer.rs:49-56` |
| format specifiers | absent | `/tmp/cpx/cases/t2_format_spec.cplus:1` failed; design says no format specifiers at `docs/design/phase8-string-interpolation.md:36` |

`HashMap[str, V]` exists and is easy: `HashMap[K,V]` plus `new`, `insert`, `get`, `contains_key`; keys use `hash`/`eq`. Evidence: `vendor/stdlib/src/hash_map.cplus:60-81`, `vendor/stdlib/src/hash_map.cplus:90-153`; examples use string keys at `docs/examples/projects/stdlib_smoke/src/main.cplus:15-25`.

Tuple types and tuple literals exist, so anonymous aggregates are possible: parser tuple type at `cplus-core/src/parser.rs:1250-1284`, tuple literal at `cplus-core/src/parser.rs:2749-2770`. There is no anonymous record syntax found; named records require `struct`.

Judgment: "just strings/maps" is possible for bags of fields because `HashMap[str,V]` is real, but string manipulation is sparse enough that serious parsing usually pushes toward named structs or manual pointer logic rather than rich stringly helpers.

## T8 - Concurrency Primitives & Arc<Mutex<>> Reflex

API inventory:

| Module | Surface |
|---|---|
| `thread` | `spawn[O](fn() -> O)`, `spawn_with[I,O](input, fn(I)->O)`, `JoinHandle[O].join(move self)`; `vendor/stdlib/src/thread.cplus:44-87` |
| `atomic` | `Ordering` plus load/store/swap/fetch/add/sub/bitops/CAS for i32/i64/u32/u64; `vendor/stdlib/src/atomic.cplus:21-325` |
| `mutex` | `Mutex[T]`, `MutexGuard[T]`, `new`, `clone`, `lock`, `strong_count`, guard `get`/`set`; `vendor/stdlib/src/mutex.cplus:35-52`, `vendor/stdlib/src/mutex.cplus:68-147` |
| `channel` | `Channel[T]`, `RecvResult[T]`, `new`, `clone`, `send`, `recv`, `close`, `strong_count`; `vendor/stdlib/src/channel.cplus:50-58`, `vendor/stdlib/src/channel.cplus:76-210` |
| `arc` | `Arc[T]`, `new`, `clone`, `get`, `strong_count`; `vendor/stdlib/src/arc.cplus:31-99` |
| `rc` | `Rc[T]`, `new`, `clone`, `get`, `strong_count`; `vendor/stdlib/src/rc.cplus:19-70` |
| `box` | `Box[T]`, `new`, `get`, `set`, `unwrap`; `vendor/stdlib/src/box.cplus:22-70` |

Canonical concurrency idiom: partition + join. Quote:

```cplus
let h1: thread::JoinHandle[i64] = thread::spawn_with::[Range, i64](left, sum_range);
let h2: thread::JoinHandle[i64] = thread::spawn_with::[Range, i64](right, sum_range);
let total: i64 = h1.join() +% h2.join();
```

Evidence: `docs/examples/recipes/parallel_sum/src/main.cplus:25-33`; the file explicitly says "No shared memory" at `docs/examples/recipes/parallel_sum/src/main.cplus:3-9`.

Shared mutable state exists, but the documented easy path is not `Arc[Mutex[T]]`: `Mutex` internally collapses the refcount into itself because C+ lacks `&T`, documented at `vendor/stdlib/src/mutex.cplus:8-19`. The shared-state recipe uses raw pointer + atomics and says to avoid it first (`docs/examples/recipes/concurrent_counter/src/main.cplus:12-18`).

E0900 confirmation: `/tmp/cpx/cases/e0900.cplus:1-2`; command `cpc check /tmp/cpx/cases/e0900.cplus` emitted `E0900 parameter s has borrow-shaped type str...`. One-line note: the language makes partition+join easiest; shared locks are available but less idiomatic than the recipe path.

## T9 - Stdlib & Vendor Breadth + Dependency Model

Stdlib modules present:

| Module | Types | Public fns | Public methods |
|---|---:|---:|---:|
| arc | 1 | 1 | 3 |
| atomic | 1 | 28 | 0 |
| box | 1 | 1 | 3 |
| channel | 2 | 1 | 5 |
| cow | 1 | 5 | 0 |
| env | 0 | 4 | 0 |
| executor | 0 | 2 | 0 |
| fs | 1 | 2 | 4 |
| future | 2 | 0 | 0 |
| hash_map | 1 | 2 | 5 |
| io | 0 | 3 | 0 |
| iterator | 1 | 0 | 0 |
| marker | 0 | 0 | 0 |
| mutex | 2 | 1 | 5 |
| net | 2 | 3 | 8 |
| option | 1 | 1 | 0 |
| range | 0 | 0 | 0 |
| rc | 1 | 1 | 3 |
| reactor | 0 | 20 | 0 |
| result | 2 | 4 | 0 |
| thread | 1 | 2 | 1 |
| time | 0 | 0 | 0 |
| vec | 1 | 3 | 10 |

All SKILL §8 modules (`SKILL.md:397-419`) are present as files under `vendor/stdlib/src`. `marker`, `range`, and `time` are mostly documentation/import shims, not public API surfaces.

Vendor packages:

| Package | Files | Types | Public fns | Public methods | Declared deps |
|---|---:|---:|---:|---:|---|
| accelerate | 3 | 4 | 26 | 0 | stdlib |
| appkit | 16 | 158 | 61 | 359 | none |
| arena | 1 | 2 | 1 | 10 | stdlib |
| clap | 1 | 3 | 2 | 17 | stdlib |
| json | 1 | 7 | 12 | 11 | stdlib |
| log | 1 | 2 | 8 | 0 | none |
| metal | 7 | 25 | 21 | 23 | stdlib |
| simd | 3 | 3 | 0 | 52 | none |
| static-arena | 1 | 2 | 0 | 16 | stdlib |
| uuid | 1 | 1 | 0 | 3 | stdlib |

Dependency model: `Cplus.toml` dependencies are parsed as name/version pairs, but resolution is presence-check only: `vendor/<name>/Cplus.toml` must exist and semver is future work. Evidence: `cplus-core/src/manifest.rs:43-48`, `cplus-core/src/manifest.rs:94-98`. Design says no package manager, no lockfile, and transitive C+ deps are ignored by `cpc`/flattened by the agent: `docs/design/phase2-packages-mvp.md:13-23`, `docs/design/phase2-packages-mvp.md:143-145`. Stability guarantee: diagnostics JSON and error codes are stable (`docs/design/diagnostics.md:144-150`); v1 package stability beyond design notes is NOT FOUND.

## T10 - "One Obvious Way" / Redundancy Audit

Loops:

| Job | Ways | Distinct or sugar | Evidence |
|---|---|---|---|
| conditional repetition | `while`, `loop` + `break` | distinct | parser statements at `cplus-core/src/parser.rs:1708-1760` |
| counted iteration | `for i in 0..n`, C-style `for` | distinct | parser has `ForLoop::CStyle` and `ForLoop::Range` at `cplus-core/src/ast.rs:584-600`; parser lowers both at `cplus-core/src/lower.rs:135-160` |
| pattern repetition | `while let` | sugar/lowered | lower pass rewrites `WhileLet` at `cplus-core/src/lower.rs:187-219` |
| array iteration | claimed, not real | DRIFT | `/tmp/cpx/cases/t2_control.cplus:7-8` failed E0312 |

Redundancy catalog:

| Job | Ways | Risk |
|---|---|---|
| fallible extraction | `match`, `if let`, `guard let` | truly distinct syntax; generated code can vary widely; `guard let` dominates recipes |
| result construction | enum variants directly, `result::ok`, `result::io_ok` | small redundancy; constructors require turbofish (`vendor/stdlib/src/result.cplus:25-43`) |
| optional construction | `Option[T]::Some(v)` or `option::some::[T](v)` | small redundancy; `None` direct only (`vendor/stdlib/src/option.cplus:17-21`) |
| strings | `str`, `string`, `CowStr` | distinct ownership types; conversion friction is explicit (`vendor/stdlib/src/cow.cplus:33-72`) |
| heap ownership | `Box`, `Rc`, `Arc`, `Mutex` internal refcount | truly distinct, but `Mutex` avoids literal `Arc<Mutex<T>>` by design (`vendor/stdlib/src/mutex.cplus:8-19`) |
| intrinsics | old docs mention include-style, current parser uses `#name` | DRIFT/inconsistency risk; AST notes replacement at `cplus-core/src/ast.rs:794-809` |

## T11 - Compile-Time & Token Density

Timing table:

| File | Chars | Est tokens | `cpc check` | debug build | release build | rebuild after `touch` |
|---|---:|---:|---:|---:|---:|---:|
| `docs/examples/factorial.cplus` | 157 | 39 | 0.57s | 0.13s | 0.14s | 0.08s |
| `docs/examples/phase7_generics.cplus` | 2,542 | 636 | 0.00s | 0.08s | 0.06s | 0.05s |
| `docs/examples/phase11_vec_generic.cplus` | 3,701 | 925 | 0.00s | 0.05s | 0.06s | 0.07s |

Command shape:

```bash
/usr/bin/time -p cpc check FILE
/usr/bin/time -p cpc FILE -o /tmp/cpx/name_dbg
/usr/bin/time -p cpc --release FILE -o /tmp/cpx/name_rel
```

Token-heavy required syntax: source-level enum type args such as `Maybe[i32]::Some` (`SKILL.md:159-167`), turbofish `::[T]` (`SKILL.md:197-205`), explicit `return` (`SKILL.md:71`, compiler E0333 at `/tmp/cpx/cases/e0333.cplus:1`), and fully qualified import aliases (`SKILL.md:13-20`).

JSON diagnostic shape and size:

```bash
cpc --diagnostics=json check /tmp/cpx/cases/e0302.cplus > /tmp/cpx/e0302.json
wc -c /tmp/cpx/e0302.json
```

Output:

```text
214 /tmp/cpx/e0302.json
{"severity":"error","code":"E0302","message":"type mismatch: expected `i32`, found `bool`","primary":{...}}
```

The JSON model has `severity`, `code`, `message`, `primary`, and optional `labels`/`notes`/`suggestions` at `cplus-core/src/diagnostics.rs:61-73`.

## T12 - Existing Test / Eval Infrastructure

Rust test inventory from `cargo test --workspace -- --list`:

```text
0 tests
406 tests
4 tests
0 tests
11 tests
1095 tests
0 doc-tests
```

Total Rust tests listed: 1,516. Vendor in-package C+ test functions found by `rg '^fn test_' vendor`: 95. `SKILL.md` says vendor packages ship in-package tests at `SKILL.md:437`.

Negative-test pattern:

```rust
let out = Command::new(cpc)
    .arg(&src)
    .arg("-o")
    .arg(&bin)
    .output()
    .expect("invoke cpc");
assert!(!out.status.success(), "expected compile failure ...");
let stderr = String::from_utf8_lossy(&out.stderr);
assert!(stderr.contains("E0335") || stderr.contains("E0370"), ...);
```

Evidence: `cpc/tests/e2e.rs:868-883`; another targeted E0302 pattern at `cpc/tests/e2e.rs:10181-10191`.

Fuzzing / snapshot / LLM-generation eval: NOT FOUND for active fuzz or LLM eval. `docs/design/diagnostics.md:162-168` mentions a planned/frozen snapshot test shape, but no active golden/snapshot harness file was found by filename search.

Available seams for an automated source-to-result loop: `cpc check FILE`, `cpc FILE -o OUT`, `cpc build`, `cpc test [FILE] [--json]`, and `--diagnostics=json` are documented in CLI help and `SKILL.md:541-555`. Exit status is used throughout e2e tests, e.g. `cpc/tests/e2e.rs:824-839`.

## T13 - Diagnostics Quality Sample

| Requested error | Human message | JSON span? | Suggestion? | Fix-it? | Evidence |
|---|---|---:|---:|---:|---|
| E0302 | `type mismatch: expected i32, found bool` | yes | no | no | `/tmp/cpx/cases/e0302.cplus:1`; JSON primary span present |
| E0335 | `use of moved value s` | yes | no | no | `/tmp/cpx/cases/e0335.cplus:1-2`; emitted twice |
| E0340 | `non-exhaustive match on enum E: missing variant(s) B` | yes | no | no | `/tmp/cpx/cases/e0340.cplus:1-3` |
| E0370 | NOT SURFACED; same probe emits E0335 | yes for E0335 | no | no | `/tmp/cpx/cases/e0370_request.cplus:1-5`; e2e comments explain drift at `cpc/tests/e2e.rs:843-846` |
| E0801 | `integer-to-pointer cast requires unsafe { ... }` | yes | no | no | `/tmp/cpx/cases/e0801.cplus:1` |
| E0500 | `cannot infer type parameter T for call to make; supply ::[T]...` | yes | textual only | no | `/tmp/cpx/cases/e0500.cplus:1-2` |
| E0900 | `parameter s has borrow-shaped type str... Use an owned type instead` | yes | textual only | no | `/tmp/cpx/cases/e0900.cplus:1-2` |
| E0333 | `function body cannot end with an implicit tail expression; use return ...; instead` | yes | textual only | no | `/tmp/cpx/cases/e0333.cplus:1` |

JSON is NDJSON and stable by design (`docs/design/diagnostics.md:144-150`). The data model supports suggestions and machine applicability (`cplus-core/src/diagnostics.rs:44-59`), but these sampled errors mostly did not populate structured `suggestions`; the actionable fix is embedded in prose. Overall: spans and stable codes are agent-friendly; missing machine-applicable fix-its and duplicate E0335 emissions reduce automated repair quality.

## Wrap-Up

Top 5 DRIFTs:

1. SKILL says interpolation is `\{x}`; compiler accepts `${x}` (`SKILL.md:224-230`, `cplus-core/src/lexer.rs:49-56`, `/tmp/cpx/cases/t2_interp_dollar2.cplus:1`).
2. SKILL says `for v in arr`; compiler rejects arrays in `for ... in` with E0312 (`SKILL.md:140-142`, `/tmp/cpx/cases/t2_control.cplus:7-8`).
3. SKILL says receiver `borrow self`; parser rejects it (`SKILL.md:246`, `/tmp/cpx/cases/t2_struct_receivers.cplus:7`).
4. SKILL uses struct shorthand `Point { x, y }`; compiler required explicit fields (`SKILL.md:151`, `/tmp/cpx/cases/t2_struct_receivers.cplus:3` fixed to `x: x, y: y`).
5. Borrow docs imply E0370-family as the visible overlapping-borrow diagnostic, but current e2e accepts E0335 for those cases (`SKILL.md:496`, `cpc/tests/e2e.rs:843-883`).

Top 5 NOT FOUND gaps:

1. `Result`/`Option` combinators (`map`, `and_then`, `map_err`, `ok_or`, `is_ok`, `is_some`) are absent (`vendor/stdlib/src/result.cplus:3-43`, `vendor/stdlib/src/option.cplus:12-21`).
2. Uniform/boxed error or `anyhow` analog absent.
3. Error context/wrapping/source chaining absent.
4. Public `panic`/`abort` API absent; only traps/assert/runtime checks found.
5. Package registry, lockfile, and compiler-managed transitive deps absent by design (`docs/design/phase2-packages-mvp.md:143-145`).

Surprising items bearing on "a language for LLMs to write":

- The compiler accepts returning an owner and a `str` view into it in the same tuple (`/tmp/cpx/cases/t6_return_owner_view.cplus:1-4`). That is important because it looks like the BlackFly-style self-referential edge the borrow model is expected to reject.
- `cpc check FILE` does not read the project manifest; a file with `import "stdlib/io"` failed with E0852 outside `cpc build`, even inside a project directory. That matters for eval harness design.
- The diagnostics infrastructure is strong enough for stable codes and spans, but most high-value sampled diagnostics did not include structured fix-its, even when prose suggested the exact repair.
