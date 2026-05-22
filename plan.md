# C+ — Plan

Version 0.0.8 shipped 2026-05-22. See [plan-0.0.8.md](plan-0.0.8.md) for the archived 0.0.8 roadmap and resolved log; [plan-0.0.7.md](plan-0.0.7.md) covers v0.0.7, [plan-0.0.6.md](plan-0.0.6.md) v0.0.6, [plan-0.0.5.md](plan-0.0.5.md) v0.0.5, [plan-0.0.4.md](plan-0.0.4.md) v0.0.4, [plan-0.0.3.md](plan-0.0.3.md) v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.9 — Tighten the safety story, close the long-tail bugs

**Strategy:** v0.0.8 validated the v0.0.7 surface against three real benchmarks and closed the bench-gap punch list. C+ now wins the raytracer outright (0.94 s vs C's 1.16 s) on Apple Silicon, the SIMD surface has a packaged consumer (`vendor/simd`), and the macro-builtin trilogy is settled (`include_bytes!` / `include_str!` / `env!`).

What's left is the long tail: small bugs that surfaced under real workloads (mixed-if-arm panic, lingering musttail edge cases), one safety footgun that contradicts the "safety as default" pitch (`fn echo(x: string) -> string` is a silent double-free without an explicit `move`), and one ergonomic gap that touches every byte-level program (no character literals).

No new principles. The locked twelve from §1.Locked-Principles stand. v0.0.9 is polish + correctness, not language reshape.

Slice sizes follow the same S/M/L assistant-paced framing.

---

### Phase 1 — Safety: default-move for non-Copy value params · size M

**The footgun**, lifted from the v0.0.8 raytracer port and from a user-flagged note in the closed plan:

```cplus
// ❌ Caller's `s` and callee's `x` both run Drop. Double-free under ASan.
fn echo(x: string) -> string { return x; }

// ✅ Today's workaround — manually mark the param `move`.
fn echo(move x: string) -> string { return x; }
```

A safety-first language should not silently emit a double-free when the user writes the "obvious" signature. The current model — value-passed non-Copy = "shared borrow that aliases the caller's heap" — was a v0.0.5 expedient and never stress-tested against a real workload until the raytracer port.

**Goal:** for a non-`Copy` value-typed parameter without `move`, default to **move** semantics (single owner: the callee). The current "borrow" interpretation requires the caller's writer to opt in via `borrow` or stays as the explicit `mut x: T` non-Copy pointer ABI.

**Locked design decisions:**

1. **Backwards compatibility break** — every `fn f(x: NonCopyT)` in the wild gets the new semantics. We accept this; the codebase is small enough to migrate, and the alternative (keep the footgun forever) violates the principle that motivated the borrow checker in the first place.
2. **`mut x: T` pointer-pass ABI is unchanged.** Exclusive borrow keeps the §2.9 shape.
3. **`borrow x: T`** becomes the explicit shared-borrow form (new keyword). Today's `x: T` shape lowers to it for source migration: parser warning E0X12 ("`x: T` for non-Copy T defaulted to `move` in v0.0.9; previously `borrow`. Add `borrow` explicitly to keep the old semantics."), at least for one release cycle.
4. **`echo` worked example from §12 of the tutorial flips** — the move marker becomes default, the comment moves to "explicit borrow if you want sharing."

**Scope:**

- Sema: change the default ownership interpretation. The `move`/`borrow` markers stay; the silent case flips.
- Codegen: existing `move_flag` plumbing is reused; the call-site Drop flag flip + the param-binding Drop registration already work — they just fire by default now.
- Migration warning E0X12 for the breaking-change interpretation.
- Tutorial §12 rewrite (the "Use `move v: T` for non-Copy value parameters" gotcha at §30 disappears).
- E2E sweep: every `fn ... (x: T) ... { return x; }` in stdlib / vendor / docs / proves needs review. Estimate ~30-50 sites; mechanical rewrite (add `move` where the old behavior was relied on, or accept the new default).

**Tests:** unit (the parser warning + sema-pass for both forms) + e2e (ASan run of the `echo` shape — must be clean under default semantics).

**Expected payoff:** the §1 principle "safety as default" stops having a load-bearing exception. The "you must remember `move`" sentence leaves the tutorial.

---

### Phase 2 — Character literals · size S

**The gap:** `'a'` doesn't parse today. ASCII bytes are written as `65u8`, which is fine for codegen but reads like assembly. Every C+ program that touches the byte alphabet (JSON / CSV / network protocol parsers) has `b'{' as u8 = 123` style comments or magic-number bytes scattered through it.

**Locked design decisions:**

1. **Syntax:** `'a'` for a single ASCII byte, type `u8`. Direct lower to the byte value as an `i8` immediate.
2. **No multi-char.** `'ab'` is a parse error (E0X20 "character literal must be exactly one byte").
3. **Escapes:** the same backslash escapes the string literal accepts — `'\n'` `'\t'` `'\\'` `'\''` `'\0'` and `'\xHH'`. UTF-8 multi-byte code points are rejected at parse time (E0X21 "character literal must be a single byte; for UTF-8 use a `str`").
4. **Type:** `u8`. Pattern-matching against `'a'` in a `match arm` matches a `u8`. Not a separate `char` type — C+ doesn't have one and won't grow one.
5. **Why now:** the JSON tokenizer + raytracer + future bytes-level workloads all have this scattered through them. One token cuts the magic-number noise in half.

**Scope:**

- Lexer: tokenize `'...'` into `TokenKind::CharLit(u8)`.
- Parser: route to `ExprKind::IntLit(byte as u64, NumSuffix::U8)`. AST stays minimal — no new variant.
- Sema: untouched (the existing `u8` literal path handles it).
- Tutorial §3 mentions the new literal; §30 "no character literals in v0.0.4" gotcha is removed.

**Tests:** unit (positive: every escape; negative: empty literal, multi-byte, UTF-8 codepoint).

**Expected payoff:** small but symbolic — closes one of the original gotchas in `Tutorial > Gotchas worth memorising`.

---

### Phase 3 — Long-tail codegen bug fixes · size S

bench.md and the v0.0.8 close surfaced two cpc bugs that didn't make it into a slice-shaped fix:

1. **Mixed-if-arm panic** — when one `if` arm is a simple call and the other is a block with internal `let` + tail expr, codegen still panics on some shapes. v0.0.8 finding 3 fixed one shape; another remains. Workaround: `let mut` + conditional assign. The bench.md raytracer has at least one of these in source — a perf tax (lost branch-elimination on the `if`-as-expression form).

2. **musttail predicate edge cases** — v0.0.8 closed the nested-arg-steals-flag bug. Other shapes may still slip through (e.g. recursive tail-call where the recursive call carries a `move`-marked arg, which currently has its own drop-flag-flip code path running between the call and the `ret`). Audit with a fuzzer / property test.

**Scope:**

- Minimum repro for each, added to `cplus-core/src/codegen.rs` test module.
- Tighten the predicates; remove the source-level workarounds in `bench-cplus/raytracer/cplus/main.cplus` once each fix lands.

**Tests:** pin per bug + a regression suite e2e.

**Expected payoff:** removes "workaround tax" lines from real code. Bench-cplus has a few labeled `// workaround for cpc bug` comments — each closure is one line of source that gets to drop.

---

### Phase 4 — Threaded raytracer · size M (deferred from v0.0.8)

**Goal:** parallel-tiles raytracer. Each thread renders one horizontal band of the image, joins, then `main` writes the assembled buffer. v0.0.5 shipped `thread::spawn` / `JoinHandle::join`; v0.0.8's raytracer ran it single-threaded. This slice exercises the v0.0.5 thread surface against a real workload and gives the bench.md raytracer benchmark a 4-8× headroom on multi-core machines.

**Locked design decisions:**

1. **No work stealing.** Static tile partition (rows 0..N/T per thread for N rows, T threads). v0.1+ design.
2. **Output buffer is pre-allocated; each thread writes its tile.** No shared writes to a `Mutex` — disjoint rows per thread is the invariant the borrow-checker doesn't yet verify, but `restrict` on the pointer + thread-local row ranges keeps it sound by construction.
3. **Thread count:** detect via libc `sysconf(_SC_NPROCESSORS_ONLN)` or hardcode to 4 for the bench. Caller-overridable via `env!("RAYTRACE_THREADS")` (v0.0.8 Phase 4's env! makes this clean).
4. **Same image hash as single-threaded.** The bench-cplus `cplus/main.cplus` uses a deterministic per-pixel RNG seed, so re-tiling work across threads produces identical bytes if and only if each pixel's RNG state is deterministic from the pixel coordinates (not from a serial RNG advance). The single-threaded path already has this property; the multi-threaded path keeps it.

**Tests:** unit (thread join correctness, already in stdlib tests) + e2e (raytracer wall-time + identical image hash to single-threaded).

**Expected payoff:** the raytracer bench gets a multi-core column. The v0.0.5 thread surface validated against a real shared workload.

---

## Phase ordering rationale

- **Phase 1 first.** It's a backwards-compat break + a tutorial rewrite; doing it early lets every other v0.0.9 source change land under the new default. Doing it late means rewriting Phase 2/3 sources twice.
- **Phase 2 alongside Phase 1.** Independent code path (lexer + parser); doesn't conflict with Phase 1 sema work. Can ship together.
- **Phase 3 after Phase 1/2.** The bug repros may overlap with the new sema rules; want the new defaults in place first so the repros are clean.
- **Phase 4 last.** Needs the bench harness updated (multi-thread column in bench.sh / bench.md). Lower priority than the bug closures.

Estimated effort across all phases: ~4-5 sessions aggregate. v0.0.9 ship target: 3 of those sessions if Phase 4 slips; full 4 if everything goes clean.

---

## Open questions (do not block phase work)

- **Per-field TBAA tree** — v0.0.7 Slice 1.2 punted with "ship when raytracer perf measures the win." The v0.0.8 raytracer is at 0.94 s, ahead of C; not a clear win available here. The work sits unless a workload (gemm / image processing) makes it measurable.
- **Mask types as a distinct `Ty` variant** — v0.0.7 Slice 2.1 aliased `mask32x4` to `i32x4`. No bug has surfaced from the aliasing in v0.0.8's `vendor/simd` consumer. Stays as-is until one does.
- **Submodule re-export through `appkit/appkit` facade for functions** — re-litigated three cycles in a row. No second bindings package in v0.0.9 (sqlite was dropped); rubric decision waits for one to land.
- **`#[align(N)]` for struct fields** — v0.0.6 cut, v0.0.7 deferred, v0.0.8 Phase 1B didn't surface a need (the SIMD raytracer worked on `f32x4` directly with no alignment trap). Stays cut until a real consumer hits a misalignment.
- **`option_env!()`** — explicitly rejected in Phase 4. If a workload genuinely needs "optional build-time config" and the empty-string sentinel pattern isn't enough, revisit.
- **`borrow` keyword reservation** — Phase 1 introduces it. Lexer already reserves the token (v0.0.5 borrow regions); v0.0.9 widens the use to value-typed params. No new lexer work.
