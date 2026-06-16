# The C+ Language Specification

Version 0.0.22 · normative reference.

**Project:** <https://cplus-lang.dev> · **Source:** <https://github.com/netdur/cplus>

## 0. About this document

This is the normative reference for the C+ language as of the v0.0.22
language feature freeze. It describes syntax and semantics; it is not a
tutorial (see the tutorial) nor an implementation guide (see
`docs/COMPILER.md`). For a dense "how to write C+" companion aimed at
LLMs, see `docs/SKILL.md`.

**The compiler is the ultimate authority.** This document is verified
against `cpc` and its test suite, but where the two ever disagree, the
compiler wins — run `cpc check` / `cpc build` and trust the diagnostic.
Exact diagnostic wording lives in the compiler's diagnostic system; this
document fixes the *categories* and the load-bearing error codes
(§20).

**Status: feature-frozen.** v0.0.22 is the last release to add language
surface. The language now accepts bug fixes only; new capability lives
in packages and tooling, never in the core language. This spec therefore
describes a stable surface.

### 0.1 Notation

Grammar is given in EBNF: `|` alternation, `( )` grouping, `?` optional,
`*` zero-or-more, `+` one-or-more, `'x'` a literal token. Lexical
terminals are `UPPER_SNAKE`; nonterminals are `lower_snake`. "MUST",
"MUST NOT", and "is an error (Exxxx)" are normative; "should" is
advisory. Each rule that the compiler enforces names its error code.

---

## 1. Lexical structure

### 1.1 Source

A source file is UTF-8. The file extension is `.cplus`. Outside string
and character literals, only ASCII is significant; a non-ASCII or
otherwise unexpected byte is a lexical error (**E0001**).

Whitespace (space, tab, CR, LF) separates tokens and is otherwise
insignificant, with one exception used only inside builder blocks: the
lexer records, per token, whether a newline precedes it (the
"line-start" bit), which the builder-block grammar consults (§17). No
other grammar is whitespace-sensitive; there are no semicolon-insertion
rules.

### 1.2 Comments

```
// line comment — to end of line
/* block comment — nests */
```

Block comments nest. An unterminated block comment is an error
(**E0002**). Comments are trivia (discarded), except that `cpc fmt`
preserves them.

### 1.3 Tokens

**Keywords** (reserved):

```
fn let mut const static if else while for in return
true false as unsafe extern struct enum union match
trait impl pub use mod import self Self defer try
break continue loop move restrict opaque guard assert
async await gen yield borrow interface type
```

**Identifiers** start with an ASCII letter or `_`, continue with ASCII
alphanumerics or `_`. `_` alone is the wildcard token.

**Integer literals**: decimal, `0x` hex, `0b` binary, with optional `_`
digit separators, and an optional type suffix (`i8 i16 i32 i64 u8 u16
u32 u64 isize usize`). A malformed number or bad suffix is an error
(**E0003** / **E0004**).

**Float literals**: decimal with `.` and/or exponent, optional suffix
`f16 f32 f64`. `f32`-suffixed literals are parsed at `f32` precision then
widened, so they round-trip exactly.

**Character literals**: `'a'`, escapes `\n \r \t \\ \' \0 \xHH`. A
character literal lexes to an `Int` with a `u8` suffix — i.e. `'A'` *is*
`65u8`. Empty, multi-byte, or non-ASCII char literals are errors
(**E0X20**).

**String literals**: `"..."`, double-quoted. Escapes as for char
literals. Two further forms:

- **C strings**: `c"..."` — a NUL-terminated `*u8` into `.rodata`, for
  FFI. Safe to form; dereferencing needs `unsafe`.
- **Interpolated strings**: `"... ${expr} ..."` — see §11. A bare `$`
  not followed by `{` or `$` is a literal `$`; `$$` is an escaped `$`.

**Punctuation and operators**:

```
( ) { } [ ]            delimiters
, ; : :: .             separators / paths / field access
-> =>                  return-type arrow / match arrow
# @                    intrinsic-or-attribute sigil / builder-block sigil
+ - * / %              arithmetic
+% -% *%               wrapping arithmetic
= == != < <= > >=      assignment / comparison
! && || & | ^ ~        logical / bitwise
<< >>                  shifts
+= -= *= /= %=         compound assignment
&= |= ^= <<= >>=       compound assignment (bitwise)
.. ..=                 ranges (exclusive / inclusive)
...                    varargs (extern signatures only)
```

