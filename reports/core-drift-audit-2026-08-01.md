# cplus-core drift audit — 2026-08-01

> **Status 2026-08-02 — the whole BUG tier is fixed and committed (13 commits,
> `c071b39`..`8abe7b6`).** All 26 bug reports are marked FIXED with their commit sha, and
> each carries a Verification section rewritten to say what was actually done — including
> where a report's own expectation turned out to be wrong. Regression tests: 26 new e2e
> tests plus unit tests in parser/codegen/mono/graph/attrs. Green at every commit:
> `cargo test -p cplus-core` (1834), `cargo test -p cpc` (629 across 5 files), the stdlib
> suite in BOTH debug and release (290 each), and diagnostic parity against the pre-audit
> binary across every `vendor/*` package and every `examples/*` project.
>
> **Status 2026-08-02 (second pass) — the ISSUES tier is worked, in number order
> (`afe362c`..`cdc6f04`, 34 commits: one per issue plus its stamp).** Every one of the 17
> reports carries `Status: DONE` or `Status: PARTIAL` with the commit that landed it, and
> a written-up outcome saying what was done, what was deliberately left, and what the
> report itself got wrong. Six are DONE (01, 02, 04, 05, 10, 12, 17); the rest are PARTIAL
> with the remainder scoped in their own file. Green at every commit: `cargo test -p
> cplus-core` (1852), `cargo test -p cpc` (608 + 16 + 5 + 6), the stdlib suite in debug and
> release (290 each), vendor-wide `cpc check` parity across all 54 packages, and — for
> every codegen-touching commit — `--emit-ll` byte-identity over the 40 `docs/examples`
> programs plus purpose-built ABI, C-ABI and method probes.
>
> Bugs fixed as a side effect of the structural work, each with a regression test:
> - bug-27 (the tuple-in-generic ICE this report found) — three of its four shapes;
> - a type alias inside a tuple literal reached codegen as `Ty::Error` (issue-01);
> - `[v; N]` inside a tuple literal or an inferred struct literal never resolved its
>   const length, and sema rejected it as a 0-element array (issue-01);
> - a generic instantiated at a unit-returning fn-pointer ICE'd, because the AST and `Ty`
>   printers spelled the return differently (issue-02);
> - four method-call sites passed a bare `ptr` where the callee declared `sret(...)`,
>   working only through an LLVM fallback the `objc_msgSend` clobber proved unreliable
>   (issue-03);
> - the impl-block bounds gate was unenforced on the enum dispatch path (issue-05);
> - a user type named `Iterator`/`Option` shadowed the stdlib one, per-process randomly
>   (issue-06);
> - a typo'd import prefix reported two different errors depending on syntactic position,
>   one of them naming a type the user never wrote (issue-09);
> - E0325 quoted `impl E: I` back as `impl I: E` (issue-11);
> - a value-passed `f16` parameter lost its `noundef` (issue-15);
> - `fn(ref (i32, i32))` and `fn(ref [i32; 2])` misparsed `ref` as a type name (issue-17).
>
> Corrections to the issue reports, found while implementing (details in each file):
> - issue-03's claim that the musttail return-coercion predicate is a divergent third copy
>   of the `want_c_abi_ret` gate is wrong: it asks a narrower question that the
>   enclosing-side check already covers.
> - issue-04's `ParamSig` name collides with `sema::ParamSig`, which means the SURFACE
>   flags where the new type means the LOWERED ones; it is `ParamAbi`.
> - issue-06's sema-side fix is not sufficient on its own — codegen's coroutine and
>   Option lookups match by mangled name too, and the negative test only passes once they
>   filter on the lang flag.
> - issue-11 items 4, 9 and 10 were already done by issues 01, 10 and 07.
> - issue-02's `Ty::Param`-vs-bare-`Path` divergence is not fixable in the printer and is
>   not a bug: the AST cannot tell a type parameter from a struct of the same name, and
>   the resulting lookup miss is the right answer.
>
> **Status 2026-08-02 (third pass) — issue-07 is DONE (`f4e5a62`..`66836d1`, 10
> commits) and issue-06 is DONE (`1e33595`, `1c7cce4`, `4c83e57`).** The view family
> (E0513/E0515/E0516) now lives in borrowck as `ViewRules`, and the lift is asked of
> the flow pass instead of hand-encoded in sema, so §3's silent-unsoundness seam is
> closed by construction. The transition release the report insisted on found three
> real coverage holes that no test and no vendor program exercised — match payloads,
> tuple returns, index write targets — each of which the port would otherwise have
> shipped as silence. §4's leaf-name identity is closed for the marker family too:
> `impl T: Send {}`, the builtin `!Send` list and the `#[no_alloc] fn drop` blessing
> all key on resolved identity now, and `__cplus_` is one constant with a
> `#[runtime_abi]` claim marker (E0919).
>
> issue-13 (b) and issue-15 (b) are DONE too (`f7704c8`, `b818a1b`), and both took a
> different shape than their reports prescribed: the drop rule and the layout rule are
> now each written ONCE, over a shared `TypeShape` seam each pass implements over its
> own ids, rather than mirrored with a check that the mirrors agree. issue-15 (b)'s
> differential — written first, as the report insisted — found a live divergence the
> audit had missed: sema hard-coded 8 bytes for every pointer-shaped type while codegen
> asked the target, so `#[max_stack]` over-counted every pointer on a 32-bit build. Both
> rules also turned out to be missing a cycle guard; that is `bug-28`, found here, its
> compiler-hang half fixed and its lowering half written up.
>
> `bug-28` is FIXED end to end (`b818a1b` the hang, `892799f` the lowering) and
> issue-07 step 5 is now PREPARED rather than merely deferred: its e2e corpus exists
> (`f3a5a4e`), written against the pre-port binary, which is the safety net the family
> was missing — it had none, where the view family had 44 assertions and still needed
> the transition assert to catch three holes.
>
> **Status 2026-08-03 (fourth pass) — issue-07 is DONE END TO END (`59e8a73`..`53235d5`,
> 7 commits).** Step 5 ported the capture family (E0365) into `ViewRules` as one
> classifier, one ownership gate and three sinks the walk already visits, and deleted
> sema's copy — the fixpoint, the taint dataflow, `check_returned_borrow`, and the two
> walkers that existed only for it. Sema now emits no view or capture diagnostic and
> holds no belief about borrowck's coverage, which is what §3 was written about.
>
> The transition assert earned its release again: it found that sema's `owns_value`
> means `param.move_ || is_copy(ty)`, so a capture of a by-value **Copy parameter**
> dangles where a VIEW of one is harmless — the two families read identically in prose
> and disagree on exactly that shape. Nothing exercised it: not the 16 sema unit tests,
> not the new e2e corpus, not one of 274 vendor and examples sources. The A/B sweep,
> which sees the direction the assert cannot, caught one over-fire in the other
> direction (an enum-constructor argument is not a call that can keep anything). Probing
> also surfaced an unrelated pre-existing codegen ICE — a bound-method reference inside
> a generic impl panics at `codegen.rs:2233`, identically before and after — which wants
> its own bug file.
>
> That E0365 was missing from `docs/errors.toml` turned out not to be one gap. An audit
> of every `E####` the compiler emits against the catalog found ELEVEN live codes with no
> entry (E0343, E0344, E0362, E0365, E0383, E0384, E0822, E0823, E0824, E0869, E1900) and
> THREE entries for codes it can no longer emit (E0862, E0863 — the `[link].triples` key
> they check is now a parse error — and E1003, shadowed by E1002). All twelve now match
> the compiler (`170b1b7`), every `repro = "checked"` example was compiled and confirmed
> to emit its own code, and the three that could not honestly claim it say why in a note.
> Two of those are a finding in themselves: **E0383 and E0384 are live in borrowck but
> unreachable through the driver**, because returning a borrow of a parameter is a sema
> E0337 and sema bails before borrowck runs.
>
> **bug-29 — a bound method reference inside a generic impl body ICEs codegen**
> (`5f511e7`), found while probing step 5 and present identically before it. Generic-impl
> method bodies are never TYPE-checked, so sema never recorded the reference,
> monomorphize never rewrote it, and codegen read `this.handler` as a field:
> `.expect("sema validated")`. Refused at check time now (E0822).
>
> **Status 2026-08-03 (fifth pass) — that seam is a bug FAMILY, now
> `reports/issue-18-generic-impl-body-checking.md` (`ee10963`, `469c2b3`).** Probing it
> with one program per span-keyed record — each written twice, once in a generic impl
> body and once in a generic free fn as the control — found FIVE distinct ICEs on
> ordinary source (`#env`, `#include_str`, an inferred struct literal, an inferred
> generic call, an inferred tuple literal) plus a false E0300 on a turbofish type
> argument. Every free-fn control worked, which is what identified the seam: free-fn
> bodies were given exactly this treatment earlier, after the same class of crash.
> Generic impl-method bodies are type-checked now — the target instantiated at its own
> parameters — which closes all five and **bug-27 shape 4** with it.
>
> The typed pass's DIAGNOSTICS are deliberately discarded, because turning them on is a
> separate decision with real work behind it: it reports ~250 diagnostics against the
> stdlib's own containers. The two that matter are findings in their own right —
> `let v: T = { *p }` in `Box`/`Vec`/`Rc`/`Arc`/`channel` does not satisfy E0337 (a
> container moving a value out of storage it owns and is about to free), and
> `struct HashMap[K: Copy, V: Copy]` calls `k.hash()` / `k.eq()` on a parameter that
> bounds neither, a contract asserted in a header comment instead of in the type. Both
> are the audit's recurring shape: a rule and its only real consumer drifted apart with
> no pass in between.
>
> Not done, and scoped in their own reports: issue-14's migration off the classification
> fixpoint (its characterization harness is in the tree), issue-03 step 4, issue-08 steps
> 1 and 3, issue-09 parts (A) and (C), and issue-11 items 1, 2, 3, 5, 7, 8.
>
> Corrections to this report, found while fixing (details in each bug's file):
> - B12's generic-argument half was already closed by the B1 fix; its `take` spelling
>   errors CORRECTLY, matching the concrete path.
> - B10's secondary move-join effect is not reproducible, before or after — the match's own
>   arm handling already drops a diverging arm's moves.
> - B13's prescribed fix (Rust's expr-with-block rule) would have REJECTED code that
>   compiles today; the report's line-aware alternative was taken instead.
> - B21's alignment covers 3 of the 5 bounds listed — `Ord` and `Clone` have no primitive
>   dispatch, so admitting them would create the mirror-image mismatch.
> - B15's t13 repro names an intrinsic that does not exist (`#len`, not `#slice_len`);
>   B11's repro has a second, unrelated pre-existing error; B10's expected exit code is
>   wrong (7, not 2).
>
> One new bug was found while verifying and is written up rather than folded in:
> `bug-27-tuple-type-in-generic-body-ice.md` — a tuple TYPE naming a type parameter inside
> a generic body ICEs, identically before and after this work. Its fix is a new
> instantiation-propagation pass, not a missing arm.

