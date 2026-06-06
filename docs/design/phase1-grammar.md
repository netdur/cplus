# Phase 1 Grammar — Design Note

> Status: draft, locks in tokens + grammar + samples for the tracer-bullet subset
> Scope: just enough to compile factorial, fibonacci, and simple integer math end-to-end
> Out of scope: strings, structs, arrays, slices, pointers, generics, modules, errors, defer, ownership

## 1. Purpose

Define the smallest C+ subset that exercises lexer → parser → name resolution → type checker → LLVM IR → linked binary. Once this works, every later feature is additive.

Phase 1 is **`i32`-only**. Literally one type plus `bool` for conditions. No strings, no structs, no arrays. The "output" of a Phase 1 program is either its exit code or values printed via the compiler builtin `println`.

## 2. Tokens

### 2.1 Whitespace and comments

- Whitespace: space, tab, CR, LF. Significant only as a token separator.
- Line comment: `// ...` to end of line.
- Block comment: `/* ... */`. **Nestable** (unlike C). Better diagnostics, easier to comment out code that already contains comments.
- Doc comment: `/// ...` (line). Reserved syntactically; ignored by the compiler in Phase 1.

### 2.2 Identifiers

- `[A-Za-z_][A-Za-z0-9_]*`
- Single underscore `_` is the wildcard, not an identifier.
- Case-sensitive.
- No Unicode in identifiers in Phase 1 (revisit later).

### 2.3 Keywords (reserved)

In Phase 1 the lexer reserves all of these, even though most have no semantics yet. Reserving early avoids painful renames.

```
fn   let   mut   const   if   else   while   for   in   return
true  false   as   unsafe   extern
```

Reserved-but-unused (parsed as keywords, error if used in Phase 1 except where noted):
```
struct  enum  union  match  trait  impl  pub  use  mod  self  Self
defer  try  break  continue  loop
```

### 2.4 Numeric literals

```
int_lit    = dec_lit | hex_lit | bin_lit | oct_lit
dec_lit    = digit (digit | '_')*
hex_lit    = '0x' hexdigit (hexdigit | '_')*
bin_lit    = '0b' [01] ([01] | '_')*
oct_lit    = '0o' [0-7] ([0-7] | '_')*

float_lit  = dec_lit '.' dec_lit (('e' | 'E') ('+'|'-')? dec_lit)?
           | dec_lit ('e' | 'E') ('+'|'-')? dec_lit
```

- Underscore digit separators: `1_000_000`, `0xDEAD_BEEF`. May not lead, may not be adjacent to `.` or `e`.
- Type suffix: `42i32`, `100u64`, `3.14f64`. Optional. Phase 1 only sees `i32`, but the lexer recognizes the full suffix set (`i8 i16 i32 i64 u8 u16 u32 u64 isize usize f32 f64`) and the type checker rejects non-`i32` ones in Phase 1.
- A literal without a suffix has type inferred from context; defaults to `i32` if unconstrained.

### 2.5 Bool literals

`true`, `false` — Phase 1 needs them for `if`/`while` conditions.

### 2.6 Operators and punctuation

Single-char: `+ - * / %  &  |  ^  ~  !  =  <  >  ( ) { } [ ] , ; : .`
Multi-char: `== != <= >=  &&  ||  << >>  +=  -=  *=  /=  %=  &=  |=  ^=  <<=  >>=  ->  =>  +%  -%  *%  ..  ..=  ::  &mut`

Note: `&mut` is a two-token sequence (`&` then `mut`) but commonly thought of as one operator; the parser handles it as two tokens with a recognized adjacency.

## 3. Operator precedence

High to low, mirrors Rust. All binary operators left-associative except where noted.

| Level | Operators                                | Associativity |
|-------|------------------------------------------|---------------|
| 1     | postfix `f()`  `s.field`  `a[i]`         | left          |
| 2     | unary prefix `-x`  `!x`  `&x`  `&mut x`  `*x` | right    |
| 3     | `as`                                     | left          |
| 4     | `* / %  *% /%`                           | left          |
| 5     | `+ -  +% -%`                             | left          |
| 6     | `<< >>`                                  | left          |
| 7     | `&`                                      | left          |
| 8     | `^`                                      | left          |
| 9     | `\|`                                     | left          |
| 10    | `== != < <= > >=`                        | none (non-chainable) |
| 11    | `&&`                                     | left          |
| 12    | `\|\|`                                   | left          |
| 13    | `..`  `..=`                              | none          |
| 14    | `=  += -= *= /= %= &= ^= \|= <<= >>=`    | right         |

