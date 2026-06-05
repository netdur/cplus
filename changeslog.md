# C+ changelog

User-facing changes per release, newest first. The changelog starts at v0.0.14;
earlier history lives in each version's archived plan.

## v0.0.14 — 2026-06-05

Language track. The headline themes are the completed ownership/Drop model, inline
assembly, and code-knowledge-graph value depth.

### Ownership & Drop
- **`unsafe impl Send for T {}` / `unsafe impl Sync for T {}`** — a manual marker
  override. A nominal type that transitively hides a raw pointer is now `!Send`
  and `!Sync` by default (moving or sharing it across a `Send`/`Sync` bound is
  rejected, E0502); a bare `*T` used directly stays Send. The override re-enables
  a type you vouch for. `Send`/`Sync` impls must carry `unsafe` (E0860); `unsafe`
  applies only to those markers (E0861). Conditional generic form carries the
  condition as bounds: `unsafe impl Send for Arc[T: Send + Sync] {}`. `Arc`,
  `Mutex`, and `Channel` carry the right conditional impls.
- **`#[no_alloc]` drop-glue** — a `#[no_alloc]` function now also rejects implicit
  destructors run at scope exit that would allocate/free (a `string`/`Vec`/`Box`
  local, or a type whose `drop` is not itself `#[no_alloc]`), reaching through
  fields, enum payloads, and array elements (E0901).
- **Container element drop** — dropping a `Vec[T]` (and Box/Arc/Rc/HashMap) runs
  each element's `drop` exactly once before freeing the buffer.
- **Consumed-enum payload** — matching an owned enum and binding a payload that is
  not moved out now drops it at arm exit (no leak), while every move-out shape
  still disarms the drop (no double-free).

### Inline assembly
- **Tier 2 — operands + clobbers.** Rust-style named operands:
  `#asm("add {s}, {a}, {b}", s = out(reg) sum, a = in(reg) a, b = in(reg) b,
  clobber("cc"))`. `in`/`out`/`inout` set direction; `reg` lets the compiler pick
  a register (then `{name}` must appear in the template) or `"x0"` pins one.
  `out`/`inout` targets must be `mut` variables; operands are register-sized
  scalars.
- **Tier 3 — `#[naked]` functions.** No prologue/epilogue; the body is inline asm
  that handles the ABI and returns itself (E0909 if the body is not asm-only).
  For trampolines, entry stubs, custom-ABI shims.

### Code knowledge graph
- **`type-at` on inferred expressions.** `cpc query type-at FILE:LINE:COL` (and
  LSP hover) now answer call results, field/index reads, arithmetic, and
  `match`/`if` values, not just annotated positions, rendered with concrete names
  (`Result[Value, ParseError]`, `Vec[i32]`).
- **`value-refs`.** `cpc query value-refs FILE:LINE:COL` returns a binding's
  value-flow: its definition plus every use classified as read / call / construct
  (re-wrap) / return / match / assign.
- **LSP dirty-buffer overlay.** Hover, type-at, value-refs, goto-definition,
  references, and document-symbols reflect unsaved editor edits before save.

### Fixes
- **Codegen:** a `match` arm (or other value position) whose body is an `if`
  building a payload-carrying enum constructor no longer discards the value
  (previously a silent miscompile; surfaced by the json package migration).

### Other
- `vendor/json` parser migrated to a match-consumable result enum with recursive
  auto-Drop; accessors borrow and deep-clone.

Deferred to v0.0.15 (additive): module-level global asm, the if-result predictor
refactor, value-refs precise scoping (shadowing), and the package side
(AppKit → agent).
