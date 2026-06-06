# Phase 8.STR.B — string interpolation

Status: design draft, awaiting approval. No code written yet.

## Goal

Let users build readable strings from values without verbose concatenation.
Today the only ergonomic option is `println` on a literal, and even that
can't embed variables. After this slice:

```
let n: i32 = 42;
let name: str = "world";
let greeting: string = "hello ${name}, n is ${n}";
#println(greeting);   // hello world, n is 42
```

## Syntax

Inside a string literal:

- `${expr}` — embeds an arbitrary C+ expression.
- `$$` — literal `$`.
- Everything else after `$` that isn't `{` or `$` is a parse error: **E0611**
  ("`$` must be followed by `{` for interpolation, or `$` for a literal `$`").

**Just one form.** No bare `$name` shortcut. The "function over syntax" + "no
several ways to do the same thing" principles win over the one-character
saving. Dart has both; we pick one.

**Balanced braces inside.** The lexer counts braces so `"${ pair.first }"`
and `"${ f({}) }"` both parse. Nested string literals are not parsed (a `"`
inside `${...}` terminates the lexer state) — embed via a `let` if you
need a nested literal.

**No format specifiers in v1.** No `${n:>5}`, no `${pi:.2}`. The embedded
expression is the *value*; the user pre-formats by calling helpers. We
keep the syntax shape open for future extension without committing now.

## Type rules

Every `${expr}` must produce a type that implements a new blessed interface:

```
interface ToString {
    fn to_string(self) -> string;
}
```

Blessed impls shipped with the slice:

- `str` (identity-ish — copies bytes into a new `string`)
- `string` (clone)
- `i8`, `i16`, `i32`, `i64`, `isize`
- `u8`, `u16`, `u32`, `u64`, `usize`
- `f32`, `f64` (uses `%g`-style formatting)
- `bool` (`"true"` / `"false"`)

User-defined types provide `impl ToString for MyType { ... }`. Same shape
as `impl Copy for MyType {}` from Phase 7.

**Fires E0612** ("type `Foo` does not implement `ToString`") on a use
site where the embedded expression's type lacks an impl. The suggestion
points at the type's declaration site with a quick-fix to insert an
`impl ToString for Foo` stub.

## Owned `string` is part of this slice

`to_string` returns `string` (owned, heap-backed), which means **Phase
8.STR.3 (owned `string`) has to ship alongside this slice.** That's a
larger scope than just interpolation parsing — it's roughly:

- A compiler-blessed `string` type. Internal representation:
  `{ ptr: *u8, len: usize, cap: usize }` — same shape as `Vec[u8]` would
  be. Lowered as a 24-byte fat struct, passed by-value (Phase 2 ABI).
- Heap-backed via libc `malloc/realloc/free` directly (not via
  `interface Allocator` — `string` predates user-pickable allocators and
  parallels what every Phase-11-using language ships). If we want
  allocator-parametric strings later, that's a separate slice.
- Blessed `Drop` impl frees the buffer at scope exit.
- Blessed move semantics; copies are explicit via `s.clone()`.
- Methods: `string::new()`, `string::with_capacity(n)`, `s.len()`,
  `s.is_empty()`, `s.as_str()`, `s.clone()`. No mutation methods in v1 —
  if you need to grow a string, build a `Vec[u8]` and convert at the
  end.

## Desugar

Each interpolated literal lowers to a single intrinsic call:

```
"hello ${name}, n is ${n}"
```

becomes

```
__string_concat(
    [
        StrPart::Lit("hello "),
        StrPart::Owned(name.to_string()),
        StrPart::Lit(", n is "),
        StrPart::Owned(n.to_string()),
    ]
)
```

where `__string_concat` is a compiler intrinsic taking a fixed-size array
of `StrPart` and returning `string`. It computes total length, allocates
once, copies each part in.

**No `+` operator overload anywhere.** This is a function call. The "no
overloading" principle stays intact.

The intrinsic's exact signature is internal — users can never call it by
name. The only surface is the `"...${expr}..."` literal itself.

## Locked-principle check

| Principle | Status |
|---|---|
| No null in safe code | ✓ — `string` and `str` are never null. |
| No closures | ✓ — embedded expr is a plain expression. |
| No operator overloading | ✓ — desugar uses function intrinsic, not `+`. |
| Function over syntax | ✓ — surface is concise, semantics are method calls + intrinsic. |
| No several ways to do same thing | ✓ — only `${expr}` form. No bare `$ident` shortcut. |

## Error codes reserved

- **E0611** — invalid `$` escape (must be `${` or `$$`).
- **E0612** — embedded expression's type doesn't implement `ToString`.
- **E0613** — unterminated `${` (missing closing `}` before string end).

(Phase 8 reserves E0600–E0610; these extend the block.)

## Implementation order

This is *not* one slice. It splits into four:

1. **8.STR.3 — owned `string`.** Compiler-blessed type, libc-backed,
   Drop. Methods: `new`, `with_capacity`, `len`, `is_empty`, `as_str`,
   `clone`. No interpolation yet — users build strings manually for
   testing. ~3 days.
2. **8.STR.6 — `ToString` interface + blessed impls.** Add the
   interface to the blessed-interfaces table; provide impls for every
   primitive listed above. ~1 day.
3. **8.STR.B.1 — lexer + parser for `${...}` and `$$`.** New
   `ExprKind::InterpStr { parts: Vec<InterpPart> }` where
   `InterpPart = Lit(String) | Expr(Expr)`. E0611, E0613. Sema checks
   each embedded expr against `ToString`. ~2 days.
4. **8.STR.B.2 — codegen lowering.** `__string_concat` intrinsic; emits
   single malloc + memcpy chain. ~1 day.

Total: roughly a week. The first three are independently useful even
without interpolation — owned `string` alone unblocks half the deferred
samples.

## Out of scope (explicit)

- Format specifiers (`${n:>5}`, `${pi:.2}`). Future slice if motivated.
- `+` for string concat. Won't add — locked.
- String mutation API (`push`, `push_str`). Build via `Vec[u8]` if needed.
- Allocator-parametric strings. `string` uses libc malloc directly.
- Multi-line / raw string literals. Existing literal lexer already allows
  multi-line; raw strings (`r"..."`) are a separate future ask.
- Localization, ICU, normalization. None of those.

## Open questions

1. **Should `${}` (empty) be allowed?** Currently no — E0613 fires. Locking
   that.
2. **Should `to_string` consume self (move) or borrow (`self`/copy)?**
   Locked as plain `self` — for Copy types it's free; for `string` it
   `clone`s internally. Matches Rust's `Display::fmt(&self, ...)` shape
   without the explicit borrow.
3. **String literal `"hello"` is `str` today. Does `"hello ${n}"` produce
   `string` or `str`?** It produces `string` (owned). A literal with
   *no* interpolations stays `str` (no allocation). Detecting this is a
   parse-time check: zero `${...}` segments → `StrLit`; one or more →
   `InterpStr`.