Scope: every pass except borrowck (recently reworked, explicitly excluded; nothing found against
it incidentally either). Method: seven parallel deep-reads over sema (type core + checking/flow),
codegen (ABI + emission), parser/lexer, monomorphize/lower/prune, resolver/graph/attrs, each
hunting the same drift taxonomy; claims verified by reading surrounding code, and the serious
ones reproduced against `target/release/cpc`. Four of the worst were re-verified independently
after the audits returned.

Churn context: 312 commits touched `cplus-core/src` since May, 207 mention "fix"; codegen (181
file-touches) and sema (179) absorbed most of them.

The one-line diagnosis: the compiler's recurring failure mode is **one decision implemented at N
sites that must agree by convention**. Every reproduced miscompile and ICE below is an instance
of a site that forgot its mirror. The high-value work is not fixing the instances; it is
collapsing each decision to one owner.

---

## 1. Verified bugs (all reproduced this session)

Re-verified independently after the audit returned:

| # | Bug | Where | Severity |
|---|-----|-------|----------|
| B1 | Generic `ref` args skip E0328: `fn bump_g[T: Copy](ref x: T, v: T)` on a frozen `let` compiles (inference AND turbofish) and mutates it at runtime (probe exits 99). Concrete path correctly rejects. | sema.rs:14710 is the only enforcement site; `check_generic_named_call` (11603) and `check_generic_method_call` (12592) never run it | soundness |
| B2 | `async fn bump(ref n: i64)`: def emits `(i64 %0)`, call passes `ptr` → prints stack garbage, write-back lost, no diagnostic. `gen_gen_function` has the same loop shape. | codegen.rs:7371-7379, 7226-7233 (9 def emitters; these 2 forgot `param_passes_by_ptr`) | miscompile |
| B3 | Release-only miscompile from `!alias.scope`/`!noalias` metadata on `ref` params: pairwise-disjointness promise that the 2026-07-27 `noalias`-attribute fix removed was left in metadata form. Probe: debug 23, release 20. Fifth member of the unsound-IR-attribute family, behind the same seams (statics, raw pointers). | codegen.rs:6652-6694, 8272-8323, applied textually at 3535-3618 | miscompile |
| B4 | Mono ICE: generic call inside a tuple literal — `rewrite_expr` has no `TupleLit` arm, call keeps template callee, template deleted. | monomorphize.rs:2426-3246, fallthrough at 3240 | ICE |

