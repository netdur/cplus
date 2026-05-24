# v0.0.12 — open

## Bugs

- **G-022** — ✅ `4067546` — E0333 diagnostic suggests `};` when the function returns `()` and the tail is unit-typed; `return ...;` only when an actual value is being abandoned.
- **G-023** — ✅ `4067546` — `let x: i64 = -100;` works the same as `let x: i64 = 100;`. Expected type propagates through unary-minus; codegen const-folds `-LIT` so it flows as a textual constant at any width.
- **G-024** — ✅ `*T.is_null()` / `*T.is_not_null()` builtin methods on raw pointers. Single `icmp eq/ne ptr p, null` lowering; safe (no memory access).
- **G-025** — ✅ `#addr_of` accepts any place expression — `Ident`, `Field`, `Index`, `Deref`, and chains. Unblocks the llama.cplus gallocr port. Codegen rides existing `gen_place`.
