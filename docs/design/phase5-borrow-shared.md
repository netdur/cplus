# Phase 5 — Shared-borrow tracking

> Status: design note. Implementation lands in 4–6 sub-slices per the Phase 5 sequencing block in [plan.md](../../plan.md) §3.
> Scope: tracking the *shared* borrow form (`x: T` and `self` on non-`Copy` types); detecting conflicts between shared borrows and concurrent moves; choosing the minimum lifetime-elision rule set that admits common signatures without explicit annotations; specifying the borrow-checker diagnostic surface.
> Out of scope: exclusive-borrow tracking (Phase 6), explicit lifetime annotation syntax (Phase 6), `noalias` codegen (Phase 6), atomic types (Phase 6), heap allocation / `Vec[T]` (Phase 5+).

This is the highest-risk design work in the project — it sets the surface every later phase compiles against. The note is longer than other Phase-5 notes deliberately.

---

## 1. Problem

Phase 3 introduced surface syntax for the three ownership kinds (`x: T` shared, `mut x: T` exclusive, `move x: T` consuming — §2.9) and linear move-tracking inside a function body. What it did **not** do:

1. Detect conflicts between a shared borrow of a place and a concurrent move of the same place. The Phase-3 checker is linear — it consumes a binding when it sees a `move`-marked call, but it doesn't notice that an earlier or sibling argument in the same expression already reads the same binding.
2. Track flow-sensitive merging across control flow. If branch A moves `x` and branch B doesn't, the post-`if` state is wrong-by-construction; Phase 3 punts.
3. Track shared borrows across function boundaries. A function returning `string` could legitimately want to return one of its `string` parameters (without copying), and the caller needs to know how long the result is valid. Without this, every non-`Copy` return is implicitly an owned copy — fine semantically but a known performance regression vs. equivalent C.

Each of these is a real bug surface today. (3) is the load-bearing one: the Phase-5 exit criterion is "`fn longest(xs: string, ys: string) -> string` works without annotations." Achieving that requires the language to admit some form of return-borrow flow, and the elision rules to handle the common case silently.

The constraint that shapes everything: **C+ has no `&T` / `&mut T` reference type (§2.9).** Borrowing is expressed by parameter form. There is no place in the source where the user writes a reference type, no `<'a>` lifetime parameter (yet — Phase 6), no `'static`. Rust's borrow checker design assumes references-as-first-class-types; C+'s does not.

This note specifies what the Phase-5 borrow checker checks, what surface (if any) the user sees beyond the existing ownership markers, what elision the compiler does silently, and what diagnostic shape the user sees when something fails.

---

## 2. What a "shared borrow" is in C+

Restating from §2.9 so the rest of the note has a firm referent:

- A parameter `x: T` where `T` is non-`Copy` is a **shared borrow** of T. The function may read `x`; the caller retains ownership; many concurrent shared borrows of the same place are fine.
- A parameter `x: T` where `T` is `Copy` (primitives, plain enums, structs whose fields are all `Copy`) is a **pass-by-value copy**. The borrow checker doesn't track it; it carries no aliasing constraints. We say it nominally as "Copy values pass through transparently."
- Method receiver `self` follows the same pattern with the same Copy-ness logic.

**The Phase-3 model is unsound under aliasing.** It accepted patterns like:

```cp
fn f(a: Buffer, b: Buffer) -> i32 { ... }    // both shared borrows
fn main() {
    let buf = make_buffer();
    consume(move buf);    // moves buf
    let x = buf.len();    // E0335 from Phase-3 linear tracking ✓
}
```

But did **not** reject:

```cp
fn drain(move b: Buffer) -> Bytes { ... }
fn peek(b: Buffer) -> u8 { ... }
fn main() {
    let buf = make_buffer();
    let bytes = drain(move buf, peek(buf));    // pre-Phase-5: silently accepted
}
```

In the second example `buf` is both moved (consumed) and shared-borrowed in the same call expression. Phase 3 doesn't track this because argument evaluation produces two reads of the binding `buf` and the move flag only flips after the whole call. Phase 5 must reject it.

The Phase-5 checker works at **place granularity** — not just at binding granularity. A "place" is a path from a root binding through field accesses and array indices: `buf`, `buf.payload`, `buf.parts[3]`. Shared-borrow conflicts are checked at the place level; the same field of two different bindings is two different places.

