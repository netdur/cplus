# Error handling — designing without exceptions

The decision this page settles: **what a fallible function returns.** Quick
syntax in [tour.md](tour.md) §5; the shapes' exact signatures in
[ref.md](ref.md).

## 1. The ground rules

There are no exceptions, no `try`/`catch`, no `?`, and **no panic** — the
only hard stop in the language is `assert`, which traps. A fallible
operation returns a tagged union, and the caller takes it apart with
`match`, `if let`, or `guard let`. `Option` and `Result` ship **variants
only**: no `.unwrap()`, `.map()`, `.is_some()`, `.unwrap_or()` — none of the
combinator surface exists, and writing it will not compile. The pattern
match *is* the API.

This is a feature with a sharp edge: every failure path is visible in the
source, and no failure path can be silently rethrown. The cost is that you
choose a return shape per function — this page is that choice.

## 2. The three shapes

| The operation is… | Return | Because |
|---|---|---|
| a **mutator** that can fail | `status::Status` | the caller needs "did it work", not a payload; the receiver is unchanged on failure |
| a **read** that can miss | `Option[T]` | absence is not an error; there is nothing to explain |
| a computation with a value **and a reason** | `Result[T, E]` — or your own enum | the caller branches on *why* |

### `Status` — the mutator's answer

`Ok`, `OutOfMemory`, `OutOfBounds`, `InvalidInput`, `Shared`, plus
`is_ok()`. The contract every stdlib mutator keeps, and yours should too:
**on failure, no change** — the receiver is still valid and untouched.

```cplus
let s: status::Status = v.append(item);
if !s.is_ok() { return s; }              // propagate by returning it
```

When the only failure mode is out-of-memory and the program's answer to OOM
is "die later, not here", binding `_` is the honest spelling:
`let _s: status::Status = v.append(x);` — visible, greppable, deliberate.

### `Option[T]` — the read's answer

`at`, `find`, `to_i64`, `slice(from:, to:)` — a miss is a normal outcome,
so it carries no explanation. Consume with `guard let` when the miss exits,
`match` when both arms do work. Cross-module patterns spell the type
(`option::Option[i64]::Some(v)`).

### `Result[T, E]` and your own enums

`result::Result[T, E]` is `Ok(v)` / `Err(e)` with `result::IoError` as the
stdlib's error payload. But the *house* pattern for a library's fallible core
is a **domain enum** — variants named for what actually happened:

```cplus
enum Parse { Ok(Config), BadKey(Text), Truncated, Overflow }
```

A domain enum beats `Result` when failures differ in what the caller should
do next; `Result[T, IoError]` is right when they don't.

## 3. Consuming: `guard let` is the workhorse

```cplus
fn load(path: str) -> i32 {
    guard let Parse::Ok(cfg) = parse(path) else { return 1; };
    // cfg is bound here; every failure already exited
    return run(cfg);
}
```

- The `else` must diverge: `return`, `break`, `continue`, or a trap.
- When the failure payload matters, take the **complement form** — the else
  receives what the primary pattern didn't match, and the two patterns
  together must cover the enum (E0349):

```cplus
guard let Read::Ok(v) = read(s) else |Read::Err(code)| {
    io::eprintln("read failed: ${code}");
    return 0 -% code;
};
```

- Inside the else, the scrutinee is already consumed — bind the complement;
  re-matching it is E0335 ([ownership.md](ownership.md) §6).

`match` earns its keep when several outcomes each do real work, and
exhaustiveness (E0340) is the tool that makes adding a variant safe: every
site that must care fails to compile until it does.

## 4. Propagating without `?`

There is no rethrow operator, and no error-wrapping machinery (no context
chains, no boxed any-error). Propagation is explicit, and three spellings
cover it:

```cplus
// Same-shape passthrough — return it.
let s: status::Status = v.reserve(n);
if !s.is_ok() { return s; }

// Shape change — convert at the boundary, once.
guard let option::Option[i64]::Some(v) = s.to_i64() else {
    return Parse::Garbled;
};

// Adding context — a variant that CARRIES it.
enum Load { Ok(Config), NoFile(Text), Bad(Text) }   // the payload is the context
```

If a caller five levels up needs to know *which file* failed, the variant
carries the path — context is data in your enum, not an invisible chain.
Design the enum for the caller that handles it, not the site that throws it.

## 5. `assert` — the program-is-wrong stop

`assert cond;` traps on false. It states an invariant of *your code*, never
a judgment about input: index math you just proved, a state machine that
cannot legally be here. If a user, a file, or a network peer can make the
condition false, it is not an assert — it is a returned error.

Contracts generalize this: `#[requires(n > 0)]` on entry,
`#[ensures(result >= n)]` at every return — checked in the same trap-on-
violation spirit, and reported nicely under `cpc test`.

## 6. At the boundaries

- **FFI**: C reports errno-style integers; convert to your domain enum at
  the binding, in one place. Exported functions (`export fn`) return C
  shapes — an `i32` code, a null pointer — because tagged unions do not
  cross the C ABI.
- **Real-time** (`#[no_alloc]` contexts): `Status` and `Option` returns are
  allocation-free by construction; interpolated `io::eprintln` logging is
  too. There is no error path in the language that secretly allocates.
- **Tests**: negative tests assert the *code*, not just failure —
  `status != 0` plus stderr containing `E0xxx` is the house pattern for
  compiler-facing tests; for library tests, match the exact variant.