Reproduced by the auditors (artifacts in session scratchpad, repro paths in section 7):

| # | Bug | Where | Severity |
|---|-----|-------|----------|
| B5 | Match arm `=> { x }` bypasses the E0337 borrowed-payload escape check (bare `=> x` is caught); Drop payload bit-copied out of a field the owner still drops → double-free. | sema.rs:9727-9760 (Ident-only sniff) | soundness |
| B6 | Mono ICE: generic call inside string interpolation — discovered by `visit_ident_calls` (1314) but `rewrite_expr` has no `InterpStr` arm. `Asm` operands missing from both walkers. | monomorphize.rs:1314-1320 vs 2426+ | ICE |
| B7 | Mono ICE: `Self { .. }` inside `loop` in a generic impl — `rewrite_stmt_self` handles only Let/Expr/Return/While/For. Same code outside the loop compiles. | monomorphize.rs:909-944, 972-1102 | ICE |
| B8 | `Iterator__` substring hijack: any user generic whose base name ends in `Iterator` (`LineIterator[Token]` → `LineIterator__Token`) dispatches as a synthesized coroutine iterator; blessed `next()` arm fires before user methods. ICE, or silent miscompile (`llvm.coro.done` on a non-coroutine) if `Option[T]` is instantiated. | codegen.rs:16351 (`rfind("Iterator__")`), 14922 | ICE / miscompile |
| B9 | `-g` silently drops ALL debug info when any tail call exists: DWARF text post-pass matcher misses `musttail call`; clang then discards the module's debug info. | codegen.rs:1992-1998 | tooling |
| B10 | Spurious E0302: `check_if` uses a private divergence predicate with no `Match` arm; an if-arm ending in an all-arms-return match reads as `()`. Also distorts move-joins (latent false E0335). | sema.rs:21193-21218 vs the canonical lower.rs:2316 | false error |
| B11 | False E0335: generic-method inference branch type-checks each arg twice; nested consuming call double-marks the move. The `fnptr_field_names` pre-filter (sema.rs:1305) exists precisely to avoid this hazard elsewhere. | sema.rs:12563-12575 + 12597-12599 | false error |
| B12 | StrLit→Text coercion is per-site: enum payloads (`Holder::Some("lit")`) and generic args (`take_g::[Text]("lit")`) miss it → spurious E0302. Five inline copies of the condition exist. | sema.rs:7427, 7541, 10264, 14685, 16233 | false error |
| B13 | Parser statement-boundary family (the known E0312 bug plus four siblings): after a statement-position `if`/`match`/`{ }` block, a following `(`, `[`, `-`, or `*` statement is absorbed into the block expression. Root cause: block-likes are ordinary primaries; the postfix chain and binary cascade continue across the newline; `is_block_like` (4275) runs too late. Three divergent continuation policies exist (stmt 2150, match-arm 4081, builder 3081). | parser.rs:2150-2168, 3143-3199, 4033 | misparse |
| B14 | `no_struct_lit` never restored at delimiter recursion: `if check(Foo { x: 1 })` fails, and `if (Foo { x: 1 }).x == 1` fails even though the flag's own doc (parser.rs:71-75) says parenthesizing is the escape. Sibling flag `stop_line_dot` IS restored at every delimiter. | parser.rs: set at 9 sites, cleared nowhere | misparse |
| B15 | `can_start_expr` (4282) drifted from `parse_primary`: missing `SelfLower`, `Pound`, `LBracket`, `Await`, … → `for i in 0..this.n {}` and `0..#len(a)` fail to parse. | parser.rs:4282-4301 | misparse |
| B16 | Stack-overflow abort on deep nested patterns and builder `else if` chains — `enter_depth` covers expr/unary/block/type but not `parse_pattern` (4202) or builder ifs (3352). | parser.rs | crash |
| B17 | Graph `is_pub` tests the resolver-QUALIFIED name (`pkg.src.util._secret` never starts with `_`) → every private top-level item in every multi-file project reported public. Third divergent copy of the visibility predicate. | graph.rs:305 (+9 more sites) | wrong output |
| B18 | Resolver hand-rolls a TOML scanner; `name = "tomly" # comment` leaks the comment into the package identity → linker symbol `_tomly____the_app.src...` (nm-verified). Duplicate of manifest.rs logic (resolver.rs:1538 ≡ manifest.rs:209). | resolver.rs:1009-1036, 1502-1539 | wrong symbols |
| B19 | Method privacy enforced for one spelling only: `lib::Gadget::_hidden(g)` → E0403, but `g._hidden()` compiles clean (sema has no method-privacy check). | resolver.rs:2210-2236 vs sema method dispatch | visibility hole |
| B20 | `impl Foo: ToText` is impossible: blessed interface signature still returns legacy `Ty::String`, which nothing produces → every user impl fails E0505. Zero in-tree impls, so tests never caught it. | sema.rs:4510, 20180 | broken surface |
| B21 | `i32` satisfies `Hash` for dispatch (`x.hash()` works) but not for bounds (`fn h[T: Hash]` at i32 → E0502). Two parallel truths; makes bounded generic containers unusable at primitives. | sema.rs:3436-3466 vs 12718-12777 | inconsistency |
| B22 | Nonexistent method reported as "is private (drop the `_` to export)" — the v0.0.12 UnknownItem/PrivateAccess split was never mirrored to methods; the pre-pass only collects pub methods so existence is unknowable. | resolver.rs:2220-2235, 1878-1885 | wrong diagnostic |
| B23 | Extern-import declare drops the ref rule: `extern fn frob(ref n: i64)` → `declare void @frob(i64)` but call passes `ptr`. Right-by-accident at runtime; the declare lies to LTO. | codegen.rs:6161-6196 | latent |
| B24 | `(1.5f16).to_text()` → E0324 (f32/f64 work); f16 missing from the blessed receiver table while sibling `to_bits` supports it. | sema.rs:12718-12736 | gap |