---

## 3. Conflicts the Phase-5 checker rejects

Phase 5 rejects three families of conflicts. Phase 6 will add a fourth (exclusive vs. anything).

### 3.1 Move-during-shared-borrow

Within a single expression, an argument position cannot move a place while a sibling argument position shared-borrows the same place.

```cp
drain(move buf, peek(buf));    // E0354 (proposed code)
```

Implementation: argument evaluation order is left-to-right (locked in for §2.9 / the codegen note); the borrow checker walks the call's argument list, classifies each argument as Read / Mut / Move based on the callee's signature, and flags any place that appears in more than one position with at least one being Move (or one being Mut once Phase 6 lands). For Phase 5 specifically: any place that is `Move`d in one argument and `Read` (shared-borrowed or used in an expression) in another argument of the same call is rejected.

The same rule applies to method receivers: `buf.into_string(write(buf))` is rejected if `into_string` takes `move self` and `write` reads `buf`.

### 3.2 Move-then-use across statements (already Phase 3, tightened)

Phase 3 catches `consume(move x); #println(x);` as E0335 (use after move). Phase 5 keeps this behavior but extends the flow-sensitive analysis:

- Flow merging across branches: if `if c { consume(move x); } else { /* no move */ }`, then after the `if`, `x`'s state is "possibly moved." Any read of `x` past that point is **E0355** (use of possibly-moved binding) — distinct from E0335 (definitely moved on all paths).
- Loops: a `move` inside a `while` / `for` / `loop` body that does not unconditionally precede the loop's exit makes `x` "possibly moved" after the loop. A `move` that *unconditionally* runs is "definitely moved" only if the loop runs at least once; conservative Phase-5 rule treats moves inside loops as "possibly" unless the loop is provably non-empty (which we don't try to prove). This is the same conservatism Rust applies; revisit if real cases hit it.
- `match` arms are treated like `if` branches — the post-match state is the intersection of per-arm states.

E0335 vs. E0355 distinction matters for diagnostic quality: "you moved it on this branch and tried to use it after" is a different fix than "you moved it here." The diagnostic format in §6 carries both forms.

### 3.3 Returning a place that is shared-borrowed elsewhere

This is the new Phase-5 conflict and the one that motivates lifetime tracking. Consider:

```cp
fn longest(xs: string, ys: string) -> string {
    if xs.len() > ys.len() {
        return xs;    // returns a borrow of the caller's xs
    } else {
        return ys;    // returns a borrow of the caller's ys
    }
}
```

If `string` is non-`Copy` (string heap-allocates, so it is), then returning `xs` cannot semantically move it — `xs` is a *borrow*, not an owned value. The caller still owns the string after `longest` returns. So what does the return value's lifetime look like?

The Phase-5 model: **the return of a non-`Copy` value derived from a borrowed parameter is itself a borrow tied to that parameter's lifetime.** The caller's binding receiving the return value is constrained to live no longer than the parameter it derives from.

Concretely:

```cp
fn main() {
    let a = read_string();
    let b = read_string();
    let r = longest(a, b);    // r borrows a and b
    drop_or_consume(move a);  // E0356 — would invalidate r
    #println(r);
}
```

The borrow checker emits **E0356** ("cannot move `a` while `r` borrows from it") at the `drop_or_consume` line, because `r` is still live.

This is where lifetime inference / elision comes in: the user didn't write any lifetime, but the compiler had to figure out that `r` borrows from both `a` and `b`. §4 specifies the elision rules.

---

## 4. Lifetime inference and elision

**Lifetimes in C+ are inferred, not written, in Phase 5.** Explicit lifetime annotation syntax is deferred to Phase 6 (per §11 of the plan — `<'a>` is taken by Rust and we may pick alternative spelling). Phase 5 picks a minimum rule set that admits common signatures without annotations.

The bias, per the Phase-5 kickoff (§3 of the plan): **elide less rather than more.** Conservative-now-relax-later is a one-way door we can walk through; permissive-now-tighten-later is a migration we want to avoid. If we accept signatures that turn out to be ambiguous later, every project written against the loose rule has to be retrained. If we reject signatures that could safely be accepted later, we just relax the rule and existing code continues to compile.

