# Phase 2 Slice 2C — Methods on Structs (`impl` blocks)

> Status: draft
> Scope: inherent methods on structs via `impl` blocks, with three receiver forms (`self`, `&self`, `&mut self`)
> Out of scope: traits / interfaces (Phase 7), generic methods (Phase 7), multiple `impl` blocks per type (deferred), default impls

## 1. Problem

Slice 2B added structs but only as data. Real systems work needs methods: `point.translate(dx, dy)`, `vec.push(x)`, `Type::new(...)`. We resolved the open `§11` method-syntax question in favor of Rust-style `impl` because: the borrow checker (Phase 5/6) needs `&self`/`&mut self` to express ownership in signatures; Phase 7 traits use the same `impl` syntax; UFCS conflicts with the §2.8 no-overloading rule.

This slice is *only* inherent methods — no traits, no defaults. Phase 7 will add `impl T for Traitype` using the same machinery.

## 2. Syntax

```cp
struct Point { x: i32, y: i32 }

impl Point {
    fn new(x: i32, y: i32) -> Point { Point { x: x, y: y } }
    fn magnitude(&self) -> i32 { self.x * self.x + self.y * self.y }
    fn translate(&mut self, dx: i32, dy: i32) {
        self.x = self.x + dx;
        self.y = self.y + dy;
    }
}
```

Grammar additions:

```
item             = function | enum_decl | struct_decl | impl_block ;
impl_block       = 'impl' ident '{' method* '}' ;
method           = 'fn' ident '(' method_params? ')' ('->' type)? block ;
method_params    = receiver (',' param_list)?
                 | param_list ;
receiver         = 'self'
                 | '&' 'self'
                 | '&' 'mut' 'self' ;
```

A method *with* a receiver is an instance method. A method *without* one is an "associated function" (constructor-like). Both live in the same `impl Type { ... }` block.

Call sites:

```
p.translate(1, 1)        // instance method on a place
Point::new(3, 4)         // associated function via `Type::method` path
```

`p.method(args)` is already representable in the existing AST as `Call { callee: Field { receiver: p, name: "method" }, args }`. `Type::method(args)` is `Call { callee: Path { segments: ["Type", "method"] }, args }`. Both shapes are recognized by sema with a special case in `check_call`.

## 3. Receiver semantics

| Form | Phase 2C behavior | Phase 5/6 borrow-checker behavior |
|---|---|---|
| `self` | Struct passed by value (LLVM aggregate param); method moves the value in. | Same; counted as a move. |
| `&self` | Struct passed as `ptr`; sema rejects field assignment (read-only). | Borrow checker enforces no aliasing-XOR-mutability conflict. |
| `&mut self` | Struct passed as `ptr`; sema allows field assignment. | Borrow checker enforces exclusive mutable borrow. |

This is the same pattern Phase 1 used for `&T`/`&mut T`: reserve the syntax now with conservative-but-correct codegen, layer real borrow checking on top later.

## 4. Sema additions

`StructDef` grows a `methods` map:

```rust
StructDef {
    name: String,
    fields: Vec<(String, Ty)>,
    methods: HashMap<String, MethodSig>,
}

MethodSig {
    receiver: Option<Receiver>,   // None = associated function
    params: Vec<Ty>,              // does NOT include receiver
    return_type: Ty,
}

enum Receiver { Value, Ref, RefMut }
```

Method collection is a third pass after type-name + struct-field passes:
- For each `impl T { ... }`, look up `T`. If not a known struct, E0325.
- For each method, type-check the receiver and param types.
- Reject duplicate method names within the same `impl` (E0326).

Method lookup at call sites:

**Path `Type::method`**: look up `method` in `structs[id].methods`. Must have `receiver: None`. Otherwise E0327 "cannot call instance method via type".

**Field+Call `p.method(args)`**: detected in `check_call` when the callee is an `ExprKind::Field`. Type-check the receiver, get `Ty::Struct(id)`, look up `method`. Must have `receiver: Some(_)`. Otherwise E0327 "cannot call associated function as method". Check receiver compatibility:
- `&mut self` → receiver must be a writable place (existing `target_is_writable_place` logic)
- `&self` or `self` → any expression OK

Within method bodies, `self` is a special local registered in the method's scope:
- `self` form: `self` is a value-typed local of type `Ty::Struct(id)`, stored in a normal alloca.
- `&self` / `&mut self` form: `self` is bound to the LLVM pointer parameter directly (no extra alloca); sema treats it as a struct expression that auto-derefs for field access.

New error codes:

| Code | Meaning |
|---|---|
| E0324 | no method named X on struct |
| E0325 | `impl` block on unknown / non-struct type |
| E0326 | duplicate method name in `impl` block |
| E0327 | wrong call form (calling associated fn via instance, or method via type) |
| E0328 | calling `&mut self` method on a non-writable place (e.g., `let p = ...; p.translate(...)` where p is `let` not `let mut`) |

