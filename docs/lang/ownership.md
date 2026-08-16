# Ownership — how values travel

The judgment companion to [memory-model.md](memory-model.md), which is
normative — when this page and that one disagree, that one wins. This page
exists to answer the question the error message can't: *which* redesign to
pick. Quick start in [tour.md](tour.md) §6; signatures in [ref.md](ref.md).

## 1. The model in four sentences

Every value has exactly one owner, and the owner's scope exit runs its
destructor. A non-Copy value moves — the compiler never duplicates it,
because a second bitwise copy would free the same resource twice. Borrowing
is a parameter mode, not a type: bare = shared read, `ref` = exclusive
write-back, `take` = move in. At any moment a value has either one exclusive
borrow or any number of shared borrows — never both.

## 2. Copy is structural, and you never write it

A type is Copy when every component is Copy and there is no `drop`.
Primitives, raw pointers, `str`, slices, and aggregates of those: Copy.
Anything owning heap (`Text`, `Vec[T]`, `Box[T]`, a struct containing one):
non-Copy, move-only. Writing `fn drop(ref this)` makes a type non-Copy on
purpose — that is how a resource-holding struct opts out of duplication.

Practical consequence: adding a `Text` field to a Copy struct silently makes
every existing pass-by-value site a move site. The compiler will point at
each (E0335/E0337); expect that churn when a struct grows its first owning
field.

## 3. Choosing a parameter mode

| Callee's need | Mode | Cost model |
|---|---|---|
| read | `x: T` (bare) | pointer pass; zero copies for any size |
| mutate in place | `ref x: T` | pointer pass; caller's place must be `var` |
| keep / return / destroy | `take x: T` | move; caller's binding dies |

Rules of thumb:

- **Start bare.** It is a borrow for every type, so there is no
  "big struct → pass by reference" decision to make. Promote to `take` only
  when the compiler proves you need it (E0337), or the API's meaning is
  transfer (`Vec::append`, a constructor argument stored in a field).
- **`ref` is for write-back, not for cheapness.** A bare parameter is
  already a pointer underneath.
- **`take` + return is the pipeline shape**: `fn appending(take this, s: str)
  -> Text` consumes and returns, letting callers chain without clones.

The same three shapes on receivers: `this`, `ref this`, `take this`. A
mutating method demands a `var` receiver (E0328) — the caller's declaration
is part of the API contract.

## 4. Escapes: E0337 and its two fixes

A bare borrow lives only for the call. Returning it, storing it in a field
or global, or re-passing it to a `take` parameter is E0337. The message
offers `take` or `.clone()`, and the choice is semantic, not mechanical:

- `take` when the caller is handing the value over — the natural reading of
  "append this to the list", "set this as the name".
- `.clone()` when both sides genuinely keep one — and then the allocation is
  in the source where a reviewer can see it.

Return values need no marker: returning an owned *local* moves it out.

## 5. Views: the `str` ↔ `Text` borrow

Every read on a string returns a **view** (`str`) into the owner's buffer —
`t.trim()`, `t.slice(from:, to:)`, the `Text`→`str` coercion itself. Views
are the highest-traffic borrow in the language, with three rules:

1. **A view at a binding needs a named owner.** A temporary has no lifetime
   to lend:

   ```cplus
   let s: str = t.clone();          // E0513 — the clone is an anonymous temp
   let owner: Text = t.clone();     // name it…
   let s: str = { owner.view() };   // …then view it
   f("x = ${n}");                   // an ARGUMENT's temp outlives its call — fine
   ```

2. **While a view lives, the owner is write-locked.** Reads stay fine;
   mutating, moving, or dropping the owner is rejected. The borrow ends at
   the view's **last use**, not scope end, so use-then-append compiles:

   ```cplus
   let v: str = t.trim();
   use(v);                          // last use — borrow ends here
   let _s: status::Status = t.append("!");   // fine
   ```

   A use inside a loop pins the borrow past the loop; a use in a `defer` or
   a block tail pins it to scope exit. When a borrow "won't die", look for
   the late use.

3. **Leaving the borrow costs one spelled copy**: `.to_text()`.

Structs that carry views inherit their borrows — a `struct Row { title: str }`
built from `t` locks `t` exactly as the bare view would.

## 6. What drops, when

Scope exit runs teardown in reverse declaration order, and `defer` shares
the same LIFO stack. For each owning value: the user `drop(ref this)` first
(if any), then each owning field, recursively, in reverse field order.
Tagged-enum payloads drop through a tag switch. You write per-field cleanup
never; you write `drop` only to release things the compiler can't see —
raw pointers, FFI handles.

The rules that follow from auto-drop:

- **You cannot move a field out of an owning aggregate** (E0509) — the
  auto-drop would free it twice. Clone the field, or consume the whole
  value with a `match`.
- **A consuming `match` is triggered by binding.** Matching an owned enum
  with a name in any pattern consumes it (payload becomes yours, its drop
  is suppressed, the binding is dead afterwards — E0335 on reuse).
  `Some(_)` binds nothing and only reads the tag; `Some(_v)` **binds** —
  the underscore prefix is privacy, not a wildcard. Presence-check first,
  match for real after, is the two-step idiom.
- **In `guard let`, the else block can't re-match the scrutinee** — its
  destructor already ran. Capture the complement instead:
  `guard let E::Ok(v) = e else |E::Err(x)| { … };`
- **`take this` does not disarm exit-drop.** A consuming method returns the
  payload and lets scope exit free the shell; calling `free` yourself
  double-frees.

## 7. Raw pointers: accountability, not ceremony

Every `*T` struct field must be accounted for (E0510): either a `drop` that
releases it, or the `opaque` marker meaning "another owner frees this".
The compiler checks the `drop` body structurally — unconditional release
(or null-guarded on the same field) is clean; a conditional release
(refcounts) is W0002, expected for `Rc`-shaped types; no release at all is
the error. `opaque` is for genuine borrows: an FFI handle the runtime owns,
a view into a sibling's allocation.

Rule of thumb: the moment you type `*T` into a struct, decide who frees it,
and write that decision down as `drop` or `opaque` in the same edit.

## 8. When the borrow checker says no

The fix ladder, cheapest first:

1. **Shorten the borrow with a `{ }` scope** — most conflicts are a borrow
   living one statement too long.
2. **Make the binding `var`** — E0328 is usually just this.
3. **Reorder: reads before writes** — batch the `let n = v.count()` reads,
   then mutate.
4. **`.clone()`** — and accept the visible allocation.
5. **Restructure ownership** — some conflicts are the design telling you two
   things want to own one value. Give it one owner and hand the other a key
   (an index, an id) instead of a pointer.

Not every conflict is fixable by scoping; the ladder's last rung is real.

## 9. Lending across threads

`thread::spawn_with` **moves** its data in — the safe default.
`thread::scope()` lends instead: `s.lend::[T](value, worker)` passes the
local by `ref`, and the scope's own drop joins every worker before the loan
ends. The three races this could create are compile errors, not bugs: the
lent value dying first (E0514), writing while lent (E0381), lending one
place to two workers (E0381). A worker that must produce a value is
`spawn_with`'s job — scoped workers report through the lent value.
