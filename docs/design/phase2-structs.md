# Phase 2 Slice 2B — Structs (no methods)

> Status: draft
> Scope: `struct Name { f: T, ... }` declarations, struct literals, field read, field assignment
> Out of scope (slice 2C): methods on structs (the open §11 question on method syntax)
> Out of scope (Phase 7): generic structs, derived equality, struct update syntax `Name { ..other }`

## 1. Problem

Phase 2 slice 2A added enums. Slice 2B adds the other half of "user-defined data": named-field aggregates. With both, programs can build linked lists, parse simple text, walk records — the Phase-2 exit criterion. Methods are a separate slice (2C) because the method-syntax decision is non-trivial and not blocking the data-shape work.

## 2. Syntax

```cp
struct Point { x: i32, y: i32 }
struct Pair  { a: f64, b: f64, }    // trailing comma allowed
struct Empty {}                       // allowed; zero-sized
```

Grammar additions:

```
item             = function | enum_decl | struct_decl ;
struct_decl      = 'struct' ident '{' field_list? '}' ;
field_list       = struct_field (',' struct_field)* ','? ;
struct_field     = ident ':' type ;
```

Struct literal expression:

```
Point { x: 1, y: 2 }
Pair  { a: 1.0, b: 2.0 }
Empty { }
```

Grammar:

```
primary          = ... | struct_lit ;
struct_lit       = ident '{' field_init_list? '}' ;
field_init_list  = field_init (',' field_init)* ','? ;
field_init       = ident ':' expr ;
```

Field access (postfix):

```
postfix          = primary (call_suffix | field_suffix | index_suffix)* ;
field_suffix     = '.' ident ;
```

Field assignment is handled at the expression level — `expr.field = value` re-uses the existing `Assign` machinery; sema validates the target is a *place expression* (see §3).

### 2.1 Struct-literal-vs-block ambiguity

Same problem Rust has: `if cond Foo { x: 1 } { ... }` is ambiguous between "if condition contains a struct literal" and "if condition is `cond Foo`, then a block." We adopt Rust's resolution:

> In **no-struct-literal context** — the head of `if`, `while`, `for in <expr>` (the iter expression) — `Foo { ... }` is parsed as the bare ident `Foo` followed by a block. To force a struct literal in those positions, parenthesize: `if (Foo { x: 1 }) == other { ... }`.

Outside no-struct-literal contexts (let init, function args, return values, normal expression position), `Foo { ... }` is unambiguously a struct literal.

The parser threads a single boolean `no_struct_lit` flag through the relevant entry points.

## 3. Place expressions

A *place expression* is one whose value is stored at a known memory location. The LHS of `=` must be a place. Phase-2 place expressions:

- An `Ident` referring to a mutable local (already supported)
- A `place . field` chain whose root is a mutable local

Anything else (literal, call result, struct literal, arbitrary expression) is **not** a place. Trying to assign to it errors with E0313 (already exists).

The mutability check propagates from the root: `p.x = 5` requires `p` to be `let mut p`. Field-level immutability isn't a thing in Phase 2 (Rust doesn't have it either; the whole struct is mut or it isn't).

## 4. Sema additions

`Ty` gains `Struct(StructId)`, mirroring the enum approach (declaration-order index, `Copy`).

```rust
Ty::Struct(StructId)
```

New error codes:

| Code | Meaning |
|---|---|
| `E0319` | duplicate field name within a single struct |
| `E0320` | unknown field on a struct (in literal or field access) |
| `E0321` | missing field in struct literal |
| `E0322` | extra field in struct literal |
| `E0323` | field access on non-struct receiver |

`E0301` (duplicate type definition) covers struct/enum collisions with each other.

Struct literal type-check:
- Resolve `Name` to a `Ty::Struct(id)`; if not found, E0303 (unknown type).
- Compare provided field names against declared fields:
  - Missing → E0321
  - Extra → E0322
  - Duplicate (same name twice in literal) → E0319 in literal-context
- For each provided field, type-check expr against the declared field type.

Field access type-check:
- Type-check receiver. If not `Ty::Struct(_)`, E0323.
- Look up field name; if absent, E0320.
- Result type is the field type.

Assignment with `Field` target:
- Walk the target chain to find the root `Ident`. If the root isn't a local — or the local isn't `mut` — E0305 (existing).
- Type-check value against the field type.

## 5. Codegen mapping

LLVM emits one named type per struct, declared in the module preamble:

```llvm
%Point = type { i32, i32 }
%Pair  = type { double, double }
```

Struct literal (alloca + per-field stores + load):

```llvm
; let p = Point { x: 1, y: 2 };
%p.addr = alloca %Point
%f0 = getelementptr %Point, ptr %p.addr, i32 0, i32 0
store i32 1, ptr %f0
%f1 = getelementptr %Point, ptr %p.addr, i32 0, i32 1
store i32 2, ptr %f1
```

Field read (`p.x`): GEP + load.
Field assignment (`p.x = 5`): GEP + store.
Pass-by-value: `define i32 @first(%Point %0)` — LLVM handles platform ABI (sret/byval insertion happens at codegen time later if needed; Phase 2 just emits the named-struct type and lets LLVM lower).
Return-by-value: `define %Point @new_point()` — same; LLVM handles.