### 4.1 The model

Every shared-borrow parameter `x: T` (non-`Copy`) has an implicit lifetime. Every non-`Copy` return value has an implicit lifetime. The borrow checker tracks which return-lifetime is constrained by which parameter-lifetimes.

Phase 5 introduces three elision rules, in priority order:

**Rule E1 (single-parameter elision).** If a function takes exactly one non-`Copy` shared-borrow parameter and returns a non-`Copy` value, the return's lifetime is the parameter's:

```cp
fn head(buf: Buffer) -> Bytes { ... }
// elided to: return-lifetime = lifetime of buf
```

This matches Rust's first elision rule and covers a huge fraction of accessor-style code (`.first()`, `.head()`, `.tail()`, `.as_bytes()`).

**Rule E2 (self elision).** If a method takes `self` (or `mut self` — Phase 6) and returns a non-`Copy` value, the return's lifetime is `self`'s. Both must be non-`Copy`.

```cp
impl Buffer {
    fn payload(self) -> Bytes { ... }
    // elided to: return-lifetime = self's
}
```

Covers all the typical getter / accessor methods. Same logic as Rust's third elision rule.

**Rule E3 (no return-borrow if ambiguous).** If a function has two or more non-`Copy` shared-borrow parameters and returns a non-`Copy` value, **the return is treated as an *owned new* value, not a borrow** — unless the function body provably returns the same path on every path through it.

This is the `longest` case. The body returns either `xs` or `ys` depending on the branch. The compiler reaches a fixed-point: the return's lifetime is the *intersection* of `xs`'s and `ys`'s lifetimes (i.e. the return is valid only while both `xs` and `ys` are valid). The caller therefore has to keep both alive until the return goes out of scope. The Phase-5 implementation does this by treating multi-source returns as if a hypothetical `'r: 'xs + 'ys` constraint is in force — but expressed entirely through the move-checker's state rather than through any user-visible annotation.

If the function body returns a non-`Copy` constructed-on-the-spot value (e.g. `return Bytes::new();`) the return lifetime is `'static` and the return is owned. The body-flow analysis decides; the function signature alone doesn't.