`@` and `#` are distinct sigils: `#` opens a compiler intrinsic
(`#name(...)`, §12) or an attribute (`#[...]`, §14); `@` opens a builder
block (`@ctx { ... }`, §17). There is no `->` member access — field and
method access is always `.` (§3.4). Binary `&` is bitwise-and; unary
`&` / `&mut` parse but are rejected by the type checker (**E0312**) —
C+ has no value-site references (§6.3). Unary `*` dereferences a raw
pointer (unsafe, §6.7).

---

## 2. Grammar

### 2.1 Program and items

```ebnf
program     = import* item* ;
import      = 'import' STRING 'as' IDENT ';' ;

item        = attribute* ( function | struct | enum | impl
                         | interface | type_alias | const | static
                         | module_asm ) ;

function    = 'pub'? ( 'unsafe' | 'extern' | 'async' | 'gen' )*
              'fn' IDENT generic_params? '(' params? ')' ret? ( block | ';' ) ;
ret         = '->' type ;
params      = param ( ',' param )* ( ',' '...' )? ;
param       = receiver | IDENT ':' type ;
receiver    = 'self' | 'mut' 'self' | 'move' 'self' ;

struct      = 'pub'? 'struct' IDENT generic_params? '{' field* '}' ;
field       = 'pub'? ( 'opaque' )? IDENT ':' type ','? ;

enum        = 'pub'? 'enum' IDENT generic_params? '{' variant* '}' ;
variant     = IDENT ( '(' type ( ',' type )* ')' )? ','? ;

impl        = ( 'unsafe' )? 'impl' generic_params? type_path
              ( 'for' type )? '{' method* '}' ;
interface   = 'interface' IDENT generic_params? '{' fn_sig* '}' ;
type_alias  = 'pub'? 'type' IDENT generic_params? '=' type ';' ;
const       = 'pub'? 'const' IDENT ':' type '=' expr ';' ;
static      = 'pub'? 'static' 'mut'? IDENT ':' type '=' expr ';' ;

generic_params = '[' generic_param ( ',' generic_param )* ']' ;
generic_param  = IDENT ( ':' bound ( '+' bound )* )? ;
bound          = type_path ;
```

All imports MUST precede all items; an `import` appearing later is a
parse error. Every import MUST have an `as` alias (there is no
unaliased form).

### 2.2 Types

```ebnf
type     = primitive
         | type_path generic_args?        // named: struct/enum/alias/param
         | '*' type                        // raw pointer
         | type '[]'                        // slice
         | '[' type ';' (INT | IDENT) ']'   // array (length: literal or const)
         | '(' type ( ',' type )+ ')'       // tuple (arity >= 2)
         | '(' ')'                          // unit
         | 'fn' '(' (type (',' type)*)? ')' ret?   // function pointer
         | 'borrow' IDENT type ;            // region-annotated borrow
generic_args = '[' type ( ',' type )* ']' ;
type_path    = IDENT ( '::' IDENT )* ;
primitive    = 'i8'|'i16'|'i32'|'i64'|'u8'|'u16'|'u32'|'u64'
             | 'isize'|'usize'|'f16'|'f32'|'f64'|'bool'|'str' ;
```

See §4 for the type system.

### 2.3 Statements and blocks

```ebnf
block    = '{' stmt* expr? '}' ;        // optional trailing expr = block value
stmt     = let_stmt | return_stmt | while_stmt | for_stmt | loop_stmt
         | break_stmt | continue_stmt | defer_stmt | assert_stmt
         | if_let_stmt | while_let_stmt | guard_let_stmt
         | expr ';' | block_like_expr ;

let_stmt = 'let' 'mut'? pattern ( ':' type )? ( '=' expr )? ';' ;
```

A block is an expression: with a trailing expression (no `;`) it
evaluates to that expression's value; otherwise to unit. A `let` without
an initializer requires a type annotation and is subject to
definite-assignment analysis (every read MUST be preceded by an
assignment).

### 2.4 Expressions

