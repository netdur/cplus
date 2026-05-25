# v0.0.12 — open

## Bugs

> **Numbering note.** llama.cplus is the canonical gap log; cpc-internal IDs occasionally diverge when we picked the next-free slot before seeing their entry. Current alignment:
>
> | cpc-side ID | llama.cplus ID | What |
> |---|---|---|
> | G-022..G-025 | G-022..G-025 | aligned |
> | G-026 | (no equivalent) | unit-type-in-turbofish — cpc-internal |
> | G-027 | G-027 | aligned (sret) |
> | G-028 | G-026 | zero-fill — collided on slot |
> | G-029 | G-028 | `--emit-obj` reads `Cplus.toml` |

- **G-022** — ✅ `4067546` — E0333 diagnostic suggests `};` when the function returns `()` and the tail is unit-typed; `return ...;` only when an actual value is being abandoned.
- **G-023** — ✅ `4067546` — `let x: i64 = -100;` works the same as `let x: i64 = 100;`. Expected type propagates through unary-minus; codegen const-folds `-LIT` so it flows as a textual constant at any width.
- **G-024** — ✅ `621633a` — `*T.is_null()` / `*T.is_not_null()` builtin methods on raw pointers. Single `icmp eq/ne ptr p, null` lowering; safe (no memory access).
- **G-025** — ✅ `621633a` — `#addr_of` accepts any place expression — `Ident`, `Field`, `Index`, `Deref`, and chains. Unblocks the llama.cplus gallocr port. Codegen rides existing `gen_place`.
- **G-026** (cpc-internal — collides with llama.cplus G-026 but unrelated) — ✅ `1745cb2` — `()` parses as the unit type wherever `parse_type` runs (turbofish, fn-pointer return, fn return), resolves to `Ty::Unit`, mangles to `unit`, and round-trips through monomorphization. Parse errors on the entry file now render with a real span instead of `1:1`. Unblocks `spawn_with::[I, ()]` for unit-returning workers.
- **G-027** — ✅ `extern fn` returning a >16-byte aggregate now emits sret on both the import declaration AND every call site, matching the AArch64-Darwin (and x86_64-sysv) C ABI. Pre-fix: cpc declared `declare %T @f(...)` and called direct, while clang on the C side emitted `void @f(ptr sret(%T), ...)` → silent ABI mismatch → SIGSEGV on first call. Cross-language e2e test exercises the case end-to-end.
- **G-028** (= llama.cplus G-026) — ✅ `5d23203` — Two complementary explicit-zero primitives, closing the C99 partial-init silent-garbage gap that caught a real bug in `ggml_dyn_tallocr_new`:
  - `#zero::[T]() -> T` — a value of type `T` with every byte zeroed. Safe; alloca + memset + load.
  - `*T.write_zeroed()` — zero the T-many bytes a raw pointer refers to. Unsafe (raw-pointer write), gated by E0801.
- **G-029** (= llama.cplus G-028) — ✅ `cpc --emit-obj FILE` (and the rest of the single-file driver paths) now walks up from `FILE`'s directory looking for `Cplus.toml`. If found, the project's `[dependencies]` flow to the resolver so `import "stdlib/atomic"` resolves correctly under the CMake `add_custom_command` shape. Unblocks per-file invocations from external build systems (their llama.cplus port couldn't import any stdlib module from CMake before this). No new flag; existing `cpc --emit-obj` invocations without a reachable manifest still behave the same.