**This is the spot where E3 picks "elide less."** A simpler permissive rule would be: "multi-parameter returns get the intersection of all parameter lifetimes, no body analysis required." That admits more signatures but reads incorrect for `fn make_thing(a: A, b: B) -> Thing { return Thing::new(); }` (where the return doesn't borrow from `a` or `b` at all but the caller is gratuitously constrained). Phase 5 takes the slightly stricter form: do body-flow analysis to figure out what the return actually borrows from. If we later regret the cost or the diagnostic complexity, relaxing to "all parameters" is a one-line change with no semantic impact on programs that already compile.

### 4.2 What is *not* elided in Phase 5

- **Stored-borrow return.** If a function tries to return a value that borrows from data inside one parameter (`fn first_field(p: Pair) -> Item { return p.first; }`), the borrow checker accepts it under Rule E1, but storing the returned `Item` in a *struct field* — extending its lifetime beyond the borrowed place's — is rejected: Phase 5 has no syntax for the user to express "the struct field borrows for as long as the outer borrow lives." Phase 6 introduces explicit lifetime annotations precisely to handle this case. For now: structs cannot have non-`Copy` borrow-typed fields; this is a deliberate restriction.
- **Returning a borrow from a moved-in parameter.** `fn f(move x: T) -> Bytes { return x.payload; }` returns a borrow of `x.payload`, but `x` is owned-by-the-function, so the borrow's lifetime is the function frame's, not the caller's. The return is invalid the moment the function returns. Phase 5 rejects this with **E0357** ("cannot return a borrow of a moved-in parameter"). Phase 6 may relax this if some pattern motivates it; current view is "never relax this, owned-then-borrowed-out is a use-after-free."
- **Cross-function lifetime threading.** If `outer` calls `inner` which returns a borrow, and `outer` wants to return that same borrow, Phase 5 admits it only when Rule E1 / E2 cover both call sites. Anything more complex needs Phase 6 annotations.

### 4.3 Why no syntax in Phase 5

Putting `<'a>` (or whatever C+'s eventual spelling is) into Phase 5 means: (a) every borrow-rejecting diagnostic has to teach the user what lifetimes mean; (b) the elision rules become "did you write a lifetime? if not, here's the rule" which doubles the surface. The conservative path is to introduce the *concept* through diagnostics in Phase 5 — when E3 rejects a signature, the diagnostic says "the return appears to borrow from both `xs` and `ys`, which Phase 5 cannot prove; consider returning an owned `String::from(...)` or wait for Phase 6's explicit lifetime annotations." Then Phase 6 introduces the syntax and the diagnostic switches to "consider annotating: `fn longest<...>(...)`."

Two-phase rollout of one feature, but each phase has a clean small surface. Worth the cost.

---

## 5. The check itself (algorithm)

This section sketches the analysis at the level of "what data structures, what passes." Implementation details go in the slice's commit messages.

### 5.1 Place expressions

A *place* is the path the user wrote to reach a value. Grammar:

```
place = ident
      | place '.' field
      | place '[' index ']'
```

The borrow checker indexes by canonical place strings (`buf`, `buf.payload`, `buf.parts[3]` where `3` is a constant; for non-constant indices we conservatively treat `buf.parts[*]` as a single place — same as Rust's MIR borrow checker for the same reason).

### 5.2 Place state

Each tracked place is in one of:

- **Owned** — fully owned, can be moved, can be borrowed.
- **Borrowed-shared(N)** — N ≥ 1 shared borrows are live. Cannot be moved, can be additionally shared-borrowed.
- **Moved** — has been consumed. Cannot be read, borrowed, or re-moved.
- **MaybePartial** — definite assignment said yes-on-some-paths, no-on-others. Cannot be read; can be assigned.

Phase 6 adds **Borrowed-exclusive** with an aliasing-XOR-mutability conflict against everything else.

### 5.3 The pass

A new module `cplus-core/src/borrowck.rs` (after the existing `lower.rs`, before / interleaved with `sema.rs`'s expression checking) runs a flow-sensitive analysis per function body. For each statement:

1. Compute the set of places read, written, moved, or borrowed-shared.
2. Compose with the pre-statement place-state map; check no transition is illegal.
3. Emit the post-statement place-state map.

At control flow joins (`if`/`else` merge, end of `match`, post-loop), intersect the per-branch state maps. The intersect rule: Owned ∩ Owned = Owned; Owned ∩ Moved = MaybePartial; Borrowed-shared(M) ∩ Borrowed-shared(N) = Borrowed-shared(max(M, N)) — though in practice the static analysis tracks "at least one borrow live" rather than counting.

The pass shares its flow infrastructure with the slice-3J definite-assignment pass — the snapshot/restore/intersect helpers extend to a richer per-place state vector.

### 5.4 Interprocedural

Phase 5 is intraprocedural (within one function body) for actual conflict detection. The interprocedural information needed — "function `f` returns a borrow from its first parameter" — comes from the elision rules in §4 applied to the *signature*, not from analyzing the body of `f` from the caller's perspective. This is the standard "modular borrow checking" property and the reason the rules in §4.1 are tied to signatures.

The function body of `f` is checked once, against its own signature; if the body returns a borrow whose source-place disagrees with the elision result, the function-body check rejects it. So callers can rely on the signature.

### 5.5 Copy fast path

The very first thing the pass does on any expression is consult `SemaCx::is_copy(&Ty)` (the slice-3C structural Copy check). If the place's type is Copy, the pass skips it entirely — Copy types carry no aliasing constraints, so there's nothing to track. This keeps the cost proportional to the non-Copy surface of the program, not the total number of expressions.

---

## 6. Diagnostic surface

The borrow-checker diagnostic surface is **the long pole of error-message quality** for Phase 5 (and Phase 6). This is the project's first encounter with the multi-place, multi-statement conflicts that Rust spent years polishing. Getting the form right here pays dividends for every later phase.

Constraints, from §5.9 (AI recovery) and §5.2 (structured diagnostics):

- **Smallest useful span.** A use-after-move points at the use, not at the move; the move appears as a `note:` at its own line.
- **Suggestion-first attempt.** Every borrow-checker error tries to attach a `MaybeIncorrect` suggestion: "consider cloning the value here," "consider passing as `mut` rather than `move`," "consider reordering arguments." Even imperfect suggestions help agents iterate.
- **Cause + effect both surfaced.** The primary span is the use that failed. Secondary spans (rendered as `note:` lines per the diagnostic JSON shape) point at the move / borrow that caused the conflict.

### 6.1 Proposed error codes

| Code | Meaning | Phase |
|------|---------|-------|
| E0335 | Use of moved binding (already exists; Phase 3) | 3 |
| E0337 | Move out of non-binding place (already exists; Phase 3) | 3 |
| E0354 | Move and shared borrow of the same place in one call | 5 |
| E0355 | Use of possibly-moved binding (one branch moved, another didn't) | 5 |
| E0356 | Move while a shared borrow is live | 5 |
| E0357 | Return of borrow from a moved-in parameter | 5 |
| E0358 | Borrow-conflict in a partial-move place (e.g. moved `buf.parts` then read `buf`) | 5 |
| E0359 | Reserved for a future "complex elision" diagnostic (Phase 5+ Rule E3 failure) | 5 |

(E0354–E0360 were tentatively reserved for the attributes design note; the attributes note actually uses E0354–E0360, so the borrow checker numbering needs to start higher. Renumbering at implementation time — recording the conflict here so it's caught before we burn a code in code.)

**Resolution note**: E0354–E0360 are owned by [phase5-attributes.md](phase5-attributes.md). This note's borrow-checker codes should start at **E0370** and proceed upward. Plan.md §11 needs to reflect this when the slice lands.

### 6.2 Sample diagnostics

**E0356 — move-during-borrow.**

```
error[E0370]: cannot move `a` while it is shared-borrowed
   --> main.cplus:7:5
    |
  5 | let r = longest(a, b);
    |                 - shared borrow of `a` extends through this binding
  6 |
  7 | drop(move a);
    |      ^^^^^^ `a` moved here
    |
  = note: the value returned by `longest` borrows from `a`; while that
          return value (here bound to `r`) is alive, `a` cannot be moved.
  = help: drop `r` before moving `a`, or clone `r` if you need both:
                let r_owned: String = r.clone();
```

**E0354 — argument-position conflict.**

```
error[E0354]: argument moves and shared-borrows the same place
  --> main.cplus:3:11
   |
 3 | drain(move buf, peek(buf));
   |       --------  -------- this argument reads `buf`
   |       |
   |       this argument consumes `buf`
   |
 = help: split the call into two statements:
              let peeked = peek(buf);
              drain(move buf, peeked);
```

The structure of every borrow-checker diagnostic: **primary span = the failing operation; one secondary span = the conflicting prior operation; help = either a suggested fix or a "wait for Phase 6 annotations" pointer.**

### 6.3 Why not unify with E0335 / E0337

Phase 3's E0335 (use after move) is structurally a single-place / single-binding error. Phase 5's E0356 introduces a multi-place / multi-line story: the *cause* is in a different statement from the *failure*. Lumping them risks diagnostic muddle. Keep them as separate codes; the message text can share infrastructure without sharing identifiers.

---

## 7. Interactions

With every Phase 1–4 feature:

- **§2.4 errors-are-values:** unaffected. Tagged-union return values are non-`Copy` aggregates that go through the borrow checker like any other non-`Copy` return.
- **`if let` / `guard let` (slice 4A.5):** lowered to `match` before the borrow checker sees them. The pattern bindings count as new places, owned by their arm scope. `guard let Ok(v) = x else { return; };` binds `v` as a new owned place in the enclosing scope; the original `x` is moved into the match scrutinee per slice-3I rules. Borrow checker sees this as ordinary move.
- **`break` / `continue` / `loop` / `while let` (slice 4-end):** flow merging extends — loops still apply the §3.2 "possibly moved" conservatism. `continue` and `break` cut control flow; the borrow-state intersect runs over all paths leading to the loop's exit point.
- **`defer` (slice 3G):** a `defer EXPR;` registers `EXPR` to run at scope exit. Phase 5 borrow-checks the deferred expression *at its registration site* using the place-state that will be in force at scope exit. If `defer #println(buf);` happens after a `move buf;`, the registration is fine but the scope-exit will run on a moved binding — Phase 5 rejects with E0355 / E0335 attributed at the `defer` site. Recording this here because it's a subtlety that needs a test the moment the slice lands.
- **Drop (slice 3F):** the drop-flag machinery in codegen already handles the "moved or not" question at runtime. Phase 5 makes the *static* analysis catch more cases, but the runtime drop flag is still the source of truth for actual drop suppression. No codegen changes needed in Phase 5; this is purely a sema-time check.
- **Modules / `pub` (slice 4B):** signatures cross file boundaries via the resolver's qualified names. The borrow checker reads each function's signature when checking call sites; same-file vs. cross-file is invisible to it. Cross-file borrow errors render via the per-file source threading already in place (slice 4C).
- **Formatter (slice 4D):** borrow-checker errors carry source spans that point at the original file. No formatter interaction beyond rendering.
- **LSP (slice 4E):** borrow-checker errors flow through the same `Diagnostic` pipeline as sema errors. The LSP picks them up automatically. Code-action quick-fixes from borrow-checker suggestions land alongside sema suggestions.

With Phase 5 sibling features:

- **Attributes ([phase5-attributes.md](phase5-attributes.md)):** `#[test] fn` bodies are borrow-checked normally. Synthesized doctest functions (per [phase5-doctests.md](phase5-doctests.md)) are borrow-checked normally. No carve-outs.

With Phase 6 (forward-looking):

- **Exclusive borrow tracking on `mut x: T`:** drops in alongside the Phase-5 conflict types as a fourth conflict class. Most of the place-state machinery is reused; the new state is **Borrowed-exclusive(at most 1)** with conflict against every other state.
- **Aliasing-XOR-mutability:** a place is *either* shared-borrowed (any count) *or* exclusive-borrowed (one), never both. Same machinery, expressed as: "Borrowed-exclusive conflicts with Borrowed-shared, Owned-after-mutation, and any future move." Phase 5's state machine is forward-compatible.
- **Explicit lifetime annotations:** Phase 6 syntax (`<'a>` or alternative — see §11 of plan.md) extends elision Rule E3 with user-written constraints. Anywhere Phase 5 rejects with "Rule E3 cannot prove the borrow source," Phase 6 admits with an explicit annotation.
- **`noalias` codegen:** Phase 6 takes the Phase-5 proof and propagates it to LLVM. Phase 5's checker output (place-state per program point) is the input; the codegen change is mostly attribute-tagging.

With deliberate non-features:

- **No macros (§2.8):** borrow-checker output is structured; no macro layer would need to consume it.
- **No comptime (§1.2):** borrow checking is fully a static-analysis-on-AST pass; no const-fn interaction.
- **No GC, no Rc<T>:** the borrow checker's job is to make memory safety provable *without* a runtime fallback. C+'s `Drop` (slice 3F) is the only runtime hook, and Phase 5 doesn't extend it.

---

## 8. Slicing

The borrow checker is large enough that it should land in 4–6 sub-slices, each landing a working compiler with strictly more programs accepted (or strictly more programs rejected — that's the borrow checker's positive contribution).

**Slice 5BC.1 — Place-state machinery.** Introduce the `borrowck.rs` module, define `Place` / `PlaceState`, walk function bodies and produce a state map at every program point. **Output:** no behavior change (the analysis runs but its conclusions aren't used). Test: dump the analysis state and verify it matches expected on small inputs.

**Slice 5BC.2 — Move-after-move / shared-then-move within a single function body.** Activate E0335 in its full flow-sensitive form; introduce E0355 (possibly-moved) and E0354 (argument-position conflict). **Output:** the example programs that pass Phase 3 still pass; some previously-accepted programs are now rejected with precise diagnostics. Update samples + tests.

**Slice 5BC.3 — Single-parameter elision (Rule E1) + self-return elision (Rule E2).** Track return-borrow flow for accessor-style methods. Add E0357 (return-of-borrow-of-moved-in) and E0356 (move-while-shared-borrow-live). **Output:** `fn payload(self) -> Bytes` accepts; caller-side move-after-return is rejected.

**Slice 5BC.4 — Multi-parameter elision (Rule E3) with body-flow analysis.** The `longest` case. Run the function body to determine which parameter(s) the return value borrows from. **Output:** `fn longest(xs: string, ys: string) -> string` accepts.

**Slice 5BC.5 — Partial-place tracking (`buf.field`, `arr[const]`).** Lift the per-binding tracking to per-place. Add E0358 (partial-move read of containing). **Output:** more programs accepted (you can move `pair.first` and still read `pair.second`); more programs rejected (reading `pair` after moving `pair.first` is E0358).

**Slice 5BC.6 (optional) — Diagnostic polish + samples.** Once the rules are in place, sweep every borrow-checker error site, verify the diagnostic structure follows §6, and write 5–10 new samples that exercise the rejected patterns explicitly (each is a negative test plus a "what you meant to write" positive sample).

Total estimate: 3–5 weeks of focused work after the design note is review-approved. Most of the cost is diagnostic polish, not the analysis itself.

---

## 9. Open questions

1. **`Place` canonicalization for non-constant array indices.** The proposed conservative rule (`buf.parts[*]` is a single place for any non-constant index) gives up precision. Rust's MIR borrow checker does the same; only with const-eval can it refine. C+ has no comptime so const-eval is bounded. Acceptable Phase-5 conservatism; revisit if real workloads hit it.

2. **Diagnostic when a `move` inside a loop body causes a borrow conflict on the *second* iteration.** The error materializes only after one trip through the loop. The diagnostic should explain "this `move` is reached on the second iteration when `x` has already been moved on the first." Tricky to phrase well. Land with simpler text and iterate; this is a known weakness in every borrow checker.

3. **Method-receiver returning a borrow tied to a non-receiver parameter.** `fn render(self, ctx: Ctx) -> Bytes` — does the return borrow from `self`, from `ctx`, or both? Rule E2 says `self`-only. If the body actually returns `ctx.buffer`, the function-body check rejects the signature with E0357 ("return borrows from parameter not covered by elision"). User has to either restructure or wait for Phase 6 annotations. Acceptable.

4. **Renumbering E0354–E0360.** As §6.1 notes, the attributes design note already allocated these codes. Borrow-checker codes should use E0370+. plan.md §11 needs the correction recorded; this note's §6.1 table is the right shape but the prefix shifts at implementation time.

5. **Should `assert EXPR;` evaluate `EXPR` borrow-checked?** Yes — `assert buf.len() > 0;` reads `buf`, the borrow checker sees the read like any other. If `assert` is later given a hidden side-channel (e.g. failure-reporting), the side-channel is borrow-checked the same way. No carve-out for asserts.

6. **Re-borrow of a borrow.** If `f(x: Buffer)` calls `g(x)` (also taking `Buffer`), is that a fresh borrow or a sub-borrow? In Phase 5 with no explicit lifetimes the question doesn't materialize — both are just "the function reads x during the call." Phase 6 will need to revisit when explicit lifetimes can express "the inner borrow lives within the outer."

7. **Doctests and borrow checking.** Synthesized doctest functions (from [phase5-doctests.md](phase5-doctests.md)) are normal functions; they borrow-check. The interaction with same-file access (doctests can call private helpers) means doctest writers will hit borrow errors on private invariants the author didn't expect. Worth catching in the doctest slice; not a borrow-checker design question.

8. **Performance of the analysis itself.** Place-state intersection at every join point can blow up on functions with many bindings × many branches. Bound via "per-place coarsening" — if too many places are tracked, fall back to whole-binding granularity. Implementation detail; not a design question. Measure at slice 5BC.5 once partial-place tracking lands.

9. **Should Phase 5 ship `Vec[T]`-style growable arrays?** Plan.md Phase 6 exit says "a `Vec[T]`-style growable array compiles cleanly and rejects iterator invalidation." That requires both the borrow checker (shared and exclusive) and heap types. Phase 5 lands the shared-borrow checker; the heap-types question is separate (likely a slice paired with the borrow-checker rollout, possibly in Phase 6). Recording the dependency so the slicing doesn't accidentally land the type system without the safety.

10. **Lifetime spelling for Phase 6.** Out of scope for this note but noted because the diagnostic-text suggestions in §6 reference "Phase 6's explicit lifetime annotations." Whatever spelling Phase 6 picks, the diagnostic text needs an update — don't bake `<'a>` into Phase 5's strings.