```ebnf
expr        = assign ;
assign      = range ( assign_op assign )? ;
range       = or ( ( '..' | '..=' ) or? )? ;
or          = and ( '||' and )* ;
and         = cmp ( '&&' cmp )* ;
cmp         = bitor ( cmp_op bitor )? ;      // NON-chainable (E from parser)
bitor       = bitxor ( '|' bitxor )* ;
bitxor      = bitand ( '^' bitand )* ;
bitand      = shift ( '&' shift )* ;
shift       = add ( ( '<<' | '>>' ) add )* ;
add         = mul ( ( '+' | '-' | '+%' | '-%' ) mul )* ;
mul         = cast ( ( '*' | '/' | '%' | '*%' ) cast )* ;
cast        = unary ( 'as' type )* ;
unary       = ( '-' | '!' | '~' )* postfix ;
postfix     = primary ( call_args | index | field | turbofish )* ;
primary     = literal | path | IDENT | '(' expr ')' | tuple | array
            | block | 'unsafe' block | if_expr | match_expr
            | struct_lit | intrinsic | builder_block | 'await' expr
            | 'yield' expr ;
```

**Operator precedence**, tightest to loosest:

| Level | Operators |
|---|---|
| postfix | `f(x)` call · `a[i]` index · `a.b` field/method · `::[T]` turbofish |
| unary | `-` `!` `~` |
| cast | `as` |
| multiplicative | `*` `/` `%` `*%` |
| additive | `+` `-` `+%` `-%` |
| shift | `<<` `>>` |
| bitwise and | `&` |
| bitwise xor | `^` |
| bitwise or | `\|` |
| comparison | `==` `!=` `<` `<=` `>` `>=` (non-chainable) |
| logical and | `&&` |
| logical or | `\|\|` |
| range | `..` `..=` |
| assignment | `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` |

Note bitwise operators bind **tighter** than comparison (so `x == y & m`
is `x == (y & m)`). Comparisons are non-chainable: `a < b < c` is a parse
error — use `a < b && b < c`.

### 2.5 Patterns

```ebnf
pattern  = '_' | IDENT | literal
         | type_path ( '(' pattern ( ',' pattern )* ')' )?   // enum variant
         | '(' pattern ( ',' pattern )+ ')' ;                 // tuple
```

Patterns appear in `match` arms, `let`, and the pattern-let forms (§8).

---

## 3. Names, modules, visibility

### 3.1 Files and imports

Each `.cplus` file is a module. A file imports another with
`import "PATH" as ALIAS;`:

```cplus
import "./math" as math;       // local path — starts with ./ or ../
import "stdlib/io" as io;      // vendored — first segment is the dependency name
```

A local path resolves relative to the importing file's directory; the
`.cplus` extension is omitted. A vendored path's first segment names a
dependency declared in `Cplus.toml` and resolved under `vendor/`. The
build driver (`cpc build`, reading `Cplus.toml`) performs resolution;
`cpc check FILE` does not read the manifest and rejects imported modules
(**E0852**) — it is for single-file, import-free snippets.

### 3.2 Paths

A qualified name uses `::`: `math::area`, `Color::Red`,
`mod::Type::method`, `Option[i32]::Some`. Path resolution distinguishes
enum-variant paths from associated-function paths after the prefix is
resolved. Paths of unsupported length or unknown prefix are errors
(**E0312** and the **E04xx** family).

### 3.3 Visibility

