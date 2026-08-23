# The C+ guide

How to think in C+, and which construct to reach for when more than one
would compile. For a first sitting read [tour.md](tour.md); for exact shapes,
[ref.md](ref.md); for the rules themselves, [spec.md](spec.md) and
[memory-model.md](memory-model.md).

The guide's spine is on this page. These topics are large enough to stand
alone:

| Topic | File | The decision it settles |
|---|---|---|
| Ownership | [ownership.md](ownership.md) | who owns a value, and how it travels |
| Errors | [error-handling.md](error-handling.md) | what a fallible function returns |
| Packages | [packages.md](packages.md) | how a project grows past one file |
| Platforms | [platforms.md](platforms.md) | where a per-OS difference belongs: a file, a dependency, or a value |
| Concurrency | [concurrency.md](concurrency.md) | threads, async, generators — which shape the work wants |
| FFI | [ffi.md](ffi.md) | what crossing the C boundary costs and promises |
| Standard library | [stdlib.md](stdlib.md) | which module answers a need |
| Testing | [testing.md](testing.md) | what to write, where it runs, what it cannot check |
| Tooling | [tooling.md](tooling.md) | which command answers the question you have |

## 1. How to think in C+

**Everything is a value, and every value has one owner.** A struct is bytes,
assignment of a Copy struct is a copy, and an owning value (`Text`, `Vec`, a
struct containing one) moves rather than copies. When the owner's scope
ends, the value frees itself. That one rule replaces the garbage collector,
`unique_ptr`, and most of the discipline C asks of you.

**Everything is explicit, so everything is visible.** No implicit
conversions, no overloading, no exceptions, no closures capturing who knows
what, no macros rewriting your code. The cost is a few more tokens per line
(`as`, `return`, named fields); what you buy is that any line means what it
says without reading the rest of the file.

**Danger is visible, not forbidden.** C+ wears some of Rust's vocabulary but
keeps C's semantics — verify against this compiler, not against Rust
reflexes. There is no `unsafe` block, because every dangerous operation is
already loud at the point of use: `*p` and `p[i]` are the only derefs,
`x as *T` is the only way to make a pointer, `extern fn` is the marker for a
foreign call. The safe subset is checked; the raw tier is spelled out.

**When the compiler and your intuition disagree, run `cpc explain Exxxx`.**
The diagnostics are the designed teaching surface; this guide covers the
places where an error message alone can't tell you which redesign to pick.

## 2. Which parameter mode

The complete decision, in signature order of preference:

| The callee needs to… | Write | Caller sees |
|---|---|---|
| read the value | `x: T` (bare) | keeps it |
| mutate the caller's value in place | `ref x: T` | must pass a `var`; sees the writes |
| keep, store, return, or destroy it | `take x: T` | loses it at the call |

Bare is the default for a reason: it is a borrow for *every* type, including
big structs — there is no "pass by value copies" tax to route around. Reach
for `take` only when the value must escape the callee (stored in a field,
returned, appended into a container): a bare borrow that escapes is E0337,
and the fix the message offers — `take` or `.clone()` — is a real fork:
`take` when the caller is done with it, `.clone()` when both sides need one.

Receivers are the same choice spelled `this` / `ref this` / `take this`.
When in doubt between `ref this` and returning a new value, prefer `ref
this` — the house style mutates in place (`set_*`, `append`) and reserves
returns for reads. The full model, including views and drop order, is
[ownership.md](ownership.md).

## 3. Which string

| You have / need | Use |
|---|---|
| a literal, a parameter, any read | `str` — the borrowed view; all read methods live here |
| a field, a return you own, building at runtime | `text::Text` — `s.to_text()` / `text::from_str(s)` |
| bytes for C | `c"..."` literal, or `#str_ptr(s)` + `#str_len(s)` |

Two habits prevent every string bug worth having:

- **Views borrow.** `t.trim()` is a window into `t`'s buffer, not a copy.
  While the view lives, `t` is write-locked; the borrow ends at the view's
  last use. A view bound at a `let` needs a *named* owner — `let s: str =
  t.clone();` dies with E0513 because the clone was a temporary. Name the
  owner first.
- **Copies are spelled.** `.to_text()` is the only string copy you ever pay,
  and it appears in the source. If you never write it, you never allocate.

There is no `+` concatenation: interpolate (`"x = ${n}"`) or `append`.
Interpolation straight into `io::println` allocates nothing; bound to a
variable it builds a `Text`.

## 4. Which error shape

The house convention, in one table (rationale and patterns in
[error-handling.md](error-handling.md)):

| The operation is… | Return | Consumed with |
|---|---|---|
| a mutator that can fail (append, insert, reserve) | `status::Status` | `if !s.is_ok() { … }` or bind `_` when the failure mode is OOM-only |
| a read that can miss (lookup, parse, index) | `Option[T]` | `match` / `guard let` |
| a computation with a value *and* a reason | `Result[T, E]` or your own enum | `match` / `guard let … else \|Err(e)\|` |

No panic exists. `assert` traps — use it for contract violations that mean
the program itself is wrong, never for input.

## 5. Which container