Comparisons are non-chainable: `a < b < c` is a parse error, must be `a < b && b < c`.

Phase 1 uses levels 1–14 except 7–9 (bitwise) and `<< >>` and `&` / `&mut` references. The grammar still parses them so they error in sema, not at parse time.

## 4. Grammar

EBNF-like. Phase 1 subset.

```
program        = item* ;

item           = function ;

function       = 'fn' ident '(' param_list? ')' ('->' type)? block ;
param_list     = param (',' param)* ','? ;
param          = ident ':' type ;

type           = 'i32' | 'bool' ;        // Phase 1: just these two

block          = '{' stmt* expr? '}' ;

stmt           = let_stmt
               | return_stmt
               | while_stmt
               | for_stmt
               | expr_stmt ;

let_stmt       = 'let' 'mut'? ident (':' type)? '=' expr ';' ;
return_stmt    = 'return' expr? ';' ;
while_stmt     = 'while' expr block ;

for_stmt       = 'for' '(' for_c_init ';' expr ';' for_c_update ')' block
               | 'for' ident 'in' expr block ;
for_c_init     = let_stmt_no_semi | expr | /* empty */ ;
for_c_update   = expr (',' expr)* | /* empty */ ;

expr_stmt      = expr ';'
               | block_expr ;        // block-expr at statement position needs no ;

expr           = assign_expr ;
assign_expr    = range_expr (assign_op assign_expr)? ;       // right-assoc
range_expr     = or_expr ( ('..' | '..=') or_expr )? ;
or_expr        = and_expr ('||' and_expr)* ;
and_expr       = cmp_expr ('&&' cmp_expr)* ;
cmp_expr       = bit_or_expr (cmp_op bit_or_expr)? ;         // non-chain
bit_or_expr    = bit_xor_expr ('|' bit_xor_expr)* ;
bit_xor_expr   = bit_and_expr ('^' bit_and_expr)* ;
bit_and_expr   = shift_expr ('&' shift_expr)* ;
shift_expr     = add_expr (('<<' | '>>') add_expr)* ;
add_expr       = mul_expr (add_op mul_expr)* ;
mul_expr       = cast_expr (mul_op cast_expr)* ;
cast_expr      = unary_expr ('as' type)* ;
unary_expr     = unary_op unary_expr | postfix_expr ;
postfix_expr   = primary (call_suffix | field_suffix | index_suffix)* ;

primary        = int_lit | float_lit | bool_lit | ident
               | '(' expr ')'
               | block_expr
               | if_expr ;

block_expr     = block ;
if_expr        = 'if' expr block ('else' (if_expr | block))? ;
```

### 4.1 Block expressions

A `block` is `{ stmt* expr? }`. The optional trailing expression — *no* semicolon — is the value of the block. With a trailing `;` it's a statement and the block evaluates to **unit** (which doesn't exist as a type in Phase 1, so this is an error if the block's value is used).

Practical Phase 1 rule: a block is "value-producing" iff its last item is an expression with no `;`. Function bodies and `if`/`else` arms are the only places block-as-expression is meaningful in Phase 1.

### 4.2 If as expression

`if cond { a } else { b }` is an expression iff both arms are value-producing blocks of the same type. Otherwise it's a statement.

```
let x = if n > 0 { n } else { -n };           // expression
if n > 0 { #println(n); }                       // statement
```

### 4.3 Function `main`

```
fn main() -> i32 { ... }
```

Required signature in Phase 1. Return value becomes the process exit code. The implicit-zero variant `fn main() { ... }` is deferred to a later phase (needs unit type).

## 5. Builtins for Phase 1

Just one, hard-wired in codegen:

```
fn #println(n: i32);     // prints n followed by '\n', via libc printf
```

Resolved as a name in the global scope. The compiler emits a `printf("%d\n", n)` call. No user-visible string type is needed yet.

This is the placeholder for the eventual proper `println` formatting intrinsic. Same name, much smaller signature.

