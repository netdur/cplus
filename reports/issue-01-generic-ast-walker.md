# Issue 01 — Generic mutable AST traversal (kill the missing-arm bug family)

- Status: DONE 2026-08-02, commit e0de12c
- Type: structural consolidation
- Area: `cplus-core/src/ast.rs` (new walker), `monomorphize.rs`, `lower.rs`
- Effort: M
- Retires / prevents: bug-04, bug-06, bug-07 (live ICEs); Await/Yield and G-026
  (historical, same family); every future ExprKind addition
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #1)

## Problem

Monomorphize alone contains four hand-rolled AST walkers, lower three more. Each is a
match over ExprKind/StmtKind with a `other => other.clone()` fallthrough. Every arm
missing from any walker is a latent bug: the audit reproduced three ICEs from exactly
this (tuple literal, interp string, Self-in-loop), and the code memorializes two more
fixed the same way (Await/Yield arms, monomorphize.rs:2826-2830). Adding one ExprKind
today requires touching 7+ walkers across 2 files to not ship a bug.

## Current state (the walkers to migrate)

In monomorphize.rs:

1. `rewrite_expr` / `rewrite_stmt` (~2426-3246) — call-site renaming to mangled names.
   Fallthrough at ~3240. Missing arms found: TupleLit, InterpStr, Asm.
2. `rewrite_expr_self` / `rewrite_stmt_self` (~972-1102, ~909-944) — Self substitution.
   Partial clone with FEWER arms (no Loop/Defer/Assert/LetDestructure statements; no
   GenericEnumCall/Await/Yield/Range/InterpStr/FnRef/TupleLit expressions). Driven by
   `rewrite_block_with_self` (~870-892).
3. `rewrite_alias_type` family (~3363) — alias-domain type rewriting.
4. `visit_ident_calls` (~1246-1400) — read-only discovery of generic calls; must traverse
   the SAME set of nodes as walker 1 (the discovered/not-rewritten asymmetry is bug-06).

In lower.rs (verify exact names by reading the file): the const-subst walk, the
lens/desugar resolution walk, and the if-let/guard-let lowering walk.

## Target design

Add to ast.rs:

```rust
pub trait ExprRewriter {
    /// Return Some(replacement) to substitute this node (children NOT auto-visited),
    /// or None to keep the node and recurse into its children.
    fn visit_expr(&mut self, e: &Expr) -> Option<Expr> { None }
    fn visit_stmt(&mut self, s: &Stmt) -> Option<Stmt> { None }
    fn visit_type(&mut self, t: &Type) -> Option<Type> { None }
}

pub fn walk_expr(e: &Expr, r: &mut impl ExprRewriter) -> Expr;
pub fn walk_stmt(s: &Stmt, r: &mut impl ExprRewriter) -> Stmt;
pub fn walk_block(b: &Block, r: &mut impl ExprRewriter) -> Block;
```

Rules:

- `walk_expr` matches EXHAUSTIVELY on ExprKind — no `_` arm — so a new variant fails to
  compile until the walker handles it. Same for StmtKind, PatternKind, TypeKind.
- Default behavior is full recursion + reconstruction (clone-with-rewritten-children);
  a rewriter only overrides the node kinds it cares about.
- A read-only visitor variant (for `visit_ident_calls`) can be a thin adapter that
  returns None always and records what it sees, guaranteeing discovery and rewrite
  traverse identical node sets.

## Migration plan

1. Implement the walker in ast.rs with unit tests (round-trip identity: walking with a
   no-op rewriter reproduces the tree).
2. Migrate `rewrite_expr`/`rewrite_stmt`: the rewriter overrides Call/MethodCall/Path
   (the call-rename cases) and GenericStructLit etc.; everything else recurses by
   default. Existing mono unit tests + full suites green. This alone fixes bug-04 and
   bug-06 if their tactical arm-fixes have not landed yet.
3. Migrate `visit_ident_calls` to the read-only adapter.
4. Delete the Self walker pair: Self becomes a substitution key in the main rewriter
   (see `issue-10-merge-method-mono-paths.md`; fixes bug-07 structurally).
5. Migrate `rewrite_alias_type`, then lower.rs's walkers, one per commit.

## Verification

- Round-trip unit test in ast.rs; the bug-04/06/07 repros as e2e tests.
- After each migration commit: `cargo test -p cplus-core && cargo test -p cpc --test e2e`.
- Grep for remaining `other => other.clone()` fallthroughs in mono/lower when done —
  target is zero.

## Risks and constraints

- Perf: mono rewrites clone heavily already; the walker's reconstruct-on-walk matches
  current cost. Do not add Rc/interning as part of this change.
