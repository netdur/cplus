# Phase 2 Slice 2A — Plain Enums + Path Expressions

> Status: draft
> Scope: `enum Name { V1, V2, ... }` declarations + `Name::Variant` path expressions, no payloads
> Out of scope (next slices): payload-carrying enums (Phase 3 tagged unions), structs (2B), methods (2C)

## 1. Problem

C+ currently has only built-in primitive types. There's no way to declare a finite set of named values — you write integer constants and lose all type-safety. Plain enums are a small, self-contained addition that:

- Gives users a way to name a closed set of values
- Lays the grammar groundwork for paths (`Foo::Bar`), which Phase-3 tagged unions, Phase-4 modules, and Phase-7 traits all reuse
- Stays simple: no payloads, no discriminators, no derive

## 2. Syntax

```cp
enum Color { Red, Green, Blue }
enum Direction { North, South, East, West, }    // trailing comma allowed
```

Grammar additions:

```
item             = function | enum_decl ;
enum_decl        = 'enum' ident '{' variant_list? '}' ;
variant_list     = ident (',' ident)* ','? ;
```

Variant access via a two-segment path expression:

```
Color::Red
Direction::North
```

```
primary          = ... | path_expr ;
path_expr        = ident '::' ident ;       // exactly two segments in 2A
```

A single bare `ident` is still an Ident expression. Paths require the `::`. Phase 4 (modules) generalizes paths to N segments.

## 3. Semantics

- An `enum` declaration introduces a new type into the program's top-level scope. Sema's enum table grows.
- Variant names must be unique within their enum. Two different enums may share variant names; `A::Red` and `B::Red` are distinct values of distinct types.
- The type of `Color::Red` is `Color`, not `i32`.
- Equality (`==`, `!=`) on enum values: same enum required, returns `bool`.
- Ordering (`<` `<=` etc.) on enums: **rejected** in Phase 2A (E0302). Per §2.8 we don't add ordering by accident; if needed, opt-in via Phase 7 trait derive.
- Cast `Color::Red as i32` is allowed and yields the variant's index (0, 1, 2, ... in declaration order). The cast table from slice 1 grows by one row.
- Cast `0 as Color` (int → enum) is **rejected** in Phase 2A (E0315). It needs a runtime range check or an `unsafe` escape hatch; revisit later.
- No discriminator syntax (`enum E { X = 5 }`). Phase 3 may add when tagged unions land.
- No nested enums. No associated functions (Phase 2C / 7).

New error codes:

| Code | Meaning |
|---|---|
| `E0317` | unknown enum variant in path expression |
| `E0318` | duplicate variant name within an enum |

## 4. LLVM mapping

Each enum lowers to **`i32`**. For Phase 2A we don't optimize for small enums (a 4-variant enum could be `i2`); LLVM will narrow as part of optimization where it matters. Variants are integer constants `0`, `1`, `2`, ... in declaration order.

| C+ | LLVM |
|---|---|
| `enum Color { Red, Green, Blue }` (the type) | `i32` |
| `Color::Red` | the constant `i32 0` |
| `Color::Green` | the constant `i32 1` |
| `c == Color::Red` | `icmp eq i32 %c, 0` |
| `Color::Red as i32` | no-op (already i32) |

The enum-name → variant-index table lives in both the sema and codegen contexts. Both modules walk `program.items` in declaration order, so they assign matching `EnumId` indices without any explicit sharing.

## 5. AST + sema additions

**AST** ([ast.rs](../../cplus-core/src/ast.rs)):

```rust
ItemKind::Enum(EnumDecl)
EnumDecl { name: Ident, variants: Vec<Ident> }

ExprKind::Path { segments: Vec<Ident> }   // for now always 2 segments
```

**`Ty`** in sema gains:

```rust
Ty::Enum(EnumId)         // EnumId(u32) — index into the enum table
```

`EnumId` is `Copy` so `Ty` stays `Copy`. The enum table is a `Vec<EnumDef>` keyed by `EnumId(0..)`, plus a `HashMap<String, EnumId>` for name lookup.

## 6. Sample programs

### 6.1 Must compile and run

[direction.cplus](../examples/direction.cplus):

```cp
enum Direction { North, South, East, West }

fn opposite(d: Direction) -> Direction {
    if d == Direction::North { Direction::South }
    else if d == Direction::South { Direction::North }
    else if d == Direction::East { Direction::West }
    else { Direction::East }
}

fn main() -> i32 {
    let d: Direction = opposite(Direction::North);
    #println(d as i32);   // South = 1
    0
}
```

Expected output: `1`.

### 6.2 Must reject

| Program | Error |
|---|---|
| `enum E { A, A }` | E0318 duplicate variant |
| `Color::Purple` when `Color` doesn't have `Purple` | E0317 unknown variant |
| `Foo::Bar` when `Foo` not declared | E0303 unknown type |
| `Color::Red < Color::Green` | E0302 ordering on non-numeric |
| `let x: i32 = Color::Red` | E0302 type mismatch |
| `let c: Color = 0` | E0302 type mismatch |
| `let c: Color = 0 as Color` | E0315 invalid cast |

## 7. Implementation order

1. AST: add `ItemKind::Enum(EnumDecl)`, `ExprKind::Path { segments }`. The `EnumDecl` struct holds `name: Ident, variants: Vec<Ident>, span: Span`.
2. Parser:
   - Extend `parse_item` to dispatch on `enum`.
   - New `parse_enum_decl`.
   - Extend `parse_primary`: when an Ident is followed by `::`, switch to path-parsing.
3. Sema:
   - Add `Ty::Enum(EnumId)`.
   - Build enum table during `collect_functions` equivalent (rename to `collect_items`).
   - Reject duplicate variants (E0318) at collection time.
   - `resolve_type`: enum names resolve to `Ty::Enum(id)`.
   - New `check_path`: resolve `A::B` to a value of type `Ty::Enum(id)`; emit E0317 if `B` not in `A`.
   - Update `check_binary` for `<`/`<=`/etc. to reject enum operands.
   - Update `check_cast` to accept `enum → integer` (only).
4. Codegen:
   - Build matching enum table.
   - `llvm_ty(Ty::Enum(_))` → `"i32"`.
   - `gen_expr` on `ExprKind::Path`: look up variant index, emit as `i32 N` constant.
   - `gen_cast` already handles `int → int`; the enum→i32 case is the same code path once we treat enum-as-i32 at the LLVM level.
5. Tests:
   - Parser: enum decl parses; path expression parses
   - Sema: positive (declared enum + variant works); negative for each new error case
   - Codegen: enum constant lowers to i32 literal
   - E2E: [direction.cplus](#) compiles and prints `1`

## 8. Open questions

- [x] Ordering on enums — rejected. Future feature gated on traits.
- [x] Int → enum cast — rejected. Needs range check.
- [x] Discriminators — deferred to Phase 3.
- [ ] Should the enum table be available to consumers (LSP) via JSON dump? Defer with the rest of the AST/IR JSON work.