| Need | Use | Note |
|---|---|---|
| fixed count, known at compile time | `[T; N]` | stack; bounds-checked; index with `0..n` |
| growable sequence | `vec::Vec[T]` | mutators return `Status`; reads return `Option` |
| borrowed window over either | `T[]` (slice) | via `Vec::as_slice` — arrays do NOT coerce |
| keyed lookup | `hash_map::HashMap[K, V]` | `K: Hash + Eq` — derive both with empty impls |
| one heap value, one owner | `box::Box[T]` | stable address; `unwrap()` moves it back out |
| shared ownership, one thread | `rc::Rc[T]` | `with_mut` gate for mutation |
| shared ownership, across threads | `arc::Arc[T]` | `Send + Sync` iff `T` is |
| back-pointer that must not keep alive | `rc::Weak` / `arc::Weak` | `upgrade() -> Option` |

The order is deliberate: prefer the top. Most designs that reach for
`Rc`/`Arc` early actually wanted a clearer owner.

## 6. Concurrency, in order of preference

1. **Partition + join.** `thread::spawn_with` moves disjoint data into each
   worker; `join()` returns the results. No sharing, no race, no locks.
2. **Scoped lend.** `thread::scope()` + `s.lend::[T](value, worker)` lends a
   local by `ref` and joins before the scope ends — the borrow checker
   enforces the join.
3. **Channels** (`stdlib/channel`) when the shape is a pipeline.
4. **`Arc` + `Mutex[T]`** last. Two guards in one scope deadlock — scope
   each lock.

`async fn` + `executor::block_on` exist for I/O-bound work over the
platform's reactor (kqueue on Darwin, epoll on Linux and Android).
Borrow-shaped types (`str`, slices, `ref` params) are rejected in `async fn`
signatures (E0900) — pass owned `Text` / `Vec`. The entry point is always a
plain `fn main` that calls `block_on`. Cancellation, scoped lends, channels,
and the async↔thread bridge are [concurrency.md](concurrency.md).

## 7. Callbacks without closures

A stateful callback is a **pair**: a named `fn` and a `*u8` context. Declare
the pair adjacently with the context defaulted, and callers can pass a bound
method with zero ceremony:

```cplus
fn row(on_click: fn(str, *u8) = 0 as fn(str, *u8),
       on_click_ctx: *u8 = 0 as *u8) { }

row(on_click: this.open_project)      // compiler fills the ctx slot
```

The one mistake that matters: **declaring the handler without the `_ctx`
slot**. Callers then can never pass a method, and the error fires at *their*
call site where it can't be fixed (W0824 warns you at the declaration —
heed it). Same rule for struct fields that store handlers: store the `*u8`
beside the fn-pointer.

## 8. The traps that cost hours

Each of these is legal-looking code with a surprise in it. The long
explanations live in the linked topic file.

- **Integer literals evaluate through `i32` before `as`.**
  `(1 << 40) as u64` is not the mask you meant — it is 256. Widen the LEFT
  operand before the shift (`1u64 << 40`), or build the mask in a `const`,
  which folds at the declared width and rejects overflow. A constant shift
  distance at or past the operand's width now warns (W0007); the general
  literal-width rule still has no warning, so the habit still matters.
- **No array→slice coercion.** `[T; N]` does not pass where `T[]` is
  expected; go through a `Vec` and `as_slice`, or index.
- **A view at a binding needs a named owner** — and a brace-block tail
  can *promote* a temporary's lifetime in ways an argument position
  doesn't. When a `str` binding fights you, name the `Text` first.
  ([ownership.md](ownership.md) §views)
- **A consuming `match` is triggered by binding a name.** `Some(_)` reads
  the tag only; `Some(_v)` binds — the leading underscore is privacy, not a
  wildcard — and consumes the value. ([ownership.md](ownership.md) §drop)
- **`take this` does not disarm drop.** Return the payload and let scope
  exit free the shell; freeing manually double-frees.
- **A parenthesized deref opening an `if` misparses** — write
  `if { (*p).field } == x`, braces first. House style does this everywhere.
- **Variadic C functions must be declared with `...`.** A fixed-arity
  declaration of `fcntl` passes garbage silently on AArch64.
  ([ffi.md](ffi.md))
- **Two mutex guards in one scope deadlock.** Scope each lock.
- **`cpc check FILE` cannot see imports.** Any code with an `import` goes
  through `cpc build` / project-mode `cpc check` (E0852 otherwise).
- **`#platform()` cannot gate an import.** Both arms of an `if` on it
  compile on every platform; varying imports is what a `_<platform>.cplus`
  file is for. ([platforms.md](platforms.md))
- **A ` ```cplus ` fence inside a `///` comment is not a doctest.** Only a
  bare three-backtick fence is extracted, so a tagged example silently
  never runs. ([testing.md](testing.md))

## 9. Style that the compiler doesn't enforce

- One blessed way per job; when the language offers a long and a short form
  (generic args in patterns), the short form is preferred where it resolves.
- `_name` is privacy — for fields, methods, and module items. Public is the
  default; make internals invisible rather than documented-as-internal.
- Named arguments read as sentences at call sites (`slice(from: 1, to: 4)`);
  use them whenever a call has two same-typed parameters.
- `cpc fmt` settles layout arguments. If a file doesn't round-trip through
  the formatter, the file is wrong.