- Behavior lock: the walkers must preserve exact span/attribute propagation of the
  current code — the span-keyed side tables (see issue-16) depend on spans surviving
  rewrites unchanged.

## Outcome

Landed as described, with the migration order the report set out. What is in the
tree now:

- `ast.rs` — `ExprRewriter` (four hooks: expr, stmt, type, pattern) plus
  `walk_expr` / `walk_expr_kind` / `walk_stmt` / `walk_block` / `walk_for_loop` /
  `walk_type` / `walk_pattern`, every one an exhaustive match with no catch-all,
  and `visit_exprs` / `visit_exprs_in_block`, the read-only adapter over the
  same walk. `walk_expr_kind` is the child half of `walk_expr`, for a hook that
  keeps a node but changes something about the node itself (`Respan` does).
- `monomorphize.rs` — `MonoRewriter` (visit_expr for FnRef/Call/StructLit/
  InferredStructLit/GenericStructLit/GenericEnumCall, visit_type for every type
  position) replaces `rewrite_expr` / `rewrite_stmt` / `rewrite_for`;
  `AliasRewriter` replaces `rewrite_alias_expr` / `_stmt` / `_block`;
  `visit_ident_calls_in_block` is now the read-only adapter.
- `lower.rs` — `ConstSubst` replaces `subst_expr` / `subst_stmt`, `LenResolver`
  replaces `resolve_lens_in_expr` / `_stmt` / `_block`, and `Respan` replaces
  `respan_tree`'s hand-rolled recursion.

Step 4 (delete the `Self` walker pair) had already landed in the bug tier: `Self`
is a substitution carried on `StructLookup`.

### Three live bugs the migration fixed

Each is a construct one of the fallthroughs skipped; each reproduces on the
pre-migration binary and is covered by the e2e test
`ast_walk_reaches_constructs_the_hand_rolled_walkers_skipped`:

1. `(7 as Meters, 1)` — a type alias inside a tuple literal. `rewrite_alias_expr`
   had no TupleLit arm, so the alias survived into codegen: "codegen reached
   Ty::Error — sema should have rejected the program".
2. `([5; N], 2)` — a const-named array-fill length inside a tuple literal.
   `resolve_lens_in_expr` had no TupleLit arm, so `count_name` was never
   resolved and sema reported `E0330: array literal has 0 element(s)`.
3. `let b: Buf = { data: [6; N] };` — the same lens inside an INFERRED struct
   literal, which `resolve_lens_in_expr` also had no arm for. Same bogus E0330.

`respan_tree`'s fallthrough was a fourth, milder one: it covered only the const
initializer shapes that existed when it was written, so when that grammar grew
struct and array literals, their field values kept definition-site spans in a
cross-file diagnostic.

### Deliberate scope limits

- `subst_ty_plain`'s `other => other.clone()` in monomorphize stays. It matches
  on `Ty` (sema's semantic types), not on the AST, and the arm is over leaf
  primitives — a different enum with a different walker, out of this issue's
  scope.
- Patterns: `walk_pattern` descends into payload sub-patterns, a literal
  pattern's expression, and a variant pattern's type-args. That last one is
  new — the previous walkers cloned patterns wholesale — and it is safe because
  codegen reads only `variant_name` and `payload` from a pattern; `enum_name`
  and `type_args` are dead by then.
- Builder blocks are walked too, though they are desugared before sema and no
  rewriting pass should meet one. A walker with a blind spot is the bug this
  module exists to prevent.

### Cost

`cpc check` on the largest single source in the tree (`vendor/uikit`, 76k lines)
goes 1.18s → 1.23s, about 4%, from the read-only discovery walk now
reconstructing the tree it throws away. Returning a leaf placeholder from the
adapter to skip that reconstruction was measured and is not faster. The full
e2e suite (604 compile-and-run programs) is unchanged at ~93s, and clang
dominates any real build, so the trade — one exhaustive walker for a 4% front-end
cost on an outlier file — was taken deliberately.

## Verification (as run)

- `ast.rs` unit tests: no-op-rewriter identity round-trip over a sample program
  covering every construct family; read-only visit and rewrite walk agree on
  node count; the walk reaches interpolated-string parts, tuple elements and
  array-fill values.
- e2e `ast_walk_reaches_constructs_the_hand_rolled_walkers_skipped` (the three
  bugs above).
- `cargo test -p cplus-core` 1837 + 8, `cargo test -p cpc` 604 + 16 + 5 + 6, all
  green; `cpc test` in `vendor/stdlib` 290 green in debug and `--release`;
  vendor-wide `cpc check` diagnostic-count parity against the pre-change binary
  across all 54 packages — no change.