## 5. Codegen

Each method becomes a regular LLVM function with mangled name:

| C+ | LLVM |
|---|---|
| `Point::new` | `@Point.new` |
| `Point::magnitude` | `@Point.magnitude` |
| `Point::translate` | `@Point.translate` |

The `.` separator is valid in LLVM named identifiers and unambiguous because C+ identifiers don't allow `.`.

For `self`-by-value methods: emit `define ... @Type.name(%Type %0, ...)`. Inside, alloca + store `self` like any other param.

For `&self` / `&mut self` methods: emit `define ... @Type.name(ptr %0, ...)`. Inside, register `self` in the locals table with slot = `%0` directly (no alloca). Field access GEPs through `%0`. Standalone `self` loads the whole struct.

For instance call `p.method(args)`:
1. `gen_place(p)` → `(ptr, Ty::Struct(id))`. Slice 2B's `gen_place` already handles Ident, Field chains, and value-producing expressions (via temp alloca for `make().method()`).
2. If method takes `self`: load struct value from ptr, pass as struct argument.
3. If method takes `&self` / `&mut self`: pass `ptr` directly.
4. Gen each non-receiver arg, emit `call`.

For associated call `Type::method(args)`: gen each arg, emit `call @Type.method(...)`.

## 6. Sample programs

### 6.1 Must compile and run

[methods.cplus](#) — covers all three receiver forms in one program:

```cp
struct Point { x: i32, y: i32 }

impl Point {
    fn new(x: i32, y: i32) -> Point { Point { x: x, y: y } }
    fn magnitude(&self) -> i32 { self.x * self.x + self.y * self.y }
    fn translate(&mut self, dx: i32, dy: i32) {
        self.x = self.x + dx;
        self.y = self.y + dy;
    }
}

fn main() -> i32 {
    let mut p: Point = Point::new(3, 4);
    p.translate(1, 1);
    #println(p.magnitude());     // (4)^2 + (5)^2 = 41
    0
}
```

Expected output: `41`.

### 6.2 Must reject

| Program | Error |
|---|---|
| `impl Foo { fn f(&self) {} }` (Foo not declared) | E0325 |
| `impl Point { fn f(&self) {} fn f(&self) {} }` | E0326 duplicate method |
| `impl Point { fn new() {} } fn main() { p.new() }` | E0327 calling associated fn as method |
| `impl Point { fn m(&self) {} } fn main() { Point::m() }` | E0327 calling instance method via type (no receiver supplied) |
| `let p = Point::new(0,0); p.translate(1,1);` (immutable `p`) | E0305 / E0328 cannot call `&mut self` on non-mut place |
| `impl Point { fn m() -> i32 { self.x } }` (uses `self` with no receiver) | E0300 undefined name `self` |

## 7. Implementation order

1. AST: `ItemKind::Impl(ImplBlock)`; `ImplBlock { target: Ident, methods: Vec<Method> }`; `Method { name, receiver, params, return_type, body, span }`; `enum Receiver { Value, Ref, RefMut }`.
2. Parser: `parse_impl_block`; `parse_method` (parses optional receiver as the first param).
3. Sema:
   - Extend `StructDef` with `methods: HashMap<String, MethodSig>` and `MethodSig { receiver, params, return_type }`.
   - Third collection pass (`collect_methods`) over `impl` blocks.
   - Type-check method bodies with `self` registered as a special local (slot type varies by receiver kind).
   - Extend `check_call` to dispatch on callee shape: `Field`/`Path`/`Ident`.
   - New: `check_method_call`, `check_path_call`.
4. Codegen:
   - Mangle method names as `Type.method`.
   - Emit method definitions like regular functions; bind `self` appropriately per receiver kind.
   - In `gen_call`, branch on callee shape: instance method goes through `gen_place(receiver)`; associated function emits a direct call by mangled name.
5. Tests: ~30 new — sema for each new error code, codegen for the three receiver-kind IR shapes, e2e for the [methods.cplus](#) sample plus negative tests.

## 8. Open questions

- [ ] Multiple `impl Point { ... }` blocks for the same type (Rust allows this). Lean: defer; not blocking; can add later by merging into the same method map.
- [ ] Method overloading via different receiver kinds (e.g., `&self` and `&mut self` versions of the same name). Lean: no — §2.8 forbids overloading; pick one name.
- [ ] `Self` (capital) as an alias for the impl'd type inside method bodies. Lean: yes — already a reserved keyword. Useful for `fn new() -> Self`. Add in this slice; minor.
- [ ] Calling methods on enum types. Lean: defer; needs to decide if enums should have inherent methods at all.