---

## 2. The structural inventory: one decision, N sites

This is the disease behind most of section 1. Each row is one decision and every place that
implements it. "Enforced by" says what keeps them in sync today.

| Decision | Sites | Enforced by | Already diverged? |
|---|---|---|---|
| Type-mangling grammar | 5 printers: sema.rs:20036, monomorphize.rs:3632, 3693, codegen.rs:449, 2374; +sema.rs:19972 length-only; 2 parsers: codegen.rs:931, 1038 | comments ("must match sema's naming") | YES: fn-ptr unit return (`_ret_unit` vs omitted), `Ty::Param` spelling, `f16` missing from `ty_from_suffix`, no SIMD arms in `mangled_ty_take`; grammar ambiguous (`_`/`__` both separator and identifier char; struct named `i8x2` parses as SIMD) |
| Param/return ABI classification | ~15 param sites, ~10 return sites (codegen.rs:6258, 6161, 13673, 13380, 8119, 15279, 7638/15398, 8425/15078, 7371, 6808, 7226, 7044, 622, 11857; returns similar) | comments ("Must match the def-side gate") | YES: B2 (async/gen fns), B23 (extern declare); sret call-site attrs bare at 3 method-call sites (15333, 15114, 15440) vs full elsewhere — survives only via LLVM's direct-call fallback, the exact fallback whose absence caused the msgSend clobber |
| `effective_move` at sig collection | 4 sites (codegen.rs:2135, 2592, 2638, 2686) — notes said 3; the count already grew | convention | not yet (v0.0.15 json double-free was this class) |
| `x86_64_indirect_uses_byval` gate | 6 consumer sites (758, 6177, 6446, 11876, 13429, 13752) — notes said 4 | convention | once already (Xtensa, recorded at codegen.rs:3361-3364) |
| sret attribute string | ~12 hand-built copies (689, 780, 6153, 6399, 7613, 8066, 8402, 11926, 13486, 13528, 13904, 13953) | convention | consistent today |
| Divergence ("does this expr diverge") | lower.rs:2316 (canonical), sema.rs:21193 (weak), sema.rs:21073 (third variant) | nothing | YES: B10 |
| Visibility (`_` privacy) | resolver.rs:1794, sema.rs:453, graph.rs:305×10 | nothing | YES: B17 |
| Per-arg call gates | 4 pipelines: sema.rs:14669 (full), 11663/11764, 12596, 10747 | nothing | YES: B1, B11; three historical holes admitted in comments (12480, 11899, 11793) |
| Type substitution | 6 impls: sema.rs:17122, monomorphize.rs:1207, 1117, 1797, 3363, borrowck.rs:949 | comments | YES: AST-path nominal blindness caused the `Vec[Point]::new()` mis-mangle and its triple-fallback patch (mono 3012-3043) |
| SIMD name→type table | 3 copies ×~150 lines: sema.rs:16657, 19528, codegen.rs:17669 | comment mandates sync | in sync today (34 entries each) |
| Layout (offsets/padding) | sema.rs:6744-6807 vs codegen.rs:568, 2531-2568 | nothing | no live divergence found |
| "Does this type carry drop" | sema ty_carries_drop:3553, codegen needs_drop:9317, register_value_drop restatement:9178 | comment ("mirrors") | not yet |
| "Is this expr a place/temporary" | codegen is_place_expr:9091 ≡ method_receiver_is_place:14827 (character-identical copy) | nothing | not yet |
| StrLit→Text coercion condition | 5 inline copies + 1 helper with one caller (sema, section 1 B12) + codegen twins | comments ("lockstep") | YES: B12 holes |
| Blessed-method dispatch | sema if-chain ladder 12089-12266 (9 arms, 21 copies of the E0501 guard) + codegen name-dispatched twins | comment-ordered ("placed after X before Y") | B24 (f16 gap) |
| Method-mono expansion | 2 paths: monomorphize.rs:1973-2005 (concrete) vs 711-762 (generic-struct), ~80 lines duplicated; mixed key universes in one BTreeSet (source name vs mangled instance name, disjoint only via E0917) | comment at 720 ("does the same...") | historically (the panic that prompted the comment) |
| Propagation discovery closure | 4 copies inside one function (monomorphize.rs:1462, 1528, 1580, 1621); INSTANTIATION_LIMIT checked in 2 of 4 | none | seeding scans uncapped |
| Whole mono fixpoint | runs twice per compile: driver pre-check (1672) + monomorphize (141) on same inputs | argument-passing discipline | not yet |
| TOML parsing | manifest.rs (real parser) + 2 hand scanners in resolver.rs:1009, 1502 | none | YES: B18 |
| Import-prefix resolution | ≥6 inline copies across resolver expr arms (3045, 3097, 3107, 3149, 3186, 3245, 2547) | none | YES: B22 + two error universes + alias-facade asymmetry |
| Expression FIRST set | parser can_start_expr:4282 vs parse_primary | none | YES: B15 |
| Bracketed type-arg list parse | 10 copies (parser.rs:1902, 3052, 3105, 3639, 3739, 3785, 3824, 3873, 4150, 4171) | none | enum-ctor copies dropped named-arg support and line-dot reset |
| Generic-params parse | 2 copies (parser.rs:1579, 596) | none | span math and error strings differ |

