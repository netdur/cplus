# Phase 3 — Ownership: Surface Syntax for Borrow vs Move

> Status: draft
> Scope: surface syntax for the three ownership kinds (shared borrow, exclusive borrow, ownership transfer) on function parameters and method receivers; how each kind maps to LLVM lowering and to the Phase 5/6 borrow-checker rules.
> Out of scope: the borrow checker itself (Phase 5 immutable, Phase 6 mutable + aliasing rule), explicit lifetime annotations (Phase 6), the `Copy` trait machinery beyond a single marker (full trait system is Phase 7), drop/destructor semantics (separate Phase 3 design note).

## 1. Problem

The §2.8a style migration removed `&` from the language: method receivers became `self` / `mut self`. That settled receivers, but left function parameters underspecified. Three semantic kinds still need surface forms:

1. **Shared borrow** — function reads the value, caller retains ownership, value still usable after the call. Common case.
2. **Exclusive borrow** — function mutates the value in place, caller retains ownership, mutations visible to caller. Common.
3. **Ownership transfer (move)** — caller gives up the value; using it after the call is a compile error. Less common but real: constructors, sinks, channels, error wrapping.

Rust marks borrows (`&T`, `&mut T`) and treats move as the default. C+ removed `&`, so we mark the opposite end: borrow is default, move is the marker. This is the price of "the common case is short."

This note locks the surface form before Phase 3 implementation begins.

## 2. Syntax

### 2.1 Function parameters

```cp
fn read(x: Buffer) -> i32 { ... }            // shared borrow
fn fill(mut x: Buffer, byte: u8) { ... }     // exclusive borrow
fn consume(move x: Buffer) -> String { ... } // ownership transfer
```

Grammar:

```
param        = move? mut? ident ':' type ;
```

Both `move` and `mut` on the same parameter is rejected at parse time (E0334 — see §5). `move` implies the function takes ownership; `mut` is meaningless on a value the function already owns at that level. (If you want a mutable local copy of a moved-in value, use `let mut y = x;` in the body.)

### 2.2 Method receivers (already settled, restated for symmetry)

```cp
impl Buffer {
    fn len(self) -> usize { ... }              // shared
    fn push(mut self, byte: u8) { ... }        // exclusive
    fn into_string(move self) -> String { ... } // consumes self
}
```

`move self` is the new addition this slice — same keyword, same semantics. `move self` plus `mut` on the same receiver is rejected (`mut move self` is meaningless).

### 2.3 Return values

Returns are always moves. No keyword. `fn f() -> T` transfers ownership of the T to the caller. This is the only thing returns can mean in the absence of lifetime annotations, so no marker is needed.

### 2.4 Local bindings (unchanged)

`let x: T = ...;` and `let mut x: T = ...;` are unchanged from Phase 1. `mut` on a local means "this binding is rebindable and the value is mutable through it." This is *not* the same `mut` as on a parameter (see §3.4).

## 3. Semantics

### 3.1 Mapping to LLVM lowering

| Surface form | LLVM parameter | Mutation visible to caller? | Borrow-checker (Phase 5/6) |
|---|---|---|---|
| `x: T` (T non-`Copy`) | `ptr` | No (read-only) | Counts as a shared borrow; conflicts with any concurrent `mut` or `move` of the same place |
| `x: T` (T is `Copy`) | by-value (i32, f64, etc.) | N/A — caller's value untouched | No borrow tracking; copied at the call boundary |
| `mut x: T` (T non-`Copy`) | `ptr` with `noalias` (Phase 6) | Yes | Exclusive borrow; conflicts with any other access to the place |
| `mut x: T` (T is `Copy`) | by-value, local rebinding mutable | No | No borrow tracking; equivalent to `let mut x = x_arg;` at top of body |
| `move x: T` | by-value (LLVM aggregate) or `ptr` (ABI choice) | N/A — caller no longer owns | Caller's place becomes uninitialized; reading it is E0335 use-after-move |

