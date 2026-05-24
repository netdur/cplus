# v0.0.12 — open

## Bugs

- **G-022** — E0333 diagnostic suggests `return ...;` even when the function returns `()` and the tail block is unit-typed; should suggest `};`. Surfaced by [llama.cplus](../llama.cplus/cpc-gaps.md).
- **G-023** — negative integer literals (`let x: i64 = -100;`) fail with E0302; LHS type annotation isn't propagated through unary-minus. Fix: magnitude-based promotion on negated literals, mirroring the positive-literal path. Surfaced by [llama.cplus](../llama.cplus/cpc-gaps.md).