Items are private to their file unless marked `pub`. Cross-file access to
a non-`pub` item is an error (**E0403**); referencing a name that does
not exist in the target module is a distinct error (**E0404**/**E0405**).
Struct fields carry their own `pub` (field privacy is enforced
per-field). A `pub type` alias may re-export a type as a small facade.

### 3.4 `::` versus `.`

`::` joins *namespace* segments (modules, types, associated functions,
enum variants, turbofish). `.` performs *value* operations (field access,
method call, tuple index `t.0`). The two are never interchangeable. There
is no `->`.

---

## 4. Types

### 4.1 Primitives

Signed integers `i8 i16 i32 i64`, unsigned `u8 u16 u32 u64`,
pointer-sized `isize usize`, floats `f16 f32 f64`, `bool`, and the unit
type `()`. There is no dedicated `char` type — character literals are
`u8`.

`usize`/`isize` and raw pointers are pointer-width: 64-bit on 64-bit
targets, 32-bit on 32-bit targets (e.g. `esp32-xtensa`, §19). Fat
pointers (`str`, slices) carry a pointer and a `usize` length.

### 4.2 Aggregates

- **struct** — nominal product type; named fields, each possibly `pub`
  and/or `opaque` (§6.6).
- **enum** — tagged union; each variant optionally carries a tuple
  payload. Pattern matching is exhaustiveness-checked (§8).
- **tuple** `(A, B, ...)` — arity ≥ 2; lowered to a synthesized struct
  with fields `_0`, `_1`, …; element access via `t.0`. `(x)` is grouping;
  `()` is unit.
- **array** `[T; N]` — fixed length `N` (an integer literal or a
  non-negative integer `const`); the fill form `[expr; N]` initializes
  every slot. Indexing is bounds-checked at runtime.
- **slice** `T[]` — a `{ptr, len}` view over a contiguous run of `T`;
  Copy (it is a view, not an owner). Constructed via
  `#slice_from_raw_parts` (unsafe); `#slice_ptr` / `#slice_len` read its
  parts (safe). Indexing is bounds-checked.

### 4.3 Strings

- **`str`** — a borrowed fat-pointer view `{ptr, len}` over UTF-8 bytes
  (string literals, `#include_str`, `#env`). Copy.
- **`Text`** — the owned, growable string type. `Text` is a library type
  with one compiler lang-item hook (interpolation builds a `Text`); it
  must be imported like any other type. (The earlier owned `string` type
  was removed in favor of `Text`.)

### 4.4 Pointers and function pointers

- **raw pointer** `*T` — an unmanaged address. Forming a `*T` is safe;
  dereferencing requires `unsafe` (§6.7). The safe null substitute in FFI
  is `0 as *T` inside `unsafe`; there is no null keyword (§6.5).
- **function pointer** `fn(A, B) -> R` — the address of a top-level
  function, taken by naming it in a function-pointer-typed context. No
  closures, no environment capture. Integers and function pointers do not
  interconvert (**E0315**).

### 4.5 Generics and aliases

Generic type parameters are written `[T]` / `[T: Bound + Bound]` on
functions, structs, enums, impls, and interfaces (§9, §10). A `type`
alias is transparent: the alias and its target resolve to the same type.

---

## 5. Type system

C+ is statically typed with local inference. There are no implicit
numeric or pointer conversions: every width or representation change is
an explicit `as` cast. A type mismatch is **E0302** (with related codes
across the **E03xx** range for specific shapes).

**Literal typing.** An unsuffixed integer literal takes its type from
context, defaulting to `i32` when unconstrained; an unsuffixed float
defaults to `f64`. A suffix fixes the type exactly. Mixed-width
arithmetic without a cast is an error.

**`as` casts** convert between numeric types, between integers and raw
pointers, and between raw pointer types. Casts that the language forbids
(e.g. integer ↔ function pointer) are errors (**E0315** and neighbors).

---

## 6. Ownership and memory

This is the part of C+ that differs most from C. There is no garbage
collector; memory safety is established statically. The parameter modes,
borrows, and the absence of value-site references are described in full in
[MEMORY-MODEL.md](MEMORY-MODEL.md).

### 6.1 Move and copy

Every value has an owner. Assigning, passing, or returning a value of a
**non-Copy** type *moves* it: the source is afterward invalid, and using
a moved-from value is an error (the **E05xx** move/borrow family).
Values of a **Copy** type are duplicated instead, leaving the source
valid.

`Copy` is a marker interface (§9.4), inferred structurally: a type is
Copy when all its components are Copy and it has no `Drop`. Primitives,
`str`, slices, raw pointers, and aggregates thereof are Copy; a type with
an owning field (e.g. `Text`, `Vec`) or a `Drop` impl is not.

### 6.2 Borrow checking

Ownership and moves are checked **flow-sensitively**: a binding may be
moved on one branch and not another, and the checker merges branch states
at join points (`if`, `match`). Reads of a possibly-moved value are
rejected. Conflicting access to overlapping places is the **E0370**
family.

### 6.3 Receivers

Methods take an explicit receiver — there is no implicit `&self`:

- **`self`** — reads the receiver (by value/borrow as the type allows).
- **`mut self`** — mutates the receiver in place.
- **`move self`** — consumes the receiver (used by finalizers like
  `finish`).

C+ has no value-site references: unary `&` / `&mut` parse but are
rejected by the type checker (**E0312**, "references are not yet
supported"). The receiver forms and ordinary by-value passing cover
their uses. (A region-annotated `borrow A T` *type* exists for advanced
APIs but is not a value-site `&`.)

### 6.4 Drop

A type may implement `Drop` (a `drop(mut self)` method). Drop glue runs
at scope exit in reverse declaration order, interleaved with `defer`
statements (lexically, LIFO). A type with a `Drop` impl is never Copy.

### 6.5 No null in safe code

There is no `null` keyword and no null value in safe code. Optionality is
expressed with `Option[T]`. In FFI, a null pointer is written `0 as *T`
inside an `unsafe` block — visibly unsafe, never implicit.

### 6.6 Raw-pointer accountability

A struct field of raw-pointer type (directly or transitively) MUST be
accounted for: either released in the type's `Drop`, or explicitly marked
`opaque` to declare "this pointer is not owned here." An unaccounted raw
pointer field is an error (**E0510**). This keeps ownership of FFI
handles explicit.

### 6.7 `unsafe`

Operations the type system cannot verify are confined to `unsafe { ... }`
blocks (or `unsafe fn` bodies): dereferencing a raw pointer, calling an
`extern` function, the raw-parts constructors, reading/writing
`static mut`. Performing them outside `unsafe` is **E0801**. `unsafe`
does not disable the borrow checker; it grants exactly the enumerated
extra operations.

---

## 7. Expressions and control flow

`if`, `match`, `loop`, and blocks are expressions and may produce values;
`while` and `for` are statements producing unit.

```cplus
let x = if cond { 1 } else { 2 };       // both arms same type
loop { if done() { break; } }           // unconditional; exits via break/return
while c { ... }
for i in 0..n { ... }                    // range iteration
for x in collection { ... }              // iterator iteration
```

`break` / `continue` are valid only inside a loop (**E0353** otherwise);
C+ has no labelled break. `return EXPR;` is **required** to return a
value — a trailing expression is the value of a *block*, but a function
body returns via `return` (an unterminated value path is **E0333**).
`assert EXPR;` traps on false. Ranges `a..b` (exclusive) and `a..=b`
(inclusive) are values usable as iterators.

---

## 8. Pattern matching and pattern-let

`match` is exhaustiveness-checked over the scrutinee's variants; a
non-exhaustive match or unreachable arm is an error (**E03xx** match
family). Arms may be `PAT => expr,` or `PAT => { ... }`.

Three sugar forms bind patterns outside `match`; all are lowered to
`match` before semantic analysis (§18):

- **`if let PAT = E { ... } else { ... }`** — refutable pattern required
  (**E0347**).
- **`while let PAT = E { ... }`** — loops while the pattern matches.
- **`guard let PAT = E else { ... };`** — the else block MUST diverge
  (**E0348**); on success the bindings live in the enclosing scope. The
  `else |COMPLEMENT|` form must cover the scrutinee exhaustively
  (**E0349**/**E0350**).

