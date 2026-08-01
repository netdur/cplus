# Issue 01 — Generic mutable AST traversal (kill the missing-arm bug family)

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
