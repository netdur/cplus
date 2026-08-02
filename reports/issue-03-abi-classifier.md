# Issue 03 — One ABI classifier (clang-ABIInfo shape) for all def/call/declare sites

- Status: PARTIAL 2026-08-02, commit <pending> — steps 1, 2, 3, 5, 6, 7, 8 done;
  step 4 (the call-site parameter families) not done, see "What is still open"
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

## Outcome

```rust
struct AbiCtx { c_export: bool, coerce_copy_aggregates: bool }

enum PassBy {
    Value { llvm_ty: String, attrs: String, ext: &'static str },
    Ptr { attrs: String },
    Indirect { ty: String },              // byval or bare ptr, already decided
    Coerced { llvm_ty: String, size: u64, align: u64 },
}

fn classify_param(p: &ParamAbi, cx: AbiCtx, types: &TypeTable) -> PassBy;
impl PassBy { fn sig_fragment(&self, idx: u32) -> String }

fn sret_fragment_for(inner: &str, size: u64, align: u64, name: &str) -> String;
fn sret_fragment(ty: &Ty, types: &TypeTable, name: &str) -> String;
fn indirect_arg_ty(inner: &str, align: u64) -> String;
fn call_wants_c_abi_ret(sig: &FnSig, symbol: &str, md: &ModuleMetadata, types: &TypeTable) -> bool;
```

The report's key property holds: the byval decision and the sret attribute
string are inside the shared result, so a site cannot forget them.

**Step 1 + 2 — the nine definition emitters.** `gen_function`,
`gen_async_method`, `gen_gen_method`, `gen_gen_function`, `gen_async_function`,
`gen_enum_method`, `gen_gen_enum_method`, `gen_method`, `gen_str_method` all
emit their parameter list as `classify_param(..).sig_fragment(idx)`. The ten-line
by_ptr/attrs/base_ty block that had been copied into eight of them is gone.
`gen_function`'s prologue binds off the same `PassBy` values its signature was
written from, so the two cannot disagree about which parameter is a pointer.

`AbiCtx` is where the per-emitter difference now lives, and writing it down is
half the value: free functions apply the C-ABI classification to Copy
aggregates, `fastcc` functions and every method emitter do not. That was
previously implicit in which emitter you happened to be reading.

**Step 3 — extern declare/export.** The declaration path shares `gen_function`'s
signature emission (it already did) and now its sret fragment too.

**Step 5 — the bare-sret call sites, a deliberate IR change.** The four method /
str-method / enum-method call sites that emitted `ptr %slot` with no attributes
now emit the same attributed fragment the callee's definition declares. They
worked only through LLVM's direct-call callee-attribute fallback — the fallback
whose absence produced the `objc_msgSend` receiver clobber. Pinned by the new
codegen test `a_method_sret_call_site_carries_the_full_attribute_set`. This is
the only IR difference the whole issue produces: five of the 40 `docs/examples`
programs change exactly one line each, all of the form
`call fastcc void @T.m(ptr %slot)` → `... (ptr sret(%T) noalias nonnull noundef
writable dereferenceable(N) align A %slot)`.

**Step 6 — byval.** All six indirect-lowering sites (fn definition, extern
declaration, native call, fn-pointer call, thread trampoline, `#msg_send`) build
their argument type with `indirect_arg_ty`. `indirect_uses_byval()` has no
callers outside it and the classifier.

Every one of the twelve hand-built sret attribute strings now comes from
`sret_fragment_for`, including the two thread trampolines, `#msg_send`, the
musttail forwarding path and both fn-pointer call paths.

**Step 7 — the fastcc method arm.** `debug_assert!` that a method name is never
in the address-taken set, replacing the comment that asserted it in prose. (The
invariant holds because a bound method reference goes through a synthesized
free-fn bridge, which the free-fn arm covers.)

**Step 8 — the borrow-ABI decision.** Recorded in `classify_param`'s doc rather
than changed: a bare non-Copy STRUCT is pointer-passed with `readonly`, while a
non-Copy enum or a `Text` is copied as a raw aggregate; both ends agree, so it
is sound, and the aggregate copy is what makes `Text` need the
`borrowed_params` net. Unifying changes emitted signatures and needs its own
test pass.

### A correction to the report

The report lists the musttail return-coercion predicate (`callee_ret_coerced`)
as "a third, divergent copy of the `want_c_abi_ret` gate". It is a third copy,
but it is not divergent: it asks only about an EXTERN callee, and the
non-extern half of the gate is already covered two lines above by
`enclosing_ret_coerced` — the predicate requires the callee's return type to
equal the enclosing fn's and their calling conventions to match, so a
coerced-return callee implies a coerced-return caller. The gate is now a named
function (`call_wants_c_abi_ret`) shared by the call-site return path, and the
musttail predicate keeps its narrower question with the reasoning written down.
Widening it would only lose valid tail calls.

## What is still open

Step 4 — adopting the classifier in `gen_named_call` and `gen_indirect_call`'s
PARAMETER paths — is not done. The TYPE half of those sites is already shared
(`indirect_arg_ty` for byval, `sret_fragment` for the return slot); what
remains is the value half, where each call site materializes an argument
(alloca, store, coerced load) before naming its type. That needs `PassBy` to
grow a `call_fragment` that can emit instructions, which is a different shape
from the pure-text `sig_fragment` and a larger change than the rest of this
issue combined. The def/call symmetry that broke in the past (Xtensa byval, the
sret attribute set) is now shared; what is left is the argument-materialization
code, which has never been the thing that drifted.

## Verification (as run)

- IR byte-identity after steps 1, 2, 3, 6, 7 and the shared return gate:
  `--emit-ll` over all 40 `docs/examples` plus three purpose-built probes (an
  ABI probe covering by-ref write-back, `take`, non-Copy borrow, Copy struct,
  `restrict`, all three receivers and an extern import; a C-ABI probe covering
  8/16/24-byte `#[repr(C)]` struct params and returns, an HFA, narrow-int
  extension and a C out-parameter; a method/enum probe) — identical.
- Step 5's IR change isolated to the five example programs named above, with a
  new codegen test pinning both ends of the attribute string.
- `cargo test -p cplus-core` 1845 + 8, `cargo test -p cpc` 605 + 16 + 5 + 6;
  `cpc test` in `vendor/stdlib` 290 green in debug and `--release`; vendor-wide
  `cpc check` parity across 54 packages — no change.
