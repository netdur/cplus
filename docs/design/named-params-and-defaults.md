# Named parameters and default values

## Motivation

C+ has no method overloading (by the "one function, one name" rule). Swift, C#,
and Python avoid overloading the same way: **default parameter values** plus
**named arguments**. Together they give readable, self-documenting call sites and
let one function cover what would otherwise be a family of overloads — which is
also why they unlock a Swift-shaped standard library (`insert(value, at: i)`,
`repeating(value, count: n)`) without name-variants.

```cplus
fn add(n1: i32, n2: i32 = 1) -> i32;

add(1, 2)            // positional        → n1 = 1, n2 = 2
add(1)               // n2 omitted        → n1 = 1, n2 = 1   (default)
add(n2: 2, n1: 1)    // named, reordered  → n1 = 1, n2 = 2
```

## Syntax

**Default values** — a trailing `= EXPR` on a parameter:

```cplus
fn f(a: i32, b: i32 = 0, c: Text = Text::new()) -> i32;
```

**Named arguments** — `label: value` at the call site, where the label is the
parameter's own name (one-name model: the parameter name *is* the label; there
is no separate external label):

```cplus
f(1, b: 2)
f(1, c: t, b: 2)
```

## Semantics — argument matching

A call's arguments are matched to the callee's parameters `p0..pn`:

1. **Positional pass.** Bind positional arguments left-to-right to `p0, p1, …`.
   More positional arguments than parameters is an error.
2. **Named pass.** For each `label: value`, find the parameter named `label`.
   Not found → *unknown label*. Already bound (positionally or by an earlier
   label) → *duplicate argument*. Otherwise bind it.
3. **Defaults pass.** Any parameter still unbound uses its default expression if
   it has one; otherwise → *missing required argument*.

Reordering is inherent in step 2 (binding is by name), so `add(n2: 2, n1: 1)`
binds in either written order.

## Locked rules

1. **One-name labels.** The parameter name is the label. No Swift-style separate
   external label (`at index: Int`) — smaller surface, fits the "one way" rule.
2. **Positional before named.** Positional arguments may precede named ones, never
   the reverse:
   - `add(1, n2: 2)` ✓
   - `add(n1: 1, 2)` ✗ — *positional after named*
3. **Defaults must be trailing.** A defaulted parameter cannot be followed by a
   non-defaulted one (`fn f(a = 0, b)` is illegal), so positional calls like
   `add(1)` stay unambiguous. (A later relaxation could allow non-trailing
   defaults filled by name; out of scope for v1.)
4. **Positional arguments keep declared order;** named arguments may be reordered.

This deliberately relaxes "one obvious way to do it" for calls — `add(1, 2)`,
`add(n1: 1, n2: 2)`, and `add(n2: 2, n1: 1)` are three spellings of one call —
in exchange for Python-grade ergonomics and self-documenting call sites (also a
win for LLM legibility).

## Constraints

- **`extern fn` takes no defaults and no labels.** The C ABI has neither; an
  `extern` declaration with a default is an error.
- **Default expressions are compile-time / `const`-evaluable in v1** (literals,
  `const` values, simple constructors). Arbitrary call-site expressions (Swift's
  model) can come later.
- **Per-call evaluation.** A default is spliced into each call site that omits the
  argument, so e.g. `c: Text = Text::new()` constructs a fresh value per call.
- **Generics.** A default expression is type-checked against the parameter's
  (possibly generic) type and instantiated under each call's substitution.

## Errors

Allocate a contiguous block from `docs/errors.toml` (via `gen_errors.py`):

| code | meaning |
|---|---|
| `E09xx` | unknown argument label `name` for `fn` |
| `E09xx` | duplicate argument: `name` given more than once |
| `E09xx` | missing required argument `name` |
| `E09xx` | positional argument after a named argument |
| `E09xx` | non-trailing default parameter (`a = … , b` with `b` required) |
| `E09xx` | default value on an `extern fn` parameter |

## Parser disambiguation

At a call site, each argument is either `EXPR` (positional) or `IDENT : EXPR`
(named). Inside the argument list `( … )`, two-token lookahead is decisive: an
`IDENT` immediately followed by `:` begins a named argument (consume `IDENT`,
`:`, then parse the value `EXPR`); anything else is a positional `EXPR`. C+ has
no expression-level type ascription, so `IDENT :` in argument position is
unambiguous. The turbofish (`f::[T](…)`) is unaffected — it precedes the `(`.

## Implementation architecture

The whole feature is **front-end**: labels and defaults are resolved to a
complete positional argument list during sema, so codegen, monomorphization,
mangling, and the C ABI are untouched.

- **AST**
  - `Param` gains `default: Option<Box<Expr>>` ([ast.rs](../../cplus-core/src/ast.rs) `struct Param`).
  - `Call` (and `GenericEnumCall`) gain `arg_labels: Vec<Option<Ident>>`, parallel
    to `args`, set by the parser. There is a single `Call` node for both free-fn
    and method calls, so only these two nodes need labels.
- **Parser** — parse trailing `= EXPR` in parameter lists; parse `IDENT : EXPR`
  named arguments (filling `arg_labels`).
- **Sema — the desugar.** In the call checker, run the matching algorithm: reorder
  `args` into positional order by label and splice in default expressions for
  omitted parameters, producing a canonical positional `args` with `arg_labels`
  cleared. All downstream passes (rest of sema, mono, codegen) see exactly what
  they see today. Emit the diagnostics above on mismatch.
- **Codegen / mono** — no change.

### Phasing

1. **Named parameters** — AST `arg_labels`, parser, the matching algorithm
   (positional + named passes, no defaults yet → omitted = error), diagnostics,
   tests. Re-skin a couple of stdlib call sites as proof.
2. **Default values** — AST `Param.default`, parser trailing-`= EXPR`, the
   defaults pass (splice), trailing/extern validation, tests.
3. **Re-skin `Vec` / `Text`** under the Swift-flavored convention as the proving
   ground (`insert(value, at: i)`, `repeating(value, count: n)`, …).

## Non-goals (revisit later)

- Two-name labels (external label ≠ internal name).
- Non-trailing defaults filled by name.
- Arbitrary (non-`const`) default expressions.
- Variadic / keyword-rest parameters.