---

## 3. Pre-borrowck residue in sema (the rework's unfinished half)

The borrowck rework moved the model, but sema still ships half the view family and encodes the
NEGATION of borrowck's coverage by hand:

- Sema emits E0513 (×19), E0515, E0516; borrowck emits E0514 (×23) + E0513 (×1). Sema skips its
  E0515 deny exactly where it believes borrowck ties (sema.rs:15511-15523:
  `current_fn_keeps_this || current_method_concrete`, `current_freefn_exported`; documented
  1388-1408, complementary claims borrowck.rs:928-940). If borrowck coverage shrinks, the result
  is NO diagnostic (silent unsoundness), not a wrong one. `method_produces_view` (15655)
  self-describes as "matching borrowck's `detect_method_view`" — a second must-agree twin.
- The capture-taint escape analysis (sema.rs:2291-2605 + E0365 at three separately-patched
  positions: return 2410, assignment 15558, call-arg 15627) is a name-string dataflow parallel
  to borrowck's model. Position-enumeration is the signature of symptom patching; a fourth
  escape position needs a fourth patch.
- Codegen's auto-clone-on-return net (10093-10101) and its `borrowed_params` feeder (6 insert
  sites, sole consumer 10095) are now dead: borrowck rejects the pattern with E0337. Verified.