`getelementptr` is the central instruction. Plan §4.1 already calls it out as the second-biggest LLVM gift after mem2reg. Spend the afternoon on the GEP FAQ before coding this.

For a temporary struct value (e.g., `make_point().x`):
- The call returns a `%Point` SSA value.
- We need a place to GEP into. Stash it in a temporary alloca, store the call result, GEP from there.
- After mem2reg, LLVM cleans this up.

## 6. Sample programs

### 6.1 Must compile and run

[point.cplus](#) — two-component struct, field read, struct passed by value:

```cp
struct Point { x: i32, y: i32 }

fn distance_squared(a: Point, b: Point) -> i32 {
    let dx: i32 = a.x - b.x;
    let dy: i32 = a.y - b.y;
    dx * dx + dy * dy
}

fn main() -> i32 {
    let origin: Point = Point { x: 0, y: 0 };
    let p: Point = Point { x: 3, y: 4 };
    #println(distance_squared(origin, p));   // 25
    0
}
```

[mutable_struct.cplus](#) — field assignment requires `let mut`:

```cp
struct Counter { count: i32, max: i32 }

fn main() -> i32 {
    let mut c: Counter = Counter { count: 0, max: 10 };
    while c.count < c.max {
        c.count = c.count + 1;
    }
    #println(c.count);   // 10
    0
}
```

[nested.cplus](#) — struct field is itself a struct:

```cp
struct Point { x: i32, y: i32 }
struct Line  { from: Point, to: Point }

fn main() -> i32 {
    let line: Line = Line {
        from: Point { x: 0, y: 0 },
        to:   Point { x: 5, y: 12 },
    };
    #println(line.to.x + line.to.y);   // 17
    0
}
```

### 6.2 Must reject

| Program | Error |
|---|---|
| `struct E { x: i32, x: i32 }` | E0319 duplicate field |
| `struct A { x: i32 } fn f(a: A) -> i32 { a.y }` | E0320 unknown field |
| `struct A { x: i32 } fn f() -> A { A { } }` | E0321 missing field |
| `struct A { x: i32 } fn f() -> A { A { x: 1, y: 2 } }` | E0322 extra field |
| `fn main() -> i32 { 1.x }` | E0323 field access on non-struct |
| `struct A { x: i32 } fn f() { let a = A { x: 1 }; a.x = 2; }` | E0305 non-mut assignment |
| `struct A { x: i32 } fn f() { A { x: 1 }.x = 2; }` | E0313 LHS is not a place |
| `struct Point {} struct Point {}` | E0301 duplicate type definition |

## 7. Implementation order

1. AST — `ItemKind::Struct(StructDecl)`, `StructDecl { name, fields }`, `StructField { name, ty, span }`, `ExprKind::StructLit { name, fields }` (`fields: Vec<(Ident, Expr)>`), `ExprKind::Field { receiver, name }`.
2. Parser — add `parse_struct_decl`; extend `parse_postfix` for `.field`; extend `parse_primary` so an Ident followed by `{` *outside no-struct-lit context* is a struct literal; thread a `no_struct_lit` boolean through `if`-cond / `while`-cond / `for-in` parsing.
3. Sema — extend `Ty` with `Struct(StructId)`; collect structs; resolve struct names; type-check struct literals (missing/extra/duplicate/wrong-type fields); type-check field access; extend assignment to support `Field` targets via a "find root mutable local" walk.
4. Codegen — extend `EnumTable` to a unified `TypeTable` holding both enums and structs; emit `%Name = type { ... }` declarations in the preamble; emit GEP-based codegen for struct literal, field read, field write; handle struct pass-by-value and return-by-value via the LLVM struct type.
5. Tests — comprehensive: parser tests for struct decl + literal + field; sema positives for the three sample programs; sema negatives for every error code in §6.2; codegen tests asserting specific GEP / alloca / load / store IR; e2e tests that compile and run the three samples.

## 8. Out of scope (this slice, with rationale)

- **Methods.** Slice 2C; needs the method-syntax decision (`impl` vs UFCS).
- **Generic structs.** Phase 7 needs the type-parameter machinery.
- **Derived equality / ordering.** A `==` between structs is not generated; users write their own. Phase 7 traits or Phase 5 attribute-derive may revisit.
- **Update syntax** `Foo { x: 1, ..other }`. Useful but not foundational; revisit later.
- **Anonymous tuple structs** `Point(i32, i32)`. Skip — named fields only in Phase 2.
- **Public/private field visibility.** Phase 4 with modules.

## 9. Open questions

- [ ] Should empty structs `Empty {}` be allowed? Lean: yes — they're useful for marker types and traits later. Costs nothing.
- [ ] Field-init shorthand `Point { x, y }` (when local names match field names)? Lean: no for Phase 2. Add later if it earns its keep.
- [ ] Struct equality auto-derived? Lean: no, force users to write it. Phase 7 may add `derive(Eq)`.
