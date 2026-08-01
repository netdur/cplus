# Issue 03 — One ABI classifier (clang-ABIInfo shape) for all def/call/declare sites

- Type: structural consolidation
- Area: `cplus-core/src/codegen.rs` (new submodule or top section)
- Effort: L (mechanical but wide)
- Retires / prevents: bug-02, bug-23; the Xtensa byval drift class; the msgSend-sret
  clobber class; the bare-sret call-site asymmetry; the musttail predicate divergence
- Master report: `core-drift-audit-2026-08-01.md` (§6 Tier 1 #3, §2 ABI row)

## Problem

"How is this parameter/return passed" is decided at ~15 independent parameter sites and
~10 return sites, kept aligned by comments ("Must match the def-side gate in
gen_function", "Mirror of the fn-definition return rule"). The audit found two live
divergences (async/gen fn defs — bug-02; extern-import declares — bug-23), and the
project history records two more ABI breaks of the same class (Xtensa byval def/call
drift, recorded at codegen.rs:3361-3364; objc_msgSend sret clobber). Every new emitter
family re-derives the rules and eventually forgets one.

## Current state (the fixer's checklist)

Definition emitters (9): gen_function 6032, gen_async_method 6734, gen_gen_method 6969,
gen_gen_function 7187, gen_async_function 7325, gen_enum_method 7540, gen_gen_enum_method
7728, gen_method 7897, gen_str_method 8343.

Call-site families: gen_named_call 13673 (params) + 13863 (returns), gen_indirect_call
13380 + 13476, method calls 15279 (struct) / 15078 (str) / 15398 (enum), trampolines
tramp_input_abi 622 / tramp_ret_abi 591-616, msg_send 11857 + 11908, extern-import
declare 6161, extern-export path ~6420.

Convention-enforced sub-invariants to absorb INTO the classification result:

- byval gate: `x86_64_indirect_uses_byval()` (3351-3366) consulted at 6 sites (758, 6177,
  6446, 11876, 13429, 13752); one past drift recorded in its own doc.
- sret attribute string hand-built ~12x (689, 780, 6153, 6399, 7613, 8066, 8402, 11926,
  13486, 13528, 13904, 13953) — includes `writable`, which must stay.
- Bare-sret call sites: struct-method 15333, str-method 15114, enum-method 15440 emit
  `ptr {slot}` with no attrs — works today only via LLVM's direct-call callee-attr
  fallback, the exact fallback whose absence caused the msgSend clobber (the comment at
  13935-13948 even says the call site "MUST" carry the attrs).
- musttail return-coercion predicate at 9989-9993 is a third, divergent copy of the
  `want_c_abi_ret` gate (real one at 13870, mirrored at 6234).
- fastcc: `md.fastcc_funcs` is the ONE decision already centralized (keep it); but the
  method arm at 1494-1501 skips the address-taken check the fn arm does (1474-1483),
  justified only by a comment ("methods aren't address-takeable", 5103-5105) — add an
  assert.
- Borrow-ABI inconsistency across families: bare non-Copy STRUCT borrows pointer-pass
  with `readonly` (gate at 2846) while non-Copy ENUM and Text borrows pass as raw
  aggregate copies (def and call agree, so sound, but two ABIs for one semantic class;
  Text's copy needs the `borrowed_params` compensation net — see issue-11 item 10).
  Decide once inside the classifier; extending 2846 to Enum/String is the cleaner end
  state but needs its own test pass.

## Target design

```rust
pub enum PassBy { Value { attrs: ParamAttrs }, Ptr { attrs: ParamAttrs },
                  Indirect { byval_align: Option<u64> }, /* C-ABI coerced forms */ }
pub enum RetBy  { Value, Sret { ty: String, align: u64 }, Coerced { .. }, Void }

fn classify_param(ty: &Ty, mode: ParamMode, fnk: FnKind, t: &Target) -> PassBy;
fn classify_return(ty: &Ty, fnk: FnKind, t: &Target) -> RetBy;

impl PassBy { fn sig_fragment(..), fn call_fragment(..), fn prologue_bind(..) }
impl RetBy  { fn sig_fragment(..), fn call_fragment(..)  /* full sret attr string */ }
```

Key property: the byval alignment and the sret attribute string are INSIDE the
classification result — a call site cannot forget them. `FnKind` distinguishes
native/fastcc vs extern/C-ABI vs msg_send so the Copy-struct C-ABI unification applies
exactly where it does today.

Note: gen_function already computes `param_abis` once and has both its signature and
prologue consume it — the model exists in-file; this issue extracts and shares it.

## Migration plan (each step green before the next)

1. Extract the classifier from gen_function's internal logic; adopt in gen_function
   itself (no behavior change; IR-text unit tests are the harness).
2. Adopt in the two broken emitters — fixes bug-02.
3. Adopt in extern declare/export — fixes bug-23.
4. Adopt in gen_named_call + gen_indirect_call; delete the local mirrors and the
   musttail predicate copy (share the real one).
5. Adopt in the three method-call sret sites (full attr string appears — deliberate IR
   change, update tests).
6. Adopt in tramps + msg_send; fold the 6 byval sites; delete
   `x86_64_indirect_uses_byval` direct calls outside the classifier.
7. Add the fastcc method-arm assert.
8. (Separate decision commit) Enum/Text borrow unification per above, or an explicit
   comment in the classifier recording why they stay aggregate-copied.

## Verification

- After every step: `cargo test -p cplus-core && cargo test -p cpc --test e2e`; the
  codegen IR-text tests will churn at steps 5-6 — update deliberately, never loosen.
- C ABI ground truth: the repo's practice is verifying struct-passing against clang;
  re-run whatever C-interop e2e tests exist (grep e2e for extern/C tests).
- Cross-platform gates: build for the targets CI covers (macOS + the Win64/Xtensa gate
  logic is target-flag driven — the classifier centralizes it; test with the target
  override flags if available).

## Risks and constraints

- Strict C ABI compatibility with clang is a hard requirement; def+call symmetry must
  hold at every intermediate commit — hence the one-family-per-step plan.
- Do not change WHICH convention any signature uses in steps 1-7 (pure consolidation);
  step 8 is the only behavior change and is optional.
