# v0.0.12 — open

## Bugs

> **Numbering note.** Gap IDs G-022..G-026 in this file are the cpc-side internal names assigned as we shipped fixes. They line up with [llama.cplus/cpc-gaps.md](../llama.cplus/cpc-gaps.md) G-022..G-025 exactly. The exception is G-026: on the cpc side, G-026 was the unit-type-in-turbofish fix (we picked the next free ID locally); on the llama.cplus side, G-026 is the partial-struct-init / zero-fill gap (they picked the next free ID independently). The two projects collided on the slot. Going forward, llama.cplus is the canonical gap log — our G-028 here corresponds to their G-026, and our G-027 corresponds to their G-027. See the handoff note at the bottom.

- **G-022** — ✅ `4067546` — E0333 diagnostic suggests `};` when the function returns `()` and the tail is unit-typed; `return ...;` only when an actual value is being abandoned.
- **G-023** — ✅ `4067546` — `let x: i64 = -100;` works the same as `let x: i64 = 100;`. Expected type propagates through unary-minus; codegen const-folds `-LIT` so it flows as a textual constant at any width.
- **G-024** — ✅ `621633a` — `*T.is_null()` / `*T.is_not_null()` builtin methods on raw pointers. Single `icmp eq/ne ptr p, null` lowering; safe (no memory access).
- **G-025** — ✅ `621633a` — `#addr_of` accepts any place expression — `Ident`, `Field`, `Index`, `Deref`, and chains. Unblocks the llama.cplus gallocr port. Codegen rides existing `gen_place`.
- **G-026** (cpc-internal — collides with llama.cplus G-026 but unrelated) — ✅ `1745cb2` — `()` parses as the unit type wherever `parse_type` runs (turbofish, fn-pointer return, fn return), resolves to `Ty::Unit`, mangles to `unit`, and round-trips through monomorphization. Parse errors on the entry file now render with a real span instead of `1:1`. Unblocks `spawn_with::[I, ()]` for unit-returning workers.
- **G-027** — ✅ `extern fn` returning a >16-byte aggregate now emits sret on both the import declaration AND every call site, matching the AArch64-Darwin (and x86_64-sysv) C ABI. Pre-fix: cpc declared `declare %T @f(...)` and called direct, while clang on the C side emitted `void @f(ptr sret(%T), ...)` → silent ABI mismatch → SIGSEGV on first call. Cross-language e2e test exercises the case end-to-end.
- **G-028** (= llama.cplus G-026) — ✅ Two complementary explicit-zero primitives, closing the C99 partial-init silent-garbage gap that caught a real bug in `ggml_dyn_tallocr_new`:
  - `#zero::[T]() -> T` — a value of type `T` with every byte zeroed. Safe; alloca + memset + load.
  - `*T.write_zeroed()` — zero the T-many bytes a raw pointer refers to. Unsafe (raw-pointer write), gated by E0801.