- Direction: E0513/E0515/E0516 emission and lift move into borrowck; sema stops shipping view
  diagnostics; the dead codegen nets get deleted or converted to `unreachable!`.

---

## 4. Name-keyed identity where structure exists

- Stdlib Option/Iterator/Future/JoinHandle located by suffix-matching HashMap keys
  (`k.ends_with(".Option")`, sema.rs:5524-5645, 11255) — per-process NONDETERMINISTIC when two
  keys match (std HashMap iteration order); a user's generic `Option` silently shadows stdlib.
  The structured fix already exists in-file: `#[lang("string")]` → `designated_string_struct`
  (2032-2047). Extend to the other four.
- `Iterator__`/`Future__` substring dispatch in codegen (B8) — same fix: an origin flag on
  StructInfo for synthesized instantiations, exactly like `is_lang_string` (16857).
- Send/Sync markers, builtin !Send list, and `#[no_alloc]`-drop blessing keyed by bare LEAF name
  (sema.rs:4699, 3141, 3585): any struct named `Rc` anywhere is !Send; `impl Handle: Send {}` in
  package A unblocks every `Handle` in every package.
- `__cplus_` runtime prefix: format-string in sema (8278, 10903), starts_with in resolver
  (1809), no shared constant, user code can squat.
- `lookup_future_ty`/`lookup_iterator_ty` fall back to `Ty::Struct(StructId(0))` on miss
  (codegen.rs:893, 910) — whichever struct was collected first; their sibling `lookup_option_ty`
  was hardened to a loud panic (16532) without back-porting.
- Option variant tags hardcoded against stdlib source order in coroutine lowering
  (codegen.rs:16399, 16434) instead of resolved by variant name.

---

## 5. Dead code and stale docs (safe deletions)

- `TypeKind::Borrowed` is unproducible (parser constructs it nowhere) yet handled at ~25 sites
  across sema/lower/borrowck/codegen/resolver/graph; `Param.borrow_` is constant-false
  (parser.rs:1631) and threaded through 7+ guard conditions; E0511/E0512 unreachable
  (sema.rs:15076, 15140); region plumbing 15034-15087 dead. One mechanical sweep.
- `&x`/`&mut x` still parse (parser.rs:3010-3023, using the retired `mut` keyword) only to die
  in sema with "references are not yet supported (Phase 5/6)" — false "yet", v0.0.24 removed
  them by design. Replace with a targeted retired-syntax hint.
- while-let scaffold parses then unconditionally errors "(not supported in v0.0.7)"
  (parser.rs:2021-2047).
- ~110 lines of dead IfLet/GuardLet/WhileLet arms in mono (2225-2332) under a comment whose
  premise ("lower runs before monomorphize currently — we still see these") is wrong — lower
  runs before SEMA; mono can never see them. Reconstructing them would mask a pass-ordering
  regression. Make them `unreachable!`.
- Legacy `Ty::String` dual path in codegen: 51 sites, every string feature exists twice; live
  only for programs using string features without importing stdlib/text. Candidate language
  decision: require the import (str methods already gate this way), delete the arm. Related
  casualties today: B20 (ToText) and the unreachable interp arm (sema.rs:12665).
- `satisfies_bound` enum arm is dead (no (iface, enum) pair can be registered; E0325 fires
  first) and the E0325 message has swapped format args (sema.rs:4737: prints
  `impl Sized2: E` for `impl E: Sized2`).
- StructDef field `is_pub` is dead data (every read discards it; v0.0.24 moved to name-based
  privacy) yet still flows through mono instantiation.
- `return_passes_by_sret` narrow variant (codegen.rs:3369) vestigial beside `_widened`.
- Parity fossil: monomorphize.rs:682-694 keeps a value "for parity with prior shape" then
  re-fetches it from two maps with `.expect`.
- `_call_monos` dead param (monomorphize.rs:1415).
- Stale headline docs: sema.rs:3 ("Phase 1 scope: only i32 and bool" on the 32k-line file);
  monomorphize header ~90% false (claims E0502 deferred, no turbofish, generic methods
  deferred — all shipped); prune.rs:48 ("iterated to a fixed point" — it is a single
  reachability walk); parser.rs:3729 cross-references lines that moved; `check_interp_str` doc
  claims a return type it does not have.

---

## 6. Ranked plan

Tier 0 — bug fixes, small and independent (bugs before gaps; each lands with a regression test):

