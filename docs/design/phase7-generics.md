# Phase 7 — Generics + interfaces

> Status: design note. Implementation lands in 5–6 sub-slices per the Phase 7 sequencing block in [plan.md](../../plan.md) §3.
> Scope: parametric polymorphism (type parameters on functions and types); bounded polymorphism (interface bounds on type parameters); interface declaration and implementation syntax; monomorphization at codegen.
> Out of scope: dynamic dispatch / interface objects (no `dyn` equivalent — Phase 7 is monomorphization-only); associated types; higher-kinded types; const generics; specialization; auto-derive attributes; heap allocation primitives (sibling slice, see §8).
>
> Depends on: [phase3-copy-derivation.md](phase3-copy-derivation.md) (structural Copy generalizes to generic instantiations); [phase6-borrow-exclusive.md](phase6-borrow-exclusive.md) (borrow checking runs on monomorphized code).

---

## 1. Problem

Phase 6 ended with `VecI32` — a fixed-capacity inline analogue of a growable vector — because writing `Vec[T]` for any element type required the generic-parameter machinery this phase introduces. The same shape applies broadly:

- Containers: `Vec[T]`, `Option[T]`, `Result[T, E]`, `Pair[A, B]`, `Map[K, V]`.
- Algorithmic primitives: `max[T](a: T, b: T) -> T`, sorting, searching, hashing.
- Composable interfaces: `Ord`, `Eq`, `Hash`, `Clone` — types declare what operations they support; functions take type parameters constrained to support those operations.

Without generics, every variation requires copy-paste and rename. The cost grows linearly with the type matrix. Every modern systems language has solved this; the question is *how*.

C+'s answer has two halves:

1. **Parametric polymorphism** — functions and types take type parameters in `[T]` brackets; the compiler generates a separate concrete version of each function for every set of concrete type arguments it's called with (monomorphization).
2. **Bounded polymorphism via interfaces** — type parameters can require that their concrete types implement specific operations; the type checker enforces this at the call site.

This phase is conceptually large but mechanically contained: most of the work is in the parser, type substitution, and codegen's monomorphization queue. The borrow checker, sema, codegen-of-primitives, and LSP are all unchanged in spirit; they just see more types.

---

## 2. Syntax

### 2.1 Generic functions

```cp
fn max[T: Ord](a: T, b: T) -> T {
    if a.compare(b) > 0 { return a; }
    return b;
}

fn identity[T](x: T) -> T { return x; }

fn make_pair[A, B](a: A, b: B) -> Pair[A, B] {
    return Pair { first: a, second: b };
}
```

The generic-parameter list sits in square brackets immediately after the function name, before the value-parameter list. Each parameter is a name optionally followed by `: BOUND` (or `: BOUND1 + BOUND2 + ...` for multiple bounds).

**Why square brackets, not angle brackets `<T>`:**
- Angle brackets collide with comparison operators. The C++ `vector<vector<int>>` close-bracket parse hack is the canonical example; Rust uses `::<>` turbofish at call sites to work around the same issue. Square brackets sidestep both.
- C+ already reserves `Vec[T]`-style syntax (mentioned in Phase 1 grammar reservations). Phase 1's lexer admits the brackets without ambiguity since `[T; N]` array types and `[T]` generic-parameter lists never appear in the same syntactic slot — array types follow `:` in type position; generic-parameter lists follow an identifier at item-definition or call-site positions.
- The `borrow REGION T` syntax (Phase 6 slice 6BC.5) deliberately avoids brackets to leave them for generics. The two annotation surfaces (borrow regions, generic types) are orthogonal and coexist on one signature: `fn first[T](xs: borrow A Vec[T]) -> borrow A T`.

### 2.2 Generic types

```cp
struct Pair[A, B] {
    first: A,
    second: B,
}

struct Vec[T] {
    data: *T,
    len: usize,
    cap: usize,
}

enum Option[T] {
    Some(T),
    None,
}

enum Result[T, E] {
    Ok(T),
    Err(E),
}
```