The `Copy` vs non-`Copy` split for `x: T` and `mut x: T` is the one place where the same surface syntax has two semantic flavors. This matches Rust. The marker that selects which flavor is the `Copy` property of the type, not the syntax.

`Copy` types in Phase 3: all primitives (`i8`–`i64`, `u8`–`u64`, `isize`, `usize`, `f32`, `f64`, `bool`), all enums (plain enums from slice 2A are integer-shaped), fixed-size arrays `[T; N]` *iff* `T: Copy` (deferred — see §8), structs *iff* all fields are `Copy` and the struct is not marked otherwise (deferred — see §8). Conservative default: only primitives and plain enums are `Copy` in the initial Phase 3 landing. Structs/arrays follow when the `Copy` derivation rules are settled.

### 3.2 Method receiver semantics

Identical to §3.1 with `self` substituted for the parameter name. `self` non-`Copy` (always true for structs in Phase 3 until `Copy` derivation lands) → pointer-pass.

| Receiver | LLVM | Caller invariant |
|---|---|---|
| `self` | `ptr` | place still valid after call |
| `mut self` | `ptr` (will gain `noalias` in Phase 6) | place still valid after call, mutations visible |
| `move self` | `ptr` or aggregate (ABI-dependent) | place uninitialized after call; use is E0335 |

`move self` requires the call site to provide a writable, fully-initialized place, just like a move out of any other variable. `let p = Point::new(0,0); p.into_string();` is valid (consumes `p`); `p.x` afterward is E0335.

### 3.3 Return values

Always moves. The caller binding receives ownership. No special syntax. This is the only form that can exist without lifetime annotations, which Phase 6 introduces and Phase 3 does not.

Consequence: every `fn f() -> T` allocates ownership of T in the caller's frame (logically). LLVM implements this via sret/aggregate-return per ABI; semantically it's a move.

### 3.4 `let mut` vs parameter `mut` — the overload, explained

These mean *related but not identical* things:

- `let mut x: T = ...;` — the binding is rebindable, and the value held by the binding is mutable. No borrow-checker involvement; pure local concern.
- `mut x: T` on a parameter — the function receives an **exclusive borrow**; mutations propagate to the caller's place; the borrow checker enforces exclusivity at the call site.