1. B1: route generic-fn (both branches) and generic-method args through `check_arg_with_move`
   on a substituted ParamSig. Kills B1, B11, B12's call-arg hole, and the duplicate-diagnostic
   leak in one move. This is the choke point where the NEXT per-call-form rule can't be
   forgotten (`consume_value_arg`'s own doc says it was already once meant to be "THE single
   place").
2. B2 tactical: add the `param_passes_by_ptr` loop to `gen_async_function`/`gen_gen_function`.
3. B3: stop emitting param-to-param alias-scope disjointness; keep local pairs (fresh allocas).
4. B5: run `collect_value_leaves` (already exists) over match-arm bodies instead of the
   bare-Ident sniff.
5. B4/B6: add TupleLit/InterpStr/Asm arms to mono's walkers (tactical, pending Tier 1 #1).
6. B8: origin flag on StructInfo for synthesized Iterator/Future; dispatch on it, not
   `rfind("Iterator__")`. Also backport the loud-panic fallback to
   `lookup_future_ty`/`lookup_iterator_ty`.
7. B9 tactical: accept `musttail`/`tail` in the DWARF matcher.
8. B10: delete sema's two private divergence predicates; use `lower::expr_diverges`.
9. B13/B14/B15/B16: parser statement-position block rule + one `in_delimited` ExprCtx
   combinator + `starts_expr` owned next to `parse_primary` + depth guards in
   `parse_pattern`/builder-if.
10. B17/B18/B19/B22: graph visibility on short name; resolver consumes the parsed Manifest
    (delete both hand scanners); method privacy in sema dispatch; collect all method names to
    split unknown-vs-private.
11. B20/B21/B24: ToText signature to the designated struct; `satisfies_bound` reads the blessed
    tables; add f16 to the to_text/interp table.

Tier 1 — structural moves that retire whole classes (ordered by bugs-prevented ÷ effort):

1. **Generic mutable AST traversal** (`walk_expr_mut`/`walk_stmt_mut` in ast.rs with default
   recursion); migrate mono's four walkers and lower's three. Retires the missing-arm family:
   3 ICEs this audit, 2 historical, and every future ExprKind addition currently needs 7+
   manual arm insertions to not be a bug. Effort M.
2. **One mangling module** (`mangling.rs`: render(Ty), join, take; property-test
   `take ∘ render == id`), consumed by sema/mono/codegen. Two of five printers verified
   line-identical; two already diverged. Longer term: mono records instance-name → arg-Ty
   side-tables so codegen never demangles at all (TrampolineSpec already shows the shape).
   Effort S-M.
3. **One ABI classifier** (clang-ABIInfo shape): `classify_param/classify_return → PassBy/RetBy`
   that render their own signature/call/prologue fragments; byval gate and sret string live
   INSIDE the classification result. gen_function already computes `param_abis` once
   internally — the model exists, it is just not shared. Absorbs B2, B23, the sret string ×12,
   the byval gate ×6, the bare-sret call sites ×3, and the musttail coercion predicate.
   Effort L, mechanical.
4. **One `lower_param` constructor** owning `effective_move` + a `ParamMode {Borrow, Ref, Take}`
   enum replacing the `(Ty, bool, bool, bool)` tuples (~20 destructure sites, 23 positional
   `param_attrs` calls). Effort S-M.
5. **Sema dispatch/gate unification**: resolve method calls to `(owner, sig, origin)` first,
   then one shared post-resolution gate sequence (contract, ext-scope, impl-bounds, receiver,
   args). Three shipped omissions are memorialized in comments; B1 was the fourth. Effort M-L.
6. **Lang-item registry**: extend `#[lang]` to option/iterator/future/join_handle; key markers
   and no_alloc-drop by resolved id, not leaf name. Effort M.
7. **View family into borrowck** (section 3): borrowck owns E0513/E0515/E0516 emission and
   lift; sema's hand-mirrored negation conditions are deleted; capture-taint (E0365) becomes a
   borrow class judged at frame exit. Effort L — the largest single de-drift, and the one that
   finishes the borrowck rework.
8. **Emit-time metadata**: single `emit_instr`/`emit_call` path carrying `!dbg`, alias scopes,
   sanitizer attrs; delete the three fail-open IR-text re-parsers (B9's root, B3's applicator,
   sanitizer pass). Effort M.
9. **Resolver ProgramIndex + ResolveConfig**: full (not pub-only) item/method index borrowed
   per file instead of four cloned maps; one `resolve_prefixed` helper for all expr arms;
   driver passes the parsed Manifest + platform explicitly (kills the side-effecting global
   and the `None`-vs-`Some(&[])` mode pun). Effort M.
10. **Merge the twin method-mono paths** (one `expand_generic_method` with optional Self
    target; Self becomes an ordinary subst key, deleting the partial-clone walker that caused
    B7). Effort M.

Tier 2 — hygiene, anytime:

- Dead-code sweep of section 5 (Borrowed/borrow_/regions, while-let scaffold, mono pattern-let
  arms, parity fossils, dead is_pub slot, sret narrow variant, borrowed_params net).
- Doc refresh: sema/mono/prune headers, stale cross-references, `nl_before` doc, E0325 arg swap.
- SIMD table → one const array consumed by all three readers.
- `carries_drop` bit precomputed on StructInfo/EnumInfo; disposition-aware `find_drop_flag`
  (return None for Always + debug_assert) so scanner/emitter drift fails loudly instead of
  double-freeing.
- Shared layout_of free function (sema + codegen).
- Copy/Drop: replace the fixpoint + `copy_flags_settled` + late-finalizer split with memoized
  on-demand derivation (one rule, no ordering contract).
- Field-load cache invalidation moved to the emit-call helper (currently skipped by
  indirect/assoc calls: codegen.rs:13356, 15470).
- Text→str coercion span-table: consider an explicit Coerce node at lower/mono instead of
  span-keyed lookup + suppress cell (span-keyed side tables are fragile against synthesized
  nodes; 11 such tables exist — worth a NodeId before the next one).

What is healthy (checked, leave alone): borrowck (nothing found, including by the auditors
told not to look at it whose scopes touched its boundaries), prune.rs (one stale doc line),
lexer.rs (clean single-pass; `nl_before` is the right foundation for the parser fix), the
fastcc def/call symmetry (the ONE ABI decision that is centralized, via `md.fastcc_funcs`),
error recovery (uniformly fail-fast by design — revisit only when the LSP needs multi-error
parses), and the deliberate name-rederivation seam between sema and codegen (no findings
against the design itself; the findings are about ad-hoc SUBSTRING parsing of mangled names,
which the design does not require).

---

## 7. Repro artifacts

Session scratchpad (`/private/tmp/claude-501/-Users-adel-Workspace-C-/58feb0bb-5507-44bf-a56b-e5d5a5e8525f/scratchpad/`):

- `vfy/refhole.cplus` (B1, exits 99), `vfy/frozen.cplus` (concrete control, E0328)
- `arp_proj/` (B2, prints garbage + lost write-back), `alias_probe.cplus` (B3, 23 vs 20)
- `monoaudit/tup.cplus` (B4), `monoaudit/proj/` (B6), `monoaudit/selfloop.cplus` + `selfok.cplus` (B7 + control)
- `divtest/m3.cplus` (B10), `divtest/m5.cplus` (B5), `divtest/g1.cplus` (B11)
- `hijack/main.cplus` (B8), `hijack/mt.cplus` (B9), `hijack/f16gap.cplus` (B24)
- `probe/t1-t10, proj/` (B11, B12, B20, B21 probes), `repro/t1-t15.cplus` (B13-B16)
- `extref.cplus` (B23), `sret_probe.cplus` (sret asymmetry IR), `tomltest/`, `privtest/`, `graphtest/` (B17-B19, B22)

Scratchpad is session-scoped; promote whichever repros become regression tests before it is
cleaned up. The per-item reports below inline every repro, so nothing is lost when the
scratchpad goes.

---

## 8. Per-item report index

Each bug/issue has a self-contained implementation report in this directory, written so a
fixer needs only the report plus repo access. Bugs carry inline repros; issues carry
verified site checklists and ordered migration plans.

Bugs (B-numbers above map 1:1; two extra cases were promoted to reports):
`bug-01-generic-ref-skips-e0328.md`, `bug-02-async-gen-fn-ref-abi.md`,
`bug-03-alias-scope-metadata-release-miscompile.md`, `bug-04-mono-tuple-literal-ice.md`,
`bug-05-match-arm-block-escape-double-free.md`, `bug-06-mono-interp-string-ice.md`,
`bug-07-mono-self-walker-ice.md`, `bug-08-iterator-name-hijack.md`,
`bug-09-debug-info-lost-musttail.md`, `bug-10-check-if-divergence-spurious-e0302.md`,
`bug-11-generic-method-inference-double-check-e0335.md`,
`bug-12-strlit-text-coercion-holes.md`, `bug-13-parser-block-statement-boundary.md`,
`bug-14-parser-no-struct-lit-leak.md`, `bug-15-parser-can-start-expr-drift.md`,
`bug-16-parser-depth-guard-holes.md`, `bug-17-graph-is-pub-wrong.md`,
`bug-18-resolver-toml-comment-in-symbols.md`,
`bug-19-method-privacy-receiver-spelling.md`, `bug-20-totext-interface-impossible.md`,
`bug-21-primitive-bounds-vs-dispatch.md`, `bug-22-unknown-method-reported-private.md`,
`bug-23-extern-declare-ref-mismatch.md`, `bug-24-f16-to-text-gap.md`,
`bug-25-literal-patterns-unsupported.md` (long-open gap, root cause located),
`bug-26-interface-attrs-silently-ignored.md` (attrs audit F6).

Issues (Tier 1 items 1-10 map to issue-01..10; Tier 2 clusters follow):
`issue-01-generic-ast-walker.md`, `issue-02-mangling-module.md`,
`issue-03-abi-classifier.md`, `issue-04-param-mode-lower-param.md`,
`issue-05-sema-call-gate-unification.md`, `issue-06-lang-item-registry.md`,
`issue-07-view-family-into-borrowck.md`, `issue-08-emit-time-metadata.md`,
`issue-09-resolver-program-index.md`, `issue-10-merge-method-mono-paths.md`,
`issue-11-dead-code-sweep.md`, `issue-12-stale-docs-refresh.md`,
`issue-13-drop-move-single-sourcing.md`, `issue-14-copy-drop-memoized.md`,
`issue-15-shared-tables-simd-layout.md`, `issue-16-span-keyed-side-tables.md`,
`issue-17-parser-shared-helpers.md`.

Suggested order for a fixer working the list: bugs 01-05 and 08 first (soundness and
miscompiles), then the remaining bugs, then issues in number order — except land
issue-04 before issue-03, and bug-03 before issue-08, as noted inside those reports.
One additional small recorded item without its own report: sema's raw-pointer
accountability lint counts ANY call taking the pointer field as a release
(`expr_releases`, sema.rs:6478-6481) — `log(this.ptr)` satisfies the "must free" lint;
recognize release via a consumes-pointer callee set instead (checking audit F13).