## 6. Semantics worth pinning down now

- **Integer overflow** of `+ - *` on `i32`: trap in debug, wrap in release (per plan §2.3). Codegen uses LLVM's `llvm.sadd.with.overflow.i32` (etc.) in debug; plain `add` in release.
- **Integer division** by zero: trap in both modes. Codegen inserts a check before `sdiv` / `srem`.
- **Unreachable code**: a function returning `i32` whose body falls off the end without a value is a compile error. Diverging paths (`return` in every branch) are accepted.
- **Definite assignment**: deferred to Phase 3. In Phase 1, `let x: i32;` (no initializer) is rejected — every `let` requires an initializer for now.

## 7. Sample programs

### 7.1 Must compile and run

**factorial.cplus**
```cplus
fn factorial(n: i32) -> i32 {
    if n <= 1 { 1 } else { n * factorial(n - 1) }
}

fn main() -> i32 {
    #println(factorial(10));   // expect: 3628800
    0
}
```

**fibonacci.cplus** (iterative)
```cplus
fn fib(n: i32) -> i32 {
    let mut a: i32 = 0;
    let mut b: i32 = 1;
    let mut i: i32 = 0;
    while i < n {
        let t = a + b;
        a = b;
        b = t;
        i = i + 1;
    }
    a
}

fn main() -> i32 {
    #println(fib(20));   // expect: 6765
    0
}
```

**sum_range.cplus**
```cplus
fn main() -> i32 {
    let mut total: i32 = 0;
    for i in 1..=100 {
        total = total + i;
    }
    #println(total);   // expect: 5050
    0
}
```

**c_for.cplus**
```cplus
fn main() -> i32 {
    let mut total: i32 = 0;
    for (let mut i: i32 = 0; i < 10; i = i + 1) {
        total = total + i;
    }
    #println(total);   // expect: 45
    0
}
```

### 7.2 Must reject

| Program | Expected error |
|--------|----------------|
| `let x = 1; x = 2;` | `x` is not mutable; need `let mut` |
| `let x: i32 = 1.5;` | type mismatch: f64 literal in i32 binding |
| `fn f() -> i32 { 1; }` | trailing `;` discards value; function expects i32 |
| `if 1 { 1 } else { 2 }` | condition must be `bool`, got `i32` |
| `a < b < c` | non-chainable comparison |
| `let x = 1u64;` | u64 not supported in Phase 1 |
| `let x;` | missing initializer (Phase 1 rule; relaxes in Phase 3) |
| `fn main() { }` | main must return i32 in Phase 1 |
| `fn f() -> i32 { return; }` | return needs a value when fn returns i32 |

## 8. Implementation order inside Phase 1

1. **Lexer** — produces a `Token` stream with span info. Test with snapshot tests on each sample program's token list.
2. **AST + parser** — recursive descent, one expression-precedence climbing pass. Test by parsing samples and snapshotting the AST.
3. **Name resolution** — resolve identifiers to function defs / local bindings. Errors: undefined names, double declarations.
4. **Type checker** — bottom-up; one type (`i32`) plus `bool` for conditions. Errors: type mismatches, non-bool condition, non-mutable assignment, comparison chain, `1;` in tail position when value expected.
5. **IR codegen** — emit textual LLVM IR. Reuse the Phase 0 driver (write `.ll`, invoke clang).
6. **Driver** — `cpc foo.cplus -o foo` end-to-end.
7. **E2E tests** — extend `compiler/tests/e2e.rs` with one test per `samples/*.cplus`.

## 9. Open issues for later phases

- Block expressions need unit type; revisit when adding it.
- `for ... in expr` where `expr` is a non-range (an iterator/collection) needs the iteration protocol, deferred until Phase 2 or wherever slices/arrays land.
- `as` cast between integer types is parsed in Phase 1 but errors in sema since only `i32` exists.
- Method call `s.foo()` is parsed as field-then-call; method dispatch comes in Phase 2.
- Operator overloading: not in Phase 1, probably never (matches Rust trait approach later).

## 10. Non-goals reminder

This grammar is *not* the language spec. It's the minimum to validate the pipeline. Expect to revise heavily as later phases land. Per plan §4, a design note exists to be thrown away when it turns out to be wrong — that's the point.