For `Copy` types, parameter `mut x: T` collapses to the local-binding meaning (caller's value is copied, mutations are local). For non-`Copy` types, parameter `mut x: T` is the exclusive borrow form.

Rationale for keeping the same keyword: the *user-visible behavior* is "this name can be mutated inside the function body" in both cases. The borrow-checker semantics differ but are invisible at the use site (you write `x = ...;` or `x.field = ...;` the same way). Two keywords for one behavior would be cargo-culted formalism.

The design note must call this out in `cpc --explain E0335` / borrow-checker error text so AI agents and humans aren't confused when a borrow-conflict error mentions a `mut x: T` parameter.

### 3.5 Call sites

Call sites do *not* carry borrow/move markers. `f(x)` is the syntax regardless of whether `f` borrows or consumes `x`. The signature tells the story.

This is a deliberate departure from Rust. Tradeoff:
- **Pro**: Less noise; common case (borrow) needs no syntax.
- **Con**: Reading caller code, you can't tell at the call site whether `x` survives. Have to look at the signature.
- **Mitigation**: The borrow checker emits a precise error at the use-after-move site; LSP hover on the call shows the signature. Real-world impact is small.

If this proves painful in practice (especially for `move` — moves are rare and "invisible moves" might surprise), an optional `f(move x)` call-site marker could be added as a *style lint* (warn if a move call doesn't write `move`) without changing the language. Deferred.

## 4. Interactions

### 4.1 Phase 1 (functions, primitives)

No surface change visible. All Phase-1 sample programs use primitives only (`i32`, `bool`); primitives are `Copy`; `fn f(x: i32)` keeps its current meaning (by-value, no borrow tracking). Zero migration cost.

### 4.2 Phase 2 slice 2B/2C (structs + methods)

Existing samples (`point.cplus`, `nested.cplus`, `mutable_struct.cplus`, `methods.cplus`):
- `fn distance(a: Point, b: Point) -> i32` — `Point` is non-`Copy` in Phase 3, so this becomes "two shared borrows of `Point`." Behavior unchanged (the function only reads `a` and `b`). LLVM lowering switches from struct-by-value to pointer-pass. Caller code unchanged.
- `impl Point { fn translate(mut self, ...) }` — already in the new style, no change.

### 4.3 Phase 2 slice 2D (arrays)

`fn sum(arr: [i32; 5]) -> i32` — `[i32; 5]` may be `Copy` once derivation lands (all elements are `Copy`); until then, treat as non-`Copy` → shared borrow. Caller code unchanged.

### 4.4 Phase 5/6 borrow checker

This note picks the surface form. The borrow checker enforces the semantics:
- Phase 5: `x: T` (non-Copy) is tracked as a shared borrow; multiple are fine; conflicts with `mut`/`move` are rejected.
- Phase 6: `mut x: T` gets the exclusive-access rule (aliasing-XOR-mutability), `noalias` parameter attribute emitted to LLVM.
- Phase 6 also resolves: can a function take both `mut x: T` and `y: T` where `x` and `y` overlap? Same rule as Rust: rejected at the call site by the borrow checker.

### 4.5 Phase 7 generics

`fn max[T](a: T, b: T) -> T where T: Ord` — the parameter forms work the same for type parameters. `move`/`mut` apply uniformly. The `Copy` bound (`where T: Copy`) is how generic code chooses whether to copy or borrow.

## 5. Error codes

| Code | Meaning |
|---|---|
| E0334 | parameter has both `move` and `mut` (mutually exclusive) |
| E0335 | use of moved value (after a `move` parameter or `move self` consumed it) |
| E0336 | `move` on a `Copy`-typed parameter (lint-level, suggest removing) — *deferred; not blocking Phase 3 landing* |
| E0337 | call-site place for `move` parameter is not a fully-initialized owned place (e.g., field of a borrowed struct) |

E0336 is intentionally a warning, not an error: `move x: i32` is valid (it's just a copy), but the `move` is redundant. AI-generated code that over-marks `move` should be cleaned up by `cpc fmt` or a lint pass, not rejected outright.

## 6. Sample programs

### 6.1 Must compile and run

Add `docs/examples/ownership.cplus`:

```cp
struct Buffer { data: [u8; 4] }

impl Buffer {
    fn new() -> Buffer { return Buffer { data: [0, 0, 0, 0] }; }
    fn first(self) -> u8 { return self.data[0]; }
    fn fill(mut self, byte: u8) {
        self.data[0] = byte;
        self.data[1] = byte;
        self.data[2] = byte;
        self.data[3] = byte;
    }
    fn checksum(move self) -> u32 {
        return (self.data[0] as u32) + (self.data[1] as u32)
             + (self.data[2] as u32) + (self.data[3] as u32);
    }
}

fn read_first(b: Buffer) -> u8 { return b.first(); }
fn fill_with(mut b: Buffer, byte: u8) { b.fill(byte); }

fn main() -> i32 {
    let mut buf: Buffer = Buffer::new();
    fill_with(buf, 7);                  // exclusive borrow; buf still usable after
    let f: u8 = read_first(buf);        // shared borrow; buf still usable after
    let sum: u32 = buf.checksum();      // moves buf; buf unusable after
    println(sum as i32);                // expects 28 (7*4)
    return 0;
}
```

Expected output: `28`.

### 6.2 Must reject

| Program fragment | Error |
|---|---|
| `fn f(move mut x: Buffer) { ... }` | E0334 both move and mut |
| `fn f(mut move x: Buffer) { ... }` | E0334 |
| `let b = Buffer::new(); let s = b.checksum(); let s2 = b.checksum();` | E0335 use of moved value `b` |
| `let b = Buffer::new(); let s = b.checksum(); println(b.first() as i32);` | E0335 |
| `fn take(move x: Buffer) {} fn main() { let b = Buffer::new(); take(b); take(b); return 0; }` | E0335 second `take(b)` is use of moved value |
| `impl Buffer { fn m(move mut self) {} }` | E0334 on receiver |

## 7. Implementation order

1. **AST**: add `move: bool` and (existing) `mut: bool` fields to `Param`. Add `Receiver::Move` variant alongside `Read` / `Mut`.
2. **Parser**: extend `parse_param` to accept optional `move` and `mut` prefixes (in either order, but reject both via E0334 at parse time). Extend `try_parse_receiver` to recognize `move self`.
3. **Sema**:
   - Track per-binding `Ownership` state: `Owned`, `BorrowedShared`, `BorrowedExclusive`, `Moved`.
   - On call-site analysis: for each argument matched to a `move`-marked param (or `move self` receiver), mark the source place as `Moved`.
   - On any read of a binding, check state: `Moved` → E0335.
   - **Note**: full borrow-conflict checking (multiple `mut` borrows, `mut` overlapping with shared) is *Phase 5/6*. Phase 3 only does move tracking. Borrows are tracked but not yet conflict-checked.
   - Add `Copy` marker: a Rust-side flag on `Ty` (`fn is_copy(&self) -> bool`) — true for primitives + enums in Phase 3. Used to skip move tracking for `Copy` types.
4. **Codegen**:
   - `move` params lower the same way `self` non-`Copy` params do today (pointer-pass for aggregates, by-value for `Copy`).
   - `mut x: T` for non-`Copy`: pointer-pass, no extra alloca.
   - `mut x: T` for `Copy`: by-value, then alloca + store so the body can mutate locally.
   - No `noalias` yet — that's a Phase 6 unlock once aliasing-XOR-mutability is checked.
5. **Tests** (~25 new):
   - Parser: `move`, `mut`, both rejected.
   - Sema: move tracking through calls, through method receivers, through assignments (`let y = x; use(x);` → E0335).
   - Codegen: pointer-pass shape for `mut Buffer`, by-value for `mut i32`.
   - E2E: `ownership.cplus` runs and prints 28.
   - Negative: each row of §6.2.

## 8. Open questions

- [ ] **`Copy` derivation rules.** Initial Phase 3 has only primitives + enums as `Copy`. Should structs auto-derive `Copy` if all fields are `Copy`? Lean: yes (implicit, no annotation needed for the obvious case). Arrays `[T; N]` `Copy` iff `T: Copy`? Lean: yes. But: a `Buffer { data: [u8; 1024] }` being implicitly `Copy` means `let b2 = b;` silently copies 1KB. Rust requires an explicit `#[derive(Copy)]` to avoid surprise. Decision deferred — wants its own short design note before Phase 3 lands.
- [ ] **`move` on a Copy-typed parameter** — E0336 as a lint vs. silently allowed. Lean: warn (suggest removing `move`).
- [ ] **Partial moves.** `move b.data` — does C+ allow moving an individual struct field, leaving the rest? Rust does. Lean: defer to Phase 5/6 alongside the rest of the move-out-of-place machinery.
- [ ] **Drop / destructors.** Move semantics imply some types have drop glue. Separate design note needed before Phase 3 lands: which types are `Drop`, how is the destructor declared, when does drop run (end of scope, after last use). Probably aligns with Phase 3 `defer` work.
- [ ] **`Self` (capital) in receiver position**: `fn into_x(move self) -> Self::Output` — interacts with Phase 7 associated types. Defer.
- [ ] **Call-site `move` keyword** as a style lint (`f(move x)` to make consumption visible). Defer; revisit after writing real C+ code.
- [ ] **Returning a borrow.** `fn first(self) -> T` returning a *reference to inside self* is exactly the case lifetimes solve. Without lifetimes (Phase 3), all returns are moves of owned values. A function that wants to return "the first element of the buffer" must either return a `Copy` of it or take `move self` and return an owned T extracted from it. This restriction lifts in Phase 6.
