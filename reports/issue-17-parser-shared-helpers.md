# Issue 17 — Parser shared helpers: one list parser, one FIRST set, one contextual-keyword check

- Status: DONE 2026-08-02, commit b186b9f — (a)–(e); (f) was the umbrella and
  its two owners landed in the bug tier
- Type: consolidation
- Area: `cplus-core/src/parser.rs`
- Effort: S
- Retires / prevents: the sync-rot class behind bug-15; named-arg loss in enum-ctor
  calls; per-site follow-set divergence for contextual keywords
- Master report: `core-drift-audit-2026-08-01.md` (§2 parser rows; parser audit F6-F9, F14-F15)

Independent small consolidations; land in any order. Line numbers from 2026-08-01.

## (a) Bracketed type-arg list parsed 10x

Identical `while !RBracket { parse_type(); eat(Comma) }` loops at: 1902-1908 (type
position), 3052-3057 (method turbofish), 3105-3111 (postfix turbofish), 3639-3645
(intrinsic turbofish), 3739-3746 + 3785-3791 (qualified enum-ctor/struct-lit),
3824-3831 + 3873-3879 (bare enum-ctor/struct-lit), 4150-4159 + 4171-4181 (patterns, two
spots). One `parse_bracketed_type_args() -> Result<Vec<Type>>` helper; all ten call it.

## (b) Generic-params parser exists twice

`parse_generic_params` (1579-1613) and `parse_optional_generic_params` (596-626) are the
same grammar with different span math and error strings — a bounds-grammar change must be
made twice. Fold into one (the optional variant wraps the required one).

## (c) Enum-ctor argument loops bypass `parse_call_args`

3750-3758 and 3835-3843 re-implement positional-only argument parsing: named arguments
(labels) are silently unsupported in `Option[i32]::Some(...)`-shaped calls, and the loops
miss the line-dot reset (bug-14's combinator covers the reset once it lands). Route both
through `parse_call_args`.

## (d) Triplicated statement-lookahead guard with raw index access

The `While/If` + (`Let` | `var`-leads-pattern) dispatch guard is copy-pasted at
2023-2026, 2092-2095, 2140-2143, each using raw
`self.tokens.get(self.pos + 1).map(|t| &t.kind)` instead of the existing `peek_kind_n`.
One `at_pattern_let_head(kw) -> bool` predicate.

## (e) Contextual keywords: 14 string-compare sites, each with a hand-rolled follow-set

`s == "ref"/"take"/"var"/…` at: receiver position (984, 991 — followed-by-`SelfLower`),
param position (1667, 1675 — followed-by-`Ident`), fn-ptr param (1823-1836 —
followed-by-`Ident|Star|Fn`), `var` pattern head (2356, 2372 —
followed-by-`Ident|Underscore`), plus asm's closed sub-grammar (647-698 — fine, leave
it). The contextual-keyword POLICY is deliberate (de-Rust); the drift is per-site
follow-sets. Known consequence: `fn(ref (i32, i32))` / `fn(ref [T; 2])` misparse `ref`
as a type name (tuple/array starts absent from the fn-ptr follow-set) with a confusing
error.
One shared `contextual_kw_at(n, kw, FollowSet) -> bool`; fix the fn-ptr follow-set to
include tuple/array/type starts while consolidating (that is a small behavior FIX —
test it).

## (f) Also owned here once bug-13/bug-14/bug-15 land

The `in_delimited` ExprCtx combinator (bug-14) and `starts_expr` (bug-15) become the
shared owners for their questions; this issue is the umbrella for keeping parser
predicates single-sourced. After all of it: grep the parser for `tokens.get(self.pos`
raw indexing — remaining hits should be inside the peek helpers only.

## Verification

- Full suites after each item (`cargo test -p cplus-core` runs the ~172 parser unit
  tests; `cargo test -p cpc --test e2e` the rest).
- (c) gains a test: named args in an enum-ctor call parse (or produce the intended
  "labels not allowed here" diagnostic if that is the design — check how struct-lit
  fields treat labels first and match).
- (e) gains tests: `fn(ref (i32, i32))` and `fn(ref [i32; 2])` parse as fn-ptr types
  with ref-mode tuple/array params.

## Outcome

**(a) One bracketed type-arg parser.** `parse_bracketed_type_args(close_msg)`
replaces ten copies of the same `while !RBracket { parse_type(); eat(Comma) }`
loop — type position, three turbofish forms, the qualified and bare enum-ctor
and struct-literal heads, and two pattern spots. Each site kept its own closing-
bracket message, which is the one thing that legitimately differed, so that is
the parameter.

**(b) One generic-parameter parser.** `parse_optional_generic_params` is a
one-line delegation to `parse_generic_params`. They were the same grammar with
different span math (the name's span vs name-through-last-bound) and different
error strings; the surviving one keeps the wider span, which is what a
bounds-related diagnostic wants to underline.

**(c) Enum-ctor arguments go through `parse_call_args`.** Both spellings —
`dep::Maybe[i32]::Some(x)` and `Maybe[i32]::Some(x)` — re-implemented
positional-only argument parsing, so a named argument was silently dropped
instead of desugared or rejected. They call the shared parser now, which is
also where the line-dot reset lives (bug-14's `in_delimited`).

**(d) One pattern-let head predicate.** `at_pattern_let_head()` replaces three
copies of the `while`/`if` + (`Let` | `var`-leads-pattern) guard, each of which
reached into `self.tokens[self.pos + 1]` by raw index rather than through the
peek helpers. One raw-index site remains in the parser
(`at_destructure_binding`), which the report's (f) grep target allows for.

**(e) One contextual-keyword follow-set for fn-pointer params — and a fix.**
`fn_ptr_param_marker_at(kw)` is the shared check, and consolidating exposed the
divergence the report predicted: the follow-set was `Ident | Star | Fn`, so
`fn(ref (i32, i32))` and `fn(ref [i32; 2])` parsed `ref` as a TYPE NAME and
failed with a confusing error. Every token that can begin a type is in the set
now. `fn_pointer_param_markers_accept_every_type_start` pins both the new
spellings and the old ones, plus the case that must NOT change: a type genuinely
named `ref`, with no type following, is still a type.

**(f)** The umbrella. Its two named owners — `in_delimited` (bug-14) and
`starts_expr` (bug-15) — landed in the bug tier; this issue's items now route
through them rather than around them.

## Verification (as run)

`cargo test -p cplus-core` 1851 + 8 (the ~172 parser unit tests included),
`cargo test -p cpc` 608 + 16 + 5 + 6, `cpc test` in `vendor/stdlib` 290, and the
vendor-wide `cpc check` sweep at its recorded baseline for all 54 packages.