---

## 9. Functions, methods, impls, interfaces

### 9.1 Functions and methods

Functions are declared with `fn`; methods live in `impl` blocks and take
a receiver (§6.3). Modifiers `pub`, `unsafe`, `extern`, `async`, `gen`
precede `fn`. The return type follows `->`; its absence means unit.

### 9.2 Interfaces (bounded polymorphism)

```cplus
interface Eq { fn eq(self, other: Self) -> bool; }
```

An `interface` lists method signatures that implementing types must
provide. `Self` denotes the implementing type. Generic bounds (§10) name
interfaces; a call requiring a bound the type does not satisfy is an
error.

### 9.3 Impls

`impl Type { ... }` adds methods; `impl Type for Interface { ... }`
provides an interface. Both may be generic.

### 9.4 Marker interfaces

The compiler blesses a small set of zero-method or single-method
interfaces with structural inference:

| Interface | Meaning |
|---|---|
| `Copy` | duplicate-on-use (§6.1) |
| `Clone` | explicit `clone(self) -> Self` |
| `Eq` / `Ord` / `Hash` | `eq` / `cmp` / `hash` |
| `ToText` | `to_text(self) -> Text` (blessed for primitives + `str`) |
| `Send` | safe to transfer across threads |
| `Sync` | safe to share across threads |

