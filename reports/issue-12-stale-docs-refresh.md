# Issue 12 — Stale documentation refresh (pure-comment PR)

- Type: cleanup (comments/docs only, zero behavior change)
- Area: sema.rs, monomorphize.rs, prune.rs, parser.rs, lexer.rs, attrs.rs
- Effort: S
- Retires / prevents: every new reader (human or model) being misled at the front door
  of the two biggest files
- Master report: `core-drift-audit-2026-08-01.md` (§5 stale docs)

Verify each quote against the current tree before editing (line numbers from
2026-08-01). Style: plain reference register, no editorial asides, no overclaiming.

## 1. sema.rs module header (line ~3)

Current: opens with "Phase 1 scope: only `i32` and `bool` types" — on the 32k-line file
implementing the full type system.
Replace with a real overview naming the major regions, e.g.:

```
//! Semantic analysis: name resolution completion, type checking, move checking.
//!
//! Major regions: type table construction and Copy/Drop classification; expression
//! and statement checking (flow-sensitive moves, divergence); method resolution and
//! blessed-method dispatch; generic instantiation recording (MonoInfo) for the
//! monomorphize pass; view/borrow gates that pre-screen for borrowck.
//! Errors here bail the pipeline before borrowck runs.
```

Adjust the region list to match reality while editing.

## 2. monomorphize.rs module header (lines ~1-35)

~90% false. It claims: "No bound checking yet (E0502 deferred)" — E0502 fires at
sema.rs:3414; "No turbofish `::[T]`" — the turbofish path is at monomorphize.rs:2504-2519
plus `mangle_call_from_ast`; "Generic methods inside `impl` blocks are deferred" — both
expansion paths shipped; "generic types … don't yet have an instantiation surface" —
struct/enum instantiations are emitted at 261-334.
Replace with the actual pass shape: aliases → instantiation propagation (fixpoint,
E0910 cap) → twin method expansion (concrete-struct and generic-struct paths) → call
rewriting to mangled names → bound-method bridges → template removal.

## 3. prune.rs (~line 48)

Current: "Removal is iterated to a fixed point." The implementation is a single
reachability walk from roots (119-130). Same result; fix the sentence.

## 4. parser.rs stale cross-references

- 3729-3734: "Mirrors the bare-Ident paths at lines 1707 and 1816 below" — those numbers
  now point into parse_param/fn-ptr-type parsing. Replace the line numbers with function
  names (line refs rot; names do not).
- 3881-3893: leftover half-edited design musing ("…is a sentinel here is overkill.
  Cleaner: extend ExprKind…") — either promote to a real TODO with an owner decision or
  delete.

## 5. sema.rs check_interp_str doc (~12652-12655)

Current: "Result type is `Ty::String`." False — the body returns the designated Text
struct or E0613. Fix the sentence; the unreachable `matches!(&ty, Ty::String)` arm at
12665 is deleted by issue-11 item 5 / bug-20.

## 6. sema.rs borrow_ doc (~374-382)

Current: describes `borrow_` as "purely informational". It is compared in
`method_sig_matches` and guards E0328. Deleted entirely by issue-11 item 1; if that
lands first, this entry is moot.

## 7. monomorphize.rs synthesize_fn comment (~1765-1767)

Current: "Defaults are already spliced into every call site by `lower` … vestigial."
Contradicted by mono's own `default_splices` application at 2643. State the real
three-pass splicing split (lower: type-free by name/arity; sema: records what lower
could not; mono: appends) and point at issue-16 for the consolidation direction.

## 8. monomorphize.rs mangle_call_from_ast rationale (~3267-3272)

Current: justified by file-less ByteSpan collisions — fixed by v0.0.22 file-aware spans
(acknowledged at 2476-2478). Update the comment to say the path is retained pending the
consolidation decision (or fold into issue-02's longer-term direction).

## 9. lexer.rs nl_before doc (~56-62)

Current: "no other grammar reads it" — already inexact (builder entries read it) and
wrong once bug-13's fix lands. Reword to list the actual consumers.

## 10. attrs.rs no_alloc doc (~135)

Current: "free functions only" while its target mask includes METHOD. Fix the sentence
to match the mask.

## Verification

`cargo build --release` (comments cannot break it, but the diff review should confirm
zero non-comment lines changed) and one full `cargo test -p cplus-core` for hygiene.
