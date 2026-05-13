# Phase 6 — Exclusive-borrow tracking, aliasing-XOR-mutability, and `noalias`

> Status: design note. Implementation lands in 5–7 sub-slices per the Phase 6 sequencing block in [plan.md](../../plan.md) §3.
> Scope: tracking the *exclusive* borrow form (`mut x: T` and `mut self` on non-`Copy` types); the aliasing-XOR-mutability rule; explicit lifetime annotation syntax for the cases Phase 5 elision cannot infer; `noalias` codegen for the proved cases; the iterator-invalidation / data-race / dangling-pointer rejection surface.
> Out of scope: atomic types `Atomic[T]` (sibling Phase-6 design note, not this one); heap allocation primitives (likely paired with a `Vec[T]` slice in Phase 6 or 7); generic types (Phase 7); raw-pointer `*T` and slice `T[]` machinery (Phase 6+ — depends on this note's exclusive-borrow rules being in place first).
>
> Depends on: [phase5-borrow-shared.md](phase5-borrow-shared.md) — every concept here extends what landed there. The `Place` / `PlaceState` machinery, the flow-sensitive analyzer, the elision rules, and the diagnostic shape are all inherited.

This note is comparable in size to phase5-borrow-shared. The rules are similar in spirit but every conflict is symmetric (mutability gets the same rigor reads do), and the §6 codegen story — `noalias` — is the load-bearing performance reason borrow-checked C+ can outperform C.

---

## 1. Problem

Phase 5 shipped the *shared* half of the borrow checker: `x: T` on non-`Copy` types is a shared borrow, many concurrent readers are fine, conflict against move was made detectable, single-parameter (E1), self-method (E2), and multi-parameter (E3) elision rules picked the minimum set that admits idiomatic accessor-style code without annotations. The §2.9 mut-borrow pointer ABI also landed (slice 5BC.codegen) so non-Copy `mut x: T` writes propagate back to the caller — the runtime semantics are correct.

What Phase 5 explicitly did **not** do:

1. **Detect conflicts involving exclusive borrows.** `mut x: T` on a non-Copy type is, per §2.9, an exclusive borrow — "at most one at a time, conflicts with everything else." Phase 5 treats `mut`-parameters as plain writes; nothing rejects two concurrent `mut`-borrows of the same place, or one `mut`-borrow concurrent with a shared borrow, or a read of `x` while `x` is exclusively borrowed elsewhere. This is the aliasing-XOR-mutability rule's actual enforcement.
2. **Allow the user to write lifetimes when inference can't solve them.** Rule E3 in Phase 5 takes the conservative path: if a function takes ≥2 non-Copy shared-borrow parameters and returns a non-Copy value, the body-flow analysis decides which parameters the return borrows from; if any return path constructs a fresh value, the elision is disqualified. Many legitimate cross-function patterns (storing a borrow in a struct field, threading a borrow through several functions, returning a borrow constrained to a specific parameter when the body is too complex to flow-analyze) need explicit annotations to compile. Phase 5 rejected these with "wait for Phase 6 explicit lifetime annotations."
3. **Tag LLVM IR with `noalias` on the proved-unique pointer parameters.** Phase 5's mut-borrow pointer ABI emits `ptr %i` for non-Copy `mut x: T`, but the parameter is untagged — LLVM has to assume the pointer might alias other pointers in scope. Once the borrow checker proves uniqueness, tagging that pointer with `noalias` unlocks the optimizer's aggressive load/store reordering. This is the single biggest reason borrow-checked code can outperform equivalent C (where the compiler is forbidden from assuming non-aliasing without `restrict` annotations).
4. **Drop analysis with conditional moves.** Phase 5's flow-sensitive analyzer detects `MaybePartial` states across branches and fires E0371 when a binding is possibly-moved. But the runtime drop-flag machinery (slice 3F) doesn't yet know how to read the *static* analysis — every drop site reads its drop flag at runtime, which is correct but wasteful when the analyzer proved a binding is definitely-moved on every path. Phase 6 lifts the static knowledge into codegen so a definitely-moved binding emits no scope-exit drop call at all (and a definitely-not-moved binding emits the call unconditionally).
5. **Reject iterator invalidation, data races, and dangling pointers at compile time.** Phase 5's exit criterion was the analytic half — `fn longest(...)` accepts under elided lifetimes — but the *guarantee*-level claim ("Rust-level memory safety: use-after-free, double-free, data races, iterator invalidation caught at compile time" — §1.2) requires the exclusive-borrow rule. Without it, `for x in vec { vec.push(...); }` compiles silently and corrupts memory at runtime. Phase 6's exit pins the canonical case as a negative test.

Each of these is a real hole today. The load-bearing one is (1) — the aliasing rule is what makes the rest sound. (3) is the performance payoff. (2) is the migration path for code that legitimately needs to express lifetimes. (4) is the drop-flag cleanup. (5) is the language-level guarantee finally landing.

The constraint that shapes Phase 6 is the same one from Phase 5: **C+ has no `&T` / `&mut T` reference types** (§2.9). Exclusive borrows are expressed by `mut x: T` / `mut self`, not by a `&mut T` reference type. Lifetime annotations, when they arrive in this phase, attach to parameters — not to reference types, because there are none.

---

## 2. What an "exclusive borrow" is in C+

Restating from §2.9 + slice 5BC.codegen so the rest of the note has a firm referent:

- A parameter `mut x: T` where `T` is non-`Copy` is an **exclusive borrow** of T. The callee may mutate `x`; the caller retains ownership; the caller cannot read or borrow `x` while the call is in flight; mutations propagate back to the caller's place when the call returns. **At most one exclusive borrow of a place is live at any program point.**
- A parameter `mut x: T` where `T` is `Copy` (primitives, plain enums, structs whose fields are all Copy and not Drop) is a **pass-by-value copy** that the callee can mutate locally; the caller's value is untouched (per §2.9 explicit table). The borrow checker doesn't track it — `mut` on Copy is local-mutability syntax, not a borrow.
- Method receiver `mut self` follows the same pattern with the same Copy-ness logic. `mut self` on a non-Copy type is the canonical exclusive-borrow form.

**The Phase-5 model accepts unsound mut-mut and mut-shared patterns.** Both of these compile today and should not:

```cp
fn modify_both(mut a: Buffer, mut b: Buffer) { ... }
fn main() {
    let mut buf = make_buffer();
    modify_both(mut buf, mut buf);   // two concurrent exclusive borrows of `buf` — must reject
}
```

```cp
fn write(mut a: Buffer) -> i32 { ... }
fn read(b: Buffer) -> i32 { ... }
fn main() {
    let mut buf = make_buffer();
    let x = write(mut buf) + read(buf);   // exclusive + shared of the same place — must reject
}
```

Argument-evaluation order is left-to-right (locked in for §2.9), but the conflict is semantic: at the program point of either call, *both* borrows are conceptually live (each parameter is a borrow that spans the callee's execution). The aliasing-XOR-mutability rule rejects on the *aliasing*, not on the evaluation order.

Phase 6 also tracks **the same place-granularity Phase 5 introduced** — a borrow of `buf.parts` does not conflict with a borrow of `buf.payload`, but a borrow of `buf` itself conflicts with a borrow of any field within it. The `Place::projections` machinery plumbed in Phase 5 (and dormant through 5BC.5) becomes load-bearing here.

---

## 3. Conflicts the Phase-6 checker rejects

Phase 5 rejected three families (move-during-shared, move-then-use-across-branches, return-while-borrow-live). Phase 6 adds three more, plus tightens the existing ones once the symmetric `BorrowedExclusive` state exists.

### 3.0 Conflict matrix at-a-glance

For a single place P, the second column lists every operation that can be claimed against P at one program point. Rows are P's current state. Cells give the result (✓ admitted, code rejected, or a state transition).

| State of P → / Op ↓ | Owned | BorrowedShared(N) | BorrowedExclusive(n) | Moved | MaybePartial |
|---|---|---|---|---|---|
| Read P | ✓ | ✓ (count unchanged) | **E0383** | E0335 | **E0371** |
| Move P (via `move`-param) | → Moved | E0372 | E0372 | E0335 | E0371 |
| Shared-borrow P (via shared param of non-Copy) | → BorrowedShared(1) | → BorrowedShared(N+1) | **E0381** | E0335 | E0371 |
| Exclusive-borrow P (via `mut`-param of non-Copy) | → BorrowedExclusive(borrower) | **E0381** | **E0380** | E0335 | E0371 |

Codes in **bold** are Phase 6 additions (or Phase 5 codes whose conflict surface widens under Phase 6). Same-place conflicts across the four argument positions of a single call → **E0380** (mut-mut), **E0381** (mut-shared), **E0382** (mut-move), **E0370** (shared-move, Phase 5). Same-place conflicts across statements while a borrower is live → **E0383** (read/write/move while exclusive borrow live) or **E0372** (move while shared borrow live, Phase 5 widened). **E0374** (partial-place conflict) supersedes the table when the projections-prefix check produces a refined verdict — see §5.2.

The matrix is the analyzer's heart. §§3.1–3.6 walk through each new family with a code example and the diagnostic's reasoning.

### 3.1 Exclusive vs. exclusive on the same place

```cp
modify_both(mut buf, mut buf);   // E0380 (proposed)
```

Implementation: every argument position that resolves to a `mut`-marked non-Copy parameter contributes a `BorrowedExclusive` claim against the argument's place. A call evaluates left-to-right but the claims are checked *as a set* — multiple `mut` claims on the same place fail. Multiple `mut` claims on *different* places (`modify_both(mut buf.left, mut buf.right)`) succeed under partial-place tracking (§5.2 below); the analyzer can distinguish.

This is the symmetric counterpart to Phase 5's E0370 (move-and-shared-borrow). The diagnostic shape mirrors that one's: primary span at one of the offenders, secondary span at the other, help text suggesting either reordering into sequential statements or restructuring the call.

### 3.2 Exclusive vs. shared on the same place

```cp
write_thing(mut buf, peek(buf));   // E0381 (proposed)
```

A `mut`-claim on `buf` plus a `Read`-claim on `buf` (via the `peek(buf)` sub-expression, which shared-borrows `buf` per Phase 5 rules) is rejected. This is the aliasing-XOR-mutability rule operating at call-site granularity.

Symmetric form — `peek(buf, write_thing(mut buf))` — also rejected; the order of conflicting claims doesn't matter.

### 3.3 Exclusive vs. move on the same place

```cp
write_thing(mut buf, consume(move buf));   // E0382 (proposed)
```

A `mut`-claim plus a `Move`-claim on the same place. This is the harder case to phrase well because the moved binding ceases to exist after the call (in a sense), but at the program point of the call both are conceptually live. The diagnostic explains that the exclusive borrow's lifetime spans the call, and the move happens during that span.

### 3.4 Reads / moves while an exclusive borrow is live across statements

```cp
let bytes = drain(mut buf);   // bytes is a borrow tied to buf (Rule E1)
let x = buf.len();            // E0383 (proposed) — buf is exclusively borrowed via bytes
println(bytes);
```

This is the cross-statement form: when `drain(mut buf)` returns a borrow tied to `buf` (via the same Phase-5 elision rules, but with the borrow flavor flipped from Shared to Exclusive), the caller's binding `bytes` keeps `buf` in `BorrowedExclusive` state for as long as `bytes` is live. While `bytes` is alive, *any* access to `buf` — read, write, move — is rejected.

The diagnostic explains the borrow chain: "the exclusive borrow established when `bytes` was bound at line N extends through this access."

### 3.5 Two exclusive borrows that span overlapping scopes

```cp
let mut buf = make_buffer();
let h1 = handle_a(mut buf);
let h2 = handle_b(mut buf);   // E0380 cross-statement form
use_both(h1, h2);
```

Both `h1` and `h2` claim exclusive borrows of `buf`; their scopes overlap until `use_both`. Phase 6 rejects. The fix is sequential use (drop `h1` before binding `h2`) or restructuring.

This is the canonical **iterator invalidation** pattern in disguise: an iterator over a collection is, semantically, a borrow tied to the collection; mutating the collection while the iterator is alive needs a fresh exclusive borrow of the collection, which conflicts. §10 below works through `for x in vec { vec.push(...); }` end-to-end as the Phase-6 exit criterion.

### 3.6 Phase 5 conflicts now also gate against exclusive borrows

The existing E0370 (move-and-shared in one call), E0371 (use of possibly-moved), and E0372 (move while shared borrow live) all extend to consider exclusive borrows in their conflict matrix:

- A `mut`-claim plus a `Move`-claim of the same place in one call → E0382 (new), not E0370 (shared).
- A binding that's possibly-mutably-borrowed (one branch ran `let h = handle(mut buf)`, the other didn't) used after the branch → still E0371 with refined message.
- A move of a place while an *exclusive* borrow is live → still E0372 with refined message.

These reuse the existing error codes since the static structure of the analysis is the same — what changed is that the `live_borrows` machinery now also tracks `BorrowedExclusive` claims, not just `BorrowedShared(N)`.

---

## 4. Lifetime annotations

Phase 5's elision rules cover the common cases. Phase 6 introduces explicit lifetime syntax for everything elision can't infer.

### 4.1 The cases Phase 5 cannot infer

- **Return borrow from one of N parameters when body analysis is ambiguous.** Phase 5 Rule E3 admits `longest` because the body is small enough to flow-analyze. Once the body involves loops, conditionals on derived values, or cross-function calls, the body-flow analysis bails (per design note 5 §4.1 "elide less rather than more") and the function is rejected with a "consider annotating" pointer.
- **Borrow stored in a struct field.** A struct whose field type is a non-Copy borrow has no way today to express how long the field is valid. Phase 5 §4.2 calls this out as deliberately deferred: "structs cannot have non-Copy borrow-typed fields." Phase 6 lifts that with explicit syntax.
- **Cross-function lifetime threading.** When `outer` calls `inner` which returns a borrow, and `outer` returns *that* borrow, Phase 5 admits only the cases Rules E1 / E2 cover at every step. Phase 6 admits arbitrary threading via annotations.

### 4.2 The syntax: `borrow REGION T`

Phase 6 lands explicit lifetime annotations using a new `borrow` keyword attached to types at the use site. Region names are ordinary identifiers that follow the keyword.

```cp
fn longest(xs: borrow A string, ys: borrow A string) -> borrow A string {
    if xs.len() > ys.len() { return xs; } else { return ys; }
}

fn split_first(xs: borrow A string, ctx: borrow B Ctx) -> borrow A Slice {
    return xs.first_word();
}

struct Cursor {
    buf: borrow A Buffer,
    pos: usize,
}
```

**Constraints that drove the choice:**

- C+ has **no `&T` / `&mut T`** (§2.9), so Rust's `&'a T` shape doesn't fit — there's no reference type to attach the lifetime to.
- Lifetimes attach to **parameter and field types**, since those are where the surface marks borrowing.
- The annotations must be **distinguishable from generic type parameters** (Phase 7).
- The annotations must be **readable when there are 2–3 of them** on one signature.

**Why `borrow REGION T` over the alternatives:**

The decision was between three forms — `borrow A T` (chosen), apostrophe-prefix lifetimes (`'a T` like Rust), and `where return borrows ...` clauses. The chosen form reads cleanest at the call site without requiring a separate generic-parameter list to declare the region names. Compare:

```cp
// chosen: borrow REGION T — read left-to-right, "a shared borrow in region A of string"
fn longest(xs: borrow A string, ys: borrow A string) -> borrow A string

// rejected: apostrophe-prefix — imports Rust's tick-letter cosmetic and needs a
// declaration list at function-name position ('a in the brackets), doubling
// the signature's surface even for two-line functions.
fn longest['a](xs: 'a string, ys: 'a string) -> 'a string

// rejected: where-clause — separates the constraint from the parameters it
// constrains, and risks confusion with future generic constraints.
fn longest(xs: string, ys: string) -> string where return borrows xs, ys
```

The "verbose, not concise" stance of §2.8c argues for `borrow` over `'a`: the keyword tells a reader who lacks context exactly what the annotation means, where `'a` is opaque until learned. The chosen form is also one fewer surface element (no `<'a>` / `['a]` declaration list) which is the smaller spec.

**Region names** are ordinary identifiers. Style: short uppercase (A, B, C) for inline use, descriptive when clarity wins (`borrow BUF Buffer`). The grammar admits any identifier — the convention is in the formatter / lint, not in the parser. Names are local to one signature; the same name `A` in two different functions is two unrelated regions.

**Composition with `mut` / `move`:** the parameter marker (`mut`, `move`) and the type annotation compose orthogonally. `mut` flips the borrow's flavor to exclusive; the region annotation pins its lifetime:

```cp
fn bump(mut buf: borrow A Buffer) -> i32 { ... }   // exclusive borrow in region A
fn read(buf: borrow A Buffer) -> i32 { ... }       // shared borrow in region A
fn take(move buf: Buffer) -> Bytes { ... }         // move; no region (ownership transfers)
```

`move` consumes the value, so no region annotation is meaningful (or admitted by the grammar — `move x: borrow A T` is a parse error).

**Struct fields** carry the annotation directly:

```cp
struct Cursor {
    buf: borrow A Buffer,
    pos: usize,
}
```

Phase 6 first cut admits only **shared** borrows in struct fields. Exclusive-borrow fields require a field-level `mut` marker that C+ doesn't have today (mutability is per-binding, not per-field), and the design implications are large enough to defer. See §9.7.

**Function-level region declaration is implicit.** Unlike Rust's `<'a>` list at function-name position, C+ does not declare region names separately. The names introduce themselves at first use within the signature. Phase 7's generic-parameter list `fn f[T: Ord](...)` lives in a different syntactic slot (after the function name, square brackets, type-parameters with constraints); region names do not collide.

**Lexer / parser impact (slice 6BC.5):**

- New reserved keyword `borrow`. Currently not an identifier in any in-tree sample; reservation lands as a one-row addition to the keyword table.
- Type grammar extends: `Type = ... | "borrow" Ident Type`. The annotation binds tighter than function-arrow but looser than array `[T; N]`; concrete precedence pinned in the slice.
- Formatter rule: `borrow REGION T` with single spaces, no line wrap inside the annotation.
- TextMate grammar in [editors/vscode/syntaxes/cplus.tmLanguage.json](../../editors/vscode/syntaxes/cplus.tmLanguage.json): `borrow` joins the keyword class; region-name token inherits the type-parameter style class.

### 4.3 Variance

Phase 6 inherits the standard variance rules: invariant in `mut`-borrow position, covariant in shared-borrow position, contravariant in argument position when a function-typed value carries a region. Same shape as Rust. The asymmetry follows from §2.9: the `mut` rule lets caller writes propagate through callee mutations, so any region-substitution that narrows the mutated place's region would invalidate prior writes — invariance is the only sound choice. Phase 6 documents the rule but admits no syntax beyond what §4.2 introduces (no `+` / variance markers); the rule is part of the type-checker's behavior, not of the surface syntax.

### 4.4 No `'static` in Phase 6 first cut

The `'static` (or equivalent) special lifetime — meaning "valid for the entire program" — exists in Rust as a load-bearing concept for things like string literals and thread-spawn closures. C+ has neither pattern yet in Phase 6; `'static` ships when the first case (probably string-literal types, probably Phase 7+) needs it. Phase 6's elision rules and annotation surface are designed without it; the absence keeps the spec smaller.

---

## 5. The check itself (algorithm)

The Phase-6 borrow checker is Phase 5's borrow checker with three changes: a new `BorrowedExclusive` state, an extended conflict matrix, and exclusive-borrow-source elision rules that mirror Phase 5's E1/E2/E3.

### 5.1 Place state

Phase 5 introduced four states:

- **Owned** — fully owned, can be moved, can be borrowed.
- **BorrowedShared(N)** — N ≥ 1 shared borrows are live. Cannot be moved or exclusively borrowed; can be additionally shared-borrowed.
- **Moved** — has been consumed.
- **MaybePartial** — definite-assignment said yes-on-some-paths, no-on-others.

Phase 6 adds the fifth:

- **BorrowedExclusive(name)** — exactly one exclusive borrower (`name` is the borrowing binding's identifier — singular, so a `String` not `Vec<String>`). Conflicts with **everything**: reads, writes, moves, shared borrows, additional exclusive borrows. Releases when the borrower goes out of scope or is itself moved.

The `PlaceState::merge` function gets four new pairwise rules:

```
BorrowedExclusive(n) ∩ BorrowedExclusive(n) = BorrowedExclusive(n)           // same borrower both branches: still exclusive
BorrowedExclusive(n) ∩ BorrowedExclusive(m), n ≠ m = MaybePartial            // different borrowers per branch: post-join state is "we don't know which"
BorrowedExclusive(n) ∩ Owned = MaybePartial                                  // one branch borrowed, other didn't: post-join state is "we don't know if borrowed"
BorrowedExclusive(n) ∩ BorrowedShared(k) = MaybePartial                      // one branch exclusive, other shared: irreconcilable
```

`MaybePartial` becomes the catch-all "I don't know precisely" state — reads of a MaybePartial place fire E0371 (the existing Phase-5 code) regardless of whether the underlying ambiguity is moved-or-not vs. exclusively-borrowed-or-not. The diagnostic message branches on the specific transitions that produced the MaybePartial.

### 5.2 Partial-place tracking (Place::projections)

Phase 5's `Place::projections` machinery was plumbed in slice 5BC.5 but had no triggering surface syntax (sema's E0337 rejects `move x.field` before borrowck sees it). Phase 6 starts using it.

`modify_both(mut buf.left, mut buf.right)` claims two exclusive borrows on *different* places (`buf.left` and `buf.right`) — the conflict matrix says this is fine. The implementation:

1. Each call argument's place is computed via the existing `gen_place`-style chain: `Ident(name)` → `Place::root(name)`; `Field(Ident(name), f)` → `Place { root: name, projections: [Field(f)] }`; chains recurse.
2. The conflict check is done at the *full place* level. Two borrows of `buf` conflict; two borrows of `buf.left` and `buf.right` do not; a borrow of `buf` and a borrow of `buf.left` *do* conflict (the larger borrow includes the smaller).
3. The "includes" check is a prefix-comparison of `projections` vectors: place `A` covers place `B` iff `A.root == B.root && A.projections` is a prefix of `B.projections`.

The same machinery generalizes to `Index(const)` projections (constant-index array access — `arr[3]` vs. `arr[7]` are distinct places) and `AnyIndex` (non-constant indices conservatively coarsen to a single per-array place, matching Phase 5's design note §5.1).

### 5.3 Exclusive-borrow-source elision

Phase 5 elision Rules E1/E2/E3 inferred the source(s) a shared-borrow return draws from. Phase 6 mirrors the same three rules for `mut`-marked parameters:

**Rule E1-mut.** If a function takes exactly one non-Copy `mut`-marked parameter and returns a non-Copy value, the return's lifetime is the parameter's, and the return is treated as an exclusive borrow.

```cp
fn handle(mut buf: Buffer) -> Cursor { ... }
// elided to: return-lifetime = buf's, return-flavor = Exclusive
```

**Rule E2-mut.** Method `mut self` returning a non-Copy value: return is an exclusive borrow of `self`.

```cp
impl Buffer {
    fn cursor(mut self) -> Cursor { ... }
}
```

**Rule E3-mut.** Multi-`mut`-parameter functions returning non-Copy: body-flow analysis decides which `mut`-parameter the return borrows from; conservative fallback rejects with "consider annotating."

In practice E3-mut is the rare case (most `mut`-functions return either nothing, a primitive, or an owned aggregate). Recording the rule for completeness.

### 5.4 Conditional drop analysis

Phase 5 leaves all drop calls runtime-flagged: every Drop binding has an `i1` flag set to `true` at declaration, flipped to `false` when moved. Codegen emits a conditional drop at scope exit. Phase 6 uses the static analyzer's output to specialize:

- **Definitely-moved on every path:** the binding's drop flag is never re-checked at scope exit (the drop call is elided). Saves the conditional branch.
- **Definitely-not-moved on every path:** the drop flag is fixed at `true`; the conditional drop becomes an unconditional one. Saves the load + branch.
- **MaybePartial:** the runtime flag mechanism kicks in (this is Phase 5's current behavior — it's the load-bearing case for the runtime flag).

The implementation reads the analyzer's per-program-point `PlaceState` map at the scope-exit point of each drop binding and picks one of the three lowerings.

This isn't strictly required for the Phase-6 exit criterion (the runtime flag mechanism is already correct), but it lands here because the static analysis is now precise enough to drive the specialization. Could slip to a later phase if it complicates the slicing — recorded here as opportunistic.

### 5.5 `noalias` codegen

This is the performance unlock. Once the borrow checker has proved a non-Copy `mut x: T` parameter is uniquely borrowed for the duration of the call (which it has, by definition of the exclusive borrow), the LLVM IR can tag the underlying pointer parameter with `noalias`:

```llvm
define void @bump(ptr noalias %0) {
  ...
}
```

LLVM's optimizer treats `noalias` as a strong promise: no other pointer in scope aliases this one. It enables aggressive load/store reordering, dead-store elimination across calls, and loop unrolling that would otherwise be blocked. This is the reason borrow-checked code can outperform equivalent C, where the compiler is forbidden from assuming non-aliasing without `restrict` annotations (which C code in the wild rarely uses).

Codegen change: slice 5BC.codegen already emits `ptr %i` for non-Copy `mut x: T`; Phase 6 changes the emission to `ptr noalias %i`. The change is one line per call site; the borrow checker's signature analysis is the actual work.

Shared non-Copy parameters get a different (smaller) tagging: `ptr readonly %i`. The aliasing-XOR-mutability rule explicitly permits `f(x, x)` for a function taking `(a: T, b: T)`, so the underlying pointers genuinely can alias — `noalias` would be unsound. But the callee cannot mutate through a shared parameter (sema rejects), and Drop is not registered on borrowed params (slice 5BC.codegen), so the LLVM-level `readonly` claim is mechanically true. `readonly` is enough to enable redundant-load elimination and read-reordering across calls without the soundness hazard.

This matches Rust's actual behavior: `&mut T` → `noalias`, `&T` → `readonly` but NOT `noalias`. The LLVM team tried `noalias` on `&T` around 2013–2014 and hit soundness bugs in the optimizer; Rust reverted it, and subsequent re-attempts have been contentious. C+'s `x: T` shared form is explicitly multi-pointer-able by design (the whole point of shared borrows), so the answer is settled rather than aspirational.

### 5.6 Copy fast path stays

Per Phase 5 §5.5, the analyzer skips Copy-typed places entirely. Phase 6 keeps that — `mut x: i32` (Copy) is local-mutability and carries no aliasing constraint.

---

## 6. Diagnostic surface

The borrow-checker diagnostic surface gets four new codes plus tightening of existing ones. The shape — primary span at the failing op, secondary span at the conflict cause, help text with a fix suggestion — is unchanged from Phase 5 §6.

### 6.1 Proposed error codes

| Code | Meaning | Phase |
|------|---------|-------|
| E0370 | Move and shared borrow of the same place in one call | 5 |
| E0371 | Use of possibly-moved binding (one branch moved, another didn't) | 5 |
| E0372 | Move while a shared borrow is live | 5 |
| E0373 | Return of borrow from a moved-in parameter | 5 |
| E0374 | Borrow-conflict in a partial-move place | 5 (reserved; activates in Phase 6 with partial-place tracking) |
| E0380 | Two exclusive borrows of the same place in one call | 6 |
| E0381 | Exclusive and shared borrow of the same place in one call | 6 |
| E0382 | Move and exclusive borrow of the same place in one call | 6 |
| E0383 | Access of an exclusively-borrowed place across statements | 6 |
| E0384 | Cannot infer return-borrow source — requires explicit annotation | 6 |
| E0385 | Mismatched lifetime annotations (parameter does not outlive return) | 6 |
| E0386 | Struct field with non-Copy borrow type requires lifetime annotation | 6 |

E0374 is the slot Phase 5 reserved for partial-place conflict diagnostics. Phase 6 activates it once `Place::projections`-aware analysis is wired in.

### 6.2 Sample diagnostics

**E0380 — two exclusive borrows in one call.**

```
error[E0380]: argument exclusively borrows `buf` already exclusively borrowed by another argument
   --> main.cplus:7:24
    |
  7 | modify_both(mut buf, mut buf);
    |             -------  ^^^^^^^ this argument also exclusively borrows `buf`
    |             |
    |             first exclusive borrow of `buf`
    |
  = note: at most one exclusive borrow of a place can be live at a time;
          `mut buf` claims exclusive access for the duration of the call,
          which conflicts with the second `mut buf` claim.
  = help: split into two calls if the operations are independent, or
          restructure to operate on different sub-places (e.g.
          `modify_both(mut buf.left, mut buf.right)`).
```

**E0383 — access of an exclusively-borrowed place.**

```
error[E0383]: cannot read `vec` while it is exclusively borrowed
   --> main.cplus:5:17
    |
  3 | let cur = cursor(mut vec);
    |                  ------- exclusive borrow of `vec` extends through `cur`
  4 | cur.advance();
  5 | let n = vec.len();
    |         ^^^ access of `vec` while exclusive borrow is live
    |
  = note: while `cur` is alive, no other access to `vec` is admitted.
  = help: drop `cur` before reading `vec`, or restructure so the read
          happens before the cursor is established.
```

**E0384 — annotation required.**

```
error[E0384]: cannot infer which parameter the return borrows from
   --> lib.cplus:12:1
    |
 12 | fn merge(left: Buffer, right: Buffer) -> Buffer {
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
 13 |     if some_complex_condition() {
 14 |         return derive_from(left);
 15 |     } else {
 16 |         return derive_from(right);
 17 |     }
 18 | }
    |
  = note: body-flow analysis cannot prove which input the return value
          borrows from; the calls `derive_from(left)` and `derive_from(right)`
          obscure the source.
  = help: annotate the lifetime explicitly. For example, if the return
          borrows from `left`:
              fn merge(left: borrow A Buffer, right: Buffer) -> borrow A Buffer
```

The `borrow REGION T` annotation (§4.2) is the surface the diagnostic teaches. Slices 6BC.1–6BC.4 emit E0384 with this exact form even before the parser admits it, so users get a concrete recipe rather than a deferred pointer; slice 6BC.5 lands the parser side and the recipe starts compiling.

### 6.3 Diagnostic prioritization within a function

When a function body has multiple borrow errors, current Phase 5 behavior is to emit them all in source order. Phase 6 should reconsider: a single root cause often cascades into many spurious "follow-on" errors. The standard fix is to suppress cascading errors when one upstream cause explains them. Land Phase 6 with the naive emit-all-in-order, then add prioritization as polish.

---

## 7. Interactions

With every Phase 1–5 feature:

- **§2.4 errors-are-values:** unaffected. Tagged-union types flow through Phase 6 with the same Copy/non-Copy rules.
- **`if let` / `guard let` (slice 4A.5):** the pattern bindings still establish new owned places; their interaction with exclusive borrows is "the bound value is owned by its arm scope, not a borrow of the scrutinee." Borrow checker sees this through the existing lowering.
- **`break` / `continue` / `loop` / `while let` (slice 4-end):** loop body analysis extends to exclusive borrows the same way Phase 5 extended it to moves. A `mut`-borrow established inside a loop body, in a way that doesn't release before the loop's back-edge, is rejected — otherwise iteration N+1 would see an already-borrowed place.
- **`defer` (slice 3G):** a `defer EXPR;` registers `EXPR` for scope exit. If `EXPR` reads a place that's exclusively borrowed at scope exit time, Phase 6 rejects (E0383-equivalent at the defer site). Same treatment Phase 5 gave to moves.
- **Drop (slice 3F):** the static analysis now drives the drop-flag lowering optimization (§5.4 above). Runtime semantics unchanged; codegen specialization is opportunistic.
- **Modules / `pub` (slice 4B):** signatures cross file boundaries; the borrow checker reads each function's signature (including any explicit lifetime annotations once §4.2 lands) when checking call sites.
- **Formatter (slice 4D):** the `borrow REGION T` annotation is part of fn-signature formatting. Rule: single spaces between `borrow`, the region name, and the type; never line-wrap inside the annotation. Lands in slice 6BC.5 alongside the parser change.
- **LSP (slice 4E):** borrow errors flow through the existing Diagnostic pipeline. Lifetime annotations show up as part of `hover` and `signatureHelp` once those LSP capabilities land.
- **`assert EXPR;` (slice 5ATTR.3) and `cpc test` (slice 5ATTR.4):** test bodies borrow-check normally. No carve-outs.
- **Doctests (slice 5DOC):** synthesized doctest functions borrow-check normally.

With Phase 5 sibling features:

- **Shared-borrow tracking (5BC):** Phase 6 extends the existing checker. The `Place`, `PlaceState`, `live_borrows`, `binding_borrows_from`, and elision-rule machinery are all inherited. Phase 6's new code adds the exclusive states and conflict cases; the surface area of the change is bounded.
- **Mut-borrow pointer ABI (5BC.codegen):** Phase 6 extends to `ptr noalias %i` and adds the missing cases (non-Copy `mut`-array params, etc., per slice 5BC.codegen's "What's still deferred" entry in §11).
- **5BC.5 / 5BC.6 carry-forwards:** activated in Phase 6. Partial-place tracking via `Place::projections` is no longer dormant; diagnostic polish becomes a real concern once exclusive-borrow errors land.

With Phase 6 sibling features (this note's scope vs. siblings):

- **Atomic types `Atomic[T]`** are a sibling Phase-6 design note. They interact with the borrow checker in that `Atomic` types have a unique "any-thread-shared-mutability" property that breaks the aliasing-XOR-mutability rule — explicitly, on purpose. `Atomic[T]` admits mutation through a *shared* `x: Atomic[T]` parameter (no `mut` needed), because the atomic-instruction lowering carries the memory-ordering semantics that make concurrent access race-free. The interaction shape: `Atomic[T]` is `Copy`-flavored for the borrow checker's purposes (the borrow checker doesn't track it), even though the type itself is non-trivially constructed. Sibling note specifies the details.
- **Heap allocation (`Box[T]`, `Vec[T]`)**: dependent slice. Once Phase 6's borrow checker can reject iterator invalidation statically, a `Vec[T]` implementation is reachable. The implementation is mostly stdlib work; the language-level requirement is just "non-Copy heap-typed aggregates that the borrow checker tracks." Phase 6's exit criterion (§9 below) needs this to exist.

With Phase 7+ (forward-looking):

- **Generics:** lifetime annotations and type-parameter annotations share syntactic surface. The §4.2 candidates each have an answer for "how do generics and lifetimes co-exist on one signature." Phase 7's design note revisits.
- **Interfaces:** interface methods can have lifetime annotations the same way fn signatures do. Phase 7 propagates the rules; Phase 6 just picks a spelling and lets Phase 7 build on it.
- **`unsafe` blocks (Phase 8+):** `unsafe` is the escape hatch for cases the borrow checker rejects but the user can prove safe. Phase 6 doesn't introduce `unsafe` (it's already reserved at Phase 1); the keyword activates when raw pointers / FFI need it.

With deliberate non-features:

- **No GC, no Rc<T>:** the borrow checker is the *whole* memory-safety story; there's no runtime fallback.
- **No comptime:** all checks are static-AST analysis.
- **No macros:** the diagnostic format is structured; no macro layer needed.

---

## 8. Slicing

The Phase-6 borrow-checker work is 5–7 sub-slices. Each lands a working compiler with strictly more programs accepted or rejected.

**Slice 6BC.1 — `BorrowedExclusive` state + intra-call detection (E0380 / E0381 / E0382).** Extend `PlaceState`, extend the conflict matrix, walk every call's argument list and check the four pairwise rules. **Output:** the `modify_both(mut buf, mut buf)` family of programs is now rejected; existing Phase-5 tests still pass.

**Slice 6BC.2 — Cross-statement exclusive-borrow tracking (E0383).** Establish `BorrowedExclusive(name)` claims when a `let` binds the return of a `mut`-source elision (Rules E1-mut / E2-mut). Reject reads, writes, moves of the source while the borrower is live. **Output:** the `let cur = cursor(mut vec); cur.advance(); vec.len();` family is rejected.

**Slice 6BC.3 — Partial-place activation (`Place::projections` becomes live).** Wire the projection-prefix check into all the conflict-detection points. Activate E0374 for partial-place conflicts. **Output:** `modify_both(mut buf.left, mut buf.right)` now succeeds; `mut buf` while `mut buf.left` is live now rejects.

**Slice 6BC.4 — Multi-`mut`-param elision (Rule E3-mut).** Body-flow analysis for `mut`-parameter sources, mirror of Phase 5's 5BC.4. Emit E0384 when inference fails. **Output:** complex `mut`-multi-param functions accept or fail with a useful annotation hint.

**Slice 6BC.5 — Explicit lifetime annotation syntax.** Pick a spelling (§4.2 candidates), parse it, sema-check it, propagate through the borrow checker. Activate E0385 (mismatched annotations) and E0386 (struct field requires annotation). **Output:** every Phase-5 program that was rejected with "wait for Phase 6 annotations" now has a syntactic fix.

**Slice 6BC.codegen — `noalias` tagging.** One-line codegen change per call site; pulls from the borrow checker's signature analysis. **Output:** `cargo bench` on a representative non-aliasing workload shows the gap from clang -O2 close.

**Slice 6BC.opt — Static drop-flag specialization (§5.4).** Read the analyzer's `PlaceState` at scope-exit points; specialize the drop lowering. Opportunistic; can slip. **Output:** drop-flag overhead drops on programs where the static analysis is precise.

Total estimate: 3–4 months focused work. Most of the cost is slice 6BC.5 (the syntax decision + the explicit-annotation surface). The codegen slice is small; the analysis slices are extensions of Phase 5 machinery.

---

## 9. Open questions

1. **Lifetime annotation spelling — CLOSED.** `borrow REGION T` (§4.2). Rejected alternatives (apostrophe-prefix lifetimes, where-clauses) recorded in §4.2 for the reasoning trail.

2. **Iterator invalidation prototype — `VecI32`, not `Vec[T]`.** The Phase-6 exit criterion says "`Vec[T]`-style growable array" but `[T]` is generic-parameter syntax, which is Phase 7. Phase 6's exit demo is hand-rolled for a concrete element type (call it `VecI32`); the iterator-invalidation property is independent of genericity. The borrow checker's exclusive-borrow rule should make `for x in vec { vec.push(...); }` reject automatically: `for x in vec` establishes a shared borrow of `vec`; `vec.push(...)` requires `mut self`, which is a fresh exclusive borrow that conflicts. As long as `VecI32::iter` takes `self` (shared) and `VecI32::push` takes `mut self` (exclusive), the conflict appears at the iteration site. **Verify with a prototype `VecI32` before slice 6BC.5 lands**, so the exit criterion is provable without waiting on Phase 7's generic-parameter machinery. The literal `Vec[T]` form lands when generics do.

3. **Atomic types' interaction with the aliasing rule.** Sibling design note. The summary: `Atomic[T]` is Copy-flavored for the borrow checker (skipped), even though atomic *operations* are non-trivial. The atomic instructions handle the race-freedom; the borrow checker doesn't need to. Specifying the exact rule in the sibling note.

4. **Drop-flag specialization (§5.4) — does it land in Phase 6 or slip?** The runtime flag mechanism is correct without specialization; the optimization is opportunistic. If slice 6BC.5 turns out long, the specialization can slip to Phase 7+. Decision at slicing time.

5. **`Place::projections` for tagged-union variants — CLOSED.** Variant-payload projections (`Projection::Variant(name, idx)`) land alongside slice 6BC.3's partial-place activation. A `match x { Variant(a, b) => ... }` binding `a` against a borrow of `x` produces a sub-borrow tied to `x`; the machinery is the same prefix-comparison §5.2 uses for struct fields. Recorded for slice planning, no design question.

6. **Phase 6 `shared` parameters — `noalias`? CLOSED: no.** The `x: T` shared form is explicitly multi-pointer-able by the aliasing-XOR-mutability rule — two arguments to one call can be the same place, e.g. `f(x, x)`. `noalias` would be unsound regardless of any analysis refinement. Shared parameters get `readonly` instead (mechanically true since sema rejects writes through shared params); see §5.5. The "prove this specific borrow is alone in scope and tag accordingly" refinement is rejected as unsound, not deferred — proving uniqueness in current scope doesn't prove uniqueness across the call (the call's other parameters could be the same place). Rust hit the same wall and ships `&T` as `readonly` only.

7. **`mut self` returning a borrow of a field — CLOSED via 6BC.3.** Default: Rule E2-mut treats `fn buf(mut self) -> Cursor` as exclusive borrow of all of `self`. The 6BC.3 partial-place machinery refines this to "exclusive borrow of `self.buf`" when the body provably restricts mutation to one field; concurrent borrows of `self.payload` then admit. Implementation reuses the projection-prefix check; not a separate design question.

8. **Self-referential struct regions — CLOSED: rejected in Phase 6 first cut.** `struct Node { next: borrow A Node }` does not type-check; the diagnostic suggests an indexed `VecNode` alternative. Rust took years to make self-referential lifetime structs work and the surface complexity is large; Phase 6 ships without it and lifts the restriction if a real motivating case appears. Not deferred-as-unresolved — deliberately deferred-as-rejected.

9. **Re-borrow within a `mut` borrow — CLOSED: admitted as implicit sub-borrow.** `f(mut x: T)` calling `g(mut x)` is a sub-borrow within `f`'s already-exclusive context, not a fresh exclusive borrow against the caller. Argument-position-claim classification admits a `mut`-claim whose source is itself a `mut`-parameter without conflict — the outer claim is the same exclusivity, not a separate one. Rust expresses this via `&mut *x` reborrow syntax; C+ leaves it implicit (no surface markers needed since `mut x: T` already names the parameter as an exclusive borrow that can be sub-borrowed).

10. **Phase 6 and `unsafe` — CLOSED: non-interaction.** `unsafe` stays reserved (since Phase 1) and unactivated through Phase 6. The natural activation slot is Phase 8 (FFI / raw pointers). Phase 6 doesn't make `unsafe` more or less urgent; recorded so the dependency isn't reopened.

11. **Variance — CLOSED with the spelling.** §4.3 commits: invariant in `mut`-borrow position, covariant in shared-borrow position, contravariant in argument position when a function-typed value carries a region. No surface syntax for variance markers; the rules are part of the type-checker's behavior.

12. **Diagnostic prioritization — CLOSED with the slicing.** Slices 6BC.1–6BC.5 emit all borrow errors in source order (naive). Cascading-error suppression (one root cause shouldn't produce many follow-on errors) lands as polish post-Phase-6, when a real example demonstrates the cost. Not a Phase-6 exit blocker.