`Send`/`Sync` gate cross-thread transfer and sharing (§16). A type that
hides a raw pointer is `!Send`/`!Sync`; to vouch for one, write
`unsafe impl T for Send {}` (the body MUST be empty, and `unsafe impl`
applies only to `Send`/`Sync` — **E0860**/**E0861**). Conditional forms
are allowed: `unsafe impl Arc[T: Send + Sync] for Send {}`.

---

## 10. Generics and monomorphization

Generic parameters use square brackets: `fn id[T](x: T) -> T`,
`struct Pair[A, B] { ... }`, `Option[i32]`. At a call or construction
site, type arguments are inferred from values, or given explicitly with
the **turbofish** `::[T]`:

```cplus
let v = vec::new::[i32]();
let p = Pair[i32, bool] { first: 7, second: true };
Option[i32]::Some(7)
```

Generics are **monomorphized**: the compiler emits one specialized copy
per concrete type combination. Internal mangling (e.g. `Option__i32`) is
an implementation detail; **source never spells a mangled name** — users
always write `Option[i32]::Some(v)`, including in patterns.

---

## 11. Strings and interpolation

String literals have type `str`. Interpolation produces an owned `Text`:

```cplus
let g = "hello ${name}, n is ${n}";    // Text
```

Each `${expr}` part's type MUST satisfy `ToText`. `$$` is a literal `$`;
a bare `$` not part of `${...}` or `$$` is a literal `$`. Interpolation
lowers to a single allocation that concatenates the parts (§18).
`c"..."` produces a NUL-terminated `*u8` for FFI.

---

## 12. Compile-time intrinsics and builtins

Compiler intrinsics are spelled `#name(...)`, optionally with a turbofish
and/or a return-type ascription. An unknown intrinsic is **E0905**.

| Intrinsic | Result |
|---|---|
| `#size_of::[T]()` / `#align_of::[T]()` | layout of `T` (`usize`) |
| `#addr_of(place)` | address of a place (unsafe to use as `*T`) |
| `#str_ptr(s)` / `#str_from_raw_parts(p, n)` | `str` ↔ raw parts |
| `#slice_ptr(s)` / `#slice_len(s)` / `#slice_from_raw_parts(p, n)` | slice parts |
| `#msg_send(recv, "sel") -> T` | Objective-C message send (interop) |
| `#asm("tmpl", ...)` | inline assembly (Tier 1 bare, Tier 2 operands) |
| `#println(...)` / `#print(...)` | formatted output builtins |

Three compile-time *file* builtins read at build time, resolving paths
relative to the containing source file:

- `#include_bytes("path")` → `*[u8; N]`
- `#include_str("path")` → `str` (UTF-8 validated; **E0875** on invalid)
- `#env("NAME")` → `str` (**E0876** if the variable is unset at build
  time)

These are ordinary `#name(...)` intrinsics (§12), not macros — C+ has no
macro system. The legacy `include_bytes!(...)` macro spelling is a parse
error.

Module-scope `#asm("...");` emits raw assembly at module level.

---

## 13. FFI and the C ABI

C+ has a **one-way** C ABI: `cpc` emits standard object files / static
libraries that C and other languages can link; it does not compile `.c`.

```cplus
extern fn printf(fmt: *u8, ...) -> i32;   // declaration; calls need unsafe
```

`extern fn` declares a C-ABI function; calling one is **E0801** outside
`unsafe`. `...` declares varargs (extern signatures only). `#[repr(C)]`
(§14) gives a struct C-compatible layout. A `[lib]` target's entry-file
`pub` items keep their bare symbol names so C consumers link `add` as
`_add`; imported files stay mangled.

ABI lowering (argument coercion, struct-by-value rules, `sret`) follows
the target's C ABI and is pinned per target (§19).

---

## 14. Attributes

Attributes are `#[name]` or `#[name(args)]`, attached to items (or, for
loop hints, to loop statements). They are **pure metadata** read by
compiler passes or tools; they never themselves transform the AST.

| Attribute | Effect |
|---|---|
| `#[test]` | marks a test fn for `cpc test` |
| `#[inline]` | inlining hint |
| `#[repr(C)]` | C-compatible struct layout |
| `#[no_alloc]` | forbid heap allocation in the fn (compile-checked) |
| `#[no_block]` | forbid blocking calls |
| `#[bounded_recursion]` | require statically-bounded recursion |
| `#[max_stack(N)]` | bound stack usage |
| `#[realtime]` | compose the real-time contract set (§15) |
| `#[naked]` | naked function (no prologue/epilogue) |
| `#[unroll(N)]` / `#[vectorize_width(N)]` | loop-statement hints |
| `#[deprecated("...")]` / `#[link(...)]` | metadata |

Invalid attribute placement or arguments are the **E09xx** attribute
family.

---

## 15. Real-time contracts

The real-time attributes turn timing/allocation guarantees into
compile-time checks. `#[no_alloc]` rejects any heap allocation reachable
from the function; `#[no_block]` rejects blocking operations;
`#[bounded_recursion]` and `#[max_stack(N)]` bound recursion and stack;
`#[realtime]` composes them. A `[profile.realtime]` section in
`Cplus.toml` synthesizes the contract onto every function in the package
(dependencies exempt), enforced by the same passes with no special
casing. Violations are reported in the **E09xx** range.

---

## 16. Concurrency

The idiomatic model is **partition + join** — no shared memory, no data
race:

```cplus
import "stdlib/thread" as thread;
let h = thread::spawn_with::[Range, i64](left, sum_r);
let total = h.join() +% other;
```

`spawn`/`spawn_with` require their payload types to be `Send` (§9.4);
passing a `!Send` type across the bound is **E0502**. Shared-state
primitives (`mutex`, `atomic`, `arc`) exist but are secondary.

`async fn` / `await` provide coroutines on 64-bit targets; the executor
is a library (`stdlib/executor`). Borrow-shaped parameters (`str`, `T[]`,
`mut x: NonCopy`) are rejected in `async fn` (**E0900**) — use owned
types. On 32-bit targets, async is unavailable (**E0867**) and
pthread-backed stdlib modules are gated (**E0866**); see §19.

---

## 17. Contextual builder blocks

A builder block is an expression that gives a package concise,
declarative construction syntax without macros, closures, or compiler
plugins. The compiler owns the syntax and the lowering (§18); a package
provides ordinary types and functions.

### 17.1 Grammar

```ebnf
builder_block = '@' type_path '{' builder_entry* '}' ;
builder_entry = item_entry | let_stmt | if_entry | for_entry ;

item_entry    = ( call_expr | container ) modifier* ;
container     = IDENT '{' builder_entry* '}' ;          // bare, same-context child
modifier      = '.' IDENT '=' expr                       // field-assign modifier
              | '.' IDENT '(' args? ')' ;                // method-call modifier
if_entry      = 'if' expr '{' builder_entry* '}'
                ( 'else' ( if_entry | '{' builder_entry* '}' ) )? ;
for_entry     = 'for' IDENT 'in' expr '{' builder_entry* '}' ;
```

A modifier line is recognized by a **line-leading** `.` (the line-start
bit, §1.1): a `.` that begins a line modifies the item above it, while a
same-line `.m()` is ordinary postfix on the item expression. A modifier
before any item is an error; the modifier name is a field/method of the
item, never a contextual lookup.

**Containers** are bare `name { ... }` — a child element of the *same*
context (not a nested DSL). A nested *different* `@`-DSL block inside a
builder block is rejected; write a same-context container without `@`.

Block contents are limited to item lines, modifier lines, `let`,
`if`/`else`/`else if`, `for … in …`, and nested containers. `while`,
`loop`, `return`, `break`, `continue`, `defer`, `guard`, `yield`,
`await`, and nested `@` are rejected at parse time.

### 17.2 Contextual name lookup

Inside `@ctx { ... }`, an unqualified name resolves in order: **locals →
same-file top-level → `ctx::name`**. So a bare `text(...)` becomes
`ctx::text` unless a local or same-file item shadows it; a bare name that
is no member of `ctx` falls through to the ordinary "undefined" error.
Children of a container inherit the enclosing context.

### 17.3 The builder protocol

A context package provides, with these fixed names:

```cplus
pub struct Item { ... }                      // one element type per context
pub fn text(...) -> Item { ... }             // leaf element constructor(s)

pub struct Builder { ... }                   // the accumulator
impl Builder {
    pub fn new() -> Builder { ... }
    pub fn add(mut self, item: Item) { ... }
    pub fn finish(move self) -> Root { ... } // root finisher (Root may differ from Item)
}

pub fn vstack(b: Builder) -> Item { ... }    // container element: takes a filled Builder
```

There is one `Item` type per context (C+ has no overloading). A container
constructor takes a `Builder` (not a collection): the package stores
children however it likes, so the compiler's lowering never names a
collection type. Missing `Builder`/`new`/`add`/`finish`, or an item type
`add` does not accept, are reported by ordinary resolution/type errors at
the user-written DSL line.

### 17.4 Lowering

Every form reduces to `Builder::new` + `add`, differing only in the
finisher (root `.finish()`, container `ctx::name(builder)`); `if`/`for`
add into the same builder. See §18.6 for the exact desugar.

---

## 18. Desugarings (normative)

Sugar forms are rewritten to the listed core forms before semantic
analysis. The rewrites preserve source spans so diagnostics point at the
written code.

1. **`if let` / `while let` / `guard let`** → `match` (§8).
2. **Tuple literal `(a, b)`** → a synthesized struct literal with fields
   `_0`, `_1`, …; `t.0` → `t._0`.
3. **String interpolation** → a single allocating concatenation building
   a `Text`.
4. **`const` references** → the const's literal initializer is
   substituted at each use site before codegen; no `const` global is
   emitted (initializers MUST be literals — **E0X30**).
5. **`for x in 0..n`** and iterator `for` → the corresponding loop with
   the iterator protocol.
6. **Builder blocks** (§17): given `@view { text("t") .font = big   if c
   { badge() }   vstack { for r in rows { item(r) } } }`:

```cplus
let mut __b = view::Builder::new();
let mut __i = view::text("t");
__i.font = big;
__b.add(__i);
if c { __b.add(view::badge()); }
let __c = { let mut __cb = view::Builder::new();
            for r in rows { __cb.add(view::item(r)); }
            view::vstack(__cb) };      // container finisher
__b.add(__c);
let result = __b.finish();             // root finisher
```

Temporary names derive from source byte offsets, unique within a function
body. After lowering, semantic analysis and codegen see only ordinary
locals, calls, field assignments, blocks, and `if`/`for`.

---

## 19. Targets and cross-compilation

`cpc --target NAME` selects a target. The host target preserves legacy
behavior byte-for-byte. Named targets include `host`, `ios-arm64`,
`ios-arm64-simulator`, `android-arm64`, `esp32-xtensa`,
`esp32c3-riscv32`. `--min-os VERSION` overrides the OS floor in versioned
triples; unversioned targets reject it.

Targets differ in pointer width (32-bit for the ESP32 targets, §4.1),
C ABI (pinned per target), object format, and **handoff**: host-linked
targets produce an executable; external-builder targets (iOS, Android,
ESP-IDF) stop at an object/static library and let the platform toolchain
(Xcode, Gradle/NDK, ESP-IDF) perform the final link. For those targets,
`[[bin]]`/cdylib/test/single-file outputs are rejected; output lives
under `target/<target-name>/<mode>/`.

The **embedded package profile** (32-bit targets) gates stdlib modules
that require an OS thread/reactor (**E0866**) and rejects `async fn`
(**E0867**).

---

## 20. Diagnostics

Every diagnostic carries a stable code, a severity, a primary source
span, and optional labels/notes/suggestions. Codes group by phase:

| Range | Area |
|---|---|
| `E0001`–`E0005` | lexical (unexpected char, unterminated comment/string, bad number) |
| `E00XX`, `E01XX` | parser (generic), builder-block parse |
| `E0300`–`E033x` | types, inference, casts, fields, calls (`E0302` type mismatch, `E0315` illegal cast, `E0320` no such field, `E0333` missing return) |
| `E0340`–`E0360` | `match`, pattern-let, `break`/`continue` context (`E0347`–`E0352` pattern-let, `E0353` break/continue outside loop) |
| `E0370`–`E0385` | borrow conflicts, moves |
| `E0401`–`E0412` | modules, paths, visibility (`E0403` private, `E0404`/`E0405` unknown item) |
| `E0500`–`E0513` | ownership/borrow checker, `Send` (`E0502`), raw-pointer accountability (`E0510`) |
| `E0801` | operation requires `unsafe` |
| `E0852`–`E0867` | imports/packages (`E0852` check-without-manifest), `unsafe impl` (`E0860`/`E0861`), embedded profile (`E0866`/`E0867`) |
| `E0870`–`E0876` | compile-time builtins (`E0875` invalid UTF-8, `E0876` unset env) |
| `E0890`–`E0909` | attributes, real-time contracts, intrinsics (`E0905` unknown intrinsic), async constraints (`E0900`) |
| `E0X20`–`E0X36` | char literals (`E0X20`), `const`/`static` initializers (`E0X30`/`E0X31`), `static mut` access (`E0X33`/`E0X34`), const array length (`E0X36`) |

The compiler's diagnostic system is authoritative for exact messages and
for any code not listed; this table fixes the categories.

---

## 21. Conformance

A program is well-formed if `cpc check` (single file) or `cpc build`
(project) reports no errors. The compiler is the normative
implementation; the in-repo test suite (`cpc/tests/e2e.rs` and
per-module unit tests) is the executable conformance record. Where this
document and the compiler disagree, the compiler is correct and this
document is in error — please report it.

### 21.1 Tooling surface (informative)

- `cpc build` / `cpc check` / `cpc run` — compile / type-check / run.
- `cpc test` — run `#[test]` functions.
- `cpc fmt` — canonical formatter; if source does not round-trip, it is
  syntactically off.
- `cpc query` / `cpc mcp` / `cpc lsp` — the resolved, typed
  code-knowledge graph for editors and agents.
- `cpc doc` — extract `pub` items and their `///` docs.
```