Generic structs and enums take parameters in brackets immediately after the type name. Bounds on declaration apply to every use of the type:

```cp
struct SortedList[T: Ord] {
    items: Vec[T],
}
```

`SortedList[Point]` requires `Point: Ord`; absent that impl, the type is rejected at instantiation (**E0502**, §7).

### 2.3 Interface declarations

```cp
interface Ord {
    fn compare(self, other: Self) -> i32;
}

interface Eq {
    fn eq(self, other: Self) -> bool;
}

interface Hash {
    fn hash(self) -> u64;
}

interface Clone {
    fn clone(self) -> Self;
}
```

New keyword: `interface`. The body is a list of method signatures (no bodies). `Self` is a magic type referring to the implementing type — replaced with the concrete type at `impl`-resolution time.

Why `interface` instead of Rust's `trait`:
- `trait` historically carries connotations of mixins and inheritance; `interface` is purer (Java, Go, TypeScript all use `interface` for the same concept).
- The keyword is already reserved (Phase 1 grammar reservations list `trait`, but `interface` is not yet reserved — Phase 7 adds it). One new keyword is the cost.
- `interface` reads more naturally as "this names a set of operations that an implementing type must support."

Receivers in interface methods: `self`, `mut self`, `move self`, or no receiver (associated functions). The `Self` type may appear in parameter positions and return positions. Default method bodies (Rust's `default fn`) are **deferred** — Phase 7 first cut requires every implementation to define every declared method.

### 2.4 Interface implementations

```cp
struct Point { x: i32, y: i32 }

impl P for Ordoint {
    fn compare(self, other: Point) -> i32 {
        let dx: i32 = self.x - other.x;
        if dx != 0 { return dx; }
        return self.y - other.y;
    }
}

impl P for Eqoint {
    fn eq(self, other: Point) -> bool {
        return self.x == other.x && self.y == other.y;
    }
}
```

The `impl T for InterfaceNameypeName { ... }` block lists method implementations. Every method declared in the interface must be implemented (**E0503**). Extra methods (not in the interface) are rejected — they belong in an inherent `impl Type { ... }` block (**E0504**). Method signatures must match exactly, with `Self` substituted to the implementing type (**E0505**).

A type can have multiple `impl T for Interfaceype` blocks for different interfaces, but at most one `impl T for Interfaceype` for any given (interface, type) pair (**E0506**).

### 2.5 Multiple bounds

```cp
fn sorted_max[T: Ord + Eq](xs: Vec[T]) -> T { ... }
```

The `+` separates bounds. Order is not semantically significant.

### 2.6 Where clauses — deferred

Complex bounds may eventually use `where` clauses:

```cp
fn complex[T, U] where T: Ord, U: Clone { ... }
```

Phase 7 first cut ships only inline bounds in the bracket list. `where`-clause syntax is deferred until inline bounds prove insufficient. Likely trigger: real user code with many parameters and bounds that exceeds a reasonable line length.

### 2.7 Generic methods inside `impl` — deferred

```cp
impl Vec[T] {
    fn map[U](self, f: fn(T) -> U) -> Vec[U] { ... }  // deferred
}
```

A method that introduces additional type parameters beyond the `impl`'s declared list. Useful for iterators and combinators. Deferred because (a) it doesn't block the Phase-7 exit criterion (a `Vec[T]` doesn't *require* method-level generics — only its `iter` family does), and (b) it interacts with function-pointer / closure syntax which is its own deferred design.

### 2.8 Associated types — deferred

Rust's `Iterator { type Item; fn next(&mut self) -> Option<Self::Item>; }` — an interface declares an associated type that each implementation specifies. Powerful but non-trivial. Phase 7 ships only method-only interfaces; associated types deferred to a later slice where the motivating cases (custom iterators with non-`T`-element types) actually appear.

### 2.9 Const generics — deferred

`Vec[T; N]` style — a constant integer as a type-parameter. Phase 7 admits only type parameters. Const generics are additive — they can land later without breaking existing Phase-7 code.

### 2.10 Self type

The `Self` keyword (capital S) refers to the implementing type inside `interface` and `impl ... for ...` blocks. Inside an inherent `impl Type { ... }` block, `Self` is also admitted as a synonym for `Type` (matches Rust's convention).

`Self` is not admitted at function-body or free-fn positions — it only makes sense in `interface` / `impl` contexts. Use outside those triggers **E0508**.

---

## 3. Semantics

### 3.1 Monomorphization

The compiler generates a separate concrete LLVM function for each `(generic_fn, [concrete_types])` pair reached from the program. **Name mangling**: arguments listed in source order, separated by `__`:

```
max[i32]                              → @max__i32
max[Point]                            → @max__Point
Pair[i32, Vec[u8]]                    → @Pair__i32__Vec__u8
Pair[i32, Vec[u8]]::new               → @Pair__i32__Vec__u8.new
Vec[i32].push                         → @Vec__i32.push
identity[Pair[i32, i32]]              → @identity__Pair__i32__i32
```

The mangling rule: type arguments flatten recursively. Mangled symbols can grow pathological for deeply nested generics; this is a known cost (same shape Rust accepts). Deterministic and uniquely-decodable.

**Codegen work-queue**: walk the program. For each generic call, infer concrete type arguments. Add `(generic_fn_name, [concrete_types])` to a `BTreeSet`. Process the queue: for each entry, substitute type parameters in the body and codegen the concrete LLVM function. Continue until the queue is empty. Deduplication is built-in via the set.

### 3.2 Generic-type instantiation

Generic struct/enum types get one LLVM type per unique instantiation:

```
Pair[i32, i32]       → %Pair__i32__i32      = type { i32, i32 }
Pair[i32, Buffer]    → %Pair__i32__Buffer   = type { i32, %Buffer }
Vec[i32]             → %Vec__i32            = type { ptr, i64, i64 }
```

Same `BTreeSet`-based work-queue covers types.

### 3.3 Type inference at call sites

Most generic call sites don't write type arguments explicitly:

```cp
let m: i32 = max(3, 7);              // T = i32 inferred from args
let p: Pair[i32, string] = make_pair(7, "hi");   // A = i32, B = string inferred
```

The type checker propagates inference bidirectionally. When inference fails (e.g., the result is discarded, no annotation, no usage hint), the user must write the type argument explicitly via the **`name::[T]`** syntax:

```cp
let n: i32 = identity::[i32](0);
```

The `::[T]` form is the "turbofish" equivalent — the `::` separator (already used for paths) plus the bracket list. Calling without explicit arguments and without enough context to infer is **E0509**.

Why not just `name[T](args)` at the call site? Because `name[T]` collides with array-indexing syntax in expression position (`arr[i]`). The `::` clearly separates the segments. Rust's `name::<T>(args)` solves the same problem; C+ uses `::[T]` for consistency with `::`-for-namespace.

### 3.4 Bound checking

When a generic function or type declares a bound `T: Ord`, the type checker verifies at each instantiation that the concrete type implements `Ord`:

1. Inferred `T = Point`.
2. Scan for an `impl P for Ordoint` block.
3. If found, the call resolves; method calls like `a.compare(b)` inside the generic body resolve to `Point.compare` via the impl.
4. If not found, fire **E0502** ("type `Point` does not implement interface `Ord`") at the call site.

Multiple bounds (`T: Ord + Eq`) require every named interface to have an impl for the concrete type.

### 3.5 Self in interface methods

In an interface method, `Self` is a placeholder. At `impl T for Interfaceype`, every `Self` is substituted to `Type`. The implementation's method signature must match the substituted interface signature exactly:

```cp
interface Clone {
    fn clone(self) -> Self;
}

impl P for Cloneoint {
    fn clone(self) -> Point { return Point { x: self.x, y: self.y }; }
    //               ^^^^^ Self substituted to Point
}
```

Signature mismatch (wrong receiver kind, wrong parameter types, wrong return type) is **E0505**.

### 3.6 Coherence (orphan rule)

An `impl T for InterfaceNameypeName` can be defined only in the file that defines `InterfaceName` *or* the file that defines `TypeName`. A third file that imports both cannot add the impl.

This prevents incoherent overlap: two different files providing different `impl P for Ordoint` blocks. Rust calls this the "orphan rule" and ships it for the same reason. Loosening (negative reasoning, specialization, fundamental types) deferred indefinitely — first cut keeps the strict version.

Fires **E0507** at the orphan `impl`.

### 3.7 Compiler-blessed interfaces

Some interfaces have semantic meaning the compiler enforces, distinct from user-defined interfaces:

- **`Copy`** — already structural per §2.9. Phase 7 surfaces it as an interface name for bound purposes (`fn duplicate[T: Copy](x: T) -> T`), but **users cannot write `impl X for Copy { }`**. Auto-derived structurally; a manual impl fires a "Copy is structural — derived automatically, not implemented" diagnostic.
- **`Drop`** — magic method (Phase 3 slice 3F). Phase 7 surfaces it as an interface, but `impl X for Drop { fn drop(mut self) { ... } }` is the existing magic-method form — same shape, just nominally an interface impl. The compiler recognizes the impl by name and folds it into the existing Drop machinery.

The other interfaces (Eq, Ord, Hash, Clone, Display) have no compiler magic — they're user-declared and user-implemented. The compiler ships the *declarations* in a blessed module so users don't have to redefine them; the implementations are the user's job. (Auto-impl for primitives is a compiler-internal hack — primitives implement Eq/Ord/Hash via codegen-generated impls that user code can rely on.)

---

## 4. Codegen

### 4.1 Monomorphization queue

Codegen gains a work-queue:

```rust
struct MonoQueue {
    pending: BTreeSet<MonoItem>,
    emitted: BTreeSet<MonoItem>,
}

struct MonoItem {
    template_name: String,   // "max" or "Pair" or "Vec.push"
    type_args: Vec<Ty>,
}
```

The driver:
1. Seed the queue with every `fn main` invocation's transitive call set (concretized starting from `main`'s body).
2. Pop items; substitute type parameters in the AST node; codegen the concrete LLVM function with the mangled name.
3. New generic calls encountered during codegen of one item add to the queue.
4. Stop when pending is empty.

Determinism guaranteed by `BTreeSet` ordering.

### 4.2 No vtables, no dynamic dispatch

Phase 7 is **monomorphization-only**. Every generic call statically resolves to a concrete function. No `dyn Trait` equivalent, no virtual-method tables, no boxed trait objects.

The motivation: dynamic dispatch is its own design decision (ABI, runtime layout, cost model). Adding it requires a separate design note. Phase 7 ships the static side cleanly; dynamic dispatch is additive — when it lands (Phase 8+ or later), it joins the existing static surface, doesn't replace it.

### 4.3 LLVM features

No new LLVM features for Phase 7 itself — monomorphization happens at AST level, then codegen emits standard LLVM IR per concrete item. The performance gain is from optimizer-friendly concrete types; the runtime cost is zero (no indirection).

---

## 5. Diagnostic surface

| Code | Meaning |
|------|---------|
| E0500 | Unknown generic parameter |
| E0501 | Wrong number of type arguments at instantiation |
| E0502 | Type does not satisfy interface bound |
| E0503 | Interface implementation missing required methods |
| E0504 | Interface implementation has extra methods |
| E0505 | Interface method signature mismatch |
| E0506 | Duplicate `impl T for Interfaceype` blocks |
| E0507 | Orphan `impl` — interface and type defined in other files |
| E0508 | Use of `Self` outside an interface/impl context |
| E0509 | Type inference fails — explicit `::[T]` arguments required |
| E0510 | Cannot manually `impl X for Copy` — Copy is structural |
| E0511 | `interface` keyword expected (parser-level) |

The numbering picks up from Phase 6's E0386 with a gap (E0500 is round enough to leave room for Phase 7 polish slices).

---

## 6. Interactions

### 6.1 Borrow checker

The borrow checker (Phase 5/6) runs on the monomorphized AST. Per-instantiation analysis is correct because Copy/Drop/non-Copy status varies with type arguments — `Pair[i32, i32]` is Copy; `Pair[Buffer, i32]` is not.

Generic functions are *not* borrow-checked pre-monomorphization in Phase 7 first cut. The reason: Copy-ness depends on concrete arguments. A generic `fn identity[T](x: T) -> T { return x; }` is fine for Copy T; for non-Copy T it's a move (and an `x.something()` after `return x;` would be E0335 use-of-moved). The right time to check is after substitution.

A consequence: borrow-check errors on a generic function can fire at the call site, not the definition site, depending on which instantiation triggers the issue. Diagnostic-quality polish lives in a future slice.

### 6.2 Copy auto-derive

Structural Copy generalizes naturally. `struct Pair[A, B] { first: A, second: B }`:
- `Pair[i32, i32]` is Copy (both fields are Copy).
- `Pair[i32, Buffer]` is not Copy (Buffer is non-Copy / Drop).

The codegen Copy oracle (Phase 6 slice 6BC.codegen) gets a generic-aware version that pattern-matches on the substituted concrete fields.

### 6.3 Modules + `pub`

Generic items can be `pub fn` / `pub struct` / `pub enum` / `pub interface`. The signature exports across files; instantiation happens at the call site in whichever file invokes the generic. No cross-file orphan issues by §3.6.

### 6.4 Tests + doctests

Generic functions cannot directly be `#[test]` — `#[test]` requires `fn() -> i32` or `fn()` (Phase 5 slice 5ATTR.2). A user tests a generic by writing a concrete-typed wrapper test fn. Doctests on generic items work the same way — the fence content monomorphizes.

### 6.5 LSP

Hover on a generic instantiation should show the concrete type. Phase-7 first cut doesn't ship this; goto-definition jumps to the generic declaration. LSP polish slice (post-Phase-7) wires the inference output into hover.

### 6.6 Formatter

`fn name[T: Ord + Eq](a: T) -> T` formats with single spaces around `:` and `+` in the bracket list. Square brackets are tight (no inner padding). `::[T]` turbofish at call sites also tight. Lands in a 4D.2-style formatter polish slice.

---

## 7. Slicing

Phase 7 work is naturally sliced 5–7 sub-slices.

**Slice 7GEN.1 — Generic-fn parsing + AST.** Lex `[` after fn name without ambiguity (the existing `[T; N]` array-type form is distinguishable by position). Parse `fn name[T, U](args)` and bounds `fn name[T: Ord + Eq](args)`. AST gains `generic_params: Vec<GenericParam>` on `Function` with each `GenericParam { name: Ident, bounds: Vec<Ident> }`. No semantics yet; sema stays generics-blind and codegen panics on any generic-call attempt.

**Slice 7GEN.2 — Generic-type parsing.** Same for `struct Name[T] { ... }` and `enum Name[T] { ... }`. AST gains parallel `generic_params` field on struct/enum decls.

**Slice 7GEN.3 — Interface declaration + impl.** New `interface` keyword. New `interface Name { fn ... }` syntax and `impl T for Interfaceype { fn ... }` syntax. AST `Item::Interface(InterfaceDecl)` variant. Sema validates: every interface-declared method has a matching impl (E0503), no extras (E0504), signature exact match with Self substituted (E0505), single-impl-per-(interface, type) (E0506), coherence (E0507).

**Slice 7GEN.4 — Sema integration: type-parameter substitution + bound checking.** When sema sees a generic-fn call or generic-type instantiation, it builds a substitution map (param → concrete type) and verifies each declared bound has a matching impl (E0502). When inference fails, emits E0509 with a `name::[T]`-form suggestion. Self resolves inside interface/impl bodies (E0508 outside). Wrong arg count: E0501.

**Slice 7GEN.5 — Codegen monomorphization.** Work-queue, name mangling, per-instantiation IR generation. Generic struct types get unique LLVM `%Type__args` definitions. Methods on generic types mangle to `Type__args.method` form. Copy oracle generalizes to consult substituted concrete fields.

**Slice 7GEN.6 — Blessed interface declarations.** Compiler ships built-in declarations of `Copy`, `Drop`, `Eq`, `Ord`, `Hash`, `Clone` in a synthetic module. Primitives auto-impl Eq/Ord/Hash via compiler-internal codegen. User `impl X for Copy` rejected with E0510. User `impl X for Drop { fn drop(mut self) { ... } }` folds into the existing magic-method path.

**Slice 7HEAP — Heap allocation primitives** (parallel slice — may land before or after Phase 7 work depending on `Vec[T]` exit-criterion sequencing). Adds an `Allocator` interface and the `Box[T]` / `Vec[T]` types that use it. The §2.2 commitment to allocator-as-parameter (Zig pattern) realizes here. Its own design note.

**Slice 7EXIT — Generic `Vec[T]` exit demo.** The Phase-6 `VecI32` becomes generic `Vec[T]` using 7HEAP's allocator. Iterator invalidation still rejected statically. Sample [docs/examples/vec_generic.cplus] demonstrates `Vec[i32]`, `Vec[Point]`, `Vec[Pair[i32, string]]`.

Total estimate: 2–4 months (per plan.md §3). Slices 7GEN.1–7GEN.3 are mechanically straightforward (parser + AST). Slice 7GEN.4 is the substantial type-system work. 7GEN.5 (codegen monomorphization) is the second-most involved. 7HEAP is its own separate effort.

---

## 8. Open questions

1. **Heap allocation sequencing.** The Phase-7 exit demo `Vec[T]` needs heap allocation. Two paths:
   - (a) Land 7GEN.1–6 first, then 7HEAP, then 7EXIT. Generics are usable for inline types (`Pair[A, B]`, `Option[T]` over Copy types) without heap.
   - (b) Co-land 7HEAP alongside 7GEN.5 so the exit demo follows immediately. Larger upfront chunk, no dependency wait.
   - Pick at slice-planning time. Default: path (a) — generics-without-heap is independently useful and reduces the per-slice risk.

2. **Allocator interface design.** Zig-style allocator-as-parameter requires defining the `Allocator` interface. Its own design note. Open questions: arena vs. general-purpose, error handling on OOM (return `Option[T]` or trap?), alignment requirements, deallocation method shape (`free(ptr)` vs. `drop_alloc(self, ptr, size, align)`).

3. **Where the blessed interfaces live.** Phase 7 ships compiler-blessed declarations of `Copy`, `Eq`, `Ord`, `Hash`, `Clone`. These need to *exist* somewhere users can reference (`fn max[T: Ord]` requires `Ord` to be resolvable). Options: (a) synthetic module always-imported (no user-visible file); (b) a stdlib prelude file imported by every project (requires §2.6's "no stdlib" decision to relax); (c) compiler-builtin names like primitive type names (`Ord` is keyword-like). Lean toward (a) for first cut — the names are reserved and resolve to compiler-internal declarations.

4. **Auto-derived Eq/Ord/Hash.** Should the compiler auto-derive these for structs/enums when every field implements them? Rust uses `#[derive(Eq, Ord, Hash)]` which is a decorator — rejected by C+'s §2.8d. The alternative is structural auto-derive (same shape as Copy): if every field implements `Eq`, the containing struct auto-implements `Eq` field-wise. Phase 7 first cut: **no auto-derive** for Eq/Ord/Hash; user writes explicit impls. This is the conservative choice (matches §2.8c verbosity-exposes-rules) and can be relaxed later by ruling auto-derive in.

5. **Generic methods inside `impl` blocks (§2.7).** Deferred. The first motivating case is iterator combinators (`Vec[T].map[U]`). Phase 7 first cut requires methods to use only the impl's declared type parameters.

6. **Negative bounds (`T: !Drop`).** Allow a generic to require its type parameter is *not* Drop. Useful for compile-time-known no-cleanup paths. Rust doesn't have this; first-cut C+ doesn't either. The motivating use case isn't yet clear in C+'s context.

7. **`Self` in inherent impls.** Admitted as a synonym for the impl's target type — `impl Point { fn new() -> Self { ... } }` works. Matches Rust convention. No new design question; recording the decision so it's not relitigated.

8. **`impl` on generic types: full bound propagation.** A `struct Vec[T] { ... }` with `impl Vec[T] { ... }` — the `T` in the impl block must match the struct's declaration. Bounds on the struct (e.g., `struct SortedList[T: Ord]`) propagate to the impl: `impl SortedList[T] { ... }` inherits `T: Ord` without restating. If the impl restates the bound (`impl SortedList[T: Ord] { ... }`), it must match (no narrowing or widening allowed in first cut).

9. **Type inference scope.** Phase 7 inference is local to each call site — argument types propagate to type-parameter inference, no project-wide whole-program inference. Rust's local inference rule. Matches C+'s "locality of reasoning" goal (§5.8). Globally-typed inference deferred indefinitely.

10. **Variance for generic types.** Rust has invariant/covariant/contravariant rules for generic type parameters that interact with borrows. Phase 6 §4.3 committed C+'s variance rules for `borrow REGION T`; Phase 7's generic-type variance is the parallel question for *type* parameters (not regions). First cut: invariant for every type parameter — the simplest rule, conservatively sound. Refining to co/contra-variance is additive; first cut ships invariance.

11. **Specialization, overlapping impls.** Forbidden. Phase 7 first cut rejects all overlap. Specialization (Rust-experimental) is deferred forever unless a use case appears.

12. **Higher-kinded types.** `fn fmap[F[_], T, U]` where `F` is itself a generic-type-with-one-parameter. Rust doesn't have this. C+ Phase 7 doesn't have this. Probably never (the cost-benefit is bad — adds large complexity, narrow use cases).

13. **Const generics.** `Vec[T; N]` as a type-parameter list with N being a usize constant. Defer to a later phase if real workloads demand it. Adding const generics is additive (doesn't break existing type-param code) so the delay is cheap.

14. **Coherence relaxation.** Strict orphan rule §3.6 may eventually be relaxed (negative reasoning à la Rust's "fundamental" trait attribute, or wholly-bypass mechanisms). First cut: strict. Loosen if a real user need appears.

---

## 9. Recap of what Phase 7 unblocks

- `Vec[T]`, `Option[T]`, `Result[T, E]`, `Pair[A, B]`, `Map[K, V]` — the standard container suite as language-level types. (Whether they live in a stdlib or just in user code remains §2.6's question.)
- The Phase-6 `VecI32` upgrades to generic `Vec[T]` once 7HEAP lands.
- Algorithmic primitives: `max[T: Ord]`, `min[T: Ord]`, `sort[T: Ord]`, hashing, generic comparison.
- User-defined interfaces. Anyone designing a domain abstraction can declare an interface and implement it for the types that should participate.
- The path to dynamic dispatch (`dyn Interface`) — Phase 8+ if motivated, additive over the monomorphization mechanism.
- Stdlib design (an open §2.6 question post-Phase-7) — once generics exist, the stdlib's shape becomes tractable.

Phase 7 is the largest single feature C+ has shipped since the borrow checker (Phase 5+6). It's structurally contained — most of the work is in the parser, sema's substitution machinery, and codegen's monomorphization queue — but conceptually load-bearing. Every later phase builds on the generic mechanism this phase introduces.
